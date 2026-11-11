//! Working together on CRM records:
//! * **Edit history** — each save records which fields changed, from what to what.
//! * **Observers** — employees who follow a record: they may open it and are told about
//!   stage changes and comments.
//! * **Tasks** — tasks linked to records (`task_crm_links`), listed on the record.
//! * **Email templates** — shared or personal, with the same `{{placeholders}}` as robots.

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::{FromRow, QueryBuilder};
use uuid::Uuid;

use super::{
    access::{is_crm_admin, require, require_read},
    broadcast, entity_type_by_id,
    items::{load_row, Row},
    timeline, EntityType, COMPANY,
};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/items/{id}/observers", put(set_observers))
        .route("/crm/items/{id}/chat", post(record_chat))
        .route("/crm/items/{id}/tasks", get(item_tasks))
        .route("/crm/email-templates", get(list_templates).post(create_template))
        .route("/crm/email-templates/{id}", patch(update_template).delete(delete_template))
        .route("/crm/email-templates/{id}/render", post(render_template))
}

// ---------------------------------------------------------------------------
// Edit history
// ---------------------------------------------------------------------------

/// What a record looked like, for comparing before and after a save.
pub struct Snapshot {
    row: Value,
    category: Option<String>,
    multifields: Vec<String>,
    contacts: Vec<String>,
}

const TRACKED: &[&str] = &[
    "title", "honorific", "first_name", "middle_name", "last_name", "job_title", "birthdate", "company_name", "company_kind",
    "industry", "employees", "annual_revenue", "opportunity", "currency", "probability", "close_date", "source", "source_details",
    "comments", "address",
];

pub async fn snapshot(state: &AppState, row: &Row) -> AppResult<Snapshot> {
    let category: Option<String> = match row.category_id {
        Some(c) => sqlx::query_scalar("SELECT name FROM crm_categories WHERE id = $1").bind(c).fetch_optional(&state.db).await?,
        None => None,
    };
    let multifields: Vec<String> = sqlx::query_scalar("SELECT value FROM crm_multifields WHERE item_id = $1 ORDER BY kind, sort")
        .bind(row.id)
        .fetch_all(&state.db)
        .await?;
    let contacts: Vec<String> = sqlx::query_scalar(
        "SELECT i.title FROM crm_item_contacts l JOIN crm_items i ON i.id = l.contact_id WHERE l.item_id = $1 ORDER BY l.is_primary DESC, l.sort",
    )
    .bind(row.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Snapshot { row: serde_json::to_value(row).unwrap_or_default(), category, multifields, contacts })
}

fn list_value(l: &[String]) -> Value {
    if l.is_empty() { Value::Null } else { json!(l.join(", ")) }
}

/// The fields that differ, as `{field, from, to}`. Stage and responsible person have their
/// own timeline entries; a person's or company's title follows from its name fields.
pub fn diff(t: &EntityType, before: &Snapshot, after: &Snapshot) -> Vec<Value> {
    let mut out = Vec::new();
    let mut push = |field: &str, from: &Value, to: &Value| {
        let blank = |v: &Value| v.is_null() || v.as_str() == Some("");
        if from != to && !(blank(from) && blank(to)) {
            out.push(json!({ "field": field, "from": from, "to": to }));
        }
    };
    for k in TRACKED {
        if *k == "title" && (t.is_person() || t.id == COMPANY) {
            continue;
        }
        push(k, &before.row[*k], &after.row[*k]);
    }
    if before.row["company_id"] != after.row["company_id"] {
        push("company_id", &before.row["company_title"], &after.row["company_title"]);
    }
    push("category_id", &json!(before.category), &json!(after.category));
    push("multifields", &list_value(&before.multifields), &list_value(&after.multifields));
    push("contacts", &list_value(&before.contacts), &list_value(&after.contacts));
    let empty = Map::new();
    let (b, a) = (before.row["uf"].as_object().unwrap_or(&empty), after.row["uf"].as_object().unwrap_or(&empty));
    let mut keys: Vec<&String> = b.keys().chain(a.keys()).collect();
    keys.sort();
    keys.dedup();
    for k in keys {
        push(&format!("uf.{k}"), b.get(k).unwrap_or(&Value::Null), a.get(k).unwrap_or(&Value::Null));
    }
    out
}

