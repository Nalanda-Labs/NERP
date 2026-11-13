//! Scrum teams: a group of kind `scrum` plans its work in sprints.
//!
//! - The **backlog** is the team's tasks that are in no sprint, in the order the team ranks them.
//! - **Sprints** are planned, then started one at a time (the points in it then are the
//!   commitment), then completed: unfinished tasks go back to the backlog or into another
//!   planned sprint, and what the sprint held is kept for its history and the velocity chart.
//! - **Epics** group tasks into larger pieces of work; **story points** estimate them.
//! - The **definition of done** is the team's checklist for finishing a task.
//! - The **burndown** of a sprint: points left each day against the ideal line; tasks added
//!   to or taken out of a running sprint count as scope changes.
//!
//! Owners of the team and its Scrum master plan sprints, epics and the definition of done;
//! members estimate, rank and move tasks between the backlog and sprints.

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post, put},
    Json, Router,
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    access::{load_task, project_access},
    broadcast_changed, record_history,
};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::{clean, double_option, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{id}/scrum", get(overview).patch(settings))
        .route("/projects/{id}/sprints", get(list_sprints).post(create_sprint))
        .route("/projects/{id}/velocity", get(velocity))
        .route("/projects/{id}/epics", get(list_epics).post(create_epic))
        .route("/projects/{id}/dod", get(get_dod).put(set_dod))
        .route("/sprints/{id}", patch(update_sprint).delete(delete_sprint))
        .route("/sprints/{id}/start", post(start_sprint))
        .route("/sprints/{id}/complete", post(complete_sprint))
        .route("/sprints/{id}/burndown", get(burndown))
        .route("/epics/{id}", patch(update_epic).delete(delete_epic))
        .route("/tasks/{id}/scrum", put(place))
        .route("/tasks/{id}/dod", post(confirm_dod))
}

const EPIC_COLORS: &[&str] = &["violet", "sky", "emerald", "amber", "rose", "indigo", "teal", "orange", "slate"];

// ---------------------------------------------------------------------------
// Access
// ---------------------------------------------------------------------------

struct Team {
    /// Owners and the Scrum master: sprints, epics, definition of done, settings.
    plan: bool,
    /// Members: estimate, rank, move tasks between the backlog and sprints.
    contribute: bool,
}

async fn team(state: &AppState, auth: &AuthUser, project: Uuid) -> AppResult<Team> {
    let access = project_access(state, auth, project).await?;
    let (kind, master): (String, Option<Uuid>) = sqlx::query!(r#"SELECT kind, scrum_master_id FROM projects WHERE id = $1"#, project).map(|r| (r.kind, r.scrum_master_id))
        .fetch_one(&state.db)
        .await?;
    if kind != "scrum" {
        return Err(AppError::bad("this group isn't a Scrum team"));
    }
    let master = master == Some(auth.id) && access.role.is_some();
    Ok(Team { plan: access.manage || master, contribute: access.contribute || master })
}

async fn require_plan(state: &AppState, auth: &AuthUser, project: Uuid) -> AppResult<()> {
    if team(state, auth, project).await?.plan { Ok(()) } else { Err(AppError::Forbidden) }
}

async fn sprint_project(state: &AppState, sprint: Uuid) -> AppResult<(Uuid, String)> {
    sqlx::query!(r#"SELECT project_id, status FROM scrum_sprints WHERE id = $1"#, sprint).map(|r| (r.project_id, r.status))
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("sprint"))
}

/// Everyone who may be looking at the team's backlog or board.
async fn broadcast(state: &AppState, project: Uuid) {
    let members: Vec<Uuid> = sqlx::query_scalar!("SELECT user_id FROM project_members WHERE project_id = $1", project)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    crate::realtime::publish(state, Some(members), "scrum.changed", json!({ "project_id": project })).await;
}

/// Checks that a task's estimate, epic and sprint fit the project it goes into.
pub async fn check_placement(state: &AppState, project: Option<Uuid>, points: Option<f64>, epic: Option<Uuid>, sprint: Option<Uuid>) -> AppResult<()> {
    check_points(points)?;
    if epic.is_none() && sprint.is_none() {
        return Ok(());
    }
    let Some(project) = project else {
        return Err(AppError::bad("only a Scrum team's tasks have epics and sprints"));
    };
    if let Some(e) = epic {
        let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM scrum_epics WHERE id = $1 AND project_id = $2) AS "v!""#, e, project)
            .fetch_one(&state.db)
            .await?;
        if !ok {
            return Err(AppError::bad("that epic belongs to another team"));
        }
    }
    if let Some(s) = sprint {
        let status: Option<String> = sqlx::query_scalar!("SELECT status FROM scrum_sprints WHERE id = $1 AND project_id = $2", s, project)
            .fetch_optional(&state.db)
            .await?;
        match status.as_deref() {
            None => return Err(AppError::bad("that sprint belongs to another team")),
            Some("completed") => return Err(AppError::bad("that sprint is already completed")),
            _ => {}
        }
    }
    Ok(())
}

