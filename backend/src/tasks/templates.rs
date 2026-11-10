//! Task templates and recurring tasks.
//!
//! A template is a saved task blueprint. Using it creates a task now; giving it a
//! recurrence makes the background job create tasks on schedule, on behalf of the
//! template's owner, with the deadline measured from each scheduled time.

use anyhow::Result;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    access::load_task,
    items::{create_task, load_detail, CreateTask, TaskDetail},
    recurrence::Recurrence,
};
use crate::{
    auth::{acting_as, permissions as perm, AuthUser},
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::{clean, required},
    state::AppState,
};

pub const JOB: &str = "tasks.recurring";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/task-templates", get(list).post(create))
        .route("/task-templates/{id}", get(detail).patch(update).delete(remove))
        .route("/task-templates/{id}/use", post(use_template))
        .route("/tasks/{id}/save-as-template", post(save_as_template))
}

#[derive(Serialize, FromRow)]
pub struct Template {
    id: Uuid,
    title: String,
    description: Option<String>,
    responsible_id: Option<Uuid>,
    responsible_name: Option<String>,
    assistant_ids: Vec<Uuid>,
    watcher_ids: Vec<Uuid>,
    priority: String,
    project_id: Option<Uuid>,
    project_name: Option<String>,
    requires_review: bool,
    time_estimate_secs: Option<i32>,
    checklist: Vec<String>,
    deadline_offset_secs: Option<i32>,
    recurrence: Option<Value>,
    next_run_at: Option<DateTime<Utc>>,
    last_run_at: Option<DateTime<Utc>>,
    run_count: i32,
    is_active: bool,
    created_by: Uuid,
    created_at: DateTime<Utc>,
    /// Subtasks made along with the task: [{title, responsible_id?, deadline_offset_secs?}].
    subtasks: Value,
    /// Everyone may see and use it.
    shared: bool,
    created_by_name: Option<String>,
    #[sqlx(skip)]
    schedule: Option<String>,
    #[sqlx(skip)]
    task_count: i64,
}

const SELECT: &str = "SELECT tt.id, tt.title, tt.description, tt.responsible_id,
    CASE WHEN r.id IS NULL THEN NULL ELSE r.first_name || ' ' || r.last_name END AS responsible_name,
    tt.assistant_ids, tt.watcher_ids, tt.priority, tt.project_id, p.name AS project_name, tt.requires_review,
    tt.time_estimate_secs, tt.checklist, tt.deadline_offset_secs, tt.recurrence, tt.next_run_at, tt.last_run_at,
    tt.run_count, tt.is_active, tt.created_by, tt.created_at, tt.subtasks, tt.shared,
    (SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = tt.created_by) AS created_by_name
    FROM task_templates tt LEFT JOIN users r ON r.id = tt.responsible_id LEFT JOIN projects p ON p.id = tt.project_id";

fn rule(v: &Option<Value>) -> Option<Recurrence> {
    v.as_ref().and_then(|v| serde_json::from_value(v.clone()).ok())
}

async fn finish(state: &AppState, mut t: Template) -> AppResult<Template> {
    t.schedule = rule(&t.recurrence).map(|r| r.describe());
    t.task_count = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE template_id = $1")
        .bind(t.id)
        .fetch_one(&state.db)
        .await?;
    Ok(t)
}

/// Loads a template the caller owns (or any, with tasks.manage_all).
async fn load_own(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Template> {
    let t: Template = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE tt.id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("template"))?;
    if t.created_by != auth.id && !auth.can(perm::TASKS_MANAGE_ALL) {
        return Err(AppError::NotFound("template"));
    }
    finish(state, t).await
}

/// A template the caller may use: their own, or one shared with everyone.
async fn load_usable(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Template> {
    let t: Template = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE tt.id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("template"))?;
    if t.created_by != auth.id && !t.shared && !auth.can(perm::TASKS_MANAGE_ALL) {
        return Err(AppError::NotFound("template"));
    }
    finish(state, t).await
}

/// Mine, and those shared with everyone.
async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Vec<Template>>> {
    let rows: Vec<Template> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE tt.created_by = $1 OR tt.shared ORDER BY (tt.created_by <> $1), (tt.recurrence IS NULL), tt.title"
    )))
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for t in rows {
        out.push(finish(&state, t).await?);
    }
    Ok(Json(out))
}

