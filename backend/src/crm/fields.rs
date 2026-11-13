//! Field definitions: the built-in fields of each record type, plus custom ("user") fields
//! whose values live in `crm_items.uf` and are validated here.

use axum::{
    extract::{Path, State},
    routing::{get, patch},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;
use std::str::FromStr;
use uuid::Uuid;

use super::{access::is_crm_admin, entity_type, COMPANY, CONTACT, DEAL, LEAD};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/{type}/fields", get(list).post(create))
        .route("/crm/{type}/fields/{id}", patch(update).delete(remove))
}

/// A built-in field, described for forms, list columns, filters and import mapping.
#[derive(Serialize, Clone, Copy)]
pub struct SystemField {
    pub code: &'static str,
    pub label: &'static str,
    pub kind: &'static str,
    #[serde(skip)]
    pub types: &'static [i16],
}

const P: &[i16] = &[LEAD, CONTACT];
const MONEY: &[i16] = &[LEAD, DEAL];
const ALL: &[i16] = &[LEAD, DEAL, CONTACT, COMPANY];

pub const SYSTEM_FIELDS: &[SystemField] = &[
    SystemField { code: "title", label: "Title", kind: "string", types: &[LEAD, DEAL] },
    SystemField { code: "honorific", label: "Salutation", kind: "string", types: P },
    SystemField { code: "first_name", label: "First name", kind: "string", types: P },
    SystemField { code: "middle_name", label: "Middle name", kind: "string", types: P },
    SystemField { code: "last_name", label: "Last name", kind: "string", types: P },
    SystemField { code: "job_title", label: "Position", kind: "string", types: P },
    SystemField { code: "birthdate", label: "Date of birth", kind: "date", types: P },
    SystemField { code: "company_name", label: "Company name", kind: "string", types: &[LEAD, COMPANY] },
    SystemField { code: "company_kind", label: "Company type", kind: "list", types: &[COMPANY] },
    SystemField { code: "industry", label: "Industry", kind: "list", types: &[COMPANY] },
    SystemField { code: "employees", label: "Employees", kind: "list", types: &[COMPANY] },
    SystemField { code: "annual_revenue", label: "Annual revenue", kind: "money", types: &[COMPANY] },
    SystemField { code: "opportunity", label: "Amount", kind: "money", types: MONEY },
    SystemField { code: "currency", label: "Currency", kind: "currency", types: MONEY },
    SystemField { code: "probability", label: "Probability, %", kind: "number", types: &[DEAL] },
    SystemField { code: "close_date", label: "Expected close date", kind: "date", types: &[DEAL] },
    SystemField { code: "source", label: "Source", kind: "list", types: ALL },
    SystemField { code: "source_details", label: "Source details", kind: "text", types: ALL },
    SystemField { code: "phone", label: "Phone", kind: "multifield", types: &[LEAD, CONTACT, COMPANY] },
    SystemField { code: "email", label: "Email", kind: "multifield", types: &[LEAD, CONTACT, COMPANY] },
    SystemField { code: "web", label: "Website", kind: "multifield", types: &[LEAD, CONTACT, COMPANY] },
    SystemField { code: "im", label: "Messenger", kind: "multifield", types: &[LEAD, CONTACT, COMPANY] },
    SystemField { code: "address", label: "Address", kind: "address", types: &[LEAD, CONTACT, COMPANY] },
    SystemField { code: "assigned_to", label: "Responsible", kind: "user", types: ALL },
    SystemField { code: "comments", label: "Comment", kind: "text", types: ALL },
];

