//! More about tasks: the recycle bin, rating finished tasks, a task's result, new comments
//! since I last looked, my planner, custom fields, and deadlines that skip weekends and
//! company holidays.

use std::collections::HashMap;

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    access::{load_task, project_access},
    broadcast_changed,
    items::{load_detail, TaskDetail, TaskSummary},
    notify_task, record_history, user_name,
};
use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

pub const TRASH_PURGE_JOB: &str = "tasks.trash_purge";
/// How long deleted tasks wait in the bin.
const TRASH_DAYS: i64 = 30;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/tasks/trash", get(trash))
        .route("/tasks/trash/{id}/restore", post(restore))
        .route("/tasks/trash/{id}", delete(purge_one))
        .route("/tasks/{id}/rate", post(rate))
        .route("/tasks/{id}/result", put(set_result))
        .route("/tasks/planner", get(planner))
        .route("/tasks/planner/stages", post(new_planner_stage))
        .route("/tasks/planner/stages/{id}", patch(update_planner_stage).delete(delete_planner_stage))
        .route("/tasks/{id}/planner", post(plan_task))
        .route("/tasks/fields", get(list_fields).post(create_field))
        .route("/tasks/fields/{id}", patch(update_field).delete(delete_field))
        .route("/tasks/{id}/fields", put(put_fields))
}

// ---------------------------------------------------------------------------
// Working days
// ---------------------------------------------------------------------------

/// Moves a moment off weekends and company holidays to the same time on the next working day.
pub async fn to_working_day(state: &AppState, at: DateTime<Utc>) -> AppResult<DateTime<Utc>> {
    let weekdays: Vec<i32> = sqlx::query_scalar!("SELECT work_weekdays FROM hr_settings WHERE id").fetch_optional(&state.db).await?.unwrap_or_else(|| vec![1, 2, 3, 4, 5]);
    if weekdays.is_empty() {
        return Ok(at);
    }
    let from = at.date_naive();
    let holidays: Vec<NaiveDate> = sqlx::query_scalar!("SELECT day FROM hr_holidays WHERE day BETWEEN $1 AND $1 + 400", from).fetch_all(&state.db).await?;
    let mut d = at;
    for _ in 0..400 {
        let day = d.date_naive();
        if weekdays.contains(&(day.weekday().number_from_monday() as i32)) && !holidays.contains(&day) {
            return Ok(d);
        }
        d += Duration::days(1);
    }
    Ok(at)
}

// ---------------------------------------------------------------------------
// New comments
// ---------------------------------------------------------------------------

