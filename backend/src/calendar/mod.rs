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

mod access;
pub mod dav;
mod ical;
mod ics;
mod invites;
mod slots;
pub mod sync;

pub use access::Level;

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
        .route("/calendar/rooms", get(rooms))
        .merge(ics::router())
        .merge(access::router())
        .merge(invites::router())
        .merge(slots::router())
        .merge(sync::router())
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
    /// Raised when the time, place or title changes (for guests' calendar apps).
    pub sequence: i32,
    pub room_id: Option<Uuid>,
    pub slot_link_id: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    /// The iCalendar UID a calendar app gave it (else "<id>@nerp").
    pub uid: Option<String>,
    /// A changed occurrence of a repeating event: the event, and which occurrence it replaces.
    pub parent_id: Option<Uuid>,
    pub recurrence_id: Option<DateTime<Utc>>,
}

const EVENT_COLUMNS: &str = "e.id, e.owner_id, e.title, e.description, e.location, e.starts_at, e.ends_at, e.all_day, e.timezone, e.recurrence, e.color, e.item_id, e.remind_minutes, e.is_private, e.project_id, e.sequence, e.room_id, e.slot_link_id, e.updated_at, e.uid, e.parent_id, e.recurrence_id";

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
    // What I may see of each colleague's calendar; closed ones are left out.
    let mut levels: std::collections::HashMap<Uuid, Level> = std::collections::HashMap::new();
    let mut closed: Vec<Uuid> = Vec::new();
    for u in users.clone() {
        let l = access::level(&state, auth.id, u).await?;
        if l == Level::None {
            closed.push(u);
        } else {
            levels.insert(u, l);
        }
    }
    users.retain(|u| !closed.contains(u));
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
    let rooms = room_names(&state, events.iter().filter_map(|(e, _)| e.room_id).collect()).await?;
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
                // Group calendars show details to the group; a colleague's calendar as they allow.
                let lvl = if group.is_some() { Level::Read } else { levels.get(u).copied().unwrap_or(Level::Busy) };
                let busy = !mine && (lvl <= Level::Busy || (e.is_private && lvl < Level::Edit));
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
                    location: if busy { None } else { place_of(e, &rooms) },
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
        "people": people.into_iter().map(|(id, name, a)| json!({ "id": id, "name": name, "avatar_file_id": a, "access": levels.get(&id).map(|l| l.name()) })).collect::<Vec<_>>(),
        "closed": closed,
    })))
}

/// Names of meeting rooms.
async fn room_names(state: &AppState, ids: Vec<Uuid>) -> AppResult<std::collections::HashMap<Uuid, String>> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, name FROM booking_resources WHERE id = ANY($1)").bind(&ids).fetch_all(&state.db).await?;
    Ok(rows.into_iter().collect())
}

/// Where it is: the room, then the typed place.
fn place_of(e: &Event, rooms: &std::collections::HashMap<Uuid, String>) -> Option<String> {
    let room = e.room_id.and_then(|r| rooms.get(&r).cloned());
    match (room, e.location.clone()) {
        (Some(r), Some(l)) if !l.eq_ignore_ascii_case(&r) => Some(format!("{r}, {l}")),
        (Some(r), _) => Some(r),
        (None, l) => l,
    }
}

