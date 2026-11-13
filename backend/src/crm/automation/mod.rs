//! CRM automation.
//!
//! * **Robots** belong to a stage. When a record enters the stage, each active robot is
//!   scheduled (now or after its delay) and runs in the background job queue, if its
//!   conditions hold and the record is still in that stage.
//! * **Triggers** listen for events (an activity completed, a comment, a document, an
//!   incoming webhook) and move the record to their stage, normally only forwards.
//!
//! * A robot runs on entering its stage (after an optional delay) or at a time relative
//!   to one of the record's date fields; it can wait for the robot before it, and it can
//!   wait for something (conditions, the client's reply, a completed activity) with a
//!   timeout. Runs whose stage the record left are skipped.
//! * **Schedules** run an action for every matching record of a pipeline, daily, weekly
//!   or monthly. **Stage time limits** flag records that stay too long.
//!
//! Automated changes are made by the built-in "Automation" user. Chains (a robot moves the
//! record, which runs more robots…) are cut off after a few steps, and each record has an
//! hourly budget of robot runs, so a misconfigured pipeline cannot loop forever.

mod actions;
mod preview;
mod report;
mod schedules;
pub mod sla;

pub use schedules::{run_due as schedules_due, JOB as SCHEDULE_JOB};

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post, put},
    Json, Router,
};
use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;
use std::collections::HashSet;
use uuid::Uuid;

use super::{
    access::is_crm_admin,
    broadcast, entity_type_by_id,
    items::{load_row, update_item, Row},
    timeline,
};
use crate::{
    auth::{permissions, AuthUser},
    error::{AppError, AppResult},
    jobs,
    routes::required,
    state::AppState,
};

pub const JOB: &str = "crm.robot";
/// The built-in user automated changes are attributed to (see migration 0010).
pub const SYSTEM_USER: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_000a_0001);
/// Stage changes made by automation in a row before the chain is stopped.
const MAX_DEPTH: u32 = 5;
/// Robot runs per record per hour.
const HOURLY_BUDGET: i64 = 200;

pub fn system_auth() -> AuthUser {
    AuthUser { id: SYSTEM_USER, session_id: Uuid::nil(), permissions: HashSet::from([permissions::ALL.to_string()]) }
}

tokio::task_local! {
    /// How many automated stage changes led to the code running now.
    static DEPTH: u32;
}

fn depth() -> u32 {
    DEPTH.try_with(|d| *d).unwrap_or(0)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/automation", get(overview))
        .route("/crm/automation/test-mode", put(set_test_mode))
        .route("/crm/robots", post(create_robot))
        .route("/crm/robots/{id}", patch(update_robot).delete(delete_robot))
        .route("/crm/robot-order", post(order_robots))
        .route("/crm/triggers", post(create_trigger))
        .route("/crm/triggers/{id}", patch(update_trigger).delete(delete_trigger))
        .route("/crm/items/{id}/automation", get(item_runs))
        .route("/crm/automation/webhook/{token}", post(webhook))
        .merge(schedules::router())
        .merge(report::router())
        .merge(preview::router())
}

// ---------------------------------------------------------------------------
// Record context: conditions and {{placeholders}}
// ---------------------------------------------------------------------------

/// Everything robots can read about a record: its fields (as in the API) plus names,
/// formatted amount, link and the client's email.
pub struct Ctx {
    pub row: Row,
    pub value: Value,
}

pub(crate) async fn context(state: &AppState, item: Uuid) -> AppResult<Ctx> {
    let row = load_row(state, item, false).await?;
    let t = entity_type_by_id(state, row.entity_type).await?;
    let mut v = serde_json::to_value(&row).unwrap_or_else(|_| json!({}));
    let assigned_email: Option<String> = match row.assigned_to {
        Some(u) => sqlx::query_scalar!("SELECT email FROM users WHERE id = $1", u).fetch_optional(&state.db).await?,
        None => None,
    };
    // The client's email: the record's own, else its primary contact's, else its company's.
    let email: Option<String> = sqlx::query_scalar!("SELECT value FROM crm_multifields WHERE kind = 'email' AND item_id = ANY(ARRAY[
             $1,
             (SELECT contact_id FROM crm_item_contacts WHERE item_id = $1 ORDER BY is_primary DESC, sort LIMIT 1),
             (SELECT company_id FROM crm_items WHERE id = $1)
         ]) ORDER BY item_id = $1 DESC, sort LIMIT 1", item)
    .fetch_optional(&state.db)
    .await?;
    let contact: Option<String> = sqlx::query_scalar!("SELECT i.title FROM crm_item_contacts l JOIN crm_items i ON i.id = l.contact_id WHERE l.item_id = $1 ORDER BY l.is_primary DESC, l.sort LIMIT 1", item)
    .fetch_optional(&state.db)
    .await?;
    let amount = match &row.currency {
        Some(c) => crate::crm::docs::data::fmt_money(row.opportunity, c, &crate::crm::docs::data::locale("en-US")),
        None => row.opportunity.normalize().to_string(),
    };
    if let Value::Object(m) = &mut v {
        m.insert("type".into(), json!(t.name));
        m.insert("stage".into(), json!(row.stage_name.clone().unwrap_or_default()));
        m.insert("assigned".into(), json!(row.assigned_name.clone().unwrap_or_default()));
        m.insert("assigned_email".into(), json!(assigned_email.unwrap_or_default()));
        m.insert("company".into(), json!(row.company_title.clone().unwrap_or_default()));
        m.insert("contact".into(), json!(contact.unwrap_or_default()));
        m.insert("client_email".into(), json!(email.unwrap_or_default()));
        m.insert("amount".into(), json!(amount));
        m.insert("link".into(), json!(format!("{}/crm/{}/{}", state.config.public_url, t.code, item)));
    }
    Ok(Ctx { row, value: v })
}

fn lookup<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |cur, k| cur.get(k))
}

fn as_text(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => if *b { "yes".into() } else { "no".into() },
        Some(Value::Array(a)) => a.iter().map(|x| as_text(Some(x))).collect::<Vec<_>>().join(", "),
        Some(other) => other.to_string(),
    }
}

fn fill_with(text: &str, ctx: &Value, value: impl Fn(String) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(s) = rest.find("{{") {
        let Some(e) = rest[s..].find("}}").map(|e| s + e) else { break };
        out.push_str(&rest[..s]);
        out.push_str(&value(as_text(lookup(ctx, rest[s + 2..e].trim()))));
        rest = &rest[e + 2..];
    }
    out.push_str(rest);
    out
}

/// Replaces `{{field}}` with the record's values.
pub fn fill(text: &str, ctx: &Value) -> String {
    fill_with(text, ctx, |v| v)
}

/// Like `fill`, for JSON bodies: values are escaped as JSON string content, so
/// `{"title": "{{title}}"}` stays valid whatever the title holds.
pub fn fill_json(text: &str, ctx: &Value) -> String {
    fill_with(text, ctx, |v| {
        let quoted = Value::String(v).to_string();
        quoted[1..quoted.len() - 1].to_string()
    })
}

