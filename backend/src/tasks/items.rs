//! Tasks: list, create, read, update, delete, status changes and board moves.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::collections::HashMap;
use uuid::Uuid;

use super::{
    access::{load_task, project_access, push_visible, resolve_status, TaskAccess, STATUSES},
    broadcast_changed, notify_task, participants, record_history, user_name,
};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::{clean, double_option, required, Page, PageParams},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tasks", get(list).post(create))
        .route("/tasks/counters", get(counters))
        .route("/tasks/{id}", get(detail).patch(update).delete(remove))
        .route("/tasks/{id}/status", post(change_status))
        .route("/tasks/{id}/crm", axum::routing::put(set_crm))
        .route("/tasks/{id}/move", post(move_card))
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow, Clone)]
pub struct TaskSummary {
    pub id: Uuid,
    pub number: i64,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub deadline: Option<DateTime<Utc>>,
    pub is_overdue: bool,
    pub project_id: Option<Uuid>,
    pub project_name: Option<String>,
    pub project_color: Option<String>,
    pub parent_id: Option<Uuid>,
    pub responsible_id: Uuid,
    pub responsible_name: String,
    pub responsible_avatar_file_id: Option<Uuid>,
    pub created_by: Uuid,
    pub creator_name: String,
    pub creator_avatar_file_id: Option<Uuid>,
    pub requires_review: bool,
    pub position: f64,
    pub start_date: Option<DateTime<Utc>>,
    pub stage_id: Option<Uuid>,
    pub template_id: Option<Uuid>,
    /// Unfinished tasks this one waits for.
    pub blocked_by_count: i64,
    pub checklist_total: i64,
    pub checklist_done: i64,
    pub comment_count: i64,
    pub subtask_count: i64,
    pub file_count: i64,
    pub time_spent_secs: i64,
    pub completed_at: Option<DateTime<Utc>>,
    pub last_activity_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    /// Scrum: estimate, epic, sprint (none: in the backlog) and order in the backlog.
    pub story_points: Option<f64>,
    pub epic_id: Option<Uuid>,
    pub epic_name: Option<String>,
    pub epic_color: Option<String>,
    pub sprint_id: Option<Uuid>,
    pub backlog_sort: f64,
    /// The creator's verdict once finished; a result comment; deadline rules.
    pub rating: Option<String>,
    pub result_comment_id: Option<Uuid>,
    pub requires_result: bool,
    pub skip_nonworking: bool,
    /// Comments by others since the viewer last opened the task (lists only).
    #[sqlx(default)]
    pub new_comments: i64,
}

pub const SUMMARY_SELECT: &str = "SELECT t.id, t.number, t.title, t.status, t.priority, t.deadline,
    (t.deadline IS NOT NULL AND t.deadline < now() AND t.status IN ('new','in_progress','review')) AS is_overdue,
    t.project_id, p.name AS project_name, p.color AS project_color, t.parent_id,
    t.responsible_id, r.first_name || ' ' || r.last_name AS responsible_name, r.avatar_file_id AS responsible_avatar_file_id,
    t.created_by, c.first_name || ' ' || c.last_name AS creator_name, c.avatar_file_id AS creator_avatar_file_id,
    t.requires_review, t.position, t.start_date, t.stage_id, t.template_id,
    (SELECT COUNT(*) FROM task_dependencies d JOIN tasks bt ON bt.id = d.depends_on_id
      WHERE d.task_id = t.id AND bt.status <> 'done') AS blocked_by_count,
    (SELECT COUNT(*) FROM task_checklist_items ci WHERE ci.task_id = t.id) AS checklist_total,
    (SELECT COUNT(*) FROM task_checklist_items ci WHERE ci.task_id = t.id AND ci.is_done) AS checklist_done,
    (SELECT COUNT(*) FROM task_comments cm WHERE cm.task_id = t.id) AS comment_count,
    (SELECT COUNT(*) FROM tasks s WHERE s.parent_id = t.id) AS subtask_count,
    (SELECT COUNT(*) FROM task_files tf WHERE tf.task_id = t.id) AS file_count,
    (SELECT COALESCE(SUM(te.seconds), 0)::bigint FROM task_time_entries te WHERE te.task_id = t.id AND te.stopped_at IS NOT NULL) AS time_spent_secs,
    t.completed_at, t.last_activity_at, t.created_at,
    t.story_points, t.epic_id, ep.name AS epic_name, ep.color AS epic_color, t.sprint_id, t.backlog_sort,
    t.rating, t.result_comment_id, t.requires_result, t.skip_nonworking
    FROM tasks t
    JOIN users r ON r.id = t.responsible_id
    JOIN users c ON c.id = t.created_by
    LEFT JOIN projects p ON p.id = t.project_id
    LEFT JOIN scrum_epics ep ON ep.id = t.epic_id
    WHERE TRUE";

#[derive(Serialize, FromRow)]
pub struct TaskPerson {
    user_id: Uuid,
    full_name: String,
    avatar_file_id: Option<Uuid>,
    position: Option<String>,
    role: String,
}

#[derive(Serialize, FromRow)]
pub struct ChecklistItem {
    id: Uuid,
    title: String,
    is_done: bool,
    sort: f64,
    done_by: Option<Uuid>,
    done_at: Option<DateTime<Utc>>,
    assignee_id: Option<Uuid>,
    assignee_name: Option<String>,
}

#[derive(Serialize, FromRow)]
pub struct Attachment {
    id: Uuid,
    original_name: String,
    mime_type: String,
    size_bytes: i64,
    added_by: Option<Uuid>,
    added_at: DateTime<Utc>,
}

#[derive(Serialize, FromRow)]
pub struct TaskRef {
    id: Uuid,
    number: i64,
    title: String,
    status: String,
}