pub(crate) async fn place(state: &AppState, e: &Event) -> AppResult<Option<String>> {
    let rooms = room_names(state, e.room_id.into_iter().collect()).await?;
    Ok(place_of(e, &rooms))
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
    let lvl = access::level(state, auth.id, e.owner_id).await?;
    if !mine {
        // An event on a workgroup's calendar: for those who can see the group; others as
        // the owner shares their calendar.
        let allowed = match e.project_id {
            Some(g) => crate::tasks::access::project_access(state, auth, g).await.is_ok(),
            None => lvl >= Level::Read,
        };
        if !allowed || (e.is_private && lvl < Level::Edit) {
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
    let room: Option<(Uuid, String)> = match e.room_id {
        Some(r) => sqlx::query_as("SELECT id, name FROM booking_resources WHERE id = $1").bind(r).fetch_optional(&state.db).await?,
        None => None,
    };
    let email_guests = invites::guests_of(state, e.id).await?;
    Ok(json!({
        "id": e.id, "owner_id": e.owner_id, "sequence": e.sequence,
        "room": room.map(|(id, name)| json!({ "id": id, "name": name })),
        "email_guests": email_guests.iter().map(|g| json!({ "id": g.id, "email": g.email, "name": g.name, "status": g.status })).collect::<Vec<_>>(),
        "slot_link_id": e.slot_link_id,
        // A changed occurrence of a repeating event (made in a calendar app).
        "parent_id": e.parent_id, "title": e.title, "description": e.description, "location": e.location,
        "starts_at": e.starts_at, "ends_at": e.ends_at, "all_day": e.all_day, "timezone": e.timezone,
        "recurrence": e.recurrence, "repeat": rule.map(|r| r.describe()), "color": e.color, "remind_minutes": e.remind_minutes, "is_private": e.is_private,
        "project": project,
        "item": e.item_id.zip(item).map(|(id, (code, title))| json!({ "id": id, "code": code, "title": title })),
        "people": people.into_iter().map(|(id, name, a, status)| json!({ "id": id, "name": name, "avatar_file_id": a, "status": status, "owner": id == e.owner_id })).collect::<Vec<_>>(),
        "can_edit": lvl == Level::Edit,
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
    #[serde(default)]
    month: Option<u8>,
    #[serde(default)]
    week_of_month: Option<i8>,
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
    /// A meeting room (a booking resource of kind room).
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    room_id: Option<Option<Uuid>>,
    /// Guests outside the company, by email (replaces the list).
    email_guests: Option<Vec<invites::GuestInput>>,
    /// Whose calendar to put it on (when creating): one's own, or a colleague's who lets one
    /// change their calendar.
    calendar_id: Option<Uuid>,
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
        month: r.month,
        week_of_month: r.week_of_month,
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
    // A room is checked and taken under its lock (shared with bookings).
    let rooms: Vec<Uuid> = b.room_id.flatten().into_iter().collect();
    let st = state.clone();
    crate::booking::with_resource_locks(&state, &rooms, || create_locked(st, auth, b)).await
}

async fn create_locked(state: AppState, auth: AuthUser, b: EventInput) -> AppResult<Json<Value>> {
    let title = required("title", b.title.as_deref().unwrap_or(""), 300)?;
    let all_day = b.all_day.unwrap_or(false);
    let t = times(b.start.as_deref().unwrap_or(""), b.end.as_deref().unwrap_or(""), all_day)?;
    let tz = b.timezone.clone().unwrap_or_else(|| "UTC".into());
    let _: Tz = tz.parse().map_err(|_| AppError::bad(format!("unknown time zone: {tz}")))?;
    let rule = b.repeat.flatten().map(|r| rule_for(r, &t, &tz)).transpose()?;
    let item = b.item_id.flatten();
    check_item(&state, &auth, item).await?;
    // On a colleague's calendar only with their leave to change it.
    let owner = b.calendar_id.unwrap_or(auth.id);
    if owner != auth.id && access::level(&state, auth.id, owner).await? < Level::Edit {
        return Err(AppError::Forbidden);
    }
    let mut guests = b.attendees.unwrap_or_default();
    guests.retain(|g| *g != owner);
    guests.sort();
    guests.dedup();
    check_guests(&state, &guests).await?;
    let email_guests = invites::clean_guests(b.email_guests.unwrap_or_default())?;
    if let Some(g) = b.project_id {
        if !crate::tasks::access::project_access(&state, &auth, g).await?.contribute {
            return Err(AppError::Forbidden);
        }
    }
    let room = b.room_id.flatten();
    if let Some(r) = room {
        let draft = Event {
            id: Uuid::nil(), owner_id: owner, title: title.clone(), description: None, location: None, starts_at: t.starts_at, ends_at: t.ends_at, all_day,
            timezone: tz.clone(), recurrence: rule.clone(), color: None, item_id: None, remind_minutes: None, is_private: false, project_id: None,
            sequence: 0, room_id: Some(r), slot_link_id: None, updated_at: Utc::now(), uid: None, parent_id: None, recurrence_id: None,
        };
        check_room(&state, r, &draft, &[]).await?;
    }
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO cal_events (owner_id, title, description, location, starts_at, ends_at, all_day, timezone, recurrence, color, item_id, remind_minutes, is_private, project_id, created_by, room_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) RETURNING id",
    )
    .bind(owner)
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
    .bind(auth.id)
    .bind(room)
    .fetch_one(&mut *tx)
    .await?;
    for g in &guests {
        sqlx::query("INSERT INTO cal_attendees (event_id, user_id) VALUES ($1, $2)").bind(id).bind(g).execute(&mut *tx).await?;
    }
    for g in &email_guests {
        sqlx::query("INSERT INTO cal_email_guests (event_id, email, name) VALUES ($1, $2, $3)").bind(id).bind(&g.email).bind(&g.name).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let (e, _) = load_events(&state, &[id]).await?.remove(0);
    invite(&state, auth.id, &e, &guests, "invited you to").await;
    if owner != auth.id {
        invite(&state, auth.id, &e, &[owner], "put on your calendar").await;
    }
    invites::send(&state, &e, &invites::guests_of(&state, id).await?, invites::Change::Invited).await;
    if let Some(i) = item {
        timeline_event(&state, i, &auth, &e, "planned").await;
    }
    event_view(&state, &auth, id).await.map(Json)
}

pub(crate) async fn invite(state: &AppState, actor: Uuid, e: &Event, who: &[Uuid], verb: &str) {
    if who.is_empty() {
        return;
    }
    let me: String = sqlx::query_scalar("SELECT first_name FROM users WHERE id = $1").bind(actor).fetch_one(&state.db).await.unwrap_or_default();
    notify(state, who, NewNotification {
        kind: "calendar.invite",
        title: format!("{me} {verb} “{}” on {}", e.title, when(e, e.starts_at)),
        body: e.location.clone(),
        link: Some(format!("/calendar?event={}", e.id)),
        actor: Some(actor),
    })
    .await;
}

async fn timeline_event(state: &AppState, item: Uuid, auth: &AuthUser, e: &Event, event: &str) {
    let _ = crate::crm::timeline(&state.db, item, "calendar", Some(auth.id), None, json!({ "event": event, "event_id": e.id, "title": e.title, "start": e.starts_at, "all_day": e.all_day })).await;
}

async fn owned(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<(Event, Vec<Uuid>)> {
    let (e, g) = load_events(state, &[id]).await?.into_iter().next().ok_or(AppError::NotFound("event"))?;
    if e.owner_id != auth.id && access::level(state, auth.id, e.owner_id).await? < Level::Edit {
        let hidden_group = match e.project_id {
            Some(p) => !g.contains(&auth.id) && crate::tasks::access::project_access(state, auth, p).await.is_err(),
            None => false,
        };
        return Err(if !hidden_group && (g.contains(&auth.id) || !e.is_private) { AppError::Forbidden } else { AppError::NotFound("event") });
    }
    Ok((e, g))
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<EventInput>) -> AppResult<Json<Value>> {
    let current: Option<Uuid> = sqlx::query_scalar("SELECT room_id FROM cal_events WHERE id = $1").bind(id).fetch_optional(&state.db).await?.flatten();
    let rooms: Vec<Uuid> = b.room_id.flatten().into_iter().chain(current).collect();
    let st = state.clone();
    crate::booking::with_resource_locks(&state, &rooms, || update_locked(st, auth, id, b)).await
}

async fn update_locked(state: AppState, auth: AuthUser, id: Uuid, b: EventInput) -> AppResult<Json<Value>> {
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
            Some(old) if moved => Some(rule_for(RepeatInput { freq: old.freq, interval: old.interval, weekdays: old.weekdays, month_day: old.month_day, month: old.month, week_of_month: old.week_of_month, ends_on: old.ends_on, max_runs: old.max_runs }, &t, &tz)?),
            _ => e.recurrence.clone(),
        },
    };
    if let Some(i) = b.item_id.flatten() {
        check_item(&state, &auth, Some(i)).await?;
    }
    let title = b.title.as_deref().map(|x| required("title", x, 300)).transpose()?;
    let room = match b.room_id {
        Some(r) => r,
        None => e.room_id,
    };
    let location = match &b.location {
        Some(l) => clean(l.clone()),
        None => e.location.clone(),
    };
    // Guests' calendar apps need to hear about changes to the time, place, title or repeats.
    let changed = moved || room != e.room_id || location != e.location || title.as_ref().is_some_and(|t| *t != e.title) || rule != e.recurrence || all_day != e.all_day;
    if let Some(r) = room {
        if moved || room != e.room_id || rule != e.recurrence {
            let draft = Event { starts_at: t.starts_at, ends_at: t.ends_at, all_day, timezone: tz.clone(), recurrence: rule.clone(), room_id: Some(r), ..e.clone() };
            let ex: Vec<DateTime<Utc>> = exceptions(&state, &[id]).await?.into_iter().map(|(_, t)| t).collect();
            check_room(&state, r, &draft, &ex).await?;
        }
    }
    let email_guests = b.email_guests.map(invites::clean_guests).transpose()?;
    let old_email = invites::guests_of(&state, id).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE cal_events SET title = COALESCE($2, title),
             description = CASE WHEN $3 THEN $4 ELSE description END, location = CASE WHEN $5 THEN $6 ELSE location END,
             starts_at = $7, ends_at = $8, all_day = $9, timezone = $10, recurrence = $11,
             color = CASE WHEN $12 THEN $13 ELSE color END, item_id = CASE WHEN $14 THEN $15 ELSE item_id END,
             remind_minutes = CASE WHEN $16 THEN $17 ELSE remind_minutes END, is_private = COALESCE($18, is_private),
             room_id = $19, sequence = sequence + CASE WHEN $20 THEN 1 ELSE 0 END, updated_at = now()
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
    .bind(room)
    .bind(changed)
    .execute(&mut *tx)
    .await?;
    // Guests by email: the removed ones are told it's cancelled (after the save), new ones invited.
    let mut email_removed = Vec::new();
    if let Some(list) = &email_guests {
        for g in &old_email {
            if !list.iter().any(|x| x.email == g.email) {
                email_removed.push(g.clone());
            }
        }
        let gone: Vec<Uuid> = email_removed.iter().map(|g| g.id).collect();
        sqlx::query("DELETE FROM cal_email_guests WHERE id = ANY($1)").bind(&gone).execute(&mut *tx).await?;
        for g in list {
            if !old_email.iter().any(|x| x.email == g.email) {
                sqlx::query("INSERT INTO cal_email_guests (event_id, email, name) VALUES ($1, $2, $3)").bind(id).bind(&g.email).bind(&g.name).execute(&mut *tx).await?;
            }
        }
    }
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
        sqlx::query("UPDATE cal_email_guests SET status = 'pending', answered_at = NULL WHERE event_id = $1").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM cal_reminders_sent WHERE event_id = $1").bind(id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    crate::im::sync_linked_chat(&state, crate::im::Link::Event(id)).await?;
    let (e, guests) = load_events(&state, &[id]).await?.remove(0);
    invite(&state, auth.id, &e, &added, "invited you to").await;
    if moved {
        let others: Vec<Uuid> = guests.iter().copied().filter(|g| !added.contains(g)).collect();
        invite(&state, auth.id, &e, &others, "moved").await;
    }
    // A colleague changing my calendar tells me.
    if e.owner_id != auth.id {
        invite(&state, auth.id, &e, &[e.owner_id], "changed").await;
    }
    invites::send(&state, &e, &email_removed, invites::Change::Cancelled).await;
    let now_email = invites::guests_of(&state, id).await?;
    let (fresh, kept): (Vec<_>, Vec<_>) = now_email.into_iter().partition(|g| !old_email.iter().any(|x| x.email == g.email));
    invites::send(&state, &e, &fresh, invites::Change::Invited).await;
    if changed {
        invites::send(&state, &e, &kept, invites::Change::Updated).await;
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
    let (e, mut guests) = owned(&state, &auth, id).await?;
    if e.owner_id != auth.id {
        guests.push(e.owner_id);
    }
    let email = invites::guests_of(&state, id).await?;
    let me: String = sqlx::query_scalar("SELECT first_name FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await?;
    match q.occurrence.filter(|_| e.recurrence.is_some()) {
        Some(occ) => {
            sqlx::query("INSERT INTO cal_exceptions (event_id, occurrence_start) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(id).bind(occ).execute(&state.db).await?;
            sqlx::query("UPDATE cal_events SET sequence = sequence + 1, updated_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
            let e2 = load_events(&state, &[id]).await?.remove(0).0;
            invites::send(&state, &e2, &email, invites::Change::OccurrenceCancelled(occ)).await;
            notify(&state, &guests, NewNotification { kind: "calendar.cancelled", title: format!("{me} cancelled “{}” on {}", e.title, when(&e, occ)), body: None, link: Some("/calendar".into()), actor: Some(auth.id) }).await;
        }
        None => {
            let chat = crate::im::linked_chat_people(&state, crate::im::Link::Event(id)).await?;
            invites::send(&state, &Event { sequence: e.sequence + 1, ..e.clone() }, &email, invites::Change::Cancelled).await;
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
    answer(&state, auth.id, id, &b.status).await?;
    event_view(&state, &auth, id).await.map(Json)
}

/// A guest's answer (from NERP or their calendar app); the organiser is told when it changes.
pub(crate) async fn answer(state: &AppState, user: Uuid, id: Uuid, status: &str) -> AppResult<()> {
    if !["accepted", "declined", "tentative"].contains(&status) {
        return Err(AppError::bad("answer accepted, declined or tentative"));
    }
    let old: Option<String> = sqlx::query_scalar("SELECT status FROM cal_attendees WHERE event_id = $1 AND user_id = $2").bind(id).bind(user).fetch_optional(&state.db).await?;
    let Some(old) = old else {
        return Err(AppError::NotFound("invitation"));
    };
    if old == status {
        return Ok(());
    }
    sqlx::query("UPDATE cal_attendees SET status = $3 WHERE event_id = $1 AND user_id = $2").bind(id).bind(user).bind(status).execute(&state.db).await?;
    let (e, _) = load_events(state, &[id]).await?.remove(0);
    let me: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(user).fetch_one(&state.db).await?;
    let verb = match status { "accepted" => "accepted", "declined" => "declined", _ => "might come to" };
    notify(state, &[e.owner_id], NewNotification { kind: "calendar.answer", title: format!("{me} {verb} “{}”", e.title), body: None, link: Some(format!("/calendar?event={id}")), actor: Some(user) }).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Meeting rooms (the booking module's rooms)
// ---------------------------------------------------------------------------

/// Events holding a room during [from, to): one interval per occurrence (`except`: an event
/// being changed).
pub(crate) async fn room_events(state: &AppState, room: Uuid, from: DateTime<Utc>, to: DateTime<Utc>, except: Option<Uuid>) -> AppResult<Vec<(DateTime<Utc>, DateTime<Utc>)>> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM cal_events WHERE room_id = $1 AND ($4::uuid IS NULL OR id <> $4) AND starts_at < $3 AND (recurrence IS NOT NULL OR ends_at > $2)",
    )
    .bind(room)
    .bind(from)
    .bind(to)
    .bind(except)
    .fetch_all(&state.db)
    .await?;
    let events = load_events(state, &ids).await?;
    let ex = exceptions(state, &ids).await?;
    let mut out = Vec::new();
    for (e, _) in &events {
        let excs: Vec<DateTime<Utc>> = ex.iter().filter(|(id, _)| *id == e.id).map(|(_, t)| *t).collect();
        for s in occurrences(e, &excs, from, to) {
            out.push((s, s + (e.ends_at - e.starts_at)));
        }
    }
    Ok(out)
}

/// Everything holding a room during [from, to): events and bookings, each with the room's
/// buffer after it.
async fn room_busy(state: &AppState, r: &crate::booking::Resource, from: DateTime<Utc>, to: DateTime<Utc>, except: Option<Uuid>) -> AppResult<Vec<(DateTime<Utc>, DateTime<Utc>)>> {
    let buf = Duration::minutes(r.buffer_minutes as i64);
    let mut out: Vec<(DateTime<Utc>, DateTime<Utc>)> = room_events(state, r.id, from - buf, to, except).await?.into_iter().map(|(s, e)| (s, e + buf)).collect();
    let bookings: Vec<(DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT b.starts_at, b.ends_at FROM bookings b JOIN booking_resources_used u ON u.booking_id = b.id
         WHERE u.resource_id = $1 AND b.status IN ('booked', 'confirmed', 'came') AND b.starts_at < $3 AND b.ends_at + $4 > $2",
    )
    .bind(r.id)
    .bind(from)
    .bind(to)
    .bind(buf)
    .fetch_all(&state.db)
    .await?;
    out.extend(bookings.into_iter().map(|(s, e)| (s, e + buf)));
    Ok(out)
}

/// Refuses a room that isn't a meeting room, or is fully taken at any occurrence of the event
/// (within the next 400 days for a repeating one).
async fn check_room(state: &AppState, room: Uuid, e: &Event, excs: &[DateTime<Utc>]) -> AppResult<()> {
    let r = crate::booking::load_resource(state, room).await.map_err(|_| AppError::bad("choose a meeting room"))?;
    if r.kind != "room" || !r.is_active {
        return Err(AppError::bad("choose a meeting room"));
    }
    let dur = e.ends_at - e.starts_at;
    let horizon = if e.recurrence.is_some() { e.starts_at.max(Utc::now()) + Duration::days(400) } else { e.ends_at + Duration::seconds(1) };
    let from = if e.recurrence.is_some() { e.starts_at.max(Utc::now() - Duration::days(1)) } else { e.starts_at };
    let occs = occurrences(e, excs, from, horizon);
    let (Some(first), Some(last)) = (occs.first().copied(), occs.last().copied()) else {
        return Ok(());
    };
    let except = (e.id != Uuid::nil()).then_some(e.id);
    let busy = room_busy(state, &r, first, last + dur, except).await?;
    let buf = Duration::minutes(r.buffer_minutes as i64);
    for o in occs {
        let n = busy.iter().filter(|(s, en)| *s < o + dur + buf && *en > o).count();
        if n as i32 >= r.capacity {
            return Err(AppError::Conflict(format!("{} is taken on {}", r.name, when(e, o))));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct RoomsQuery {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    /// An event being changed (its own time doesn't count).
    except: Option<Uuid>,
}

/// Meeting rooms with what holds them during [from, to) and whether they are free then.
async fn rooms(State(state): State<AppState>, _auth: AuthUser, Query(q): Query<RoomsQuery>) -> AppResult<Json<Value>> {
    if q.to < q.from || q.to - q.from > Duration::days(31) {
        return Err(AppError::bad("ask for at most 31 days"));
    }
    let list: Vec<crate::booking::Resource> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{} WHERE r.kind = 'room' AND r.is_active ORDER BY r.sort, r.name", crate::booking::RESOURCE_SELECT)))
        .fetch_all(&state.db)
        .await?;
    let mut out = Vec::new();
    for r in &list {
        let busy = room_busy(&state, r, q.from, q.to, q.except).await?;
        let taken = busy.iter().filter(|(s, e)| *s < q.to && *e > q.from).count() as i32;
        out.push(json!({
            "id": r.id, "name": r.name, "description": r.description, "capacity": r.capacity, "color": r.color,
            "free": taken < r.capacity,
            "busy": busy.iter().map(|(s, e)| json!({ "start": s, "end": e })).collect::<Vec<_>>(),
        }));
    }
    Ok(Json(json!({ "items": out })))
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
            sequence: 0, room_id: None, slot_link_id: None, updated_at: s, uid: None, parent_id: None, recurrence_id: None,
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
