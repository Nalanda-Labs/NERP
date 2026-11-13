//! Custom board columns per project.
//!
//! Columns are independent of task status, but a column may be *linked* to a status:
//! - dropping a card into a linked column applies that status (subject to the normal rules,
//!   so "Done" on a review task lands it in review instead);
//! - when a task's status changes anywhere else, it moves to the first column linked to the
//!   new status, unless it sits in an unlinked column and is merely being worked on.

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    access::{load_task, project_access, STATUSES},
    broadcast_changed,
    items::{apply_status, load_detail, TaskDetail},
    record_history,
};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::{double_option, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{id}/stages", get(list).post(create))
        .route("/projects/{id}/stages/{stage}", patch(update).delete(remove))
        .route("/tasks/{id}/stage", post(move_to_stage))
}

pub const COLORS: &[&str] = &["slate", "sky", "indigo", "violet", "emerald", "amber", "orange", "rose", "teal"];

#[derive(Serialize, FromRow, Clone)]
pub struct Stage {
    pub id: Uuid,
    pub name: String,
    pub color: String,
    pub sort: f64,
    pub status: Option<String>,
}

pub async fn load_stages(state: &AppState, project: Uuid) -> AppResult<Vec<Stage>> {
    Ok(sqlx::query_as!(Stage, r#"SELECT id, name, color, sort, status AS "status?" FROM project_stages WHERE project_id = $1 ORDER BY sort, created_at"#, project)
        .fetch_all(&state.db)
        .await?)
}

/// The column a task in `project` with `status` belongs in by default.
pub async fn default_stage<'e, E: sqlx::PgExecutor<'e>>(executor: E, project: Uuid, status: &str) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar!("SELECT id FROM project_stages WHERE project_id = $1
         ORDER BY (status IS NOT DISTINCT FROM $2) DESC, (status IS NULL) DESC, sort LIMIT 1", project, status)
    .fetch_optional(executor)
    .await
}

/// Keeps a task's column in step after its status changed. Returns the column it moved to.
pub async fn sync_after_status(state: &AppState, task: Uuid, new_status: &str) -> AppResult<Option<Uuid>> {
    let row: Option<(Option<Uuid>, Option<Uuid>, Option<String>)> = sqlx::query!(r#"SELECT t.project_id, t.stage_id, s.status FROM tasks t LEFT JOIN project_stages s ON s.id = t.stage_id WHERE t.id = $1"#, task).map(|r| (r.project_id, r.stage_id, r.status))
    .fetch_optional(&state.db)
    .await?;
    let Some((Some(project), stage, linked)) = row else { return Ok(None) };
    if linked.as_deref() == Some(new_status) {
        return Ok(None);
    }
    // A card in a custom (unlinked) column stays put while work is ongoing.
    if stage.is_some() && linked.is_none() && (new_status == "new" || new_status == "in_progress") {
        return Ok(None);
    }
    let target: Option<Uuid> = sqlx::query_scalar!("SELECT id FROM project_stages WHERE project_id = $1 AND status = $2 ORDER BY sort LIMIT 1", project, new_status)
    .fetch_optional(&state.db)
    .await?;
    if let Some(target) = target.filter(|t| Some(*t) != stage) {
        sqlx::query!("UPDATE tasks SET stage_id = $1 WHERE id = $2", target, task).execute(&state.db).await?;
        return Ok(Some(target));
    }
    Ok(None)
}

async fn list(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Vec<Stage>>> {
    project_access(&state, &auth, id).await?;
    Ok(Json(load_stages(&state, id).await?))
}

async fn require_manage(state: &AppState, auth: &AuthUser, project: Uuid) -> AppResult<()> {
    if project_access(state, auth, project).await?.manage { Ok(()) } else { Err(AppError::Forbidden) }
}

fn validate(color: Option<&str>, status: Option<&str>) -> AppResult<()> {
    if color.is_some_and(|c| !COLORS.contains(&c)) {
        return Err(AppError::bad("unknown colour"));
    }
    if status.is_some_and(|s| !STATUSES.contains(&s)) {
        return Err(AppError::bad("unknown status"));
    }
    Ok(())
}

#[derive(Deserialize)]
struct NewStage {
    name: String,
    color: Option<String>,
    status: Option<String>,
    /// Insert after this column; at the end when absent.
    after_id: Option<Uuid>,
}

async fn create(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(body): Json<NewStage>) -> AppResult<Json<Vec<Stage>>> {
    require_manage(&state, &auth, id).await?;
    let name = required("column name", &body.name, 60)?;
    validate(body.color.as_deref(), body.status.as_deref())?;
    let stages = load_stages(&state, id).await?;
    if stages.len() >= 20 {
        return Err(AppError::bad("a board can have at most 20 columns"));
    }
    let sort = match body.after_id.and_then(|a| stages.iter().position(|s| s.id == a)) {
        Some(i) => match stages.get(i + 1) {
            Some(next) => (stages[i].sort + next.sort) / 2.0,
            None => stages[i].sort + 1.0,
        },
        None => stages.last().map(|s| s.sort + 1.0).unwrap_or(1.0),
    };
    sqlx::query!("INSERT INTO project_stages (project_id, name, color, sort, status) VALUES ($1, $2, $3, $4, $5)", id, name, body.color.unwrap_or_else(|| "slate".into()), sort, body.status)
        .execute(&state.db)
        .await?;
    broadcast_board(&state, id).await;
    Ok(Json(load_stages(&state, id).await?))
}

#[derive(Deserialize)]
struct UpdateStage {
    name: Option<String>,
    color: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    status: Option<Option<String>>,
    sort: Option<f64>,
}

async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, stage)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateStage>,
) -> AppResult<Json<Vec<Stage>>> {
    require_manage(&state, &auth, id).await?;
    validate(body.color.as_deref(), body.status.as_ref().and_then(|s| s.as_deref()))?;
    let exists: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM project_stages WHERE id = $1 AND project_id = $2) AS "v!""#, stage, id)
        .fetch_one(&state.db)
        .await?;
    if !exists {
        return Err(AppError::NotFound("column"));
    }
    if let Some(n) = &body.name {
        sqlx::query!("UPDATE project_stages SET name = $1 WHERE id = $2", required("column name", n, 60)?, stage).execute(&state.db).await?;
    }
    if let Some(c) = &body.color {
        sqlx::query!("UPDATE project_stages SET color = $1 WHERE id = $2", c, stage).execute(&state.db).await?;
    }
    if let Some(s) = &body.status {
        sqlx::query!("UPDATE project_stages SET status = $1 WHERE id = $2", s.as_deref(), stage).execute(&state.db).await?;
    }
    if let Some(s) = body.sort {
        sqlx::query!("UPDATE project_stages SET sort = $1 WHERE id = $2", s, stage).execute(&state.db).await?;
    }
    broadcast_board(&state, id).await;
    Ok(Json(load_stages(&state, id).await?))
}

