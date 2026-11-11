//! HR: the work day (time tracking) and absences (time off with approval).
//!
//! Who manages whom: people with "Manage HR" manage everyone; a department head manages the
//! people in their department and its sub-departments (not themselves). Managers approve
//! time off, see timesheets and correct work days. Working days come from the company's
//! working weekdays minus its holidays.

mod absences;
mod checkin;
mod myday;
pub mod reports;
mod shifts;
mod sign;
mod time;

use axum::{
    extract::{Path, State},
    routing::{delete, get, put},
    Json, Router,
};
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/hr/settings", get(get_settings).patch(update_settings))
        .route("/hr/holidays", get(list_holidays).post(add_holiday))
        .route("/hr/holidays/{day}", delete(remove_holiday))
        .route("/hr/allowances/{user}/{year}", put(set_allowance))
        .merge(time::router())
        .merge(absences::router())
        .merge(checkin::router())
        .merge(reports::router())
        .merge(myday::router())
        .merge(shifts::router())
        .merge(sign::router())
}

pub(crate) fn hr_manager(auth: &AuthUser) -> bool {
    auth.can(perm::HR)
}

/// People the user manages: None = everyone (HR managers).
pub(crate) async fn managed(state: &AppState, auth: &AuthUser) -> AppResult<Option<Vec<Uuid>>> {
    if hr_manager(auth) {
        return Ok(None);
    }
    Ok(Some(
        sqlx::query_scalar(
            "WITH RECURSIVE mine AS (SELECT id FROM departments WHERE head_user_id = $1 OR deputy_user_id = $1
                                     UNION SELECT d.id FROM departments d JOIN mine m ON d.parent_id = m.id)
             SELECT DISTINCT ud.user_id FROM user_departments ud JOIN users u ON u.id = ud.user_id
             WHERE ud.department_id IN (SELECT id FROM mine) AND ud.user_id <> $1 AND u.is_active",
        )
        .bind(auth.id)
        .fetch_all(&state.db)
        .await?,
    ))
}

pub(crate) async fn manages(state: &AppState, auth: &AuthUser, user: Uuid) -> AppResult<bool> {
    if user == auth.id && !hr_manager(auth) {
        return Ok(false);
    }
    Ok(match managed(state, auth).await? {
        None => true,
        Some(list) => list.contains(&user),
    })
}

/// Who decides someone's time off: the nearest heads (and deputies) above them (not themselves), or HR
/// managers when there are none.
pub(crate) async fn approvers(state: &AppState, user: Uuid) -> AppResult<Vec<Uuid>> {
    // Heads above the person, with how many levels up; the nearest ones decide.
    let above: Vec<(Uuid, i32)> = sqlx::query_as(
        "WITH RECURSIVE up AS (
             SELECT d.parent_id, d.head_user_id, d.deputy_user_id, 0 AS n FROM departments d JOIN user_departments ud ON ud.department_id = d.id WHERE ud.user_id = $1
             UNION ALL SELECT p.parent_id, p.head_user_id, p.deputy_user_id, up.n + 1 FROM departments p JOIN up ON p.id = up.parent_id),
              who AS (SELECT head_user_id AS h, n FROM up UNION ALL SELECT deputy_user_id, n FROM up)
         SELECT who.h, who.n FROM who JOIN users u ON u.id = who.h WHERE who.h <> $1 AND u.is_active",
    )
    .bind(user)
    .fetch_all(&state.db)
    .await?;
    let nearest = above.iter().map(|(_, n)| *n).min();
    let mut heads: Vec<Uuid> = above.iter().filter(|(_, n)| Some(*n) == nearest).map(|(u, _)| *u).collect();
    heads.sort();
    heads.dedup();
    if !heads.is_empty() {
        return Ok(heads);
    }
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT ur.user_id FROM user_roles ur JOIN role_permissions rp ON rp.role_id = ur.role_id JOIN users u ON u.id = ur.user_id
         WHERE rp.permission IN ($1, '*') AND u.is_active AND ur.user_id <> $2",
    )
    .bind(perm::HR)
    .bind(user)
    .fetch_all(&state.db)
    .await?)
}

// ---------------------------------------------------------------------------
// Settings, holidays, allowances
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow, Clone)]
pub(crate) struct Settings {
    pub work_weekdays: Vec<i32>,
    pub workday_hours: Decimal,
    pub annual_vacation_days: Decimal,
}

pub(crate) async fn settings(state: &AppState) -> AppResult<Settings> {
    Ok(sqlx::query_as("SELECT work_weekdays, workday_hours, annual_vacation_days FROM hr_settings").fetch_one(&state.db).await?)
}

pub(crate) async fn holidays(state: &AppState, from: NaiveDate, to: NaiveDate) -> AppResult<Vec<NaiveDate>> {
    Ok(sqlx::query_scalar("SELECT day FROM hr_holidays WHERE day BETWEEN $1 AND $2").bind(from).bind(to).fetch_all(&state.db).await?)
}