/// Choices for built-in list fields. Kept in code so every deployment agrees on the keys;
/// labels are translated in the UI.
pub fn system_options() -> Value {
    json!({
        "source": ["call", "email", "website", "web_form", "booking", "referral", "advertising", "partner", "exhibition", "social", "other"],
        "company_kind": ["customer", "partner", "supplier", "reseller", "competitor", "other"],
        "industry": ["it", "telecom", "manufacturing", "banking", "consulting", "finance", "government", "delivery",
                     "entertainment", "nonprofit", "retail", "healthcare", "education", "construction", "energy",
                     "hospitality", "real_estate", "agriculture", "transport", "other"],
        "employees": ["1-10", "11-50", "51-250", "251-1000", "1001-5000", "5000+"],
        "phone": ["work", "mobile", "home", "fax", "other"],
        "email": ["work", "home", "other"],
        "web": ["work", "personal", "facebook", "linkedin", "x", "instagram", "other"],
        "im": ["telegram", "whatsapp", "signal", "viber", "wechat", "line", "skype", "other"],
    })
}

/// Built-in fields of a record type. Smart processes get the deal-like subset their
/// options allow.
pub fn system_fields_for(t: &super::EntityType) -> Vec<SystemField> {
    if t.is_custom() {
        // Quotes are valid until, invoices due by, the date smart processes call "close date".
        let date_label = match t.kind.as_deref() {
            Some("quote") => "Valid until",
            Some("invoice") => "Due date",
            Some("order") => "Deliver by",
            _ => "Due date",
        };
        return SYSTEM_FIELDS
            .iter()
            .copied()
            .map(|f| if f.code == "close_date" { SystemField { label: date_label, ..f } } else { f })
            .filter(|f| match f.code {
                "title" | "close_date" | "source" | "source_details" | "assigned_to" | "comments" => true,
                "opportunity" | "currency" => t.has_amount,
                _ => false,
            })
            .collect();
    }
    SYSTEM_FIELDS.iter().copied().filter(|f| f.types.contains(&t.id)).collect()
}

#[derive(Serialize, FromRow, Clone)]
pub struct FieldDef {
    pub id: Uuid,
    pub code: String,
    pub label: String,
    pub kind: String,
    pub is_multiple: bool,
    pub is_required: bool,
    pub show_in_list: bool,
    pub settings: Value,
    pub sort: i32,
    /// Who sees the field's values; both empty means everyone.
    pub visible_user_ids: Vec<Uuid>,
    pub visible_department_ids: Vec<Uuid>,
}

impl FieldDef {
    pub fn restricted(&self) -> bool {
        !self.visible_user_ids.is_empty() || !self.visible_department_ids.is_empty()
    }
}