/// Records a save in the timeline (nothing when nothing tracked changed).
pub async fn record_edit(state: &AppState, t: &EntityType, before: &Snapshot, id: Uuid, actor: Uuid) -> AppResult<Vec<Value>> {
    let after = snapshot(state, &load_row(state, id, false).await?).await?;
    let changes = diff(t, before, &after);
    if !changes.is_empty() {
        timeline(&state.db, id, "edit", Some(actor), None, json!({ "changes": changes })).await?;
    }
    Ok(changes)
}

// ---------------------------------------------------------------------------
// Observers
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
pub struct Person {
    id: Uuid,
    full_name: String,
    avatar_file_id: Option<Uuid>,
}

pub async fn observers(state: &AppState, item: Uuid) -> AppResult<Vec<Person>> {
    Ok(sqlx::query_as(
        "SELECT u.id, u.first_name || ' ' || u.last_name AS full_name, u.avatar_file_id
         FROM crm_item_observers o JOIN users u ON u.id = o.user_id WHERE o.item_id = $1 ORDER BY o.created_at",
    )
    .bind(item)
    .fetch_all(&state.db)
    .await?)
}

/// Tells the record's observers (and, for comments, its responsible person) what happened.
pub async fn tell_followers(state: &AppState, item: Uuid, actor: Uuid, include_responsible: bool, title: String, body: Option<String>) {
    let Ok(row) = load_row(state, item, false).await else { return };
    let Ok(t) = entity_type_by_id(state, row.entity_type).await else { return };
    let mut to: Vec<Uuid> = sqlx::query_scalar("SELECT user_id FROM crm_item_observers WHERE item_id = $1").bind(item).fetch_all(&state.db).await.unwrap_or_default();
    if include_responsible && let Some(a) = row.assigned_to && !to.contains(&a) {
        to.push(a);
    }
    to.retain(|u| *u != actor);
    if to.is_empty() {
        return;
    }
    notify(state, &to, NewNotification {
        kind: "crm.observed",
        title: title.replace("{record}", &format!("{} “{}”", t.name, row.title)),
        body,
        link: Some(format!("/crm/{}/{item}", t.code)),
        actor: Some(actor),
    })
    .await;
}

async fn user_names(state: &AppState, ids: &[Uuid]) -> AppResult<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT first_name || ' ' || last_name FROM users WHERE id = ANY($1) ORDER BY last_name").bind(ids).fetch_all(&state.db).await?)
}

#[derive(Deserialize)]
struct ObserversBody {
    user_ids: Vec<Uuid>,
}

/// Replaces a record's observers. Needs write access to the record.
/// The record's chat (made on first use): its responsible person, creator and observers.
/// Someone else who may read the record joins by becoming an observer.
async fn record_chat(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = load_row(&state, id, false).await?;
    super::access::require_read(&state, &auth, row.entity_type, id, row.assigned_to).await?;
    if auth.is_guest() {
        return Err(AppError::Forbidden);
    }
    let link = crate::im::Link::CrmItem(id);
    if !link.has(&state, auth.id).await? {
        sqlx::query("INSERT INTO crm_item_observers (item_id, user_id, added_by) VALUES ($1, $2, $2) ON CONFLICT DO NOTHING").bind(id).bind(auth.id).execute(&state.db).await?;
        broadcast(&state, row.entity_type, id, false).await;
    }
    Ok(Json(json!({ "chat_id": crate::im::linked_chat(&state, link).await? })))
}

