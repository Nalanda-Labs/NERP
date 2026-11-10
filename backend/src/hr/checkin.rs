//! Check-in: once a day each person says whether they are working today and where (office,
//! home, remotely, fieldwork or their own words), optionally posting it to a chat with a message
//! and where they are; or that they aren't working, and why. Someone who checked in as working
//! can say later that they aren't after all.
//!
//! - **The reminder** (top bar) shows on working days until the person checks in, to people who
//!   have checked in before or are in a chat check-ins were posted to; never on a holiday or
//!   approved time off. It can be muted for today, a week, or for good.
//! - **Departments:** a day's view (who works where, who isn't working and why, who hasn't checked
//!   in) and a month's (each person's days). Members of the department see it, as do its heads
//!   (and heads above) and HR managers.
//! - Checking in as working can also start the person's work day (HR → working time).
//! - HR managers turn Check-in, and sharing where people are, on or off for the company.

use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{hr_manager, managed, time::{start_if_idle, tz_of}};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::clean,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/checkin/today", get(today))
        .route("/checkin", post(check_in))
        .route("/checkin/{id}/cancel", post(cancel))
        .route("/checkin/{id}", delete(remove))
        .route("/checkin/history", get(history))
        .route("/checkin/prefs", patch(update_prefs))
        .route("/checkin/settings", get(get_settings).patch(update_settings))
        .route("/checkin/departments", get(departments))
        .route("/checkin/departments/{id}/day", get(department_day))
        .route("/checkin/departments/{id}/month", get(department_month))
}

pub const LOCATIONS: [&str; 5] = ["office", "home", "remote", "field", "custom"];
pub const REASONS: [&str; 5] = ["illness", "sick_leave", "time_off", "vacation", "custom"];

fn location_words(kind: &str, text: Option<&str>) -> String {
    match kind {
        "office" => "Working in office".into(),
        "home" => "Working from home".into(),
        "remote" => "Working remotely".into(),
        "field" => "Fieldwork".into(),
        _ => text.unwrap_or("Working").to_string(),
    }
}

fn reason_words(kind: &str, text: Option<&str>) -> String {
    match kind {
        "illness" => "I'm not feeling well".into(),
        "sick_leave" => "Sick leave".into(),
        "time_off" => "Leave of absence".into(),
        "vacation" => "Vacation".into(),
        _ => text.unwrap_or("Not working").to_string(),
    }
}

#[derive(Serialize, FromRow, Clone)]
struct CheckIn {
    id: Uuid,
    user_id: Uuid,
    day: NaiveDate,
    status: String,
    location: Option<String>,
    location_text: Option<String>,
    message: Option<String>,
    chat_id: Option<Uuid>,
    chat_title: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    address: Option<String>,
    reason: Option<String>,
    reason_text: Option<String>,
    cancelled_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

const SELECT: &str = "SELECT k.id, k.user_id, k.day, k.status, k.location, k.location_text, k.message, k.chat_id,
        CASE WHEN c.kind = 'direct' THEN (SELECT trim(u.first_name || ' ' || u.last_name) FROM im_members m JOIN users u ON u.id = m.user_id
                                         WHERE m.chat_id = c.id AND m.user_id <> k.user_id LIMIT 1) ELSE c.title END AS chat_title,
        k.latitude, k.longitude, k.address, k.reason, k.reason_text, k.cancelled_at, k.created_at
    FROM checkins k LEFT JOIN im_chats c ON c.id = k.chat_id";

#[derive(Serialize, FromRow)]
struct Prefs {
    location: String,
    location_text: Option<String>,
    message: Option<String>,
    send_message: bool,
    send_geo: bool,
    chat_id: Option<Uuid>,
    start_workday: bool,
    department_id: Option<Uuid>,
    muted_forever: bool,
    muted_until: Option<DateTime<Utc>>,
}

async fn settings(state: &AppState) -> AppResult<(bool, bool)> {
    Ok(sqlx::query_as("SELECT enabled, geo_enabled FROM checkin_settings WHERE id").fetch_one(&state.db).await?)
}

async fn today_of(state: &AppState, user: Uuid) -> AppResult<NaiveDate> {
    let tz = tz_of(state, user).await?;
    Ok(Utc::now().with_timezone(&tz).date_naive())
}

async fn load(state: &AppState, id: Uuid) -> AppResult<CheckIn> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE k.id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("check-in"))
}

