//! Web forms: public forms that create CRM records.
//!
//! A form is shown on its hosted page (`/f/{key}`) or embedded on any website (an iframe of
//! that page, sized by a small script). A submission creates a lead, or, for types linked to
//! a client (deals, smart processes), the record plus its contact and company, matching
//! existing contacts by email or phone. Spam protection: a hidden decoy field, a signed
//! "form shown at" token with a minimum fill time, and rate limits per visitor and per form.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    access::is_crm_admin,
    automation::{fire, system_auth},
    broadcast, entity_type_by_id,
    fields::custom_fields,
    items::{create_item, normalize},
    timeline, CONTACT, COMPANY, LEAD,
};
use crate::{
    auth::{tokens::random_token, AuthUser, ClientIp},
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/webforms", get(list).post(create))
        .route("/crm/webforms/targets", get(targets))
        .route("/crm/webforms/{id}", get(get_form).patch(update).delete(remove))
        .route("/crm/webforms/{id}/submissions", get(submissions))
        .route("/public/forms/{key}", get(public_form))
        .route("/public/forms/{key}/submit", post(submit))
}

const KINDS: &[&str] = &["text", "textarea", "email", "phone", "number", "date", "select", "checkbox"];
/// Minimum seconds between showing a form and submitting it (bots are faster).
const MIN_FILL_SECS: i64 = 2;

#[derive(Serialize, Deserialize, Clone)]
pub struct Field {
    /// Assigned by the server when missing.
    #[serde(default)]
    key: String,
    label: String,
    kind: String,
    #[serde(default)]
    required: bool,
    /// Where the value goes; empty = into the record's comment.
    #[serde(default)]
    maps_to: String,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    placeholder: String,
}

#[derive(Serialize, FromRow)]
pub struct Form {
    id: Uuid,
    key: String,
    name: String,
    entity_type: i16,
    category_id: Option<Uuid>,
    stage_id: Option<Uuid>,
    fields: Value,
    settings: Value,
    responsible: Vec<Uuid>,
    reply_mailbox: Option<Uuid>,
    reply_subject: String,
    reply_body: String,
    is_active: bool,
    created_by: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[sqlx(skip)]
    submissions: i64,
}

const COLUMNS: &str = "id, key, name, entity_type, category_id, stage_id, fields, settings, responsible, reply_mailbox, reply_subject, reply_body,
    is_active, created_by, created_at, updated_at";

fn require_admin(auth: &AuthUser) -> AppResult<()> {
    if is_crm_admin(auth) { Ok(()) } else { Err(AppError::Forbidden) }
}

async fn load(state: &AppState, id: Uuid) -> AppResult<Form> {
    let mut f: Form = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLUMNS} FROM crm_webforms WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("form"))?;
    f.submissions = sqlx::query_scalar("SELECT COUNT(*) FROM crm_webform_submissions WHERE form_id = $1").bind(id).fetch_one(&state.db).await?;
    Ok(f)
}

// ---------------------------------------------------------------------------
// Mappable fields
// ---------------------------------------------------------------------------

