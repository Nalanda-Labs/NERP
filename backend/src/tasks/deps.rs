//! Task dependencies ("A is blocked by B", finish-to-start) and Gantt data.
//!
//! When a task's dates move later, tasks that depend on it and would now start before it
//! ends are shifted by the same amount, recursively. Only tasks the actor may edit are
//! shifted; any others are left alone, and the Gantt chart shows the conflict.

use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::collections::{HashSet, VecDeque};
use uuid::Uuid;

use super::{
    access::{load_task, project_access, push_visible},
    broadcast_changed,
    items::{TaskSummary, SUMMARY_SELECT},
    notify_task, record_history, user_name,
};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tasks/gantt", get(gantt))
        .route("/tasks/{id}/dependencies", post(add))
        .route("/tasks/{id}/dependencies/{other}", delete(remove))
}

#[derive(Serialize, FromRow)]
pub struct DepRef {
    pub id: Uuid,
    pub number: i64,
    pub title: String,
    pub status: String,
    pub deadline: Option<DateTime<Utc>>,
}

/// Tasks this one waits for, and tasks waiting for this one (only those the user can see).
pub async fn links(state: &AppState, auth: &AuthUser, task: Uuid) -> AppResult<(Vec<DepRef>, Vec<DepRef>)> {
    let mut out = (Vec::new(), Vec::new());
    for (sql, slot) in [
        ("SELECT t.id, t.number, t.title, t.status, t.deadline FROM task_dependencies d JOIN tasks t ON t.id = d.depends_on_id WHERE d.task_id = ", 0),
        ("SELECT t.id, t.number, t.title, t.status, t.deadline FROM task_dependencies d JOIN tasks t ON t.id = d.task_id WHERE d.depends_on_id = ", 1),
    ] {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(sql);
        qb.push_bind(task);
        push_visible(&mut qb, auth);
        qb.push(" ORDER BY t.number");
        let rows: Vec<DepRef> = qb.build_query_as().fetch_all(&state.db).await?;
        if slot == 0 { out.0 = rows } else { out.1 = rows }
    }
    Ok(out)
}

/// Would making `task` depend on `on` create a loop? (Does `on` already depend on `task`?)
async fn creates_cycle(state: &AppState, task: Uuid, on: Uuid) -> AppResult<bool> {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([on]);
    while let Some(cur) = queue.pop_front() {
        if cur == task {
            return Ok(true);
        }
        if !seen.insert(cur) || seen.len() > 10_000 {
            continue;
        }
        let next: Vec<Uuid> = sqlx::query_scalar("SELECT depends_on_id FROM task_dependencies WHERE task_id = $1")
            .bind(cur)
            .fetch_all(&state.db)
            .await?;
        queue.extend(next);
    }
    Ok(false)
}

#[derive(Deserialize)]
struct AddDependency {
    depends_on_id: Uuid,
}