/// Whether the person is due to work on a day: a company working weekday, not a holiday, and
/// not approved time off.
async fn working_day(state: &AppState, user: Uuid, day: NaiveDate) -> AppResult<bool> {
    let s = super::settings(state).await?;
    if !s.work_weekdays.contains(&(day.weekday().number_from_monday() as i32)) || !super::holidays(state, day, day).await?.is_empty() {
        return Ok(false);
    }
    let away: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM hr_absences WHERE user_id = $1 AND status = 'approved' AND $2 BETWEEN starts_on AND ends_on)")
        .bind(user)
        .bind(day)
        .fetch_one(&state.db)
        .await?;
    Ok(!away)
}

async fn today_view(state: &AppState, auth: &AuthUser) -> AppResult<Value> {
    let (enabled, geo) = settings(state).await?;
    let day = today_of(state, auth.id).await?;
    let mine: Option<CheckIn> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE k.user_id = $1 AND k.day = $2"))).bind(auth.id).bind(day).fetch_optional(&state.db).await?;
    let prefs: Option<Prefs> = sqlx::query_as(
        "SELECT location, location_text, message, send_message, send_geo, chat_id, start_workday, department_id, muted_forever, muted_until FROM checkin_prefs WHERE user_id = $1",
    )
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;
    let muted = prefs.as_ref().is_some_and(|p| p.muted_forever || p.muted_until.is_some_and(|t| t > Utc::now()));
    let due = working_day(state, auth.id, day).await?;
    let remind = enabled && mine.is_none() && prefs.is_some() && !muted && due;
    let prefs = match prefs {
        Some(p) => json!(p),
        None => json!({ "location": "office", "location_text": null, "message": null, "send_message": true, "send_geo": false, "chat_id": null,
                        "start_workday": false, "department_id": null, "muted_forever": false, "muted_until": null }),
    };
    Ok(json!({
        "enabled": enabled, "geo_enabled": geo, "can_manage": hr_manager(auth), "day": day,
        "checkin": mine, "prefs": prefs, "remind": remind, "muted": muted, "working_day": due,
    }))
}

async fn today(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    today_view(&state, &auth).await.map(Json)
}

fn limit(field: &str, v: Option<String>, max: usize) -> AppResult<Option<String>> {
    let v = clean(v);
    if v.as_ref().is_some_and(|s| s.chars().count() > max) {
        return Err(AppError::bad(format!("{field} is too long (max {max})")));
    }
    Ok(v)
}

/// Checks the person may post in the chat; returns its members.
async fn postable(state: &AppState, auth: &AuthUser, chat: Uuid) -> AppResult<Vec<Uuid>> {
    let seat = crate::im::seat(state, auth, chat).await?;
    if !seat.post {
        return Err(AppError::bad("you can't post in that chat"));
    }
    crate::im::members(state, chat).await
}

/// After a check-in is posted to a chat, its members get the reminder too (as in a team that
/// checks in together).
async fn spread(state: &AppState, members: &[Uuid]) -> AppResult<()> {
    sqlx::query("INSERT INTO checkin_prefs (user_id) SELECT u FROM unnest($1::uuid[]) AS u ON CONFLICT DO NOTHING").bind(members).execute(&state.db).await?;
    Ok(())
}

async fn changed(state: &AppState, user: Uuid, day: NaiveDate) {
    crate::realtime::publish(state, None, "checkin.changed", json!({ "user_id": user, "day": day })).await;
}