/// Where form fields can go, per target type: (code, label).
async fn mappable(state: &AppState, entity_type: i16) -> AppResult<Vec<(String, String)>> {
    let t = entity_type_by_id(state, entity_type).await?;
    let mut out: Vec<(String, String)> = Vec::new();
    let push = |out: &mut Vec<(String, String)>, c: &str, l: &str| out.push((c.to_string(), l.to_string()));
    if t.id == LEAD {
        for (c, l) in [("first_name", "First name"), ("last_name", "Last name"), ("company_name", "Company name"), ("email", "Email"), ("phone", "Phone"), ("title", "Lead name")] {
            push(&mut out, c, l);
        }
    } else {
        for (c, l) in [("contact.first_name", "Contact: first name"), ("contact.last_name", "Contact: last name"), ("contact.email", "Contact: email"), ("contact.phone", "Contact: phone"), ("company.name", "Company name"), ("title", "Title")] {
            push(&mut out, c, l);
        }
        if t.has_amount {
            push(&mut out, "opportunity", "Amount");
        }
    }
    push(&mut out, "comments", "Comment");
    for f in custom_fields(state, t.id).await? {
        if ["string", "text", "number", "date", "boolean", "list", "url", "money"].contains(&f.kind.as_str()) {
            out.push((format!("uf.{}", f.code), f.label));
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
struct TargetsQuery {
    entity_type: i16,
}

async fn targets(State(state): State<AppState>, auth: AuthUser, Query(q): Query<TargetsQuery>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let list = mappable(&state, q.entity_type).await?;
    Ok(Json(json!({ "items": list.iter().map(|(c, l)| json!({ "code": c, "label": l })).collect::<Vec<_>>() })))
}

// ---------------------------------------------------------------------------
// Admin API
// ---------------------------------------------------------------------------

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let rows: Vec<(Uuid, String, String, i16, bool, i64, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT f.id, f.key, f.name, f.entity_type, f.is_active,
             (SELECT COUNT(*) FROM crm_webform_submissions s WHERE s.form_id = f.id),
             (SELECT MAX(s.created_at) FROM crm_webform_submissions s WHERE s.form_id = f.id)
         FROM crm_webforms f ORDER BY f.name",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "items": rows.iter().map(|(id, key, name, et, active, n, last)| json!({
            "id": id, "key": key, "name": name, "entity_type": et, "is_active": active, "submissions": n, "last_submission_at": last,
        })).collect::<Vec<_>>(),
        "public_base": format!("{}/f/", state.config.public_url),
    })))
}

async fn get_form(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Form>> {
    require_admin(&auth)?;
    Ok(Json(load(&state, id).await?))
}

#[derive(Deserialize)]
struct Body {
    name: Option<String>,
    entity_type: Option<i16>,
    category_id: Option<Uuid>,
    stage_id: Option<Uuid>,
    fields: Option<Vec<Field>>,
    settings: Option<Map<String, Value>>,
    responsible: Option<Vec<Uuid>>,
    reply_mailbox: Option<Uuid>,
    reply_subject: Option<String>,
    reply_body: Option<String>,
    is_active: Option<bool>,
}

async fn check_fields(state: &AppState, entity_type: i16, fields: Vec<Field>) -> AppResult<Value> {
    if fields.is_empty() || fields.len() > 50 {
        return Err(AppError::bad("a form has 1 to 50 fields"));
    }
    let targets: Vec<String> = mappable(state, entity_type).await?.into_iter().map(|(c, _)| c).collect();
    let mut keys = Vec::new();
    let mut out = Vec::new();
    for (i, mut f) in fields.into_iter().enumerate() {
        f.label = required("field label", &f.label, 200)?;
        if !KINDS.contains(&f.kind.as_str()) {
            return Err(AppError::bad(format!("field type must be one of {}", KINDS.join(", "))));
        }
        if !f.maps_to.is_empty() && !targets.contains(&f.maps_to) {
            return Err(AppError::bad(format!("“{}” cannot be saved into {}", f.label, f.maps_to)));
        }
        if f.kind == "select" {
            f.options = f.options.into_iter().map(|o| o.trim().chars().take(200).collect::<String>()).filter(|o| !o.is_empty()).take(100).collect();
            if f.options.is_empty() {
                return Err(AppError::bad(format!("“{}” needs options", f.label)));
            }
        } else {
            f.options.clear();
        }
        // Stable keys: keep the given one if it is clean and unique, else derive one.
        let clean = f.key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !f.key.is_empty() && f.key.len() <= 40;
        if !clean || keys.contains(&f.key) {
            f.key = format!("f{}", i + 1);
            while keys.contains(&f.key) {
                f.key.push('_');
            }
        }
        keys.push(f.key.clone());
        f.placeholder = f.placeholder.chars().take(200).collect();
        out.push(f);
    }
    Ok(serde_json::to_value(out).unwrap())
}

fn check_settings(s: Map<String, Value>) -> AppResult<Value> {
    let mut out = Map::new();
    for (k, max) in [("title", 200), ("description", 2000), ("button", 60), ("success_message", 1000), ("redirect_url", 1000), ("color", 7), ("consent_text", 2000)] {
        if let Some(v) = s.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()) {
            if v.chars().count() > max {
                return Err(AppError::bad(format!("{k} is too long")));
            }
            if k == "redirect_url" && !(v.starts_with("https://") || v.starts_with("http://")) {
                return Err(AppError::bad("the redirect must be an http(s) address"));
            }
            if k == "color" && !(v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit())) {
                return Err(AppError::bad("colour must look like #2563eb"));
            }
            out.insert(k.into(), json!(v));
        }
    }
    Ok(Value::Object(out))
}

