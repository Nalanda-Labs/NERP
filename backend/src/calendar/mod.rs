//! Company calendar: events with invited colleagues and their answers, repeats, reminders,
//! a link to a CRM record, and (in the view) task deadlines and CRM calls and meetings.
//!
//! Who sees what: everyone sees their own events and those they are invited to. Looking at a
//! colleague's calendar shows their events; private ones (unless you are invited) only as
//! "Busy". Only the owner changes an event; guests answer.
//!
//! A repeating event is stored once; its occurrences are worked out for the range being
//! viewed. Removing one occurrence keeps the rest. Each person can subscribe other calendar
//! apps to a private link (`/api/public/calendar/<token>.ics`).

mod ics;

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::{tokens::random_token, AuthUser},
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::{clean, required},
    state::AppState,
    tasks::recurrence::Recurrence,
};

pub const REMIND_JOB: &str = "calendar.remind";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/calendar", get(view))
        .route("/calendar/events", post(create))
        .route("/calendar/events/{id}", get(get_event).patch(update).delete(remove))
        .route("/calendar/events/{id}/respond", post(respond))
        .route("/calendar/events/{id}/chat", post(event_chat))
        .route("/calendar/feed", get(feed_info).post(new_feed).delete(drop_feed))
        .merge(ics::router())
}

// ---------------------------------------------------------------------------
// Events and their occurrences
// ---------------------------------------------------------------------------

#[derive(FromRow, Clone)]
pub(crate) struct Event {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub all_day: bool,
    pub timezone: String,
    pub recurrence: Option<Value>,
    pub color: Option<String>,
    pub item_id: Option<Uuid>,
    pub remind_minutes: Option<i32>,
    pub is_private: bool,
    /// On a workgroup's calendar.
    pub project_id: Option<Uuid>,
}

const EVENT_COLUMNS: &str = "e.id, e.owner_id, e.title, e.description, e.location, e.starts_at, e.ends_at, e.all_day, e.timezone, e.recurrence, e.color, e.item_id, e.remind_minutes, e.is_private, e.project_id";

impl Event {
    fn rule(&self) -> Option<Recurrence> {
        self.recurrence.clone().and_then(|r| serde_json::from_value(r).ok())
    }
}

/// Starts of the occurrences overlapping [from, to), skipping removed ones.
pub(crate) fn occurrences(e: &Event, exceptions: &[DateTime<Utc>], from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<DateTime<Utc>> {
    let dur = e.ends_at - e.starts_at;
    // A zero-length event still shows on its day.
    let overlaps = |s: DateTime<Utc>| s < to && (s + dur > from || (dur.is_zero() && s >= from));
    let Some(rule) = e.rule() else {
        return if overlaps(e.starts_at) { vec![e.starts_at] } else { vec![] };
    };
    let mut out = Vec::new();
    let mut occ = e.starts_at;
    let mut n = 1;
    for _ in 0..3000 {
        if occ >= to {
            break;
        }
        if overlaps(occ) && !exceptions.contains(&occ) {
            out.push(occ);
        }
        if rule.max_runs.is_some_and(|m| n >= m) {
            break;
        }
        match rule.next_after(occ) {
            Some(next) => occ = next,
            None => break,
        }
        n += 1;
    }
    out
}

pub(crate) async fn exceptions(state: &AppState, ids: &[Uuid]) -> AppResult<Vec<(Uuid, DateTime<Utc>)>> {
    Ok(sqlx::query_as("SELECT event_id, occurrence_start FROM cal_exceptions WHERE event_id = ANY($1)").bind(ids).fetch_all(&state.db).await?)
}

// ---------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ViewQuery {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    /// Comma-separated colleagues to show besides oneself.
    users: Option<String>,
    /// Show my task deadlines and CRM calls and meetings too.
    extras: Option<String>,
    /// A workgroup's calendar instead.
    project_id: Option<Uuid>,
}

#[derive(Serialize)]
struct Entry {
    /// event | task | activity
    kind: &'static str,
    id: Uuid,
    /// Whose calendar this is on.
    user_id: Uuid,
    title: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    all_day: bool,
    /// For repeating events: which occurrence.
    occurrence: Option<DateTime<Utc>>,
    recurring: bool,
    location: Option<String>,
    color: Option<String>,
    /// Shown as "Busy": a colleague's private event.
    busy: bool,
    owner_id: Option<Uuid>,
    /// My answer when I am a guest.
    my_status: Option<String>,
    link: Option<String>,
}