async fn set_observers(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ObserversBody>) -> AppResult<Json<Value>> {
    let row = load_row(&state, id, false).await?;
    require(&state, &auth, row.entity_type, "read", row.assigned_to).await?;
    require(&state, &auth, row.entity_type, "write", row.assigned_to).await?;
    let mut ids = b.user_ids;
    ids.sort();
    ids.dedup();
    if ids.len() > 50 {
        return Err(AppError::bad("at most 50 observers"));
    }
    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND is_active AND NOT is_system").bind(&ids).fetch_one(&state.db).await?;
    if active != ids.len() as i64 {
        return Err(AppError::bad("observers must be active employees"));
    }
    let before: Vec<Uuid> = sqlx::query_scalar("SELECT user_id FROM crm_item_observers WHERE item_id = $1").bind(id).fetch_all(&state.db).await?;
    let added: Vec<Uuid> = ids.iter().copied().filter(|u| !before.contains(u)).collect();
    let removed: Vec<Uuid> = before.iter().copied().filter(|u| !ids.contains(u)).collect();
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM crm_item_observers WHERE item_id = $1 AND user_id = ANY($2)").bind(id).bind(&removed).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO crm_item_observers (item_id, user_id, added_by) SELECT $1, UNNEST($2::uuid[]), $3 ON CONFLICT DO NOTHING")
        .bind(id)
        .bind(&added)
        .bind(auth.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    if !added.is_empty() || !removed.is_empty() {
        let (a, r) = (user_names(&state, &added).await?, user_names(&state, &removed).await?);
        timeline(&state.db, id, "observers", Some(auth.id), None, json!({ "added": a, "removed": r })).await?;
        let t = entity_type_by_id(&state, row.entity_type).await?;
        let to: Vec<Uuid> = added.into_iter().filter(|u| *u != auth.id).collect();
        notify(&state, &to, NewNotification {
            kind: "crm.observed",
            title: format!("You now follow {} “{}”", t.name.to_lowercase(), row.title),
            body: None,
            link: Some(format!("/crm/{}/{id}", t.code)),
            actor: Some(auth.id),
        })
        .await;
        broadcast(&state, row.entity_type, id, false).await;
        crate::im::sync_linked_chat(&state, crate::im::Link::CrmItem(id)).await?;
    }
    Ok(Json(json!({ "observers": observers(&state, id).await? })))
}

// ---------------------------------------------------------------------------
// Tasks of a record
// ---------------------------------------------------------------------------

/// Links a new task to records the creator may read (unknown or hidden ones fail).
pub async fn link_task(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, state: &AppState, auth: &AuthUser, task: Uuid, items: &[Uuid]) -> AppResult<()> {
    for &item in items.iter().take(20) {
        let row = load_row(state, item, false).await?;
        require_read(state, auth, row.entity_type, item, row.assigned_to).await?;
        sqlx::query("INSERT INTO task_crm_links (task_id, item_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(task).bind(item).execute(&mut **tx).await?;
    }
    Ok(())
}

/// After a linked task was created or finished: a timeline entry on each record.
pub async fn task_event(state: &AppState, task: Uuid, event: &str, actor: Uuid) {
    let links: Vec<(Uuid, i64, String)> = sqlx::query_as("SELECT l.item_id, t.number, t.title FROM task_crm_links l JOIN tasks t ON t.id = l.task_id WHERE l.task_id = $1")
        .bind(task)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    for (item, number, title) in links {
        let _ = timeline(&state.db, item, "task", Some(actor), None, json!({ "task_id": task, "number": number, "title": title, "event": event })).await;
        if let Ok(row) = load_row(state, item, false).await {
            broadcast(state, row.entity_type, item, false).await;
        }
    }
}

/// The records a task is linked to that the viewer may open.
#[derive(Serialize, FromRow)]
pub struct LinkedRecord {
    id: Uuid,
    entity_type: i16,
    code: String,
    type_name: String,
    number: i64,
    title: String,
    #[serde(skip)]
    assigned_to: Option<Uuid>,
}

impl LinkedRecord {
    pub fn id(&self) -> Uuid {
        self.id
    }
}

pub async fn task_records(state: &AppState, auth: &AuthUser, task: Uuid) -> AppResult<Vec<LinkedRecord>> {
    let all: Vec<LinkedRecord> = sqlx::query_as(
        "SELECT i.id, i.entity_type, t.code, t.name AS type_name, i.number, i.title, i.assigned_to
         FROM task_crm_links l JOIN crm_items i ON i.id = l.item_id JOIN crm_entity_types t ON t.id = i.entity_type
         WHERE l.task_id = $1 AND i.deleted_at IS NULL ORDER BY l.created_at",
    )
    .bind(task)
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::new();
    for r in all {
        if require_read(state, auth, r.entity_type, r.id, r.assigned_to).await.is_ok() {
            out.push(r);
        }
    }
    Ok(out)
}

/// Tasks linked to a record, as far as the viewer may see them.
async fn item_tasks(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = load_row(&state, id, false).await?;
    require_read(&state, &auth, row.entity_type, id, row.assigned_to).await?;
    let mut qb = QueryBuilder::new(crate::tasks::items::SUMMARY_SELECT);
    qb.push(" AND t.id IN (SELECT task_id FROM task_crm_links WHERE item_id = ").push_bind(id).push(")");
    crate::tasks::access::push_visible(&mut qb, &auth);
    qb.push(" ORDER BY (t.status = 'done'), t.deadline NULLS LAST, t.created_at DESC LIMIT 100");
    let items: Vec<crate::tasks::items::TaskSummary> = qb.build_query_as().fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": items })))
}

// ---------------------------------------------------------------------------
// Email templates
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct Template {
    id: Uuid,
    name: String,
    subject: String,
    body: String,
    entity_types: Vec<i16>,
    owner_id: Option<Uuid>,
    updated_at: DateTime<Utc>,
}

const TEMPLATE_COLUMNS: &str = "id, name, subject, body, entity_types, owner_id, updated_at";

#[derive(Deserialize)]
struct TemplateQuery {
    /// Only templates offered for this record type.
    entity_type: Option<i16>,
}

/// Shared templates and the caller's own.
async fn list_templates(State(state): State<AppState>, auth: AuthUser, Query(q): Query<TemplateQuery>) -> AppResult<Json<Value>> {
    let items: Vec<Template> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TEMPLATE_COLUMNS} FROM crm_email_templates
          WHERE (owner_id IS NULL OR owner_id = $1) AND ($2::smallint IS NULL OR cardinality(entity_types) = 0 OR $2 = ANY(entity_types))
          ORDER BY owner_id NULLS FIRST, lower(name)"
    )))
    .bind(auth.id)
    .bind(q.entity_type)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items, "can_share": is_crm_admin(&auth) })))
}