async fn check_target(state: &AppState, entity_type: i16, category: Option<Uuid>, stage: Option<Uuid>) -> AppResult<()> {
    let t = entity_type_by_id(state, entity_type).await?;
    if !(t.id == LEAD || t.has_client) {
        return Err(AppError::bad("forms create leads, or records linked to a client (deals, smart processes)"));
    }
    if let Some(c) = category {
        let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_categories WHERE id = $1 AND entity_type = $2)").bind(c).bind(t.id).fetch_one(&state.db).await?;
        if !ok {
            return Err(AppError::bad("that pipeline is not of this record type"));
        }
    }
    if let Some(s) = stage {
        let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_stages s JOIN crm_categories c ON c.id = s.category_id WHERE s.id = $1 AND c.entity_type = $2 AND ($3::uuid IS NULL OR c.id = $3))")
            .bind(s)
            .bind(t.id)
            .bind(category)
            .fetch_one(&state.db)
            .await?;
        if !ok {
            return Err(AppError::bad("that stage is not in the chosen pipeline"));
        }
    }
    Ok(())
}

async fn check_people(state: &AppState, ids: &[Uuid]) -> AppResult<Vec<Uuid>> {
    let ok: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND is_active AND NOT is_system").bind(ids).fetch_one(&state.db).await?;
    if ok as usize != ids.len() {
        return Err(AppError::bad("choose active employees as responsible"));
    }
    Ok(ids.iter().copied().take(100).collect())
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<Body>) -> AppResult<Json<Form>> {
    require_admin(&auth)?;
    let et = b.entity_type.unwrap_or(LEAD);
    check_target(&state, et, b.category_id, b.stage_id).await?;
    let fields = check_fields(&state, et, b.fields.unwrap_or_default()).await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO crm_webforms (key, name, entity_type, category_id, stage_id, fields, settings, responsible, reply_mailbox, reply_subject, reply_body, is_active, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING id",
    )
    .bind(random_token(12))
    .bind(required("name", b.name.as_deref().unwrap_or(""), 200)?)
    .bind(et)
    .bind(b.category_id)
    .bind(b.stage_id)
    .bind(fields)
    .bind(check_settings(b.settings.unwrap_or_default())?)
    .bind(check_people(&state, &b.responsible.unwrap_or_default()).await?)
    .bind(b.reply_mailbox)
    .bind(b.reply_subject.unwrap_or_default().chars().take(300).collect::<String>())
    .bind(b.reply_body.unwrap_or_default().chars().take(20_000).collect::<String>())
    .bind(b.is_active.unwrap_or(true))
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(load(&state, id).await?))
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Body>) -> AppResult<Json<Form>> {
    require_admin(&auth)?;
    let cur = load(&state, id).await?;
    let et = b.entity_type.unwrap_or(cur.entity_type);
    let (category, stage) = if b.entity_type.is_some_and(|e| e != cur.entity_type) {
        (b.category_id, b.stage_id)
    } else {
        (b.category_id.or(cur.category_id), b.stage_id.or(cur.stage_id))
    };
    check_target(&state, et, category, stage).await?;
    let fields = match b.fields {
        Some(f) => check_fields(&state, et, f).await?,
        None => {
            let f: Vec<Field> = serde_json::from_value(cur.fields).unwrap_or_default();
            check_fields(&state, et, f).await?
        }
    };
    sqlx::query(
        "UPDATE crm_webforms SET name = $2, entity_type = $3, category_id = $4, stage_id = $5, fields = $6, settings = $7, responsible = $8,
             reply_mailbox = $9, reply_subject = $10, reply_body = $11, is_active = $12, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(match &b.name { Some(n) => required("name", n, 200)?, None => cur.name })
    .bind(et)
    .bind(category)
    .bind(stage)
    .bind(fields)
    .bind(match b.settings { Some(s) => check_settings(s)?, None => cur.settings })
    .bind(match &b.responsible { Some(r) => check_people(&state, r).await?, None => cur.responsible })
    .bind(if b.reply_mailbox.is_some() || b.reply_subject.is_some() { b.reply_mailbox } else { cur.reply_mailbox })
    .bind(b.reply_subject.unwrap_or(cur.reply_subject).chars().take(300).collect::<String>())
    .bind(b.reply_body.unwrap_or(cur.reply_body).chars().take(20_000).collect::<String>())
    .bind(b.is_active.unwrap_or(cur.is_active))
    .execute(&state.db)
    .await?;
    Ok(Json(load(&state, id).await?))
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    // Records created by the form stay; only its submission log goes.
    let n = sqlx::query("DELETE FROM crm_webforms WHERE id = $1").bind(id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("form"));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn submissions(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let rows: Vec<(Uuid, Option<Uuid>, Option<i16>, Option<String>, Value, Option<Value>, Option<String>, DateTime<Utc>, Option<Uuid>)> = sqlx::query_as(
        "SELECT s.id, s.item_id, i.entity_type, i.title, s.data, s.utm, s.page_url, s.created_at, s.contact_id
         FROM crm_webform_submissions s LEFT JOIN crm_items i ON i.id = s.item_id
         WHERE s.form_id = $1 ORDER BY s.created_at DESC LIMIT 200",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "items": rows.iter().map(|(id, item, et, title, data, utm, page, at, contact)| json!({
            "id": id, "item_id": item, "contact_id": contact, "entity_type": et, "item_title": title, "data": data, "utm": utm, "page_url": page, "created_at": at,
        })).collect::<Vec<_>>(),
    })))
}