#[derive(Deserialize)]
struct CheckInInput {
    /// working | not_working
    status: String,
    location: Option<String>,
    location_text: Option<String>,
    message: Option<String>,
    /// Post it to this chat (and remember it).
    chat_id: Option<Uuid>,
    #[serde(default)]
    send_message: bool,
    latitude: Option<f64>,
    longitude: Option<f64>,
    address: Option<String>,
    reason: Option<String>,
    reason_text: Option<String>,
    /// Remember: start the work day on checking in.
    start_workday: Option<bool>,
}

async fn check_in(State(state): State<AppState>, auth: AuthUser, Json(b): Json<CheckInInput>) -> AppResult<Json<Value>> {
    let (enabled, geo_enabled) = settings(&state).await?;
    if !enabled {
        return Err(AppError::bad("Check-in is turned off"));
    }
    let day = today_of(&state, auth.id).await?;
    let message = limit("message", b.message, 1000)?;
    let working = match b.status.as_str() {
        "working" => true,
        "not_working" => false,
        _ => return Err(AppError::bad("status must be working or not_working")),
    };
    let (mut location, mut location_text, mut reason, mut reason_text) = (None, None, None, None);
    let (mut lat, mut lng, mut address) = (None, None, None);
    if working {
        let l = b.location.unwrap_or_else(|| "office".into());
        if !LOCATIONS.contains(&l.as_str()) {
            return Err(AppError::bad("unknown location"));
        }
        location_text = limit("location", b.location_text, 100)?;
        if l == "custom" && location_text.is_none() {
            return Err(AppError::bad("say where you are working"));
        }
        if l != "custom" {
            location_text = None;
        }
        location = Some(l);
        if b.latitude.is_some() || b.longitude.is_some() || b.address.is_some() {
            if !geo_enabled {
                return Err(AppError::bad("sharing where you are is turned off"));
            }
            match (b.latitude, b.longitude) {
                (Some(a), Some(o)) if (-90.0..=90.0).contains(&a) && (-180.0..=180.0).contains(&o) => (lat, lng) = (Some(a), Some(o)),
                (None, None) => {}
                _ => return Err(AppError::bad("latitude and longitude go together, within range")),
            }
            address = limit("address", b.address, 300)?;
        }
    } else {
        let r = b.reason.ok_or_else(|| AppError::bad("say why you aren't working"))?;
        if !REASONS.contains(&r.as_str()) {
            return Err(AppError::bad("unknown reason"));
        }
        reason_text = limit("reason", b.reason_text, 300)?;
        if r == "custom" && reason_text.is_none() {
            return Err(AppError::bad("say why you aren't working"));
        }
        if r != "custom" {
            reason_text = Some(reason_words(&r, None));
        }
        reason = Some(r);
    }
    let chat = if b.send_message { b.chat_id } else { None };
    let members = match chat {
        Some(c) => postable(&state, &auth, c).await?,
        None => vec![],
    };

    let id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO checkins (user_id, day, status, location, location_text, message, chat_id, latitude, longitude, address, reason, reason_text)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) ON CONFLICT (user_id, day) DO NOTHING RETURNING id",
    )
    .bind(auth.id)
    .bind(day)
    .bind(&b.status)
    .bind(&location)
    .bind(&location_text)
    .bind(&message)
    .bind(chat)
    .bind(lat)
    .bind(lng)
    .bind(&address)
    .bind(&reason)
    .bind(&reason_text)
    .fetch_optional(&state.db)
    .await?;
    let Some(id) = id else { return Err(AppError::Conflict("you have already checked in today".into())) };

    // Remember the choices for next time.
    sqlx::query(
        "INSERT INTO checkin_prefs (user_id, location, location_text, message, send_message, send_geo, chat_id, start_workday)
         VALUES ($1, COALESCE($2, 'office'), $3, $4, $5, $6, $7, COALESCE($8, FALSE))
         ON CONFLICT (user_id) DO UPDATE SET
            location = COALESCE($2, checkin_prefs.location),
            location_text = CASE WHEN $2 IS NULL THEN checkin_prefs.location_text ELSE $3 END,
            message = CASE WHEN $9 THEN $4 ELSE checkin_prefs.message END,
            send_message = $5, send_geo = CASE WHEN $9 THEN $6 ELSE checkin_prefs.send_geo END,
            chat_id = COALESCE($7, checkin_prefs.chat_id),
            start_workday = COALESCE($8, checkin_prefs.start_workday), updated_at = now()",
    )
    .bind(auth.id)
    .bind(&location)
    .bind(&location_text)
    .bind(&message)
    .bind(b.send_message)
    .bind(lat.is_some())
    .bind(b.chat_id.filter(|_| b.send_message))
    .bind(b.start_workday)
    .bind(working)
    .execute(&state.db)
    .await?;

    if let Some(c) = chat {
        let mut body = if working {
            format!("✅ {} today", location_words(location.as_deref().unwrap_or("office"), location_text.as_deref()))
        } else {
            format!("🚫 Not working today: {}", reason_text.as_deref().unwrap_or_default())
        };
        if let Some(m) = &message {
            body.push_str(&format!("\n{m}"));
        }
        if let (Some(a), Some(o)) = (lat, lng) {
            let place = address.clone().unwrap_or_else(|| format!("{a:.5}, {o:.5}"));
            body.push_str(&format!("\n📍 {place} — https://www.openstreetmap.org/?mlat={a:.6}&mlon={o:.6}#map=17/{a:.6}/{o:.6}"));
        } else if let Some(addr) = &address {
            body.push_str(&format!("\n📍 {addr}"));
        }
        crate::im::post_message(&state, c, Some(auth.id), &body, &[]).await?;
        spread(&state, &members).await?;
    }
    let started = if working && b.start_workday.unwrap_or(false) { start_if_idle(&state, auth.id).await? } else { false };
    changed(&state, auth.id, day).await;
    let mut v = today_view(&state, &auth).await?;
    v["checkin_id"] = json!(id);
    v["workday_started"] = json!(started);
    Ok(Json(v))
}

