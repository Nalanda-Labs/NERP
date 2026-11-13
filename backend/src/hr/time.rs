//! The work day: start, pause, resume, end; timesheets; corrections by managers.

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{holidays, manages, managed, settings, working_days};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/hr/workday", get(today))
        .route("/hr/workday/{action}", post(act))
        .route("/hr/timesheet", get(timesheet))
        .route("/hr/team", get(team))
        .route("/hr/workdays", post(add_day))
        .route("/hr/workdays/{id}", patch(edit_day))
}

pub(crate) async fn tz_of(state: &AppState, user: Uuid) -> AppResult<Tz> {
    let tz: String = sqlx::query_scalar!("SELECT timezone FROM users WHERE id = $1", user).fetch_one(&state.db).await?;
    Ok(tz.parse().unwrap_or(chrono_tz::UTC))
}

#[derive(FromRow, Serialize, Clone)]
struct Day {
    id: Uuid,
    day: NaiveDate,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    note: Option<String>,
    edit_reason: Option<String>,
    edited_by_name: Option<String>,
}

#[derive(FromRow)]
struct Break {
    day_id: Uuid,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
}

/// Minutes worked: from start to end (or now), less breaks.
fn worked(d: &Day, breaks: &[Break], now: DateTime<Utc>) -> i64 {
    let end = d.ended_at.unwrap_or(now);
    let paused: i64 = breaks.iter().filter(|b| b.day_id == d.id).map(|b| (b.ended_at.unwrap_or(end).min(end) - b.started_at).num_seconds().max(0)).sum();
    (((end - d.started_at).num_seconds() - paused).max(0)) / 60
}