// ---------------------------------------------------------------------------
// Public side
// ---------------------------------------------------------------------------

fn sign(state: &AppState, form: Uuid, issued_ms: i64) -> String {
    let mut h = Sha256::new();
    h.update(state.config.jwt_secret.as_bytes());
    h.update(b"|webform|");
    h.update(form.as_bytes());
    h.update(issued_ms.to_be_bytes());
    format!("{issued_ms}.{}", hex::encode(&h.finalize()[..16]))
}

fn token_age_secs(state: &AppState, form: Uuid, token: &str) -> Option<i64> {
    let (ms, _) = token.split_once('.')?;
    let ms: i64 = ms.parse().ok()?;
    (sign(state, form, ms) == token).then(|| (Utc::now().timestamp_millis() - ms) / 1000)
}

async fn active_form(state: &AppState, key: &str) -> AppResult<Form> {
    let id: Option<Uuid> = sqlx::query_scalar("SELECT id FROM crm_webforms WHERE key = $1 AND is_active").bind(key).fetch_optional(&state.db).await?;
    load(state, id.ok_or(AppError::NotFound("form"))?).await
}

/// What a visitor's browser needs to show the form (nothing internal).
async fn public_form(State(state): State<AppState>, Path(key): Path<String>) -> AppResult<Json<Value>> {
    let f = active_form(&state, &key).await?;
    let company: String = sqlx::query_scalar("SELECT value #>> '{}' FROM settings WHERE key = 'company_name'").fetch_optional(&state.db).await?.unwrap_or_default();
    let fields: Vec<Field> = serde_json::from_value(f.fields).unwrap_or_default();
    Ok(Json(json!({
        "name": f.name,
        "company": company,
        "settings": f.settings,
        "fields": fields.iter().map(|x| json!({ "key": x.key, "label": x.label, "kind": x.kind, "required": x.required, "options": x.options, "placeholder": x.placeholder })).collect::<Vec<_>>(),
        "token": sign(&state, f.id, Utc::now().timestamp_millis()),
    })))
}

