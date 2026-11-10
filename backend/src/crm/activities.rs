//! Activities: to-dos, calls, meetings and logged emails attached to CRM records, with
//! deadlines, reminders and the counters that drive "what needs my attention".

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use uuid::Uuid;

use super::{access::require, broadcast, entity_type_by_id, items::load_row, timeline};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::{clean, required, Page},
    state::AppState,
};

pub const REMIND_JOB: &str = "crm.reminders";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/activities", get(list).post(create))
        .route("/crm/activities/{id}", patch(update).delete(remove))
        .route("/crm/activities/{id}/complete", post(complete))
        .route("/crm/counters", get(counters))
}

#[derive(Serialize, FromRow)]
pub struct Activity {
    id: Uuid,
    item_id: Uuid,
    item_type: i16,
    item_title: String,
    kind: String,
    subject: String,
    description: Option<String>,
    direction: Option<String>,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    deadline: Option<DateTime<Utc>>,
    responsible_id: Uuid,
    responsible_name: String,
    is_completed: bool,
    completed_at: Option<DateTime<Utc>>,
    result: Option<String>,
    remind_before_minutes: Option<i32>,
    is_overdue: bool,
    created_at: DateTime<Utc>,
}

const SELECT: &str = "SELECT a.id, a.item_id, i.entity_type AS item_type, i.title AS item_title, a.kind, a.subject, a.description,
    a.direction, a.starts_at, a.ends_at, a.deadline, a.responsible_id, u.first_name || ' ' || u.last_name AS responsible_name,
    a.is_completed, a.completed_at, a.result, a.remind_before_minutes,
    (NOT a.is_completed AND a.deadline < now()) AS is_overdue, a.created_at
    FROM crm_activities a JOIN crm_items i ON i.id = a.item_id JOIN users u ON u.id = a.responsible_id
    WHERE i.deleted_at IS NULL";

async fn load(state: &AppState, id: Uuid) -> AppResult<Activity> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(SELECT);
    qb.push(" AND a.id = ").push_bind(id);
    qb.build_query_as().fetch_optional(&state.db).await?.ok_or(AppError::NotFound("activity"))
}