#[derive(Deserialize)]
struct TemplateBody {
    name: Option<String>,
    subject: Option<String>,
    body: Option<String>,
    entity_types: Option<Vec<i16>>,
    /// Shared with everyone (CRM admins only).
    shared: Option<bool>,
}

async fn check_types(state: &AppState, types: &[i16]) -> AppResult<Vec<i16>> {
    let mut t = types.to_vec();
    t.sort_unstable();
    t.dedup();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crm_entity_types WHERE id = ANY($1)").bind(&t).fetch_one(&state.db).await?;
    if n != t.len() as i64 {
        return Err(AppError::bad("unknown record type"));
    }
    Ok(t)
}

fn text(v: Option<&str>, label: &str, max: usize) -> AppResult<String> {
    let s = v.unwrap_or("").to_string();
    if s.chars().count() > max {
        return Err(AppError::bad(format!("{label} is too long")));
    }
    Ok(s)
}

async fn load_template(state: &AppState, id: Uuid) -> AppResult<Template> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {TEMPLATE_COLUMNS} FROM crm_email_templates WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("template"))
}

/// Personal templates are their owner's; shared ones are for CRM admins to change.
fn may_change(auth: &AuthUser, t: &Template) -> AppResult<()> {
    match t.owner_id {
        Some(o) if o == auth.id => Ok(()),
        None if is_crm_admin(auth) => Ok(()),
        Some(_) => Err(AppError::NotFound("template")),
        None => Err(AppError::Forbidden),
    }
}

async fn create_template(State(state): State<AppState>, auth: AuthUser, Json(b): Json<TemplateBody>) -> AppResult<Json<Template>> {
    let shared = b.shared.unwrap_or(false);
    if shared && !is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crm_email_templates WHERE owner_id IS NOT DISTINCT FROM $1").bind((!shared).then_some(auth.id)).fetch_one(&state.db).await?;
    if n >= 500 {
        return Err(AppError::bad("at most 500 templates"));
    }
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO crm_email_templates (name, subject, body, entity_types, owner_id, created_by) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(required("name", b.name.as_deref().unwrap_or(""), 120)?)
    .bind(text(b.subject.as_deref(), "subject", 300)?)
    .bind(text(b.body.as_deref(), "message", 50_000)?)
    .bind(check_types(&state, &b.entity_types.unwrap_or_default()).await?)
    .bind((!shared).then_some(auth.id))
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(load_template(&state, id).await?))
}