async fn view(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ViewQuery>) -> AppResult<Json<Value>> {
    if q.to <= q.from || q.to - q.from > Duration::days(400) {
        return Err(AppError::bad("show at most 400 days at once"));
    }
    let mut users = vec![auth.id];
    for u in q.users.as_deref().unwrap_or("").split(',').filter(|s| !s.is_empty()) {
        let id = Uuid::parse_str(u).map_err(|_| AppError::bad("bad user id"))?;
        if !users.contains(&id) {
            users.push(id);
        }
    }
    if users.len() > 20 {
        return Err(AppError::bad("show at most 20 calendars at once"));
    }
    let group = q.project_id;
    if let Some(g) = group {
        crate::tasks::access::project_access(&state, &auth, g).await?;
        users.truncate(1);
    }
    // Events of these people (owned or invited to), or on the group's calendar, that may
    // overlap the range.
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT e.id FROM cal_events e LEFT JOIN cal_attendees a ON a.event_id = e.id
         WHERE (CASE WHEN $4::uuid IS NULL THEN e.owner_id = ANY($1) OR a.user_id = ANY($1) ELSE e.project_id = $4 END)
           AND e.starts_at < $3 AND (e.recurrence IS NOT NULL OR e.ends_at >= $2)
         LIMIT 2000",
    )
    .bind(&users)
    .bind(q.from)
    .bind(q.to)
    .bind(group)
    .fetch_all(&state.db)
    .await?;
    let events = load_events(&state, &ids).await?;
    let ids: Vec<Uuid> = events.iter().map(|(e, _)| e.id).collect();
    let ex = exceptions(&state, &ids).await?;
    let answers: Vec<(Uuid, Uuid, String)> = sqlx::query_as("SELECT event_id, user_id, status FROM cal_attendees WHERE event_id = ANY($1)").bind(&ids).fetch_all(&state.db).await?;
    let mut out: Vec<Entry> = Vec::new();
    for (e, guests) in &events {
        let mine = e.owner_id == auth.id || guests.contains(&auth.id);
        let excs: Vec<DateTime<Utc>> = ex.iter().filter(|(id, _)| *id == e.id).map(|(_, t)| *t).collect();
        // Whose calendars it shows on (one entry per person being looked at).
        let on: Vec<Uuid> = if group.is_some() { vec![auth.id] } else { users.iter().copied().filter(|u| *u == e.owner_id || guests.contains(u)).collect() };
        let my_status = answers.iter().find(|(id, u, _)| *id == e.id && *u == auth.id).map(|(_, _, s)| s.clone());
        for start in occurrences(e, &excs, q.from, q.to) {
            for u in &on {
                // Mine shows once, on my own calendar.
                if mine && *u != auth.id {
                    continue;
                }
                let busy = !mine && e.is_private;
                out.push(Entry {
                    kind: "event",
                    id: e.id,
                    user_id: *u,
                    title: if busy { "Busy".into() } else { e.title.clone() },
                    start,
                    end: start + (e.ends_at - e.starts_at),
                    all_day: e.all_day,
                    occurrence: e.recurrence.as_ref().map(|_| start),
                    recurring: e.recurrence.is_some(),
                    location: if busy { None } else { e.location.clone() },
                    color: e.color.clone(),
                    busy,
                    owner_id: Some(e.owner_id),
                    my_status: if e.owner_id == auth.id { None } else { my_status.clone() },
                    link: None,
                });
            }
        }
    }
    if q.extras.as_deref() == Some("true") && group.is_none() {
        let tasks: Vec<(Uuid, i64, String, DateTime<Utc>)> = sqlx::query_as(
            "SELECT id, number, title, deadline FROM tasks WHERE responsible_id = $1 AND status NOT IN ('done', 'deferred') AND deadline >= $2 AND deadline < $3",
        )
        .bind(auth.id)
        .bind(q.from)
        .bind(q.to)
        .fetch_all(&state.db)
        .await?;
        for (id, number, title, at) in tasks {
            out.push(Entry {
                kind: "task", id, user_id: auth.id, title: format!("#{number} {title}"), start: at, end: at, all_day: false, occurrence: None, recurring: false,
                location: None, color: None, busy: false, owner_id: None, my_status: None, link: Some(format!("/tasks/{id}")),
            });
        }
        let acts: Vec<(Uuid, String, String, DateTime<Utc>, Option<DateTime<Utc>>, Uuid, String)> = sqlx::query_as(
            "SELECT a.id, a.kind, a.subject, COALESCE(a.starts_at, a.deadline), a.ends_at, a.item_id, t.code
             FROM crm_activities a JOIN crm_items i ON i.id = a.item_id JOIN crm_entity_types t ON t.id = i.entity_type
             WHERE a.responsible_id = $1 AND NOT a.is_completed AND a.kind IN ('call', 'meeting')
               AND COALESCE(a.starts_at, a.deadline) >= $2 AND COALESCE(a.starts_at, a.deadline) < $3 AND i.deleted_at IS NULL",
        )
        .bind(auth.id)
        .bind(q.from)
        .bind(q.to)
        .fetch_all(&state.db)
        .await?;
        for (id, kind, subject, start, end, item, code) in acts {
            out.push(Entry {
                kind: "activity", id, user_id: auth.id, title: format!("{} {subject}", if kind == "call" { "Call:" } else { "Meeting:" }),
                start, end: end.filter(|e| *e >= start).unwrap_or(start + Duration::minutes(30)), all_day: false, occurrence: None, recurring: false,
                location: None, color: None, busy: false, owner_id: None, my_status: None, link: Some(format!("/crm/{code}/{item}")),
            });
        }
    }
    // Bookings with the people shown (those who are a booking resource).
    if group.is_none() {
        let bookings: Vec<(Uuid, i64, String, Option<String>, DateTime<Utc>, DateTime<Utc>, Uuid)> = sqlx::query_as(
            "SELECT b.id, b.number, b.client_name, s.name, b.starts_at, b.ends_at, r.user_id FROM bookings b
             JOIN booking_resources_used u ON u.booking_id = b.id JOIN booking_resources r ON r.id = u.resource_id
             LEFT JOIN booking_services s ON s.id = b.service_id
             WHERE r.user_id = ANY($1) AND b.status IN ('booked', 'confirmed', 'came') AND b.starts_at < $3 AND b.ends_at > $2",
        )
        .bind(&users)
        .bind(q.from)
        .bind(q.to)
        .fetch_all(&state.db)
        .await?;
        for (id, number, client, service, start, end, user) in bookings {
            out.push(Entry {
                kind: "booking", id, user_id: user, title: format!("Booking #{number}: {client}{}", service.map(|s| format!(" — {s}")).unwrap_or_default()),
                start, end, all_day: false, occurrence: None, recurring: false, location: None, color: Some("teal".into()), busy: false, owner_id: None,
                my_status: None, link: Some(format!("/booking?booking={id}")),
            });
        }
    }
    out.sort_by_key(|e| (e.start, e.title.clone()));
    let people: Vec<(Uuid, String, Option<Uuid>)> = sqlx::query_as("SELECT id, trim(first_name || ' ' || last_name), avatar_file_id FROM users WHERE id = ANY($1)")
        .bind(&users)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!({
        "items": out,
        "people": people.into_iter().map(|(id, name, a)| json!({ "id": id, "name": name, "avatar_file_id": a })).collect::<Vec<_>>(),
    })))
}

