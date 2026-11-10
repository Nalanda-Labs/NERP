//! Flows: a queue of similar tasks (requests to IT, design jobs, …) that a team works through.
//!
//! A flow belongs to a workgroup or project. Every employee sees the flows and can put a task
//! into an active one (its list of tasks is for the team and the group); the
//! task goes to the group, with the flow's planned time as its deadline when none is given.
//! How it is handed out depends on the flow:
//! - **queue**: straight to the team member with the fewest open tasks in the flow (then the
//!   one who got one longest ago);
//! - **manual**: to the flow's administrator, who gives it to someone;
//! - **self**: it waits until someone in the team takes it.
//!
//! The flow shows tasks waiting, at work and done, how long tasks wait before work starts,
//! how long they take, and its **efficiency**: the share of tasks done within the planned time.
//! The group's owners and the flow's administrator change it.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    access::{load_task, project_access},
    broadcast_changed,
    items::{create_task, load_detail, CreateTask, TaskDetail, TaskSummary},
    notify_task, record_history, user_name,
};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/flows", get(list).post(create))
        .route("/flows/{id}", get(detail).patch(update).delete(remove))
        .route("/flows/{id}/tasks", get(tasks).post(add_task))
        .route("/tasks/{id}/take", post(take))
}

#[derive(sqlx::FromRow)]
struct FlowRow {
    id: Uuid,
    project_id: Uuid,
    project_name: String,
    name: String,
    description: Option<String>,
    owner_id: Uuid,
    owner_name: String,
    distribution: String,
    team: Vec<Uuid>,
    planned_secs: Option<i32>,
    is_active: bool,
}

const SELECT: &str = "SELECT f.id, f.project_id, p.name AS project_name, f.name, f.description, f.owner_id,
    trim(o.first_name || ' ' || o.last_name) AS owner_name, f.distribution, f.team, f.planned_secs, f.is_active
    FROM task_flows f JOIN projects p ON p.id = f.project_id JOIN users o ON o.id = f.owner_id";

async fn load(state: &AppState, id: Uuid) -> AppResult<FlowRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE f.id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("flow"))
}

/// Who sees the flow's tasks: its team, its administrator and the group's members.
async fn insider(state: &AppState, auth: &AuthUser, f: &FlowRow) -> AppResult<bool> {
    Ok(f.team.contains(&auth.id) || f.owner_id == auth.id || project_access(state, auth, f.project_id).await.is_ok())
}

async fn can_manage(state: &AppState, auth: &AuthUser, f: &FlowRow) -> AppResult<bool> {
    Ok(f.owner_id == auth.id || project_access(state, auth, f.project_id).await.map(|a| a.manage).unwrap_or(false))
}

/// Numbers for a flow over the last 30 days.
async fn stats(state: &AppState, f: &FlowRow) -> AppResult<Value> {
    #[allow(clippy::type_complexity)]
    let (waiting, at_work, done, done_in_time, avg_wait, avg_done): (i64, i64, i64, i64, Option<f64>, Option<f64>) = sqlx::query_as(
        "SELECT
            COUNT(*) FILTER (WHERE status IN ('new','deferred','review') OR flow_waiting) AS waiting,
            COUNT(*) FILTER (WHERE status = 'in_progress' AND NOT flow_waiting) AS at_work,
            COUNT(*) FILTER (WHERE status = 'done' AND completed_at > now() - interval '30 days') AS done,
            COUNT(*) FILTER (WHERE status = 'done' AND completed_at > now() - interval '30 days'
                             AND ($2::int IS NULL OR completed_at - created_at <= make_interval(secs => $2))) AS done_in_time,
            (AVG(EXTRACT(EPOCH FROM (started_at - created_at)) / 3600) FILTER (WHERE started_at IS NOT NULL AND created_at > now() - interval '30 days'))::float8 AS avg_wait,
            (AVG(EXTRACT(EPOCH FROM (completed_at - created_at)) / 3600) FILTER (WHERE status = 'done' AND completed_at > now() - interval '30 days'))::float8 AS avg_done
         FROM tasks WHERE flow_id = $1",
    )
    .bind(f.id)
    .bind(f.planned_secs)
    .fetch_one(&state.db)
    .await?;
    let round = |x: f64| (x * 10.0).round() / 10.0;
    Ok(json!({
        "waiting": waiting, "at_work": at_work, "done": done,
        "avg_wait_hours": avg_wait.map(round), "avg_completion_hours": avg_done.map(round),
        "efficiency": if done == 0 { None } else { Some(((done_in_time as f64 / done as f64) * 100.0).round() as i64) },
    }))
}