async fn breaks_of(state: &AppState, ids: &[Uuid]) -> AppResult<Vec<Break>> {
    Ok(sqlx::query_as!(Break, r#"SELECT day_id, started_at, ended_at AS "ended_at?" FROM hr_work_breaks WHERE day_id = ANY($1) ORDER BY started_at"#, ids).fetch_all(&state.db).await?)
}

const DAY_SELECT: &str = "SELECT d.id, d.day, d.started_at, d.ended_at, d.note, d.edit_reason, trim(e.first_name || ' ' || e.last_name) AS edited_by_name
    FROM hr_work_days d LEFT JOIN users e ON e.id = d.edited_by";

/// A day left running from an earlier date ends at midnight of that date.
async fn close_forgotten(state: &AppState, user: Uuid, tz: Tz, today: NaiveDate) -> AppResult<()> {
    let open: Vec<(Uuid, NaiveDate)> = sqlx::query!(r#"SELECT id, day FROM hr_work_days WHERE user_id = $1 AND ended_at IS NULL AND day < $2"#, user, today).map(|r| (r.id, r.day)).fetch_all(&state.db).await?;
    for (id, day) in open {
        let midnight = tz.from_local_datetime(&(day + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap()).earliest().map(|t| t.with_timezone(&Utc)).unwrap_or_else(Utc::now);
        sqlx::query!("UPDATE hr_work_breaks SET ended_at = $2 WHERE day_id = $1 AND ended_at IS NULL", id, midnight).execute(&state.db).await?;
        sqlx::query!("UPDATE hr_work_days SET ended_at = $2, note = COALESCE(note || ' · ', '') || 'ended automatically at midnight' WHERE id = $1", id, midnight).execute(&state.db).await?;
    }
    Ok(())
}

pub(crate) async fn today_view(state: &AppState, user: Uuid) -> AppResult<Value> {
    let tz = tz_of(state, user).await?;
    let today = Utc::now().with_timezone(&tz).date_naive();
    close_forgotten(state, user, tz, today).await?;
    let d: Option<Day> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{DAY_SELECT} WHERE d.user_id = $1 AND d.day = $2"))).bind(user).bind(today).fetch_optional(&state.db).await?;
    let Some(d) = d else { return Ok(json!({ "state": "none", "day": today })) };
    let br = breaks_of(state, &[d.id]).await?;
    let paused = br.iter().any(|b| b.ended_at.is_none());
    let state_name = if d.ended_at.is_some() { "ended" } else if paused { "paused" } else { "working" };
    Ok(json!({
        "state": state_name, "day": today, "id": d.id, "started_at": d.started_at, "ended_at": d.ended_at,
        "worked_minutes": worked(&d, &br, Utc::now()), "breaks": br.len(),
        "paused_since": br.iter().find(|b| b.ended_at.is_none()).map(|b| b.started_at),
    }))
}

/// Starts the person's work day now unless today's has already started (Check-in does this).
/// Returns whether it started one.
pub(crate) async fn start_if_idle(state: &AppState, user: Uuid) -> AppResult<bool> {
    let tz = tz_of(state, user).await?;
    let now = Utc::now();
    let today = now.with_timezone(&tz).date_naive();
    close_forgotten(state, user, tz, today).await?;
    let r = sqlx::query!("INSERT INTO hr_work_days (user_id, day, started_at) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING", user, today, now)
        .execute(&state.db)
        .await?;
    Ok(r.rows_affected() > 0)
}

async fn today(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    today_view(&state, auth.id).await.map(Json)
}

async fn act(State(state): State<AppState>, auth: AuthUser, Path(action): Path<String>) -> AppResult<Json<Value>> {
    let tz = tz_of(&state, auth.id).await?;
    let now = Utc::now();
    let today = now.with_timezone(&tz).date_naive();
    close_forgotten(&state, auth.id, tz, today).await?;
    let d: Option<(Uuid, Option<DateTime<Utc>>)> = sqlx::query!(r#"SELECT id, ended_at FROM hr_work_days WHERE user_id = $1 AND day = $2"#, auth.id, today).map(|r| (r.id, r.ended_at)).fetch_optional(&state.db).await?;
    let open_break: Option<Uuid> = match d {
        Some((id, _)) => sqlx::query_scalar!("SELECT id FROM hr_work_breaks WHERE day_id = $1 AND ended_at IS NULL", id).fetch_optional(&state.db).await?,
        None => None,
    };
    match (action.as_str(), d) {
        ("start", None) => {
            sqlx::query!("INSERT INTO hr_work_days (user_id, day, started_at) VALUES ($1, $2, $3)", auth.id, today, now).execute(&state.db).await?;
        }
        // Starting again after ending: the time in between counts as a break.
        ("start", Some((id, Some(ended)))) => {
            sqlx::query!("INSERT INTO hr_work_breaks (day_id, started_at, ended_at) VALUES ($1, $2, $3)", id, ended, now).execute(&state.db).await?;
            sqlx::query!("UPDATE hr_work_days SET ended_at = NULL WHERE id = $1", id).execute(&state.db).await?;
        }
        ("start", Some(_)) => return Err(AppError::bad("your day has already started")),
        ("pause", Some((id, None))) if open_break.is_none() => {
            sqlx::query!("INSERT INTO hr_work_breaks (day_id, started_at) VALUES ($1, $2)", id, now).execute(&state.db).await?;
        }
        ("resume", Some((_, None))) if open_break.is_some() => {
            sqlx::query!("UPDATE hr_work_breaks SET ended_at = $2 WHERE id = $1", open_break, now).execute(&state.db).await?;
        }
        ("end", Some((id, None))) => {
            sqlx::query!("UPDATE hr_work_breaks SET ended_at = $2 WHERE day_id = $1 AND ended_at IS NULL", id, now).execute(&state.db).await?;
            sqlx::query!("UPDATE hr_work_days SET ended_at = $2 WHERE id = $1", id, now).execute(&state.db).await?;
        }
        ("pause" | "resume" | "end", _) => return Err(AppError::bad(format!("you can't {action} now"))),
        _ => return Err(AppError::NotFound("action")),
    }
    today_view(&state, auth.id).await.map(Json)
}

#[derive(Deserialize)]
struct SheetQuery {
    user: Option<Uuid>,
    /// YYYY-MM
    month: String,
}

fn month_range(m: &str) -> AppResult<(NaiveDate, NaiveDate)> {
    let first = NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d").map_err(|_| AppError::bad("month must look like 2026-03"))?;
    let next = if first.month() == 12 { NaiveDate::from_ymd_opt(first.year() + 1, 1, 1) } else { NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1) }.unwrap();
    Ok((first, next - Duration::days(1)))
}

/// Expected minutes in a range for a person: working days, less approved absence days.
async fn expected(state: &AppState, user: Uuid, from: NaiveDate, to: NaiveDate) -> AppResult<(i64, i64)> {
    let s = settings(state).await?;
    let hol = holidays(state, from, to).await?;
    let mut days = working_days(&s, &hol, from, to);
    let away: Vec<(NaiveDate, NaiveDate)> = sqlx::query!(r#"SELECT starts_on, ends_on FROM hr_absences WHERE user_id = $1 AND status = 'approved' AND starts_on <= $3 AND ends_on >= $2"#, user, from, to).map(|r| (r.starts_on, r.ends_on))
        .fetch_all(&state.db)
        .await?;
    let mut away_days = 0;
    for (a, b) in away {
        away_days += working_days(&s, &hol, a.max(from), b.min(to));
    }
    days -= away_days;
    let mins = (s.workday_hours.to_f64().unwrap_or(8.0) * 60.0) as i64;
    Ok((days.max(0) * mins, away_days))
}

async fn sheet(state: &AppState, user: Uuid, month: &str) -> AppResult<Value> {
    let (from, to) = month_range(month)?;
    let days: Vec<Day> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{DAY_SELECT} WHERE d.user_id = $1 AND d.day BETWEEN $2 AND $3 ORDER BY d.day")))
        .bind(user)
        .bind(from)
        .bind(to)
        .fetch_all(&state.db)
        .await?;
    let ids: Vec<Uuid> = days.iter().map(|d| d.id).collect();
    let br = breaks_of(state, &ids).await?;
    let now = Utc::now();
    let rows: Vec<Value> = days
        .iter()
        .map(|d| {
            let mut v = serde_json::to_value(d).unwrap();
            v["worked_minutes"] = json!(worked(d, &br, now));
            v["break_minutes"] = json!(br.iter().filter(|b| b.day_id == d.id).map(|b| (b.ended_at.unwrap_or(now) - b.started_at).num_minutes()).sum::<i64>());
            v
        })
        .collect();
    let total: i64 = days.iter().map(|d| worked(d, &br, now)).sum();
    let (expected, away) = expected(state, user, from, to).await?;
    let name: String = sqlx::query_scalar!(r#"SELECT trim(first_name || ' ' || last_name) AS "v!" FROM users WHERE id = $1"#, user).fetch_one(&state.db).await?;
    Ok(json!({ "user_id": user, "name": name, "month": month, "days": rows, "worked_minutes": total, "expected_minutes": expected, "absence_days": away }))
}

async fn timesheet(State(state): State<AppState>, auth: AuthUser, Query(q): Query<SheetQuery>) -> AppResult<Json<Value>> {
    let user = q.user.unwrap_or(auth.id);
    if user != auth.id && !manages(&state, &auth, user).await? {
        return Err(AppError::Forbidden);
    }
    let mut v = sheet(&state, user, &q.month).await?;
    v["can_edit"] = json!(manages(&state, &auth, user).await?);
    Ok(Json(v))
}

#[derive(Deserialize)]
struct TeamQuery {
    month: String,
}

/// Everyone the user manages, with their month in figures.
async fn team(State(state): State<AppState>, auth: AuthUser, Query(q): Query<TeamQuery>) -> AppResult<Json<Value>> {
    let (from, to) = month_range(&q.month)?;
    let people: Vec<(Uuid, String)> = match managed(&state, &auth).await? {
        None => sqlx::query_as("SELECT id, trim(first_name || ' ' || last_name) FROM users WHERE is_active ORDER BY first_name, last_name LIMIT 1000").fetch_all(&state.db).await?,
        Some(ids) => sqlx::query_as("SELECT id, trim(first_name || ' ' || last_name) FROM users WHERE id = ANY($1) ORDER BY first_name, last_name").bind(&ids).fetch_all(&state.db).await?,
    };
    let ids: Vec<Uuid> = people.iter().map(|(id, _)| *id).collect();
    let days: Vec<(Uuid, Uuid, DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query!(r#"SELECT id, user_id, started_at, ended_at FROM hr_work_days WHERE user_id = ANY($1) AND day BETWEEN $2 AND $3"#, &ids, from, to).map(|r| (r.id, r.user_id, r.started_at, r.ended_at))
        .fetch_all(&state.db)
        .await?;
    let br = breaks_of(&state, &days.iter().map(|d| d.0).collect::<Vec<_>>()).await?;
    let now = Utc::now();
    let mut out = Vec::new();
    for (id, name) in people {
        let total: i64 = days
            .iter()
            .filter(|d| d.1 == id)
            .map(|(did, _, s, e)| worked(&Day { id: *did, day: from, started_at: *s, ended_at: *e, note: None, edit_reason: None, edited_by_name: None }, &br, now))
            .sum();
        let (exp, away) = expected(&state, id, from, to).await?;
        out.push(json!({ "user_id": id, "name": name, "worked_minutes": total, "expected_minutes": exp, "days": days.iter().filter(|d| d.1 == id).count(), "absence_days": away }));
    }
    Ok(Json(json!({ "items": out, "month": q.month })))
}

#[derive(Deserialize)]
struct DayInput {
    user_id: Option<Uuid>,
    day: Option<NaiveDate>,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    reason: String,
}

fn check_times(s: DateTime<Utc>, e: Option<DateTime<Utc>>) -> AppResult<()> {
    if e.is_some_and(|e| e < s || e - s > Duration::hours(24)) {
        return Err(AppError::bad("a work day ends after it starts, within 24 hours"));
    }
    if s > Utc::now() + Duration::minutes(5) {
        return Err(AppError::bad("a work day can't start in the future"));
    }
    Ok(())
}

/// A manager records a day someone forgot to track.
async fn add_day(State(state): State<AppState>, auth: AuthUser, Json(b): Json<DayInput>) -> AppResult<Json<Value>> {
    let user = b.user_id.ok_or_else(|| AppError::bad("whose day is it?"))?;
    if !manages(&state, &auth, user).await? {
        return Err(AppError::Forbidden);
    }
    let reason = required("reason", &b.reason, 500)?;
    check_times(b.started_at, b.ended_at)?;
    let tz = tz_of(&state, user).await?;
    let day = b.day.unwrap_or_else(|| b.started_at.with_timezone(&tz).date_naive());
    sqlx::query!("INSERT INTO hr_work_days (user_id, day, started_at, ended_at, edited_by, edit_reason) VALUES ($1, $2, $3, $4, $5, $6)", user, day, b.started_at, b.ended_at, auth.id, &reason)
        .execute(&state.db)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(d) if d.is_unique_violation() => AppError::bad("that day is already recorded; correct it instead"),
            e => e.into(),
        })?;
    crate::audit::record(&state.db, Some(auth.id), "hr.workday.add", "user", Some(user.to_string()), json!({ "day": day, "reason": reason }), None).await;
    sheet(&state, user, &day.format("%Y-%m").to_string()).await.map(Json)
}

/// A manager corrects a day: its start and end (breaks stay), with a reason.
async fn edit_day(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<DayInput>) -> AppResult<Json<Value>> {
    let (user, day): (Uuid, NaiveDate) = sqlx::query!(r#"SELECT user_id, day FROM hr_work_days WHERE id = $1"#, id).map(|r| (r.user_id, r.day)).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("work day"))?;
    if !manages(&state, &auth, user).await? {
        return Err(AppError::Forbidden);
    }
    let reason = required("reason", &b.reason, 500)?;
    check_times(b.started_at, b.ended_at)?;
    sqlx::query!("UPDATE hr_work_days SET started_at = $2, ended_at = $3, edited_by = $4, edit_reason = $5 WHERE id = $1", id, b.started_at, b.ended_at, auth.id, &reason)
        .execute(&state.db)
        .await?;
    // Breaks outside the new times are dropped.
    sqlx::query!("DELETE FROM hr_work_breaks WHERE day_id = $1 AND (started_at < $2 OR ($3::timestamptz IS NOT NULL AND started_at > $3))", id, b.started_at, b.ended_at).execute(&state.db).await?;
    crate::audit::record(&state.db, Some(auth.id), "hr.workday.edit", "user", Some(user.to_string()), json!({ "day": day, "reason": reason }), None).await;
    sheet(&state, user, &day.format("%Y-%m").to_string()).await.map(Json)
}

/// Minutes someone worked on the days in [from, to] (their local dates).
pub(crate) async fn worked_minutes(state: &AppState, user: Uuid, from: NaiveDate, to: NaiveDate) -> AppResult<i64> {
    let days: Vec<Day> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{DAY_SELECT} WHERE d.user_id = $1 AND d.day BETWEEN $2 AND $3")))
        .bind(user)
        .bind(from)
        .bind(to)
        .fetch_all(&state.db)
        .await?;
    let ids: Vec<Uuid> = days.iter().map(|d| d.id).collect();
    let br = breaks_of(state, &ids).await?;
    let now = Utc::now();
    Ok(days.iter().map(|d| worked(d, &br, now)).sum())
}

/// When someone started work on each of these days.
pub(crate) async fn starts(state: &AppState, users: &[Uuid], from: NaiveDate, to: NaiveDate) -> AppResult<Vec<(Uuid, NaiveDate, DateTime<Utc>)>> {
    Ok(sqlx::query!(r#"SELECT user_id, day, started_at FROM hr_work_days WHERE user_id = ANY($1) AND day BETWEEN $2 AND $3"#, users, from, to).map(|r| (r.user_id, r.day, r.started_at)).fetch_all(&state.db).await?)
}