/// Fills `new_comments`: comments by others since the viewer last opened each task.
pub async fn count_new_comments(state: &AppState, viewer: Uuid, items: &mut [TaskSummary]) -> AppResult<()> {
    if items.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = items.iter().map(|t| t.id).collect();
    let rows: Vec<(Uuid, i64)> = sqlx::query!(r#"SELECT cm.task_id AS "task_id!", COUNT(*) AS "c1!" FROM task_comments cm LEFT JOIN task_views v ON v.task_id = cm.task_id AND v.user_id = $2
         WHERE cm.task_id = ANY($1) AND cm.author_id IS DISTINCT FROM $2 AND (v.viewed_at IS NULL OR cm.created_at > v.viewed_at)
         GROUP BY cm.task_id"#, &ids, viewer).map(|r| (r.task_id, r.c1))
    .fetch_all(&state.db)
    .await?;
    let by: HashMap<Uuid, i64> = rows.into_iter().collect();
    for t in items.iter_mut() {
        t.new_comments = by.get(&t.id).copied().unwrap_or(0);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Rating and result
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Rate {
    /// good | bad | null to take it back
    rating: Option<String>,
}

/// The creator rates a finished task; the responsible person is told.
async fn rate(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Rate>) -> AppResult<Json<TaskDetail>> {
    let (core, _) = load_task(&state, &auth, id).await?;
    if core.created_by != auth.id && !auth.can(perm::TASKS_MANAGE_ALL) {
        return Err(AppError::bad("only whoever set the task rates it"));
    }
    if core.status != "done" {
        return Err(AppError::bad("rate a task once it's finished"));
    }
    if b.rating.as_deref().is_some_and(|r| r != "good" && r != "bad") {
        return Err(AppError::bad("rating must be good or bad"));
    }
    sqlx::query("UPDATE tasks SET rating = $2, rated_at = CASE WHEN $2 IS NULL THEN NULL ELSE now() END WHERE id = $1").bind(id).bind(&b.rating).execute(&state.db).await?;
    record_history(&state.db, id, auth.id, "rating", None, b.rating.clone()).await?;
    if let Some(r) = &b.rating {
        let who = user_name(&state, auth.id).await;
        let verdict = if r == "good" { "👍 rated your work well" } else { "👎 wasn't happy with" };
        notify_task(&state, &[core.responsible_id], "task.rated", format!("{who} {verdict}: {}", core.title), None, id, auth.id).await;
    }
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(load_detail(&state, &auth, id).await?))
}

pub async fn result_of(state: &AppState, task: Uuid) -> AppResult<Option<Value>> {
    let row: Option<(Uuid, String, Option<String>, DateTime<Utc>)> = sqlx::query!(r#"SELECT c.id, c.body, trim(u.first_name || ' ' || u.last_name) AS "c2", c.created_at FROM tasks t JOIN task_comments c ON c.id = t.result_comment_id
         LEFT JOIN users u ON u.id = c.author_id WHERE t.id = $1"#, task).map(|r| (r.id, r.body, r.c2, r.created_at))
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|(id, body, author, at)| json!({ "comment_id": id, "body": body, "author_name": author, "created_at": at })))
}

#[derive(Deserialize)]
struct ResultBody {
    /// A comment of the task, or null to unmark it.
    comment_id: Option<Uuid>,
}

/// Marks one of the task's comments as its result.
async fn set_result(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ResultBody>) -> AppResult<Json<TaskDetail>> {
    let (core, access) = load_task(&state, &auth, id).await?;
    if !access.participate {
        return Err(AppError::Forbidden);
    }
    if let Some(c) = b.comment_id {
        let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM task_comments WHERE id = $1 AND task_id = $2) AS "v!""#, c, id).fetch_one(&state.db).await?;
        if !ok {
            return Err(AppError::bad("that comment isn't on this task"));
        }
    }
    sqlx::query!("UPDATE tasks SET result_comment_id = $2, last_activity_at = now() WHERE id = $1", id, b.comment_id).execute(&state.db).await?;
    record_history(&state.db, id, auth.id, "result", None, b.comment_id.map(|_| "marked".to_string())).await?;
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(load_detail(&state, &auth, id).await?))
}

// ---------------------------------------------------------------------------
// Recycle bin
// ---------------------------------------------------------------------------

/// The task's parts, table by table: (table, rows of this task).
const PARTS: &[(&str, &str)] = &[
    ("task_members", "SELECT * FROM task_members WHERE task_id = $1"),
    ("task_checklist_items", "SELECT * FROM task_checklist_items WHERE task_id = $1"),
    ("task_comments", "SELECT * FROM task_comments WHERE task_id = $1"),
    ("task_files", "SELECT * FROM task_files WHERE task_id = $1"),
    ("task_history", "SELECT * FROM task_history WHERE task_id = $1"),
    ("task_time_entries", "SELECT * FROM task_time_entries WHERE task_id = $1"),
    ("task_dependencies", "SELECT * FROM task_dependencies WHERE task_id = $1 OR depends_on_id = $1"),
    ("task_crm_links", "SELECT * FROM task_crm_links WHERE task_id = $1"),
    ("task_field_values", "SELECT * FROM task_field_values WHERE task_id = $1"),
    ("task_views", "SELECT * FROM task_views WHERE task_id = $1"),
];