pub async fn custom_fields(state: &AppState, entity_type: i16) -> AppResult<Vec<FieldDef>> {
    Ok(sqlx::query_as!(FieldDef, "SELECT id, code, label, kind, is_multiple, is_required, show_in_list, settings, sort, visible_user_ids, visible_department_ids
         FROM crm_field_defs WHERE entity_type = $1 ORDER BY sort, created_at", entity_type)
    .fetch_all(&state.db)
    .await?)
}

/// The codes of the type's custom fields this person may not see (restricted to other people
/// or departments). CRM administrators and the system see everything.
pub async fn hidden_codes(state: &AppState, auth: &AuthUser, entity_type: i16) -> AppResult<Vec<String>> {
    let defs = custom_fields(state, entity_type).await?;
    hidden_among(state, auth, &defs).await
}

pub async fn hidden_among(state: &AppState, auth: &AuthUser, defs: &[FieldDef]) -> AppResult<Vec<String>> {
    if is_crm_admin(auth) || !defs.iter().any(FieldDef::restricted) {
        return Ok(vec![]);
    }
    // The person's departments and those above them.
    let mine: Vec<Uuid> = sqlx::query_scalar!(r#"WITH RECURSIVE mine AS (
             SELECT department_id AS id FROM user_departments WHERE user_id = $1
             UNION SELECT d.parent_id FROM departments d JOIN mine ON d.id = mine.id WHERE d.parent_id IS NOT NULL)
         SELECT id AS "id!" FROM mine"#, auth.id)
    .fetch_all(&state.db)
    .await?;
    Ok(defs
        .iter()
        .filter(|d| d.restricted() && !d.visible_user_ids.contains(&auth.id) && !d.visible_department_ids.iter().any(|x| mine.contains(x)))
        .map(|d| d.code.clone())
        .collect())
}

/// Takes hidden fields' values out of a record's `uf`.
pub fn strip_uf(uf: &mut Value, hidden: &[String]) {
    if let Value::Object(m) = uf {
        m.retain(|k, _| !hidden.contains(k));
    }
}

/// The custom fields this person may see.
pub async fn visible_fields(state: &AppState, auth: &AuthUser, entity_type: i16) -> AppResult<Vec<FieldDef>> {
    let defs = custom_fields(state, entity_type).await?;
    let hidden = hidden_among(state, auth, &defs).await?;
    Ok(defs.into_iter().filter(|d| !hidden.contains(&d.code)).collect())
}

const KINDS: &[&str] = &["string", "text", "number", "money", "date", "datetime", "boolean", "list", "user", "crm", "url", "file", "address"];

fn invalid(label: &str, why: &str) -> AppError {
    AppError::bad(format!("{label}: {why}"))
}

/// Validates and normalises one value of a custom field.
fn check_one(state_users: &[Uuid], def: &FieldDef, v: &Value) -> AppResult<Value> {
    let l = def.label.as_str();
    Ok(match def.kind.as_str() {
        "string" | "text" | "url" => {
            let s = v.as_str().ok_or_else(|| invalid(l, "must be text"))?.trim();
            let max = if def.kind == "text" { 20_000 } else { 1000 };
            if s.chars().count() > max {
                return Err(invalid(l, "is too long"));
            }
            if def.kind == "url" && !s.is_empty() && !(s.starts_with("http://") || s.starts_with("https://")) {
                return Err(invalid(l, "must start with http:// or https://"));
            }
            json!(s)
        }
        "number" => {
            let n = match v {
                Value::Number(n) => n.as_f64(),
                Value::String(s) => s.trim().parse::<f64>().ok(),
                _ => None,
            }
            .filter(|n| n.is_finite())
            .ok_or_else(|| invalid(l, "must be a number"))?;
            json!(n)
        }
        "money" => {
            // {"amount": "12.50", "currency": "EUR"}
            let amount = v.get("amount").and_then(|a| match a {
                Value::String(s) => Decimal::from_str(s.trim()).ok(),
                Value::Number(n) => Decimal::from_str(&n.to_string()).ok(),
                _ => None,
            });
            let currency = v.get("currency").and_then(Value::as_str).map(str::to_uppercase);
            match (amount, currency) {
                (Some(a), Some(c)) if c.len() == 3 && c.chars().all(|ch| ch.is_ascii_uppercase()) => json!({ "amount": a.to_string(), "currency": c }),
                _ => return Err(invalid(l, "needs an amount and a 3-letter currency code")),
            }
        }
        "date" => {
            let s = v.as_str().ok_or_else(|| invalid(l, "must be a date"))?;
            NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| invalid(l, "must be a date (YYYY-MM-DD)"))?;
            json!(s)
        }
        "datetime" => {
            let s = v.as_str().ok_or_else(|| invalid(l, "must be a date and time"))?;
            let d: DateTime<Utc> = s.parse().map_err(|_| invalid(l, "must be an ISO date and time"))?;
            json!(d.to_rfc3339())
        }
        "boolean" => json!(v.as_bool().ok_or_else(|| invalid(l, "must be yes or no"))?),
        "list" => {
            let s = v.as_str().ok_or_else(|| invalid(l, "must be one of the options"))?;
            let ok = def.settings["options"].as_array().is_some_and(|o| o.iter().any(|x| x["id"] == s));
            if !ok {
                return Err(invalid(l, "is not one of the options"));
            }
            json!(s)
        }
        "user" => {
            let id = v.as_str().and_then(|s| Uuid::parse_str(s).ok()).ok_or_else(|| invalid(l, "must be a person"))?;
            if !state_users.contains(&id) {
                return Err(invalid(l, "person not found"));
            }
            json!(id)
        }
        "crm" | "file" => {
            let id = v.as_str().and_then(|s| Uuid::parse_str(s).ok()).ok_or_else(|| invalid(l, "must be a reference"))?;
            json!(id)
        }
        "address" => {
            let o = v.as_object().ok_or_else(|| invalid(l, "must be an address"))?;
            json!(clean_address(o)?)
        }
        _ => return Err(invalid(l, "unsupported field type")),
    })
}