#[derive(Serialize)]
pub struct TaskDetail {
    #[serde(flatten)]
    pub summary: TaskSummary,
    pub description: Option<String>,
    pub time_estimate_secs: Option<i32>,
    people: Vec<TaskPerson>,
    checklist: Vec<ChecklistItem>,
    files: Vec<Attachment>,
    subtasks: Vec<TaskSummary>,
    parent: Option<TaskRef>,
    access: TaskAccess,
    /// When the caller's running timer on this task started, if any.
    my_timer_started_at: Option<DateTime<Utc>>,
    blocked_by: Vec<super::deps::DepRef>,
    blocking: Vec<super::deps::DepRef>,
    template: Option<TemplateRef>,
    /// CRM records the task is about (those the viewer may open).
    crm: Vec<crate::crm::collab::LinkedRecord>,
    /// Custom fields that apply to it, with their values.
    fields: Vec<Value>,
    /// The comment marked as the task's result.
    result: Option<Value>,
}

#[derive(Serialize)]
pub struct TemplateRef {
    id: Uuid,
    title: String,
    schedule: Option<String>,
    /// Only the template's owner can open it.
    mine: bool,
}

impl TaskDetail {
    pub fn people_with_role(&self, role: &str) -> Vec<Uuid> {
        self.people.iter().filter(|p| p.role == role).map(|p| p.user_id).collect()
    }
    pub fn checklist_titles(&self) -> Vec<String> {
        self.checklist.iter().map(|c| c.title.clone()).collect()
    }
    pub fn subtask_titles(&self) -> Vec<String> {
        self.subtasks.iter().map(|t| t.title.clone()).collect()
    }
}

pub async fn load_summary(state: &AppState, id: Uuid) -> AppResult<TaskSummary> {
    let mut qb = QueryBuilder::new(SUMMARY_SELECT);
    qb.push(" AND t.id = ").push_bind(id);
    Ok(qb.build_query_as().fetch_one(&state.db).await?)
}

