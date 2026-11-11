//! Guests from outside the company, invited by email.
//!
//! Each gets an iCalendar invitation (mail apps show it with Accept / Decline and add it to
//! their calendar) and a link to answer in the browser. Changes to the time, place or title
//! send an update with a higher sequence number, so their calendar replaces the old copy;
//! removing the event (or one occurrence of it, or the guest) sends a cancellation.
//! The answer link is signed with the server's secret, so no token is stored.

use axum::{
    extract::{Path, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use hmac::{KeyInit, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{ical, Event};
use crate::{
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/public/calendar/rsvp/{token}", get(rsvp_view).post(rsvp_answer))
        .route("/public/calendar/rsvp/{token}/invite.ics", get(rsvp_ics))
}

#[derive(sqlx::FromRow, Clone)]
pub struct EmailGuest {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub status: String,
}

pub async fn guests_of(state: &AppState, event: Uuid) -> AppResult<Vec<EmailGuest>> {
    Ok(sqlx::query_as("SELECT id, email, name, status FROM cal_email_guests WHERE event_id = $1 ORDER BY email").bind(event).fetch_all(&state.db).await?)
}

/// A guest given in the editor: an email address and, optionally, a name.
#[derive(Deserialize, Clone)]
pub struct GuestInput {
    pub email: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// Checks and normalises the list (lower-case, unique).
pub fn clean_guests(list: Vec<GuestInput>) -> AppResult<Vec<GuestInput>> {
    if list.len() > 100 {
        return Err(AppError::bad("at most 100 guests by email"));
    }
    let mut out: Vec<GuestInput> = Vec::new();
    for g in list {
        let email = g.email.trim().to_lowercase();
        let valid = email.len() <= 254 && email.split_once('@').is_some_and(|(a, d)| !a.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.')) && !email.contains(char::is_whitespace);
        if !valid {
            return Err(AppError::bad(format!("“{}” is not an email address", g.email.trim())));
        }
        if !out.iter().any(|x| x.email == email) {
            let name = g.name.map(|n| n.trim().chars().take(200).collect::<String>()).filter(|n| !n.is_empty());
            out.push(GuestInput { email, name });
        }
    }
    Ok(out)
}

fn sign(state: &AppState, id: Uuid) -> String {
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(state.config.jwt_secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(format!("cal-rsvp:{id}").as_bytes());
    hex::encode(&mac.finalize().into_bytes()[..16])
}

pub fn token(state: &AppState, id: Uuid) -> String {
    format!("{id}.{}", sign(state, id))
}

fn verify(state: &AppState, token: &str) -> AppResult<Uuid> {
    let (id, sig) = token.split_once('.').ok_or(AppError::NotFound("invitation"))?;
    let id = Uuid::parse_str(id).map_err(|_| AppError::NotFound("invitation"))?;
    // Constant-time comparison of the signature.
    let want = sign(state, id);
    if want.len() != sig.len() || want.bytes().zip(sig.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) != 0 {
        return Err(AppError::NotFound("invitation"));
    }
    Ok(id)
}

pub fn link(state: &AppState, id: Uuid) -> String {
    format!("{}/rsvp/{}", state.config.public_url, token(state, id))
}

async fn company(state: &AppState) -> String {
    let v: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'company_name'").fetch_optional(&state.db).await.ok().flatten();
    v.and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "NERP".into())
}

/// The event as an iCalendar VEVENT: the owner as organiser, everyone invited as attendees.
pub async fn vevent(state: &AppState, e: &Event) -> AppResult<ical::VEvent> {
    let (oname, oemail): (String, String) = sqlx::query_as("SELECT trim(first_name || ' ' || last_name), email FROM users WHERE id = $1").bind(e.owner_id).fetch_one(&state.db).await?;
    let inside: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT trim(u.first_name || ' ' || u.last_name), u.email, a.status FROM cal_attendees a JOIN users u ON u.id = a.user_id WHERE a.event_id = $1 ORDER BY u.first_name",
    )
    .bind(e.id)
    .fetch_all(&state.db)
    .await?;
    let mut attendees: Vec<ical::Person> = inside.into_iter().map(|(name, email, s)| ical::Person { name, email, partstat: ical::partstat(&s) }).collect();
    for g in guests_of(state, e.id).await? {
        attendees.push(ical::Person { name: g.name.clone().unwrap_or_else(|| g.email.clone()), email: g.email, partstat: ical::partstat(&g.status) });
    }
    let exdates: Vec<DateTime<Utc>> = sqlx::query_scalar("SELECT occurrence_start FROM cal_exceptions WHERE event_id = $1 ORDER BY 1").bind(e.id).fetch_all(&state.db).await?;
    Ok(ical::VEvent {
        uid: format!("{}@nerp", e.id),
        sequence: e.sequence,
        start: e.starts_at,
        end: e.ends_at,
        all_day: e.all_day,
        timezone: e.timezone.clone(),
        rule: e.rule(),
        exdates,
        recurrence_id: None,
        summary: e.title.clone(),
        location: super::place(state, e).await?,
        description: e.description.clone(),
        url: None,
        organizer: Some(ical::Person { name: oname, email: oemail, partstat: "ACCEPTED" }),
        attendees,
        cancelled: false,
        private: false,
        last_modified: None,
        alarm_minutes: None,
    })
}

/// What happened, for the email.
pub enum Change {
    Invited,
    Updated,
    Cancelled,
    /// One occurrence of a repeating event.
    OccurrenceCancelled(DateTime<Utc>),
}

/// Emails the invitation (or update, or cancellation) to these guests.
pub async fn send(state: &AppState, e: &Event, to: &[EmailGuest], change: Change) {
    if to.is_empty() {
        return;
    }
    if let Err(err) = send_inner(state, e, to, change).await {
        tracing::warn!("calendar invitations for {}: {err}", e.id);
    }
}

async fn send_inner(state: &AppState, e: &Event, to: &[EmailGuest], change: Change) -> AppResult<()> {
    let company = company(state).await;
    let mut ev = vevent(state, e).await?;
    let organizer = ev.organizer.as_ref().map(|o| o.name.clone()).unwrap_or_default();
    let place = ev.location.clone();
    let (method, subject, heading, intro) = match &change {
        Change::Invited => ("REQUEST", format!("Invitation: {}", e.title), "You're invited".to_string(), format!("{organizer} invited you to “{}”.", e.title)),
        Change::Updated => ("REQUEST", format!("Updated: {}", e.title), "The meeting changed".to_string(), format!("{organizer} changed “{}”.", e.title)),
        Change::Cancelled => ("CANCEL", format!("Cancelled: {}", e.title), "The meeting is cancelled".to_string(), format!("{organizer} cancelled “{}”.", e.title)),
        Change::OccurrenceCancelled(occ) => ("CANCEL", format!("Cancelled: {} on {}", e.title, super::when(e, *occ)), "One meeting is cancelled".to_string(), format!("{organizer} cancelled “{}” on {}.", e.title, super::when(e, *occ))),
    };
    let mut when = super::when(e, e.starts_at);
    if let Some(r) = e.rule() {
        when = format!("{when}; repeats: {}", r.describe());
    }
    match change {
        Change::Cancelled => {
            ev.cancelled = true;
        }
        Change::OccurrenceCancelled(occ) => {
            ev.cancelled = true;
            ev.rule = None;
            ev.recurrence_id = Some(occ);
            ev.start = occ;
            ev.end = occ + (e.ends_at - e.starts_at);
        }
        _ => {}
    }
    for g in to {
        // Each guest's copy asks only them to answer.
        let mut mine = ev.clone();
        for a in &mut mine.attendees {
            if a.email == g.email {
                a.partstat = ical::partstat(&g.status);
            }
        }
        let ics = ical::calendar(Some(method), None, &[mine]);
        let mut paragraphs = vec![intro.clone()];
        if !matches!(method, "CANCEL") {
            paragraphs.push(format!("When: {when}"));
            if let Some(p) = &place {
                paragraphs.push(format!("Where: {p}"));
            }
            if let Some(d) = &e.description {
                paragraphs.push(d.clone());
            }
        }
        let url = link(state, g.id);
        let button = if method == "CANCEL" { None } else { Some(("Accept or decline", url.as_str())) };
        let mut mail = crate::mail::notice(&g.email, &company, &subject, &heading, &paragraphs, button, "You get this email because you were invited to a meeting. The attached invitation adds it to your calendar.");
        mail.calendar = Some(ics);
        crate::jobs::send_email(state, mail).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The guest's answer page
// ---------------------------------------------------------------------------

async fn guest_and_event(state: &AppState, token: &str) -> AppResult<(EmailGuest, Event)> {
    let id = verify(state, token)?;
    let (event_id,): (Uuid,) = sqlx::query_as("SELECT event_id FROM cal_email_guests WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("invitation"))?;
    let g: EmailGuest = sqlx::query_as("SELECT id, email, name, status FROM cal_email_guests WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let (e, _) = super::load_events(state, &[event_id]).await?.into_iter().next().ok_or(AppError::NotFound("invitation"))?;
    Ok((g, e))
}

async fn view_json(state: &AppState, g: &EmailGuest, e: &Event) -> AppResult<Value> {
    let organizer: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(e.owner_id).fetch_one(&state.db).await?;
    let next = if e.recurrence.is_some() {
        let ex: Vec<DateTime<Utc>> = sqlx::query_scalar("SELECT occurrence_start FROM cal_exceptions WHERE event_id = $1").bind(e.id).fetch_all(&state.db).await?;
        super::occurrences(e, &ex, Utc::now(), Utc::now() + chrono::Duration::days(400)).into_iter().next()
    } else {
        None
    };
    Ok(json!({
        "title": e.title, "description": e.description, "location": super::place(state, e).await?,
        "starts_at": e.starts_at, "ends_at": e.ends_at, "all_day": e.all_day, "timezone": e.timezone,
        "repeat": e.rule().map(|r| r.describe()), "next": next,
        "organizer": organizer, "company": company(state).await,
        "guest": { "email": g.email, "name": g.name, "status": g.status },
    }))
}

async fn rsvp_view(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let (g, e) = guest_and_event(&state, &token).await?;
    Ok(Json(view_json(&state, &g, &e).await?))
}

#[derive(Deserialize)]
struct Answer {
    status: String,
}

async fn rsvp_answer(State(state): State<AppState>, Path(token): Path<String>, Json(a): Json<Answer>) -> AppResult<Json<Value>> {
    if !["accepted", "declined", "tentative"].contains(&a.status.as_str()) {
        return Err(AppError::bad("answer accepted, declined or tentative"));
    }
    let (g, e) = guest_and_event(&state, &token).await?;
    // Someone who booked a time through a slot link and now declines cancels the meeting.
    if a.status == "declined" && e.slot_link_id.is_some() {
        let mut view = view_json(&state, &g, &e).await?;
        let who = g.name.clone().unwrap_or_else(|| g.email.clone());
        let mut people: Vec<Uuid> = sqlx::query_scalar("SELECT user_id FROM cal_attendees WHERE event_id = $1").bind(e.id).fetch_all(&state.db).await?;
        people.push(e.owner_id);
        let chat = crate::im::linked_chat_people(&state, crate::im::Link::Event(e.id)).await?;
        super::slots::booking_cancelled(&state, e.id, &who).await;
        sqlx::query("DELETE FROM cal_events WHERE id = $1").bind(e.id).execute(&state.db).await?;
        crate::im::linked_chat_gone(&state, chat).await;
        notify(&state, &people, NewNotification { kind: "calendar.cancelled", title: format!("{who} cancelled “{}” on {}", e.title, super::when(&e, e.starts_at)), body: None, link: Some("/calendar".into()), actor: None }).await;
        view["cancelled"] = json!(true);
        view["guest"]["status"] = json!("declined");
        return Ok(Json(view));
    }
    if g.status != a.status {
        sqlx::query("UPDATE cal_email_guests SET status = $2, answered_at = now() WHERE id = $1").bind(g.id).bind(&a.status).execute(&state.db).await?;
        let who = g.name.clone().unwrap_or_else(|| g.email.clone());
        let verb = match a.status.as_str() { "accepted" => "accepted", "declined" => "declined", _ => "might come to" };
        notify(&state, &[e.owner_id], NewNotification { kind: "calendar.answer", title: format!("{who} {verb} “{}”", e.title), body: None, link: Some(format!("/calendar?event={}", e.id)), actor: None }).await;
    }
    let g: EmailGuest = sqlx::query_as("SELECT id, email, name, status FROM cal_email_guests WHERE id = $1").bind(g.id).fetch_one(&state.db).await?;
    Ok(Json(view_json(&state, &g, &e).await?))
}

/// The event to add to a calendar by hand.
async fn rsvp_ics(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Response> {
    let (_, e) = guest_and_event(&state, &token).await?;
    let ics = ical::calendar(Some("PUBLISH"), None, &[vevent(&state, &e).await?]);
    let mut r = ics.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/calendar; charset=utf-8"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"invite.ics\""));
    Ok(r)
}
