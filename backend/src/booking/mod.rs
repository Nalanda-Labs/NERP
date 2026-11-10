//! Booking: appointments with the company's resources.
//!
//! - **Resources** are people (an employee: their bookings go on their calendar), rooms,
//!   equipment or anything else, each with working hours per weekday in its time zone, days
//!   off, a capacity (how many bookings may overlap), and the rules clients book by: slot
//!   length, a buffer after each booking, minimum notice and how far ahead.
//! - **Services** have a duration (minutes, up to whole days for long bookings) and a price,
//!   and may be limited to some resources; **extras** are added to a booking.
//! - A **booking** takes one or more resources at once, has a client (who becomes a CRM
//!   contact) and moves booked → confirmed → came or no-show, or is cancelled. Staff can book
//!   over capacity where a resource allows it (overbooking). When nothing is free, a client can
//!   join the **waiting list**.
//! - **Forms** at `/book/<slug>` let clients book themselves; they get a link to see, confirm
//!   or cancel, a confirmation by email and a reminder before the visit.
//!
//! Everyone sees and makes bookings; "Manage booking" sets up resources, services and forms.

mod bookings;
mod public;

use anyhow::Result;
use axum::{
    extract::{Path, State},
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

pub use bookings::REMIND_JOB;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/booking/settings", get(get_settings).put(put_settings))
        .route("/booking/resources", get(list_resources).post(create_resource))
        .route("/booking/resources/{id}", get(get_resource).patch(update_resource).delete(delete_resource))
        .route("/booking/resources/{id}/hours", put(set_hours))
        .route("/booking/resources/{id}/days-off", put(set_days_off))
        .route("/booking/services", get(list_services).post(create_service))
        .route("/booking/services/{id}", axum::routing::patch(update_service).delete(delete_service))
        .merge(bookings::router())
        .merge(public::router())
}

pub async fn run_reminders(state: &AppState) -> Result<()> {
    bookings::run_reminders(state).await
}

fn manage(auth: &AuthUser) -> AppResult<()> {
    auth.require(perm::BOOKING)
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

async fn settings_json(state: &AppState) -> AppResult<Value> {
    let (confirm, remind, contact, cat, cur): (bool, i32, bool, Option<Uuid>, String) =
        sqlx::query_as("SELECT confirm_email, remind_hours, make_contact, deal_category_id, currency FROM booking_settings WHERE id").fetch_one(&state.db).await?;
    Ok(json!({ "confirm_email": confirm, "remind_hours": remind, "make_contact": contact, "deal_category_id": cat, "currency": cur }))
}

async fn get_settings(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    Ok(Json(settings_json(&state).await?))
}

#[derive(Deserialize)]
struct SettingsInput {
    confirm_email: Option<bool>,
    remind_hours: Option<i32>,
    make_contact: Option<bool>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    deal_category_id: Option<Option<Uuid>>,
    currency: Option<String>,
}

async fn put_settings(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SettingsInput>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    if b.remind_hours.is_some_and(|h| !(0..=168).contains(&h)) {
        return Err(AppError::bad("remind 0 to 168 hours before"));
    }
    if let Some(Some(c)) = b.deal_category_id {
        let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_categories WHERE id = $1 AND entity_type = 2)").bind(c).fetch_one(&state.db).await?;
        if !ok {
            return Err(AppError::bad("choose a deal pipeline"));
        }
    }
    if let Some(c) = &b.currency {
        if c.len() != 3 || !c.chars().all(|x| x.is_ascii_uppercase()) {
            return Err(AppError::bad("currency is a three-letter code"));
        }
    }
    sqlx::query(
        "UPDATE booking_settings SET confirm_email = COALESCE($1, confirm_email), remind_hours = COALESCE($2, remind_hours),
            make_contact = COALESCE($3, make_contact), deal_category_id = CASE WHEN $4 THEN $5 ELSE deal_category_id END,
            currency = COALESCE($6, currency), updated_at = now() WHERE id",
    )
    .bind(b.confirm_email)
    .bind(b.remind_hours)
    .bind(b.make_contact)
    .bind(b.deal_category_id.is_some())
    .bind(b.deal_category_id.flatten())
    .bind(b.currency)
    .execute(&state.db)
    .await?;
    Ok(Json(settings_json(&state).await?))
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow, Clone)]
pub(crate) struct Resource {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub description: Option<String>,
    pub user_id: Option<Uuid>,
    pub user_name: Option<String>,
    pub capacity: i32,
    pub allow_overbooking: bool,
    pub color: String,
    pub timezone: String,
    pub slot_minutes: i32,
    pub buffer_minutes: i32,
    pub min_notice_minutes: i32,
    pub max_days_ahead: i32,
    pub is_active: bool,
    pub sort: i32,
}