/// Condition values may be relative: `now`, `today`, `today+7`, `today-3` (days, UTC).
fn resolve_value(v: &str, now: DateTime<Utc>) -> String {
    let t = v.trim().to_lowercase();
    if t == "now" {
        return now.format("%Y-%m-%dT%H:%M:%S").to_string();
    }
    let Some(rest) = t.strip_prefix("today") else { return v.to_string() };
    let days = match rest.trim() {
        "" => Some(0),
        r => r.replace(' ', "").parse::<i64>().ok(),
    };
    match days {
        Some(d) if d.abs() <= 36_500 => (now.date_naive() + Duration::days(d)).format("%Y-%m-%d").to_string(),
        _ => v.to_string(),
    }
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Condition {
    field: String,
    op: String,
    #[serde(default)]
    value: Value,
}

const OPS: &[&str] = &["eq", "ne", "gt", "gte", "lt", "lte", "contains", "empty", "not_empty"];

fn check_conditions(v: &Value) -> AppResult<Value> {
    let list: Vec<Condition> = serde_json::from_value(v.clone()).map_err(|_| AppError::bad("conditions must be a list of {field, op, value}"))?;
    if list.len() > 20 {
        return Err(AppError::bad("at most 20 conditions"));
    }
    for c in &list {
        if !OPS.contains(&c.op.as_str()) {
            return Err(AppError::bad(format!("unknown condition operator {}", c.op)));
        }
        if c.field.is_empty() || c.field.len() > 100 {
            return Err(AppError::bad("each condition needs a field"));
        }
    }
    Ok(serde_json::to_value(list).unwrap())
}

/// All conditions must hold. Numbers compare as numbers, everything else as
/// case-insensitive text.
pub fn conditions_hold(conds: &Value, ctx: &Value) -> bool {
    let Ok(list) = serde_json::from_value::<Vec<Condition>>(conds.clone()) else { return true };
    list.iter().all(|c| {
        let actual = as_text(lookup(ctx, &c.field));
        let expected = resolve_value(&as_text(Some(&c.value)), Utc::now());
        let num = |s: &str| s.trim().parse::<f64>().ok();
        let cmp = match (num(&actual), num(&expected)) {
            (Some(a), Some(b)) => a.partial_cmp(&b),
            _ => Some(actual.to_lowercase().cmp(&expected.to_lowercase())),
        };
        use std::cmp::Ordering::*;
        match c.op.as_str() {
            "eq" => cmp == Some(Equal),
            "ne" => cmp != Some(Equal),
            "gt" => cmp == Some(Greater),
            "gte" => matches!(cmp, Some(Greater | Equal)),
            "lt" => cmp == Some(Less),
            "lte" => matches!(cmp, Some(Less | Equal)),
            "contains" => actual.to_lowercase().contains(&expected.to_lowercase()),
            "empty" => actual.trim().is_empty(),
            "not_empty" => !actual.trim().is_empty(),
            _ => false,
        }
    })
}

// ---------------------------------------------------------------------------
// Robots: scheduling and running
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow, Clone)]
pub struct Robot {
    id: Uuid,
    entity_type: i16,
    category_id: Uuid,
    stage_id: Uuid,
    name: String,
    action: String,
    params: Value,
    delay_minutes: i32,
    conditions: Value,
    sort: i32,
    is_active: bool,
    timing: String,
    timing_field: Option<String>,
    offset_minutes: i32,
    after_previous: bool,
    wait: Option<Value>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const ROBOT_COLUMNS: &str = "id, entity_type, category_id, stage_id, name, action, params, delay_minutes, conditions, sort, is_active, timing, timing_field, offset_minutes, after_previous, wait, created_at, updated_at";

/// A date field's value as an instant: datetimes as they are, dates at 09:00 in `tz`.
fn field_instant(v: &Value, field: &str, tz: Tz) -> Option<DateTime<Utc>> {
    let s = as_text(lookup(v, field));
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&Utc));
    }
    let d = NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok()?;
    tz.from_local_datetime(&d.and_time(NaiveTime::from_hms_opt(9, 0, 0)?)).earliest().map(|t| t.with_timezone(&Utc))
}

/// When a robot should run for a record that entered its stage now (None: the date field
/// is empty). Times in the past mean "now".
fn due(r: &Robot, record: &Value, tz: Tz, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let at = match r.timing.as_str() {
        "field" => field_instant(record, r.timing_field.as_deref()?, tz)? + Duration::minutes(r.offset_minutes as i64),
        _ => now + Duration::minutes(r.delay_minutes as i64),
    };
    Some(at.max(now))
}

/// The time zone dates of a record are read in: its responsible person's, else UTC.
async fn record_tz(state: &AppState, assigned: Option<Uuid>) -> Tz {
    let Some(u) = assigned else { return Tz::UTC };
    let name: Option<String> = sqlx::query_scalar!("SELECT timezone FROM users WHERE id = $1", u).fetch_optional(&state.db).await.ok().flatten();
    name.and_then(|n| n.parse::<Tz>().ok()).unwrap_or(Tz::UTC)
}

async fn enqueue_run(state: &AppState, run: Uuid, at: DateTime<Utc>, timeout: bool) -> AppResult<()> {
    let payload = if timeout { json!({ "run_id": run, "timeout": true }) } else { json!({ "run_id": run }) };
    jobs::enqueue(&state.db, JOB, payload, Some(at)).await?;
    state.job_wakeup.notify_one();
    Ok(())
}

