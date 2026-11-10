//! Schedules: "every Monday at 09:00, for every record of this pipeline in these stages
//! that matches these conditions, do this". A recurring job starts schedules that are due;
//! each matching record gets a robot run of its own, so the work spreads over the queue
//! and shows up in the records' automation logs.

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use uuid::Uuid;

use super::{actions, check_conditions, conditions_hold, require_admin, Robot};
use crate::{
    auth::AuthUser,
    crm::items::{Row, ROW_SELECT},
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub const JOB: &str = "crm.schedules";
/// Records one schedule run handles at most.
const MAX_RECORDS: i64 = 5000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/schedules", get(list).post(create))
        .route("/crm/schedules/{id}", patch(update).delete(delete))
        .route("/crm/schedules/{id}/run", post(run_now))
}

#[derive(Serialize, FromRow, Clone)]
pub struct Schedule {
    id: Uuid,
    entity_type: i16,
    category_id: Uuid,
    name: String,
    frequency: String,
    weekdays: Vec<i16>,
    month_day: i16,
    at_time: NaiveTime,
    timezone: String,
    stage_ids: Vec<Uuid>,
    conditions: Value,
    action: String,
    params: Value,
    is_active: bool,
    next_run_at: Option<DateTime<Utc>>,
    last_run_at: Option<DateTime<Utc>>,
    last_count: Option<i32>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const COLUMNS: &str = "id, entity_type, category_id, name, frequency, weekdays, month_day, at_time, timezone, stage_ids, conditions, action, params, is_active, next_run_at, last_run_at, last_count, created_at, updated_at";

async fn load(state: &AppState, id: Uuid) -> AppResult<Schedule> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLUMNS} FROM crm_schedules WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("schedule"))
}