pub(crate) const RESOURCE_SELECT: &str = "SELECT r.id, r.name, r.kind, r.description, r.user_id, trim(u.first_name || ' ' || u.last_name) AS user_name,
    r.capacity, r.allow_overbooking, r.color, r.timezone, r.slot_minutes, r.buffer_minutes, r.min_notice_minutes, r.max_days_ahead, r.is_active, r.sort
    FROM booking_resources r LEFT JOIN users u ON u.id = r.user_id";

pub(crate) async fn load_resource(state: &AppState, id: Uuid) -> AppResult<Resource> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{RESOURCE_SELECT} WHERE r.id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("resource"))
}

pub(crate) async fn hours_of(state: &AppState, resource: Uuid) -> AppResult<Vec<(i32, i32, i32)>> {
    Ok(sqlx::query_as("SELECT weekday, start_min, end_min FROM booking_hours WHERE resource_id = $1 ORDER BY weekday, start_min").bind(resource).fetch_all(&state.db).await?)
}

async fn resource_json(state: &AppState, r: Resource) -> AppResult<Value> {
    let hours = hours_of(state, r.id).await?;
    let off: Vec<(NaiveDate, Option<String>)> = sqlx::query_as("SELECT day, note FROM booking_days_off WHERE resource_id = $1 AND day >= CURRENT_DATE - 30 ORDER BY day").bind(r.id).fetch_all(&state.db).await?;
    Ok(json!({
        "id": r.id, "name": r.name, "kind": r.kind, "description": r.description, "user_id": r.user_id, "user_name": r.user_name,
        "capacity": r.capacity, "allow_overbooking": r.allow_overbooking, "color": r.color, "timezone": r.timezone,
        "slot_minutes": r.slot_minutes, "buffer_minutes": r.buffer_minutes, "min_notice_minutes": r.min_notice_minutes, "max_days_ahead": r.max_days_ahead,
        "is_active": r.is_active, "sort": r.sort,
        "hours": hours.into_iter().map(|(d, s, e)| json!({ "weekday": d, "start_min": s, "end_min": e })).collect::<Vec<_>>(),
        "days_off": off.into_iter().map(|(d, n)| json!({ "day": d, "note": n })).collect::<Vec<_>>(),
    }))
}

async fn list_resources(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    let rows: Vec<Resource> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{RESOURCE_SELECT} ORDER BY r.is_active DESC, r.sort, r.name"))).fetch_all(&state.db).await?;
    let mut items = Vec::new();
    for r in rows {
        items.push(resource_json(&state, r).await?);
    }
    Ok(Json(json!({ "items": items })))
}

async fn get_resource(State(state): State<AppState>, _auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let r = load_resource(&state, id).await?;
    Ok(Json(resource_json(&state, r).await?))
}

#[derive(Deserialize)]
struct ResourceInput {
    name: Option<String>,
    kind: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    user_id: Option<Option<Uuid>>,
    capacity: Option<i32>,
    allow_overbooking: Option<bool>,
    color: Option<String>,
    timezone: Option<String>,
    slot_minutes: Option<i32>,
    buffer_minutes: Option<i32>,
    min_notice_minutes: Option<i32>,
    max_days_ahead: Option<i32>,
    is_active: Option<bool>,
    sort: Option<i32>,
}

