//! Automation on a project's board.
//!
//! - **Rules** belong to a column and run, in order, when a task enters it: tell someone,
//!   comment, change the responsible person, add an assistant or watcher, set a deadline, the
//!   priority or the status, or make a subtask. Texts may hold `{task}`, `{number}`, `{column}`
//!   and `{responsible}`.
//! - **Triggers** move a task to a column when someone comments on it, when its deadline
//!   passes, when its whole checklist is ticked, or when work on it starts (one column per
//!   event in a project). Entering the column then runs that column's rules.
//!
//! Rules run in the background job queue; a rule that changes the status (and with it the
//! column) can set off the next column's rules, at most a few steps deep. What each rule did
//! is written to the project's automation log. Owners of the project set all this up.

use anyhow::Result;
use axum::{
    extract::{Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{access::project_access, broadcast_changed, notify_task, record_history};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::clean,
    state::AppState,
};

pub const JOB: &str = "tasks.automation";
/// How many columns in a row rules can carry a task through.
const MAX_DEPTH: i64 = 4;

const KINDS: &[&str] = &["notify", "comment", "set_responsible", "add_assistant", "add_watcher", "set_deadline", "set_priority", "set_status", "create_subtask"];
const EVENTS: &[&str] = &["comment", "overdue", "checklist_done", "started"];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/projects/{id}/automation", get(overview))
        .route("/projects/{id}/automation/rules", post(add_rule))
        .route("/projects/{id}/automation/rules/{rule}", delete(remove_rule))
        .route("/projects/{id}/automation/triggers", post(set_trigger))
        .route("/projects/{id}/automation/triggers/{event}", delete(remove_trigger))
}

async fn require_owner(state: &AppState, auth: &AuthUser, project: Uuid) -> AppResult<()> {
    if project_access(state, auth, project).await?.manage { Ok(()) } else { Err(AppError::Forbidden) }
}

async fn overview(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    project_access(&state, &auth, id).await?;
    let rules: Vec<(Uuid, Uuid, String, Value, i32)> =
        sqlx::query_as("SELECT id, stage_id, kind, params, sort FROM project_stage_rules WHERE project_id = $1 ORDER BY sort, created_at").bind(id).fetch_all(&state.db).await?;
    let triggers: Vec<(String, Uuid)> = sqlx::query_as("SELECT event, stage_id FROM project_stage_triggers WHERE project_id = $1").bind(id).fetch_all(&state.db).await?;
    let log: Vec<(Option<Uuid>, Option<i64>, Option<String>, String, bool, chrono::DateTime<Utc>)> = sqlx::query_as(
        "SELECT l.task_id, t.number, t.title, l.what, l.ok, l.created_at FROM project_automation_log l LEFT JOIN tasks t ON t.id = l.task_id
         WHERE l.project_id = $1 ORDER BY l.created_at DESC LIMIT 50",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "rules": rules.into_iter().map(|(id, stage, kind, params, sort)| json!({ "id": id, "stage_id": stage, "kind": kind, "params": params, "sort": sort })).collect::<Vec<_>>(),
        "triggers": triggers.into_iter().map(|(event, stage)| json!({ "event": event, "stage_id": stage })).collect::<Vec<_>>(),
        "log": log.into_iter().map(|(t, n, title, what, ok, at)| json!({ "task_id": t, "number": n, "title": title, "what": what, "ok": ok, "at": at })).collect::<Vec<_>>(),
    })))
}

async fn stage_of(state: &AppState, project: Uuid, stage: Uuid) -> AppResult<()> {
    let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM project_stages WHERE id = $1 AND project_id = $2)").bind(stage).bind(project).fetch_one(&state.db).await?;
    if ok { Ok(()) } else { Err(AppError::bad("that column belongs to another project")) }
}