/// Keeps only the known address parts, trimmed; checks the country code shape.
pub fn clean_address(o: &Map<String, Value>) -> AppResult<Map<String, Value>> {
    let mut out = Map::new();
    for k in ["line1", "line2", "city", "region", "postal_code", "country"] {
        if let Some(s) = o.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
            if s.chars().count() > 300 {
                return Err(AppError::bad("address is too long"));
            }
            if k == "country" && !(s.len() == 2 && s.chars().all(|c| c.is_ascii_uppercase())) {
                return Err(AppError::bad("country must be a two-letter ISO code"));
            }
            out.insert(k.into(), json!(s));
        }
    }
    Ok(out)
}

/// Validates a full or partial `uf` object against the type's custom fields.
/// `partial`: only the keys present are checked (PATCH); otherwise required fields must be set.
/// `hidden`: fields the person can't see, so they aren't asked to fill them in.
pub async fn validate_uf(state: &AppState, entity_type: i16, uf: &Map<String, Value>, partial: bool, hidden: &[String]) -> AppResult<Map<String, Value>> {
    let defs = custom_fields(state, entity_type).await?;
    // Collect user references once.
    let mut user_refs: Vec<Uuid> = Vec::new();
    for d in defs.iter().filter(|d| d.kind == "user") {
        let vals = match uf.get(&d.code) {
            Some(Value::Array(a)) => a.clone(),
            Some(v) => vec![v.clone()],
            None => vec![],
        };
        user_refs.extend(vals.iter().filter_map(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok())));
    }
    let existing: Vec<Uuid> = if user_refs.is_empty() {
        vec![]
    } else {
        sqlx::query_scalar!("SELECT id FROM users WHERE id = ANY($1)", &user_refs).fetch_all(&state.db).await?
    };

    let mut out = Map::new();
    for (k, _) in uf.iter() {
        if !defs.iter().any(|d| &d.code == k) {
            return Err(AppError::bad(format!("unknown field: {k}")));
        }
    }
    for d in &defs {
        let Some(v) = uf.get(&d.code) else {
            if !partial && d.is_required && !hidden.contains(&d.code) {
                return Err(invalid(&d.label, "is required"));
            }
            continue;
        };
        let empty = v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty()) || v.as_array().is_some_and(|a| a.is_empty());
        if empty {
            if d.is_required {
                return Err(invalid(&d.label, "is required"));
            }
            out.insert(d.code.clone(), Value::Null);
            continue;
        }
        let value = if d.is_multiple {
            let arr = v.as_array().ok_or_else(|| invalid(&d.label, "must be a list"))?;
            Value::Array(arr.iter().map(|x| check_one(&existing, d, x)).collect::<AppResult<_>>()?)
        } else {
            check_one(&existing, d, v)?
        };
        out.insert(d.code.clone(), value);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Field management (crm.admin)
// ---------------------------------------------------------------------------

async fn list(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>) -> AppResult<Json<Vec<FieldDef>>> {
    let t = entity_type(&state, &code).await?;
    Ok(Json(visible_fields(&state, &auth, t.id).await?))
}

#[derive(Deserialize)]
struct NewField {
    label: String,
    kind: String,
    #[serde(default)]
    is_multiple: bool,
    #[serde(default)]
    is_required: bool,
    #[serde(default)]
    show_in_list: bool,
    #[serde(default)]
    settings: Value,
    #[serde(default)]
    visible_user_ids: Vec<Uuid>,
    #[serde(default)]
    visible_department_ids: Vec<Uuid>,
}