#[derive(Deserialize)]
struct ListQuery {
    item_id: Option<Uuid>,
    /// mine: activities I'm responsible for (default when no item is given)
    mine: Option<String>,
    /// open | completed | overdue | today | all
    state: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Page<Activity>>> {
    let limit: i64 = q.limit.as_deref().and_then(|v| v.parse().ok()).unwrap_or(50).clamp(1, 200);
    let offset: i64 = q.offset.as_deref().and_then(|v| v.parse().ok()).unwrap_or(0).max(0);
    let build = |select: &str| -> QueryBuilder<Postgres> {
        let mut qb = QueryBuilder::new(select);
        if let Some(item) = q.item_id {
            qb.push(" AND a.item_id = ").push_bind(item);
        }
        if q.item_id.is_none() || q.mine.as_deref() == Some("true") {
            qb.push(" AND a.responsible_id = ").push_bind(auth.id);
        }
        match q.state.as_deref().unwrap_or("open") {
            "completed" => qb.push(" AND a.is_completed"),
            "overdue" => qb.push(" AND NOT a.is_completed AND a.deadline < now()"),
            "today" => qb.push(" AND NOT a.is_completed AND a.deadline::date = CURRENT_DATE"),
            "all" => &mut qb,
            _ => qb.push(" AND NOT a.is_completed"),
        };
        qb
    };
    if let Some(item) = q.item_id {
        let r = load_row(&state, item, false).await?;
        super::access::require_read(&state, &auth, r.entity_type, item, r.assigned_to).await?;
    }
    let total: i64 = build("SELECT COUNT(*) FROM crm_activities a JOIN crm_items i ON i.id = a.item_id WHERE i.deleted_at IS NULL")
        .build_query_scalar()
        .fetch_one(&state.db)
        .await?;
    let mut qb = build(SELECT);
    qb.push(" ORDER BY a.is_completed, a.deadline ASC NULLS LAST, a.created_at DESC LIMIT ").push_bind(limit).push(" OFFSET ").push_bind(offset);
    let items: Vec<Activity> = qb.build_query_as().fetch_all(&state.db).await?;
    Ok(Json(Page { items, total, limit, offset }))
}

#[derive(Deserialize)]
struct Input {
    item_id: Option<Uuid>,
    kind: Option<String>,
    subject: Option<String>,
    description: Option<String>,
    direction: Option<String>,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    deadline: Option<DateTime<Utc>>,
    responsible_id: Option<Uuid>,
    remind_before_minutes: Option<i32>,
}

fn validate(b: &Input) -> AppResult<()> {
    if let Some(k) = &b.kind
        && !["todo", "call", "meeting", "email"].contains(&k.as_str()) {
            return Err(AppError::bad("kind must be todo, call, meeting or email"));
        }
    if let Some(d) = &b.direction
        && !["incoming", "outgoing"].contains(&d.as_str()) {
            return Err(AppError::bad("direction must be incoming or outgoing"));
        }
    if let (Some(s), Some(e)) = (b.starts_at, b.ends_at)
        && e < s {
            return Err(AppError::bad("the end is before the start"));
        }
    if b.remind_before_minutes.is_some_and(|m| !(0..=60 * 24 * 14).contains(&m)) {
        return Err(AppError::bad("reminder must be within two weeks of the deadline"));
    }
    Ok(())
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<Input>) -> AppResult<Json<Activity>> {
    validate(&b)?;
    let item = b.item_id.ok_or_else(|| AppError::bad("item_id is required"))?;
    let r = load_row(&state, item, false).await?;
    super::access::require_read(&state, &auth, r.entity_type, item, r.assigned_to).await?;
    let kind = b.kind.clone().unwrap_or_else(|| "todo".into());
    let subject = required("subject", b.subject.as_deref().unwrap_or(""), 500)?;
    let responsible = b.responsible_id.unwrap_or(auth.id);
    // Meetings default their deadline to the start; logged calls/emails are done already.
    let deadline = b.deadline.or(b.starts_at);
    let done_now = (kind == "call" || kind == "email") && deadline.is_none();
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO crm_activities (item_id, kind, subject, description, direction, starts_at, ends_at, deadline,
             responsible_id, created_by, remind_before_minutes, is_completed, completed_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, CASE WHEN $12 THEN now() END) RETURNING id",
    )
    .bind(item)
    .bind(&kind)
    .bind(&subject)
    .bind(clean(b.description))
    .bind(b.direction)
    .bind(b.starts_at)
    .bind(b.ends_at)
    .bind(deadline)
    .bind(responsible)
    .bind(auth.id)
    .bind(b.remind_before_minutes)
    .bind(done_now)
    .fetch_one(&state.db)
    .await?;
    timeline(&state.db, item, "activity", Some(auth.id), Some(subject.clone()), json!({ "activity_id": id, "kind": kind, "event": "created" })).await?;
    sqlx::query("UPDATE crm_items SET last_activity_at = now() WHERE id = $1").bind(item).execute(&state.db).await?;
    if responsible != auth.id {
        let t = entity_type_by_id(&state, r.entity_type).await?;
        notify(
            &state,
            &[responsible],
            NewNotification {
                kind: "crm.activity",
                title: format!("New {kind} for you: {subject}"),
                body: Some(format!("{}: {}", t.name, r.title)),
                link: Some(format!("/crm/{}/{item}", t.code)),
                actor: Some(auth.id),
            },
        )
        .await;
    }
    broadcast(&state, r.entity_type, item, false).await;
    let a = load(&state, id).await?;
    crate::api::webhooks::emit(&state, "crm.activity.created", serde_json::to_value(&a).unwrap_or_default()).await;
    Ok(Json(a))
}