/// Checks a rule's settings and keeps only what it uses.
async fn check_params(state: &AppState, kind: &str, p: &Value) -> AppResult<Value> {
    let text = |k: &str| -> AppResult<String> {
        clean(p[k].as_str().map(String::from)).map(|t| t.chars().take(2000).collect()).ok_or_else(|| AppError::bad(format!("{k} is missing")))
    };
    let user = |k: &str| -> AppResult<Uuid> { p[k].as_str().and_then(|s| s.parse().ok()).ok_or_else(|| AppError::bad("choose someone")) };
    let active = |u: Uuid| async move {
        let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND is_active)").bind(u).fetch_one(&state.db).await?;
        if ok { Ok::<_, AppError>(u) } else { Err(AppError::bad("choose an active employee")) }
    };
    Ok(match kind {
        "notify" => {
            let to = p["to"].as_str().unwrap_or("responsible").to_string();
            if !["responsible", "creator", "owners", "participants"].contains(&to.as_str()) {
                active(to.parse().map_err(|_| AppError::bad("tell whom?"))?).await?;
            }
            json!({ "to": to, "text": text("text")? })
        }
        "comment" => json!({ "text": text("text")? }),
        "set_responsible" | "add_assistant" | "add_watcher" => json!({ "user_id": active(user("user_id")?).await? }),
        "set_deadline" => {
            let days = p["days"].as_i64().unwrap_or(0);
            let hours = p["hours"].as_i64().unwrap_or(0);
            if !(0..=365).contains(&days) || !(0..=23).contains(&hours) || days + hours == 0 {
                return Err(AppError::bad("a deadline 1 hour to a year away"));
            }
            json!({ "days": days, "hours": hours })
        }
        "set_priority" => {
            let v = p["priority"].as_str().unwrap_or("");
            if !["low", "normal", "high"].contains(&v) {
                return Err(AppError::bad("priority must be low, normal or high"));
            }
            json!({ "priority": v })
        }
        "set_status" => {
            let v = p["status"].as_str().unwrap_or("");
            if !super::access::STATUSES.contains(&v) {
                return Err(AppError::bad("unknown status"));
            }
            json!({ "status": v })
        }
        "create_subtask" => {
            let resp = p["responsible"].as_str().unwrap_or("responsible").to_string();
            if resp != "responsible" && resp != "creator" {
                active(resp.parse().map_err(|_| AppError::bad("who does the subtask?"))?).await?;
            }
            json!({ "title": text("title")?, "responsible": resp })
        }
        _ => return Err(AppError::bad(format!("kind must be one of: {}", KINDS.join(", ")))),
    })
}

#[derive(Deserialize)]
struct NewRule {
    stage_id: Uuid,
    kind: String,
    #[serde(default)]
    params: Value,
}

