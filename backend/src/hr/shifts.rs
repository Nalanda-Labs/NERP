//! Shift schedules: HR sets up shifts (name, hours, break); managers put their people on
//! shifts day by day (the rota), or fill a range of days at once (e.g. weekdays); the rota
//! shows for each day who works which shift and when they actually started (late starts are
//! marked).

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch},
    Json, Router,
};
use chrono::{Datelike, Duration, NaiveDate, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{hr_manager, managed, manages, time::tz_of};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::required,
    state::AppState,
};

/// Minutes after the shift's start before a start counts as late.
const GRACE_MINUTES: i64 = 5;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/hr/shifts", get(list).post(create))
        .route("/hr/shifts/{id}", patch(update).delete(remove))
        .route("/hr/rota", get(rota).put(assign))
}

#[derive(sqlx::FromRow, serde::Serialize)]
struct Shift {
    id: Uuid,
    name: String,
    start_min: i32,
    end_min: i32,
    break_minutes: i32,
    color: String,
    is_active: bool,
}

async fn shifts(state: &AppState) -> AppResult<Vec<Shift>> {
    Ok(sqlx::query_as!(Shift, "SELECT id, name, start_min, end_min, break_minutes, color, is_active FROM hr_shifts ORDER BY start_min, name").fetch_all(&state.db).await?)
}

async fn list(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "items": shifts(&state).await? })))
}

#[derive(Deserialize)]
struct ShiftInput {
    name: Option<String>,
    start_min: Option<i32>,
    end_min: Option<i32>,
    break_minutes: Option<i32>,
    color: Option<String>,
    is_active: Option<bool>,
}

fn check(s: i32, e: i32, b: i32) -> AppResult<()> {
    if !(0..1440).contains(&s) || !(1..=2880).contains(&e) || e <= s || e - s > 1440 || !(0..=480).contains(&b) || b >= e - s {
        return Err(AppError::bad("a shift starts within the day, lasts up to 24 hours (ending after midnight is fine), and its break is shorter than it"));
    }
    Ok(())
}