/// Puts a task in the bin: everything about it is kept, then it is deleted. Subtasks stay
/// (without their parent) and get it back if it is restored.
pub async fn trash_task(state: &AppState, task: Uuid, by: Uuid) -> AppResult<()> {
    let row: Value = sqlx::query_scalar!(r#"SELECT to_jsonb(t) AS "v!" FROM tasks t WHERE id = $1"#, task).fetch_one(&state.db).await?;
    let mut parts = serde_json::Map::new();
    for (table, sql) in PARTS {
        let rows: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COALESCE(jsonb_agg(to_jsonb(x)), '[]') FROM ({sql}) x")))
            .bind(task)
            .fetch_one(&state.db)
            .await?;
        parts.insert((*table).into(), rows);
    }
    let children: Vec<Uuid> = sqlx::query_scalar!("SELECT id FROM tasks WHERE parent_id = $1", task).fetch_all(&state.db).await?;
    let snapshot = json!({ "task": row, "parts": parts, "children": children });
    let mut tx = state.db.begin().await?;
    sqlx::query!("INSERT INTO task_trash (id, number, title, project_id, created_by, responsible_id, snapshot, deleted_by)
         SELECT id, number, title, project_id, created_by, responsible_id, $2, $3 FROM tasks WHERE id = $1
         ON CONFLICT (id) DO UPDATE SET snapshot = EXCLUDED.snapshot, deleted_at = now(), deleted_by = EXCLUDED.deleted_by", task, &snapshot, by)
    .execute(&mut *tx)
    .await?;
    sqlx::query!("DELETE FROM tasks WHERE id = $1", task).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

#[derive(Deserialize)]
struct TrashQuery {
    q: Option<String>,
}

/// Deleted tasks I can put back: those I made or deleted (all of them with "manage all tasks").
async fn trash(State(state): State<AppState>, auth: AuthUser, Query(q): Query<TrashQuery>) -> AppResult<Json<Value>> {
    let term = clean(q.q).map(|t| format!("%{}%", t.replace('%', "\\%").replace('_', "\\_")));
    let rows: Vec<(Uuid, i64, String, Option<String>, Option<String>, DateTime<Utc>)> = sqlx::query!(r#"SELECT t.id, t.number, t.title, (SELECT name FROM projects WHERE id = t.project_id) AS "c3", trim(u.first_name || ' ' || u.last_name) AS "c4", t.deleted_at
         FROM task_trash t LEFT JOIN users u ON u.id = t.deleted_by
         WHERE ($1 OR t.created_by = $2 OR t.deleted_by = $2) AND ($3::text IS NULL OR t.title ILIKE $3)
         ORDER BY t.deleted_at DESC LIMIT 500"#, auth.can(perm::TASKS_MANAGE_ALL), auth.id, term).map(|r| (r.id, r.number, r.title, r.c3, r.c4, r.deleted_at))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "days": TRASH_DAYS,
        "items": rows.into_iter().map(|(id, n, title, project, by, at)| json!({ "id": id, "number": n, "title": title, "project_name": project, "deleted_by_name": by, "deleted_at": at, "purge_at": at + Duration::days(TRASH_DAYS) })).collect::<Vec<_>>(),
    })))
}

