//! Slot links: a page at `/meet/<slug>` where someone (a client, a candidate, a colleague)
//! picks a free time, and a meeting is made on the owner's calendar.
//!
//! A link offers its owner's open slots: times within the link's hours that are free in their
//! calendar (events they haven't declined, bookings of them, CRM calls and meetings, approved
//! absences and company holidays). With members it offers shared slots: only times when the
//! owner and every member are free, and the meeting invites them all. The person booking gets
//! an email invitation (with an answer link, through which they can also decline).

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch},
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{access, exceptions, load_events, occurrences};
use crate::{
    auth::{AuthUser, ClientIp},
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::{clean, required, slugify},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/calendar/slot-links", get(list).post(create))
        .route("/calendar/slot-links/{id}", patch(update).delete(remove))
        .route("/calendar/slot-links/{id}/for-record", axum::routing::post(for_record))
        .route("/public/calendar/meet/{slug}", get(page).post(book))
        .route("/public/calendar/meet/{slug}/slots", get(slots))
}

#[derive(sqlx::FromRow, Clone)]
struct Link {
    id: Uuid,
    owner_id: Uuid,
    slug: String,
    title: String,
    description: Option<String>,
    location: Option<String>,
    duration_minutes: i32,
    members: Vec<Uuid>,
    hours: Value,
    timezone: String,
    slot_step_minutes: i32,
    buffer_minutes: i32,
    min_notice_minutes: i32,
    days_ahead: i32,
    max_per_day: Option<i32>,
    remind_minutes: Option<i32>,
    is_active: bool,
}

const COLS: &str = "id, owner_id, slug, title, description, location, duration_minutes, members, hours, timezone, slot_step_minutes, buffer_minutes, min_notice_minutes, days_ahead, max_per_day, remind_minutes, is_active";

impl Link {
    fn tz(&self) -> Tz {
        self.timezone.parse().unwrap_or(chrono_tz::UTC)
    }
    fn hours(&self) -> Vec<(u32, i32, i32)> {
        serde_json::from_value(self.hours.clone()).unwrap_or_default()
    }
    /// Everyone who must be free.
    fn people(&self) -> Vec<Uuid> {
        let mut p = vec![self.owner_id];
        p.extend(self.members.iter().copied().filter(|m| *m != self.owner_id));
        p
    }
}

async fn names(state: &AppState, ids: &[Uuid]) -> AppResult<Vec<(Uuid, String, Option<Uuid>)>> {
    Ok(sqlx::query_as("SELECT id, trim(first_name || ' ' || last_name), avatar_file_id FROM users WHERE id = ANY($1) ORDER BY array_position($1, id)").bind(ids).fetch_all(&state.db).await?)
}

async fn link_json(state: &AppState, l: &Link) -> AppResult<Value> {
    let people = names(state, &l.people()).await?;
    Ok(json!({
        "id": l.id, "owner_id": l.owner_id, "slug": l.slug, "title": l.title, "description": l.description, "location": l.location,
        "duration_minutes": l.duration_minutes, "members": l.members, "hours": l.hours, "timezone": l.timezone,
        "slot_step_minutes": l.slot_step_minutes, "buffer_minutes": l.buffer_minutes, "min_notice_minutes": l.min_notice_minutes,
        "days_ahead": l.days_ahead, "max_per_day": l.max_per_day, "remind_minutes": l.remind_minutes, "is_active": l.is_active,
        "url": format!("{}/meet/{}", state.config.public_url, l.slug),
        "people": people.into_iter().map(|(id, name, a)| json!({ "id": id, "name": name, "avatar_file_id": a })).collect::<Vec<_>>(),
    }))
}

async fn load(state: &AppState, id: Uuid) -> AppResult<Link> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM cal_slot_links WHERE id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("slot link"))
}

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let links: Vec<Link> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM cal_slot_links WHERE owner_id = $1 OR $1 = ANY(members) ORDER BY created_at")))
        .bind(auth.id)
        .fetch_all(&state.db)
        .await?;
    let mut out = Vec::new();
    for l in &links {
        out.push(link_json(&state, l).await?);
    }
    Ok(Json(json!({ "items": out })))
}