async fn update_template(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<TemplateBody>) -> AppResult<Json<Template>> {
    let cur = load_template(&state, id).await?;
    may_change(&auth, &cur)?;
    let owner = match b.shared {
        Some(true) if cur.owner_id.is_some() => {
            if !is_crm_admin(&auth) {
                return Err(AppError::Forbidden);
            }
            None
        }
        Some(false) if cur.owner_id.is_none() => Some(auth.id),
        _ => cur.owner_id,
    };
    sqlx::query("UPDATE crm_email_templates SET name = $2, subject = $3, body = $4, entity_types = $5, owner_id = $6, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(match &b.name { Some(n) => required("name", n, 120)?, None => cur.name })
        .bind(match &b.subject { Some(s) => text(Some(s), "subject", 300)?, None => cur.subject })
        .bind(match &b.body { Some(s) => text(Some(s), "message", 50_000)?, None => cur.body })
        .bind(match &b.entity_types { Some(t) => check_types(&state, t).await?, None => cur.entity_types })
        .bind(owner)
        .execute(&state.db)
        .await?;
    Ok(Json(load_template(&state, id).await?))
}

async fn delete_template(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let cur = load_template(&state, id).await?;
    may_change(&auth, &cur)?;
    sqlx::query("DELETE FROM crm_email_templates WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct RenderBody {
    item_id: Uuid,
}

/// A template filled in for one record: `{subject, body}`.
async fn render_template(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<RenderBody>) -> AppResult<Json<Value>> {
    let t = load_template(&state, id).await?;
    if t.owner_id.is_some_and(|o| o != auth.id) {
        return Err(AppError::NotFound("template"));
    }
    let row = load_row(&state, b.item_id, false).await?;
    require_read(&state, &auth, row.entity_type, b.item_id, row.assigned_to).await?;
    let ctx = super::automation::context(&state, b.item_id).await?;
    // The sender is whoever writes, not the record's responsible person.
    let mut v = ctx.value;
    let me: Option<(String, String)> = sqlx::query_as("SELECT first_name || ' ' || last_name, email FROM users WHERE id = $1").bind(auth.id).fetch_optional(&state.db).await?;
    if let Some((name, email)) = me {
        v["me"] = json!({ "name": name, "email": email });
    }
    Ok(Json(json!({ "subject": super::automation::fill(&t.subject, &v), "body": super::automation::fill(&t.body, &v) })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(row: Value, contacts: &[&str]) -> Snapshot {
        Snapshot { row, category: Some("Sales".into()), multifields: vec![], contacts: contacts.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn diffs() {
        let deal = EntityType { id: 2, code: "deal".into(), name: "Deal".into(), name_plural: "Deals".into(), has_pipelines: true, has_stages: true, has_amount: true, has_client: true, is_system: true, icon: "funnel".into(), kind: None };
        let a = snap(json!({ "title": "A", "opportunity": "100.0000", "comments": null, "uf": { "tier": "gold" }, "company_id": null, "company_title": null }), &["Ann"]);
        let b = snap(json!({ "title": "B", "opportunity": "100.0000", "comments": "", "uf": { "tier": "silver", "region": "EU" }, "company_id": "x", "company_title": "Acme" }), &["Ann", "Bob"]);
        let d = diff(&deal, &a, &b);
        let fields: Vec<&str> = d.iter().map(|c| c["field"].as_str().unwrap()).collect();
        assert_eq!(fields, vec!["title", "company_id", "contacts", "uf.region", "uf.tier"]);
        assert_eq!(d[1]["to"], "Acme");
        assert_eq!(d[2]["to"], "Ann, Bob");
        assert!(diff(&deal, &a, &a).is_empty());
    }
}