#[derive(Deserialize)]
struct Submission {
    #[serde(default)]
    values: Map<String, Value>,
    /// The decoy field: people never see it, bots fill it in.
    #[serde(default)]
    website: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    consent: bool,
    utm: Option<Map<String, Value>>,
    page_url: Option<String>,
    referrer: Option<String>,
}

async fn limited(state: &AppState, key: String, max: i64, secs: u64) -> AppResult<bool> {
    let mut redis = state.redis.clone();
    let n: i64 = redis.incr(&key, 1).await?;
    if n == 1 {
        let _: () = redis.expire(&key, secs as i64).await?;
    }
    Ok(n > max)
}

fn clean_value(f: &Field, v: Option<&Value>) -> AppResult<Option<String>> {
    let s = match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::Bool(b)) => if *b { "yes".into() } else { String::new() },
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.trim().to_string(),
        _ => return Err(AppError::bad(format!("“{}” is not valid", f.label))),
    };
    if s.is_empty() {
        if f.required {
            return Err(AppError::bad(format!("please fill in “{}”", f.label)));
        }
        return Ok(None);
    }
    let max = if f.kind == "textarea" { 10_000 } else { 1_000 };
    if s.chars().count() > max {
        return Err(AppError::bad(format!("“{}” is too long", f.label)));
    }
    let bad = || AppError::bad(format!("please check “{}”", f.label));
    match f.kind.as_str() {
        "email" if s.parse::<lettre::Address>().is_err() => return Err(bad()),
        "phone" if s.chars().filter(|c| c.is_ascii_digit()).count() < 5 => return Err(bad()),
        "number" if s.replace(',', ".").parse::<f64>().is_err() => return Err(bad()),
        "date" if chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d").is_err() => return Err(bad()),
        "select" if !f.options.contains(&s) => return Err(bad()),
        _ => {}
    }
    Ok(Some(s))
}