async fn editable(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Activity> {
    let a = load(state, id).await?;
    let r = load_row(state, a.item_id, false).await?;
    require(state, auth, r.entity_type, "read", r.assigned_to).await?;
    if a.responsible_id != auth.id && !super::access::visible(state, auth, r.entity_type, "write").await?.allows(r.assigned_to) {
        return Err(AppError::Forbidden);
    }
    Ok(a)
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Input>) -> AppResult<Json<Activity>> {
    validate(&b)?;
    let a = editable(&state, &auth, id).await?;
    sqlx::query(
        "UPDATE crm_activities SET subject = $1, description = $2, direction = $3, starts_at = $4, ends_at = $5,
             deadline = $6, responsible_id = $7, remind_before_minutes = $8,
             reminded_at = CASE WHEN $6 IS DISTINCT FROM deadline THEN NULL ELSE reminded_at END, updated_at = now()
         WHERE id = $9",
    )
    .bind(match &b.subject {
        Some(s) => required("subject", s, 500)?,
        None => a.subject.clone(),
    })
    .bind(b.description.or(a.description))
    .bind(b.direction.or(a.direction))
    .bind(b.starts_at.or(a.starts_at))
    .bind(b.ends_at.or(a.ends_at))
    .bind(b.deadline.or(a.deadline))
    .bind(b.responsible_id.unwrap_or(a.responsible_id))
    .bind(b.remind_before_minutes.or(a.remind_before_minutes))
    .bind(id)
    .execute(&state.db)
    .await?;
    broadcast(&state, a.item_type, a.item_id, false).await;
    Ok(Json(load(&state, id).await?))
}

#[derive(Deserialize)]
struct Complete {
    result: Option<String>,
    /// Re-open instead of completing.
    #[serde(default)]
    reopen: bool,
}

async fn complete(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Complete>) -> AppResult<Json<Activity>> {
    let a = editable(&state, &auth, id).await?;
    sqlx::query("UPDATE crm_activities SET is_completed = $1, completed_at = CASE WHEN $1 THEN now() END, result = COALESCE($2, result), updated_at = now() WHERE id = $3")
        .bind(!b.reopen)
        .bind(clean(b.result.clone()))
        .bind(id)
        .execute(&state.db)
        .await?;
    if !b.reopen {
        timeline(&state.db, a.item_id, "activity", Some(auth.id), Some(a.subject.clone()), json!({ "activity_id": id, "kind": a.kind, "event": "completed", "result": b.result })).await?;
        sqlx::query("UPDATE crm_items SET last_activity_at = now() WHERE id = $1").bind(a.item_id).execute(&state.db).await?;
    }
    broadcast(&state, a.item_type, a.item_id, false).await;
    if !b.reopen {
        let kind = a.kind.clone();
        super::automation::fire(&state, a.item_id, "activity_done", move |p| p.get("kind").and_then(Value::as_str).is_none_or(|k| k == kind)).await;
    }
    let a = load(&state, id).await?;
    crate::api::webhooks::emit(&state, "crm.activity.completed", serde_json::to_value(&a).unwrap_or_default()).await;
    Ok(Json(a))
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let a = editable(&state, &auth, id).await?;
    sqlx::query("DELETE FROM crm_activities WHERE id = $1").bind(id).execute(&state.db).await?;
    broadcast(&state, a.item_type, a.item_id, false).await;
    Ok(Json(json!({ "ok": true })))
}

/// Per record type: my records needing attention (overdue activity, activity due today,
/// open records with no planned activity), plus my overdue/today activity totals.
async fn counters(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let rows: Vec<(i16, i64, i64, i64)> = sqlx::query_as(
        "SELECT i.entity_type,
            COUNT(*) FILTER (WHERE EXISTS (SELECT 1 FROM crm_activities a WHERE a.item_id = i.id AND NOT a.is_completed AND a.deadline < now())),
            COUNT(*) FILTER (WHERE EXISTS (SELECT 1 FROM crm_activities a WHERE a.item_id = i.id AND NOT a.is_completed AND a.deadline::date = CURRENT_DATE)),
            COUNT(*) FILTER (WHERE i.entity_type IN (SELECT id FROM crm_entity_types WHERE has_stages) AND NOT i.is_closed
                             AND NOT EXISTS (SELECT 1 FROM crm_activities a WHERE a.item_id = i.id AND NOT a.is_completed))
         FROM crm_items i WHERE i.assigned_to = $1 AND i.deleted_at IS NULL AND NOT i.is_closed
         GROUP BY i.entity_type",
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    let (overdue, today): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE a.deadline < now()), COUNT(*) FILTER (WHERE a.deadline::date = CURRENT_DATE)
         FROM crm_activities a JOIN crm_items i ON i.id = a.item_id
         WHERE a.responsible_id = $1 AND NOT a.is_completed AND i.deleted_at IS NULL",
    )
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    let mut per = serde_json::Map::new();
    for (t, o, td, idle) in rows {
        per.insert(t.to_string(), json!({ "overdue": o, "today": td, "idle": idle, "attention": o + td + idle }));
    }
    Ok(Json(json!({ "types": per, "activities_overdue": overdue, "activities_today": today })))
}

/// Background job: reminders before an activity's deadline.
pub async fn run_reminders(state: &AppState) -> Result<()> {
    let due: Vec<(Uuid, Uuid, String, String, Uuid, i16, String)> = sqlx::query_as(
        "UPDATE crm_activities a SET reminded_at = now()
         FROM crm_items i
         WHERE i.id = a.item_id AND NOT a.is_completed AND a.reminded_at IS NULL AND a.deadline IS NOT NULL
           AND a.deadline - make_interval(mins => COALESCE(a.remind_before_minutes, 15)) <= now()
           AND a.deadline > now() - interval '1 hour' AND i.deleted_at IS NULL
         RETURNING a.id, a.responsible_id, a.subject, a.kind, a.item_id, i.entity_type, i.title",
    )
    .fetch_all(&state.db)
    .await?;
    for (_, user, subject, kind, item, t, title) in due {
        let code = match t {
            1 => "lead",
            2 => "deal",
            3 => "contact",
            _ => "company",
        };
        notify(
            state,
            &[user],
            NewNotification {
                kind: "crm.reminder",
                title: format!("Coming up: {subject}"),
                body: Some(format!("{} · {title}", kind)),
                link: Some(format!("/crm/{code}/{item}")),
                actor: None,
            },
        )
        .await;
    }
    Ok(())
}