async fn view(state: &AppState, auth: &AuthUser, f: FlowRow, full: bool) -> AppResult<Value> {
    let mut v = json!({
        "id": f.id, "project_id": f.project_id, "project_name": f.project_name, "name": f.name, "description": f.description,
        "owner_id": f.owner_id, "owner_name": f.owner_name, "distribution": f.distribution, "team": f.team,
        "planned_hours": f.planned_secs.map(|s| s as f64 / 3600.0), "is_active": f.is_active,
        "can_manage": can_manage(state, auth, &f).await?, "in_team": f.team.contains(&auth.id),
        "stats": stats(state, &f).await?,
    });
    if full {
        // The team, with each person's open tasks in the flow.
        let team: Vec<(Uuid, String, Option<Uuid>, i64)> = sqlx::query_as(
            "SELECT u.id, trim(u.first_name || ' ' || u.last_name), u.avatar_file_id,
                    (SELECT COUNT(*) FROM tasks t WHERE t.flow_id = $2 AND t.responsible_id = u.id AND NOT t.flow_waiting AND t.status IN ('new','in_progress','review'))
             FROM users u WHERE u.id = ANY($1) ORDER BY u.first_name",
        )
        .bind(&f.team)
        .bind(f.id)
        .fetch_all(&state.db)
        .await?;
        v["members"] = json!(team.into_iter().map(|(id, n, a, open)| json!({ "id": id, "full_name": n, "avatar_file_id": a, "open": open })).collect::<Vec<_>>());
    }
    Ok(v)
}

#[derive(Deserialize)]
struct ListQuery {
    project_id: Option<Uuid>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let rows: Vec<FlowRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE ($1::uuid IS NULL OR f.project_id = $1) ORDER BY f.is_active DESC, f.name LIMIT 300")))
        .bind(q.project_id)
        .fetch_all(&state.db)
        .await?;
    // Every employee sees the flows (and can send them tasks).
    let mut items = Vec::new();
    for f in rows {
        items.push(view(&state, &auth, f, false).await?);
    }
    Ok(Json(json!({ "items": items })))
}

async fn detail(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let f = load(&state, id).await?;
    Ok(Json(view(&state, &auth, f, true).await?))
}

#[derive(Deserialize)]
struct FlowInput {
    project_id: Option<Uuid>,
    name: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    owner_id: Option<Uuid>,
    distribution: Option<String>,
    team: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    planned_hours: Option<Option<f64>>,
    is_active: Option<bool>,
}

async fn check_people(state: &AppState, ids: &[Uuid]) -> AppResult<()> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND is_active").bind(ids).fetch_one(&state.db).await?;
    if n as usize != ids.len() {
        return Err(AppError::bad("the team must be active employees"));
    }
    Ok(())
}

