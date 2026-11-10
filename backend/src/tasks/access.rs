//! Who may see and change tasks and projects.
//!
//! A task is visible to:
//!   - its creator, responsible person, assistants and watchers;
//!   - members of its project, and everyone if the project is open;
//!   - the head of any department the responsible person belongs to, or of one above it;
//!   - holders of `tasks.manage_all`.
//!
//! Full editing (title, people, deadline, project, delete) belongs to the creator, the
//! project's owners, the responsible person's department head and `tasks.manage_all`.
//! The responsible person and assistants can move the task through its statuses, tick
//! checklist items and log time.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    state::AppState,
};

#[derive(Debug, Clone, FromRow)]
pub struct TaskCore {
    pub title: String,
    pub status: String,
    pub project_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    pub created_by: Uuid,
    pub responsible_id: Uuid,
    pub requires_review: bool,
    pub deadline: Option<DateTime<Utc>>,
    member_role: Option<String>,
    project_visibility: Option<String>,
    project_role: Option<String>,
    is_supervisor: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct TaskAccess {
    pub view: bool,
    /// Change title, description, people, deadline, project; delete.
    pub edit: bool,
    /// Change status, tick checklist items, log time.
    pub participate: bool,
    pub delete: bool,
    pub is_creator: bool,
    pub is_responsible: bool,
    pub is_assistant: bool,
    pub is_watcher: bool,
}

pub async fn load_task(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<(TaskCore, TaskAccess)> {
    let core: TaskCore = sqlx::query_as(
        "SELECT t.id, t.number, t.title, t.status, t.project_id, t.parent_id, t.created_by, t.responsible_id,
                t.requires_review, t.deadline,
                (SELECT m.role FROM task_members m WHERE m.task_id = t.id AND m.user_id = $2) AS member_role,
                p.visibility AS project_visibility,
                (SELECT pm.role FROM project_members pm WHERE pm.project_id = t.project_id AND pm.user_id = $2) AS project_role,
                EXISTS (WITH RECURSIVE tree AS (SELECT id FROM departments WHERE head_user_id = $2
                                               UNION ALL SELECT d.id FROM departments d JOIN tree ON d.parent_id = tree.id)
                        SELECT 1 FROM user_departments ud JOIN tree ON tree.id = ud.department_id WHERE ud.user_id = t.responsible_id) AS is_supervisor
         FROM tasks t LEFT JOIN projects p ON p.id = t.project_id
         WHERE t.id = $1",
    )
    .bind(id)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("task"))?;

    let access = compute(&core, auth);
    if !access.view {
        return Err(AppError::NotFound("task"));
    }
    Ok((core, access))
}

fn compute(t: &TaskCore, auth: &AuthUser) -> TaskAccess {
    let all = auth.can(perm::TASKS_MANAGE_ALL);
    let is_creator = t.created_by == auth.id;
    let is_responsible = t.responsible_id == auth.id;
    let is_assistant = t.member_role.as_deref() == Some("assistant");
    let is_watcher = t.member_role.as_deref() == Some("watcher");
    let project_owner = t.project_role.as_deref() == Some("owner");
    let view = all
        || is_creator
        || is_responsible
        || t.member_role.is_some()
        || t.project_role.is_some()
        || t.project_visibility.as_deref() == Some("open")
        || t.is_supervisor;
    let edit = all || is_creator || project_owner || t.is_supervisor;
    TaskAccess {
        view,
        edit,
        participate: edit || is_responsible || is_assistant,
        delete: all || is_creator || project_owner,
        is_creator,
        is_responsible,
        is_assistant,
        is_watcher,
    }
}

/// Appends `AND <task t is visible to user>` to a query over `tasks t`.
pub fn push_visible(qb: &mut QueryBuilder<Postgres>, auth: &AuthUser) {
    if auth.can(perm::TASKS_MANAGE_ALL) {
        return;
    }
    let u = auth.id;
    qb.push(" AND (t.created_by = ").push_bind(u)
        .push(" OR t.responsible_id = ").push_bind(u)
        .push(" OR EXISTS (SELECT 1 FROM task_members m WHERE m.task_id = t.id AND m.user_id = ").push_bind(u).push(")")
        .push(" OR (t.project_id IS NOT NULL AND EXISTS (SELECT 1 FROM projects p WHERE p.id = t.project_id AND (p.visibility = 'open' OR EXISTS (SELECT 1 FROM project_members pm WHERE pm.project_id = p.id AND pm.user_id = ").push_bind(u).push("))))")
        // Heads of the responsible person's department or of any department above it.
        .push(" OR t.responsible_id IN (WITH RECURSIVE tree AS (SELECT id FROM departments WHERE head_user_id = ").push_bind(u)
        .push(" UNION ALL SELECT d.id FROM departments d JOIN tree ON d.parent_id = tree.id) SELECT ud.user_id FROM user_departments ud JOIN tree ON tree.id = ud.department_id))");
}

/// Statuses a task moves through.
pub const STATUSES: &[&str] = &["new", "in_progress", "review", "done", "deferred"];

/// Works out the status a requested change actually produces, or refuses it.
/// Completing a task that needs review sends it to the creator instead of closing it.
pub fn resolve_status(t: &TaskCore, a: &TaskAccess, actor: Uuid, target: &str) -> AppResult<String> {
    if !STATUSES.contains(&target) {
        return Err(AppError::bad(format!("unknown status: {target}")));
    }
    if !a.participate {
        return Err(AppError::Forbidden);
    }
    let current = t.status.as_str();
    if target == current {
        return Ok(target.into());
    }
    let reviewer = a.edit; // creator, project owner, supervisor, admin
    match (current, target) {
        // Accepting or sending back a task under review is the reviewer's call.
        ("review", "done") | ("review", "in_progress") | ("review", "new") if !reviewer => {
            Err(AppError::bad("only the task's creator can accept or return it"))
        }
        (_, "done") | (_, "review") => {
            if t.requires_review && actor != t.created_by && !(reviewer && current == "review") {
                Ok("review".into())
            } else {
                Ok("done".into())
            }
        }
        _ => Ok(target.into()),
    }
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct ProjectAccess {
    pub view: bool,
    /// Rename, archive, change visibility, manage members.
    pub manage: bool,
    /// Create tasks in the project.
    pub contribute: bool,
    pub role: Option<&'static str>,
}

pub async fn project_access(state: &AppState, auth: &AuthUser, project: Uuid) -> AppResult<ProjectAccess> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT p.visibility, (SELECT role FROM project_members WHERE project_id = p.id AND user_id = $2)
         FROM projects p WHERE p.id = $1",
    )
    .bind(project)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;
    let Some((visibility, role)) = row else {
        return Err(AppError::NotFound("project"));
    };
    let all = auth.can(perm::TASKS_MANAGE_ALL);
    let role: Option<&'static str> = match role.as_deref() {
        Some("owner") => Some("owner"),
        Some("member") => Some("member"),
        Some("viewer") => Some("viewer"),
        _ => None,
    };
    let access = ProjectAccess {
        view: all || role.is_some() || visibility == "open",
        manage: all || role == Some("owner"),
        contribute: all || matches!(role, Some("owner") | Some("member")),
        role,
    };
    if !access.view {
        return Err(AppError::NotFound("project"));
    }
    Ok(access)
}

/// Appends `AND <project p is visible to user>`.
/// Whether the user may read a file because it is attached to a task they can see.
pub async fn can_view_task_file(state: &AppState, auth: &AuthUser, file: Uuid) -> AppResult<bool> {
    let tasks: Vec<Uuid> = sqlx::query_scalar("SELECT task_id FROM task_files WHERE file_id = $1")
        .bind(file)
        .fetch_all(&state.db)
        .await?;
    for t in tasks {
        if load_task(state, auth, t).await.is_ok() {
            return Ok(true);
        }
    }
    Ok(false)
}