/// Called after a record entered a stage (created in it or moved to it). Never fails the
/// caller: problems are logged.
/// Test mode of a pipeline's automation: `None` when the record is left alone (test mode is on
/// and its responsible person isn't a tester), otherwise whether the run is a test run.
pub async fn test_gate(state: &AppState, category: Uuid, assigned: Option<Uuid>) -> AppResult<Option<bool>> {
    let mode: Option<(bool, Vec<Uuid>)> = sqlx::query!(r#"SELECT automation_test_mode, automation_testers FROM crm_categories WHERE id = $1"#, category).map(|r| (r.automation_test_mode, r.automation_testers))
        .fetch_optional(&state.db)
        .await?;
    Ok(match mode {
        Some((true, testers)) => assigned.filter(|a| testers.contains(a)).map(|_| true),
        _ => Some(false),
    })
}

pub async fn stage_entered(state: &AppState, item: Uuid, stage: Uuid) {
    if let Err(e) = schedule(state, item, stage).await {
        tracing::warn!(item = %item, error = ?e, "could not schedule robots");
    }
    if let Err(e) = sla::stage_entered(state, item, stage).await {
        tracing::warn!(item = %item, error = ?e, "could not schedule the stage time limit");
    }
}

async fn schedule(state: &AppState, item: Uuid, stage: Uuid) -> AppResult<()> {
    // Robots still waiting in the stage the record left will never run.
    sqlx::query!("UPDATE crm_robot_runs SET status = 'skipped', result = 'the record left the stage', finished_at = now()
         WHERE item_id = $1 AND status IN ('pending', 'scheduled', 'waiting') AND schedule_id IS NULL AND stage_id <> $2", item, stage)
    .execute(&state.db)
    .await?;
    let robots: Vec<Robot> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {ROBOT_COLUMNS} FROM crm_robots WHERE stage_id = $1 AND is_active ORDER BY sort, created_at"
    )))
    .bind(stage)
    .fetch_all(&state.db)
    .await?;
    if robots.is_empty() {
        return Ok(());
    }
    let d = depth();
    if d >= MAX_DEPTH {
        timeline(&state.db, item, "automation", Some(SYSTEM_USER), None, json!({ "event": "stopped", "reason": "too many automatic stage changes in a row" })).await?;
        return Ok(());
    }
    let mut redis = state.redis.clone();
    let key = format!("crm:automation:budget:{item}");
    let used: i64 = redis.incr(&key, robots.len() as i64).await?;
    if used == robots.len() as i64 {
        let _: () = redis.expire(&key, 3600).await?;
    }
    if used > HOURLY_BUDGET {
        timeline(&state.db, item, "automation", Some(SYSTEM_USER), None, json!({ "event": "stopped", "reason": "hourly limit of automated actions reached" })).await?;
        return Ok(());
    }
    let row = load_row(state, item, false).await?;
    let Some(is_test) = test_gate(state, robots[0].category_id, row.assigned_to).await? else {
        return Ok(());
    };
    let record = serde_json::to_value(&row).unwrap_or_default();
    let tz = record_tz(state, row.assigned_to).await;
    let now = Utc::now();
    let mut previous: Option<Uuid> = None;
    for r in robots {
        let (status, run_at, result) = match (r.after_previous, previous) {
            (true, Some(_)) => ("pending", now, None),
            _ => match due(&r, &record, tz, now) {
                Some(t) => ("scheduled", t, None),
                None => ("skipped", now, Some("the date field is empty")),
            },
        };
        let run: Uuid = sqlx::query_scalar!("INSERT INTO crm_robot_runs (robot_id, robot_name, item_id, stage_id, category_id, action, run_at, depth, status, result, after_run, finished_at, is_test)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, CASE WHEN $9 = 'skipped' THEN now() END, $12) RETURNING id", r.id, &r.name, item, stage, r.category_id, &r.action, run_at, d as i32, status, result, if status == "pending" { previous } else { None }, is_test)
        .fetch_one(&state.db)
        .await?;
        if status == "scheduled" {
            jobs::enqueue(&state.db, JOB, json!({ "run_id": run }), Some(run_at)).await?;
        }
        // A robot skipped up front has nothing to wait for.
        if status != "skipped" {
            previous = Some(run);
        }
    }
    state.job_wakeup.notify_one();
    Ok(())
}

