//! Signing templates: who signs (by role, filled in from each record: its contact, its
//! company, the record itself, the responsible person, the sender, or a fixed person), the
//! message, and optionally a document template. "Send for signature" with a template makes
//! the document from that template (or takes the record's newest ready document) and sends
//! it; a document that renders in the job queue goes out as soon as it is ready. Robots use
//! the same path ("Send for signature" action).

use axum::{
    extract::{Path, State},
    routing::{get, patch, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{access::require, items::load_row};
use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/sign-templates", get(list).post(create))
        .route("/crm/sign-templates/{id}", patch(update).delete(remove))
        .route("/crm/items/{id}/sign-templates/{template}/signers", get(preview))
        .route("/crm/items/{id}/sign", post(sign_with))
}

const ROLES: &[(&str, &str)] = &[
    ("contact", "The record's contact"),
    ("company", "The record's company"),
    ("record", "The record itself (a contact, company or lead)"),
    ("responsible", "The responsible person"),
    ("sender", "Whoever sends it"),
    ("person", "A fixed person"),
];

fn manage(auth: &AuthUser) -> AppResult<()> {
    if auth.can(perm::CRM_ADMIN) || auth.can(perm::ESIGN) { Ok(()) } else { Err(AppError::Forbidden) }
}

#[derive(Deserialize)]
struct SignerSpec {
    role: String,
    name: Option<String>,
    email: Option<String>,
}

#[derive(Deserialize)]
struct Input {
    name: Option<String>,
    message: Option<String>,
    doc_template_id: Option<Option<Uuid>>,
    signers: Option<Vec<SignerSpec>>,
    entity_types: Option<Vec<i16>>,
}

async fn check(state: &AppState, b: &Input) -> AppResult<()> {
    if let Some(list) = &b.signers {
        if list.is_empty() || list.len() > 10 {
            return Err(AppError::bad("one to ten signers"));
        }
        for s in list {
            if !ROLES.iter().any(|(r, _)| *r == s.role) {
                return Err(AppError::bad(format!("signers are one of: {}", ROLES.iter().map(|r| r.0).collect::<Vec<_>>().join(", "))));
            }
            if s.role == "person" {
                crate::routes::required("signer's name", s.name.as_deref().unwrap_or(""), 200)?;
                let e = s.email.as_deref().unwrap_or("").trim();
                if !e.contains('@') || e.contains(char::is_whitespace) || e.len() > 254 {
                    return Err(AppError::bad("a fixed signer needs an email address"));
                }
            }
        }
    }
    if let Some(Some(t)) = b.doc_template_id {
        let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM crm_doc_templates WHERE id = $1) AS "v!""#, t).fetch_one(&state.db).await?;
        if !ok {
            return Err(AppError::bad("that document template doesn't exist"));
        }
    }
    if let Some(t) = &b.entity_types {
        let n: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM crm_entity_types WHERE id = ANY($1)"#, t).fetch_one(&state.db).await?;
        if n as usize != t.len() {
            return Err(AppError::bad("unknown record type"));
        }
    }
    Ok(())
}

fn specs(list: &[SignerSpec]) -> Value {
    json!(list.iter().map(|s| if s.role == "person" {
        json!({ "role": "person", "name": s.name.as_deref().unwrap_or("").trim(), "email": s.email.as_deref().unwrap_or("").trim().to_lowercase() })
    } else {
        json!({ "role": s.role })
    }).collect::<Vec<_>>())
}

const COLS: &str = "t.id, t.name, t.message, t.doc_template_id, d.name AS doc_template_name, t.signers, t.entity_types, t.updated_at";