fn planned(h: Option<f64>) -> AppResult<Option<i32>> {
    match h {
        None => Ok(None),
        Some(h) if h > 0.0 && h <= 24.0 * 365.0 => Ok(Some((h * 3600.0).round() as i32)),
        Some(_) => Err(AppError::bad("the planned time is 1 minute to a year")),
    }
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<FlowInput>) -> AppResult<Json<Value>> {
    let project = b.project_id.ok_or_else(|| AppError::bad("choose the workgroup or project"))?;
    if !project_access(&state, &auth, project).await?.manage {
        return Err(AppError::bad("only the group's owners make flows"));
    }
    let name = required("name", b.name.as_deref().unwrap_or(""), 200)?;
    let distribution = b.distribution.unwrap_or_else(|| "queue".into());
    if !["manual", "queue", "self"].contains(&distribution.as_str()) {
        return Err(AppError::bad("distribution must be manual, queue or self"));
    }
    let mut team = b.team.unwrap_or_default();
    team.sort();
    team.dedup();
    check_people(&state, &team).await?;
    if distribution != "manual" && team.is_empty() {
        return Err(AppError::bad("add the team that works on the flow"));
    }
    let owner = b.owner_id.unwrap_or(auth.id);
    check_people(&state, &[owner]).await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO task_flows (project_id, name, description, owner_id, distribution, team, planned_secs, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(project)
    .bind(name)
    .bind(clean(b.description.flatten()))
    .bind(owner)
    .bind(&distribution)
    .bind(&team)
    .bind(planned(b.planned_hours.flatten())?)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    detail(State(state), auth, Path(id)).await
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<FlowInput>) -> AppResult<Json<Value>> {
    let f = load(&state, id).await?;
    if !can_manage(&state, &auth, &f).await? {
        return Err(AppError::Forbidden);
    }
    if let Some(n) = &b.name {
        sqlx::query("UPDATE task_flows SET name = $2 WHERE id = $1").bind(id).bind(required("name", n, 200)?).execute(&state.db).await?;
    }
    if let Some(d) = b.description {
        sqlx::query("UPDATE task_flows SET description = $2 WHERE id = $1").bind(id).bind(clean(d)).execute(&state.db).await?;
    }
    if let Some(o) = b.owner_id {
        check_people(&state, &[o]).await?;
        sqlx::query("UPDATE task_flows SET owner_id = $2 WHERE id = $1").bind(id).bind(o).execute(&state.db).await?;
    }
    if let Some(d) = &b.distribution {
        if !["manual", "queue", "self"].contains(&d.as_str()) {
            return Err(AppError::bad("distribution must be manual, queue or self"));
        }
        sqlx::query("UPDATE task_flows SET distribution = $2 WHERE id = $1").bind(id).bind(d).execute(&state.db).await?;
    }
    if let Some(mut t) = b.team {
        t.sort();
        t.dedup();
        check_people(&state, &t).await?;
        sqlx::query("UPDATE task_flows SET team = $2 WHERE id = $1").bind(id).bind(&t).execute(&state.db).await?;
    }
    if let Some(h) = b.planned_hours {
        sqlx::query("UPDATE task_flows SET planned_secs = $2 WHERE id = $1").bind(id).bind(planned(h)?).execute(&state.db).await?;
    }
    if let Some(a) = b.is_active {
        sqlx::query("UPDATE task_flows SET is_active = $2 WHERE id = $1").bind(id).bind(a).execute(&state.db).await?;
    }
    detail(State(state), auth, Path(id)).await
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let f = load(&state, id).await?;
    if !can_manage(&state, &auth, &f).await? {
        return Err(AppError::Forbidden);
    }
    // Its tasks stay, out of the flow.
    sqlx::query("DELETE FROM task_flows WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct TasksQuery {
    /// waiting | at_work | done
    state: Option<String>,
}

async fn tasks(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<TasksQuery>) -> AppResult<Json<Value>> {
    let f = load(&state, id).await?;
    if !insider(&state, &auth, &f).await? {
        return Err(AppError::NotFound("flow"));
    }
    let mut qb = sqlx::QueryBuilder::new(super::items::SUMMARY_SELECT);
    qb.push(" AND t.flow_id = ").push_bind(id);
    match q.state.as_deref() {
        Some("waiting") => qb.push(" AND (t.status IN ('new','deferred','review') OR t.flow_waiting) AND t.status <> 'done' ORDER BY t.created_at"),
        Some("at_work") => qb.push(" AND t.status = 'in_progress' AND NOT t.flow_waiting ORDER BY t.deadline ASC NULLS LAST"),
        Some("done") => qb.push(" AND t.status = 'done' ORDER BY t.completed_at DESC LIMIT 100"),
        _ => qb.push(" ORDER BY t.created_at DESC LIMIT 300"),
    };
    let items: Vec<TaskSummary> = qb.build_query_as().fetch_all(&state.db).await?;
    let waiting: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM tasks WHERE flow_id = $1 AND flow_waiting").bind(id).fetch_all(&state.db).await?;
    let items: Vec<Value> = items
        .into_iter()
        .map(|t| {
            let w = waiting.contains(&t.id);
            let mut v = serde_json::to_value(t).unwrap();
            v["flow_waiting"] = json!(w);
            v
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
struct NewFlowTask {
    title: String,
    description: Option<String>,
    deadline: Option<DateTime<Utc>>,
    priority: Option<String>,
    #[serde(default)]
    file_ids: Vec<Uuid>,
}

/// Who a new task in the flow goes to, and whether it waits to be handed out.
async fn assignee(state: &AppState, f: &FlowRow) -> AppResult<(Uuid, bool)> {
    match f.distribution.as_str() {
        "queue" if !f.team.is_empty() => {
            let next: Uuid = sqlx::query_scalar(
                "SELECT u FROM unnest($1::uuid[]) u JOIN users x ON x.id = u AND x.is_active
                 ORDER BY (SELECT COUNT(*) FROM tasks t WHERE t.flow_id = $2 AND t.responsible_id = u AND t.status IN ('new','in_progress','review')),
                          (SELECT MAX(t.created_at) FROM tasks t WHERE t.flow_id = $2 AND t.responsible_id = u) ASC NULLS FIRST
                 LIMIT 1",
            )
            .bind(&f.team)
            .bind(f.id)
            .fetch_optional(&state.db)
            .await?
            .unwrap_or(f.owner_id);
            Ok((next, false))
        }
        _ => Ok((f.owner_id, true)),
    }
}

/// Puts a task into the flow (anyone may, while it is active).
async fn add_task(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<NewFlowTask>) -> AppResult<Json<TaskDetail>> {
    let f = load(&state, id).await?;
    if !f.is_active {
        return Err(AppError::bad("this flow is paused"));
    }
    let (responsible, waiting) = assignee(&state, &f).await?;
    let deadline = b.deadline.or(f.planned_secs.map(|s| Utc::now() + Duration::seconds(s as i64)));
    // Made outside the group (the sender may not belong to it), then put in it.
    let task = create_task(&state, &auth, CreateTask {
        title: b.title,
        description: b.description,
        responsible_id: Some(responsible),
        deadline,
        priority: b.priority,
        file_ids: b.file_ids,
        ..Default::default()
    })
    .await?;
    sqlx::query(
        "UPDATE tasks SET project_id = $2, flow_id = $3, flow_waiting = $4,
           stage_id = (SELECT id FROM project_stages WHERE project_id = $2 ORDER BY (status IS NOT DISTINCT FROM 'new') DESC, sort LIMIT 1)
         WHERE id = $1",
    )
    .bind(task)
    .bind(f.project_id)
    .bind(f.id)
    .bind(waiting)
    .execute(&state.db)
    .await?;
    record_history(&state.db, task, auth.id, "flow", None, Some(f.name.clone())).await?;
    if waiting && f.distribution == "self" {
        let title: String = sqlx::query_scalar("SELECT title FROM tasks WHERE id = $1").bind(task).fetch_one(&state.db).await?;
        notify_task(&state, &f.team, "flow.waiting", format!("A task is waiting in {}: {title}", f.name), None, task, auth.id).await;
    }
    broadcast_changed(&state, task, Some(f.project_id), false).await;
    Ok(Json(load_detail(&state, &auth, task).await?))
}

/// Someone in the team takes a waiting task (self-service flows), or the administrator does.
async fn take(State(state): State<AppState>, auth: AuthUser, Path(task): Path<Uuid>) -> AppResult<Json<TaskDetail>> {
    let row: Option<(Option<Uuid>, bool)> = sqlx::query_as("SELECT flow_id, flow_waiting FROM tasks WHERE id = $1").bind(task).fetch_optional(&state.db).await?;
    let Some((Some(flow), waiting)) = row else { return Err(AppError::NotFound("task")) };
    let f = load(&state, flow).await?;
    if !f.team.contains(&auth.id) && f.owner_id != auth.id {
        return Err(AppError::bad("only the flow's team takes its tasks"));
    }
    if !waiting {
        return Err(AppError::bad("someone already has this task"));
    }
    let taken = sqlx::query("UPDATE tasks SET responsible_id = $2, flow_waiting = FALSE, updated_at = now() WHERE id = $1 AND flow_waiting")
        .bind(task)
        .bind(auth.id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if taken == 0 {
        return Err(AppError::bad("someone already has this task"));
    }
    sqlx::query("DELETE FROM task_members WHERE task_id = $1 AND user_id = $2").bind(task).bind(auth.id).execute(&state.db).await?;
    record_history(&state.db, task, auth.id, "responsible", Some(user_name(&state, f.owner_id).await), Some(user_name(&state, auth.id).await)).await?;
    broadcast_changed(&state, task, Some(f.project_id), false).await;
    load_task(&state, &auth, task).await?;
    Ok(Json(load_detail(&state, &auth, task).await?))
}