/// After a run ends, start the robots that waited for it ("after the previous robot").
async fn release_next(state: &AppState, finished: Uuid) -> AppResult<()> {
    let mut queue = vec![finished];
    while let Some(done) = queue.pop() {
        let next: Vec<(Uuid, Option<Uuid>, Uuid)> = sqlx::query!(r#"SELECT id, robot_id, item_id FROM crm_robot_runs WHERE after_run = $1 AND status = 'pending'"#, done).map(|r| (r.id, r.robot_id, r.item_id))
            .fetch_all(&state.db)
            .await?;
        for (run, robot_id, item) in next {
            let robot = match robot_id {
                Some(id) => sqlx::query_as::<_, Robot>(sqlx::AssertSqlSafe(format!("SELECT {ROBOT_COLUMNS} FROM crm_robots WHERE id = $1"))).bind(id).fetch_optional(&state.db).await?,
                None => None,
            };
            let at = match (&robot, load_row(state, item, false).await) {
                (Some(r), Ok(row)) => {
                    let tz = record_tz(state, row.assigned_to).await;
                    due(r, &serde_json::to_value(&row).unwrap_or_default(), tz, Utc::now())
                }
                _ => Some(Utc::now()), // the run itself reports what is gone
            };
            match at {
                Some(at) => {
                    let n = sqlx::query!("UPDATE crm_robot_runs SET status = 'scheduled', run_at = $2 WHERE id = $1 AND status = 'pending'", run, at).execute(&state.db).await?.rows_affected();
                    if n == 1 {
                        enqueue_run(state, run, at, false).await?;
                    }
                }
                None => {
                    sqlx::query!("UPDATE crm_robot_runs SET status = 'skipped', result = 'the date field is empty', finished_at = now() WHERE id = $1 AND status = 'pending'", run).execute(&state.db).await?;
                    queue.push(run);
                }
            }
        }
    }
    Ok(())
}

async fn finish_run(state: &AppState, run: Uuid, status: &str, result: &str) -> AppResult<()> {
    sqlx::query!("UPDATE crm_robot_runs SET status = $2, result = $3, finished_at = now(), wait_until = NULL WHERE id = $1", run, status, result.chars().take(2000).collect::<String>())
        .execute(&state.db)
        .await?;
    release_next(state, run).await
}

/// Puts a run to sleep until something happens or `minutes` pass.
async fn enter_wait(state: &AppState, run: Uuid, minutes: i64, data: Value) -> AppResult<()> {
    let until = Utc::now() + Duration::minutes(minutes.max(1));
    sqlx::query!("UPDATE crm_robot_runs SET status = 'waiting', wait_until = $2, data = data || $3 WHERE id = $1", run, until, data)
        .execute(&state.db)
        .await?;
    enqueue_run(state, run, until, true).await
}

/// Wakes waiting runs: they run now and do not wait again.
async fn resume(state: &AppState, runs: &[Uuid]) -> AppResult<()> {
    for &run in runs {
        let n = sqlx::query!("UPDATE crm_robot_runs SET status = 'scheduled', run_at = now(), wait_until = NULL, data = data || '{\"waited\": true}' WHERE id = $1 AND status = 'waiting'", run)
            .execute(&state.db)
            .await?
            .rows_affected();
        if n == 1 {
            enqueue_run(state, run, Utc::now(), false).await?;
        }
    }
    Ok(())
}

/// A record was edited: robots timed from a date field follow the new date, and robots
/// waiting for conditions check them again. Never fails the caller.
pub async fn record_changed(state: &AppState, item: Uuid) {
    if let Err(e) = try_record_changed(state, item).await {
        tracing::warn!(item = %item, error = ?e, "could not update scheduled robots");
    }
}

async fn try_record_changed(state: &AppState, item: Uuid) -> AppResult<()> {
    let open: Vec<(Uuid, String, DateTime<Utc>, Uuid)> = sqlx::query!(r#"SELECT r.id, r.status, r.run_at, b.id AS "c3" FROM crm_robot_runs r JOIN crm_robots b ON b.id = r.robot_id
         WHERE r.item_id = $1 AND ((r.status = 'scheduled' AND b.timing = 'field')
            OR (r.status = 'waiting' AND b.wait->>'until' = 'conditions' AND NOT (r.data ? 'document_id')))"#, item).map(|r| (r.id, r.status, r.run_at, r.c3))
    .fetch_all(&state.db)
    .await?;
    if open.is_empty() {
        return Ok(());
    }
    let ctx = match context(state, item).await {
        Ok(c) => c,
        Err(AppError::NotFound(_)) => return Ok(()),
        Err(e) => return Err(e),
    };
    let record = serde_json::to_value(&ctx.row).unwrap_or_default();
    let tz = record_tz(state, ctx.row.assigned_to).await;
    let mut wake = Vec::new();
    for (run, status, run_at, robot_id) in open {
        let robot: Robot = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {ROBOT_COLUMNS} FROM crm_robots WHERE id = $1"))).bind(robot_id).fetch_one(&state.db).await?;
        if status == "waiting" {
            let conds = robot.wait.as_ref().and_then(|w| w.get("conditions")).cloned().unwrap_or(json!([]));
            if conditions_hold(&conds, &ctx.value) {
                wake.push(run);
            }
            continue;
        }
        match due(&robot, &record, tz, Utc::now()) {
            Some(at) if (at - run_at).num_seconds().abs() >= 1 => {
                // The old job finds the run not yet due and leaves it; the new one runs it.
                sqlx::query!("UPDATE crm_robot_runs SET run_at = $2 WHERE id = $1 AND status = 'scheduled'", run, at).execute(&state.db).await?;
                enqueue_run(state, run, at, false).await?;
            }
            Some(_) => {}
            None => finish_run(state, run, "skipped", "the date field was cleared").await?,
        }
    }
    resume(state, &wake).await
}

/// A document finished rendering: an email robot waiting to attach it carries on.
pub async fn document_finished(state: &AppState, item: Uuid, document: Uuid) {
    crate::crm::sign_templates::document_ready(state, document).await;
    let runs: Result<Vec<Uuid>, _> = sqlx::query_scalar!("SELECT id FROM crm_robot_runs WHERE item_id = $1 AND status = 'waiting' AND data->>'document_id' = $2", item, document.to_string())
        .fetch_all(&state.db)
        .await;
    if let Ok(runs) = runs
        && let Err(e) = resume(state, &runs).await {
            tracing::warn!(item = %item, error = ?e, "could not resume robots waiting for a document");
        }
}

#[derive(FromRow)]
struct Claim {
    robot_id: Option<Uuid>,
    schedule_id: Option<Uuid>,
    item_id: Uuid,
    stage_id: Uuid,
    depth: i32,
    robot_name: String,
    data: Value,
    attempts: i32,
}

/// Background job: runs one scheduled robot (or ends one whose wait timed out).
pub async fn run_job(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let run_id: Uuid = serde_json::from_value(payload["run_id"].clone())?;
    let timed_out = payload["timeout"].as_bool().unwrap_or(false);
    // Claim it, so a retried job never runs a robot twice. A run moved to a later time
    // is left to the job queued for that time.
    let sql = if timed_out {
        "UPDATE crm_robot_runs SET status = 'running' WHERE id = $1 AND status = 'waiting' AND wait_until <= now() + interval '5 seconds'
         RETURNING robot_id, schedule_id, item_id, stage_id, depth, robot_name, data, attempts"
    } else {
        "UPDATE crm_robot_runs SET status = 'running' WHERE id = $1 AND status = 'scheduled' AND run_at <= now() + interval '5 seconds'
         RETURNING robot_id, schedule_id, item_id, stage_id, depth, robot_name, data, attempts"
    };
    let claimed: Option<Claim> = sqlx::query_as(sql).bind(run_id).fetch_optional(&state.db).await?;
    let Some(c) = claimed else { return Ok(()) };
    let result = DEPTH.scope(c.depth as u32 + 1, run_robot(state, &c, run_id, timed_out)).await;
    let ae = |e: AppError| anyhow::anyhow!("{e:?}");
    let (status, text) = match result {
        Ok(Outcome::Done(t)) => ("done", t),
        Ok(Outcome::Skipped(t)) => ("skipped", t),
        Ok(Outcome::Waiting) => return Ok(()),
        Err(f) if f.retry && c.attempts < RETRIES.len() as i32 => {
            let at = Utc::now() + Duration::minutes(RETRIES[c.attempts as usize]);
            sqlx::query!("UPDATE crm_robot_runs SET status = 'scheduled', run_at = $2, attempts = attempts + 1, result = $3, data = data || '{\"waited\": true}' WHERE id = $1", run_id, at, format!("attempt {} failed: {}; trying again at {}", c.attempts + 1, f.msg, at.format("%H:%M UTC")))
                .execute(&state.db)
                .await?;
            enqueue_run(state, run_id, at, false).await.map_err(ae)?;
            return Ok(());
        }
        Err(f) => ("failed", f.msg),
    };
    finish_run(state, run_id, status, &text).await.map_err(ae)?;
    if status != "skipped" {
        let _ = timeline(&state.db, c.item_id, "robot", Some(SYSTEM_USER), None, json!({ "name": c.robot_name, "status": status, "result": text })).await;
        if let Ok(row) = load_row(state, c.item_id, false).await {
            broadcast(state, row.entity_type, c.item_id, false).await;
        }
    }
    Ok(())
}

/// Minutes before each automatic retry of a webhook that failed for a passing reason.
const RETRIES: &[i64] = &[1, 5, 30];

enum Outcome {
    Done(String),
    Skipped(String),
    Waiting,
}

async fn run_robot(state: &AppState, c: &Claim, run: Uuid, timed_out: bool) -> Result<Outcome, actions::Fail> {
    let e = |e: AppError| actions::Fail::from(e.to_string());
    let robot = match (c.schedule_id, c.robot_id) {
        (Some(s), _) => schedules::as_robot(state, s).await.map_err(e)?,
        (None, Some(r)) => sqlx::query_as::<_, Robot>(sqlx::AssertSqlSafe(format!("SELECT {ROBOT_COLUMNS} FROM crm_robots WHERE id = $1")))
            .bind(r)
            .fetch_optional(&state.db)
            .await
            .map_err(|x| e(x.into()))?,
        _ => None,
    };
    let Some(robot) = robot else { return Ok(Outcome::Skipped("the robot was deleted".into())) };
    if !robot.is_active {
        return Ok(Outcome::Skipped("the robot was turned off".into()));
    }
    let ctx = match context(state, c.item_id).await {
        Ok(c) => c,
        Err(AppError::NotFound(_)) => return Ok(Outcome::Skipped("the record was deleted".into())),
        Err(x) => return Err(e(x)),
    };
    // A robot belongs to its stage: if the record has moved on, the run is void.
    if c.schedule_id.is_none() && ctx.row.stage_id != Some(c.stage_id) {
        return Ok(Outcome::Skipped("the record left the stage".into()));
    }
    let waiting_for_document = c.data.get("document_id").is_some();
    let mut data = c.data.clone();
    if timed_out {
        if waiting_for_document {
            return Err("the document was not ready in time".into());
        }
        if robot.wait.as_ref().and_then(|w| w["on_timeout"].as_str()) == Some("skip") {
            return Ok(Outcome::Skipped("stopped waiting: timed out".into()));
        }
        data["waited"] = json!(true);
    } else if let Some(w) = robot.wait.as_ref().filter(|_| c.data["waited"] != json!(true) && !waiting_for_document) {
        let ready = w["until"] == "conditions" && conditions_hold(&w["conditions"], &ctx.value);
        if !ready {
            enter_wait(state, run, w["timeout_minutes"].as_i64().unwrap_or(1440), json!({})).await.map_err(e)?;
            return Ok(Outcome::Waiting);
        }
    }
    if !conditions_hold(&robot.conditions, &ctx.value) {
        return Ok(Outcome::Skipped("conditions not met".into()));
    }
    match actions::execute(state, &robot, &ctx, &data).await? {
        actions::Step::Done(t) => Ok(Outcome::Done(t)),
        actions::Step::AwaitDocument(doc) => {
            enter_wait(state, run, 15, json!({ "document_id": doc })).await.map_err(e)?;
            Ok(Outcome::Waiting)
        }
    }
}

// ---------------------------------------------------------------------------
// Triggers
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow, Clone)]
pub struct Trigger {
    id: Uuid,
    entity_type: i16,
    category_id: Uuid,
    stage_id: Uuid,
    name: String,
    event: String,
    params: Value,
    conditions: Value,
    allow_backwards: bool,
    token: Option<String>,
    sort: i32,
    is_active: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const TRIGGER_COLUMNS: &str = "id, entity_type, category_id, stage_id, name, event, params, conditions, allow_backwards, token, sort, is_active, created_at, updated_at";
/// Events a robot can wait for.
const WAIT_EVENTS: &[&str] = &["email_received", "activity_done"];
const EVENTS: &[&str] = &["activity_done", "comment_added", "document_created", "email_received", "webform_submitted", "webhook", "signature_completed", "signature_declined", "tasks_done", "booking"];

/// An event happened on a record: the first matching trigger of its pipeline moves it.
/// `matches` checks the trigger's event parameters. Never fails the caller.
pub async fn fire(state: &AppState, item: Uuid, event: &str, matches: impl Fn(&Value) -> bool) {
    if let Err(e) = wake_on(state, item, event).await {
        tracing::warn!(item = %item, event, error = ?e, "could not wake waiting robots");
    }
    if let Err(e) = try_fire(state, item, event, None, matches).await {
        tracing::warn!(item = %item, event, error = ?e, "trigger failed");
    }
}

/// Robots waiting for this event on the record carry on.
async fn wake_on(state: &AppState, item: Uuid, event: &str) -> AppResult<()> {
    if !WAIT_EVENTS.contains(&event) {
        return Ok(());
    }
    let runs: Vec<Uuid> = sqlx::query_scalar!("SELECT r.id FROM crm_robot_runs r JOIN crm_robots b ON b.id = r.robot_id
         WHERE r.item_id = $1 AND r.status = 'waiting' AND b.wait->>'until' = $2 AND NOT (r.data ? 'document_id')", item, event)
    .fetch_all(&state.db)
    .await?;
    resume(state, &runs).await
}

async fn try_fire(state: &AppState, item: Uuid, event: &str, only: Option<Uuid>, matches: impl Fn(&Value) -> bool) -> AppResult<bool> {
    let (category, stage): (Option<Uuid>, Option<Uuid>) = match sqlx::query_as("SELECT category_id, stage_id FROM crm_items WHERE id = $1 AND deleted_at IS NULL")
        .bind(item)
        .fetch_optional(&state.db)
        .await?
    {
        Some(x) => x,
        None => return Ok(false),
    };
    let (Some(category), Some(stage)) = (category, stage) else { return Ok(false) };
    let triggers: Vec<Trigger> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TRIGGER_COLUMNS} FROM crm_triggers WHERE category_id = $1 AND event = $2 AND is_active AND ($3::uuid IS NULL OR id = $3) ORDER BY sort, created_at"
    )))
    .bind(category)
    .bind(event)
    .bind(only)
    .fetch_all(&state.db)
    .await?;
    if triggers.is_empty() {
        return Ok(false);
    }
    let sorts: Vec<(Uuid, i32)> = sqlx::query!(r#"SELECT id, sort FROM crm_stages WHERE category_id = $1"#, category).map(|r| (r.id, r.sort)).fetch_all(&state.db).await?;
    let sort_of = |s: Uuid| sorts.iter().find(|x| x.0 == s).map(|x| x.1).unwrap_or(0);
    let ctx = context(state, item).await?;
    let Some(is_test) = test_gate(state, category, ctx.row.assigned_to).await? else {
        return Ok(false);
    };
    for t in triggers {
        if t.stage_id == stage || !matches(&t.params) || !conditions_hold(&t.conditions, &ctx.value) {
            continue;
        }
        if !t.allow_backwards && sort_of(t.stage_id) <= sort_of(stage) {
            continue;
        }
        let et = entity_type_by_id(state, ctx.row.entity_type).await?;
        let mut body = Map::new();
        body.insert("stage_id".into(), json!(t.stage_id));
        update_item(state, &system_auth(), &et, item, body).await?;
        timeline(&state.db, item, "trigger", Some(SYSTEM_USER), None, json!({ "name": t.name, "event": event, "test": is_test })).await?;
        return Ok(true);
    }
    Ok(false)
}