/// A schedule seen as a robot, so runs execute it like any other.
pub async fn as_robot(state: &AppState, id: Uuid) -> AppResult<Option<Robot>> {
    let s = match load(state, id).await {
        Ok(s) => s,
        Err(AppError::NotFound(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(Some(Robot {
        id: s.id,
        entity_type: s.entity_type,
        category_id: s.category_id,
        stage_id: Uuid::nil(),
        name: s.name,
        action: s.action,
        params: s.params,
        delay_minutes: 0,
        conditions: s.conditions,
        sort: 0,
        is_active: s.is_active,
        timing: "stage".into(),
        timing_field: None,
        offset_minutes: 0,
        after_previous: false,
        wait: None,
        created_at: s.created_at,
        updated_at: s.updated_at,
    }))
}

fn last_day(d: NaiveDate) -> u32 {
    let (y, m) = if d.month() == 12 { (d.year() + 1, 1) } else { (d.year(), d.month() + 1) };
    NaiveDate::from_ymd_opt(y, m, 1).and_then(|n| n.pred_opt()).map(|n| n.day()).unwrap_or(28)
}

/// The first time after `after` the schedule is due, in its time zone. A time that does
/// not exist on a day (a daylight-saving gap) moves an hour later.
pub fn next_run(frequency: &str, weekdays: &[i16], month_day: i16, at: NaiveTime, tz: Tz, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let start = after.with_timezone(&tz).date_naive();
    for i in 0..400 {
        let d = start + Duration::days(i);
        let matches = match frequency {
            "daily" => true,
            "weekly" => weekdays.contains(&(d.weekday().number_from_monday() as i16)),
            "monthly" => d.day() == (month_day.max(1) as u32).min(last_day(d)),
            _ => false,
        };
        if !matches {
            continue;
        }
        let local = d.and_time(at);
        let Some(t) = tz.from_local_datetime(&local).earliest().or_else(|| tz.from_local_datetime(&(local + Duration::hours(1))).earliest()) else { continue };
        let t = t.with_timezone(&Utc);
        if t > after {
            return Some(t);
        }
    }
    None
}

fn tz_of(s: &Schedule) -> Tz {
    s.timezone.parse().unwrap_or(Tz::UTC)
}

/// Fields robots read that a plain row does not hold; conditions on them are checked when
/// each run executes instead of when records are picked.
const CONTEXT_ONLY: &[&str] = &["contact", "client_email", "assigned_email", "amount", "link"];

/// Queues a run for every record the schedule applies to. Returns how many.
async fn start(state: &AppState, s: &Schedule) -> AppResult<i64> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(ROW_SELECT);
    qb.push(" AND i.deleted_at IS NULL AND i.category_id = ").push_bind(s.category_id);
    if s.stage_ids.is_empty() {
        qb.push(" AND NOT i.is_closed");
    } else {
        qb.push(" AND i.stage_id = ANY(").push_bind(s.stage_ids.clone()).push(")");
    }
    qb.push(" ORDER BY i.created_at LIMIT ").push_bind(MAX_RECORDS);
    let rows: Vec<Row> = qb.build_query_as().fetch_all(&state.db).await?;
    let pre: Vec<Value> = s
        .conditions
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| !CONTEXT_ONLY.contains(&c["field"].as_str().unwrap_or("")))
        .collect();
    let pre = Value::Array(pre);
    let mut items = Vec::new();
    let mut stages = Vec::new();
    for r in &rows {
        let mut v = serde_json::to_value(r).unwrap_or_default();
        v["stage"] = json!(r.stage_name.clone().unwrap_or_default());
        v["assigned"] = json!(r.assigned_name.clone().unwrap_or_default());
        v["company"] = json!(r.company_title.clone().unwrap_or_default());
        if conditions_hold(&pre, &v) {
            items.push(r.id);
            stages.push(r.stage_id.unwrap_or(Uuid::nil()));
        }
    }
    if items.is_empty() {
        return Ok(0);
    }
    let mut tx = state.db.begin().await?;
    let runs: Vec<Uuid> = sqlx::query_scalar(
        "INSERT INTO crm_robot_runs (schedule_id, robot_name, item_id, stage_id, category_id, action, run_at)
         SELECT $1, $2, x.item, x.stage, $3, $4, now() FROM unnest($5::uuid[], $6::uuid[]) AS x(item, stage) RETURNING id",
    )
    .bind(s.id)
    .bind(&s.name)
    .bind(s.category_id)
    .bind(&s.action)
    .bind(&items)
    .bind(&stages)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO jobs (kind, payload, run_at) SELECT $1, jsonb_build_object('run_id', r), now() FROM unnest($2::uuid[]) AS r")
        .bind(super::JOB)
        .bind(&runs)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    state.job_wakeup.notify_one();
    Ok(runs.len() as i64)
}

/// Recurring job: starts every schedule that is due.
pub async fn run_due(state: &AppState) -> anyhow::Result<()> {
    let ae = |e: AppError| anyhow::anyhow!("{e:?}");
    let due: Vec<Schedule> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM crm_schedules WHERE is_active AND next_run_at <= now() ORDER BY next_run_at LIMIT 20"
    )))
    .fetch_all(&state.db)
    .await?;
    for s in due {
        let next = next_run(&s.frequency, &s.weekdays, s.month_day, s.at_time, tz_of(&s), Utc::now());
        // Claim this occurrence, so only one instance starts it.
        let claimed = sqlx::query("UPDATE crm_schedules SET next_run_at = $2, last_run_at = now() WHERE id = $1 AND next_run_at = $3")
            .bind(s.id)
            .bind(next)
            .bind(s.next_run_at)
            .execute(&state.db)
            .await?
            .rows_affected()
            == 1;
        if !claimed {
            continue;
        }
        match start(state, &s).await {
            Ok(n) => {
                sqlx::query("UPDATE crm_schedules SET last_count = $2 WHERE id = $1").bind(s.id).bind(n as i32).execute(&state.db).await?;
            }
            Err(e) => tracing::warn!(schedule = %s.id, error = ?ae(e), "schedule failed to start"),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// API (crm.admin)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ListQuery {
    category_id: Uuid,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let items: Vec<Schedule> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLUMNS} FROM crm_schedules WHERE category_id = $1 ORDER BY name, created_at")))
        .bind(q.category_id)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
struct Body {
    category_id: Option<Uuid>,
    name: Option<String>,
    frequency: Option<String>,
    weekdays: Option<Vec<i16>>,
    month_day: Option<i16>,
    /// "HH:MM"
    at_time: Option<String>,
    timezone: Option<String>,
    stage_ids: Option<Vec<Uuid>>,
    conditions: Option<Value>,
    action: Option<String>,
    params: Option<Value>,
    is_active: Option<bool>,
}

struct Checked {
    frequency: String,
    weekdays: Vec<i16>,
    month_day: i16,
    at_time: NaiveTime,
    timezone: String,
    stage_ids: Vec<Uuid>,
}

async fn check(state: &AppState, category: Uuid, b: &Body, cur: Option<&Schedule>) -> AppResult<Checked> {
    let frequency = b.frequency.clone().or(cur.map(|c| c.frequency.clone())).unwrap_or_else(|| "daily".into());
    if !["daily", "weekly", "monthly"].contains(&frequency.as_str()) {
        return Err(AppError::bad("frequency must be daily, weekly or monthly"));
    }
    let mut weekdays = b.weekdays.clone().or(cur.map(|c| c.weekdays.clone())).unwrap_or_default();
    weekdays.sort_unstable();
    weekdays.dedup();
    if weekdays.iter().any(|d| !(1..=7).contains(d)) {
        return Err(AppError::bad("weekdays go from 1 (Monday) to 7 (Sunday)"));
    }
    if frequency == "weekly" && weekdays.is_empty() {
        return Err(AppError::bad("choose at least one day of the week"));
    }
    let month_day = b.month_day.or(cur.map(|c| c.month_day)).unwrap_or(1);
    if !(1..=31).contains(&month_day) {
        return Err(AppError::bad("the day of the month must be between 1 and 31"));
    }
    let at_time = match &b.at_time {
        Some(t) => NaiveTime::parse_from_str(t.trim(), "%H:%M").map_err(|_| AppError::bad("the time must look like 09:00"))?,
        None => cur.map(|c| c.at_time).unwrap_or(NaiveTime::from_hms_opt(9, 0, 0).unwrap()),
    };
    let timezone = b.timezone.clone().or(cur.map(|c| c.timezone.clone())).unwrap_or_else(|| "UTC".into());
    timezone.parse::<Tz>().map_err(|_| AppError::bad("unknown time zone"))?;
    let stage_ids = b.stage_ids.clone().or(cur.map(|c| c.stage_ids.clone())).unwrap_or_default();
    if stage_ids.len() > 100 {
        return Err(AppError::bad("too many stages"));
    }
    if !stage_ids.is_empty() {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crm_stages WHERE category_id = $1 AND id = ANY($2)").bind(category).bind(&stage_ids).fetch_one(&state.db).await?;
        if n != stage_ids.len() as i64 {
            return Err(AppError::bad("choose stages of this pipeline"));
        }
    }
    Ok(Checked { frequency, weekdays, month_day, at_time, timezone, stage_ids })
}

fn next_of(c: &Checked, active: bool) -> Option<DateTime<Utc>> {
    if !active {
        return None;
    }
    next_run(&c.frequency, &c.weekdays, c.month_day, c.at_time, c.timezone.parse().unwrap_or(Tz::UTC), Utc::now())
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(mut b): Json<Body>) -> AppResult<Json<Schedule>> {
    require_admin(&auth)?;
    let category = b.category_id.ok_or_else(|| AppError::bad("category_id is required"))?;
    let et: i16 = sqlx::query_scalar("SELECT c.entity_type FROM crm_categories c JOIN crm_entity_types t ON t.id = c.entity_type WHERE c.id = $1 AND t.has_stages")
        .bind(category)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::bad("choose a pipeline"))?;
    if b.timezone.is_none() {
        b.timezone = sqlx::query_scalar("SELECT timezone FROM users WHERE id = $1").bind(auth.id).fetch_optional(&state.db).await?.flatten();
    }
    let c = check(&state, category, &b, None).await?;
    let action = b.action.clone().unwrap_or_default();
    let params = actions::validate(&state, et, category, &action, b.params.clone().unwrap_or_else(|| json!({}))).await?;
    let active = b.is_active.unwrap_or(true);
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO crm_schedules (entity_type, category_id, name, frequency, weekdays, month_day, at_time, timezone, stage_ids, conditions, action, params, is_active, next_run_at, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15) RETURNING id",
    )
    .bind(et)
    .bind(category)
    .bind(required("name", b.name.as_deref().unwrap_or(actions::label(&action)), 200)?)
    .bind(&c.frequency)
    .bind(&c.weekdays)
    .bind(c.month_day)
    .bind(c.at_time)
    .bind(&c.timezone)
    .bind(&c.stage_ids)
    .bind(check_conditions(&b.conditions.clone().unwrap_or_else(|| json!([])))?)
    .bind(&action)
    .bind(params)
    .bind(active)
    .bind(next_of(&c, active))
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(load(&state, id).await?))
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Body>) -> AppResult<Json<Schedule>> {
    require_admin(&auth)?;
    let cur = load(&state, id).await?;
    let c = check(&state, cur.category_id, &b, Some(&cur)).await?;
    let action = b.action.clone().unwrap_or(cur.action.clone());
    let params = actions::validate(&state, cur.entity_type, cur.category_id, &action, b.params.clone().unwrap_or(cur.params.clone())).await?;
    let active = b.is_active.unwrap_or(cur.is_active);
    sqlx::query(
        "UPDATE crm_schedules SET name = $2, frequency = $3, weekdays = $4, month_day = $5, at_time = $6, timezone = $7, stage_ids = $8,
                conditions = $9, action = $10, params = $11, is_active = $12, next_run_at = $13, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(match &b.name { Some(n) => required("name", n, 200)?, None => cur.name.clone() })
    .bind(&c.frequency)
    .bind(&c.weekdays)
    .bind(c.month_day)
    .bind(c.at_time)
    .bind(&c.timezone)
    .bind(&c.stage_ids)
    .bind(match &b.conditions { Some(x) => check_conditions(x)?, None => cur.conditions.clone() })
    .bind(&action)
    .bind(params)
    .bind(active)
    .bind(next_of(&c, active))
    .execute(&state.db)
    .await?;
    Ok(Json(load(&state, id).await?))
}

async fn delete(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let n = sqlx::query("DELETE FROM crm_schedules WHERE id = $1").bind(id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("schedule"));
    }
    Ok(Json(json!({ "ok": true })))
}