fn colour(c: &str) -> AppResult<String> {
    if c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit()) { Ok(c.to_lowercase()) } else { Err(AppError::bad("colour must look like #1a73e8")) }
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<ShiftInput>) -> AppResult<Json<Value>> {
    if !hr_manager(&auth) {
        return Err(AppError::Forbidden);
    }
    let name = required("name", b.name.as_deref().unwrap_or(""), 100)?;
    let (s, e, br) = (b.start_min.unwrap_or(540), b.end_min.unwrap_or(1020), b.break_minutes.unwrap_or(0));
    check(s, e, br)?;
    sqlx::query!("INSERT INTO hr_shifts (name, start_min, end_min, break_minutes, color) VALUES ($1, $2, $3, $4, $5)", name, s, e, br, b.color.as_deref().map(colour).transpose()?.unwrap_or_else(|| "#1a73e8".into()))
        .execute(&state.db)
        .await?;
    list(State(state), auth).await
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ShiftInput>) -> AppResult<Json<Value>> {
    if !hr_manager(&auth) {
        return Err(AppError::Forbidden);
    }
    let cur: Option<(i32, i32, i32)> = sqlx::query!(r#"SELECT start_min, end_min, break_minutes FROM hr_shifts WHERE id = $1"#, id).map(|r| (r.start_min, r.end_min, r.break_minutes)).fetch_optional(&state.db).await?;
    let (s0, e0, b0) = cur.ok_or(AppError::NotFound("shift"))?;
    check(b.start_min.unwrap_or(s0), b.end_min.unwrap_or(e0), b.break_minutes.unwrap_or(b0))?;
    let name = b.name.as_deref().map(|n| required("name", n, 100)).transpose()?;
    sqlx::query!("UPDATE hr_shifts SET name = COALESCE($2, name), start_min = COALESCE($3, start_min), end_min = COALESCE($4, end_min),
             break_minutes = COALESCE($5, break_minutes), color = COALESCE($6, color), is_active = COALESCE($7, is_active) WHERE id = $1", id, name, b.start_min, b.end_min, b.break_minutes, b.color.as_deref().map(colour).transpose()?, b.is_active)
    .execute(&state.db)
    .await?;
    list(State(state), auth).await
}

/// Deleting a shift takes it off the rota from today on; past days keep it only if it's
/// switched off instead.
async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    if !hr_manager(&auth) {
        return Err(AppError::Forbidden);
    }
    sqlx::query!("DELETE FROM hr_shifts WHERE id = $1", id).execute(&state.db).await?;
    list(State(state), auth).await
}

#[derive(Deserialize)]
struct RotaQuery {
    from: NaiveDate,
    to: NaiveDate,
    department_id: Option<Uuid>,
}

/// Who works which shift each day, with when they actually started.
async fn rota(State(state): State<AppState>, auth: AuthUser, Query(q): Query<RotaQuery>) -> AppResult<Json<Value>> {
    if q.to < q.from || (q.to - q.from).num_days() > 62 {
        return Err(AppError::bad("show at most two months"));
    }
    // People: a department's, else those I manage (and me).
    let mut people: Vec<Uuid> = match q.department_id {
        Some(d) => sqlx::query_scalar!("SELECT ud.user_id FROM user_departments ud JOIN users u ON u.id = ud.user_id WHERE ud.department_id = $1 AND u.is_active", d).fetch_all(&state.db).await?,
        None => match managed(&state, &auth).await? {
            Some(list) => list,
            None => sqlx::query_scalar!("SELECT id FROM users WHERE is_active").fetch_all(&state.db).await?,
        },
    };
    if !people.contains(&auth.id) && q.department_id.is_none() {
        people.push(auth.id);
    }
    // Only people I may see the rota of: myself, people I manage, or everyone for HR.
    let mut visible = Vec::new();
    for p in people {
        if p == auth.id || hr_manager(&auth) || manages(&state, &auth, p).await? {
            visible.push(p);
        }
    }
    let names: Vec<(Uuid, String)> = sqlx::query!(r#"SELECT id AS "id!", trim(first_name || ' ' || last_name) AS "c1!" FROM users WHERE id = ANY($1) ORDER BY 2"#, &visible).map(|r| (r.id, r.c1)).fetch_all(&state.db).await?;
    let entries: Vec<(Uuid, NaiveDate, Uuid, i32)> = sqlx::query!(r#"SELECT r.user_id, r.day, r.shift_id, s.start_min FROM hr_rota r JOIN hr_shifts s ON s.id = r.shift_id WHERE r.user_id = ANY($1) AND r.day BETWEEN $2 AND $3"#, &visible, q.from, q.to).map(|r| (r.user_id, r.day, r.shift_id, r.start_min))
        .fetch_all(&state.db)
        .await?;
    let starts = super::time::starts(&state, &visible, q.from, q.to).await?;
    let mut cells = Vec::new();
    for (u, day, shift, start_min) in &entries {
        let tz = tz_of(&state, *u).await?;
        let planned = tz.from_local_datetime(&day.and_hms_opt((start_min / 60) as u32, (start_min % 60) as u32, 0).unwrap()).earliest().map(|t| t.with_timezone(&Utc));
        let actual = starts.iter().find(|(x, d, _)| x == u && d == day).map(|(_, _, t)| *t);
        let late = match (planned, actual) {
            (Some(p), Some(a)) => (a - p).num_minutes() > GRACE_MINUTES,
            _ => false,
        };
        cells.push(json!({ "user_id": u, "day": day, "shift_id": shift, "started_at": actual, "late": late, "late_minutes": planned.zip(actual).map(|(p, a)| (a - p).num_minutes().max(0)) }));
    }
    Ok(Json(json!({
        "shifts": shifts(&state).await?,
        "people": names.into_iter().map(|(id, n)| json!({ "id": id, "name": n, "can_edit": hr_manager(&auth) || (id != auth.id) })).collect::<Vec<_>>(),
        "cells": cells,
    })))
}

#[derive(Deserialize)]
struct Entry {
    user_id: Uuid,
    day: Option<NaiveDate>,
    /// Or a range of days, optionally only some weekdays (1 = Monday).
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    #[serde(default)]
    weekdays: Vec<u32>,
    /// None takes the person off the rota those days.
    shift_id: Option<Uuid>,
}

#[derive(Deserialize)]
struct AssignInput {
    entries: Vec<Entry>,
}

/// Puts people on shifts (HR, or the people's managers); they're told about their changes.
async fn assign(State(state): State<AppState>, auth: AuthUser, Json(b): Json<AssignInput>) -> AppResult<Json<Value>> {
    if b.entries.len() > 500 {
        return Err(AppError::bad("at most 500 changes at once"));
    }
    let mut tx = state.db.begin().await?;
    let mut told: Vec<Uuid> = Vec::new();
    for e in &b.entries {
        if !manages(&state, &auth, e.user_id).await? {
            return Err(AppError::Forbidden);
        }
        let days: Vec<NaiveDate> = match (e.day, e.from, e.to) {
            (Some(d), _, _) => vec![d],
            (None, Some(f), Some(t)) if t >= f && (t - f).num_days() <= 366 => {
                let mut v = Vec::new();
                let mut d = f;
                while d <= t {
                    if e.weekdays.is_empty() || e.weekdays.contains(&d.weekday().number_from_monday()) {
                        v.push(d);
                    }
                    d += Duration::days(1);
                }
                v
            }
            _ => return Err(AppError::bad("give a day, or a range of up to a year")),
        };
        for d in days {
            match e.shift_id {
                Some(s) => {
                    sqlx::query!("INSERT INTO hr_rota (user_id, day, shift_id) VALUES ($1, $2, $3) ON CONFLICT (user_id, day) DO UPDATE SET shift_id = $3", e.user_id, d, s)
                        .execute(&mut *tx)
                        .await
                        .map_err(|_| AppError::bad("no such shift"))?;
                }
                None => {
                    sqlx::query!("DELETE FROM hr_rota WHERE user_id = $1 AND day = $2", e.user_id, d).execute(&mut *tx).await?;
                }
            }
        }
        if !told.contains(&e.user_id) {
            told.push(e.user_id);
        }
    }
    tx.commit().await?;
    let me: String = sqlx::query_scalar!("SELECT first_name FROM users WHERE id = $1", auth.id).fetch_one(&state.db).await?;
    notify(&state, &told, NewNotification { kind: "hr.rota", title: format!("{me} changed your shifts"), body: None, link: Some("/hr/rota".into()), actor: Some(auth.id) }).await;
    Ok(Json(json!({ "ok": true, "changed": b.entries.len() })))
}