#[derive(Deserialize)]
struct WebhookBody {
    item_id: Option<Uuid>,
    id: Option<Uuid>,
}

/// Incoming webhook: `POST /api/crm/automation/webhook/{token}` with `{"item_id": "…"}`.
/// The token is the secret; no session is needed, so other systems can move records.
async fn webhook(State(state): State<AppState>, Path(token): Path<String>, Json(b): Json<WebhookBody>) -> AppResult<Json<Value>> {
    let trigger: Option<(Uuid, Uuid)> = sqlx::query!(r#"SELECT id, category_id FROM crm_triggers WHERE token = $1 AND event = 'webhook' AND is_active"#, &token).map(|r| (r.id, r.category_id))
        .fetch_optional(&state.db)
        .await?;
    let Some((id, category)) = trigger else { return Err(AppError::NotFound("webhook")) };
    let mut redis = state.redis.clone();
    let key = format!("crm:automation:webhook:{id}");
    let n: i64 = redis.incr(&key, 1).await?;
    if n == 1 {
        let _: () = redis.expire(&key, 60).await?;
    }
    if n > 120 {
        return Err(AppError::TooManyRequests("too many calls to this webhook; try again in a minute".into()));
    }
    let item = b.item_id.or(b.id).ok_or_else(|| AppError::bad("item_id is required"))?;
    let in_category: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM crm_items WHERE id = $1 AND category_id = $2 AND deleted_at IS NULL) AS "v!""#, item, category)
        .fetch_one(&state.db)
        .await?;
    if !in_category {
        return Err(AppError::NotFound("record"));
    }
    let moved = try_fire(&state, item, "webhook", Some(id), |_| true).await?;
    Ok(Json(json!({ "moved": moved })))
}

// ---------------------------------------------------------------------------
// Configuration API (crm.admin)
// ---------------------------------------------------------------------------

fn require_admin(auth: &AuthUser) -> AppResult<()> {
    if is_crm_admin(auth) { Ok(()) } else { Err(AppError::Forbidden) }
}