async fn trashed(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Value> {
    let row: Option<(Value, Option<Uuid>, Option<Uuid>)> = sqlx::query!(r#"SELECT snapshot, created_by, deleted_by FROM task_trash WHERE id = $1"#, id).map(|r| (r.snapshot, r.created_by, r.deleted_by)).fetch_optional(&state.db).await?;
    let Some((snapshot, creator, deleter)) = row else { return Err(AppError::NotFound("deleted task")) };
    if !(auth.can(perm::TASKS_MANAGE_ALL) || creator == Some(auth.id) || deleter == Some(auth.id)) {
        return Err(AppError::NotFound("deleted task"));
    }
    Ok(snapshot)
}

/// Puts a deleted task back with its parts. Links to things that have gone since (its project,
/// column, sprint, a dependency's other task) are dropped.
async fn restore(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<TaskDetail>> {
    let snapshot = trashed(&state, &auth, id).await?;
    let mut task = snapshot["task"].clone();
    for (col, table) in [("project_id", "projects"), ("parent_id", "tasks"), ("stage_id", "project_stages"), ("template_id", "task_templates"), ("epic_id", "scrum_epics"), ("sprint_id", "scrum_sprints"), ("result_comment_id", "")] {
        let Some(v) = task[col].as_str().and_then(|s| s.parse::<Uuid>().ok()) else { continue };
        if table.is_empty() {
            continue;
        }
        let exists: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE id = $1)"))).bind(v).fetch_one(&state.db).await?;
        if !exists {
            task[col] = Value::Null;
            if col == "project_id" {
                task["stage_id"] = Value::Null;
                task["sprint_id"] = Value::Null;
                task["epic_id"] = Value::Null;
            }
        }
    }
    // A sprint that has finished meanwhile can't take it back.
    if let Some(s) = task["sprint_id"].as_str().and_then(|s| s.parse::<Uuid>().ok()) {
        let done: bool = sqlx::query_scalar!(r#"SELECT status = 'completed' AS "v!" FROM scrum_sprints WHERE id = $1"#, s).fetch_one(&state.db).await?;
        if done {
            task["sprint_id"] = Value::Null;
        }
    }
    let result = task["result_comment_id"].take();
    let mut tx = state.db.begin().await?;
    sqlx::query!("INSERT INTO tasks SELECT * FROM jsonb_populate_record(NULL::tasks, $1)", &task).execute(&mut *tx).await?;
    for (table, _) in PARTS {
        let rows = &snapshot["parts"][table];
        if rows.as_array().is_none_or(|r| r.is_empty()) {
            continue;
        }
        // Rows pointing at things that are gone are left out.
        let keep = match *table {
            "task_dependencies" => "WHERE EXISTS (SELECT 1 FROM tasks a WHERE a.id = r.task_id) AND EXISTS (SELECT 1 FROM tasks b WHERE b.id = r.depends_on_id)",
            "task_files" => "WHERE EXISTS (SELECT 1 FROM files f WHERE f.id = r.file_id)",
            "task_crm_links" => "WHERE EXISTS (SELECT 1 FROM crm_items i WHERE i.id = r.item_id)",
            "task_field_values" => "WHERE EXISTS (SELECT 1 FROM task_field_defs d WHERE d.id = r.field_id)",
            "task_members" | "task_views" | "task_time_entries" => "WHERE EXISTS (SELECT 1 FROM users u WHERE u.id = r.user_id)",
            _ => "",
        };
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT INTO {table} SELECT r.* FROM jsonb_populate_recordset(NULL::{table}, $1) r {keep} ON CONFLICT DO NOTHING"
        )))
        .bind(rows)
        .execute(&mut *tx)
        .await?;
    }
    if result.is_string() {
        sqlx::query!("UPDATE tasks SET result_comment_id = $2 WHERE id = $1 AND EXISTS (SELECT 1 FROM task_comments WHERE id = $2)", id, result.as_str().and_then(|s| s.parse::<Uuid>().ok()))
            .execute(&mut *tx)
            .await?;
    }
    let children: Vec<Uuid> = serde_json::from_value(snapshot["children"].clone()).unwrap_or_default();
    sqlx::query!("UPDATE tasks SET parent_id = $1 WHERE id = ANY($2) AND parent_id IS NULL", id, &children).execute(&mut *tx).await?;
    sqlx::query!("DELETE FROM task_trash WHERE id = $1", id).execute(&mut *tx).await?;
    tx.commit().await?;
    record_history(&state.db, id, auth.id, "restored", None, None).await?;
    let project = task["project_id"].as_str().and_then(|s| s.parse().ok());
    broadcast_changed(&state, id, project, false).await;
    Ok(Json(load_detail(&state, &auth, id).await?))
}