const COLORS: &[&str] = &["sky", "indigo", "violet", "emerald", "amber", "rose", "teal", "orange", "slate"];

fn check_resource(b: &ResourceInput) -> AppResult<()> {
    if b.kind.as_deref().is_some_and(|k| !["person", "room", "equipment", "other"].contains(&k)) {
        return Err(AppError::bad("kind must be person, room, equipment or other"));
    }
    if b.color.as_deref().is_some_and(|c| !COLORS.contains(&c)) {
        return Err(AppError::bad("unknown colour"));
    }
    if let Some(tz) = &b.timezone {
        tz.parse::<Tz>().map_err(|_| AppError::bad(format!("unknown time zone: {tz}")))?;
    }
    if b.capacity.is_some_and(|c| !(1..=1000).contains(&c)) {
        return Err(AppError::bad("capacity is 1 to 1000"));
    }
    if b.slot_minutes.is_some_and(|m| !(5..=1440).contains(&m)) {
        return Err(AppError::bad("slots are 5 minutes to a day"));
    }
    if b.buffer_minutes.is_some_and(|m| !(0..=480).contains(&m)) {
        return Err(AppError::bad("the buffer is 0 to 480 minutes"));
    }
    if b.min_notice_minutes.is_some_and(|m| m < 0) || b.max_days_ahead.is_some_and(|d| !(1..=730).contains(&d)) {
        return Err(AppError::bad("check the booking window"));
    }
    Ok(())
}

async fn create_resource(State(state): State<AppState>, auth: AuthUser, Json(b): Json<ResourceInput>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    check_resource(&b)?;
    let kind = b.kind.clone().unwrap_or_else(|| "person".into());
    let user = b.user_id.flatten();
    let name = match (clean(b.name.clone()), user) {
        (Some(n), _) => required("name", &n, 200)?,
        (None, Some(u)) => sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(u).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("user"))?,
        _ => return Err(AppError::bad("name the resource")),
    };
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO booking_resources (name, kind, description, user_id, capacity, allow_overbooking, color, timezone, slot_minutes, buffer_minutes, min_notice_minutes, max_days_ahead, sort)
         VALUES ($1, $2, $3, $4, COALESCE($5, 1), COALESCE($6, FALSE), COALESCE($7, 'sky'), COALESCE($8, 'UTC'), COALESCE($9, 30), COALESCE($10, 0), COALESCE($11, 60), COALESCE($12, 60), COALESCE($13, 0))
         RETURNING id",
    )
    .bind(&name)
    .bind(&kind)
    .bind(clean(b.description.flatten()))
    .bind(user)
    .bind(b.capacity)
    .bind(b.allow_overbooking)
    .bind(&b.color)
    .bind(&b.timezone)
    .bind(b.slot_minutes)
    .bind(b.buffer_minutes)
    .bind(b.min_notice_minutes)
    .bind(b.max_days_ahead)
    .bind(b.sort)
    .fetch_one(&state.db)
    .await?;
    // Office hours to start with: Monday to Friday, 9 to 6.
    for d in 1..=5 {
        sqlx::query("INSERT INTO booking_hours (resource_id, weekday, start_min, end_min) VALUES ($1, $2, 540, 1080)").bind(id).bind(d).execute(&state.db).await?;
    }
    get_resource(State(state), auth, Path(id)).await
}