#[derive(Deserialize)]
struct OverviewQuery {
    category_id: Uuid,
}

async fn overview(State(state): State<AppState>, auth: AuthUser, Query(q): Query<OverviewQuery>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let robots: Vec<Robot> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {ROBOT_COLUMNS} FROM crm_robots WHERE category_id = $1 ORDER BY sort, created_at")))
        .bind(q.category_id)
        .fetch_all(&state.db)
        .await?;
    let triggers: Vec<Trigger> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {TRIGGER_COLUMNS} FROM crm_triggers WHERE category_id = $1 ORDER BY sort, created_at")))
        .bind(q.category_id)
        .fetch_all(&state.db)
        .await?;
    let et: i16 = sqlx::query_scalar!("SELECT entity_type FROM crm_categories WHERE id = $1", q.category_id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("pipeline"))?;
    let date_fields: Vec<Value> = date_fields(&state, et).await?.into_iter().map(|(code, label)| json!({ "code": code, "label": label })).collect();
    let (test_mode, testers): (bool, Vec<Uuid>) =
        sqlx::query!(r#"SELECT automation_test_mode, automation_testers FROM crm_categories WHERE id = $1"#, q.category_id).map(|r| (r.automation_test_mode, r.automation_testers)).fetch_one(&state.db).await?;
    Ok(Json(json!({
        "test_mode": { "enabled": test_mode, "user_ids": testers },
        "robots": robots,
        "triggers": triggers,
        "date_fields": date_fields,
        "webhook_base": format!("{}/api/crm/automation/webhook/", state.config.public_url),
    })))
}

#[derive(Deserialize)]
struct TestMode {
    category_id: Uuid,
    enabled: bool,
    #[serde(default)]
    user_ids: Vec<Uuid>,
}

/// Turns a pipeline's test mode on (for the chosen testers' records only) or off.
async fn set_test_mode(State(state): State<AppState>, auth: AuthUser, Json(mut b): Json<TestMode>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    b.user_ids.sort();
    b.user_ids.dedup();
    if b.enabled && b.user_ids.is_empty() {
        return Err(AppError::bad("choose who tests the automation"));
    }
    if b.user_ids.len() > 50 {
        return Err(AppError::bad("choose at most 50 testers"));
    }
    let known: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM users WHERE id = ANY($1) AND NOT is_guest"#, &b.user_ids).fetch_one(&state.db).await?;
    if known as usize != b.user_ids.len() {
        return Err(AppError::bad("choose employees"));
    }
    let n = sqlx::query!("UPDATE crm_categories SET automation_test_mode = $2, automation_testers = $3 WHERE id = $1", b.category_id, b.enabled, &b.user_ids)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("pipeline"));
    }
    Ok(Json(json!({ "enabled": b.enabled, "user_ids": b.user_ids })))
}

/// The stage must belong to the pipeline, and the pipeline to a record type with stages.
async fn stage_in(state: &AppState, category: Uuid, stage: Uuid) -> AppResult<i16> {
    let et: Option<i16> = sqlx::query_scalar!("SELECT c.entity_type FROM crm_stages s JOIN crm_categories c ON c.id = s.category_id WHERE s.id = $1 AND c.id = $2", stage, category)
    .fetch_optional(&state.db)
    .await?;
    et.ok_or_else(|| AppError::bad("that stage is not in this pipeline"))
}

#[derive(Deserialize)]
struct RobotBody {
    category_id: Option<Uuid>,
    stage_id: Option<Uuid>,
    name: Option<String>,
    action: Option<String>,
    params: Option<Value>,
    delay_minutes: Option<i32>,
    conditions: Option<Value>,
    is_active: Option<bool>,
    timing: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    timing_field: Option<Option<String>>,
    offset_minutes: Option<i32>,
    after_previous: Option<bool>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    wait: Option<Option<Value>>,
}

/// Date fields a robot can be timed from.
pub async fn date_fields(state: &AppState, entity_type: i16) -> AppResult<Vec<(String, String)>> {
    let t = entity_type_by_id(state, entity_type).await?;
    let mut out = vec![("created_at".to_string(), "Created".to_string())];
    for f in super::fields::system_fields_for(&t) {
        if f.kind == "date" && f.code != "birthdate" {
            out.push((f.code.to_string(), f.label.to_string()));
        }
    }
    for f in super::fields::custom_fields(state, entity_type).await? {
        if (f.kind == "date" || f.kind == "datetime") && !f.is_multiple {
            out.push((format!("uf.{}", f.code), f.label));
        }
    }
    Ok(out)
}

async fn check_timing(state: &AppState, entity_type: i16, timing: &str, field: Option<String>, offset: i32) -> AppResult<(String, Option<String>, i32)> {
    match timing {
        "stage" => Ok(("stage".into(), None, 0)),
        "field" => {
            let field = field.filter(|f| !f.is_empty()).ok_or_else(|| AppError::bad("choose the date field the robot is timed from"))?;
            if !date_fields(state, entity_type).await?.iter().any(|f| f.0 == field) {
                return Err(AppError::bad("choose a date field of this record type"));
            }
            if !(-525_600..=525_600).contains(&offset) {
                return Err(AppError::bad("the time before or after the date must be within a year"));
            }
            Ok(("field".into(), Some(field), offset))
        }
        _ => Err(AppError::bad("timing must be stage or field")),
    }
}

/// `{until, conditions, timeout_minutes, on_timeout}` or null.
fn check_wait(v: Option<Value>) -> AppResult<Option<Value>> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(None) };
    let until = v["until"].as_str().unwrap_or("");
    if until != "conditions" && !WAIT_EVENTS.contains(&until) {
        return Err(AppError::bad("wait until conditions hold, the client replies, or an activity is completed"));
    }
    let conditions = if until == "conditions" {
        let c = check_conditions(&v.get("conditions").cloned().unwrap_or(json!([])))?;
        if c.as_array().is_none_or(Vec::is_empty) {
            return Err(AppError::bad("add the conditions to wait for"));
        }
        c
    } else {
        json!([])
    };
    let timeout = v["timeout_minutes"].as_i64().unwrap_or(1440);
    if !(1..=525_600).contains(&timeout) {
        return Err(AppError::bad("stop waiting after 1 minute to 365 days"));
    }
    let on_timeout = v["on_timeout"].as_str().unwrap_or("run");
    if !["run", "skip"].contains(&on_timeout) {
        return Err(AppError::bad("after the timeout, either run the robot or skip it"));
    }
    Ok(Some(json!({ "until": until, "conditions": conditions, "timeout_minutes": timeout, "on_timeout": on_timeout })))
}

async fn load_robot(state: &AppState, id: Uuid) -> AppResult<Robot> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {ROBOT_COLUMNS} FROM crm_robots WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("robot"))
}

fn check_delay(d: i32) -> AppResult<i32> {
    if (0..=525_600).contains(&d) { Ok(d) } else { Err(AppError::bad("the delay must be between 0 and 365 days")) }
}

