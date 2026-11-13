//! The parts of a task: checklist, comments, attachments, watching, time tracking, history.

use axum::{
    extract::{Path, State},
    routing::{delete, get, patch, post},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    access::{load_task, TaskAccess},
    broadcast_changed,
    items::attach_owned_file,
    notify_task, participants, record_history, touch, user_name,
};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    realtime,
    routes::{clean, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tasks/{id}/checklist", post(add_item))
        .route("/tasks/{id}/checklist/{item}", patch(update_item).delete(delete_item))
        .route("/tasks/{id}/comments", get(list_comments).post(add_comment))
        .route("/tasks/{id}/comments/{comment}", patch(edit_comment).delete(delete_comment))
        .route("/tasks/{id}/files", post(attach_file))
        .route("/tasks/{id}/files/{file}", delete(detach_file))
        .route("/tasks/{id}/watch", post(watch).delete(unwatch))
        .route("/tasks/{id}/chat", post(task_chat))
        .route("/tasks/{id}/time", get(list_time).post(add_time))
        .route("/tasks/{id}/time/{entry}", delete(delete_time))
        .route("/tasks/{id}/timer/start", post(start_timer))
        .route("/tasks/{id}/timer/stop", post(stop_timer))
        .route("/tasks/timer", get(my_timer))
        .route("/tasks/{id}/history", get(history))
}

fn require(ok: bool) -> AppResult<()> {
    if ok { Ok(()) } else { Err(AppError::Forbidden) }
}

// ---------------------------------------------------------------------------
// Checklist
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct NewItem {
    title: String,
    /// Who does this point.
    assignee_id: Option<Uuid>,
}

/// Gives a checklist point to someone: they join the task as an assistant if they aren't on
/// it, and are told.
async fn assign_item(state: &AppState, auth: &AuthUser, task: Uuid, item: Uuid, title: &str, user: Option<Uuid>) -> AppResult<()> {
    if let Some(u) = user {
        let active: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND is_active) AS "v!""#, u).fetch_one(&state.db).await?;
        if !active {
            return Err(AppError::bad("choose an active employee"));
        }
    }
    sqlx::query!("UPDATE task_checklist_items SET assignee_id = $2 WHERE id = $1", item, user).execute(&state.db).await?;
    if let Some(u) = user {
        if !super::participants(state, task).await.contains(&u) {
            sqlx::query!("INSERT INTO task_members (task_id, user_id, role) VALUES ($1, $2, 'assistant') ON CONFLICT DO NOTHING", task, u).execute(&state.db).await?;
        }
        let t: String = sqlx::query_scalar!("SELECT title FROM tasks WHERE id = $1", task).fetch_one(&state.db).await?;
        notify_task(state, &[u], "task.checklist_assigned", format!("A checklist point is yours: {title}"), Some(t), task, auth.id).await;
    }
    Ok(())
}

async fn add_item(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(body): Json<NewItem>) -> AppResult<Json<Value>> {
    let (core, a) = load_task(&state, &auth, id).await?;
    require(a.participate)?;
    let title = required("checklist item", &body.title, 1000)?;
    let item: Uuid = sqlx::query_scalar!("INSERT INTO task_checklist_items (task_id, title, sort)
         VALUES ($1, $2, COALESCE((SELECT MAX(sort) FROM task_checklist_items WHERE task_id = $1), 0) + 1) RETURNING id", id, &title)
    .fetch_one(&state.db)
    .await?;
    if body.assignee_id.is_some() {
        assign_item(&state, &auth, id, item, &title, body.assignee_id).await?;
    }
    touch(&state, id).await;
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(json!({ "id": item })))
}

#[derive(Deserialize)]
struct UpdateItem {
    title: Option<String>,
    is_done: Option<bool>,
    sort: Option<f64>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    assignee_id: Option<Option<Uuid>>,
}