#[derive(Deserialize)]
struct CancelInput {
    reason: String,
    reason_text: Option<String>,
}

/// Checked in as working, then not working after all (today only). The reason goes to the
/// same chat.
async fn cancel(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<CancelInput>) -> AppResult<Json<Value>> {
    let k = load(&state, id).await?;
    if k.user_id != auth.id {
        return Err(AppError::NotFound("check-in"));
    }
    if k.status != "working" {
        return Err(AppError::bad("only a check-in as working can be cancelled"));
    }
    if k.day != today_of(&state, auth.id).await? {
        return Err(AppError::bad("only today's check-in can be cancelled"));
    }
    if !REASONS.contains(&b.reason.as_str()) {
        return Err(AppError::bad("unknown reason"));
    }
    let mut text = limit("reason", b.reason_text, 300)?;
    if b.reason == "custom" && text.is_none() {
        return Err(AppError::bad("say why you aren't working"));
    }
    if b.reason != "custom" {
        text = Some(reason_words(&b.reason, None));
    }
    sqlx::query("UPDATE checkins SET status = 'cancelled', reason = $2, reason_text = $3, cancelled_at = now() WHERE id = $1")
        .bind(id)
        .bind(&b.reason)
        .bind(&text)
        .execute(&state.db)
        .await?;
    if let Some(c) = k.chat_id {
        // The chat may have been left since; then it just isn't posted.
        if postable(&state, &auth, c).await.is_ok() {
            crate::im::post_message(&state, c, Some(auth.id), &format!("🚫 Not working today after all: {}", text.unwrap_or_default()), &[]).await?;
        }
    }
    changed(&state, auth.id, k.day).await;
    today_view(&state, &auth).await.map(Json)
}

/// Undo a check-in: your own today's, or anyone's for HR managers (a mistake to clear).
async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let k = load(&state, id).await?;
    let own_today = k.user_id == auth.id && k.day == today_of(&state, auth.id).await?;
    if !own_today && !hr_manager(&auth) {
        return Err(if k.user_id == auth.id { AppError::bad("only today's check-in can be undone") } else { AppError::NotFound("check-in") });
    }
    sqlx::query("DELETE FROM checkins WHERE id = $1").bind(id).execute(&state.db).await?;
    if k.user_id != auth.id {
        crate::audit::record(&state.db, Some(auth.id), "checkin.delete", "user", Some(k.user_id.to_string()), json!({ "day": k.day }), None).await;
    }
    changed(&state, k.user_id, k.day).await;
    today_view(&state, &auth).await.map(Json)
}