async fn create_robot(State(state): State<AppState>, auth: AuthUser, Json(b): Json<RobotBody>) -> AppResult<Json<Robot>> {
    require_admin(&auth)?;
    let category = b.category_id.ok_or_else(|| AppError::bad("category_id is required"))?;
    let stage = b.stage_id.ok_or_else(|| AppError::bad("stage_id is required"))?;
    let et = stage_in(&state, category, stage).await?;
    let action = b.action.unwrap_or_default();
    let params = actions::validate(&state, et, category, &action, b.params.unwrap_or_else(|| json!({}))).await?;
    let name = required("name", b.name.as_deref().unwrap_or(actions::label(&action)), 200)?;
    let (timing, field, offset) = check_timing(&state, et, b.timing.as_deref().unwrap_or("stage"), b.timing_field.flatten(), b.offset_minutes.unwrap_or(0)).await?;
    let id: Uuid = sqlx::query_scalar!("INSERT INTO crm_robots (entity_type, category_id, stage_id, name, action, params, delay_minutes, conditions, is_active, created_by, sort,
                                 timing, timing_field, offset_minutes, after_previous, wait)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, (SELECT COALESCE(MAX(sort), 0) + 10 FROM crm_robots WHERE stage_id = $3),
                 $11, $12, $13, $14, $15) RETURNING id", et, category, stage, name, &action, params, check_delay(b.delay_minutes.unwrap_or(0))?, check_conditions(&b.conditions.unwrap_or_else(|| json!([])))?, b.is_active.unwrap_or(true), auth.id, timing, field, offset, b.after_previous.unwrap_or(false), check_wait(b.wait.flatten())?)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(load_robot(&state, id).await?))
}

async fn update_robot(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<RobotBody>) -> AppResult<Json<Robot>> {
    require_admin(&auth)?;
    let cur = load_robot(&state, id).await?;
    let stage = b.stage_id.unwrap_or(cur.stage_id);
    stage_in(&state, cur.category_id, stage).await?;
    let action = b.action.unwrap_or(cur.action.clone());
    let params = actions::validate(&state, cur.entity_type, cur.category_id, &action, b.params.unwrap_or(cur.params)).await?;
    let (timing, field, offset) = check_timing(
        &state,
        cur.entity_type,
        b.timing.as_deref().unwrap_or(&cur.timing),
        b.timing_field.unwrap_or(cur.timing_field),
        b.offset_minutes.unwrap_or(cur.offset_minutes),
    )
    .await?;
    sqlx::query!("UPDATE crm_robots SET stage_id = $2, name = $3, action = $4, params = $5, delay_minutes = $6, conditions = $7, is_active = $8,
                timing = $9, timing_field = $10, offset_minutes = $11, after_previous = $12, wait = $13, updated_at = now() WHERE id = $1", id, stage, match b.name { Some(n) => required("name", &n, 200)?, None => cur.name }, &action, params, check_delay(b.delay_minutes.unwrap_or(cur.delay_minutes))?, match b.conditions { Some(c) => check_conditions(&c)?, None => cur.conditions }, b.is_active.unwrap_or(cur.is_active), timing, field, offset, b.after_previous.unwrap_or(cur.after_previous), match b.wait { Some(w) => check_wait(w)?, None => cur.wait })
    .execute(&state.db)
    .await?;
    Ok(Json(load_robot(&state, id).await?))
}

#[derive(Deserialize)]
struct RobotOrder {
    stage_id: Uuid,
    ids: Vec<Uuid>,
}

/// Sets the order robots of a stage run in (and so what "the previous robot" is).
async fn order_robots(State(state): State<AppState>, auth: AuthUser, Json(b): Json<RobotOrder>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    if b.ids.len() > 500 {
        return Err(AppError::bad("too many robots"));
    }
    for (i, id) in b.ids.iter().enumerate() {
        sqlx::query!("UPDATE crm_robots SET sort = $1 WHERE id = $2 AND stage_id = $3", (i as i32 + 1) * 10, id, b.stage_id).execute(&state.db).await?;
    }
    Ok(Json(json!({ "ok": true })))
}

async fn delete_robot(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    // Scheduled runs are skipped when they find the robot gone.
    let n = sqlx::query!("DELETE FROM crm_robots WHERE id = $1", id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("robot"));
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct TriggerBody {
    category_id: Option<Uuid>,
    stage_id: Option<Uuid>,
    name: Option<String>,
    event: Option<String>,
    params: Option<Value>,
    conditions: Option<Value>,
    allow_backwards: Option<bool>,
    is_active: Option<bool>,
}

async fn load_trigger(state: &AppState, id: Uuid) -> AppResult<Trigger> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {TRIGGER_COLUMNS} FROM crm_triggers WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("trigger"))
}

fn check_trigger_params(event: &str, p: Value) -> AppResult<Value> {
    let p = match p {
        Value::Object(m) => m,
        Value::Null => Map::new(),
        _ => return Err(AppError::bad("params must be an object")),
    };
    let mut out = Map::new();
    match event {
        "activity_done" => {
            if let Some(k) = p.get("kind").and_then(Value::as_str).filter(|k| !k.is_empty()) {
                if !["todo", "call", "meeting", "email"].contains(&k) {
                    return Err(AppError::bad("kind must be todo, call, meeting or email"));
                }
                out.insert("kind".into(), json!(k));
            }
        }
        "document_created" => {
            if let Some(t) = p.get("template_id").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                Uuid::parse_str(t).map_err(|_| AppError::bad("template_id must be a template"))?;
                out.insert("template_id".into(), json!(t));
            }
        }
        "webform_submitted" => {
            if let Some(t) = p.get("form_id").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                Uuid::parse_str(t).map_err(|_| AppError::bad("form_id must be a web form"))?;
                out.insert("form_id".into(), json!(t));
            }
        }
        "signature_completed" | "signature_declined" => {
            if let Some(t) = p.get("sign_template_id").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                Uuid::parse_str(t).map_err(|_| AppError::bad("sign_template_id must be a signing template"))?;
                out.insert("sign_template_id".into(), json!(t));
            }
        }
        // A booking of the record's client: made, confirmed, came, didn't come, cancelled
        // (any of them when no status is given).
        "booking" => {
            if let Some(st) = p.get("status").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                if !["booked", "confirmed", "came", "no_show", "cancelled"].contains(&st) {
                    return Err(AppError::bad("status must be booked, confirmed, came, no_show or cancelled"));
                }
                out.insert("status".into(), json!(st));
            }
        }
        // Every task linked to the record is done (a stage's tasks finished: move on).
        "comment_added" | "email_received" | "webhook" | "tasks_done" => {}
        _ => return Err(AppError::bad(format!("event must be one of {}", EVENTS.join(", ")))),
    }
    Ok(Value::Object(out))
}