async fn detail(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Template>> {
    Ok(Json(load_usable(&state, &auth, id).await?))
}

#[derive(Deserialize, Default)]
pub struct TemplateInput {
    title: Option<String>,
    description: Option<String>,
    responsible_id: Option<Uuid>,
    assistant_ids: Option<Vec<Uuid>>,
    watcher_ids: Option<Vec<Uuid>>,
    priority: Option<String>,
    project_id: Option<Uuid>,
    requires_review: Option<bool>,
    time_estimate_secs: Option<i32>,
    checklist: Option<Vec<String>>,
    deadline_offset_secs: Option<i32>,
    /// `null` makes the template non-recurring.
    recurrence: Option<Recurrence>,
    is_active: Option<bool>,
    subtasks: Option<Vec<SubtaskInput>>,
    shared: Option<bool>,
}

#[derive(Deserialize, Serialize, Clone)]
struct SubtaskInput {
    title: String,
    responsible_id: Option<Uuid>,
    deadline_offset_secs: Option<i32>,
}

fn validate(input: &TemplateInput) -> AppResult<()> {
    if let Some(p) = &input.priority
        && !["low", "normal", "high"].contains(&p.as_str()) {
            return Err(AppError::bad("priority must be low, normal or high"));
        }
    if input.deadline_offset_secs.is_some_and(|s| !(0..=366 * 86400).contains(&s)) {
        return Err(AppError::bad("the deadline must be within a year of creation"));
    }
    if input.time_estimate_secs.is_some_and(|s| s < 0) {
        return Err(AppError::bad("time estimate cannot be negative"));
    }
    if let Some(r) = &input.recurrence {
        r.validate()?;
    }
    Ok(())
}

/// Writes every field of a template (create and full update share this).
async fn save(state: &AppState, auth: &AuthUser, id: Option<Uuid>, input: TemplateInput) -> AppResult<Uuid> {
    validate(&input)?;
    let title = required("title", input.title.as_deref().unwrap_or(""), 500)?;
    if let Some(p) = input.project_id {
        let access = super::access::project_access(state, auth, p).await?;
        if !access.contribute {
            return Err(AppError::bad("you can't add tasks to that project"));
        }
    }
    let checklist: Vec<String> = input
        .checklist
        .unwrap_or_default()
        .into_iter()
        .filter_map(|c| clean(Some(c)))
        .map(|c| c.chars().take(1000).collect())
        .collect();
    let subtasks: Vec<SubtaskInput> = input
        .subtasks
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|mut s| clean(Some(s.title.clone())).map(|t| {
            s.title = t.chars().take(500).collect();
            s
        }))
        .collect();
    if subtasks.len() > 50 {
        return Err(AppError::bad("at most 50 subtasks"));
    }
    if subtasks.iter().any(|s| s.deadline_offset_secs.is_some_and(|d| !(0..=366 * 86400).contains(&d))) {
        return Err(AppError::bad("a subtask's deadline must be within a year"));
    }
    let next_run = input.recurrence.as_ref().and_then(|r| r.next_after(Utc::now()));
    let recurrence = input.recurrence.as_ref().map(|r| serde_json::to_value(r).unwrap());
    let is_active = input.is_active.unwrap_or(true);

    let shared = input.shared.unwrap_or(false);
    let id = match id {
        Some(id) => {
            sqlx::query(
                "UPDATE task_templates SET title = $1, description = $2, responsible_id = $3, assistant_ids = $4, watcher_ids = $5,
                     priority = $6, project_id = $7, requires_review = $8, time_estimate_secs = $9, checklist = $10,
                     deadline_offset_secs = $11, recurrence = $12, next_run_at = $13, is_active = $14, updated_at = now()
                 WHERE id = $15",
            )
            .bind(&title)
            .bind(clean(input.description))
            .bind(input.responsible_id)
            .bind(input.assistant_ids.unwrap_or_default())
            .bind(input.watcher_ids.unwrap_or_default())
            .bind(input.priority.unwrap_or_else(|| "normal".into()))
            .bind(input.project_id)
            .bind(input.requires_review.unwrap_or(false))
            .bind(input.time_estimate_secs)
            .bind(&checklist)
            .bind(input.deadline_offset_secs)
            .bind(recurrence)
            .bind(next_run)
            .bind(is_active)
            .bind(id)
            .execute(&state.db)
            .await?;
            sqlx::query("UPDATE task_templates SET subtasks = $2, shared = $3 WHERE id = $1").bind(id).bind(json!(subtasks)).bind(shared).execute(&state.db).await?;
            id
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO task_templates (title, description, responsible_id, assistant_ids, watcher_ids, priority, project_id,
                     requires_review, time_estimate_secs, checklist, deadline_offset_secs, recurrence, next_run_at, is_active, created_by)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15) RETURNING id",
            )
            .bind(&title)
            .bind(clean(input.description))
            .bind(input.responsible_id)
            .bind(input.assistant_ids.unwrap_or_default())
            .bind(input.watcher_ids.unwrap_or_default())
            .bind(input.priority.unwrap_or_else(|| "normal".into()))
            .bind(input.project_id)
            .bind(input.requires_review.unwrap_or(false))
            .bind(input.time_estimate_secs)
            .bind(&checklist)
            .bind(input.deadline_offset_secs)
            .bind(recurrence)
            .bind(next_run)
            .bind(is_active)
            .bind(auth.id)
            .fetch_one(&state.db)
            .await?
        }
    };
    if has_extras(&subtasks, shared) {
        sqlx::query("UPDATE task_templates SET subtasks = $2, shared = $3 WHERE id = $1").bind(id).bind(json!(subtasks)).bind(shared).execute(&state.db).await?;
    }
    Ok(id)
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(input): Json<TemplateInput>) -> AppResult<Json<Template>> {
    let id = save(&state, &auth, None, input).await?;
    Ok(Json(load_own(&state, &auth, id).await?))
}