async fn one(state: &AppState, id: Uuid) -> AppResult<Value> {
    let r: Option<(Uuid, String, Option<String>, Option<Uuid>, Option<String>, Value, Vec<i16>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM sign_templates t LEFT JOIN crm_doc_templates d ON d.id = t.doc_template_id WHERE t.id = $1"))).bind(id).fetch_optional(&state.db).await?;
    let (id, name, message, dt, dtn, signers, et, at) = r.ok_or(AppError::NotFound("signing template"))?;
    Ok(json!({ "id": id, "name": name, "message": message, "doc_template_id": dt, "doc_template_name": dtn, "signers": signers, "entity_types": et, "updated_at": at }))
}

async fn list(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    let ids: Vec<Uuid> = sqlx::query_scalar!("SELECT id FROM sign_templates ORDER BY lower(name)").fetch_all(&state.db).await?;
    let mut items = Vec::new();
    for id in ids {
        items.push(one(&state, id).await?);
    }
    Ok(Json(json!({ "items": items, "roles": ROLES.iter().map(|(r, l)| json!({ "role": r, "label": l })).collect::<Vec<_>>() })))
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<Input>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    check(&state, &b).await?;
    let name = crate::routes::required("name", b.name.as_deref().unwrap_or(""), 120)?;
    let signers = b.signers.as_deref().ok_or_else(|| AppError::bad("add the signers"))?;
    let id: Uuid = sqlx::query_scalar!("INSERT INTO sign_templates (name, message, doc_template_id, signers, entity_types, created_by) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id", &name, crate::routes::clean(b.message.clone()).map(|m| m.chars().take(2000).collect::<String>()), b.doc_template_id.flatten(), specs(signers), &b.entity_types.clone().unwrap_or_default(), auth.id)
        .fetch_one(&state.db)
        .await?;
    one(&state, id).await.map(Json)
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Input>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    one(&state, id).await?;
    check(&state, &b).await?;
    let name = b.name.as_deref().map(|n| crate::routes::required("name", n, 120)).transpose()?;
    sqlx::query(
        "UPDATE sign_templates SET name = COALESCE($2, name), message = CASE WHEN $3 THEN $4 ELSE message END,
            doc_template_id = CASE WHEN $5 THEN $6 ELSE doc_template_id END, signers = COALESCE($7, signers),
            entity_types = COALESCE($8, entity_types), updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(name)
    .bind(b.message.is_some())
    .bind(crate::routes::clean(b.message.clone()).map(|m| m.chars().take(2000).collect::<String>()))
    .bind(b.doc_template_id.is_some())
    .bind(b.doc_template_id.flatten())
    .bind(b.signers.as_deref().map(specs))
    .bind(b.entity_types.clone())
    .execute(&state.db)
    .await?;
    one(&state, id).await.map(Json)
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    let n = sqlx::query!("DELETE FROM sign_templates WHERE id = $1", id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("signing template"));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Using a template on a record
// ---------------------------------------------------------------------------

/// A record's (or contact's) display name and first email address.
async fn person_of(state: &AppState, item: Uuid) -> AppResult<Option<(String, String)>> {
    Ok(sqlx::query_as(
        "SELECT COALESCE(NULLIF(trim(COALESCE(i.first_name, '') || ' ' || COALESCE(i.last_name, '')), ''), i.title),
                (SELECT m.value FROM crm_multifields m WHERE m.item_id = i.id AND m.kind = 'email' ORDER BY m.sort LIMIT 1)
         FROM crm_items i WHERE i.id = $1",
    )
    .bind(item)
    .fetch_optional(&state.db)
    .await?
    .and_then(|(n, e): (String, Option<String>)| e.map(|e| (n, e))))
}

async fn user(state: &AppState, id: Option<Uuid>) -> AppResult<Option<(String, String)>> {
    let Some(id) = id else { return Ok(None) };
    Ok(sqlx::query_as("SELECT trim(first_name || ' ' || last_name), email FROM users WHERE id = $1 AND is_active").bind(id).fetch_optional(&state.db).await?)
}

/// The signers a template names, for this record and sender. A role that finds nobody with
/// an email address is reported, so the template isn't sent half-filled.
pub(crate) async fn resolve(state: &AppState, item: Uuid, sender: Uuid, signers: &Value) -> AppResult<Vec<(String, String)>> {
    let row = load_row(state, item, false).await?;
    let contact: Option<Uuid> = sqlx::query_scalar!("SELECT contact_id FROM crm_item_contacts WHERE item_id = $1 ORDER BY is_primary DESC, sort LIMIT 1", item).fetch_optional(&state.db).await?;
    let company: Option<Uuid> = sqlx::query_scalar!(r#"SELECT company_id AS "company_id?" FROM crm_items WHERE id = $1"#, item).fetch_one(&state.db).await?;
    let mut out: Vec<(String, String)> = Vec::new();
    for s in signers.as_array().cloned().unwrap_or_default() {
        let role = s["role"].as_str().unwrap_or("");
        let who = match role {
            "contact" => match contact { Some(c) => person_of(state, c).await?, None => None },
            "company" => match company { Some(c) => person_of(state, c).await?, None => None },
            "record" => person_of(state, item).await?,
            "responsible" => user(state, row.assigned_to).await?,
            "sender" => user(state, Some(sender)).await?,
            "person" => Some((s["name"].as_str().unwrap_or("").to_string(), s["email"].as_str().unwrap_or("").to_string())),
            _ => None,
        };
        let label = ROLES.iter().find(|(r, _)| *r == role).map(|r| r.1).unwrap_or(role);
        let (name, email) = who.ok_or_else(|| AppError::bad(format!("{label}: nobody with an email address")))?;
        if !out.iter().any(|(_, e)| e.eq_ignore_ascii_case(&email)) {
            out.push((name, email));
        }
    }
    Ok(out)
}

async fn template_row(state: &AppState, id: Uuid) -> AppResult<(Option<String>, Option<Uuid>, Value, Vec<i16>)> {
    sqlx::query!(r#"SELECT message, doc_template_id, signers, entity_types FROM sign_templates WHERE id = $1"#, id).map(|r| (r.message, r.doc_template_id, r.signers, r.entity_types)).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("signing template"))
}

async fn preview(State(state): State<AppState>, auth: AuthUser, Path((item, template)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    let row = load_row(&state, item, false).await?;
    super::access::require_read(&state, &auth, row.entity_type, item, row.assigned_to).await?;
    let (_, _, signers, _) = template_row(&state, template).await?;
    let list = resolve(&state, item, auth.id, &signers).await?;
    Ok(Json(json!({ "signers": list.into_iter().map(|(n, e)| json!({ "name": n, "email": e })).collect::<Vec<_>>() })))
}

/// Sends a record's document for signature with a template. `by`: who sends (an employee,
/// or the automation acting for the record's responsible person). Returns what happened.
pub(crate) async fn start(state: &AppState, by: Uuid, item: Uuid, template: Uuid, document: Option<Uuid>) -> AppResult<Value> {
    let row = load_row(state, item, false).await?;
    let (message, doc_template, signers, types) = template_row(state, template).await?;
    if !types.is_empty() && !types.contains(&row.entity_type) {
        return Err(AppError::bad("that signing template isn't offered for this kind of record"));
    }
    let list = resolve(state, item, by, &signers).await?;
    let doc = match (document, doc_template) {
        (Some(d), _) => {
            let of: Option<Uuid> = sqlx::query_scalar!("SELECT item_id FROM crm_documents WHERE id = $1", d).fetch_optional(&state.db).await?;
            if of != Some(item) {
                return Err(AppError::NotFound("document"));
            }
            d
        }
        (None, Some(t)) => {
            let sender = crate::auth::load_auth_user(state, by).await?;
            let d = super::docs::api::generate(state, &sender, item, t).await?;
            let ready: bool = sqlx::query_scalar!(r#"SELECT status = 'ready' AS "v!" FROM crm_documents WHERE id = $1"#, d).fetch_one(&state.db).await?;
            if !ready {
                sqlx::query!("INSERT INTO sign_pending (document_id, template_id, requested_by) VALUES ($1, $2, $3) ON CONFLICT (document_id) DO NOTHING", d, template, by)
                    .execute(&state.db)
                    .await?;
                return Ok(json!({ "status": "preparing", "document_id": d }));
            }
            d
        }
        (None, None) => sqlx::query_scalar!("SELECT id FROM crm_documents WHERE item_id = $1 AND status = 'ready' ORDER BY created_at DESC LIMIT 1", item)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::bad("the record has no ready document to sign; make one first"))?,
    };
    let id = super::esign::send_document(state, by, doc, list, message, Some(template)).await?;
    Ok(json!({ "status": "sent", "request_id": id, "document_id": doc }))
}

#[derive(Deserialize)]
struct SignWith {
    template_id: Uuid,
    document_id: Option<Uuid>,
}

async fn sign_with(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>, Json(b): Json<SignWith>) -> AppResult<Json<Value>> {
    super::esign::may_send(&state, &auth).await?;
    let row = load_row(&state, item, false).await?;
    require(&state, &auth, row.entity_type, "write", row.assigned_to).await?;
    start(&state, auth.id, item, b.template_id, b.document_id).await.map(Json)
}

pub const JOB: &str = "crm.sign_pending";

/// A document finished rendering: if it was waiting to go out for signature, a job sends it
/// (a job, not a call: making a document can finish at once, inside the sending itself).
pub async fn document_ready(state: &AppState, document: Uuid) {
    let waiting: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM sign_pending WHERE document_id = $1) AS "v!""#, document).fetch_one(&state.db).await.unwrap_or(false);
    if waiting {
        if let Err(e) = crate::jobs::enqueue(&state.db, JOB, json!({ "document_id": document }), None).await {
            tracing::warn!(document = %document, error = ?e, "could not queue sending for signature");
        }
        state.job_wakeup.notify_one();
    }
}

/// The job: sends a waiting document for signature.
pub async fn run_job(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let document: Uuid = serde_json::from_value(payload["document_id"].clone())?;
    let waiting: Option<(Uuid, Option<Uuid>)> = sqlx::query!(r#"DELETE FROM sign_pending WHERE document_id = $1 RETURNING template_id, requested_by"#, document).map(|r| (r.template_id, r.requested_by)).fetch_optional(&state.db).await?;
    let Some((template, Some(by))) = waiting else { return Ok(()) };
    let item: Option<Uuid> = sqlx::query_scalar!("SELECT item_id FROM crm_documents WHERE id = $1 AND status = 'ready'", document).fetch_optional(&state.db).await?;
    let Some(item) = item else { return Ok(()) };
    if let Err(e) = start(state, by, item, template, Some(document)).await {
        tracing::warn!(document = %document, error = ?e, "could not send the document for signature");
    }
    Ok(())
}