fn month_range(m: &str) -> AppResult<(NaiveDate, NaiveDate)> {
    let first = NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d").map_err(|_| AppError::bad("month must look like 2026-03"))?;
    let next = if first.month() == 12 { NaiveDate::from_ymd_opt(first.year() + 1, 1, 1) } else { NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1) }.unwrap();
    Ok((first, next - Duration::days(1)))
}

#[derive(Deserialize)]
struct HistoryQuery {
    user: Option<Uuid>,
    month: String,
}

/// A person's check-ins in a month: your own, or someone you manage.
async fn history(State(state): State<AppState>, auth: AuthUser, Query(q): Query<HistoryQuery>) -> AppResult<Json<Value>> {
    let user = q.user.unwrap_or(auth.id);
    if user != auth.id && !super::manages(&state, &auth, user).await? {
        return Err(AppError::Forbidden);
    }
    let (from, to) = month_range(&q.month)?;
    let items: Vec<CheckIn> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE k.user_id = $1 AND k.day BETWEEN $2 AND $3 ORDER BY k.day")))
        .bind(user)
        .bind(from)
        .bind(to)
        .fetch_all(&state.db)
        .await?;
    let working = items.iter().filter(|k| k.status == "working").count();
    Ok(Json(json!({ "items": items, "month": q.month, "working_days": working })))
}

#[derive(Deserialize)]
struct PrefsInput {
    location: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    location_text: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    message: Option<Option<String>>,
    send_message: Option<bool>,
    send_geo: Option<bool>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    chat_id: Option<Option<Uuid>>,
    start_workday: Option<bool>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    department_id: Option<Option<Uuid>>,
    /// The reminder: off (unmute) | day (until tomorrow) | week | forever
    mute: Option<String>,
}

async fn update_prefs(State(state): State<AppState>, auth: AuthUser, Json(b): Json<PrefsInput>) -> AppResult<Json<Value>> {
    if b.location.as_deref().is_some_and(|l| !LOCATIONS.contains(&l)) {
        return Err(AppError::bad("unknown location"));
    }
    let location_text = match b.location_text {
        Some(v) => Some(limit("location", v, 100)?),
        None => None,
    };
    let message = match b.message {
        Some(v) => Some(limit("message", v, 1000)?),
        None => None,
    };
    if let Some(Some(c)) = b.chat_id {
        postable(&state, &auth, c).await?;
    }
    let (forever, until): (Option<bool>, Option<Option<DateTime<Utc>>>) = match b.mute.as_deref() {
        None => (None, None),
        Some("off") => (Some(false), Some(None)),
        Some("day") => {
            // Until the start of the person's tomorrow.
            let tz = tz_of(&state, auth.id).await?;
            let tomorrow = Utc::now().with_timezone(&tz).date_naive() + Duration::days(1);
            let t = tomorrow.and_hms_opt(0, 0, 0).unwrap().and_local_timezone(tz).earliest().map(|t| t.with_timezone(&Utc)).unwrap_or_else(|| Utc::now() + Duration::days(1));
            (Some(false), Some(Some(t)))
        }
        Some("week") => (Some(false), Some(Some(Utc::now() + Duration::days(7)))),
        Some("forever") => (Some(true), Some(None)),
        Some(_) => return Err(AppError::bad("mute must be off, day, week or forever")),
    };
    sqlx::query("INSERT INTO checkin_prefs (user_id) VALUES ($1) ON CONFLICT DO NOTHING").bind(auth.id).execute(&state.db).await?;
    sqlx::query(
        "UPDATE checkin_prefs SET
            location = COALESCE($2, location),
            location_text = CASE WHEN $3 THEN $4 ELSE location_text END,
            message = CASE WHEN $5 THEN $6 ELSE message END,
            send_message = COALESCE($7, send_message), send_geo = COALESCE($8, send_geo),
            chat_id = CASE WHEN $9 THEN $10 ELSE chat_id END,
            start_workday = COALESCE($11, start_workday),
            department_id = CASE WHEN $12 THEN $13 ELSE department_id END,
            muted_forever = COALESCE($14, muted_forever),
            muted_until = CASE WHEN $15 THEN $16 ELSE muted_until END,
            updated_at = now()
         WHERE user_id = $1",
    )
    .bind(auth.id)
    .bind(&b.location)
    .bind(location_text.is_some())
    .bind(location_text.flatten())
    .bind(message.is_some())
    .bind(message.flatten())
    .bind(b.send_message)
    .bind(b.send_geo)
    .bind(b.chat_id.is_some())
    .bind(b.chat_id.flatten())
    .bind(b.start_workday)
    .bind(b.department_id.is_some())
    .bind(b.department_id.flatten())
    .bind(forever)
    .bind(until.is_some())
    .bind(until.flatten())
    .execute(&state.db)
    .await?;
    today_view(&state, &auth).await.map(Json)
}