/// Full replacement of the template's settings.
async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(input): Json<TemplateInput>) -> AppResult<Json<Template>> {
    load_own(&state, &auth, id).await?;
    save(&state, &auth, Some(id), input).await?;
    Ok(Json(load_own(&state, &auth, id).await?))
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    load_own(&state, &auth, id).await?;
    sqlx::query("DELETE FROM task_templates WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Builds the task a template produces for a given moment.
fn task_from(t: &Template, at: DateTime<Utc>, creator: Uuid) -> CreateTask {
    CreateTask {
        title: t.title.clone(),
        description: t.description.clone(),
        responsible_id: Some(t.responsible_id.unwrap_or(creator)),
        assistant_ids: t.assistant_ids.clone(),
        watcher_ids: t.watcher_ids.clone(),
        start_date: t.deadline_offset_secs.map(|_| at),
        deadline: t.deadline_offset_secs.map(|s| at + Duration::seconds(s as i64)),
        priority: Some(t.priority.clone()),
        project_id: t.project_id,
        requires_review: t.requires_review,
        time_estimate_secs: t.time_estimate_secs,
        checklist: t.checklist.clone(),
        template_id: Some(t.id),
        ..Default::default()
    }
}

fn has_extras(subtasks: &[SubtaskInput], shared: bool) -> bool {
    !subtasks.is_empty() || shared
}

/// Creates the task a template produces, with the template's subtasks under it.
async fn make_from(state: &AppState, auth: &AuthUser, t: &Template, at: DateTime<Utc>) -> AppResult<Uuid> {
    let task = create_task(state, auth, task_from(t, at, auth.id)).await?;
    let subs: Vec<SubtaskInput> = serde_json::from_value(t.subtasks.clone()).unwrap_or_default();
    for s in subs {
        create_task(state, auth, CreateTask {
            title: s.title,
            responsible_id: Some(s.responsible_id.or(t.responsible_id).unwrap_or(auth.id)),
            deadline: s.deadline_offset_secs.map(|d| at + Duration::seconds(d as i64)),
            project_id: t.project_id,
            parent_id: Some(task),
            template_id: Some(t.id),
            ..Default::default()
        })
        .await?;
    }
    Ok(task)
}

async fn use_template(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<TaskDetail>> {
    let t = load_usable(&state, &auth, id).await?;
    let task = make_from(&state, &auth, &t, Utc::now()).await?;
    Ok(Json(load_detail(&state, &auth, task).await?))
}

#[derive(Deserialize, Default)]
struct SaveAsTemplate {
    recurrence: Option<Recurrence>,
}

/// Turns an existing task into a template (optionally recurring).
async fn save_as_template(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    body: Option<Json<SaveAsTemplate>>,
) -> AppResult<Json<Template>> {
    load_task(&state, &auth, id).await?;
    let d = load_detail(&state, &auth, id).await?;
    let ids = |role: &str| d.people_with_role(role);
    let offset = match (d.summary.deadline, d.summary.start_date) {
        (Some(dl), Some(st)) => Some((dl - st).num_seconds().clamp(0, 366 * 86400) as i32),
        (Some(dl), None) => Some((dl - d.summary.created_at).num_seconds().clamp(0, 366 * 86400) as i32),
        _ => None,
    };
    let input = TemplateInput {
        title: Some(d.summary.title.clone()),
        description: d.description.clone(),
        responsible_id: Some(d.summary.responsible_id),
        assistant_ids: Some(ids("assistant")),
        watcher_ids: Some(ids("watcher")),
        priority: Some(d.summary.priority.clone()),
        project_id: d.summary.project_id,
        requires_review: Some(d.summary.requires_review),
        time_estimate_secs: d.time_estimate_secs,
        checklist: Some(d.checklist_titles()),
        deadline_offset_secs: offset,
        recurrence: body.and_then(|b| b.0.recurrence),
        is_active: Some(true),
        // Its subtasks become the template's.
        subtasks: Some(d.subtask_titles().into_iter().map(|title| SubtaskInput { title, responsible_id: None, deadline_offset_secs: None }).collect()),
        shared: Some(false),
    };
    let tid = save(&state, &auth, None, input).await?;
    Ok(Json(load_own(&state, &auth, tid).await?))
}

// ---------------------------------------------------------------------------
// Recurring job
// ---------------------------------------------------------------------------

pub async fn run_due(state: &AppState) -> Result<()> {
    let due: Vec<Template> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE tt.is_active AND tt.recurrence IS NOT NULL AND tt.next_run_at <= now() ORDER BY tt.next_run_at LIMIT 100"
    )))
    .fetch_all(&state.db)
    .await?;

    for t in due {
        let Some(r) = rule(&t.recurrence) else { continue };
        let scheduled = t.next_run_at.unwrap_or_else(Utc::now);
        // After downtime, create one task for the missed slot and resume from now.
        let next = r.next_after(Utc::now().max(scheduled));
        let runs = t.run_count + 1;
        let finished = next.is_none() || r.max_runs.is_some_and(|m| runs >= m);

        // Claim this run: only the worker whose UPDATE matches the old next_run_at proceeds.
        let claimed = sqlx::query(
            "UPDATE task_templates SET next_run_at = $1, last_run_at = now(), run_count = run_count + 1,
                 is_active = $2, updated_at = now()
             WHERE id = $3 AND next_run_at = $4",
        )
        .bind(if finished { None } else { next })
        .bind(!finished)
        .bind(t.id)
        .bind(scheduled)
        .execute(&state.db)
        .await?
        .rows_affected();
        if claimed == 0 {
            continue;
        }

        let Some(owner) = acting_as(state, t.created_by).await? else {
            sqlx::query("UPDATE task_templates SET is_active = FALSE WHERE id = $1").bind(t.id).execute(&state.db).await?;
            tracing::warn!(template = %t.id, "recurring template paused: owner is deactivated");
            continue;
        };
        match make_from(state, &owner, &t, scheduled).await {
            Ok(task) => tracing::info!(template = %t.id, task = %task, "created recurring task"),
            Err(e) => {
                tracing::warn!(template = %t.id, error = %e, "recurring task could not be created");
                notify(
                    state,
                    &[t.created_by],
                    NewNotification {
                        kind: "task.recurring_failed",
                        title: format!("Couldn't create the recurring task \"{}\"", t.title),
                        body: Some(e.to_string()),
                        link: Some(format!("/tasks/templates/{}", t.id)),
                        actor: None,
                    },
                )
                .await;
            }
        }
    }
    Ok(())
}