async fn add(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(body): Json<AddDependency>) -> AppResult<Json<Value>> {
    let (core, access) = load_task(&state, &auth, id).await?;
    if !access.edit {
        return Err(AppError::Forbidden);
    }
    if body.depends_on_id == id {
        return Err(AppError::bad("a task cannot wait for itself"));
    }
    let (other, _) = load_task(&state, &auth, body.depends_on_id).await?;
    if creates_cycle(&state, id, body.depends_on_id).await? {
        return Err(AppError::bad("that would create a loop: the other task already waits for this one"));
    }
    sqlx::query("INSERT INTO task_dependencies (task_id, depends_on_id, created_by) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(id)
        .bind(body.depends_on_id)
        .bind(auth.id)
        .execute(&state.db)
        .await?;
    record_history(&state.db, id, auth.id, "blocked_by_added", None, Some(other.title.clone())).await?;
    // Respect the new constraint straight away.
    reschedule_dependents(&state, &auth, body.depends_on_id).await?;
    broadcast_changed(&state, id, core.project_id, false).await;
    broadcast_changed(&state, body.depends_on_id, other.project_id, false).await;
    Ok(Json(json!({ "ok": true })))
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path((id, other)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    let (core, access) = load_task(&state, &auth, id).await?;
    if !access.edit {
        return Err(AppError::Forbidden);
    }
    let title: Option<String> = sqlx::query_scalar("SELECT title FROM tasks WHERE id = $1").bind(other).fetch_optional(&state.db).await?;
    sqlx::query("DELETE FROM task_dependencies WHERE task_id = $1 AND depends_on_id = $2")
        .bind(id)
        .bind(other)
        .execute(&state.db)
        .await?;
    record_history(&state.db, id, auth.id, "blocked_by_removed", None, title).await?;
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(json!({ "ok": true })))
}

/// (id, start_date, deadline, status) of a dependent task.
type Successor = (Uuid, Option<DateTime<Utc>>, Option<DateTime<Utc>>, String);

/// Shifts dependants that would start before `task` ends. Returns the ids that moved.
pub async fn reschedule_dependents(state: &AppState, auth: &AuthUser, task: Uuid) -> AppResult<Vec<Uuid>> {
    let mut moved = Vec::new();
    let mut seen = HashSet::from([task]);
    let mut queue = VecDeque::from([task]);
    while let Some(pred) = queue.pop_front() {
        let end: Option<DateTime<Utc>> = sqlx::query_scalar("SELECT COALESCE(deadline, start_date) FROM tasks WHERE id = $1")
            .bind(pred)
            .fetch_optional(&state.db)
            .await?
            .flatten();
        let Some(end) = end else { continue };
        let succs: Vec<Successor> = sqlx::query_as(
            "SELECT t.id, t.start_date, t.deadline, t.status FROM task_dependencies d JOIN tasks t ON t.id = d.task_id
             WHERE d.depends_on_id = $1",
        )
        .bind(pred)
        .fetch_all(&state.db)
        .await?;
        for (sid, start, deadline, status) in succs {
            if !seen.insert(sid) || status == "done" {
                continue;
            }
            // A task with no start date starts "whenever"; use its deadline as the anchor.
            let Some(anchor) = start.or(deadline) else { continue };
            if anchor >= end {
                continue;
            }
            let Ok((_, a)) = load_task(state, auth, sid).await else { continue };
            if !a.edit {
                continue;
            }
            let delta: Duration = end - anchor;
            let new_start = start.map(|s| s + delta);
            let new_deadline = deadline.map(|d| d + delta);
            sqlx::query(
                "UPDATE tasks SET start_date = $1, deadline = $2, deadline_reminded_at = NULL, overdue_notified_at = NULL,
                     updated_at = now() WHERE id = $3",
            )
            .bind(new_start)
            .bind(new_deadline)
            .bind(sid)
            .execute(&state.db)
            .await?;
            record_history(
                &state.db,
                sid,
                auth.id,
                "rescheduled",
                deadline.map(|d| d.to_rfc3339()),
                new_deadline.map(|d| d.to_rfc3339()),
            )
            .await?;
            moved.push(sid);
            queue.push_back(sid);
        }
    }
    if !moved.is_empty() {
        let actor = user_name(state, auth.id).await;
        for sid in &moved {
            let row: Option<(String, Uuid, Option<Uuid>)> = sqlx::query_as("SELECT title, responsible_id, project_id FROM tasks WHERE id = $1")
                .bind(sid)
                .fetch_optional(&state.db)
                .await?;
            if let Some((title, responsible, project)) = row {
                notify_task(
                    state,
                    &[responsible],
                    "task.rescheduled",
                    format!("Dates moved: {title}"),
                    Some(format!("{actor} moved a task this one waits for, so its dates shifted.")),
                    *sid,
                    auth.id,
                )
                .await;
                broadcast_changed(state, *sid, project, false).await;
            }
        }
    }
    Ok(moved)
}

#[derive(Deserialize)]
struct GanttQuery {
    project_id: Option<Uuid>,
    /// my | created | all (default: my when no project is given)
    view: Option<String>,
    /// Include finished tasks (default true for projects).
    include_done: Option<String>,
}

#[derive(Serialize, FromRow)]
struct Edge {
    task_id: Uuid,
    depends_on_id: Uuid,
}

async fn gantt(State(state): State<AppState>, auth: AuthUser, Query(q): Query<GanttQuery>) -> AppResult<Json<Value>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(SUMMARY_SELECT);
    push_visible(&mut qb, &auth);
    if let Some(p) = q.project_id {
        project_access(&state, &auth, p).await?;
        qb.push(" AND t.project_id = ").push_bind(p);
    } else {
        match q.view.as_deref().unwrap_or("my") {
            "created" => {
                qb.push(" AND t.created_by = ").push_bind(auth.id);
            }
            "all" => {}
            _ => {
                qb.push(" AND (t.responsible_id = ").push_bind(auth.id)
                    .push(" OR EXISTS (SELECT 1 FROM task_members m WHERE m.task_id = t.id AND m.role = 'assistant' AND m.user_id = ").push_bind(auth.id).push("))");
            }
        }
    }
    if q.include_done.as_deref() == Some("false") {
        qb.push(" AND t.status <> 'done'");
    }
    qb.push(" AND t.status <> 'deferred' ORDER BY COALESCE(t.start_date, t.created_at), t.number LIMIT 500");
    let tasks: Vec<TaskSummary> = qb.build_query_as().fetch_all(&state.db).await?;
    let ids: Vec<Uuid> = tasks.iter().map(|t| t.id).collect();
    let edges: Vec<Edge> = sqlx::query_as(
        "SELECT task_id, depends_on_id FROM task_dependencies WHERE task_id = ANY($1) AND depends_on_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "tasks": tasks, "dependencies": edges })))
}