fn check_points(points: Option<f64>) -> AppResult<()> {
    match points {
        Some(p) if !p.is_finite() || !(0.0..=1000.0).contains(&p) => Err(AppError::bad("story points must be between 0 and 1000")),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
pub struct Sprint {
    id: Uuid,
    name: String,
    goal: Option<String>,
    status: String,
    sort: f64,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    committed_points: Option<f64>,
    completed_points: Option<f64>,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    task_count: i64,
    done_count: i64,
    points_total: f64,
    points_done: f64,
}

/// Counts come from the tasks in the sprint, or (once completed) from what it held then.
const SPRINT_SELECT: &str = "SELECT s.id, s.name, s.goal, s.status, s.sort, s.starts_at, s.ends_at, s.committed_points, s.completed_points,
    s.started_at, s.completed_at,
    CASE WHEN s.status = 'completed' THEN (SELECT COUNT(*) FROM scrum_sprint_results r WHERE r.sprint_id = s.id)
         ELSE (SELECT COUNT(*) FROM tasks t WHERE t.sprint_id = s.id) END AS task_count,
    CASE WHEN s.status = 'completed' THEN (SELECT COUNT(*) FROM scrum_sprint_results r WHERE r.sprint_id = s.id AND r.done)
         ELSE (SELECT COUNT(*) FROM tasks t WHERE t.sprint_id = s.id AND t.status = 'done') END AS done_count,
    CASE WHEN s.status = 'completed' THEN (SELECT COALESCE(SUM(r.points), 0) FROM scrum_sprint_results r WHERE r.sprint_id = s.id)
         ELSE (SELECT COALESCE(SUM(t.story_points), 0) FROM tasks t WHERE t.sprint_id = s.id) END AS points_total,
    CASE WHEN s.status = 'completed' THEN (SELECT COALESCE(SUM(r.points), 0) FROM scrum_sprint_results r WHERE r.sprint_id = s.id AND r.done)
         ELSE (SELECT COALESCE(SUM(t.story_points), 0) FROM tasks t WHERE t.sprint_id = s.id AND t.status = 'done') END AS points_done
    FROM scrum_sprints s";

async fn load_sprint(state: &AppState, id: Uuid) -> AppResult<Sprint> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("{SPRINT_SELECT} WHERE s.id = $1"))).bind(id).fetch_one(&state.db).await?)
}

#[derive(Serialize, FromRow)]
struct Epic {
    id: Uuid,
    name: String,
    description: Option<String>,
    color: String,
    task_count: i64,
    done_count: i64,
    points_total: f64,
    points_done: f64,
}

async fn load_epics(state: &AppState, project: Uuid) -> AppResult<Vec<Epic>> {
    Ok(sqlx::query_as!(Epic, r#"SELECT e.id AS "id!", e.name AS "name!", e.description AS "description?", e.color AS "color!",
            (SELECT COUNT(*) FROM tasks t WHERE t.epic_id = e.id) AS "task_count!",
            (SELECT COUNT(*) FROM tasks t WHERE t.epic_id = e.id AND t.status = 'done') AS "done_count!",
            (SELECT COALESCE(SUM(t.story_points), 0) FROM tasks t WHERE t.epic_id = e.id) AS "points_total!",
            (SELECT COALESCE(SUM(t.story_points), 0) FROM tasks t WHERE t.epic_id = e.id AND t.status = 'done') AS "points_done!"
         FROM scrum_epics e WHERE e.project_id = $1 ORDER BY e.created_at"#, project)
    .fetch_all(&state.db)
    .await?)
}

#[derive(Serialize, FromRow)]
struct DodItem {
    id: Uuid,
    title: String,
}

async fn load_dod(state: &AppState, project: Uuid) -> AppResult<Vec<DodItem>> {
    Ok(sqlx::query_as!(DodItem, "SELECT id, title FROM scrum_dod_items WHERE project_id = $1 ORDER BY sort, title", project)
        .fetch_all(&state.db)
        .await?)
}

// ---------------------------------------------------------------------------
// Overview and settings
// ---------------------------------------------------------------------------

async fn overview(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let t = team(&state, &auth, id).await?;
    let (master, master_name, days): (Option<Uuid>, Option<String>, i32) = sqlx::query!(r#"SELECT p.scrum_master_id AS "scrum_master_id?", trim(u.first_name || ' ' || u.last_name) AS "c1?", p.sprint_days AS "sprint_days!"
         FROM projects p LEFT JOIN users u ON u.id = p.scrum_master_id WHERE p.id = $1"#, id).map(|r| (r.scrum_master_id, r.c1, r.sprint_days))
    .fetch_one(&state.db)
    .await?;
    let sprints: Vec<Sprint> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SPRINT_SELECT} WHERE s.project_id = $1 AND s.status <> 'completed' ORDER BY s.status = 'active' DESC, s.sort, s.created_at")))
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    let (backlog_count, backlog_points, unestimated): (i64, f64, i64) = sqlx::query!(r#"SELECT COUNT(*) AS "c0!", COALESCE(SUM(story_points), 0) AS "c1!", COUNT(*) FILTER (WHERE story_points IS NULL) AS "c2!"
         FROM tasks WHERE project_id = $1 AND sprint_id IS NULL AND status <> 'done'"#, id).map(|r| (r.c0, r.c1, r.c2))
    .fetch_one(&state.db)
    .await?;
    let completed: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM scrum_sprints WHERE project_id = $1 AND status = 'completed'"#, id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(json!({
        "scrum_master": master.map(|m| json!({ "id": m, "full_name": master_name })),
        "sprint_days": days,
        "can_plan": t.plan,
        "can_contribute": t.contribute,
        "sprints": sprints,
        "completed_sprints": completed,
        "epics": load_epics(&state, id).await?,
        "dod": load_dod(&state, id).await?,
        "backlog": { "count": backlog_count, "points": backlog_points, "unestimated": unestimated },
    })))
}