async fn update_resource(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ResourceInput>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    check_resource(&b)?;
    load_resource(&state, id).await?;
    let name = match &b.name {
        Some(n) => Some(required("name", n, 200)?),
        None => None,
    };
    sqlx::query(
        "UPDATE booking_resources SET name = COALESCE($2, name), kind = COALESCE($3, kind),
            description = CASE WHEN $4 THEN $5 ELSE description END, user_id = CASE WHEN $6 THEN $7 ELSE user_id END,
            capacity = COALESCE($8, capacity), allow_overbooking = COALESCE($9, allow_overbooking), color = COALESCE($10, color),
            timezone = COALESCE($11, timezone), slot_minutes = COALESCE($12, slot_minutes), buffer_minutes = COALESCE($13, buffer_minutes),
            min_notice_minutes = COALESCE($14, min_notice_minutes), max_days_ahead = COALESCE($15, max_days_ahead),
            is_active = COALESCE($16, is_active), sort = COALESCE($17, sort)
         WHERE id = $1",
    )
    .bind(id)
    .bind(name)
    .bind(&b.kind)
    .bind(b.description.is_some())
    .bind(clean(b.description.clone().flatten()))
    .bind(b.user_id.is_some())
    .bind(b.user_id.flatten())
    .bind(b.capacity)
    .bind(b.allow_overbooking)
    .bind(&b.color)
    .bind(&b.timezone)
    .bind(b.slot_minutes)
    .bind(b.buffer_minutes)
    .bind(b.min_notice_minutes)
    .bind(b.max_days_ahead)
    .bind(b.is_active)
    .bind(b.sort)
    .execute(&state.db)
    .await?;
    get_resource(State(state), auth, Path(id)).await
}

async fn delete_resource(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    let future: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM booking_resources_used u JOIN bookings b ON b.id = u.booking_id
         WHERE u.resource_id = $1 AND b.ends_at > now() AND b.status IN ('booked', 'confirmed')",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if future > 0 {
        return Err(AppError::bad(format!("{future} bookings to come use it; switch it off instead, or move them first")));
    }
    sqlx::query("DELETE FROM booking_resources WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct HoursInput {
    hours: Vec<HourInterval>,
}
#[derive(Deserialize)]
struct HourInterval {
    weekday: i32,
    start_min: i32,
    end_min: i32,
}