#[derive(Deserialize)]
struct RemoveQuery {
    /// Column that receives the deleted column's cards; defaults to the first remaining one.
    move_to: Option<Uuid>,
}

async fn remove(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, stage)): Path<(Uuid, Uuid)>,
    Query(q): Query<RemoveQuery>,
) -> AppResult<Json<Vec<Stage>>> {
    require_manage(&state, &auth, id).await?;
    let stages = load_stages(&state, id).await?;
    if !stages.iter().any(|s| s.id == stage) {
        return Err(AppError::NotFound("column"));
    }
    let remaining: Vec<&Stage> = stages.iter().filter(|s| s.id != stage).collect();
    if remaining.is_empty() {
        return Err(AppError::bad("a board needs at least one column"));
    }
    let target = q
        .move_to
        .filter(|m| remaining.iter().any(|s| s.id == *m))
        .unwrap_or(remaining[0].id);
    let mut tx = state.db.begin().await?;
    sqlx::query!("UPDATE tasks SET stage_id = $1 WHERE stage_id = $2", target, stage).execute(&mut *tx).await?;
    sqlx::query!("DELETE FROM project_stages WHERE id = $1", stage).execute(&mut *tx).await?;
    tx.commit().await?;
    broadcast_board(&state, id).await;
    Ok(Json(load_stages(&state, id).await?))
}

/// Column changes affect everyone looking at the board.
async fn broadcast_board(state: &AppState, project: Uuid) {
    let members: Vec<Uuid> = sqlx::query_scalar!("SELECT user_id FROM project_members WHERE project_id = $1", project)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    crate::realtime::publish(state, Some(members), "project.stages", json!({ "project_id": project })).await;
}

#[derive(Deserialize)]
struct MoveToStage {
    stage_id: Uuid,
    after_id: Option<Uuid>,
    before_id: Option<Uuid>,
}

/// Drag-and-drop on a project board with custom columns.
async fn move_to_stage(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<MoveToStage>,
) -> AppResult<Json<TaskDetail>> {
    let (core, access) = load_task(&state, &auth, id).await?;
    if !access.participate {
        return Err(AppError::Forbidden);
    }
    let Some(project) = core.project_id else {
        return Err(AppError::bad("only project tasks have board columns"));
    };
    let stage: Option<(String, Option<String>)> =
        sqlx::query!(r#"SELECT name, status FROM project_stages WHERE id = $1 AND project_id = $2"#, body.stage_id, project).map(|r| (r.name, r.status))
            .fetch_optional(&state.db)
            .await?;
    let Some((stage_name, linked)) = stage else {
        return Err(AppError::bad("that column belongs to another project"));
    };

    let pos = |n: Option<Uuid>| {
        let db = state.db.clone();
        async move {
            match n {
                Some(n) => sqlx::query_scalar!("SELECT position FROM tasks WHERE id = $1", n).fetch_optional(&db).await,
                None => Ok(None),
            }
        }
    };
    let position = match (pos(body.after_id).await?, pos(body.before_id).await?) {
        (Some(a), Some(b)) => (a + b) / 2.0,
        (Some(a), None) => a + 1.0,
        (None, Some(b)) => b - 1.0,
        (None, None) => 0.0,
    };

    let previous: Option<Uuid> = sqlx::query_scalar!(r#"SELECT stage_id AS "stage_id?" FROM tasks WHERE id = $1"#, id).fetch_one(&state.db).await?;
    sqlx::query!("UPDATE tasks SET stage_id = $1, position = $2, updated_at = now(), last_activity_at = now() WHERE id = $3", body.stage_id, position, id)
        .execute(&state.db)
        .await?;
    if previous != Some(body.stage_id) {
        record_history(&state.db, id, auth.id, "stage", None, Some(stage_name)).await?;
        super::automation::stage_entered(&state, id, body.stage_id, auth.id).await;
    }
    match linked {
        // apply_status syncs the column too (e.g. Done → Review for tasks that need review).
        Some(status) if status != core.status => Ok(Json(apply_status(&state, &auth, id, &status, None).await?)),
        _ => {
            broadcast_changed(&state, id, core.project_id, false).await;
            Ok(Json(load_detail(&state, &auth, id).await?))
        }
    }
}