pub(crate) async fn load_events(state: &AppState, ids: &[Uuid]) -> AppResult<Vec<(Event, Vec<Uuid>)>> {
    let events: Vec<Event> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {EVENT_COLUMNS} FROM cal_events e WHERE e.id = ANY($1)")))
        .bind(ids)
        .fetch_all(&state.db)
        .await?;
    let guests: Vec<(Uuid, Uuid)> = sqlx::query_as("SELECT event_id, user_id FROM cal_attendees WHERE event_id = ANY($1)").bind(ids).fetch_all(&state.db).await?;
    Ok(events
        .into_iter()
        .map(|e| {
            let g = guests.iter().filter(|(id, _)| *id == e.id).map(|(_, u)| *u).collect();
            (e, g)
        })
        .collect())
}

// ---------------------------------------------------------------------------
// One event
// ---------------------------------------------------------------------------

async fn event_view(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Value> {
    let (e, guests) = load_events(state, &[id]).await?.into_iter().next().ok_or(AppError::NotFound("event"))?;
    let mine = e.owner_id == auth.id || guests.contains(&auth.id);
    if !mine && e.is_private {
        return Err(AppError::NotFound("event"));
    }
    // An event on a workgroup's calendar: for those who can see the group (and its guests).
    if let Some(g) = e.project_id {
        if !mine && crate::tasks::access::project_access(state, auth, g).await.is_err() {
            return Err(AppError::NotFound("event"));
        }
    }
    let people: Vec<(Uuid, String, Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT u.id, trim(u.first_name || ' ' || u.last_name), u.avatar_file_id, a.status FROM users u
         LEFT JOIN cal_attendees a ON a.user_id = u.id AND a.event_id = $1 WHERE u.id = $2 OR a.event_id = $1 ORDER BY (u.id = $2) DESC, u.first_name",
    )
    .bind(id)
    .bind(e.owner_id)
    .fetch_all(&state.db)
    .await?;
    let item: Option<(String, String)> = match e.item_id {
        Some(i) => sqlx::query_as("SELECT t.code, i.title FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type WHERE i.id = $1").bind(i).fetch_optional(&state.db).await?,
        None => None,
    };
    let project: Option<Value> = match e.project_id {
        Some(g) => sqlx::query_scalar::<_, String>("SELECT name FROM projects WHERE id = $1").bind(g).fetch_optional(&state.db).await?.map(|n| json!({ "id": g, "name": n })),
        None => None,
    };
    let rule = e.rule();
    Ok(json!({
        "id": e.id, "owner_id": e.owner_id, "title": e.title, "description": e.description, "location": e.location,
        "starts_at": e.starts_at, "ends_at": e.ends_at, "all_day": e.all_day, "timezone": e.timezone,
        "recurrence": e.recurrence, "repeat": rule.map(|r| r.describe()), "color": e.color, "remind_minutes": e.remind_minutes, "is_private": e.is_private,
        "project": project,
        "item": e.item_id.zip(item).map(|(id, (code, title))| json!({ "id": id, "code": code, "title": title })),
        "people": people.into_iter().map(|(id, name, a, status)| json!({ "id": id, "name": name, "avatar_file_id": a, "status": status, "owner": id == e.owner_id })).collect::<Vec<_>>(),
        "can_edit": e.owner_id == auth.id,
    }))
}

async fn get_event(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    event_view(&state, &auth, id).await.map(Json)
}

/// The repeat part a person chooses; the rest comes from the event's start.
#[derive(Deserialize)]
struct RepeatInput {
    freq: String,
    #[serde(default = "one")]
    interval: u32,
    #[serde(default)]
    weekdays: Vec<u8>,
    month_day: Option<u8>,
    ends_on: Option<NaiveDate>,
    max_runs: Option<i32>,
}
fn one() -> u32 {
    1
}

#[derive(Deserialize)]
struct EventInput {
    title: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    location: Option<Option<String>>,
    /// Timed: instants. All-day: the first and last day (YYYY-MM-DD).
    start: Option<String>,
    end: Option<String>,
    all_day: Option<bool>,
    timezone: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    repeat: Option<Option<RepeatInput>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    color: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    item_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    remind_minutes: Option<Option<i32>>,
    is_private: Option<bool>,
    /// Guests (replaces the list).
    attendees: Option<Vec<Uuid>>,
    /// Put it on a workgroup's calendar (when creating).
    project_id: Option<Uuid>,
}

struct Times {
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    all_day: bool,
}

fn times(start: &str, end: &str, all_day: bool) -> AppResult<Times> {
    if all_day {
        let s = NaiveDate::parse_from_str(start, "%Y-%m-%d").map_err(|_| AppError::bad("the first day must look like 2026-03-01"))?;
        let e = NaiveDate::parse_from_str(end, "%Y-%m-%d").map_err(|_| AppError::bad("the last day must look like 2026-03-01"))?;
        if e < s {
            return Err(AppError::bad("the event ends before it starts"));
        }
        let at = |d: NaiveDate| Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap());
        Ok(Times { starts_at: at(s), ends_at: at(e) + Duration::days(1), all_day })
    } else {
        let s: DateTime<Utc> = DateTime::parse_from_rfc3339(start).map_err(|_| AppError::bad("bad start time"))?.with_timezone(&Utc);
        let e: DateTime<Utc> = DateTime::parse_from_rfc3339(end).map_err(|_| AppError::bad("bad end time"))?.with_timezone(&Utc);
        if e < s {
            return Err(AppError::bad("the event ends before it starts"));
        }
        if e - s > Duration::days(31) {
            return Err(AppError::bad("an event can last at most 31 days"));
        }
        Ok(Times { starts_at: s, ends_at: e, all_day })
    }
}