/// Who may see a field: employees (not guests) and existing departments.
async fn check_audience(state: &AppState, users: &mut Vec<Uuid>, deps: &mut Vec<Uuid>) -> AppResult<()> {
    users.sort();
    users.dedup();
    deps.sort();
    deps.dedup();
    if users.len() + deps.len() > 200 {
        return Err(AppError::bad("choose at most 200 people and departments"));
    }
    let u: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM users WHERE id = ANY($1) AND NOT is_guest"#, &*users).fetch_one(&state.db).await?;
    let d: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM departments WHERE id = ANY($1)"#, &*deps).fetch_one(&state.db).await?;
    if u as usize != users.len() || d as usize != deps.len() {
        return Err(AppError::bad("choose employees and existing departments"));
    }
    Ok(())
}

fn validate_settings(kind: &str, settings: &Value) -> AppResult<Value> {
    match kind {
        "list" => {
            let opts = settings["options"].as_array().ok_or_else(|| AppError::bad("a list field needs options"))?;
            if opts.is_empty() || opts.len() > 500 {
                return Err(AppError::bad("a list field needs between 1 and 500 options"));
            }
            let mut seen = std::collections::HashSet::new();
            let mut clean = Vec::new();
            for o in opts {
                let label = o["label"].as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| AppError::bad("every option needs a label"))?;
                let id = o["id"].as_str().map(String::from).unwrap_or_else(|| Uuid::new_v4().simple().to_string()[..8].to_string());
                if !seen.insert(id.clone()) {
                    return Err(AppError::bad("option ids must be unique"));
                }
                clean.push(json!({ "id": id, "label": label }));
            }
            Ok(json!({ "options": clean }))
        }
        "crm" => {
            let types: Vec<String> = settings["types"].as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect()).unwrap_or_default();
            if types.is_empty() {
                return Err(AppError::bad("choose which kinds of records the field can link to"));
            }
            Ok(json!({ "types": types }))
        }
        _ => Ok(json!({})),
    }
}

fn slug(label: &str) -> String {
    let s: String = label
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    let s: String = s.chars().take(40).collect();
    if s.is_empty() { "field".into() } else { s }
}

async fn create(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(mut body): Json<NewField>) -> AppResult<Json<FieldDef>> {
    if !is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    let t = entity_type(&state, &code).await?;
    let label = required("label", &body.label, 100)?;
    if !KINDS.contains(&body.kind.as_str()) {
        return Err(AppError::bad("unknown field type"));
    }
    let settings = validate_settings(&body.kind, &body.settings)?;
    check_audience(&state, &mut body.visible_user_ids, &mut body.visible_department_ids).await?;
    // Unique code derived from the label: uf_<slug>, uf_<slug>_2, …
    let base = format!("uf_{}", slug(&label));
    let mut code = base.clone();
    for n in 2..1000 {
        let taken: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM crm_field_defs WHERE entity_type = $1 AND code = $2) AS "v!""#, t.id, &code)
            .fetch_one(&state.db)
            .await?;
        if !taken {
            break;
        }
        code = format!("{base}_{n}");
    }
    let id: Uuid = sqlx::query_scalar!("INSERT INTO crm_field_defs (entity_type, code, label, kind, is_multiple, is_required, show_in_list, settings, sort, visible_user_ids, visible_department_ids)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8,
                 COALESCE((SELECT MAX(sort) FROM crm_field_defs WHERE entity_type = $1), 0) + 10, $9, $10) RETURNING id", t.id, &code, &label, &body.kind, body.is_multiple, body.is_required, body.show_in_list, settings, &body.visible_user_ids, &body.visible_department_ids)
    .fetch_one(&state.db)
    .await?;
    let def = custom_fields(&state, t.id).await?.into_iter().find(|d| d.id == id).unwrap();
    Ok(Json(def))
}