async fn submit(State(state): State<AppState>, ClientIp(ip): ClientIp, Path(key): Path<String>, Json(b): Json<Submission>) -> AppResult<Json<Value>> {
    let f = active_form(&state, &key).await?;
    let settings = f.settings.clone();
    let success = json!({
        "message": settings["success_message"].as_str().unwrap_or("Thank you! We will get back to you soon."),
        "redirect_url": settings["redirect_url"],
    });
    // Bots: answer as if it worked, store nothing.
    if !b.website.is_empty() {
        return Ok(Json(success));
    }
    match token_age_secs(&state, f.id, &b.token) {
        Some(age) if (MIN_FILL_SECS..=24 * 3600).contains(&age) => {}
        Some(age) if age < MIN_FILL_SECS => return Ok(Json(success)),
        _ => return Err(AppError::bad("the form has expired; reload the page and try again")),
    }
    let who = ip.unwrap_or_else(|| "unknown".into());
    if limited(&state, format!("crm:webform:ip:{}:{who}", f.id), 10, 600).await? || limited(&state, format!("crm:webform:form:{}", f.id), 2000, 3600).await? {
        return Err(AppError::TooManyRequests("too many submissions; please try again later".into()));
    }
    if settings["consent_text"].as_str().is_some() && !b.consent {
        return Err(AppError::bad("please accept the terms to continue"));
    }
    let fields: Vec<Field> = serde_json::from_value(f.fields.clone()).unwrap_or_default();
    let mut values: Vec<(Field, String)> = Vec::new();
    for field in &fields {
        if let Some(v) = clean_value(field, b.values.get(&field.key))? {
            values.push((field.clone(), v));
        }
    }
    if values.is_empty() {
        return Err(AppError::bad("the form is empty"));
    }
    let utm: Option<Value> = b.utm.map(|u| {
        let mut m = Map::new();
        for k in ["source", "medium", "campaign", "content", "term"] {
            if let Some(s) = u.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
                m.insert(k.into(), json!(s.chars().take(200).collect::<String>()));
            }
        }
        Value::Object(m)
    });
    let utm = utm.filter(|u| u.as_object().is_some_and(|m| !m.is_empty()));
    let page = b.page_url.map(|p| p.chars().take(2000).collect::<String>()).filter(|p| p.starts_with("http"));
    let referrer = b.referrer.map(|p| p.chars().take(2000).collect::<String>()).filter(|p| p.starts_with("http"));

    let (item, contact) = create_records(&state, &f, &values, utm.clone(), page.clone()).await?;
    let data: Vec<Value> = values.iter().map(|(fl, v)| json!({ "key": fl.key, "label": fl.label, "value": v })).collect();
    sqlx::query("INSERT INTO crm_webform_submissions (form_id, item_id, contact_id, data, utm, page_url, referrer) VALUES ($1, $2, $3, $4, $5, $6, $7)")
        .bind(f.id)
        .bind(item)
        .bind(contact)
        .bind(json!(data))
        .bind(&utm)
        .bind(&page)
        .bind(&referrer)
        .execute(&state.db)
        .await?;
    for id in [Some(item), contact].into_iter().flatten() {
        timeline(&state.db, id, "webform", None, None, json!({ "form": f.name, "form_id": f.id, "values": data, "page_url": page })).await?;
    }
    fire(&state, item, "webform_submitted", |p| p.get("form_id").and_then(Value::as_str).is_none_or(|x| x.is_empty() || x == f.id.to_string())).await;

    // Automatic reply from a connected mailbox.
    let email = values.iter().find(|(fl, _)| fl.kind == "email").map(|(_, v)| v.clone());
    if let (Some(mb_id), Some(to)) = (f.reply_mailbox, email)
        && !f.reply_subject.trim().is_empty() {
            let fill = |t: &str| {
                let mut out = t.replace("{{form}}", &f.name);
                for (fl, v) in &values {
                    out = out.replace(&format!("{{{{{}}}}}", fl.key), v);
                }
                out
            };
            if let Err(e) = super::email::send_from_mailbox(&state, mb_id, item, &to, &fill(&f.reply_subject), &fill(&f.reply_body)).await {
                tracing::warn!(form = %f.id, error = ?e, "web form auto-reply failed");
            }
        }
    Ok(Json(success))
}

/// Picks the next responsible person in turn.
async fn next_responsible(state: &AppState, f: &Form) -> AppResult<Option<Uuid>> {
    if f.responsible.is_empty() {
        return Ok(f.created_by);
    }
    let mut redis = state.redis.clone();
    let n: i64 = redis.incr(format!("crm:webform:rr:{}", f.id), 1).await?;
    Ok(Some(f.responsible[((n - 1) as usize) % f.responsible.len()]))
}

