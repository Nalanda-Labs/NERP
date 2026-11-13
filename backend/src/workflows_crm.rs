//! Business processes on CRM records: the same engine as for list items (`workflows.rs`),
//! with a record's fields — title, amount, stage, source, comments, close date, probability
//! and its custom fields — for conditions and changes, and its responsible person in the
//! author's place. Processes of a record type start when a record is made, when it changes,
//! or by hand from the record; CRM administrators design them.
//!
//! Changes made by robots and by processes themselves (the built-in Automation user) don't
//! start processes, so processes can't set each other off in a loop.

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use rust_decimal::prelude::ToPrimitive;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    crm::{access, automation::SYSTEM_USER, entity_type, entity_type_by_id, items::load_row},
    error::{AppError, AppResult},
    lists::Field,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/{type}/workflows", get(list_templates).post(create_template))
        .route("/crm/items/{id}/workflows", get(record_processes))
        .route("/crm/items/{id}/workflows/{template}", post(start_manual))
}

fn field(code: &str, label: &str, kind: &str, options: Vec<String>) -> Field {
    Field { id: None, code: code.into(), label: label.into(), kind: kind.into(), options, required: false }
}

/// The fields a process on records of this type may look at and change.
pub(crate) async fn fields(state: &AppState, et: i16) -> AppResult<Vec<Field>> {
    let t = entity_type_by_id(state, et).await?;
    let mut out = vec![field("title", "Title", "string", vec![]), field("comments", "Comments", "text", vec![]), field("source", "Source", "string", vec![])];
    if t.has_stages {
        let stages: Vec<String> = sqlx::query_scalar!("SELECT DISTINCT s.name FROM crm_stages s JOIN crm_categories c ON c.id = s.category_id WHERE c.entity_type = $1 ORDER BY 1", et).fetch_all(&state.db).await?;
        out.push(field("stage", "Stage", "select", stages));
    }
    if t.has_amount {
        out.push(field("opportunity", "Amount", "number", vec![]));
        out.push(field("currency", "Currency", "string", vec![]));
    }
    if et == crate::crm::DEAL || t.is_custom() {
        out.push(field("close_date", "Close date", "date", vec![]));
    }
    if et == crate::crm::DEAL {
        out.push(field("probability", "Probability, %", "number", vec![]));
    }
    for f in crate::crm::fields_custom(state, et).await? {
        let (kind, options) = match f.kind.as_str() {
            "number" | "money" => ("number", vec![]),
            "boolean" => ("checkbox", vec![]),
            "list" => ("select", f.settings["options"].as_array().map(|o| o.iter().filter_map(|x| x["label"].as_str().map(String::from)).collect()).unwrap_or_default()),
            "date" => ("date", vec![]),
            "string" | "text" | "url" => ("string", vec![]),
            // People, records, files and addresses can be looked at but not set.
            other => (if other == "user" { "user" } else { "file" }, vec![]),
        };
        if f.is_multiple && kind != "file" {
            continue;
        }
        out.push(field(&f.code, &f.label, kind, options));
    }
    Ok(out)
}

/// A record's values as a process sees them (list options by their labels).
pub(crate) async fn values(state: &AppState, item: Uuid) -> AppResult<(crate::crm::items::Row, Value)> {
    let r = load_row(state, item, false).await?;
    let mut v = Map::new();
    v.insert("title".into(), json!(r.title));
    v.insert("comments".into(), json!(r.comments));
    v.insert("source".into(), json!(r.source));
    v.insert("stage".into(), json!(r.stage_name));
    v.insert("opportunity".into(), json!(r.opportunity.to_f64()));
    v.insert("currency".into(), json!(r.currency));
    v.insert("close_date".into(), json!(r.close_date.map(|d| d.to_string())));
    v.insert("probability".into(), json!(r.probability));
    let defs = crate::crm::fields_custom(state, r.entity_type).await?;
    if let Some(uf) = r.uf.as_object() {
        for (k, val) in uf {
            let label = defs.iter().find(|d| &d.code == k && d.kind == "list").and_then(|d| d.settings["options"].as_array()?.iter().find(|o| o["id"] == *val)?["label"].as_str().map(String::from));
            v.insert(k.clone(), label.map(Value::from).unwrap_or_else(|| val.clone()));
        }
    }
    Ok((r, Value::Object(v)))
}