/// A full repeat rule anchored at the event's start, in its time zone.
fn rule_for(r: RepeatInput, t: &Times, tz: &str) -> AppResult<Value> {
    let zone: Tz = if t.all_day { chrono_tz::UTC } else { tz.parse().map_err(|_| AppError::bad(format!("unknown time zone: {tz}")))? };
    let local = t.starts_at.with_timezone(&zone);
    let rule = Recurrence {
        freq: r.freq,
        interval: r.interval,
        weekdays: r.weekdays,
        month_day: r.month_day,
        time: local.format("%H:%M").to_string(),
        timezone: zone.name().to_string(),
        starts_on: local.date_naive(),
        ends_on: r.ends_on,
        max_runs: r.max_runs,
    };
    rule.validate()?;
    Ok(serde_json::to_value(rule).unwrap())
}

async fn check_guests(state: &AppState, ids: &[Uuid]) -> AppResult<()> {
    if ids.len() > 100 {
        return Err(AppError::bad("at most 100 guests"));
    }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND is_active").bind(ids).fetch_one(&state.db).await?;
    if n as usize != ids.len() {
        return Err(AppError::bad("guests must be active employees"));
    }
    Ok(())
}

fn color(c: Option<String>) -> AppResult<Option<String>> {
    match clean(c) {
        None => Ok(None),
        Some(c) if c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit()) => Ok(Some(c.to_lowercase())),
        Some(_) => Err(AppError::bad("colour must look like #1a73e8")),
    }
}