/// Creates the records for a submission. Returns (main record, contact).
async fn create_records(state: &AppState, f: &Form, values: &[(Field, String)], utm: Option<Value>, page: Option<String>) -> AppResult<(Uuid, Option<Uuid>)> {
    let auth = system_auth();
    let t = entity_type_by_id(state, f.entity_type).await?;
    let get = |code: &str| values.iter().find(|(fl, _)| fl.maps_to == code).map(|(_, v)| v.clone());
    let assigned = next_responsible(state, f).await?;
    // Unmapped answers go into the comment, so nothing is lost.
    let mut comment: Vec<String> = values.iter().filter(|(fl, _)| fl.maps_to.is_empty()).map(|(fl, v)| format!("{}: {v}", fl.label)).collect();
    if let Some(c) = get("comments") {
        comment.insert(0, c);
    }
    let mut body = Map::new();
    let mut uf = Map::new();
    for (fl, v) in values {
        if let Some(code) = fl.maps_to.strip_prefix("uf.") {
            uf.insert(code.into(), match fl.kind.as_str() {
                "number" => v.replace(',', ".").parse::<f64>().map(|n| json!(n)).unwrap_or(json!(v)),
                "checkbox" => json!(true),
                _ => json!(v),
            });
        }
    }
    if !uf.is_empty() {
        body.insert("uf".into(), Value::Object(uf));
    }
    if !comment.is_empty() {
        body.insert("comments".into(), json!(comment.join("\n")));
    }
    body.insert("source".into(), json!("web_form"));
    body.insert("source_details".into(), json!(page.clone().unwrap_or_else(|| f.name.clone())));
    if let Some(u) = utm {
        body.insert("utm".into(), u);
    }
    if let Some(a) = assigned {
        body.insert("assigned_to".into(), json!(a));
    }
    if let Some(c) = f.category_id.filter(|_| t.has_pipelines) {
        body.insert("category_id".into(), json!(c));
    }
    if let Some(s) = f.stage_id {
        body.insert("stage_id".into(), json!(s));
    }
    let person = |first: Option<String>, last: Option<String>, email: Option<String>| -> String {
        let n = [first, last].into_iter().flatten().collect::<Vec<_>>().join(" ");
        if n.is_empty() { email.unwrap_or_default() } else { n }
    };

    if t.id == LEAD {
        for c in ["first_name", "last_name", "company_name"] {
            if let Some(v) = get(c) {
                body.insert(c.into(), json!(v.chars().take(300).collect::<String>()));
            }
        }
        let who = person(get("first_name"), get("last_name"), get("email"));
        body.insert("title".into(), json!(get("title").unwrap_or_else(|| format!("{}{}", f.name, if who.is_empty() { String::new() } else { format!(": {who}") }))));
        body.insert("multifields".into(), multifields(get("email"), get("phone")));
        let id = create_item(state, &auth, &t, body, None).await?;
        broadcast(state, t.id, id, false).await;
        notify_responsible(state, assigned, &t.name, &t.code, id, f).await;
        return Ok((id, None));
    }

    // Deal or smart process: find or create the contact, and the company.
    let (email, phone) = (get("contact.email"), get("contact.phone"));
    let mut contact: Option<Uuid> = None;
    for (kind, v) in [("email", &email), ("phone", &phone)] {
        if contact.is_some() {
            break;
        }
        if let Some(v) = v {
            contact = sqlx::query_scalar(
                "SELECT i.id FROM crm_multifields m JOIN crm_items i ON i.id = m.item_id
                 WHERE m.kind = $1 AND m.normalized = $2 AND i.entity_type = $3 AND i.deleted_at IS NULL ORDER BY i.updated_at DESC LIMIT 1",
            )
            .bind(kind)
            .bind(normalize(kind, v))
            .bind(CONTACT)
            .fetch_optional(&state.db)
            .await?;
        }
    }
    let (first, last) = (get("contact.first_name"), get("contact.last_name"));
    let ct = entity_type_by_id(state, CONTACT).await?;
    if contact.is_none() && (first.is_some() || last.is_some() || email.is_some() || phone.is_some()) {
        let mut c = Map::new();
        if let Some(v) = &first {
            c.insert("first_name".into(), json!(v));
        }
        if let Some(v) = &last {
            c.insert("last_name".into(), json!(v));
        }
        if first.is_none() && last.is_none() {
            c.insert("first_name".into(), json!(email.clone().or(phone.clone()).unwrap_or_default()));
        }
        c.insert("multifields".into(), multifields(email.clone(), phone.clone()));
        c.insert("source".into(), json!("web_form"));
        if let Some(a) = assigned {
            c.insert("assigned_to".into(), json!(a));
        }
        let id = create_item(state, &auth, &ct, c, None).await?;
        broadcast(state, CONTACT, id, false).await;
        contact = Some(id);
    }
    if let Some(name) = get("company.name") {
        let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM crm_items WHERE entity_type = $1 AND deleted_at IS NULL AND lower(title) = lower($2) LIMIT 1")
            .bind(COMPANY)
            .bind(&name)
            .fetch_optional(&state.db)
            .await?;
        let company = match existing {
            Some(c) => c,
            None => {
                let cot = entity_type_by_id(state, COMPANY).await?;
                let mut c = Map::new();
                c.insert("company_name".into(), json!(name.chars().take(300).collect::<String>()));
                c.insert("source".into(), json!("web_form"));
                if let Some(a) = assigned {
                    c.insert("assigned_to".into(), json!(a));
                }
                create_item(state, &auth, &cot, c, None).await?
            }
        };
        body.insert("company_id".into(), json!(company));
    }
    if let Some(c) = contact {
        body.insert("contact_ids".into(), json!([c]));
    }
    if let Some(v) = get("opportunity").and_then(|v| v.replace(',', ".").parse::<f64>().ok()) {
        body.insert("opportunity".into(), json!(v.to_string()));
    }
    let who = person(first, last, email);
    body.insert("title".into(), json!(get("title").unwrap_or_else(|| format!("{}{}", f.name, if who.is_empty() { String::new() } else { format!(": {who}") }))));
    let id = create_item(state, &auth, &t, body, None).await?;
    broadcast(state, t.id, id, false).await;
    notify_responsible(state, assigned, &t.name, &t.code, id, f).await;
    Ok((id, contact))
}