async fn get_settings(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let (enabled, geo) = settings(&state).await?;
    Ok(Json(json!({ "enabled": enabled, "geo_enabled": geo, "can_manage": hr_manager(&auth) })))
}

#[derive(Deserialize)]
struct SettingsInput {
    enabled: Option<bool>,
    geo_enabled: Option<bool>,
}

async fn update_settings(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SettingsInput>) -> AppResult<Json<Value>> {
    auth.require(crate::auth::permissions::HR)?;
    sqlx::query("UPDATE checkin_settings SET enabled = COALESCE($1, enabled), geo_enabled = COALESCE($2, geo_enabled), updated_at = now() WHERE id")
        .bind(b.enabled)
        .bind(b.geo_enabled)
        .execute(&state.db)
        .await?;
    crate::audit::record(&state.db, Some(auth.id), "checkin.settings", "settings", None, json!({ "enabled": b.enabled, "geo_enabled": b.geo_enabled }), None).await;
    crate::realtime::publish(&state, None, "checkin.changed", json!({ "settings": true })).await;
    get_settings(State(state), auth).await
}

// ---------------------------------------------------------------------------
// Departments
// ---------------------------------------------------------------------------

/// Departments the person may look at: HR managers all; others the ones they belong to, and
/// those they head with everything below.
async fn visible_departments(state: &AppState, auth: &AuthUser) -> AppResult<Vec<(Uuid, String, Option<Uuid>)>> {
    if hr_manager(auth) {
        return Ok(sqlx::query_as("SELECT id, name, parent_id FROM departments ORDER BY name").fetch_all(&state.db).await?);
    }
    Ok(sqlx::query_as(
        "WITH RECURSIVE headed AS (SELECT id FROM departments WHERE head_user_id = $1
                                   UNION SELECT d.id FROM departments d JOIN headed h ON d.parent_id = h.id)
         SELECT id, name, parent_id FROM departments
         WHERE id IN (SELECT id FROM headed) OR id IN (SELECT department_id FROM user_departments WHERE user_id = $1)
         ORDER BY name",
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?)
}

async fn departments(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let list = visible_departments(&state, &auth).await?;
    Ok(Json(json!({ "items": list.into_iter().map(|(id, name, parent)| json!({ "id": id, "name": name, "parent_id": parent })).collect::<Vec<_>>() })))
}