async fn check_item(state: &AppState, auth: &AuthUser, item: Option<Uuid>) -> AppResult<()> {
    if let Some(i) = item {
        let row = crate::crm::items::load_row(state, i, false).await?;
        crate::crm::access::require_read(state, auth, row.entity_type, i, row.assigned_to).await?;
    }
    Ok(())
}

fn reminder(m: Option<i32>) -> AppResult<Option<i32>> {
    if m.is_some_and(|m| !(0..=40320).contains(&m)) {
        return Err(AppError::bad("reminders can be up to four weeks before"));
    }
    Ok(m)
}

/// "Tue 10 Nov, 10:00" in the event's time zone (or the day, for all-day events).
fn when(e: &Event, at: DateTime<Utc>) -> String {
    if e.all_day {
        return at.format("%a %-d %b").to_string();
    }
    let tz: Tz = e.timezone.parse().unwrap_or(chrono_tz::UTC);
    let l = at.with_timezone(&tz);
    format!("{} ({})", l.format("%a %-d %b, %H:%M"), tz.name())
}

/// Creates an event from the same JSON the API takes (for other modules, e.g. "event from a chat").
pub async fn create_event(state: &AppState, auth: &AuthUser, input: Value) -> AppResult<Value> {
    let b: EventInput = serde_json::from_value(input).map_err(|e| AppError::bad(format!("event: {e}")))?;
    create(State(state.clone()), auth.clone(), Json(b)).await.map(|j| j.0)
}

/// Changes an event from the same JSON the API takes (for other modules, e.g. meetings).
pub async fn update_event(state: &AppState, auth: &AuthUser, id: Uuid, input: Value) -> AppResult<Value> {
    let b: EventInput = serde_json::from_value(input).map_err(|e| AppError::bad(format!("event: {e}")))?;
    update(State(state.clone()), auth.clone(), Path(id), Json(b)).await.map(|j| j.0)
}