#[derive(Deserialize)]
struct Settings {
    #[serde(default, deserialize_with = "double_option")]
    scrum_master_id: Option<Option<Uuid>>,
    sprint_days: Option<i32>,
}

async fn settings(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Settings>) -> AppResult<Json<Value>> {
    require_plan(&state, &auth, id).await?;
    if let Some(m) = b.scrum_master_id {
        if let Some(m) = m {
            let member: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM project_members WHERE project_id = $1 AND user_id = $2 AND role <> 'viewer') AS "v!""#, id, m)
                .fetch_one(&state.db)
                .await?;
            if !member {
                return Err(AppError::bad("the Scrum master must be a member of the team"));
            }
        }
        sqlx::query!("UPDATE projects SET scrum_master_id = $1 WHERE id = $2", m, id).execute(&state.db).await?;
    }
    if let Some(d) = b.sprint_days {
        if !(1..=90).contains(&d) {
            return Err(AppError::bad("a sprint lasts 1 to 90 days"));
        }
        sqlx::query!("UPDATE projects SET sprint_days = $1 WHERE id = $2", d, id).execute(&state.db).await?;
    }
    broadcast(&state, id).await;
    overview(State(state), auth, Path(id)).await
}

// ---------------------------------------------------------------------------
// Sprints
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SprintQuery {
    /// planned | active | completed; all when absent.
    status: Option<String>,
}

async fn list_sprints(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<SprintQuery>) -> AppResult<Json<Vec<Sprint>>> {
    team(&state, &auth, id).await?;
    let rows = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SPRINT_SELECT} WHERE s.project_id = $1 AND ($2::text IS NULL OR s.status = $2)
         ORDER BY CASE s.status WHEN 'active' THEN 0 WHEN 'planned' THEN 1 ELSE 2 END, s.completed_at DESC NULLS LAST, s.sort, s.created_at"
    )))
    .bind(id)
    .bind(q.status)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct NewSprint {
    name: Option<String>,
    goal: Option<String>,
}