async fn add_rule(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<NewRule>) -> AppResult<Json<Value>> {
    require_owner(&state, &auth, id).await?;
    stage_of(&state, id, b.stage_id).await?;
    let params = check_params(&state, &b.kind, &b.params).await?;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project_stage_rules WHERE stage_id = $1").bind(b.stage_id).fetch_one(&state.db).await?;
    if n >= 20 {
        return Err(AppError::bad("a column can have at most 20 rules"));
    }
    sqlx::query("INSERT INTO project_stage_rules (project_id, stage_id, kind, params, sort, created_by) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(id)
        .bind(b.stage_id)
        .bind(&b.kind)
        .bind(params)
        .bind(n as i32)
        .bind(auth.id)
        .execute(&state.db)
        .await?;
    overview(State(state), auth, Path(id)).await
}

async fn remove_rule(State(state): State<AppState>, auth: AuthUser, Path((id, rule)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    require_owner(&state, &auth, id).await?;
    sqlx::query("DELETE FROM project_stage_rules WHERE id = $1 AND project_id = $2").bind(rule).bind(id).execute(&state.db).await?;
    overview(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct TriggerBody {
    event: String,
    stage_id: Uuid,
}

async fn set_trigger(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<TriggerBody>) -> AppResult<Json<Value>> {
    require_owner(&state, &auth, id).await?;
    if !EVENTS.contains(&b.event.as_str()) {
        return Err(AppError::bad(format!("event must be one of: {}", EVENTS.join(", "))));
    }
    stage_of(&state, id, b.stage_id).await?;
    sqlx::query(
        "INSERT INTO project_stage_triggers (project_id, stage_id, event) VALUES ($1, $2, $3)
         ON CONFLICT (project_id, event) DO UPDATE SET stage_id = EXCLUDED.stage_id",
    )
    .bind(id)
    .bind(b.stage_id)
    .bind(&b.event)
    .execute(&state.db)
    .await?;
    overview(State(state), auth, Path(id)).await
}

async fn remove_trigger(State(state): State<AppState>, auth: AuthUser, Path((id, event)): Path<(Uuid, String)>) -> AppResult<Json<Value>> {
    require_owner(&state, &auth, id).await?;
    sqlx::query("DELETE FROM project_stage_triggers WHERE project_id = $1 AND event = $2").bind(id).bind(event).execute(&state.db).await?;
    overview(State(state), auth, Path(id)).await
}

// ---------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------

/// A task entered a column: queue the column's rules (if it has any).
pub async fn stage_entered(state: &AppState, task: Uuid, stage: Uuid, actor: Uuid) {
    stage_entered_at(state, task, stage, actor, 0).await
}

async fn stage_entered_at(state: &AppState, task: Uuid, stage: Uuid, actor: Uuid, depth: i64) {
    let has: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM project_stage_rules WHERE stage_id = $1)").bind(stage).fetch_one(&state.db).await.unwrap_or(false);
    if !has || depth >= MAX_DEPTH {
        return;
    }
    if let Err(e) = crate::jobs::enqueue(&state.db, JOB, json!({ "task_id": task, "stage_id": stage, "actor": actor, "depth": depth }), None).await {
        tracing::warn!(error = ?e, "could not queue task automation");
    }
}

/// Something happened to a task: if its project has a trigger for it, move the task there.
pub async fn fire(state: &AppState, task: Uuid, event: &str, actor: Uuid) {
    let target: Option<(Uuid, Option<String>, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT g.stage_id, s.status, t.stage_id, s.name FROM tasks t JOIN project_stage_triggers g ON g.project_id = t.project_id AND g.event = $2
         JOIN project_stages s ON s.id = g.stage_id WHERE t.id = $1",
    )
    .bind(task)
    .bind(event)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let Some((stage, linked, current, name)) = target else { return };
    if current == Some(stage) {
        return;
    }
    if let Err(e) = move_to(state, task, stage, linked, &name, actor).await {
        tracing::warn!(error = ?e, "trigger could not move the task");
        return;
    }
    let project: Option<Uuid> = sqlx::query_scalar("SELECT project_id FROM tasks WHERE id = $1").bind(task).fetch_optional(&state.db).await.ok().flatten().flatten();
    if let Some(p) = project {
        log(state, p, Some(task), format!("Moved to “{name}” ({})", match event {
            "comment" => "a comment",
            "overdue" => "the deadline passed",
            "checklist_done" => "the checklist is done",
            _ => "work started",
        }), true)
        .await;
    }
    broadcast_changed(state, task, project, false).await;
    stage_entered(state, task, stage, actor).await;
}

/// Moves a task to a column (and to the column's status, if it has one).
async fn move_to(state: &AppState, task: Uuid, stage: Uuid, linked: Option<String>, name: &str, actor: Uuid) -> Result<()> {
    sqlx::query("UPDATE tasks SET stage_id = $2, updated_at = now(), last_activity_at = now() WHERE id = $1").bind(task).bind(stage).execute(&state.db).await?;
    record_history(&state.db, task, actor, "stage", None, Some(name.to_string())).await?;
    if let Some(status) = linked {
        set_status(state, task, &status, actor).await?;
    }
    Ok(())
}

async fn set_status(state: &AppState, task: Uuid, status: &str, actor: Uuid) -> Result<()> {
    let old: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = $1").bind(task).fetch_one(&state.db).await?;
    if old == status {
        return Ok(());
    }
    sqlx::query("UPDATE tasks SET status = $2, completed_at = CASE WHEN $2 = 'done' THEN now() ELSE NULL END WHERE id = $1")
    .bind(task)
    .bind(status)
    .execute(&state.db)
    .await?;
    record_history(&state.db, task, actor, "status", Some(old), Some(status.to_string())).await?;
    Ok(())
}

async fn log(state: &AppState, project: Uuid, task: Option<Uuid>, what: String, ok: bool) {
    let _ = sqlx::query("INSERT INTO project_automation_log (project_id, task_id, what, ok) VALUES ($1, $2, $3, $4)")
        .bind(project)
        .bind(task)
        .bind(what)
        .bind(ok)
        .execute(&state.db)
        .await;
}

/// The background job: runs the column's rules on the task (if it is still in that column).
pub async fn run_job(state: &AppState, payload: &Value) -> Result<()> {
    let task: Uuid = serde_json::from_value(payload["task_id"].clone())?;
    let stage: Uuid = serde_json::from_value(payload["stage_id"].clone())?;
    let actor: Uuid = serde_json::from_value(payload["actor"].clone())?;
    let depth = payload["depth"].as_i64().unwrap_or(0);
    let row: Option<(Option<Uuid>, Option<Uuid>, i64, String, Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT t.project_id, t.stage_id, t.number, t.title, t.responsible_id, t.created_by, COALESCE(s.name, '') FROM tasks t
         LEFT JOIN project_stages s ON s.id = t.stage_id WHERE t.id = $1",
    )
    .bind(task)
    .fetch_optional(&state.db)
    .await?;
    let Some((Some(project), current, number, title, _, creator, column)) = row else { return Ok(()) };
    if current != Some(stage) {
        return Ok(());
    }
    let rules: Vec<(String, Value)> = sqlx::query_as("SELECT kind, params FROM project_stage_rules WHERE stage_id = $1 ORDER BY sort, created_at").bind(stage).fetch_all(&state.db).await?;
    let fill = |t: &str, resp_name: &str| t.replace("{task}", &title).replace("{number}", &number.to_string()).replace("{column}", &column).replace("{responsible}", resp_name);
    for (kind, p) in rules {
        // An earlier rule may have changed who is responsible.
        let responsible: Uuid = sqlx::query_scalar("SELECT responsible_id FROM tasks WHERE id = $1").bind(task).fetch_one(&state.db).await?;
        let resp_name = super::user_name(state, responsible).await;
        let outcome: Result<String> = async {
            let uid = |k: &str| p[k].as_str().and_then(|s| s.parse::<Uuid>().ok());
            match kind.as_str() {
                "notify" => {
                    let to: Vec<Uuid> = match p["to"].as_str().unwrap_or("responsible") {
                        "responsible" => vec![responsible],
                        "creator" => vec![creator],
                        "owners" => sqlx::query_scalar("SELECT user_id FROM project_members WHERE project_id = $1 AND role = 'owner'").bind(project).fetch_all(&state.db).await?,
                        "participants" => super::participants(state, task).await,
                        other => other.parse().ok().into_iter().collect(),
                    };
                    let text = fill(p["text"].as_str().unwrap_or(""), &resp_name);
                    notify_task(state, &to, "task.automation", text.clone(), Some(title.clone()), task, Uuid::nil()).await;
                    Ok(format!("Told {} {}: {text}", to.len(), if to.len() == 1 { "person" } else { "people" }))
                }
                "comment" => {
                    let text = fill(p["text"].as_str().unwrap_or(""), &resp_name);
                    sqlx::query("INSERT INTO task_comments (task_id, author_id, body) VALUES ($1, NULL, $2)").bind(task).bind(&text).execute(&state.db).await?;
                    Ok(format!("Commented: {text}"))
                }
                "set_responsible" => {
                    let u = uid("user_id").ok_or_else(|| anyhow::anyhow!("no one chosen"))?;
                    sqlx::query("UPDATE tasks SET responsible_id = $2 WHERE id = $1").bind(task).bind(u).execute(&state.db).await?;
                    sqlx::query("DELETE FROM task_members WHERE task_id = $1 AND user_id = $2").bind(task).bind(u).execute(&state.db).await?;
                    let name = super::user_name(state, u).await;
                    record_history(&state.db, task, actor, "responsible", Some(resp_name.clone()), Some(name.clone())).await?;
                    notify_task(state, &[u], "task.assigned", format!("New task for you: {title}"), None, task, Uuid::nil()).await;
                    Ok(format!("Made {name} responsible"))
                }
                "add_assistant" | "add_watcher" => {
                    let u = uid("user_id").ok_or_else(|| anyhow::anyhow!("no one chosen"))?;
                    let role = if kind == "add_assistant" { "assistant" } else { "watcher" };
                    if u != responsible {
                        sqlx::query("INSERT INTO task_members (task_id, user_id, role) VALUES ($1, $2, $3) ON CONFLICT (task_id, user_id) DO UPDATE SET role = EXCLUDED.role")
                            .bind(task)
                            .bind(u)
                            .bind(role)
                            .execute(&state.db)
                            .await?;
                    }
                    Ok(format!("Added {} as {role}", super::user_name(state, u).await))
                }
                "set_deadline" => {
                    let mut at = Utc::now() + Duration::days(p["days"].as_i64().unwrap_or(0)) + Duration::hours(p["hours"].as_i64().unwrap_or(0));
                    let skip: bool = sqlx::query_scalar("SELECT skip_nonworking FROM tasks WHERE id = $1").bind(task).fetch_one(&state.db).await?;
                    if skip {
                        at = super::depth::to_working_day(state, at).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
                    }
                    sqlx::query("UPDATE tasks SET deadline = $2, deadline_reminded_at = NULL, overdue_notified_at = NULL WHERE id = $1").bind(task).bind(at).execute(&state.db).await?;
                    record_history(&state.db, task, actor, "deadline", None, Some(at.to_rfc3339())).await?;
                    Ok(format!("Set the deadline to {}", at.format("%Y-%m-%d %H:%M UTC")))
                }
                "set_priority" => {
                    let v = p["priority"].as_str().unwrap_or("normal");
                    sqlx::query("UPDATE tasks SET priority = $2 WHERE id = $1").bind(task).bind(v).execute(&state.db).await?;
                    Ok(format!("Set the priority to {v}"))
                }
                "set_status" => {
                    let v = p["status"].as_str().unwrap_or("new");
                    set_status(state, task, v, actor).await?;
                    // The status may take the task to another column, whose rules then run.
                    super::stages::sync_after_status(state, task, v).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
                    let now: Option<Uuid> = sqlx::query_scalar("SELECT stage_id FROM tasks WHERE id = $1").bind(task).fetch_one(&state.db).await?;
                    if let Some(next) = now.filter(|n| *n != stage) {
                        stage_entered_at(state, task, next, actor, depth + 1).await;
                    }
                    Ok(format!("Set the status to {v}"))
                }
                "create_subtask" => {
                    let who = match p["responsible"].as_str().unwrap_or("responsible") {
                        "responsible" => responsible,
                        "creator" => creator,
                        other => other.parse().unwrap_or(responsible),
                    };
                    let sub_title = fill(p["title"].as_str().unwrap_or(""), &resp_name);
                    let number: i64 = sqlx::query_scalar("UPDATE counters SET value = value + 1 WHERE name = 'task_number' RETURNING value").fetch_one(&state.db).await?;
                    let sid: Uuid = sqlx::query_scalar(
                        "INSERT INTO tasks (title, responsible_id, created_by, project_id, parent_id, number, stage_id)
                         VALUES ($1, $2, $3, $4, $5, $6, (SELECT id FROM project_stages WHERE project_id = $4 ORDER BY (status IS NOT DISTINCT FROM 'new') DESC, sort LIMIT 1))
                         RETURNING id",
                    )
                    .bind(&sub_title)
                    .bind(who)
                    .bind(creator)
                    .bind(project)
                    .bind(task)
                    .bind(number)
                    .fetch_one(&state.db)
                    .await?;
                    record_history(&state.db, sid, actor, "created", None, Some(sub_title.clone())).await?;
                    notify_task(state, &[who], "task.assigned", format!("New task for you: {sub_title}"), None, sid, Uuid::nil()).await;
                    broadcast_changed(state, sid, Some(project), false).await;
                    Ok(format!("Made the subtask #{number} {sub_title}"))
                }
                _ => Ok(String::new()),
            }
        }
        .await;
        match outcome {
            Ok(what) => log(state, project, Some(task), what, true).await,
            Err(e) => log(state, project, Some(task), format!("{kind} failed: {e}"), false).await,
        }
    }
    broadcast_changed(state, task, Some(project), false).await;
    Ok(())
}