/// Runs a schedule now, without changing when it runs next.
async fn run_now(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let s = load(&state, id).await?;
    let n = start(&state, &s).await?;
    sqlx::query("UPDATE crm_schedules SET last_run_at = now(), last_count = $2 WHERE id = $1").bind(id).bind(n as i32).execute(&state.db).await?;
    Ok(Json(json!({ "count": n })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }
    fn nine() -> NaiveTime {
        NaiveTime::from_hms_opt(9, 0, 0).unwrap()
    }

    #[test]
    fn daily_and_weekly() {
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        // Sunday 2026-11-08 10:00 Berlin (09:00 UTC): today's 09:00 has passed.
        let now = utc("2026-11-08T09:00:00Z");
        assert_eq!(next_run("daily", &[], 1, nine(), tz, now), Some(utc("2026-11-09T08:00:00Z")));
        // Mondays and Thursdays.
        assert_eq!(next_run("weekly", &[1, 4], 1, nine(), tz, now), Some(utc("2026-11-09T08:00:00Z")));
        assert_eq!(next_run("weekly", &[4], 1, nine(), tz, now), Some(utc("2026-11-12T08:00:00Z")));
        // Before today's time: today.
        assert_eq!(next_run("daily", &[], 1, nine(), tz, utc("2026-11-08T07:00:00Z")), Some(utc("2026-11-08T08:00:00Z")));
    }

    #[test]
    fn monthly_clamps_to_the_last_day() {
        let tz = Tz::UTC;
        assert_eq!(next_run("monthly", &[], 31, nine(), tz, utc("2026-11-08T00:00:00Z")), Some(utc("2026-11-30T09:00:00Z")));
        assert_eq!(next_run("monthly", &[], 31, nine(), tz, utc("2027-02-01T00:00:00Z")), Some(utc("2027-02-28T09:00:00Z")));
        assert_eq!(next_run("monthly", &[], 15, nine(), tz, utc("2026-11-15T09:00:00Z")), Some(utc("2026-12-15T09:00:00Z")));
    }

    #[test]
    fn daylight_saving_gap_moves_later() {
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        // 2027-03-28 02:30 does not exist in Berlin; it runs at 03:30 local (01:30 UTC).
        let t = NaiveTime::from_hms_opt(2, 30, 0).unwrap();
        assert_eq!(next_run("daily", &[], 1, t, tz, utc("2027-03-27T12:00:00Z")), Some(utc("2027-03-28T01:30:00Z")));
    }
}