#[derive(Deserialize)]
struct LinkInput {
    title: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    location: Option<Option<String>>,
    duration_minutes: Option<i32>,
    members: Option<Vec<Uuid>>,
    hours: Option<Vec<(u32, i32, i32)>>,
    timezone: Option<String>,
    slot_step_minutes: Option<i32>,
    buffer_minutes: Option<i32>,
    min_notice_minutes: Option<i32>,
    days_ahead: Option<i32>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    max_per_day: Option<Option<i32>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    remind_minutes: Option<Option<i32>>,
    is_active: Option<bool>,
}

fn check_hours(h: &[(u32, i32, i32)]) -> AppResult<()> {
    if h.is_empty() || h.len() > 21 {
        return Err(AppError::bad("give one to 21 time ranges"));
    }
    for (d, from, to) in h {
        if !(1..=7).contains(d) || !(0..1440).contains(from) || !(1..=1440).contains(to) || from >= to {
            return Err(AppError::bad("each range is a weekday (1–7) and minutes from and to within the day"));
        }
    }
    Ok(())
}

/// Members must be active colleagues whose calendar the owner may at least see as busy times.
async fn check_members(state: &AppState, owner: Uuid, members: &[Uuid]) -> AppResult<Vec<Uuid>> {
    let mut m: Vec<Uuid> = members.iter().copied().filter(|x| *x != owner).collect();
    m.sort();
    m.dedup();
    if m.len() > 10 {
        return Err(AppError::bad("at most 10 members"));
    }
    let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND is_active").bind(&m).fetch_one(&state.db).await?;
    if active as usize != m.len() {
        return Err(AppError::bad("members must be active employees"));
    }
    for x in &m {
        if access::level(state, owner, *x).await? < access::Level::Busy {
            let name: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(x).fetch_one(&state.db).await?;
            return Err(AppError::bad(format!("{name} doesn't share their calendar with you")));
        }
    }
    Ok(m)
}

fn check_numbers(b: &LinkInput) -> AppResult<()> {
    let within = |v: Option<i32>, lo: i32, hi: i32| v.is_none_or(|v| (lo..=hi).contains(&v));
    if !within(b.duration_minutes, 5, 480) {
        return Err(AppError::bad("a meeting lasts 5 minutes to 8 hours"));
    }
    if !within(b.slot_step_minutes, 5, 240) || !within(b.buffer_minutes, 0, 240) || !within(b.min_notice_minutes, 0, 20160) || !within(b.days_ahead, 1, 365) {
        return Err(AppError::bad("check the step, buffer, notice and days ahead"));
    }
    if !within(b.max_per_day.flatten(), 1, 50) || !within(b.remind_minutes.flatten(), 0, 40320) {
        return Err(AppError::bad("check the daily limit and the reminder"));
    }
    if let Some(tz) = &b.timezone {
        tz.parse::<Tz>().map_err(|_| AppError::bad(format!("unknown time zone: {tz}")))?;
    }
    Ok(())
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<LinkInput>) -> AppResult<Json<Value>> {
    let title = required("title", b.title.as_deref().unwrap_or(""), 200)?;
    check_numbers(&b)?;
    let hours = b.hours.clone().unwrap_or_else(|| (1..=5).map(|d| (d, 9 * 60, 17 * 60)).collect());
    check_hours(&hours)?;
    let members = check_members(&state, auth.id, b.members.as_deref().unwrap_or(&[])).await?;
    let slug = format!("{}-{}", slugify(&title, "meet"), &crate::auth::tokens::random_token(6).to_lowercase().replace(['-', '_'], "x")[..6]);
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO cal_slot_links (owner_id, slug, title, description, location, duration_minutes, members, hours, timezone, slot_step_minutes,
             buffer_minutes, min_notice_minutes, days_ahead, max_per_day, remind_minutes)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15) RETURNING id",
    )
    .bind(auth.id)
    .bind(&slug)
    .bind(&title)
    .bind(clean(b.description.flatten()))
    .bind(clean(b.location.flatten()))
    .bind(b.duration_minutes.unwrap_or(30))
    .bind(&members)
    .bind(json!(hours))
    .bind(b.timezone.clone().unwrap_or_else(|| "UTC".into()))
    .bind(b.slot_step_minutes.unwrap_or(30))
    .bind(b.buffer_minutes.unwrap_or(0))
    .bind(b.min_notice_minutes.unwrap_or(120))
    .bind(b.days_ahead.unwrap_or(30))
    .bind(b.max_per_day.flatten())
    .bind(b.remind_minutes.flatten().or(Some(15)))
    .fetch_one(&state.db)
    .await?;
    Ok(Json(link_json(&state, &load(&state, id).await?).await?))
}