/// A process sets a field: as an ordinary change of the record (by the Automation user).
pub(crate) async fn set_field(state: &AppState, item: Uuid, code: &str, value: &str) -> AppResult<()> {
    let r = load_row(state, item, false).await?;
    let t = entity_type_by_id(state, r.entity_type).await?;
    let mut body = Map::new();
    let empty = value.trim().is_empty();
    match code {
        "stage" => {
            let stage: Option<Uuid> = sqlx::query_scalar!("SELECT id FROM crm_stages WHERE category_id = $1 AND lower(name) = lower($2) LIMIT 1", r.category_id, value.trim()).fetch_optional(&state.db).await?;
            let Some(s) = stage else { return Err(AppError::bad(format!("no stage “{value}” in the record's pipeline"))) };
            body.insert("stage_id".into(), json!(s));
        }
        "opportunity" | "probability" => {
            body.insert(code.into(), if empty { Value::Null } else { json!(value.replace(',', ".").trim()) });
        }
        "title" | "comments" | "source" | "currency" | "close_date" => {
            body.insert(code.into(), if empty { Value::Null } else { json!(value) });
        }
        uf => {
            let defs = crate::crm::fields_custom(state, r.entity_type).await?;
            let d = defs.iter().find(|d| d.code == uf).ok_or_else(|| AppError::bad(format!("unknown field “{uf}”")))?;
            let v = if empty {
                Value::Null
            } else {
                match d.kind.as_str() {
                    "number" => value.replace(',', ".").trim().parse::<f64>().map(|n| json!(n)).map_err(|_| AppError::bad(format!("“{}” needs a number", d.label)))?,
                    "boolean" => json!(["true", "yes", "1"].contains(&value.trim().to_lowercase().as_str())),
                    "list" => d.settings["options"].as_array().and_then(|o| o.iter().find(|x| x["label"].as_str().is_some_and(|l| l.eq_ignore_ascii_case(value.trim()))).map(|x| x["id"].clone())).ok_or_else(|| AppError::bad(format!("“{value}” isn't a choice of “{}”", d.label)))?,
                    "money" => json!({ "amount": value.trim(), "currency": r.currency.clone().unwrap_or_default() }),
                    _ => json!(value),
                }
            };
            body.insert("uf".into(), json!({ uf: v }));
        }
    }
    crate::crm::items::update_item(state, &crate::crm::automation::system_auth(), &t, item, body).await
}

/// Starts the processes a record's creation or change calls for. Automatic changes (robots,
/// processes) start none. Errors are logged.
pub async fn on_record(state: &AppState, auth: &AuthUser, et: i16, item: Uuid, trigger: &str) {
    if auth.id == SYSTEM_USER {
        return;
    }
    let templates: Vec<Uuid> = sqlx::query_scalar!("SELECT t.id FROM wf_templates t WHERE t.crm_entity_type = $1 AND t.trigger = $2 AND t.is_active
           AND (cardinality(t.test_users) = 0 OR $4 = ANY(t.test_users))
           AND NOT EXISTS (SELECT 1 FROM wf_instances w WHERE w.template_id = t.id AND w.crm_item_id = $3 AND w.status IN ('running', 'waiting'))", et, trigger, item, auth.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    for t in templates {
        // Boxed: a process's own changes come back here (and stop at the Automation user).
        if let Err(e) = Box::pin(crate::workflows::start(state, t, crate::workflows::Subject::Crm(item), Some(auth.id))).await {
            tracing::warn!(template = %t, item = %item, error = ?e, "business process could not start on a CRM record");
        }
    }
}

async fn list_templates(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    access::require_any(&state, &auth, t.id, "read").await?;
    let rows: Vec<(Uuid, String, String, Value, bool, Vec<Uuid>)> =
        sqlx::query!(r#"SELECT id, name, trigger, steps, is_active, test_users FROM wf_templates WHERE crm_entity_type = $1 ORDER BY name"#, t.id).map(|r| (r.id, r.name, r.trigger, r.steps, r.is_active, r.test_users)).fetch_all(&state.db).await?;
    let fs = fields(&state, t.id).await?;
    Ok(Json(json!({
        "items": rows.into_iter().map(|(id, n, tr, st, a, tu)| json!({ "id": id, "name": n, "trigger": tr, "steps": st, "is_active": a, "test_users": tu })).collect::<Vec<_>>(),
        "fields": fs,
        "can_manage": access::is_crm_admin(&auth),
    })))
}

async fn create_template(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(b): Json<crate::workflows::TemplateInput>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    if !access::is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    let fs = fields(&state, t.id).await?;
    crate::workflows::insert_template(&state, &auth, crate::workflows::Scope::Crm(t.id), &fs, b).await
}

/// A record's processes (newest first) and those that may be started on it by hand.
async fn record_processes(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let r = load_row(&state, id, false).await?;
    access::require_read(&state, &auth, r.entity_type, id, r.assigned_to).await?;
    let manual: Vec<(Uuid, String)> = sqlx::query!(r#"SELECT id, name FROM wf_templates WHERE crm_entity_type = $1 AND trigger = 'manual' AND is_active AND (cardinality(test_users) = 0 OR $2 = ANY(test_users)) ORDER BY name"#, r.entity_type, auth.id).map(|r| (r.id, r.name))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "processes": crate::workflows::processes_of(&state, crate::workflows::Subject::Crm(id)).await?,
        "can_start": manual.into_iter().map(|(id, n)| json!({ "id": id, "name": n })).collect::<Vec<_>>(),
    })))
}

async fn start_manual(State(state): State<AppState>, auth: AuthUser, Path((id, template)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    let r = load_row(&state, id, false).await?;
    access::require(&state, &auth, r.entity_type, "write", r.assigned_to).await?;
    let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM wf_templates WHERE id = $1 AND crm_entity_type = $2 AND is_active AND (cardinality(test_users) = 0 OR $3 = ANY(test_users))) AS "v!""#, template, r.entity_type, auth.id)
        .fetch_one(&state.db)
        .await?;
    if !ok {
        return Err(AppError::NotFound("process"));
    }
    let running: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM wf_instances WHERE template_id = $1 AND crm_item_id = $2 AND status IN ('running', 'waiting')) AS "v!""#, template, id).fetch_one(&state.db).await?;
    if running {
        return Err(AppError::bad("this process is already running on the record"));
    }
    crate::workflows::start(&state, template, crate::workflows::Subject::Crm(id), Some(auth.id)).await?;
    record_processes(State(state), auth, Path(id)).await
}