async fn set_hours(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<HoursInput>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    load_resource(&state, id).await?;
    let mut list = b.hours;
    list.sort_by_key(|h| (h.weekday, h.start_min));
    for (i, h) in list.iter().enumerate() {
        if !(1..=7).contains(&h.weekday) || !(0..1440).contains(&h.start_min) || !(1..=1440).contains(&h.end_min) || h.end_min <= h.start_min {
            return Err(AppError::bad("each interval is a weekday with a start before its end"));
        }
        if i > 0 && list[i - 1].weekday == h.weekday && list[i - 1].end_min > h.start_min {
            return Err(AppError::bad("working hours of a day overlap"));
        }
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM booking_hours WHERE resource_id = $1").bind(id).execute(&mut *tx).await?;
    for h in &list {
        sqlx::query("INSERT INTO booking_hours (resource_id, weekday, start_min, end_min) VALUES ($1, $2, $3, $4)").bind(id).bind(h.weekday).bind(h.start_min).bind(h.end_min).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    get_resource(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct DaysOff {
    days: Vec<DayOff>,
}
#[derive(Deserialize)]
struct DayOff {
    day: NaiveDate,
    note: Option<String>,
}

async fn set_days_off(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<DaysOff>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    load_resource(&state, id).await?;
    if b.days.len() > 500 {
        return Err(AppError::bad("at most 500 days off"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM booking_days_off WHERE resource_id = $1 AND day >= CURRENT_DATE - 30").bind(id).execute(&mut *tx).await?;
    for d in &b.days {
        sqlx::query("INSERT INTO booking_days_off (resource_id, day, note) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING").bind(id).bind(d.day).bind(clean(d.note.clone())).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    get_resource(State(state), auth, Path(id)).await
}

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow, Clone)]
pub(crate) struct Service {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub duration_minutes: i32,
    pub price: rust_decimal::Decimal,
    pub is_extra: bool,
    pub resource_ids: Vec<Uuid>,
    pub is_active: bool,
    pub sort: i32,
}

pub(crate) fn service_json(s: &Service) -> Value {
    json!({ "id": s.id, "name": s.name, "description": s.description, "duration_minutes": s.duration_minutes, "price": s.price.to_string(),
            "is_extra": s.is_extra, "resource_ids": s.resource_ids, "is_active": s.is_active, "sort": s.sort })
}

pub(crate) async fn load_services(state: &AppState) -> AppResult<Vec<Service>> {
    Ok(sqlx::query_as("SELECT id, name, description, duration_minutes, price, is_extra, resource_ids, is_active, sort FROM booking_services ORDER BY is_extra, sort, name").fetch_all(&state.db).await?)
}

async fn list_services(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "items": load_services(&state).await?.iter().map(service_json).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct ServiceInput {
    name: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    duration_minutes: Option<i32>,
    price: Option<rust_decimal::Decimal>,
    is_extra: Option<bool>,
    resource_ids: Option<Vec<Uuid>>,
    is_active: Option<bool>,
    sort: Option<i32>,
}

async fn create_service(State(state): State<AppState>, auth: AuthUser, Json(b): Json<ServiceInput>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 200)?;
    let duration = b.duration_minutes.unwrap_or(60);
    if !(5..=525_600).contains(&duration) {
        return Err(AppError::bad("a service lasts 5 minutes to a year"));
    }
    if b.price.is_some_and(|p| p.is_sign_negative()) {
        return Err(AppError::bad("the price can't be negative"));
    }
    sqlx::query("INSERT INTO booking_services (name, description, duration_minutes, price, is_extra, resource_ids, sort) VALUES ($1, $2, $3, COALESCE($4, 0), COALESCE($5, FALSE), $6, COALESCE($7, 0))")
        .bind(name)
        .bind(clean(b.description.flatten()))
        .bind(duration)
        .bind(b.price)
        .bind(b.is_extra)
        .bind(b.resource_ids.unwrap_or_default())
        .bind(b.sort)
        .execute(&state.db)
        .await?;
    list_services(State(state), auth).await
}

async fn update_service(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ServiceInput>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    if b.duration_minutes.is_some_and(|d| !(5..=525_600).contains(&d)) {
        return Err(AppError::bad("a service lasts 5 minutes to a year"));
    }
    let name = match &b.name {
        Some(n) => Some(required("name", n, 200)?),
        None => None,
    };
    let n = sqlx::query(
        "UPDATE booking_services SET name = COALESCE($2, name), description = CASE WHEN $3 THEN $4 ELSE description END,
            duration_minutes = COALESCE($5, duration_minutes), price = COALESCE($6, price), is_extra = COALESCE($7, is_extra),
            resource_ids = COALESCE($8, resource_ids), is_active = COALESCE($9, is_active), sort = COALESCE($10, sort) WHERE id = $1",
    )
    .bind(id)
    .bind(name)
    .bind(b.description.is_some())
    .bind(clean(b.description.flatten()))
    .bind(b.duration_minutes)
    .bind(b.price)
    .bind(b.is_extra)
    .bind(b.resource_ids)
    .bind(b.is_active)
    .bind(b.sort)
    .execute(&state.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("service"));
    }
    list_services(State(state), auth).await
}

async fn delete_service(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    manage(&auth)?;
    sqlx::query("DELETE FROM booking_services WHERE id = $1").bind(id).execute(&state.db).await?;
    list_services(State(state), auth).await
}

// ---------------------------------------------------------------------------
// Availability
// ---------------------------------------------------------------------------

/// Whether [start, end) lies within the resource's working hours (on one working interval,
/// or across whole working days for bookings of a day or more) and not on a day off.
pub(crate) async fn within_hours(state: &AppState, r: &Resource, start: DateTime<Utc>, end: DateTime<Utc>) -> AppResult<bool> {
    let tz: Tz = r.timezone.parse().unwrap_or(chrono_tz::UTC);
    let (ls, le) = (start.with_timezone(&tz), end.with_timezone(&tz));
    let hours = hours_of(state, r.id).await?;
    let off: Vec<NaiveDate> = sqlx::query_scalar("SELECT day FROM booking_days_off WHERE resource_id = $1 AND day BETWEEN $2 AND $3")
        .bind(r.id)
        .bind(ls.date_naive())
        .bind(le.date_naive())
        .fetch_all(&state.db)
        .await?;
    if off.contains(&ls.date_naive()) {
        return Ok(false);
    }
    let minute = |t: chrono::DateTime<Tz>| (t.time().num_seconds_from_midnight() / 60) as i32;
    use chrono::Timelike;
    if ls.date_naive() == le.date_naive() || (le - ls) < Duration::days(1) && le.time().num_seconds_from_midnight() == 0 {
        let wd = ls.weekday().number_from_monday() as i32;
        let (s, e) = (minute(ls), if le.date_naive() != ls.date_naive() { 1440 } else { minute(le) });
        return Ok(hours.iter().any(|(d, hs, he)| *d == wd && *hs <= s && e <= *he));
    }
    // Several days: every day it covers must be a working day (and not a day off).
    let mut d = ls.date_naive();
    while d <= le.date_naive() {
        let wd = d.weekday().number_from_monday() as i32;
        if !hours.iter().any(|(x, ..)| *x == wd) || off.contains(&d) {
            // The last day may end at midnight.
            if !(d == le.date_naive() && minute(le) == 0) {
                return Ok(false);
            }
        }
        d = d.succ_opt().unwrap();
    }
    Ok(true)
}

/// Bookings that hold the resource during [start, end), the buffer counted (`except`: a
/// booking being moved).
pub(crate) async fn overlapping(state: &AppState, r: &Resource, start: DateTime<Utc>, end: DateTime<Utc>, except: Option<Uuid>) -> AppResult<i64> {
    let buffer = Duration::minutes(r.buffer_minutes as i64);
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM bookings b JOIN booking_resources_used u ON u.booking_id = b.id
         WHERE u.resource_id = $1 AND b.status IN ('booked', 'confirmed', 'came') AND ($4::uuid IS NULL OR b.id <> $4)
           AND b.starts_at < $3 AND b.ends_at + $5 > $2",
    )
    .bind(r.id)
    .bind(start)
    .bind(end + buffer)
    .bind(except)
    .bind(buffer)
    .fetch_one(&state.db)
    .await?)
}

/// Free start times for a service on a resource on a day (in the resource's zone).
pub(crate) async fn free_slots(state: &AppState, r: &Resource, duration: Duration, day: NaiveDate, for_clients: bool) -> AppResult<Vec<DateTime<Utc>>> {
    let tz: Tz = r.timezone.parse().unwrap_or(chrono_tz::UTC);
    let now = Utc::now();
    if for_clients && (day < now.with_timezone(&tz).date_naive() || day > (now + Duration::days(r.max_days_ahead as i64)).with_timezone(&tz).date_naive()) {
        return Ok(vec![]);
    }
    let off: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM booking_days_off WHERE resource_id = $1 AND day = $2)").bind(r.id).bind(day).fetch_one(&state.db).await?;
    if off {
        return Ok(vec![]);
    }
    let wd = day.weekday().number_from_monday() as i32;
    let mut out = Vec::new();
    for (d, hs, he) in hours_of(state, r.id).await? {
        if d != wd {
            continue;
        }
        let mut m = hs;
        while m + (duration.num_minutes() as i32).min(1440) <= he || (duration >= Duration::days(1) && m == hs) {
            let Some(local) = tz.from_local_datetime(&day.and_hms_opt((m / 60) as u32, (m % 60) as u32, 0).unwrap()).earliest() else {
                m += r.slot_minutes;
                continue;
            };
            let start = local.with_timezone(&Utc);
            let end = start + duration;
            let early = for_clients && start < now + Duration::minutes(r.min_notice_minutes as i64);
            if !early && within_hours(state, r, start, end).await? && overlapping(state, r, start, end, None).await? < r.capacity as i64 {
                out.push(start);
            }
            if duration >= Duration::days(1) {
                break;
            }
            m += r.slot_minutes;
        }
    }
    Ok(out)
}