fn multifields(email: Option<String>, phone: Option<String>) -> Value {
    let mut m = Vec::new();
    if let Some(e) = email {
        m.push(json!({ "kind": "email", "value_type": "work", "value": e }));
    }
    if let Some(p) = phone {
        m.push(json!({ "kind": "phone", "value_type": "work", "value": p }));
    }
    json!(m)
}

async fn notify_responsible(state: &AppState, who: Option<Uuid>, type_name: &str, code: &str, id: Uuid, f: &Form) {
    if let Some(u) = who {
        notify(state, &[u], NewNotification {
            kind: "crm.webform",
            title: format!("New {} from the form “{}”", type_name.to_lowercase(), f.name),
            body: None,
            link: Some(format!("/crm/{code}/{id}")),
            actor: None,
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(kind: &str, required: bool) -> Field {
        Field { key: "k".into(), label: "L".into(), kind: kind.into(), required, maps_to: String::new(), options: vec!["A".into(), "B".into()], placeholder: String::new() }
    }

    #[test]
    fn values_are_checked() {
        assert!(clean_value(&field("email", true), Some(&json!("a@b.example"))).unwrap().is_some());
        assert!(clean_value(&field("email", true), Some(&json!("nope"))).is_err());
        assert!(clean_value(&field("text", true), Some(&json!("  "))).is_err());
        assert!(clean_value(&field("text", false), None).unwrap().is_none());
        assert!(clean_value(&field("phone", false), Some(&json!("12"))).is_err());
        assert_eq!(clean_value(&field("number", false), Some(&json!(3.5))).unwrap().as_deref(), Some("3.5"));
        assert!(clean_value(&field("select", false), Some(&json!("C"))).is_err());
        assert!(clean_value(&field("date", false), Some(&json!("2026-02-30"))).is_err());
        assert_eq!(clean_value(&field("checkbox", true), Some(&json!(true))).unwrap().as_deref(), Some("yes"));
    }
}