/// The department's people (its members and its head), once access is checked.
async fn department_people(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<(String, Vec<(Uuid, String, Option<Uuid>, Option<String>)>)> {
    let Some((_, name, _)) = visible_departments(state, auth).await?.into_iter().find(|d| d.0 == id) else {
        return Err(AppError::NotFound("department"));
    };
    let people = sqlx::query_as(
        "SELECT u.id, trim(u.first_name || ' ' || u.last_name), u.avatar_file_id, u.position FROM users u
         WHERE u.is_active AND (u.id IN (SELECT user_id FROM user_departments WHERE department_id = $1)
                                OR u.id = (SELECT head_user_id FROM departments WHERE id = $1))
         ORDER BY u.first_name, u.last_name",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok((name, people))
}

#[derive(Deserialize)]
struct DayQuery {
    day: Option<NaiveDate>,
}

async fn department_day(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<DayQuery>) -> AppResult<Json<Value>> {
    let (name, people) = department_people(&state, &auth, id).await?;
    let day = match q.day {
        Some(d) => d,
        None => today_of(&state, auth.id).await?,
    };
    let ids: Vec<Uuid> = people.iter().map(|p| p.0).collect();
    let items: Vec<CheckIn> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE k.user_id = ANY($1) AND k.day = $2"))).bind(&ids).bind(day).fetch_all(&state.db).await?;
    let away: Vec<(Uuid, String)> = sqlx::query_as("SELECT user_id, kind FROM hr_absences WHERE user_id = ANY($1) AND status = 'approved' AND $2 BETWEEN starts_on AND ends_on")
        .bind(&ids)
        .bind(day)
        .fetch_all(&state.db)
        .await?;
    // The location is shown to the department; the exact place only to the person and managers.
    let mine = managed(&state, &auth).await?;
    let sees_place = |u: Uuid| u == auth.id || mine.as_ref().is_none_or(|m| m.contains(&u));
    let out: Vec<Value> = people
        .iter()
        .map(|(uid, n, avatar, position)| {
            let k = items.iter().find(|k| k.user_id == *uid).cloned().map(|mut k| {
                if !sees_place(*uid) {
                    (k.latitude, k.longitude, k.address) = (None, None, None);
                }
                k
            });
            json!({ "user_id": uid, "name": n, "avatar_file_id": avatar, "position": position, "checkin": k,
                    "absence": away.iter().find(|a| a.0 == *uid).map(|a| a.1.clone()) })
        })
        .collect();
    let count = |s: &str| items.iter().filter(|k| k.status == s).count();
    Ok(Json(json!({
        "department": { "id": id, "name": name }, "day": day, "people": out,
        "totals": { "people": people.len(), "working": count("working"), "not_working": count("not_working") + count("cancelled"),
                    "no_checkin": people.len() - items.len() },
    })))
}

#[derive(Deserialize)]
struct MonthQuery {
    month: String,
}

async fn department_month(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<MonthQuery>) -> AppResult<Json<Value>> {
    let (name, people) = department_people(&state, &auth, id).await?;
    let (from, to) = month_range(&q.month)?;
    let ids: Vec<Uuid> = people.iter().map(|p| p.0).collect();
    let rows: Vec<(Uuid, NaiveDate, String, Option<String>)> = sqlx::query_as("SELECT user_id, day, status, location FROM checkins WHERE user_id = ANY($1) AND day BETWEEN $2 AND $3 ORDER BY day")
        .bind(&ids)
        .bind(from)
        .bind(to)
        .fetch_all(&state.db)
        .await?;
    let s = super::settings(&state).await?;
    let hol = super::holidays(&state, from, to).await?;
    let working_days = super::working_days(&s, &hol, from, to);
    let out: Vec<Value> = people
        .iter()
        .map(|(uid, n, avatar, position)| {
            let mine: Vec<&(Uuid, NaiveDate, String, Option<String>)> = rows.iter().filter(|r| r.0 == *uid).collect();
            json!({
                "user_id": uid, "name": n, "avatar_file_id": avatar, "position": position,
                "working": mine.iter().filter(|r| r.2 == "working").count(),
                "not_working": mine.iter().filter(|r| r.2 != "working").count(),
                "days": mine.iter().map(|r| json!({ "day": r.1, "status": r.2, "location": r.3 })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Json(json!({ "department": { "id": id, "name": name }, "month": q.month, "working_days": working_days, "people": out })))
}