/// A plain custom field (no settings, seen by everyone), as a migration from another CRM makes
/// them; returns its code.
pub async fn add_plain(state: &AppState, entity_type: i16, label: &str, kind: &str) -> AppResult<String> {
    let label: String = label.trim().chars().take(100).collect();
    let label = if label.is_empty() { "Imported".to_string() } else { label };
    let base = format!("uf_{}", slug(&label));
    let mut code = base.clone();
    for n in 2..1000 {
        let taken: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM crm_field_defs WHERE entity_type = $1 AND code = $2) AS "v!""#, entity_type, &code)
            .fetch_one(&state.db)
            .await?;
        if !taken {
            break;
        }
        code = format!("{base}_{n}");
    }
    sqlx::query!("INSERT INTO crm_field_defs (entity_type, code, label, kind, settings, sort)
         VALUES ($1, $2, $3, $4, '{}', COALESCE((SELECT MAX(sort) FROM crm_field_defs WHERE entity_type = $1), 0) + 10)", entity_type, &code, &label, kind)
    .execute(&state.db)
    .await?;
    Ok(code)
}

#[derive(Deserialize)]
struct UpdateField {
    label: Option<String>,
    is_required: Option<bool>,
    show_in_list: Option<bool>,
    settings: Option<Value>,
    sort: Option<i32>,
    visible_user_ids: Option<Vec<Uuid>>,
    visible_department_ids: Option<Vec<Uuid>>,
}

async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((code, id)): Path<(String, Uuid)>,
    Json(body): Json<UpdateField>,
) -> AppResult<Json<FieldDef>> {
    if !is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    let t = entity_type(&state, &code).await?;
    let def = custom_fields(&state, t.id).await?.into_iter().find(|d| d.id == id).ok_or(AppError::NotFound("field"))?;
    let settings = match &body.settings {
        Some(s) => validate_settings(&def.kind, s)?,
        None => def.settings.clone(),
    };
    let mut users = body.visible_user_ids.clone().unwrap_or(def.visible_user_ids.clone());
    let mut deps = body.visible_department_ids.clone().unwrap_or(def.visible_department_ids.clone());
    check_audience(&state, &mut users, &mut deps).await?;
    sqlx::query!("UPDATE crm_field_defs SET label = $1, is_required = $2, show_in_list = $3, settings = $4, sort = $5,
             visible_user_ids = $7, visible_department_ids = $8 WHERE id = $6", match &body.label {
        Some(l) => required("label", l, 100)?,
        None => def.label.clone(),
    }, body.is_required.unwrap_or(def.is_required), body.show_in_list.unwrap_or(def.show_in_list), settings, body.sort.unwrap_or(def.sort), id, &users, &deps)
    .execute(&state.db)
    .await?;
    Ok(Json(custom_fields(&state, t.id).await?.into_iter().find(|d| d.id == id).unwrap()))
}

/// Deleting a field hides its values; they are removed from records by the purge job lazily
/// (rewriting millions of rows synchronously would block the request).
async fn remove(State(state): State<AppState>, auth: AuthUser, Path((code, id)): Path<(String, Uuid)>) -> AppResult<Json<Value>> {
    if !is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    let t = entity_type(&state, &code).await?;
    let n = sqlx::query!("DELETE FROM crm_field_defs WHERE id = $1 AND entity_type = $2", id, t.id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("field"));
    }
    Ok(Json(json!({ "ok": true })))
}

/// A custom field value as display text (for documents and exports).
pub fn ufield_text(def: &FieldDef, v: Option<&Value>) -> String {
    let one = |x: &Value| -> String {
        match def.kind.as_str() {
            "boolean" => if x.as_bool() == Some(true) { "Yes".into() } else { "No".into() },
            "list" => def.settings["options"]
                .as_array()
                .and_then(|o| o.iter().find(|o| o["id"] == *x))
                .and_then(|o| o["label"].as_str())
                .unwrap_or_default()
                .to_string(),
            "money" => format!("{} {}", x["amount"].as_str().unwrap_or(""), x["currency"].as_str().unwrap_or("")).trim().to_string(),
            "address" => ["line1", "line2", "postal_code", "city", "region", "country"]
                .iter()
                .filter_map(|k| x[*k].as_str())
                .collect::<Vec<_>>()
                .join(", "),
            _ => match x {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            },
        }
    };
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::Array(a)) => a.iter().map(one).collect::<Vec<_>>().join(", "),
        Some(x) => one(x),
    }
}