async fn update_item(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, item)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateItem>,
) -> AppResult<Json<Value>> {
    let (core, a) = load_task(&state, &auth, id).await?;
    require(a.participate)?;
    let current: (String, bool) = sqlx::query!(r#"SELECT title, is_done FROM task_checklist_items WHERE id = $1 AND task_id = $2"#, item, id).map(|r| (r.title, r.is_done))
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("checklist item"))?;
    if let Some(t) = &body.title {
        sqlx::query!("UPDATE task_checklist_items SET title = $1 WHERE id = $2", required("checklist item", t, 1000)?, item)
            .execute(&state.db)
            .await?;
    }
    if let Some(done) = body.is_done.filter(|d| *d != current.1) {
        sqlx::query(
            "UPDATE task_checklist_items SET is_done = $1,
                 done_by = CASE WHEN $1 THEN $2 ELSE NULL END, done_at = CASE WHEN $1 THEN now() ELSE NULL END
             WHERE id = $3",
        )
        .bind(done)
        .bind(auth.id)
        .bind(item)
        .execute(&state.db)
        .await?;
        record_history(&state.db, id, auth.id, if done { "checklist_done" } else { "checklist_undone" }, None, Some(current.0.clone())).await?;
        let left: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) FILTER (WHERE NOT is_done) AS "v!" FROM task_checklist_items WHERE task_id = $1"#, id).fetch_one(&state.db).await?;
        if done && left == 0 {
            super::automation::fire(&state, id, "checklist_done", auth.id).await;
        }
    }
    if let Some(s) = body.sort {
        sqlx::query!("UPDATE task_checklist_items SET sort = $1 WHERE id = $2", s, item).execute(&state.db).await?;
    }
    if let Some(a) = body.assignee_id {
        assign_item(&state, &auth, id, item, &current.0, a).await?;
    }
    touch(&state, id).await;
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_item(State(state): State<AppState>, auth: AuthUser, Path((id, item)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    let (core, a) = load_task(&state, &auth, id).await?;
    require(a.participate)?;
    sqlx::query!("DELETE FROM task_checklist_items WHERE id = $1 AND task_id = $2", item, id)
        .execute(&state.db)
        .await?;
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Comments
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow, Clone)]
pub struct Comment {
    id: Uuid,
    task_id: Uuid,
    author_id: Option<Uuid>,
    author_name: Option<String>,
    author_avatar_file_id: Option<Uuid>,
    body: String,
    edited_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

const COMMENT_SELECT: &str = "SELECT c.id, c.task_id, c.author_id,
    CASE WHEN u.id IS NULL THEN NULL ELSE u.first_name || ' ' || u.last_name END AS author_name,
    u.avatar_file_id AS author_avatar_file_id, c.body, c.edited_at, c.created_at
    FROM task_comments c LEFT JOIN users u ON u.id = c.author_id";

async fn load_comment(state: &AppState, id: Uuid) -> AppResult<Comment> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{COMMENT_SELECT} WHERE c.id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("comment"))
}

async fn list_comments(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Vec<Comment>>> {
    load_task(&state, &auth, id).await?;
    Ok(Json(
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{COMMENT_SELECT} WHERE c.task_id = $1 ORDER BY c.created_at LIMIT 1000")))
            .bind(id)
            .fetch_all(&state.db)
            .await?,
    ))
}

/// Mentions are written as `@[Display Name](user-uuid)` by the editor.
fn mentioned_users(body: &str) -> Vec<Uuid> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("@[") {
        rest = &rest[start + 2..];
        let Some(close) = rest.find("](") else { break };
        let after = &rest[close + 2..];
        if after.len() >= 37 && after.as_bytes()[36] == b')'
            && let Ok(id) = Uuid::parse_str(&after[..36])
                && !out.contains(&id) {
                    out.push(id);
                }
        rest = after;
    }
    out
}