/// Removes an event (all its occurrences), for other modules.
pub async fn delete_event(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<()> {
    remove(State(state.clone()), auth.clone(), Path(id), Query(RemoveQuery { occurrence: None })).await.map(|_| ())
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<EventInput>) -> AppResult<Json<Value>> {
    let title = required("title", b.title.as_deref().unwrap_or(""), 300)?;
    let all_day = b.all_day.unwrap_or(false);
    let t = times(b.start.as_deref().unwrap_or(""), b.end.as_deref().unwrap_or(""), all_day)?;
    let tz = b.timezone.clone().unwrap_or_else(|| "UTC".into());
    let _: Tz = tz.parse().map_err(|_| AppError::bad(format!("unknown time zone: {tz}")))?;
    let rule = b.repeat.flatten().map(|r| rule_for(r, &t, &tz)).transpose()?;
    let item = b.item_id.flatten();
    check_item(&state, &auth, item).await?;
    let mut guests = b.attendees.unwrap_or_default();
    guests.retain(|g| *g != auth.id);
    guests.sort();
    guests.dedup();
    check_guests(&state, &guests).await?;
    if let Some(g) = b.project_id {
        if !crate::tasks::access::project_access(&state, &auth, g).await?.contribute {
            return Err(AppError::Forbidden);
        }
    }
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO cal_events (owner_id, title, description, location, starts_at, ends_at, all_day, timezone, recurrence, color, item_id, remind_minutes, is_private, project_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) RETURNING id",
    )
    .bind(auth.id)
    .bind(&title)
    .bind(clean(b.description.flatten()))
    .bind(clean(b.location.flatten()))
    .bind(t.starts_at)
    .bind(t.ends_at)
    .bind(all_day)
    .bind(&tz)
    .bind(rule)
    .bind(color(b.color.flatten())?)
    .bind(item)
    .bind(reminder(b.remind_minutes.flatten())?)
    .bind(b.is_private.unwrap_or(false))
    .bind(b.project_id)
    .fetch_one(&mut *tx)
    .await?;
    for g in &guests {
        sqlx::query("INSERT INTO cal_attendees (event_id, user_id) VALUES ($1, $2)").bind(id).bind(g).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let (e, _) = load_events(&state, &[id]).await?.remove(0);
    invite(&state, &auth, &e, &guests, "invited you to").await;
    if let Some(i) = item {
        timeline_event(&state, i, &auth, &e, "planned").await;
    }
    event_view(&state, &auth, id).await.map(Json)
}

async fn invite(state: &AppState, auth: &AuthUser, e: &Event, who: &[Uuid], verb: &str) {
    if who.is_empty() {
        return;
    }
    let me: String = sqlx::query_scalar("SELECT first_name FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await.unwrap_or_default();
    notify(state, who, NewNotification {
        kind: "calendar.invite",
        title: format!("{me} {verb} “{}” on {}", e.title, when(e, e.starts_at)),
        body: e.location.clone(),
        link: Some(format!("/calendar?event={}", e.id)),
        actor: Some(auth.id),
    })
    .await;
}

async fn timeline_event(state: &AppState, item: Uuid, auth: &AuthUser, e: &Event, event: &str) {
    let _ = crate::crm::timeline(&state.db, item, "calendar", Some(auth.id), None, json!({ "event": event, "event_id": e.id, "title": e.title, "start": e.starts_at, "all_day": e.all_day })).await;
}

async fn owned(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<(Event, Vec<Uuid>)> {
    let (e, g) = load_events(state, &[id]).await?.into_iter().next().ok_or(AppError::NotFound("event"))?;
    if e.owner_id != auth.id {
        let hidden_group = match e.project_id {
            Some(p) => !g.contains(&auth.id) && crate::tasks::access::project_access(state, auth, p).await.is_err(),
            None => false,
        };
        return Err(if !hidden_group && (g.contains(&auth.id) || !e.is_private) { AppError::Forbidden } else { AppError::NotFound("event") });
    }
    Ok((e, g))
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<EventInput>) -> AppResult<Json<Value>> {
    let (e, old_guests) = owned(&state, &auth, id).await?;
    let all_day = b.all_day.unwrap_or(e.all_day);
    let t = match (&b.start, &b.end) {
        (Some(s), Some(en)) => times(s, en, all_day)?,
        (None, None) if all_day == e.all_day => Times { starts_at: e.starts_at, ends_at: e.ends_at, all_day },
        _ => return Err(AppError::bad("send both the start and the end")),
    };
    let tz = b.timezone.clone().unwrap_or(e.timezone.clone());
    let _: Tz = tz.parse().map_err(|_| AppError::bad(format!("unknown time zone: {tz}")))?;
    let moved = t.starts_at != e.starts_at || t.ends_at != e.ends_at;
    // The repeat rule follows the start; a changed start keeps the old rule's pattern.
    let rule = match b.repeat {
        Some(r) => r.map(|r| rule_for(r, &t, &tz)).transpose()?,
        None => match e.rule() {
            Some(old) if moved => Some(rule_for(RepeatInput { freq: old.freq, interval: old.interval, weekdays: old.weekdays, month_day: old.month_day, ends_on: old.ends_on, max_runs: old.max_runs }, &t, &tz)?),
            _ => e.recurrence.clone(),
        },
    };
    if let Some(i) = b.item_id.flatten() {
        check_item(&state, &auth, Some(i)).await?;
    }
    let title = b.title.as_deref().map(|x| required("title", x, 300)).transpose()?;
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE cal_events SET title = COALESCE($2, title),
             description = CASE WHEN $3 THEN $4 ELSE description END, location = CASE WHEN $5 THEN $6 ELSE location END,
             starts_at = $7, ends_at = $8, all_day = $9, timezone = $10, recurrence = $11,
             color = CASE WHEN $12 THEN $13 ELSE color END, item_id = CASE WHEN $14 THEN $15 ELSE item_id END,
             remind_minutes = CASE WHEN $16 THEN $17 ELSE remind_minutes END, is_private = COALESCE($18, is_private), updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(title)
    .bind(b.description.is_some())
    .bind(clean(b.description.flatten()))
    .bind(b.location.is_some())
    .bind(clean(b.location.flatten()))
    .bind(t.starts_at)
    .bind(t.ends_at)
    .bind(all_day)
    .bind(&tz)
    .bind(rule)
    .bind(b.color.is_some())
    .bind(color(b.color.flatten())?)
    .bind(b.item_id.is_some())
    .bind(b.item_id.flatten())
    .bind(b.remind_minutes.is_some())
    .bind(reminder(b.remind_minutes.flatten())?)
    .bind(b.is_private)
    .execute(&mut *tx)
    .await?;
    let mut added = Vec::new();
    if let Some(mut guests) = b.attendees {
        guests.retain(|g| *g != auth.id);
        guests.sort();
        guests.dedup();
        check_guests(&state, &guests).await?;
        sqlx::query("DELETE FROM cal_attendees WHERE event_id = $1 AND NOT user_id = ANY($2)").bind(id).bind(&guests).execute(&mut *tx).await?;
        for g in &guests {
            if !old_guests.contains(g) {
                sqlx::query("INSERT INTO cal_attendees (event_id, user_id) VALUES ($1, $2)").bind(id).bind(g).execute(&mut *tx).await?;
                added.push(*g);
            }
        }
    }
    // A new time asks everyone again.
    if moved {
        sqlx::query("UPDATE cal_attendees SET status = 'pending' WHERE event_id = $1").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM cal_reminders_sent WHERE event_id = $1").bind(id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    crate::im::sync_linked_chat(&state, crate::im::Link::Event(id)).await?;
    let (e, guests) = load_events(&state, &[id]).await?.remove(0);
    invite(&state, &auth, &e, &added, "invited you to").await;
    if moved {
        let others: Vec<Uuid> = guests.iter().copied().filter(|g| !added.contains(g)).collect();
        invite(&state, &auth, &e, &others, "moved").await;
    }
    event_view(&state, &auth, id).await.map(Json)
}

/// The event's chat (made on first use), for its organiser and guests: a call in it is the
/// meeting's call.
async fn event_chat(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    event_view(&state, &auth, id).await?;
    let link = crate::im::Link::Event(id);
    if !link.has(&state, auth.id).await? {
        return Err(AppError::bad("only the organiser and guests can use the event's chat"));
    }
    Ok(Json(json!({ "chat_id": crate::im::linked_chat(&state, link).await? })))
}

#[derive(Deserialize)]
struct RemoveQuery {
    /// Remove only this occurrence of a repeating event.
    occurrence: Option<DateTime<Utc>>,
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<RemoveQuery>) -> AppResult<Json<Value>> {
    let (e, guests) = owned(&state, &auth, id).await?;
    let me: String = sqlx::query_scalar("SELECT first_name FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await?;
    match q.occurrence.filter(|_| e.recurrence.is_some()) {
        Some(occ) => {
            sqlx::query("INSERT INTO cal_exceptions (event_id, occurrence_start) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(id).bind(occ).execute(&state.db).await?;
            notify(&state, &guests, NewNotification { kind: "calendar.cancelled", title: format!("{me} cancelled “{}” on {}", e.title, when(&e, occ)), body: None, link: Some("/calendar".into()), actor: Some(auth.id) }).await;
        }
        None => {
            let chat = crate::im::linked_chat_people(&state, crate::im::Link::Event(id)).await?;
            sqlx::query("DELETE FROM cal_events WHERE id = $1").bind(id).execute(&state.db).await?;
            crate::im::linked_chat_gone(&state, chat).await;
            notify(&state, &guests, NewNotification { kind: "calendar.cancelled", title: format!("{me} cancelled “{}”", e.title), body: None, link: Some("/calendar".into()), actor: Some(auth.id) }).await;
        }
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct RespondBody {
    status: String,
}

async fn respond(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<RespondBody>) -> AppResult<Json<Value>> {
    if !["accepted", "declined", "tentative"].contains(&b.status.as_str()) {
        return Err(AppError::bad("answer accepted, declined or tentative"));
    }
    let n = sqlx::query("UPDATE cal_attendees SET status = $3 WHERE event_id = $1 AND user_id = $2")
        .bind(id)
        .bind(auth.id)
        .bind(&b.status)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("invitation"));
    }
    let (e, _) = load_events(&state, &[id]).await?.remove(0);
    let me: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await?;
    let verb = match b.status.as_str() { "accepted" => "accepted", "declined" => "declined", _ => "might come to" };
    notify(&state, &[e.owner_id], NewNotification { kind: "calendar.answer", title: format!("{me} {verb} “{}”", e.title), body: None, link: Some(format!("/calendar?event={id}")), actor: Some(auth.id) }).await;
    event_view(&state, &auth, id).await.map(Json)
}

// ---------------------------------------------------------------------------
// Subscription link
// ---------------------------------------------------------------------------

fn hash(t: &str) -> String {
    hex::encode(Sha256::digest(t.as_bytes()))
}

async fn feed_info(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let has: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM cal_feeds WHERE user_id = $1)").bind(auth.id).fetch_one(&state.db).await?;
    Ok(Json(json!({ "active": has })))
}

/// Makes a new link (the old one stops working). The link is shown only now.
async fn new_feed(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let token = random_token(24);
    sqlx::query("INSERT INTO cal_feeds (user_id, token_hash) VALUES ($1, $2) ON CONFLICT (user_id) DO UPDATE SET token_hash = $2, created_at = now()")
        .bind(auth.id)
        .bind(hash(&token))
        .execute(&state.db)
        .await?;
    let base = state.config.public_url.clone();
    Ok(Json(json!({ "active": true, "url": format!("{base}/api/public/calendar/{token}.ics"), "webcal": format!("{}/api/public/calendar/{token}.ics", base.replacen("https://", "webcal://", 1).replacen("http://", "webcal://", 1)) })))
}

async fn drop_feed(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    sqlx::query("DELETE FROM cal_feeds WHERE user_id = $1").bind(auth.id).execute(&state.db).await?;
    Ok(Json(json!({ "active": false })))
}

// ---------------------------------------------------------------------------
// Reminders
// ---------------------------------------------------------------------------

/// Every minute: reminds the owner and guests who haven't declined.
pub async fn run_reminders(state: &AppState) -> Result<()> {
    let now = Utc::now();
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM cal_events WHERE remind_minutes IS NOT NULL
           AND starts_at <= now() + make_interval(mins => remind_minutes + 1)
           AND (recurrence IS NOT NULL OR starts_at >= now() - interval '5 minutes')",
    )
    .fetch_all(&state.db)
    .await?;
    if ids.is_empty() {
        return Ok(());
    }
    let events = load_events(state, &ids).await?;
    let ex = exceptions(state, &ids).await?;
    for (e, guests) in events {
        let lead = Duration::minutes(e.remind_minutes.unwrap_or(0) as i64);
        let excs: Vec<DateTime<Utc>> = ex.iter().filter(|(id, _)| *id == e.id).map(|(_, t)| *t).collect();
        // Occurrences starting from now until the reminder lead time ahead.
        for occ in occurrences(&e, &excs, now, now + lead + Duration::minutes(1)).into_iter().filter(|o| *o >= now && *o - lead <= now) {
            let declined: Vec<Uuid> = sqlx::query_scalar("SELECT user_id FROM cal_attendees WHERE event_id = $1 AND status = 'declined'").bind(e.id).fetch_all(&state.db).await?;
            let mut who = vec![e.owner_id];
            who.extend(guests.iter().copied().filter(|g| !declined.contains(g)));
            for u in who {
                let fresh = sqlx::query("INSERT INTO cal_reminders_sent (event_id, user_id, occurrence_start) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
                    .bind(e.id)
                    .bind(u)
                    .bind(occ)
                    .execute(&state.db)
                    .await?
                    .rows_affected();
                if fresh == 1 {
                    let mins = (occ - now).num_minutes().max(0);
                    let title = if e.all_day { format!("Today: {}", e.title) } else if mins == 0 { format!("Now: {}", e.title) } else { format!("In {mins} min: {}", e.title) };
                    notify(state, &[u], NewNotification { kind: "calendar.reminder", title, body: e.location.clone(), link: Some(format!("/calendar?event={}", e.id)), actor: None }).await;
                }
            }
        }
    }
    sqlx::query("DELETE FROM cal_reminders_sent WHERE sent_at < now() - interval '60 days'").execute(&state.db).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(start: &str, mins: i64, rule: Option<Value>) -> Event {
        let s = DateTime::parse_from_rfc3339(start).unwrap().with_timezone(&Utc);
        Event {
            id: Uuid::nil(), owner_id: Uuid::nil(), title: "x".into(), description: None, location: None, starts_at: s, ends_at: s + Duration::minutes(mins),
            all_day: false, timezone: "Europe/Berlin".into(), recurrence: rule, color: None, item_id: None, remind_minutes: None, is_private: false, project_id: None,
        }
    }
    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn single_event_overlap() {
        let e = ev("2026-03-02T09:00:00Z", 60, None);
        assert_eq!(occurrences(&e, &[], at("2026-03-02T09:30:00Z"), at("2026-03-03T00:00:00Z")).len(), 1);
        assert!(occurrences(&e, &[], at("2026-03-02T10:00:00Z"), at("2026-03-03T00:00:00Z")).is_empty());
    }

    #[test]
    fn weekly_repeats_keep_local_time_and_skip_exceptions() {
        // Mondays 10:00 Berlin, from 2 March 2026 (winter time, UTC+1), across the DST change.
        let rule = json!({ "freq": "weekly", "interval": 1, "weekdays": [1], "time": "10:00", "timezone": "Europe/Berlin", "starts_on": "2026-03-02", "max_runs": 5 });
        let e = ev("2026-03-02T09:00:00Z", 30, Some(rule));
        let all = occurrences(&e, &[], at("2026-03-01T00:00:00Z"), at("2026-06-01T00:00:00Z"));
        assert_eq!(all.len(), 5);
        assert_eq!(all[4], at("2026-03-30T08:00:00Z")); // summer time: 10:00 = 08:00 UTC
        let some = occurrences(&e, &[at("2026-03-09T09:00:00Z")], at("2026-03-01T00:00:00Z"), at("2026-03-20T00:00:00Z"));
        assert_eq!(some, vec![at("2026-03-02T09:00:00Z"), at("2026-03-16T09:00:00Z")]);
    }
}