async fn purge_one(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    trashed(&state, &auth, id).await?;
    sqlx::query!("DELETE FROM task_trash WHERE id = $1", id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn run_trash_purge(state: &AppState) -> Result<()> {
    let n = sqlx::query!("DELETE FROM task_trash WHERE deleted_at < now() - make_interval(days => $1)", TRASH_DAYS as i32).execute(&state.db).await?.rows_affected();
    if n > 0 {
        tracing::info!(purged = n, "emptied old tasks from the recycle bin");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Planner
// ---------------------------------------------------------------------------

async fn planner_stages(state: &AppState, user: Uuid) -> AppResult<Vec<(Uuid, String, String, f64)>> {
    let mut stages: Vec<(Uuid, String, String, f64)> =
        sqlx::query!(r#"SELECT id, name, color, sort FROM task_planner_stages WHERE user_id = $1 ORDER BY sort"#, user).map(|r| (r.id, r.name, r.color, r.sort)).fetch_all(&state.db).await?;
    if stages.is_empty() {
        for (i, (n, c)) in [("Do now", "rose"), ("Next", "sky"), ("Later", "slate")].iter().enumerate() {
            sqlx::query!("INSERT INTO task_planner_stages (user_id, name, color, sort) VALUES ($1, $2, $3, $4)", user, n, c, i as f64).execute(&state.db).await?;
        }
        stages = sqlx::query_as("SELECT id, name, color, sort FROM task_planner_stages WHERE user_id = $1 ORDER BY sort").bind(user).fetch_all(&state.db).await?;
    }
    Ok(stages)
}

/// My planner: my own columns, with the open tasks I'm responsible for or assist on.
async fn planner(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let stages = planner_stages(&state, auth.id).await?;
    let mut qb = sqlx::QueryBuilder::new(super::items::SUMMARY_SELECT);
    qb.push(" AND t.status IN ('new','in_progress','review') AND (t.responsible_id = ").push_bind(auth.id)
        .push(" OR EXISTS (SELECT 1 FROM task_members m WHERE m.task_id = t.id AND m.role = 'assistant' AND m.user_id = ").push_bind(auth.id)
        .push(")) ORDER BY t.deadline ASC NULLS LAST LIMIT 500");
    let tasks: Vec<TaskSummary> = qb.build_query_as().fetch_all(&state.db).await?;
    let placed: Vec<(Uuid, Uuid, f64)> = sqlx::query!(r#"SELECT task_id, stage_id, position FROM task_planner WHERE user_id = $1"#, auth.id).map(|r| (r.task_id, r.stage_id, r.position)).fetch_all(&state.db).await?;
    let first = stages.first().map(|s| s.0);
    let items: Vec<Value> = tasks
        .into_iter()
        .map(|t| {
            let p = placed.iter().find(|p| p.0 == t.id);
            let mut v = serde_json::to_value(&t).unwrap();
            v["planner_stage_id"] = json!(p.map(|p| p.1).or(first));
            // The board orders cards by `position`: the planner's own.
            v["position"] = json!(p.map(|p| p.2).unwrap_or(0.0));
            v
        })
        .collect();
    Ok(Json(json!({
        "stages": stages.into_iter().map(|(id, name, color, sort)| json!({ "id": id, "name": name, "color": color, "sort": sort })).collect::<Vec<_>>(),
        "tasks": items,
    })))
}

#[derive(Deserialize)]
struct PlannerStage {
    name: Option<String>,
    color: Option<String>,
    sort: Option<f64>,
}

async fn new_planner_stage(State(state): State<AppState>, auth: AuthUser, Json(b): Json<PlannerStage>) -> AppResult<Json<Value>> {
    let stages = planner_stages(&state, auth.id).await?;
    if stages.len() >= 12 {
        return Err(AppError::bad("at most 12 columns"));
    }
    let name = required("column name", b.name.as_deref().unwrap_or(""), 60)?;
    let color = b.color.unwrap_or_else(|| "slate".into());
    if !super::stages::COLORS.contains(&color.as_str()) {
        return Err(AppError::bad("unknown colour"));
    }
    sqlx::query!("INSERT INTO task_planner_stages (user_id, name, color, sort) VALUES ($1, $2, $3, $4)", auth.id, name, color, b.sort.unwrap_or(stages.last().map(|s| s.3 + 1.0).unwrap_or(0.0)))
        .execute(&state.db)
        .await?;
    planner(State(state), auth).await
}

async fn own_stage(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<()> {
    let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM task_planner_stages WHERE id = $1 AND user_id = $2) AS "v!""#, id, auth.id).fetch_one(&state.db).await?;
    if ok { Ok(()) } else { Err(AppError::NotFound("column")) }
}

async fn update_planner_stage(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PlannerStage>) -> AppResult<Json<Value>> {
    own_stage(&state, &auth, id).await?;
    if let Some(n) = &b.name {
        sqlx::query!("UPDATE task_planner_stages SET name = $2 WHERE id = $1", id, required("column name", n, 60)?).execute(&state.db).await?;
    }
    if let Some(c) = &b.color {
        if !super::stages::COLORS.contains(&c.as_str()) {
            return Err(AppError::bad("unknown colour"));
        }
        sqlx::query!("UPDATE task_planner_stages SET color = $2 WHERE id = $1", id, c).execute(&state.db).await?;
    }
    if let Some(s) = b.sort {
        sqlx::query!("UPDATE task_planner_stages SET sort = $2 WHERE id = $1", id, s).execute(&state.db).await?;
    }
    planner(State(state), auth).await
}

async fn delete_planner_stage(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    own_stage(&state, &auth, id).await?;
    let n: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM task_planner_stages WHERE user_id = $1"#, auth.id).fetch_one(&state.db).await?;
    if n <= 1 {
        return Err(AppError::bad("the planner needs a column"));
    }
    // Its cards go back to the first column.
    sqlx::query!("DELETE FROM task_planner_stages WHERE id = $1", id).execute(&state.db).await?;
    planner(State(state), auth).await
}

#[derive(Deserialize)]
struct PlanTask {
    stage_id: Uuid,
    after_id: Option<Uuid>,
    before_id: Option<Uuid>,
}

async fn plan_task(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PlanTask>) -> AppResult<Json<TaskDetail>> {
    load_task(&state, &auth, id).await?;
    own_stage(&state, &auth, b.stage_id).await?;
    let pos = |n: Option<Uuid>| {
        let db = state.db.clone();
        let user = auth.id;
        async move {
            match n {
                Some(n) => sqlx::query_scalar!("SELECT position FROM task_planner WHERE user_id = $1 AND task_id = $2", user, n).fetch_optional(&db).await,
                None => Ok(None),
            }
        }
    };
    let position = match (pos(b.after_id).await?, pos(b.before_id).await?) {
        (Some(a), Some(c)) => (a + c) / 2.0,
        (Some(a), None) => a + 1.0,
        (None, Some(c)) => c - 1.0,
        (None, None) => 0.0,
    };
    sqlx::query!("INSERT INTO task_planner (user_id, task_id, stage_id, position) VALUES ($1, $2, $3, $4)
         ON CONFLICT (user_id, task_id) DO UPDATE SET stage_id = EXCLUDED.stage_id, position = EXCLUDED.position", auth.id, id, b.stage_id, position)
    .execute(&state.db)
    .await?;
    Ok(Json(load_detail(&state, &auth, id).await?))
}

// ---------------------------------------------------------------------------
// Custom fields
// ---------------------------------------------------------------------------

const KINDS: &[&str] = &["text", "number", "date", "select", "bool", "user", "url"];

#[derive(Deserialize)]
struct FieldQuery {
    /// Fields of this project (with the company-wide ones).
    project_id: Option<Uuid>,
    /// "true": every field, for administration.
    all: Option<String>,
}

async fn defs(state: &AppState, project: Option<Uuid>, all: bool) -> AppResult<Vec<Value>> {
    let rows: Vec<(Uuid, Option<Uuid>, String, String, Vec<String>, bool, bool, i32, Option<String>)> = sqlx::query!(r#"SELECT d.id AS "id!", d.project_id AS "project_id?", d.name AS "name!", d.kind AS "kind!", d.options AS "options!", d.required AS "required!", d.show_in_list AS "show_in_list!", d.sort AS "sort!", p.name AS "c8?"
         FROM task_field_defs d LEFT JOIN projects p ON p.id = d.project_id
         WHERE $2 OR d.project_id IS NULL OR d.project_id = $1 ORDER BY d.project_id NULLS FIRST, d.sort, d.created_at"#, project, all).map(|r| (r.id, r.project_id, r.name, r.kind, r.options, r.required, r.show_in_list, r.sort, r.c8))
    .fetch_all(&state.db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, p, name, kind, options, req, list, sort, pname)| json!({ "id": id, "project_id": p, "project_name": pname, "name": name, "kind": kind, "options": options, "required": req, "show_in_list": list, "sort": sort }))
        .collect())
}

async fn list_fields(State(state): State<AppState>, auth: AuthUser, Query(q): Query<FieldQuery>) -> AppResult<Json<Value>> {
    if let Some(p) = q.project_id {
        project_access(&state, &auth, p).await?;
    }
    let all = q.all.as_deref() == Some("true") && auth.can(perm::TASKS_MANAGE_ALL);
    Ok(Json(json!({ "items": defs(&state, q.project_id, all).await? })))
}

/// Company-wide fields are for "manage all tasks"; a project's for its owners.
async fn require_field_admin(state: &AppState, auth: &AuthUser, project: Option<Uuid>) -> AppResult<()> {
    let ok = match project {
        None => auth.can(perm::TASKS_MANAGE_ALL),
        Some(p) => project_access(state, auth, p).await?.manage,
    };
    if ok { Ok(()) } else { Err(AppError::Forbidden) }
}

#[derive(Deserialize)]
struct FieldInput {
    project_id: Option<Uuid>,
    name: Option<String>,
    kind: Option<String>,
    options: Option<Vec<String>>,
    required: Option<bool>,
    show_in_list: Option<bool>,
    sort: Option<i32>,
}

fn clean_options(kind: &str, options: Option<Vec<String>>) -> AppResult<Vec<String>> {
    let opts: Vec<String> = options.unwrap_or_default().into_iter().filter_map(|o| clean(Some(o))).map(|o| o.chars().take(100).collect()).collect();
    if kind == "select" && opts.is_empty() {
        return Err(AppError::bad("a list field needs its choices"));
    }
    if opts.len() > 100 {
        return Err(AppError::bad("at most 100 choices"));
    }
    Ok(if kind == "select" { opts } else { vec![] })
}

async fn create_field(State(state): State<AppState>, auth: AuthUser, Json(b): Json<FieldInput>) -> AppResult<Json<Value>> {
    require_field_admin(&state, &auth, b.project_id).await?;
    let name = required("field name", b.name.as_deref().unwrap_or(""), 100)?;
    let kind = b.kind.unwrap_or_else(|| "text".into());
    if !KINDS.contains(&kind.as_str()) {
        return Err(AppError::bad(format!("kind must be one of: {}", KINDS.join(", "))));
    }
    let options = clean_options(&kind, b.options)?;
    sqlx::query!("INSERT INTO task_field_defs (project_id, name, kind, options, required, show_in_list, sort) VALUES ($1, $2, $3, $4, $5, $6, $7)", b.project_id, name, &kind, &options, b.required.unwrap_or(false), b.show_in_list.unwrap_or(false), b.sort.unwrap_or(0))
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "items": defs(&state, b.project_id, false).await? })))
}

async fn field_project(state: &AppState, id: Uuid) -> AppResult<(Option<Uuid>, String)> {
    sqlx::query!(r#"SELECT project_id, kind FROM task_field_defs WHERE id = $1"#, id).map(|r| (r.project_id, r.kind)).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("field"))
}

async fn update_field(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<FieldInput>) -> AppResult<Json<Value>> {
    let (project, kind) = field_project(&state, id).await?;
    require_field_admin(&state, &auth, project).await?;
    if let Some(n) = &b.name {
        sqlx::query!("UPDATE task_field_defs SET name = $2 WHERE id = $1", id, required("field name", n, 100)?).execute(&state.db).await?;
    }
    if b.options.is_some() {
        sqlx::query!("UPDATE task_field_defs SET options = $2 WHERE id = $1", id, &(clean_options(&kind, b.options)?)).execute(&state.db).await?;
    }
    if let Some(r) = b.required {
        sqlx::query!("UPDATE task_field_defs SET required = $2 WHERE id = $1", id, r).execute(&state.db).await?;
    }
    if let Some(l) = b.show_in_list {
        sqlx::query!("UPDATE task_field_defs SET show_in_list = $2 WHERE id = $1", id, l).execute(&state.db).await?;
    }
    if let Some(s) = b.sort {
        sqlx::query!("UPDATE task_field_defs SET sort = $2 WHERE id = $1", id, s).execute(&state.db).await?;
    }
    Ok(Json(json!({ "items": defs(&state, project, false).await? })))
}

async fn delete_field(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (project, _) = field_project(&state, id).await?;
    require_field_admin(&state, &auth, project).await?;
    sqlx::query!("DELETE FROM task_field_defs WHERE id = $1", id).execute(&state.db).await?;
    Ok(Json(json!({ "items": defs(&state, project, false).await? })))
}

/// The fields that apply to a task (company-wide and its project's) with its values.
pub async fn field_values(state: &AppState, task: Uuid, project: Option<Uuid>) -> AppResult<Vec<Value>> {
    let mut list = defs(state, project, false).await?;
    let values: Vec<(Uuid, Value)> = sqlx::query!(r#"SELECT field_id, value FROM task_field_values WHERE task_id = $1"#, task).map(|r| (r.field_id, r.value)).fetch_all(&state.db).await?;
    for d in list.iter_mut() {
        let id: Uuid = d["id"].as_str().unwrap().parse().unwrap();
        d["value"] = values.iter().find(|v| v.0 == id).map(|v| v.1.clone()).unwrap_or(Value::Null);
    }
    Ok(list)
}

/// Checks and normalises one value for its field; Null clears it.
async fn check_value(state: &AppState, kind: &str, options: &[String], name: &str, v: &Value) -> AppResult<Value> {
    let bad = || AppError::bad(format!("{name}: not a valid value"));
    Ok(match (kind, v) {
        (_, Value::Null) => Value::Null,
        ("text", Value::String(s)) => match clean(Some(s.clone())) {
            Some(s) => json!(s.chars().take(5000).collect::<String>()),
            None => Value::Null,
        },
        ("url", Value::String(s)) if s.trim().is_empty() => Value::Null,
        ("url", Value::String(s)) if s.starts_with("http://") || s.starts_with("https://") => json!(s.trim()),
        ("number", Value::Number(n)) => json!(n),
        ("number", Value::String(s)) if s.trim().is_empty() => Value::Null,
        ("number", Value::String(s)) => json!(s.trim().parse::<f64>().map_err(|_| bad())?),
        ("date", Value::String(s)) if s.is_empty() => Value::Null,
        ("date", Value::String(s)) => json!(NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| bad())?),
        ("bool", Value::Bool(b)) => json!(b),
        ("select", Value::String(s)) if s.is_empty() => Value::Null,
        ("select", Value::String(s)) if options.contains(s) => json!(s),
        ("user", Value::String(s)) if s.is_empty() => Value::Null,
        ("user", Value::String(s)) => {
            let id: Uuid = s.parse().map_err(|_| bad())?;
            let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users WHERE id = $1) AS "v!""#, id).fetch_one(&state.db).await?;
            if !ok {
                return Err(bad());
            }
            json!(id)
        }
        _ => return Err(bad()),
    })
}

/// Sets custom field values (field id → value; null clears). With `check_required`, a
/// required field that ends up empty is refused.
pub async fn set_fields(state: &AppState, task: Uuid, project: Option<Uuid>, values: &HashMap<Uuid, Value>, check_required: bool) -> AppResult<()> {
    let applicable = defs(state, project, false).await?;
    for (field, v) in values {
        let Some(d) = applicable.iter().find(|d| d["id"].as_str() == Some(&field.to_string())) else {
            return Err(AppError::bad("a field doesn't apply to this task"));
        };
        let options: Vec<String> = serde_json::from_value(d["options"].clone()).unwrap_or_default();
        let v = check_value(state, d["kind"].as_str().unwrap_or("text"), &options, d["name"].as_str().unwrap_or("field"), v).await?;
        if v.is_null() {
            sqlx::query!("DELETE FROM task_field_values WHERE task_id = $1 AND field_id = $2", task, field).execute(&state.db).await?;
        } else {
            sqlx::query!("INSERT INTO task_field_values (task_id, field_id, value) VALUES ($1, $2, $3) ON CONFLICT (task_id, field_id) DO UPDATE SET value = EXCLUDED.value", task, field, v)
                .execute(&state.db)
                .await?;
        }
    }
    if check_required {
        let missing: Option<String> = sqlx::query_scalar!("SELECT d.name FROM task_field_defs d WHERE d.required AND (d.project_id IS NULL OR d.project_id = $2)
               AND NOT EXISTS (SELECT 1 FROM task_field_values v WHERE v.task_id = $1 AND v.field_id = d.id) ORDER BY d.sort LIMIT 1", task, project)
        .fetch_optional(&state.db)
        .await?;
        if let Some(m) = missing {
            return Err(AppError::bad(format!("fill in “{m}”")));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct PutFields {
    values: HashMap<Uuid, Value>,
}

async fn put_fields(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PutFields>) -> AppResult<Json<TaskDetail>> {
    let (core, access) = load_task(&state, &auth, id).await?;
    if !access.edit {
        return Err(AppError::Forbidden);
    }
    set_fields(&state, id, core.project_id, &b.values, true).await?;
    record_history(&state.db, id, auth.id, "fields", None, None).await?;
    broadcast_changed(&state, id, core.project_id, false).await;
    Ok(Json(load_detail(&state, &auth, id).await?))
}