/// Plain-text preview of a comment for notifications (mentions shown as @Name).
fn preview(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(start) = rest.find("@[") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        match (tail.find("]("), tail.find(')')) {
            (Some(close), Some(end)) if close < end => {
                out.push('@');
                out.push_str(&tail[..close]);
                rest = &tail[end + 1..];
            }
            _ => {
                out.push_str("@[");
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    let flat: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 140 { format!("{}…", flat.chars().take(140).collect::<String>()) } else { flat }
}

#[derive(Deserialize)]
struct NewComment {
    body: String,
}

async fn add_comment(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(body): Json<NewComment>) -> AppResult<Json<Comment>> {
    let (core, _) = load_task(&state, &auth, id).await?;
    let text = required("comment", &body.body, 20_000)?;

    // People mentioned who can't see the task yet become watchers, so the link works for them.
    let mentioned = mentioned_users(&text);
    let mut newly_watching = Vec::new();
    for user in &mentioned {
        let already: bool = participants(&state, id).await.contains(user);
        if !already {
            let added = sqlx::query!("INSERT INTO task_members (task_id, user_id, role)
                 SELECT $1, id, 'watcher' FROM users WHERE id = $2 AND is_active ON CONFLICT DO NOTHING", id, user)
            .execute(&state.db)
            .await?
            .rows_affected();
            if added > 0 {
                newly_watching.push(*user);
            }
        }
    }

    let cid: Uuid = sqlx::query_scalar!("INSERT INTO task_comments (task_id, author_id, body) VALUES ($1, $2, $3) RETURNING id", id, auth.id, &text)
        .fetch_one(&state.db)
        .await?;
    touch(&state, id).await;
    let comment = load_comment(&state, cid).await?;
    super::automation::fire(&state, id, "comment", auth.id).await;

    let author = user_name(&state, auth.id).await;
    let snippet = Some(preview(&text));
    notify_task(&state, &mentioned, "task.mention", format!("{author} mentioned you in {}", core.title), snippet.clone(), id, auth.id).await;
    let others: Vec<Uuid> = participants(&state, id).await.into_iter().filter(|u| !mentioned.contains(u)).collect();
    notify_task(&state, &others, "task.comment", format!("{author} commented on {}", core.title), snippet, id, auth.id).await;

    let mut audience = participants(&state, id).await;
    if let Some(p) = core.project_id {
        let members: Vec<Uuid> = sqlx::query_scalar!("SELECT user_id FROM project_members WHERE project_id = $1", p)
            .fetch_all(&state.db)
            .await?;
        audience.extend(members);
    }
    audience.sort();
    audience.dedup();
    realtime::publish(&state, Some(audience), "task.comment", serde_json::to_value(&comment).unwrap()).await;
    if !newly_watching.is_empty() {
        broadcast_changed(&state, id, core.project_id, false).await;
    }
    Ok(Json(comment))
}

async fn edit_comment(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Json(body): Json<NewComment>,
) -> AppResult<Json<Comment>> {
    let (core, _) = load_task(&state, &auth, id).await?;
    let existing = load_comment(&state, cid).await?;
    if existing.task_id != id {
        return Err(AppError::NotFound("comment"));
    }
    if existing.author_id != Some(auth.id) {
        return Err(AppError::bad("you can only edit your own comments"));
    }
    sqlx::query!("UPDATE task_comments SET body = $1, edited_at = now() WHERE id = $2", required("comment", &body.body, 20_000)?, cid)
        .execute(&state.db)
        .await?;
    let comment = load_comment(&state, cid).await?;
    realtime::publish(&state, Some(participants(&state, id).await), "task.comment", serde_json::to_value(&comment).unwrap()).await;
    let _ = core;
    Ok(Json(comment))
}

async fn delete_comment(State(state): State<AppState>, auth: AuthUser, Path((id, cid)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    let (_, a) = load_task(&state, &auth, id).await?;
    let existing = load_comment(&state, cid).await?;
    if existing.task_id != id {
        return Err(AppError::NotFound("comment"));
    }
    if existing.author_id != Some(auth.id) && !a.edit {
        return Err(AppError::Forbidden);
    }
    sqlx::query!("DELETE FROM task_comments WHERE id = $1", cid).execute(&state.db).await?;
    realtime::publish(
        &state,
        Some(participants(&state, id).await),
        "task.comment_deleted",
        json!({ "id": cid, "task_id": id }),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Attachments & watching
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AttachFile {
    file_id: Uuid,
}

async fn attach_file(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(body): Json<AttachFile>) -> AppResult<Json<Value>> {
    let (core, a) = load_task(&state, &auth, id).await?;
    require(a.participate)?;
    let mut tx = state.db.begin().await?;
    attach_owned_file(&mut tx, id, body.file_id, auth.id).await?;
    let name: String = sqlx::query_scalar!("SELECT original_name FROM files WHERE id = $1", body.file_id).fetch_one(&mut *tx).await?;
    record_history(&mut *tx, id, auth.id, "file_added", None, Some(name)).await?;
    tx.commit().await?;
    touch(&state, id).await;
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(json!({ "ok": true })))
}

async fn detach_file(State(state): State<AppState>, auth: AuthUser, Path((id, file)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    let (core, a) = load_task(&state, &auth, id).await?;
    let added_by: Option<Option<Uuid>> = sqlx::query_scalar!(r#"SELECT added_by AS "added_by?" FROM task_files WHERE task_id = $1 AND file_id = $2"#, id, file)
        .fetch_optional(&state.db)
        .await?;
    let Some(added_by) = added_by else { return Err(AppError::NotFound("attachment")) };
    require(a.edit || added_by == Some(auth.id))?;
    sqlx::query!("DELETE FROM task_files WHERE task_id = $1 AND file_id = $2", id, file).execute(&state.db).await?;
    let name: Option<String> = sqlx::query_scalar!("SELECT original_name FROM files WHERE id = $1", file).fetch_optional(&state.db).await?;
    record_history(&state.db, id, auth.id, "file_removed", None, name).await?;
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(json!({ "ok": true })))
}

/// The task's chat (made on first use), for the people on the task: a call in it is the
/// task's call.
async fn task_chat(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    load_task(&state, &auth, id).await?;
    let link = crate::im::Link::Task(id);
    if !link.has(&state, auth.id).await? {
        return Err(AppError::bad("only the people on the task can use its chat: watch it to join in"));
    }
    Ok(Json(json!({ "chat_id": crate::im::linked_chat(&state, link).await? })))
}

async fn watch(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (core, a) = load_task(&state, &auth, id).await?;
    if !(a.is_creator || a.is_responsible || a.is_assistant) {
        sqlx::query!("INSERT INTO task_members (task_id, user_id, role) VALUES ($1, $2, 'watcher') ON CONFLICT DO NOTHING", id, auth.id)
            .execute(&state.db)
            .await?;
        broadcast_changed(&state, id, core.project_id, false).await;
    }
    Ok(Json(json!({ "ok": true })))
}

async fn unwatch(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (core, _) = load_task(&state, &auth, id).await?;
    sqlx::query!("DELETE FROM task_members WHERE task_id = $1 AND user_id = $2 AND role = 'watcher'", id, auth.id)
        .execute(&state.db)
        .await?;
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Time tracking
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct TimeEntry {
    id: Uuid,
    user_id: Uuid,
    user_name: String,
    started_at: DateTime<Utc>,
    stopped_at: Option<DateTime<Utc>>,
    seconds: i32,
    comment: Option<String>,
}

fn can_track(a: &TaskAccess) -> bool {
    a.participate || a.is_creator
}

async fn list_time(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Vec<TimeEntry>>> {
    load_task(&state, &auth, id).await?;
    Ok(Json(
        sqlx::query_as(
            "SELECT e.id, e.user_id, u.first_name || ' ' || u.last_name AS user_name, e.started_at, e.stopped_at, e.seconds, e.comment
             FROM task_time_entries e JOIN users u ON u.id = e.user_id
             WHERE e.task_id = $1 ORDER BY e.started_at DESC LIMIT 500",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await?,
    ))
}

/// Stops the user's running timer wherever it is.
async fn stop_running(state: &AppState, user: Uuid) -> AppResult<Option<Uuid>> {
    Ok(sqlx::query_scalar!("UPDATE task_time_entries
         SET stopped_at = now(), seconds = GREATEST(1, EXTRACT(EPOCH FROM (now() - started_at))::int)
         WHERE user_id = $1 AND stopped_at IS NULL RETURNING task_id", user)
    .fetch_optional(&state.db)
    .await?)
}

/// Called when a task is finished: everyone's timers on it stop.
pub async fn stop_all_timers_on(state: &AppState, task: Uuid) -> AppResult<()> {
    sqlx::query!("UPDATE task_time_entries
         SET stopped_at = now(), seconds = GREATEST(1, EXTRACT(EPOCH FROM (now() - started_at))::int)
         WHERE task_id = $1 AND stopped_at IS NULL", task)
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn start_timer(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (core, a) = load_task(&state, &auth, id).await?;
    require(can_track(&a))?;
    if core.status == "done" {
        return Err(AppError::bad("this task is already done"));
    }
    if let Some(prev) = stop_running(&state, auth.id).await? {
        realtime::publish(&state, Some(vec![auth.id]), "timer.changed", json!({ "task_id": prev, "running": false })).await;
    }
    sqlx::query!("INSERT INTO task_time_entries (task_id, user_id, started_at) VALUES ($1, $2, now())", id, auth.id)
        .execute(&state.db)
        .await?;
    // Starting the clock on a new task means work has started.
    if core.status == "new" && a.participate {
        sqlx::query!("UPDATE tasks SET status = 'in_progress', updated_at = now() WHERE id = $1 AND status = 'new'", id)
            .execute(&state.db)
            .await?;
        record_history(&state.db, id, auth.id, "status", Some("new".into()), Some("in_progress".into())).await?;
    }
    touch(&state, id).await;
    realtime::publish(&state, Some(vec![auth.id]), "timer.changed", json!({ "task_id": id, "running": true })).await;
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(json!({ "ok": true })))
}

async fn stop_timer(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (core, _) = load_task(&state, &auth, id).await?;
    let stopped = sqlx::query!("UPDATE task_time_entries
         SET stopped_at = now(), seconds = GREATEST(1, EXTRACT(EPOCH FROM (now() - started_at))::int)
         WHERE user_id = $1 AND task_id = $2 AND stopped_at IS NULL", auth.id, id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if stopped == 0 {
        return Err(AppError::bad("no timer is running on this task"));
    }
    realtime::publish(&state, Some(vec![auth.id]), "timer.changed", json!({ "task_id": id, "running": false })).await;
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(json!({ "ok": true })))
}

/// The caller's running timer, for the header indicator.
async fn my_timer(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let row: Option<(Uuid, i64, String, DateTime<Utc>)> = sqlx::query!(r#"SELECT t.id, t.number, t.title, e.started_at FROM task_time_entries e JOIN tasks t ON t.id = e.task_id
         WHERE e.user_id = $1 AND e.stopped_at IS NULL"#, auth.id).map(|r| (r.id, r.number, r.title, r.started_at))
    .fetch_optional(&state.db)
    .await?;
    Ok(Json(match row {
        Some((id, number, title, started_at)) => json!({ "task_id": id, "number": number, "title": title, "started_at": started_at }),
        None => Value::Null,
    }))
}

#[derive(Deserialize)]
struct ManualTime {
    /// Duration in minutes.
    minutes: i32,
    date: Option<NaiveDate>,
    comment: Option<String>,
}

async fn add_time(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(body): Json<ManualTime>) -> AppResult<Json<Value>> {
    let (core, a) = load_task(&state, &auth, id).await?;
    require(can_track(&a))?;
    if !(1..=24 * 60).contains(&body.minutes) {
        return Err(AppError::bad("time must be between 1 minute and 24 hours"));
    }
    let start = body
        .date
        .and_then(|d| d.and_hms_opt(9, 0, 0))
        .map(|d| d.and_utc())
        .unwrap_or_else(Utc::now);
    let seconds = body.minutes * 60;
    sqlx::query(
        "INSERT INTO task_time_entries (task_id, user_id, started_at, stopped_at, seconds, comment)
         VALUES ($1, $2, $3, $3 + make_interval(secs => $4), $5, $6)",
    )
    .bind(id)
    .bind(auth.id)
    .bind(start)
    .bind(seconds as f64)
    .bind(seconds)
    .bind(clean(body.comment))
    .execute(&state.db)
    .await?;
    touch(&state, id).await;
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_time(State(state): State<AppState>, auth: AuthUser, Path((id, entry)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    let (core, a) = load_task(&state, &auth, id).await?;
    let owner: Option<Uuid> = sqlx::query_scalar!("SELECT user_id FROM task_time_entries WHERE id = $1 AND task_id = $2", entry, id)
        .fetch_optional(&state.db)
        .await?;
    let Some(owner) = owner else { return Err(AppError::NotFound("time entry")) };
    require(owner == auth.id || a.edit)?;
    sqlx::query!("DELETE FROM task_time_entries WHERE id = $1", entry).execute(&state.db).await?;
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct HistoryEntry {
    id: Uuid,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    field: String,
    old_value: Option<String>,
    new_value: Option<String>,
    created_at: DateTime<Utc>,
}

async fn history(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Vec<HistoryEntry>>> {
    load_task(&state, &auth, id).await?;
    Ok(Json(
        sqlx::query_as(
            "SELECT h.id, h.actor_id, CASE WHEN u.id IS NULL THEN NULL ELSE u.first_name || ' ' || u.last_name END AS actor_name,
                    h.field, h.old_value, h.new_value, h.created_at
             FROM task_history h LEFT JOIN users u ON u.id = h.actor_id
             WHERE h.task_id = $1 ORDER BY h.created_at DESC LIMIT 500",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mentions_and_preview() {
        let id = "0b9f6a52-8e3c-4c1e-9d1f-2b0e8e2a7c11";
        let body = format!("Hi @[Ada Admin]({id}), see @[bad](nope) and @[Ada Admin]({id}) again");
        assert_eq!(mentioned_users(&body), vec![Uuid::parse_str(id).unwrap()]);
        assert_eq!(preview(&body), "Hi @Ada Admin, see @bad and @Ada Admin again");
    }
}