pub async fn load_detail(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<TaskDetail> {
    let (core, access) = load_task(state, auth, id).await?;
    let summary = load_summary(state, id).await?;
    let (description, time_estimate_secs): (Option<String>, Option<i32>) =
        sqlx::query!(r#"SELECT description, time_estimate_secs FROM tasks WHERE id = $1"#, id).map(|r| (r.description, r.time_estimate_secs))
            .fetch_one(&state.db)
            .await?;
    let people: Vec<TaskPerson> = sqlx::query_as!(TaskPerson, r#"SELECT u.id AS "user_id!", u.first_name || ' ' || u.last_name AS "full_name!", u.avatar_file_id AS "avatar_file_id?", u.position AS "position?", x.role AS "role!"
         FROM (SELECT created_by AS user_id, 'creator' AS role, 0 AS o FROM tasks WHERE id = $1
               UNION ALL SELECT responsible_id, 'responsible', 1 FROM tasks WHERE id = $1
               UNION ALL SELECT user_id, role, CASE role WHEN 'assistant' THEN 2 ELSE 3 END FROM task_members WHERE task_id = $1) x
         JOIN users u ON u.id = x.user_id
         ORDER BY x.o, u.last_name"#, id)
    .fetch_all(&state.db)
    .await?;
    let checklist: Vec<ChecklistItem> = sqlx::query_as!(ChecklistItem, r#"SELECT ci.id, ci.title, ci.is_done, ci.sort, ci.done_by AS "done_by?", ci.done_at AS "done_at?", ci.assignee_id AS "assignee_id?", trim(u.first_name || ' ' || u.last_name) AS "assignee_name?"
         FROM task_checklist_items ci LEFT JOIN users u ON u.id = ci.assignee_id WHERE ci.task_id = $1 ORDER BY ci.sort, ci.created_at"#, id)
    .fetch_all(&state.db)
    .await?;
    let files: Vec<Attachment> = sqlx::query_as!(Attachment, r#"SELECT f.id, f.original_name, f.mime_type, f.size_bytes, tf.added_by AS "added_by?", tf.added_at
         FROM task_files tf JOIN files f ON f.id = tf.file_id WHERE tf.task_id = $1 ORDER BY tf.added_at"#, id)
    .fetch_all(&state.db)
    .await?;
    let mut qb = QueryBuilder::new(SUMMARY_SELECT);
    qb.push(" AND t.parent_id = ").push_bind(id);
    push_visible(&mut qb, auth);
    qb.push(" ORDER BY t.created_at");
    let subtasks: Vec<TaskSummary> = qb.build_query_as().fetch_all(&state.db).await?;
    let parent: Option<TaskRef> = match core.parent_id {
        Some(p) if load_task(state, auth, p).await.is_ok() => {
            sqlx::query_as("SELECT id, number, title, status FROM tasks WHERE id = $1")
                .bind(p)
                .fetch_optional(&state.db)
                .await?
        }
        _ => None,
    };
    let my_timer_started_at: Option<DateTime<Utc>> = sqlx::query_scalar!("SELECT started_at FROM task_time_entries WHERE task_id = $1 AND user_id = $2 AND stopped_at IS NULL", id, auth.id)
    .fetch_optional(&state.db)
    .await?;
    let (blocked_by, blocking) = super::deps::links(state, auth, id).await?;
    let template = match summary.template_id {
        Some(tid) => sqlx::query!(r#"SELECT title, recurrence, created_by FROM task_templates WHERE id = $1"#, tid).map(|r| (r.title, r.recurrence, r.created_by))
        .fetch_optional(&state.db)
        .await?
        .map(|(title, rec, owner)| TemplateRef {
            id: tid,
            title,
            schedule: rec
                .and_then(|r| serde_json::from_value::<super::recurrence::Recurrence>(r).ok())
                .map(|r| r.describe()),
            mine: owner == auth.id,
        }),
        None => None,
    };
    Ok(TaskDetail {
        blocked_by,
        blocking,
        template,
        summary,
        description,
        time_estimate_secs,
        people,
        checklist,
        files,
        subtasks,
        parent,
        access,
        my_timer_started_at,
        crm: crate::crm::collab::task_records(state, auth, id).await?,
        fields: super::depth::field_values(state, id, core.project_id).await?,
        result: super::depth::result_of(state, id).await?,
    })
}

#[derive(Deserialize)]
struct CrmLinks {
    item_ids: Vec<Uuid>,
}

/// Sets the CRM records a task is about. Links to records the caller cannot see stay.
async fn set_crm(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<CrmLinks>) -> AppResult<Json<TaskDetail>> {
    let (_, access) = load_task(&state, &auth, id).await?;
    if !access.edit {
        return Err(AppError::Forbidden);
    }
    if b.item_ids.len() > 20 {
        return Err(AppError::bad("link at most 20 records"));
    }
    let seen: Vec<Uuid> = crate::crm::collab::task_records(&state, &auth, id).await?.iter().map(|r| r.id()).collect();
    let all: Vec<Uuid> = sqlx::query_scalar!("SELECT item_id FROM task_crm_links WHERE task_id = $1", id).fetch_all(&state.db).await?;
    let drop: Vec<Uuid> = seen.iter().copied().filter(|x| !b.item_ids.contains(x)).collect();
    let add: Vec<Uuid> = b.item_ids.iter().copied().filter(|x| !all.contains(x)).collect();
    let mut tx = state.db.begin().await?;
    sqlx::query!("DELETE FROM task_crm_links WHERE task_id = $1 AND item_id = ANY($2)", id, &drop).execute(&mut *tx).await?;
    crate::crm::collab::link_task(&mut tx, &state, &auth, id, &add).await?;
    tx.commit().await?;
    if !add.is_empty() {
        crate::crm::collab::task_event(&state, id, "linked", auth.id).await;
    }
    Ok(Json(load_detail(&state, &auth, id).await?))
}

// ---------------------------------------------------------------------------
// List & counters
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ListQuery {
    /// With counts by status (and overdue) for the whole filtered list.
    summary: Option<String>,
    /// my (responsible or assistant) | created | assisting | watching | all (everything visible)
    view: Option<String>,
    /// Comma-separated statuses; default: everything.
    status: Option<String>,
    project_id: Option<Uuid>,
    responsible_id: Option<Uuid>,
    parent_id: Option<Uuid>,
    q: Option<String>,
    overdue: Option<String>,
    /// Scrum: tasks in this sprint; "true" for the backlog (no sprint); tasks of this epic.
    sprint_id: Option<Uuid>,
    backlog: Option<String>,
    epic_id: Option<Uuid>,
    /// "true": tasks with comments by others I haven't seen.
    unread: Option<String>,
    /// Deadline window (calendar view).
    deadline_from: Option<DateTime<Utc>>,
    deadline_to: Option<DateTime<Utc>>,
    /// deadline | created | activity | priority | position | backlog
    sort: Option<String>,
    #[serde(flatten)]
    page: PageParams,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let statuses: Vec<String> = q
        .status
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();
    if let Some(bad) = statuses.iter().find(|s| !STATUSES.contains(&s.as_str())) {
        return Err(AppError::bad(format!("unknown status: {bad}")));
    }
    if let Some(p) = q.project_id {
        project_access(&state, &auth, p).await?;
    }
    let u = auth.id;

    let build = |select: &str| {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(select);
        push_visible(&mut qb, &auth);
        match q.view.as_deref().unwrap_or("all") {
            "my" => {
                qb.push(" AND (t.responsible_id = ").push_bind(u)
                    .push(" OR EXISTS (SELECT 1 FROM task_members m WHERE m.task_id = t.id AND m.role = 'assistant' AND m.user_id = ").push_bind(u).push("))");
            }
            "created" => {
                qb.push(" AND t.created_by = ").push_bind(u);
            }
            "assisting" => {
                qb.push(" AND EXISTS (SELECT 1 FROM task_members m WHERE m.task_id = t.id AND m.role = 'assistant' AND m.user_id = ").push_bind(u).push(")");
            }
            "watching" => {
                qb.push(" AND EXISTS (SELECT 1 FROM task_members m WHERE m.task_id = t.id AND m.role = 'watcher' AND m.user_id = ").push_bind(u).push(")");
            }
            // The tasks of the people in departments I head (and their sub-departments).
            "supervising" => {
                qb.push(" AND t.responsible_id <> ").push_bind(u)
                    .push(" AND t.responsible_id IN (WITH RECURSIVE tree AS (SELECT id FROM departments WHERE head_user_id = ").push_bind(u)
                    .push(" UNION ALL SELECT d.id FROM departments d JOIN tree ON d.parent_id = tree.id) SELECT ud.user_id FROM user_departments ud JOIN tree ON tree.id = ud.department_id)");
            }
            _ => {}
        }
        if !statuses.is_empty() {
            qb.push(" AND t.status = ANY(").push_bind(statuses.clone()).push(")");
        }
        if let Some(p) = q.project_id {
            qb.push(" AND t.project_id = ").push_bind(p);
        }
        if let Some(r) = q.responsible_id {
            qb.push(" AND t.responsible_id = ").push_bind(r);
        }
        if let Some(p) = q.parent_id {
            qb.push(" AND t.parent_id = ").push_bind(p);
        }
        if q.overdue.as_deref() == Some("true") {
            qb.push(" AND t.deadline < now() AND t.status IN ('new','in_progress','review')");
        }
        if let Some(s) = q.sprint_id {
            qb.push(" AND t.sprint_id = ").push_bind(s);
        }
        if q.backlog.as_deref() == Some("true") {
            qb.push(" AND t.sprint_id IS NULL");
        }
        if let Some(e) = q.epic_id {
            qb.push(" AND t.epic_id = ").push_bind(e);
        }
        if q.unread.as_deref() == Some("true") {
            qb.push(" AND EXISTS (SELECT 1 FROM task_comments cm WHERE cm.task_id = t.id AND cm.author_id IS DISTINCT FROM ").push_bind(u)
                .push(" AND cm.created_at > COALESCE((SELECT v.viewed_at FROM task_views v WHERE v.task_id = t.id AND v.user_id = ").push_bind(u).push("), '-infinity'))");
        }
        if let Some(f) = q.deadline_from {
            qb.push(" AND t.deadline >= ").push_bind(f);
        }
        if let Some(to) = q.deadline_to {
            qb.push(" AND t.deadline < ").push_bind(to);
        }
        if let Some(term) = clean(q.q.clone()) {
            let digits = term.trim_start_matches('#');
            let like = format!("%{}%", term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            qb.push(" AND (t.title ILIKE ").push_bind(like.clone()).push(" OR t.description ILIKE ").push_bind(like);
            if let Ok(n) = digits.parse::<i64>() {
                qb.push(" OR t.number = ").push_bind(n);
            }
            qb.push(")");
        }
        qb
    };

    let mut count_qb = build("SELECT COUNT(*) FROM tasks t WHERE TRUE");
    let total: i64 = count_qb.build_query_scalar().fetch_one(&state.db).await?;
    let mut qb = build(SUMMARY_SELECT);
    qb.push(match q.sort.as_deref().unwrap_or("deadline") {
        "created" => " ORDER BY t.created_at DESC",
        "activity" => " ORDER BY t.last_activity_at DESC",
        "priority" => " ORDER BY CASE t.priority WHEN 'high' THEN 0 WHEN 'normal' THEN 1 ELSE 2 END, t.deadline ASC NULLS LAST",
        "position" => " ORDER BY t.position, t.created_at",
        "backlog" => " ORDER BY t.backlog_sort, t.created_at",
        _ => " ORDER BY t.deadline ASC NULLS LAST, t.created_at DESC",
    });
    qb.push(", t.id LIMIT ").push_bind(q.page.limit()).push(" OFFSET ").push_bind(q.page.offset());
    let mut items: Vec<TaskSummary> = qb.build_query_as().fetch_all(&state.db).await?;
    super::depth::count_new_comments(&state, auth.id, &mut items).await?;
    let mut v = serde_json::to_value(Page { items, total, limit: q.page.limit(), offset: q.page.offset() }).map_err(anyhow::Error::from)?;
    // How the listed tasks stand: how many in each status, and overdue.
    if q.summary.as_deref() == Some("true") {
        let mut sq = build("SELECT t.status, COUNT(*), COUNT(*) FILTER (WHERE t.deadline < now() AND t.status IN ('new','in_progress','review')) FROM tasks t WHERE TRUE");
        sq.push(" GROUP BY t.status");
        let rows: Vec<(String, i64, i64)> = sq.build_query_as().fetch_all(&state.db).await?;
        let mut by = serde_json::Map::new();
        for st in STATUSES {
            by.insert((*st).into(), json!(rows.iter().find(|r| r.0 == *st).map(|r| r.1).unwrap_or(0)));
        }
        v["summary"] = json!({ "by_status": by, "overdue": rows.iter().map(|r| r.2).sum::<i64>() });
    }
    Ok(Json(v))
}

/// Badge numbers for the sidebar and dashboard.
async fn counters(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let row: (i64, i64, i64, i64, i64) = sqlx::query!(r#"WITH mine AS (
             SELECT t.* FROM tasks t WHERE t.status IN ('new','in_progress','review')
               AND (t.responsible_id = $1 OR EXISTS (SELECT 1 FROM task_members m WHERE m.task_id = t.id AND m.role = 'assistant' AND m.user_id = $1)))
         SELECT
           (SELECT COUNT(*) FROM mine) AS "c0!",
           (SELECT COUNT(*) FROM mine WHERE deadline < now()) AS "c1!",
           (SELECT COUNT(*) FROM mine WHERE deadline >= now() AND deadline < now() + interval '1 day') AS "c2!",
           (SELECT COUNT(*) FROM tasks WHERE created_by = $1 AND status = 'review') AS "c3!",
           (SELECT COUNT(*) FROM tasks WHERE created_by = $1 AND status IN ('new','in_progress','review')) AS "c4!""#, auth.id).map(|r| (r.c0, r.c1, r.c2, r.c3, r.c4))
    .fetch_one(&state.db)
    .await?;
    Ok(Json(json!({
        "my_open": row.0,
        "my_overdue": row.1,
        "my_due_soon": row.2,
        "awaiting_my_review": row.3,
        "created_open": row.4,
    })))
}

async fn detail(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<TaskDetail>> {
    // Opening the task reads its comments.
    let _ = sqlx::query!("INSERT INTO task_views (task_id, user_id) SELECT $1, $2 WHERE EXISTS (SELECT 1 FROM tasks WHERE id = $1) ON CONFLICT (task_id, user_id) DO UPDATE SET viewed_at = now()", id, auth.id)
        .execute(&state.db)
        .await;
    Ok(Json(load_detail(&state, &auth, id).await?))
}

// ---------------------------------------------------------------------------
// Create / update / delete
// ---------------------------------------------------------------------------

fn validate_priority(p: &str) -> AppResult<()> {
    if ["low", "normal", "high"].contains(&p) { Ok(()) } else { Err(AppError::bad("priority must be low, normal or high")) }
}

async fn ensure_active_users(state: &AppState, ids: &[Uuid]) -> AppResult<()> {
    let mut unique = ids.to_vec();
    unique.sort();
    unique.dedup();
    let n: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM users WHERE id = ANY($1) AND is_active"#, &unique)
        .fetch_one(&state.db)
        .await?;
    if n as usize != unique.len() {
        return Err(AppError::bad("one or more selected people do not exist or are deactivated"));
    }
    Ok(())
}

/// Tasks may only be put in projects where the caller can contribute.
async fn ensure_can_use_project(state: &AppState, auth: &AuthUser, project: Uuid) -> AppResult<()> {
    let access = project_access(state, auth, project).await?;
    if !access.contribute {
        return Err(AppError::bad("you can't add tasks to this project; ask an owner to make you a member"));
    }
    let archived: bool = sqlx::query_scalar!("SELECT is_archived FROM projects WHERE id = $1", project)
        .fetch_one(&state.db)
        .await?;
    if archived {
        return Err(AppError::bad("this project is archived"));
    }
    Ok(())
}

#[derive(Deserialize, Default)]
pub struct CreateTask {
    pub title: String,
    pub description: Option<String>,
    pub responsible_id: Option<Uuid>,
    #[serde(default)]
    pub assistant_ids: Vec<Uuid>,
    #[serde(default)]
    pub watcher_ids: Vec<Uuid>,
    pub start_date: Option<DateTime<Utc>>,
    pub deadline: Option<DateTime<Utc>>,
    pub priority: Option<String>,
    pub project_id: Option<Uuid>,
    pub parent_id: Option<Uuid>,
    #[serde(default)]
    pub requires_review: bool,
    pub time_estimate_secs: Option<i32>,
    #[serde(default)]
    pub checklist: Vec<String>,
    #[serde(default)]
    pub file_ids: Vec<Uuid>,
    /// CRM records the task is about.
    #[serde(default)]
    pub crm_item_ids: Vec<Uuid>,
    /// Scrum teams: estimate, epic and sprint (the backlog when absent).
    pub story_points: Option<f64>,
    pub epic_id: Option<Uuid>,
    pub sprint_id: Option<Uuid>,
    /// Move the deadline and start off weekends and company holidays.
    #[serde(default)]
    pub skip_nonworking: bool,
    /// The task can't be finished without a comment marked as its result.
    #[serde(default)]
    pub requires_result: bool,
    /// Custom field values (field id → value). When given, required fields are checked.
    pub fields: Option<HashMap<Uuid, Value>>,
    /// Set by the server when a template creates the task.
    #[serde(skip)]
    pub template_id: Option<Uuid>,
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(body): Json<CreateTask>) -> AppResult<Json<TaskDetail>> {
    let id = create_task(&state, &auth, body).await?;
    Ok(Json(load_detail(&state, &auth, id).await?))
}

fn check_dates(start: Option<DateTime<Utc>>, deadline: Option<DateTime<Utc>>) -> AppResult<()> {
    if let (Some(s), Some(d)) = (start, deadline)
        && s > d {
            return Err(AppError::bad("the start date is after the deadline"));
        }
    Ok(())
}

/// Creates a task on behalf of `auth`. Used by the API, templates and recurring schedules.
pub async fn create_task(state: &AppState, auth: &AuthUser, body: CreateTask) -> AppResult<Uuid> {
    let title = required("title", &body.title, 500)?;
    let mut body = body;
    if body.skip_nonworking {
        if let Some(d) = body.deadline {
            body.deadline = Some(super::depth::to_working_day(state, d).await?);
        }
        if let Some(d) = body.start_date {
            body.start_date = Some(super::depth::to_working_day(state, d).await?);
        }
    }
    check_dates(body.start_date, body.deadline)?;
    let responsible = body.responsible_id.unwrap_or(auth.id);
    let priority = body.priority.unwrap_or_else(|| "normal".into());
    validate_priority(&priority)?;
    let mut people = vec![responsible];
    people.extend(&body.assistant_ids);
    people.extend(&body.watcher_ids);
    ensure_active_users(state, &people).await?;

    // A subtask inherits its parent's project unless told otherwise.
    let mut project = body.project_id;
    if let Some(parent) = body.parent_id {
        let (pcore, _) = load_task(state, auth, parent).await?;
        if project.is_none() {
            project = pcore.project_id;
        }
    }
    if let Some(p) = project {
        ensure_can_use_project(state, auth, p).await?;
    }
    if body.time_estimate_secs.is_some_and(|s| s < 0) {
        return Err(AppError::bad("time estimate cannot be negative"));
    }
    super::scrum::check_placement(state, project, body.story_points, body.epic_id, body.sprint_id).await?;

    let mut tx = state.db.begin().await?;
    let number: i64 = sqlx::query_scalar!("UPDATE counters SET value = value + 1 WHERE name = 'task_number' RETURNING value")
        .fetch_one(&mut *tx)
        .await?;
    let stage = match project {
        Some(p) => super::stages::default_stage(&mut *tx, p, "new").await?,
        None => None,
    };
    let id: Uuid = sqlx::query_scalar!("INSERT INTO tasks (title, description, responsible_id, created_by, deadline, priority, project_id, parent_id,
                            requires_review, time_estimate_secs, number, start_date, stage_id, template_id, position)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                 COALESCE((SELECT MIN(position) FROM tasks WHERE status = 'new' AND project_id IS NOT DISTINCT FROM $7), 1000) - 1)
         RETURNING id", &title, clean(body.description), responsible, auth.id, body.deadline, &priority, project, body.parent_id, body.requires_review, body.time_estimate_secs, number, body.start_date, stage, body.template_id)
    .fetch_one(&mut *tx)
    .await?;

    set_members(&mut tx, id, responsible, auth.id, &body.assistant_ids, &body.watcher_ids).await?;
    if body.skip_nonworking || body.requires_result {
        sqlx::query!("UPDATE tasks SET skip_nonworking = $2, requires_result = $3 WHERE id = $1", id, body.skip_nonworking, body.requires_result).execute(&mut *tx).await?;
    }
    if body.story_points.is_some() || body.epic_id.is_some() || body.sprint_id.is_some() {
        sqlx::query!("UPDATE tasks SET story_points = $2, epic_id = $3, sprint_id = $4,
             backlog_sort = COALESCE((SELECT MAX(backlog_sort) FROM tasks WHERE project_id = $5 AND sprint_id IS NOT DISTINCT FROM $4), 0) + 1
             WHERE id = $1", id, body.story_points, body.epic_id, body.sprint_id, project)
        .execute(&mut *tx)
        .await?;
        // Added straight into a running sprint: a scope change for its burndown.
        sqlx::query(
            "INSERT INTO scrum_scope_changes (sprint_id, task_id, points)
             SELECT id, $2, $3 FROM scrum_sprints WHERE id = $1 AND status = 'active' AND $3 > 0",
        )
        .bind(body.sprint_id)
        .bind(id)
        .bind(body.story_points.unwrap_or(0.0))
        .execute(&mut *tx)
        .await?;
    } else if project.is_some() {
        // New tasks join the end of the backlog.
        sqlx::query!("UPDATE tasks SET backlog_sort = COALESCE((SELECT MAX(backlog_sort) FROM tasks WHERE project_id = $2 AND sprint_id IS NULL AND id <> $1), 0) + 1 WHERE id = $1", id, project)
            .execute(&mut *tx)
            .await?;
    }
    for (i, item) in body.checklist.iter().filter_map(|c| clean(Some(c.clone()))).enumerate() {
        sqlx::query!("INSERT INTO task_checklist_items (task_id, title, sort) VALUES ($1, $2, $3)", id, item.chars().take(1000).collect::<String>(), i as f64)
            .execute(&mut *tx)
            .await?;
    }
    for f in &body.file_ids {
        attach_owned_file(&mut tx, id, *f, auth.id).await?;
    }
    crate::crm::collab::link_task(&mut tx, state, auth, id, &body.crm_item_ids).await?;
    record_history(&mut *tx, id, auth.id, "created", None, Some(title.clone())).await?;
    tx.commit().await?;
    if let Some(f) = &body.fields {
        if let Err(e) = super::depth::set_fields(state, id, project, f, true).await {
            // Nothing half-made: the task goes again.
            let _ = sqlx::query!("DELETE FROM tasks WHERE id = $1", id).execute(&state.db).await;
            return Err(e);
        }
    }
    if !body.crm_item_ids.is_empty() {
        crate::crm::collab::task_event(state, id, "created", auth.id).await;
    }

    notify_task(state, &[responsible], "task.assigned", format!("New task for you: {title}"), None, id, auth.id).await;
    notify_task(state, &body.assistant_ids, "task.assistant", format!("You were added as an assistant: {title}"), None, id, auth.id).await;
    notify_task(state, &body.watcher_ids, "task.watcher", format!("You are watching: {title}"), None, id, auth.id).await;
    broadcast_changed(state, id, project, false).await;
    emit_task(state, "task.created", id).await;
    if let Some(s) = stage {
        super::automation::stage_entered(state, id, s, auth.id).await;
    }
    Ok(id)
}

/// What webhooks get about a task.
async fn emit_task(state: &AppState, event: &str, id: Uuid) {
    if let Ok(t) = load_summary(state, id).await {
        let mut data = serde_json::to_value(&t).unwrap_or_default();
        data["url"] = json!(format!("{}/tasks/{id}", state.config.public_url));
        crate::api::webhooks::emit(state, event, data).await;
    }
}

/// Replaces assistants and watchers. The creator and responsible person are never listed
/// as members (they already take part), and nobody holds two roles.
async fn set_members(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    task: Uuid,
    responsible: Uuid,
    creator: Uuid,
    assistants: &[Uuid],
    watchers: &[Uuid],
) -> AppResult<()> {
    sqlx::query!("DELETE FROM task_members WHERE task_id = $1", task).execute(&mut **tx).await?;
    let assistants: Vec<Uuid> = assistants.iter().copied().filter(|u| *u != responsible).collect();
    let watchers: Vec<Uuid> = watchers
        .iter()
        .copied()
        .filter(|u| *u != responsible && *u != creator && !assistants.contains(u))
        .collect();
    sqlx::query!("INSERT INTO task_members (task_id, user_id, role) SELECT $1, UNNEST($2::uuid[]), 'assistant' ON CONFLICT DO NOTHING", task, &assistants)
        .execute(&mut **tx)
        .await?;
    sqlx::query!("INSERT INTO task_members (task_id, user_id, role) SELECT $1, UNNEST($2::uuid[]), 'watcher' ON CONFLICT DO NOTHING", task, &watchers)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Attaches a file the caller uploaded.
pub async fn attach_owned_file(tx: &mut sqlx::Transaction<'_, Postgres>, task: Uuid, file: Uuid, user: Uuid) -> AppResult<()> {
    let owner: Option<Option<Uuid>> = sqlx::query_scalar!(r#"SELECT owner_id AS "owner_id?" FROM files WHERE id = $1"#, file)
        .fetch_optional(&mut **tx)
        .await?;
    if owner != Some(Some(user)) {
        return Err(AppError::bad("you can only attach files you uploaded"));
    }
    sqlx::query!("INSERT INTO task_files (task_id, file_id, added_by) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING", task, file, user)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[derive(Deserialize)]
struct UpdateTask {
    title: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    description: Option<Option<String>>,
    responsible_id: Option<Uuid>,
    assistant_ids: Option<Vec<Uuid>>,
    watcher_ids: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "double_option")]
    deadline: Option<Option<DateTime<Utc>>>,
    #[serde(default, deserialize_with = "double_option")]
    start_date: Option<Option<DateTime<Utc>>>,
    priority: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    project_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "double_option")]
    parent_id: Option<Option<Uuid>>,
    requires_review: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    time_estimate_secs: Option<Option<i32>>,
    skip_nonworking: Option<bool>,
    requires_result: Option<bool>,
    fields: Option<HashMap<Uuid, Value>>,
}

fn fmt_time(t: Option<DateTime<Utc>>) -> Option<String> {
    t.map(|d| d.to_rfc3339())
}

async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateTask>,
) -> AppResult<Json<TaskDetail>> {
    let (core, access) = load_task(&state, &auth, id).await?;
    if !access.edit {
        return Err(AppError::Forbidden);
    }
    let before = load_detail(&state, &auth, id).await?;
    let mut body = body;
    // Deadlines move off weekends and holidays when the task asks for it.
    if body.skip_nonworking.unwrap_or(before.summary.skip_nonworking) {
        if let Some(Some(d)) = body.deadline {
            body.deadline = Some(Some(super::depth::to_working_day(&state, d).await?));
        } else if body.skip_nonworking == Some(true) && !before.summary.skip_nonworking {
            if let Some(d) = core.deadline {
                body.deadline = Some(Some(super::depth::to_working_day(&state, d).await?));
            }
        }
        if let Some(Some(d)) = body.start_date {
            body.start_date = Some(Some(super::depth::to_working_day(&state, d).await?));
        }
    }
    let mut changes: Vec<(&str, Option<String>, Option<String>)> = Vec::new();
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("UPDATE tasks SET updated_at = now(), last_activity_at = now()");

    if let Some(v) = &body.title {
        let v = required("title", v, 500)?;
        if v != core.title {
            changes.push(("title", Some(core.title.clone()), Some(v.clone())));
            qb.push(", title = ").push_bind(v);
        }
    }
    if let Some(v) = body.description {
        let v = clean(v);
        if v != before.description {
            changes.push(("description", None, None));
            qb.push(", description = ").push_bind(v);
        }
    }
    if let Some(r) = body.responsible_id.filter(|r| *r != core.responsible_id) {
        ensure_active_users(&state, &[r]).await?;
        changes.push(("responsible", Some(user_name(&state, core.responsible_id).await), Some(user_name(&state, r).await)));
        qb.push(", responsible_id = ").push_bind(r);
        // A waiting flow task is handed out by giving it to someone.
        qb.push(", flow_waiting = FALSE");
    }
    let new_start = body.start_date.unwrap_or(before.summary.start_date);
    check_dates(new_start, body.deadline.unwrap_or(core.deadline))?;
    if let Some(s) = body.start_date.filter(|s| *s != before.summary.start_date) {
        changes.push(("start_date", fmt_time(before.summary.start_date), fmt_time(s)));
        qb.push(", start_date = ").push_bind(s);
    }
    if let Some(d) = body.deadline.filter(|d| *d != core.deadline) {
        changes.push(("deadline", fmt_time(core.deadline), fmt_time(d)));
        // A new deadline earns new reminders.
        qb.push(", deadline = ").push_bind(d).push(", deadline_reminded_at = NULL, overdue_notified_at = NULL");
    }
    if let Some(p) = &body.priority {
        validate_priority(p)?;
        if *p != before.summary.priority {
            changes.push(("priority", Some(before.summary.priority.clone()), Some(p.clone())));
            qb.push(", priority = ").push_bind(p.clone());
        }
    }
    if let Some(p) = body.project_id.filter(|p| *p != core.project_id) {
        if let Some(p) = p {
            ensure_can_use_project(&state, &auth, p).await?;
        }
        changes.push(("project", before.summary.project_name.clone(), match p {
            Some(p) => sqlx::query_scalar!("SELECT name FROM projects WHERE id = $1", p).fetch_optional(&state.db).await?,
            None => None,
        }));
        qb.push(", project_id = ").push_bind(p);
        // The task joins the new project's board in its default column.
        let stage = match p {
            Some(p) => super::stages::default_stage(&state.db, p, &core.status).await?,
            None => None,
        };
        qb.push(", stage_id = ").push_bind(stage);
        // Sprints and epics belong to the old team.
        qb.push(", sprint_id = NULL, epic_id = NULL");
    }
    if let Some(p) = body.parent_id.filter(|p| *p != core.parent_id) {
        if let Some(parent) = p {
            if parent == id {
                return Err(AppError::bad("a task cannot be its own parent"));
            }
            load_task(&state, &auth, parent).await?;
            // Walk up from the new parent to make sure we don't create a loop.
            let mut cur = Some(parent);
            while let Some(c) = cur {
                if c == id {
                    return Err(AppError::bad("a task cannot be placed under its own subtask"));
                }
                cur = sqlx::query_scalar!("SELECT parent_id FROM tasks WHERE id = $1", c).fetch_one(&state.db).await?;
            }
        }
        changes.push(("parent", None, None));
        qb.push(", parent_id = ").push_bind(p);
    }
    if let Some(v) = body.requires_review.filter(|v| *v != core.requires_review) {
        changes.push(("requires_review", Some(core.requires_review.to_string()), Some(v.to_string())));
        qb.push(", requires_review = ").push_bind(v);
    }
    if let Some(v) = body.time_estimate_secs {
        if v.is_some_and(|s| s < 0) {
            return Err(AppError::bad("time estimate cannot be negative"));
        }
        if v != before.time_estimate_secs {
            changes.push(("time_estimate", before.time_estimate_secs.map(|s| s.to_string()), v.map(|s| s.to_string())));
            qb.push(", time_estimate_secs = ").push_bind(v);
        }
    }
    if let Some(v) = body.skip_nonworking.filter(|v| *v != before.summary.skip_nonworking) {
        changes.push(("skip_nonworking", None, Some(v.to_string())));
        qb.push(", skip_nonworking = ").push_bind(v);
    }
    if let Some(v) = body.requires_result.filter(|v| *v != before.summary.requires_result) {
        changes.push(("requires_result", None, Some(v.to_string())));
        qb.push(", requires_result = ").push_bind(v);
    }
    if let Some(f) = &body.fields {
        let project = body.project_id.unwrap_or(core.project_id);
        super::depth::set_fields(&state, id, project, f, true).await?;
        changes.push(("fields", None, None));
    }
    qb.push(" WHERE id = ").push_bind(id);

    let old_members: HashMap<Uuid, String> = before
        .people
        .iter()
        .filter(|p| p.role == "assistant" || p.role == "watcher")
        .map(|p| (p.user_id, p.role.clone()))
        .collect();
    let new_responsible = body.responsible_id.unwrap_or(core.responsible_id);
    let assistants = body.assistant_ids.clone().unwrap_or_else(|| old_members.iter().filter(|(_, r)| *r == "assistant").map(|(u, _)| *u).collect());
    let watchers = body.watcher_ids.clone().unwrap_or_else(|| old_members.iter().filter(|(_, r)| *r == "watcher").map(|(u, _)| *u).collect());
    let members_changed = body.assistant_ids.is_some() || body.watcher_ids.is_some() || body.responsible_id.is_some();
    if members_changed {
        let mut all: Vec<Uuid> = assistants.clone();
        all.extend(&watchers);
        ensure_active_users(&state, &all).await?;
    }

    let mut tx = state.db.begin().await?;
    qb.build().execute(&mut *tx).await?;
    if members_changed {
        set_members(&mut tx, id, new_responsible, core.created_by, &assistants, &watchers).await?;
    }
    for (field, old, new) in &changes {
        record_history(&mut *tx, id, auth.id, field, old.clone(), new.clone()).await?;
    }
    tx.commit().await?;

    if changes.iter().any(|c| c.0 == "deadline" || c.0 == "start_date") {
        super::deps::reschedule_dependents(&state, &auth, id).await?;
    }
    let after = load_detail(&state, &auth, id).await?;
    let title = after.summary.title.clone();
    if new_responsible != core.responsible_id {
        notify_task(&state, &[new_responsible], "task.assigned", format!("{} assigned you a task: {title}", user_name(&state, auth.id).await), None, id, auth.id).await;
    }
    let added = |role: &str| -> Vec<Uuid> {
        after.people.iter().filter(|p| p.role == role && old_members.get(&p.user_id).map(String::as_str) != Some(role)).map(|p| p.user_id).collect()
    };
    notify_task(&state, &added("assistant"), "task.assistant", format!("You were added as an assistant: {title}"), None, id, auth.id).await;
    notify_task(&state, &added("watcher"), "task.watcher", format!("You are watching: {title}"), None, id, auth.id).await;
    if let Some((_, _, new)) = changes.iter().find(|c| c.0 == "deadline") {
        let body = match new {
            Some(d) => Some(format!("New deadline: {d}")),
            None => Some("The deadline was removed.".into()),
        };
        let mut people = vec![new_responsible];
        people.extend(&assistants);
        notify_task(&state, &people, "task.deadline_changed", format!("Deadline changed: {title}"), body, id, auth.id).await;
    }
    broadcast_changed(&state, id, after.summary.project_id, false).await;
    emit_task(&state, "task.updated", id).await;
    if core.project_id != after.summary.project_id {
        broadcast_changed(&state, id, core.project_id, false).await;
    }
    Ok(Json(after))
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (core, access) = load_task(&state, &auth, id).await?;
    if !access.delete {
        return Err(AppError::Forbidden);
    }
    let people = participants(&state, id).await;
    let gone = load_summary(&state, id).await.ok();
    let chat = crate::im::linked_chat_people(&state, crate::im::Link::Task(id)).await?;
    // Into the recycle bin, from where it can be put back for a while.
    super::depth::trash_task(&state, id, auth.id).await?;
    crate::im::linked_chat_gone(&state, chat).await;
    if let Some(t) = gone {
        crate::api::webhooks::emit(&state, "task.deleted", json!({ "id": t.id, "number": t.number, "title": t.title, "project_id": t.project_id })).await;
    }
    crate::realtime::publish(
        &state,
        Some(people),
        "task.changed",
        json!({ "id": id, "project_id": core.project_id, "deleted": true }),
    )
    .await;
    if core.project_id.is_some() {
        broadcast_changed(&state, id, core.project_id, true).await;
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Status & board
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct StatusChange {
    status: String,
}

/// Applies a status change, records it, and notifies the right people.
pub async fn apply_status(state: &AppState, auth: &AuthUser, id: Uuid, target: &str, position: Option<f64>) -> AppResult<TaskDetail> {
    let (core, access) = load_task(state, auth, id).await?;
    let new_status = resolve_status(&core, &access, auth.id, target)?;
    let changed = new_status != core.status;
    if changed && (new_status == "done" || new_status == "review") {
        let (needs, has): (bool, bool) = sqlx::query!(r#"SELECT requires_result AS "requires_result!", result_comment_id IS NOT NULL AS "c1!" FROM tasks WHERE id = $1"#, id).map(|r| (r.requires_result, r.c1)).fetch_one(&state.db).await?;
        if needs && !has {
            return Err(AppError::bad("this task needs a result: write a comment and mark it as the result first"));
        }
    }

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("UPDATE tasks SET updated_at = now(), last_activity_at = now()");
    if changed {
        qb.push(", status = ").push_bind(new_status.clone());
        qb.push(if new_status == "done" { ", completed_at = now()" } else { ", completed_at = NULL" });
        if new_status == "in_progress" {
            qb.push(", started_at = COALESCE(started_at, now())");
        }
    }
    if let Some(p) = position {
        qb.push(", position = ").push_bind(p);
    }
    qb.push(" WHERE id = ").push_bind(id);
    let mut tx = state.db.begin().await?;
    qb.build().execute(&mut *tx).await?;
    if changed {
        record_history(&mut *tx, id, auth.id, "status", Some(core.status.clone()), Some(new_status.clone())).await?;
    }
    tx.commit().await?;

    if changed {
        if let Some(stage) = super::stages::sync_after_status(state, id, &new_status).await? {
            super::automation::stage_entered(state, id, stage, auth.id).await;
        }
        if new_status == "in_progress" {
            super::automation::fire(state, id, "started", auth.id).await;
        }
    }
    // Stop running timers when work is finished.
    if changed && (new_status == "done" || new_status == "review") {
        super::parts::stop_all_timers_on(state, id).await?;
    }
    if changed && new_status == "done" {
        crate::workflows::task_done(state, id).await;
        crate::crm::collab::task_event(state, id, "completed", auth.id).await;
    }
    if changed {
        emit_task(state, if new_status == "done" { "task.completed" } else { "task.updated" }, id).await;
    }

    if changed {
        let actor = user_name(state, auth.id).await;
        let t = &core.title;
        match (core.status.as_str(), new_status.as_str()) {
            (_, "review") => {
                notify_task(state, &[core.created_by], "task.review", format!("{actor} finished a task for you to review: {t}"), None, id, auth.id).await
            }
            ("review", "done") => {
                notify_task(state, &[core.responsible_id], "task.accepted", format!("{actor} accepted your task: {t}"), None, id, auth.id).await
            }
            ("review", _) => {
                notify_task(state, &[core.responsible_id], "task.returned", format!("{actor} sent a task back to you: {t}"), None, id, auth.id).await
            }
            (_, "done") => {
                notify_task(state, &[core.created_by], "task.completed", format!("{actor} completed: {t}"), None, id, auth.id).await
            }
            _ => {}
        }
        broadcast_changed(state, id, core.project_id, false).await;
    } else if position.is_some() {
        broadcast_changed(state, id, core.project_id, false).await;
    }
    load_detail(state, auth, id).await
}

async fn change_status(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<StatusChange>,
) -> AppResult<Json<TaskDetail>> {
    Ok(Json(apply_status(&state, &auth, id, &body.status, None).await?))
}

#[derive(Deserialize)]
struct Move {
    status: String,
    /// Place the card between these two neighbours (either may be absent at the ends).
    after_id: Option<Uuid>,
    before_id: Option<Uuid>,
}

/// Drag-and-drop on a board: change column (status) and position in one call.
async fn move_card(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<Move>,
) -> AppResult<Json<TaskDetail>> {
    let pos = |n: Option<Uuid>| {
        let db = state.db.clone();
        async move {
            match n {
                Some(n) => sqlx::query_scalar!("SELECT position FROM tasks WHERE id = $1", n).fetch_optional(&db).await,
                None => Ok(None),
            }
        }
    };
    let after = pos(body.after_id).await?;
    let before = pos(body.before_id).await?;
    let position = match (after, before) {
        (Some(a), Some(b)) => (a + b) / 2.0,
        (Some(a), None) => a + 1.0,
        (None, Some(b)) => b - 1.0,
        (None, None) => 0.0,
    };
    Ok(Json(apply_status(&state, &auth, id, &body.status, Some(position)).await?))
}