async fn create_trigger(State(state): State<AppState>, auth: AuthUser, Json(b): Json<TriggerBody>) -> AppResult<Json<Trigger>> {
    require_admin(&auth)?;
    let category = b.category_id.ok_or_else(|| AppError::bad("category_id is required"))?;
    let stage = b.stage_id.ok_or_else(|| AppError::bad("stage_id is required"))?;
    let et = stage_in(&state, category, stage).await?;
    let event = b.event.unwrap_or_default();
    let params = check_trigger_params(&event, b.params.unwrap_or(Value::Null))?;
    let token = (event == "webhook").then(|| crate::auth::tokens::random_token(24));
    let id: Uuid = sqlx::query_scalar!("INSERT INTO crm_triggers (entity_type, category_id, stage_id, name, event, params, conditions, allow_backwards, token, is_active, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id", et, category, stage, required("name", b.name.as_deref().unwrap_or(&event), 200)?, &event, params, check_conditions(&b.conditions.unwrap_or_else(|| json!([])))?, b.allow_backwards.unwrap_or(false), token, b.is_active.unwrap_or(true), auth.id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(load_trigger(&state, id).await?))
}

async fn update_trigger(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<TriggerBody>) -> AppResult<Json<Trigger>> {
    require_admin(&auth)?;
    let cur = load_trigger(&state, id).await?;
    if b.event.as_deref().is_some_and(|e| e != cur.event) {
        return Err(AppError::bad("the event of a trigger cannot change; create a new trigger"));
    }
    let stage = b.stage_id.unwrap_or(cur.stage_id);
    stage_in(&state, cur.category_id, stage).await?;
    sqlx::query!("UPDATE crm_triggers SET stage_id = $2, name = $3, params = $4, conditions = $5, allow_backwards = $6, is_active = $7, updated_at = now() WHERE id = $1", id, stage, match b.name { Some(n) => required("name", &n, 200)?, None => cur.name }, match b.params { Some(p) => check_trigger_params(&cur.event, p)?, None => cur.params }, match b.conditions { Some(c) => check_conditions(&c)?, None => cur.conditions }, b.allow_backwards.unwrap_or(cur.allow_backwards), b.is_active.unwrap_or(cur.is_active))
    .execute(&state.db)
    .await?;
    Ok(Json(load_trigger(&state, id).await?))
}

async fn delete_trigger(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let n = sqlx::query!("DELETE FROM crm_triggers WHERE id = $1", id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("trigger"));
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Serialize, FromRow)]
struct Run {
    id: Uuid,
    robot_id: Option<Uuid>,
    robot_name: String,
    status: String,
    run_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    result: Option<String>,
    wait_until: Option<DateTime<Utc>>,
    attempts: i32,
    schedule_id: Option<Uuid>,
    is_test: bool,
}

/// A record's automation log: scheduled (waiting) and recent robot runs.
async fn item_runs(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = load_row(&state, item, false).await?;
    super::access::require_read(&state, &auth, row.entity_type, item, row.assigned_to).await?;
    let runs: Vec<Run> = sqlx::query_as!(Run, r#"SELECT id, robot_id AS "robot_id?", robot_name, status, run_at, finished_at AS "finished_at?", result AS "result?", wait_until AS "wait_until?", attempts, schedule_id AS "schedule_id?", is_test FROM crm_robot_runs WHERE item_id = $1 ORDER BY created_at DESC LIMIT 100"#, item)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": runs })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders() {
        let ctx = json!({ "title": "Big deal", "uf": { "region": "EMEA" }, "amount": "€1,000.00", "tags": ["a", "b"] });
        assert_eq!(fill("{{title}} ({{ uf.region }}) {{amount}} {{missing}}!", &ctx), "Big deal (EMEA) €1,000.00 !");
        assert_eq!(fill("{{tags}} {{unclosed", &ctx), "a, b {{unclosed");
    }

    #[test]
    fn conditions() {
        let ctx = json!({ "opportunity": "1500.00", "source": "web", "comments": null, "uf": { "tier": "Gold" } });
        let c = |v: Value| conditions_hold(&v, &ctx);
        assert!(c(json!([{ "field": "opportunity", "op": "gt", "value": "999" }])));
        assert!(!c(json!([{ "field": "opportunity", "op": "gt", "value": 2000 }])));
        assert!(c(json!([{ "field": "source", "op": "eq", "value": "WEB" }, { "field": "comments", "op": "empty" }])));
        assert!(c(json!([{ "field": "uf.tier", "op": "contains", "value": "old" }])));
        assert!(!c(json!([{ "field": "uf.tier", "op": "ne", "value": "gold" }])));
        assert!(c(json!([])));
    }

    #[test]
    fn json_placeholders_stay_valid() {
        let ctx = json!({ "title": "Say \"hi\"\nnow", "n": 5 });
        let out = fill_json(r#"{"t": "{{title}}", "n": {{n}}}"#, &ctx);
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["t"], "Say \"hi\"\nnow");
        assert_eq!(v["n"], 5);
    }

    #[test]
    fn relative_dates() {
        let now = DateTime::parse_from_rfc3339("2026-11-08T15:30:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(resolve_value("today", now), "2026-11-08");
        assert_eq!(resolve_value("today+7", now), "2026-11-15");
        assert_eq!(resolve_value("Today - 8", now), "2026-10-31");
        assert_eq!(resolve_value("now", now), "2026-11-08T15:30:00");
        assert_eq!(resolve_value("todayish", now), "todayish");
        let ctx = json!({ "close_date": "2026-11-01" });
        assert!(conditions_hold(&json!([{ "field": "close_date", "op": "lt", "value": "today" }]), &ctx));
    }

    fn robot(timing: &str, field: Option<&str>, offset: i32, delay: i32) -> Robot {
        let t = Utc::now();
        Robot {
            id: Uuid::nil(), entity_type: 2, category_id: Uuid::nil(), stage_id: Uuid::nil(), name: "r".into(), action: "comment".into(),
            params: json!({}), delay_minutes: delay, conditions: json!([]), sort: 0, is_active: true, timing: timing.into(),
            timing_field: field.map(String::from), offset_minutes: offset, after_previous: false, wait: None, created_at: t, updated_at: t,
        }
    }

    #[test]
    fn due_times() {
        let now = DateTime::parse_from_rfc3339("2026-11-08T12:00:00Z").unwrap().with_timezone(&Utc);
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        let rec = json!({ "close_date": "2026-11-20", "uf": { "call_at": "2026-11-10T08:00:00Z" }, "created_at": "2026-11-01T00:00:00Z" });
        let at = |s: &str| Some(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc));
        // 3 days before the close date: 09:00 Berlin on the 17th.
        assert_eq!(due(&robot("field", Some("close_date"), -3 * 1440, 0), &rec, tz, now), at("2026-11-17T08:00:00Z"));
        // 2 hours after a datetime field.
        assert_eq!(due(&robot("field", Some("uf.call_at"), 120, 0), &rec, tz, now), at("2026-11-10T10:00:00Z"));
        // In the past: now.
        assert_eq!(due(&robot("field", Some("created_at"), 60, 0), &rec, tz, now), Some(now));
        // Empty field: never.
        assert_eq!(due(&robot("field", Some("uf.missing"), 0, 0), &rec, tz, now), None);
        assert_eq!(due(&robot("stage", None, 0, 90), &rec, tz, now), Some(now + Duration::minutes(90)));
    }
}