async fn owned(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Link> {
    let l = load(state, id).await?;
    if l.owner_id != auth.id {
        return Err(if l.members.contains(&auth.id) { AppError::Forbidden } else { AppError::NotFound("slot link") });
    }
    Ok(l)
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<LinkInput>) -> AppResult<Json<Value>> {
    let l = owned(&state, &auth, id).await?;
    check_numbers(&b)?;
    let title = b.title.as_deref().map(|t| required("title", t, 200)).transpose()?;
    if let Some(h) = &b.hours {
        check_hours(h)?;
    }
    let members = match &b.members {
        Some(m) => Some(check_members(&state, auth.id, m).await?),
        None => None,
    };
    sqlx::query(
        "UPDATE cal_slot_links SET title = COALESCE($2, title), description = CASE WHEN $3 THEN $4 ELSE description END,
             location = CASE WHEN $5 THEN $6 ELSE location END, duration_minutes = COALESCE($7, duration_minutes), members = COALESCE($8, members),
             hours = COALESCE($9, hours), timezone = COALESCE($10, timezone), slot_step_minutes = COALESCE($11, slot_step_minutes),
             buffer_minutes = COALESCE($12, buffer_minutes), min_notice_minutes = COALESCE($13, min_notice_minutes), days_ahead = COALESCE($14, days_ahead),
             max_per_day = CASE WHEN $15 THEN $16 ELSE max_per_day END, remind_minutes = CASE WHEN $17 THEN $18 ELSE remind_minutes END,
             is_active = COALESCE($19, is_active), updated_at = now()
         WHERE id = $1",
    )
    .bind(l.id)
    .bind(title)
    .bind(b.description.is_some())
    .bind(clean(b.description.flatten()))
    .bind(b.location.is_some())
    .bind(clean(b.location.flatten()))
    .bind(b.duration_minutes)
    .bind(members)
    .bind(b.hours.map(|h| json!(h)))
    .bind(b.timezone)
    .bind(b.slot_step_minutes)
    .bind(b.buffer_minutes)
    .bind(b.min_notice_minutes)
    .bind(b.days_ahead)
    .bind(b.max_per_day.is_some())
    .bind(b.max_per_day.flatten())
    .bind(b.remind_minutes.is_some())
    .bind(b.remind_minutes.flatten())
    .bind(b.is_active)
    .execute(&state.db)
    .await?;
    Ok(Json(link_json(&state, &load(&state, id).await?).await?))
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let l = owned(&state, &auth, id).await?;
    sqlx::query("DELETE FROM cal_slot_links WHERE id = $1").bind(l.id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Free time
// ---------------------------------------------------------------------------

/// When these people are busy during [from, to): their events (unless declined) and every
/// occurrence of repeating ones, bookings of them, open CRM calls and meetings, approved
/// absences and company holidays (whole days in `tz`).
pub async fn busy_of(state: &AppState, users: &[Uuid], from: DateTime<Utc>, to: DateTime<Utc>, tz: Tz) -> AppResult<Vec<(DateTime<Utc>, DateTime<Utc>)>> {
    let mut out = Vec::new();
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT e.id FROM cal_events e LEFT JOIN cal_attendees a ON a.event_id = e.id AND a.user_id = ANY($1)
         WHERE (e.owner_id = ANY($1) OR (a.user_id IS NOT NULL AND a.status <> 'declined'))
           AND e.starts_at < $3 AND (e.recurrence IS NOT NULL OR e.ends_at > $2)",
    )
    .bind(users)
    .bind(from)
    .bind(to)
    .fetch_all(&state.db)
    .await?;
    let events = load_events(state, &ids).await?;
    let ex = exceptions(state, &ids).await?;
    for (e, _) in &events {
        let excs: Vec<DateTime<Utc>> = ex.iter().filter(|(id, _)| *id == e.id).map(|(_, t)| *t).collect();
        for s in occurrences(e, &excs, from, to) {
            out.push((s, s + (e.ends_at - e.starts_at)));
        }
    }
    let bookings: Vec<(DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT b.starts_at, b.ends_at FROM bookings b JOIN booking_resources_used u ON u.booking_id = b.id JOIN booking_resources r ON r.id = u.resource_id
         WHERE r.user_id = ANY($1) AND b.status IN ('booked', 'confirmed', 'came') AND b.starts_at < $3 AND b.ends_at > $2",
    )
    .bind(users)
    .bind(from)
    .bind(to)
    .fetch_all(&state.db)
    .await?;
    out.extend(bookings);
    let acts: Vec<(DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT COALESCE(starts_at, deadline), ends_at FROM crm_activities
         WHERE responsible_id = ANY($1) AND NOT is_completed AND kind IN ('call', 'meeting') AND COALESCE(starts_at, deadline) < $3 AND COALESCE(ends_at, COALESCE(starts_at, deadline) + interval '30 minutes') > $2",
    )
    .bind(users)
    .bind(from)
    .bind(to)
    .fetch_all(&state.db)
    .await?;
    out.extend(acts.into_iter().map(|(s, e)| (s, e.filter(|e| *e > s).unwrap_or(s + Duration::minutes(30)))));
    // Whole days off.
    let (d0, d1) = (from.with_timezone(&tz).date_naive(), to.with_timezone(&tz).date_naive());
    let mut days: Vec<NaiveDate> = sqlx::query_scalar("SELECT day FROM hr_holidays WHERE day BETWEEN $1 AND $2").bind(d0).bind(d1).fetch_all(&state.db).await?;
    let absences: Vec<(NaiveDate, NaiveDate)> = sqlx::query_as("SELECT starts_on, ends_on FROM hr_absences WHERE user_id = ANY($1) AND status = 'approved' AND starts_on <= $3 AND ends_on >= $2")
        .bind(users)
        .bind(d0)
        .bind(d1)
        .fetch_all(&state.db)
        .await?;
    for (s, e) in absences {
        let mut d = s.max(d0);
        while d <= e.min(d1) {
            days.push(d);
            d = d.succ_opt().unwrap();
        }
    }
    for d in days {
        let start = tz.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).earliest();
        let end = tz.from_local_datetime(&d.succ_opt().unwrap().and_hms_opt(0, 0, 0).unwrap()).earliest();
        if let (Some(s), Some(e)) = (start, end) {
            out.push((s.with_timezone(&Utc), e.with_timezone(&Utc)));
        }
    }
    Ok(out)
}

/// Free start times on each day of [first, last] (the link's zone), with the link's rules.
async fn free(state: &AppState, l: &Link, first: NaiveDate, last: NaiveDate) -> AppResult<Vec<(NaiveDate, Vec<DateTime<Utc>>)>> {
    let tz = l.tz();
    let now = Utc::now();
    let earliest = now + Duration::minutes(l.min_notice_minutes as i64);
    let latest_day = (now + Duration::days(l.days_ahead as i64)).with_timezone(&tz).date_naive();
    let first = first.max(now.with_timezone(&tz).date_naive());
    let last = last.min(latest_day);
    if last < first {
        return Ok(vec![]);
    }
    let at = |d: NaiveDate| tz.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).earliest().map(|t| t.with_timezone(&Utc)).unwrap_or_else(|| Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap()));
    let (from, to) = (at(first) - Duration::hours(12), at(last.succ_opt().unwrap()) + Duration::hours(12));
    let busy = busy_of(state, &l.people(), from, to, tz).await?;
    // Meetings already booked through this link, per day (for the daily limit).
    let booked: Vec<DateTime<Utc>> = sqlx::query_scalar("SELECT starts_at FROM cal_events WHERE slot_link_id = $1 AND starts_at >= $2 AND starts_at < $3").bind(l.id).bind(from).bind(to).fetch_all(&state.db).await?;
    let dur = Duration::minutes(l.duration_minutes as i64);
    let buf = Duration::minutes(l.buffer_minutes as i64);
    let hours = l.hours();
    let mut out = Vec::new();
    let mut d = first;
    while d <= last {
        let wd = d.weekday().number_from_monday();
        let full = l.max_per_day.is_some_and(|m| booked.iter().filter(|b| b.with_timezone(&tz).date_naive() == d).count() as i32 >= m);
        let mut day = Vec::new();
        if !full {
            for (w, hs, he) in hours.iter().filter(|(w, ..)| *w == wd) {
                let _ = w;
                let mut m = *hs;
                while m + l.duration_minutes <= *he {
                    if let Some(local) = tz.from_local_datetime(&d.and_hms_opt((m / 60) as u32, (m % 60) as u32, 0).unwrap()).earliest() {
                        let s = local.with_timezone(&Utc);
                        let e = s + dur;
                        if s >= earliest && !busy.iter().any(|(bs, be)| *bs < e + buf && *be + buf > s) && !day.contains(&s) {
                            day.push(s);
                        }
                    }
                    m += l.slot_step_minutes;
                }
            }
        }
        day.sort();
        out.push((d, day));
        d = d.succ_opt().unwrap();
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The public page
// ---------------------------------------------------------------------------

async fn active(state: &AppState, slug: &str) -> AppResult<Link> {
    let l: Link = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM cal_slot_links WHERE slug = $1 AND is_active"))).bind(slug).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("page"))?;
    let owner_active: bool = sqlx::query_scalar("SELECT is_active FROM users WHERE id = $1").bind(l.owner_id).fetch_one(&state.db).await?;
    if !owner_active {
        return Err(AppError::NotFound("page"));
    }
    Ok(l)
}

async fn page(State(state): State<AppState>, Path(slug): Path<String>) -> AppResult<Json<Value>> {
    let l = active(&state, &slug).await?;
    let people = names(&state, &l.people()).await?;
    let company: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'company_name'").fetch_optional(&state.db).await?;
    Ok(Json(json!({
        "title": l.title, "description": l.description, "location": l.location, "duration_minutes": l.duration_minutes,
        "timezone": l.timezone, "days_ahead": l.days_ahead, "company": company.and_then(|v| v.as_str().map(String::from)),
        "people": people.into_iter().map(|(_, name, _)| name).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct SlotsQuery {
    from: NaiveDate,
    to: NaiveDate,
}

async fn slots(State(state): State<AppState>, Path(slug): Path<String>, Query(q): Query<SlotsQuery>) -> AppResult<Json<Value>> {
    let l = active(&state, &slug).await?;
    if q.to < q.from || (q.to - q.from).num_days() > 31 {
        return Err(AppError::bad("ask for at most 31 days"));
    }
    let days = free(&state, &l, q.from, q.to).await?;
    Ok(Json(json!({ "timezone": l.timezone, "days": days.into_iter().map(|(d, s)| json!({ "date": d, "slots": s })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct BookInput {
    starts_at: DateTime<Utc>,
    name: String,
    email: String,
    #[serde(default)]
    note: Option<String>,
    /// A field people don't see: bots fill it in.
    #[serde(default)]
    website: String,
    /// The CRM record the link was sent from (a signed token), if any.
    #[serde(default)]
    r: Option<String>,
}

async fn limited(state: &AppState, key: String, max: i64, secs: i64) -> AppResult<bool> {
    let mut redis = state.redis.clone();
    let n: i64 = redis.incr(&key, 1).await?;
    if n == 1 {
        let _: () = redis.expire(&key, secs).await?;
    }
    Ok(n > max)
}

async fn book(State(state): State<AppState>, ClientIp(ip): ClientIp, Path(slug): Path<String>, Json(b): Json<BookInput>) -> AppResult<Json<Value>> {
    let l = active(&state, &slug).await?;
    if !b.website.is_empty() {
        return Err(AppError::bad("something went wrong; please try again"));
    }
    let name = required("name", &b.name, 200)?;
    let guest = super::invites::clean_guests(vec![super::invites::GuestInput { email: b.email.clone(), name: Some(name.clone()) }])?.remove(0);
    let who = ip.unwrap_or_else(|| "unknown".into());
    if limited(&state, format!("meet:ip:{who}"), 20, 600).await? || limited(&state, format!("meet:email:{}", guest.email), 5, 600).await? {
        return Err(AppError::TooManyRequests("too many bookings in a short time; please try again later".into()));
    }
    // Only a time that is still free (checked under a lock, so two people can't take it).
    let mut redis = state.redis.clone();
    let lock = format!("meet:lock:{}", l.id);
    let got: bool = redis::cmd("SET").arg(&lock).arg(1).arg("NX").arg("EX").arg(15).query_async(&mut redis).await.map(|v: Option<String>| v.is_some()).unwrap_or(true);
    if !got {
        return Err(AppError::Conflict("someone is booking right now; please try again".into()));
    }
    let result = book_locked(&state, &l, &b, &name, guest).await;
    let _: Result<(), _> = redis.del(&lock).await;
    result
}

async fn book_locked(state: &AppState, l: &Link, b: &BookInput, name: &str, guest: super::invites::GuestInput) -> AppResult<Json<Value>> {
    let tz = l.tz();
    let day = b.starts_at.with_timezone(&tz).date_naive();
    let free_now = free(state, l, day, day).await?;
    if !free_now.iter().any(|(_, s)| s.contains(&b.starts_at)) {
        return Err(AppError::Conflict("that time was just taken; please choose another".into()));
    }
    let end = b.starts_at + Duration::minutes(l.duration_minutes as i64);
    let mut description = format!("Booked by {name} <{}> through “{}”.", guest.email, l.title);
    if let Some(n) = clean(b.note.clone()) {
        description.push_str("\n\n");
        description.push_str(&n.chars().take(2000).collect::<String>());
    }
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO cal_events (owner_id, created_by, title, description, location, starts_at, ends_at, all_day, timezone, remind_minutes, slot_link_id)
         VALUES ($1, $1, $2, $3, $4, $5, $6, FALSE, $7, $8, $9) RETURNING id",
    )
    .bind(l.owner_id)
    .bind(format!("{}: {name}", l.title))
    .bind(&description)
    .bind(&l.location)
    .bind(b.starts_at)
    .bind(end)
    .bind(&l.timezone)
    .bind(l.remind_minutes)
    .bind(l.id)
    .fetch_one(&mut *tx)
    .await?;
    for m in l.members.iter().filter(|m| **m != l.owner_id) {
        sqlx::query("INSERT INTO cal_attendees (event_id, user_id, status) VALUES ($1, $2, 'accepted')").bind(id).bind(m).execute(&mut *tx).await?;
    }
    // Sent from a CRM record: the meeting is about it.
    let item = b.r.as_deref().and_then(|t| record_of(state, l.id, t));
    if let Some(i) = item {
        sqlx::query("UPDATE cal_events SET item_id = (SELECT id FROM crm_items WHERE id = $2 AND deleted_at IS NULL) WHERE id = $1").bind(id).bind(i).execute(&mut *tx).await?;
    }
    let gid: Uuid = sqlx::query_scalar("INSERT INTO cal_email_guests (event_id, email, name, status, answered_at) VALUES ($1, $2, $3, 'accepted', now()) RETURNING id")
        .bind(id)
        .bind(&guest.email)
        .bind(&guest.name)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    if let Some(i) = item {
        booked_for_record(state, l, i, id, name, b.starts_at, end).await?;
    }
    let (e, _) = load_events(state, &[id]).await?.remove(0);
    let guests = super::invites::guests_of(state, id).await?;
    super::invites::send(state, &e, &guests, super::invites::Change::Invited).await;
    notify(state, &l.people(), NewNotification {
        kind: "calendar.invite",
        title: format!("{name} booked “{}” on {}", l.title, super::when(&e, e.starts_at)),
        body: clean(b.note.clone()),
        link: Some(format!("/calendar?event={id}")),
        actor: None,
    })
    .await;
    Ok(Json(json!({ "starts_at": e.starts_at, "ends_at": e.ends_at, "timezone": e.timezone, "manage_url": super::invites::link(state, gid) })))
}

// ---------------------------------------------------------------------------
// Links sent from a CRM record
// ---------------------------------------------------------------------------

fn record_sig(state: &AppState, link: Uuid, item: Uuid) -> String {
    let s = crate::outbound::sign(&state.config.jwt_secret, format!("meet:{link}:{item}").as_bytes());
    s.trim_start_matches("sha256=")[..32].to_string()
}

/// The record a booking token names, when it was made for this link.
fn record_of(state: &AppState, link: Uuid, token: &str) -> Option<Uuid> {
    let (item, sig) = token.split_once('.')?;
    let item = Uuid::parse_str(item).ok()?;
    let want = record_sig(state, link, item);
    (sig.len() == want.len() && sig.bytes().zip(want.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0).then_some(item)
}

#[derive(Deserialize)]
struct ForRecord {
    item_id: Uuid,
}

/// The link's address for a client of a CRM record: what they book lands on the record as a
/// meeting. For the link's owner or members, on a record they may read.
async fn for_record(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ForRecord>) -> AppResult<Json<Value>> {
    let l = load(&state, id).await?;
    if !l.people().contains(&auth.id) {
        return Err(AppError::NotFound("slot link"));
    }
    if !l.is_active {
        return Err(AppError::bad("this link is switched off"));
    }
    let row = crate::crm::items::load_row(&state, b.item_id, false).await?;
    crate::crm::access::require_read(&state, &auth, row.entity_type, b.item_id, row.assigned_to).await?;
    let token = format!("{}.{}", b.item_id.simple(), record_sig(&state, l.id, b.item_id));
    Ok(Json(json!({ "url": format!("{}/meet/{}?r={token}", state.config.public_url, l.slug), "title": l.title })))
}

/// A booking made through a record's link: a meeting on the record, done or not like any other.
async fn booked_for_record(state: &AppState, l: &Link, item: Uuid, event: Uuid, name: &str, starts: DateTime<Utc>, ends: DateTime<Utc>) -> AppResult<()> {
    let Ok(row) = crate::crm::items::load_row(state, item, false).await else { return Ok(()) };
    let subject = format!("{}: {name}", l.title);
    let act: Uuid = sqlx::query_scalar(
        "INSERT INTO crm_activities (item_id, kind, subject, description, starts_at, ends_at, deadline, responsible_id, cal_event_id, remind_before_minutes)
         VALUES ($1, 'meeting', $2, $3, $4, $5, $4, $6, $7, $8) RETURNING id",
    )
    .bind(item)
    .bind(&subject)
    .bind(format!("Booked through “{}”.", l.title))
    .bind(starts)
    .bind(ends)
    .bind(l.owner_id)
    .bind(event)
    .bind(l.remind_minutes)
    .fetch_one(&state.db)
    .await?;
    sqlx::query("UPDATE crm_items SET last_activity_at = now() WHERE id = $1").bind(item).execute(&state.db).await?;
    crate::crm::timeline(&state.db, item, "booking", None, None, json!({ "activity_id": act, "event_id": event, "name": name, "link": l.title, "starts_at": starts })).await?;
    crate::crm::broadcast(state, row.entity_type, item, false).await;
    Ok(())
}

/// The client cancelled a meeting booked from a record: its activity is closed as cancelled.
pub async fn booking_cancelled(state: &AppState, event: Uuid, who: &str) {
    let done: Result<Vec<(Uuid, Uuid)>, _> = sqlx::query_as(
        "UPDATE crm_activities SET is_completed = TRUE, completed_at = now(), result = $2, cal_event_id = NULL, updated_at = now()
         WHERE cal_event_id = $1 AND NOT is_completed RETURNING id, item_id",
    )
    .bind(event)
    .bind(format!("Cancelled by {who}"))
    .fetch_all(&state.db)
    .await;
    for (act, item) in done.unwrap_or_default() {
        let _ = crate::crm::timeline(&state.db, item, "booking", None, None, json!({ "activity_id": act, "cancelled": true, "name": who })).await;
        if let Ok(row) = crate::crm::items::load_row(state, item, false).await {
            crate::crm::broadcast(state, row.entity_type, item, false).await;
        }
    }
}