/// Working days from `from` to `to`, both included.
pub(crate) fn working_days(s: &Settings, holidays: &[NaiveDate], from: NaiveDate, to: NaiveDate) -> i64 {
    from.iter_days()
        .take_while(|d| *d <= to)
        .filter(|d| s.work_weekdays.contains(&(d.weekday().number_from_monday() as i32)) && !holidays.contains(d))
        .count() as i64
}

async fn get_settings(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    let s = settings(&state).await?;
    Ok(Json(json!({ "settings": s, "can_manage": hr_manager(&_auth) })))
}

#[derive(Deserialize)]
struct SettingsInput {
    work_weekdays: Option<Vec<i32>>,
    workday_hours: Option<f64>,
    annual_vacation_days: Option<f64>,
}

async fn update_settings(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SettingsInput>) -> AppResult<Json<Value>> {
    auth.require(perm::HR)?;
    if let Some(w) = &b.work_weekdays
        && (w.is_empty() || w.iter().any(|d| !(1..=7).contains(d))) {
            return Err(AppError::bad("choose at least one working weekday"));
        }
    if b.workday_hours.is_some_and(|h| !(0.5..=24.0).contains(&h)) {
        return Err(AppError::bad("a working day is between half an hour and 24 hours"));
    }
    if b.annual_vacation_days.is_some_and(|d| !(0.0..=366.0).contains(&d)) {
        return Err(AppError::bad("vacation days must be between 0 and 366"));
    }
    let dec = |v: Option<f64>| v.map(|x| Decimal::try_from(x).unwrap_or_default().round_dp(2));
    sqlx::query(
        "UPDATE hr_settings SET work_weekdays = COALESCE($1, work_weekdays), workday_hours = COALESCE($2, workday_hours),
             annual_vacation_days = COALESCE($3, annual_vacation_days), updated_at = now()",
    )
    .bind(b.work_weekdays.map(|mut w| { w.sort(); w.dedup(); w }))
    .bind(dec(b.workday_hours))
    .bind(dec(b.annual_vacation_days))
    .execute(&state.db)
    .await?;
    get_settings(State(state), auth).await
}

async fn list_holidays(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    let rows: Vec<(NaiveDate, String)> = sqlx::query_as("SELECT day, name FROM hr_holidays WHERE day >= CURRENT_DATE - 366 ORDER BY day").fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(d, n)| json!({ "day": d, "name": n })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct HolidayInput {
    day: NaiveDate,
    name: String,
}

async fn add_holiday(State(state): State<AppState>, auth: AuthUser, Json(b): Json<HolidayInput>) -> AppResult<Json<Value>> {
    auth.require(perm::HR)?;
    let name = required("name", &b.name, 100)?;
    sqlx::query("INSERT INTO hr_holidays (day, name) VALUES ($1, $2) ON CONFLICT (day) DO UPDATE SET name = $2").bind(b.day).bind(name).execute(&state.db).await?;
    list_holidays(State(state), auth).await
}

async fn remove_holiday(State(state): State<AppState>, auth: AuthUser, Path(day): Path<NaiveDate>) -> AppResult<Json<Value>> {
    auth.require(perm::HR)?;
    sqlx::query("DELETE FROM hr_holidays WHERE day = $1").bind(day).execute(&state.db).await?;
    list_holidays(State(state), auth).await
}

#[derive(Deserialize)]
struct AllowanceInput {
    /// None: back to the company's allowance.
    days: Option<f64>,
}

async fn set_allowance(State(state): State<AppState>, auth: AuthUser, Path((user, year)): Path<(Uuid, i32)>, Json(b): Json<AllowanceInput>) -> AppResult<Json<Value>> {
    auth.require(perm::HR)?;
    match b.days {
        None => {
            sqlx::query("DELETE FROM hr_allowances WHERE user_id = $1 AND year = $2").bind(user).bind(year).execute(&state.db).await?;
        }
        Some(d) if (0.0..=366.0).contains(&d) => {
            sqlx::query("INSERT INTO hr_allowances (user_id, year, days) VALUES ($1, $2, $3) ON CONFLICT (user_id, year) DO UPDATE SET days = $3")
                .bind(user)
                .bind(year)
                .bind(Decimal::try_from(d).unwrap_or_default().round_dp(1))
                .execute(&state.db)
                .await?;
        }
        Some(_) => return Err(AppError::bad("vacation days must be between 0 and 366")),
    }
    absences::balance_of(&state, user, year).await.map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_days_skip_weekends_and_holidays() {
        let s = Settings { work_weekdays: vec![1, 2, 3, 4, 5], workday_hours: Decimal::from(8), annual_vacation_days: Decimal::from(20) };
        let d = |x: &str| NaiveDate::parse_from_str(x, "%Y-%m-%d").unwrap();
        // Mon 2 Mar – Sun 15 Mar 2026: ten weekdays.
        assert_eq!(working_days(&s, &[], d("2026-03-02"), d("2026-03-15")), 10);
        assert_eq!(working_days(&s, &[d("2026-03-04")], d("2026-03-02"), d("2026-03-15")), 9);
        assert_eq!(working_days(&s, &[], d("2026-03-07"), d("2026-03-08")), 0);
    }
}