async fn create_sprint(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<NewSprint>) -> AppResult<Json<Sprint>> {
    require_plan(&state, &auth, id).await?;
    let n: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM scrum_sprints WHERE project_id = $1"#, id).fetch_one(&state.db).await?;
    let name = match clean(b.name) {
        Some(n) => required("sprint name", &n, 120)?,
        None => format!("Sprint {}", n + 1),
    };
    let sid: Uuid = sqlx::query_scalar!("INSERT INTO scrum_sprints (project_id, name, goal, sort, created_by)
         VALUES ($1, $2, $3, COALESCE((SELECT MAX(sort) FROM scrum_sprints WHERE project_id = $1), 0) + 1, $4) RETURNING id", id, &name, clean(b.goal).map(|g| g.chars().take(2000).collect::<String>()), auth.id)
    .fetch_one(&state.db)
    .await?;
    broadcast(&state, id).await;
    Ok(Json(load_sprint(&state, sid).await?))
}

#[derive(Deserialize)]
struct UpdateSprint {
    name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    goal: Option<Option<String>>,
    sort: Option<f64>,
    /// Only for the active sprint: move its end.
    ends_at: Option<DateTime<Utc>>,
}

async fn update_sprint(State(state): State<AppState>, auth: AuthUser, Path(sid): Path<Uuid>, Json(b): Json<UpdateSprint>) -> AppResult<Json<Sprint>> {
    let (project, status) = sprint_project(&state, sid).await?;
    require_plan(&state, &auth, project).await?;
    if status == "completed" {
        return Err(AppError::bad("a completed sprint can't be changed"));
    }
    if let Some(n) = &b.name {
        sqlx::query!("UPDATE scrum_sprints SET name = $1 WHERE id = $2", required("sprint name", n, 120)?, sid).execute(&state.db).await?;
    }
    if let Some(g) = b.goal {
        sqlx::query!("UPDATE scrum_sprints SET goal = $1 WHERE id = $2", clean(g), sid).execute(&state.db).await?;
    }
    if let Some(s) = b.sort {
        sqlx::query!("UPDATE scrum_sprints SET sort = $1 WHERE id = $2", s, sid).execute(&state.db).await?;
    }
    if let Some(e) = b.ends_at {
        if status != "active" {
            return Err(AppError::bad("a sprint gets its dates when it starts"));
        }
        let ok: bool = sqlx::query_scalar!(r#"SELECT starts_at < $1 AS "v!" FROM scrum_sprints WHERE id = $2"#, e, sid).fetch_one(&state.db).await?;
        if !ok {
            return Err(AppError::bad("the sprint can't end before it starts"));
        }
        sqlx::query!("UPDATE scrum_sprints SET ends_at = $1 WHERE id = $2", e, sid).execute(&state.db).await?;
    }
    broadcast(&state, project).await;
    Ok(Json(load_sprint(&state, sid).await?))
}

/// Deletes a planned sprint; its tasks go back to the top of the backlog.
async fn delete_sprint(State(state): State<AppState>, auth: AuthUser, Path(sid): Path<Uuid>) -> AppResult<Json<Value>> {
    let (project, status) = sprint_project(&state, sid).await?;
    require_plan(&state, &auth, project).await?;
    if status != "planned" {
        return Err(AppError::bad("only a sprint that hasn't started can be deleted"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query!("UPDATE tasks SET sprint_id = NULL,
         backlog_sort = COALESCE((SELECT MIN(backlog_sort) FROM tasks WHERE project_id = $2 AND sprint_id IS NULL), 0) - 1000 + backlog_sort
         WHERE sprint_id = $1", sid, project)
    .execute(&mut *tx)
    .await?;
    sqlx::query!("DELETE FROM scrum_sprints WHERE id = $1", sid).execute(&mut *tx).await?;
    tx.commit().await?;
    broadcast(&state, project).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
struct StartSprint {
    /// Defaults to now.
    starts_at: Option<DateTime<Utc>>,
    /// Defaults to the team's sprint length after the start.
    ends_at: Option<DateTime<Utc>>,
    goal: Option<String>,
}

async fn start_sprint(State(state): State<AppState>, auth: AuthUser, Path(sid): Path<Uuid>, body: Option<Json<StartSprint>>) -> AppResult<Json<Sprint>> {
    let b = body.map(|b| b.0).unwrap_or_default();
    let (project, status) = sprint_project(&state, sid).await?;
    require_plan(&state, &auth, project).await?;
    if status != "planned" {
        return Err(AppError::bad("this sprint has already started"));
    }
    let active: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM scrum_sprints WHERE project_id = $1 AND status = 'active') AS "v!""#, project)
        .fetch_one(&state.db)
        .await?;
    if active {
        return Err(AppError::bad("finish the active sprint before starting another"));
    }
    let tasks: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM tasks WHERE sprint_id = $1"#, sid).fetch_one(&state.db).await?;
    if tasks == 0 {
        return Err(AppError::bad("add tasks to the sprint before starting it"));
    }
    let days: i32 = sqlx::query_scalar!("SELECT sprint_days FROM projects WHERE id = $1", project).fetch_one(&state.db).await?;
    let starts = b.starts_at.unwrap_or_else(Utc::now);
    let ends = b.ends_at.unwrap_or(starts + Duration::days(days as i64));
    if ends <= starts {
        return Err(AppError::bad("the sprint can't end before it starts"));
    }
    if ends - starts > Duration::days(90) {
        return Err(AppError::bad("a sprint lasts at most 90 days"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query!("UPDATE scrum_sprints SET status = 'active', starts_at = $2, ends_at = $3, started_at = now(), goal = COALESCE($4, goal),
         committed_points = (SELECT COALESCE(SUM(story_points), 0) FROM tasks WHERE sprint_id = $1 AND status <> 'done')
         WHERE id = $1", sid, starts, ends, clean(b.goal))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let name: String = sqlx::query_scalar!("SELECT name FROM scrum_sprints WHERE id = $1", sid).fetch_one(&state.db).await?;
    notify_team(&state, project, &auth, format!("{name} started"), "scrum.sprint_started").await;
    broadcast(&state, project).await;
    Ok(Json(load_sprint(&state, sid).await?))
}

#[derive(Deserialize, Default)]
struct CompleteSprint {
    /// Where unfinished tasks go: a planned sprint of the team; the backlog when absent.
    move_to: Option<Uuid>,
}

async fn complete_sprint(State(state): State<AppState>, auth: AuthUser, Path(sid): Path<Uuid>, body: Option<Json<CompleteSprint>>) -> AppResult<Json<Sprint>> {
    let b = body.map(|b| b.0).unwrap_or_default();
    let (project, status) = sprint_project(&state, sid).await?;
    require_plan(&state, &auth, project).await?;
    if status != "active" {
        return Err(AppError::bad("only the active sprint can be completed"));
    }
    if let Some(m) = b.move_to {
        let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM scrum_sprints WHERE id = $1 AND project_id = $2 AND status = 'planned') AS "v!""#, m, project)
            .fetch_one(&state.db)
            .await?;
        if !ok {
            return Err(AppError::bad("unfinished tasks can move to the backlog or a planned sprint of this team"));
        }
    }
    let mut tx = state.db.begin().await?;
    sqlx::query!("INSERT INTO scrum_sprint_results (sprint_id, task_id, points, done)
         SELECT $1, id, story_points, status = 'done' FROM tasks WHERE sprint_id = $1", sid)
    .execute(&mut *tx)
    .await?;
    let moved: Vec<Uuid> = match b.move_to {
        Some(m) => sqlx::query_scalar!("UPDATE tasks SET sprint_id = $2,
             backlog_sort = backlog_sort - 1000 + COALESCE((SELECT MIN(backlog_sort) FROM tasks WHERE sprint_id = $2), 0)
             WHERE sprint_id = $1 AND status <> 'done' RETURNING id", sid, m)
        .fetch_all(&mut *tx)
        .await?,
        None => sqlx::query_scalar!("UPDATE tasks SET sprint_id = NULL,
             backlog_sort = backlog_sort - 1000 + COALESCE((SELECT MIN(backlog_sort) FROM tasks WHERE project_id = $2 AND sprint_id IS NULL), 0)
             WHERE sprint_id = $1 AND status <> 'done' RETURNING id", sid, project)
        .fetch_all(&mut *tx)
        .await?,
    };
    sqlx::query!("UPDATE scrum_sprints SET status = 'completed', completed_at = now(),
         completed_points = (SELECT COALESCE(SUM(points), 0) FROM scrum_sprint_results WHERE sprint_id = $1 AND done)
         WHERE id = $1", sid)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let name: String = sqlx::query_scalar!("SELECT name FROM scrum_sprints WHERE id = $1", sid).fetch_one(&state.db).await?;
    let left = if moved.is_empty() { String::new() } else { format!(" · {} unfinished moved on", moved.len()) };
    notify_team(&state, project, &auth, format!("{name} completed{left}"), "scrum.sprint_completed").await;
    broadcast(&state, project).await;
    Ok(Json(load_sprint(&state, sid).await?))
}

async fn notify_team(state: &AppState, project: Uuid, auth: &AuthUser, title: String, kind: &'static str) {
    let members: Vec<Uuid> = sqlx::query_scalar!("SELECT user_id FROM project_members WHERE project_id = $1 AND user_id <> $2", project, auth.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    crate::notify::notify(state, &members, crate::notify::NewNotification {
        kind,
        title,
        body: None,
        link: Some(format!("/projects/{project}?tab=sprint")),
        actor: Some(auth.id),
    })
    .await;
}

// ---------------------------------------------------------------------------
// Burndown and velocity
// ---------------------------------------------------------------------------

async fn burndown(State(state): State<AppState>, auth: AuthUser, Path(sid): Path<Uuid>) -> AppResult<Json<Value>> {
    let (project, status) = sprint_project(&state, sid).await?;
    team(&state, &auth, project).await?;
    if status == "planned" {
        return Err(AppError::bad("the sprint hasn't started"));
    }
    let (starts, ends, committed, completed_at): (DateTime<Utc>, DateTime<Utc>, f64, Option<DateTime<Utc>>) =
        sqlx::query!(r#"SELECT starts_at AS "starts_at!", ends_at AS "ends_at!", COALESCE(committed_points, 0) AS "c2!", completed_at AS "completed_at?" FROM scrum_sprints WHERE id = $1"#, sid).map(|r| (r.starts_at, r.ends_at, r.c2, r.completed_at))
            .fetch_one(&state.db)
            .await?;
    // Points finished, and when.
    let done: Vec<(DateTime<Utc>, f64)> = if status == "completed" {
        sqlx::query_as(
            "SELECT t.completed_at, COALESCE(r.points, 0) FROM scrum_sprint_results r JOIN tasks t ON t.id = r.task_id
             WHERE r.sprint_id = $1 AND r.done AND t.completed_at IS NOT NULL",
        )
        .bind(sid)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as("SELECT completed_at, COALESCE(story_points, 0) FROM tasks WHERE sprint_id = $1 AND status = 'done' AND completed_at IS NOT NULL")
            .bind(sid)
            .fetch_all(&state.db)
            .await?
    };
    let scope: Vec<(DateTime<Utc>, f64)> = sqlx::query!(r#"SELECT created_at, points FROM scrum_scope_changes WHERE sprint_id = $1"#, sid).map(|r| (r.created_at, r.points))
        .fetch_all(&state.db)
        .await?;

    let first: NaiveDate = starts.date_naive();
    // A sprint completed early ends there.
    let last: NaiveDate = completed_at.map_or(ends, |c| c.min(ends)).date_naive().max(first);
    // Points left are known up to today, or up to the day the sprint was completed.
    let known = completed_at.map_or(Utc::now(), |c| c).date_naive();
    let total_days = (last - first).num_days().max(1) as f64;
    let mut days = Vec::new();
    let mut d = first;
    while d <= last {
        let end_of_day = d.and_hms_opt(23, 59, 59).unwrap().and_utc();
        let ideal = committed * (1.0 - (d - first).num_days() as f64 / total_days);
        let remaining = if d <= known {
            let added: f64 = scope.iter().filter(|(at, _)| *at <= end_of_day).map(|(_, p)| p).sum();
            // Tasks done before the sprint started were never part of the commitment.
            let burned: f64 = done.iter().filter(|(at, _)| *at <= end_of_day && *at >= starts).map(|(_, p)| p).sum();
            Some(((committed + added - burned) * 10.0).round() / 10.0)
        } else {
            None
        };
        days.push(json!({ "date": d, "ideal": (ideal * 10.0).round() / 10.0, "remaining": remaining }));
        d = d.succ_opt().unwrap();
    }
    let added: f64 = scope.iter().filter(|(_, p)| *p > 0.0).map(|(_, p)| p).sum();
    let removed: f64 = -scope.iter().filter(|(_, p)| *p < 0.0).map(|(_, p)| p).sum::<f64>();
    Ok(Json(json!({ "committed": committed, "scope_added": added, "scope_removed": removed, "days": days })))
}

async fn velocity(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    team(&state, &auth, id).await?;
    let rows: Vec<(Uuid, String, Option<DateTime<Utc>>, f64, f64)> = sqlx::query!(r#"SELECT id AS "id!", name AS "name!", completed_at AS "completed_at?", COALESCE(committed_points, 0) AS "c3!", COALESCE(completed_points, 0) AS "c4!"
         FROM scrum_sprints WHERE project_id = $1 AND status = 'completed' ORDER BY completed_at DESC LIMIT 12"#, id).map(|r| (r.id, r.name, r.completed_at, r.c3, r.c4))
    .fetch_all(&state.db)
    .await?;
    let recent: Vec<f64> = rows.iter().take(3).map(|r| r.4).collect();
    let average = if recent.is_empty() { 0.0 } else { recent.iter().sum::<f64>() / recent.len() as f64 };
    let sprints: Vec<Value> = rows
        .into_iter()
        .rev()
        .map(|(id, name, at, c, d)| json!({ "id": id, "name": name, "completed_at": at, "committed": c, "completed": d }))
        .collect();
    Ok(Json(json!({ "sprints": sprints, "average": (average * 10.0).round() / 10.0 })))
}

// ---------------------------------------------------------------------------
// Epics
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct EpicInput {
    name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    description: Option<Option<String>>,
    color: Option<String>,
}

fn epic_color(c: Option<&str>) -> AppResult<()> {
    if c.is_some_and(|c| !EPIC_COLORS.contains(&c)) { Err(AppError::bad("unknown colour")) } else { Ok(()) }
}

async fn list_epics(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Vec<Epic>>> {
    team(&state, &auth, id).await?;
    Ok(Json(load_epics(&state, id).await?))
}

async fn create_epic(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<EpicInput>) -> AppResult<Json<Vec<Epic>>> {
    if !team(&state, &auth, id).await?.contribute {
        return Err(AppError::Forbidden);
    }
    let name = required("epic name", b.name.as_deref().unwrap_or(""), 200)?;
    epic_color(b.color.as_deref())?;
    sqlx::query!("INSERT INTO scrum_epics (project_id, name, description, color, created_by) VALUES ($1, $2, $3, $4, $5)", id, name, clean(b.description.flatten()), b.color.unwrap_or_else(|| "violet".into()), auth.id)
        .execute(&state.db)
        .await?;
    broadcast(&state, id).await;
    Ok(Json(load_epics(&state, id).await?))
}

async fn epic_project(state: &AppState, epic: Uuid) -> AppResult<Uuid> {
    sqlx::query_scalar!("SELECT project_id FROM scrum_epics WHERE id = $1", epic)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("epic"))
}

async fn update_epic(State(state): State<AppState>, auth: AuthUser, Path(eid): Path<Uuid>, Json(b): Json<EpicInput>) -> AppResult<Json<Vec<Epic>>> {
    let project = epic_project(&state, eid).await?;
    if !team(&state, &auth, project).await?.contribute {
        return Err(AppError::Forbidden);
    }
    epic_color(b.color.as_deref())?;
    if let Some(n) = &b.name {
        sqlx::query!("UPDATE scrum_epics SET name = $1 WHERE id = $2", required("epic name", n, 200)?, eid).execute(&state.db).await?;
    }
    if let Some(d) = b.description {
        sqlx::query!("UPDATE scrum_epics SET description = $1 WHERE id = $2", clean(d), eid).execute(&state.db).await?;
    }
    if let Some(c) = &b.color {
        sqlx::query!("UPDATE scrum_epics SET color = $1 WHERE id = $2", c, eid).execute(&state.db).await?;
    }
    broadcast(&state, project).await;
    Ok(Json(load_epics(&state, project).await?))
}

/// Deletes an epic; its tasks stay, without an epic.
async fn delete_epic(State(state): State<AppState>, auth: AuthUser, Path(eid): Path<Uuid>) -> AppResult<Json<Vec<Epic>>> {
    let project = epic_project(&state, eid).await?;
    require_plan(&state, &auth, project).await?;
    sqlx::query!("DELETE FROM scrum_epics WHERE id = $1", eid).execute(&state.db).await?;
    broadcast(&state, project).await;
    Ok(Json(load_epics(&state, project).await?))
}

// ---------------------------------------------------------------------------
// Definition of done
// ---------------------------------------------------------------------------

async fn get_dod(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Vec<DodItem>>> {
    team(&state, &auth, id).await?;
    Ok(Json(load_dod(&state, id).await?))
}

#[derive(Deserialize)]
struct DodInput {
    items: Vec<String>,
}

async fn set_dod(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<DodInput>) -> AppResult<Json<Vec<DodItem>>> {
    require_plan(&state, &auth, id).await?;
    let items: Vec<String> = b.items.into_iter().filter_map(|i| clean(Some(i))).map(|i| i.chars().take(300).collect()).collect();
    if items.len() > 50 {
        return Err(AppError::bad("a definition of done has at most 50 points"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query!("DELETE FROM scrum_dod_items WHERE project_id = $1", id).execute(&mut *tx).await?;
    for (i, t) in items.iter().enumerate() {
        sqlx::query!("INSERT INTO scrum_dod_items (project_id, title, sort) VALUES ($1, $2, $3)", id, t, i as i32).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    broadcast(&state, id).await;
    Ok(Json(load_dod(&state, id).await?))
}

#[derive(Deserialize)]
struct ConfirmDod {
    /// The ids of the definition-of-done points ticked; all of them must be.
    checked: Vec<Uuid>,
}

/// Records that the task meets the team's definition of done (before it is finished).
async fn confirm_dod(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ConfirmDod>) -> AppResult<Json<Value>> {
    let (core, access) = load_task(&state, &auth, id).await?;
    if !access.participate {
        return Err(AppError::Forbidden);
    }
    let Some(project) = core.project_id else {
        return Err(AppError::bad("only a Scrum team's tasks have a definition of done"));
    };
    let items = load_dod(&state, project).await?;
    if let Some(missing) = items.iter().find(|i| !b.checked.contains(&i.id)) {
        return Err(AppError::bad(format!("not done yet: {}", missing.title)));
    }
    sqlx::query!("UPDATE tasks SET dod_confirmed_at = now() WHERE id = $1", id).execute(&state.db).await?;
    record_history(&state.db, id, auth.id, "dod", None, Some(format!("{} of {} points", items.len(), items.len()))).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Placing a task: estimate, epic, sprint and rank
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Place {
    #[serde(default, deserialize_with = "double_option")]
    story_points: Option<Option<f64>>,
    #[serde(default, deserialize_with = "double_option")]
    epic_id: Option<Option<Uuid>>,
    /// A sprint of the team, or null for the backlog.
    #[serde(default, deserialize_with = "double_option")]
    sprint_id: Option<Option<Uuid>>,
    /// Rank: between these neighbours in the target list.
    after_id: Option<Uuid>,
    before_id: Option<Uuid>,
}

async fn place(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Place>) -> AppResult<Json<crate::tasks::items::TaskSummary>> {
    let (core, _) = load_task(&state, &auth, id).await?;
    let Some(project) = core.project_id else {
        return Err(AppError::bad("only a Scrum team's tasks go in sprints"));
    };
    if !team(&state, &auth, project).await?.contribute {
        return Err(AppError::Forbidden);
    }
    let (cur_points, cur_epic, cur_sprint): (Option<f64>, Option<Uuid>, Option<Uuid>) =
        sqlx::query!(r#"SELECT story_points, epic_id, sprint_id FROM tasks WHERE id = $1"#, id).map(|r| (r.story_points, r.epic_id, r.sprint_id)).fetch_one(&state.db).await?;
    let points = b.story_points.unwrap_or(cur_points);
    let epic = b.epic_id.unwrap_or(cur_epic);
    let sprint = b.sprint_id.unwrap_or(cur_sprint);
    if sprint != cur_sprint {
        if let Some(s) = cur_sprint {
            let st: String = sqlx::query_scalar!("SELECT status FROM scrum_sprints WHERE id = $1", s).fetch_one(&state.db).await?;
            if st == "completed" {
                return Err(AppError::bad("tasks of a completed sprint stay in it"));
            }
        }
    }
    check_placement(&state, Some(project), points, epic, sprint).await?;

    let pos = |n: Option<Uuid>| {
        let db = state.db.clone();
        async move {
            match n {
                Some(n) => sqlx::query_scalar!("SELECT backlog_sort FROM tasks WHERE id = $1", n).fetch_optional(&db).await,
                None => Ok(None),
            }
        }
    };
    let rank = match (pos(b.after_id).await?, pos(b.before_id).await?) {
        (Some(a), Some(c)) => Some((a + c) / 2.0),
        (Some(a), None) => Some(a + 1.0),
        (None, Some(c)) => Some(c - 1.0),
        (None, None) if sprint != cur_sprint => Some(
            sqlx::query_scalar!(r#"SELECT MAX(backlog_sort) AS "v?" FROM tasks WHERE project_id = $1 AND sprint_id IS NOT DISTINCT FROM $2 AND id <> $3"#, project, sprint, id)
                .fetch_one(&state.db)
                .await?
                .unwrap_or(0.0)
                + 1.0,
        ),
        (None, None) => None,
    };

    let mut tx = state.db.begin().await?;
    sqlx::query!("UPDATE tasks SET story_points = $2, epic_id = $3, sprint_id = $4, backlog_sort = COALESCE($5, backlog_sort), updated_at = now() WHERE id = $1", id, points, epic, sprint, rank)
        .execute(&mut *tx)
        .await?;
    // Scope changes of a running sprint, for its burndown.
    let active: Option<Uuid> = sqlx::query_scalar!("SELECT id FROM scrum_sprints WHERE project_id = $1 AND status = 'active'", project)
        .fetch_optional(&mut *tx)
        .await?;
    if let Some(a) = active.filter(|_| core.status != "done") {
        let before = if cur_sprint == Some(a) { cur_points.unwrap_or(0.0) } else { 0.0 };
        let after = if sprint == Some(a) { points.unwrap_or(0.0) } else { 0.0 };
        if (after - before).abs() > f64::EPSILON {
            sqlx::query!("INSERT INTO scrum_scope_changes (sprint_id, task_id, points) VALUES ($1, $2, $3)", a, id, after - before)
                .execute(&mut *tx)
                .await?;
        }
    }
    if points != cur_points {
        record_history(&mut *tx, id, auth.id, "story_points", cur_points.map(fmt_points), points.map(fmt_points)).await?;
    }
    if sprint != cur_sprint {
        let name = |s: Option<Uuid>| {
            let db = state.db.clone();
            async move {
                match s {
                    Some(s) => sqlx::query_scalar!("SELECT name FROM scrum_sprints WHERE id = $1", s).fetch_optional(&db).await.ok().flatten(),
                    None => Some("Backlog".into()),
                }
            }
        };
        record_history(&mut *tx, id, auth.id, "sprint", name(cur_sprint).await, name(sprint).await).await?;
    }
    if epic != cur_epic {
        let name: Option<String> = match epic {
            Some(e) => sqlx::query_scalar!("SELECT name FROM scrum_epics WHERE id = $1", e).fetch_optional(&mut *tx).await?,
            None => None,
        };
        record_history(&mut *tx, id, auth.id, "epic", None, name).await?;
    }
    tx.commit().await?;
    broadcast_changed(&state, id, Some(project), false).await;
    broadcast(&state, project).await;
    Ok(Json(crate::tasks::items::load_summary(&state, id).await?))
}

fn fmt_points(p: f64) -> String {
    if p.fract() == 0.0 { format!("{}", p as i64) } else { format!("{p}") }
}
