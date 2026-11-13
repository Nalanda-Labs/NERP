//! A person's calendar as an iCalendar feed, for Google Calendar, Outlook, phones…
//! Their events (not declined) and CRM calls and meetings, from 60 days ago to a year ahead,
//! each occurrence of a repeating event as its own entry.

use axum::{
    extract::{Path, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use super::{exceptions, hash, ical::{esc, fold, stamp}, load_events, occurrences};
use crate::{
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/public/calendar/{file}", get(feed))
}

struct Entry {
    uid: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    all_day: bool,
    summary: String,
    location: Option<String>,
    description: Option<String>,
    url: String,
}

fn render(name: &str, entries: &[Entry]) -> String {
    let now = stamp(Utc::now());
    let mut s = String::new();
    for l in ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//NERP//Calendar//EN", "CALSCALE:GREGORIAN", "METHOD:PUBLISH"] {
        s.push_str(&fold(l));
    }
    s.push_str(&fold(&format!("X-WR-CALNAME:{}", esc(name))));
    s.push_str(&fold("REFRESH-INTERVAL;VALUE=DURATION:PT30M"));
    for e in entries {
        s.push_str(&fold("BEGIN:VEVENT"));
        s.push_str(&fold(&format!("UID:{}", e.uid)));
        s.push_str(&fold(&format!("DTSTAMP:{now}")));
        if e.all_day {
            s.push_str(&fold(&format!("DTSTART;VALUE=DATE:{}", e.start.format("%Y%m%d"))));
            s.push_str(&fold(&format!("DTEND;VALUE=DATE:{}", e.end.format("%Y%m%d"))));
        } else {
            s.push_str(&fold(&format!("DTSTART:{}", stamp(e.start))));
            s.push_str(&fold(&format!("DTEND:{}", stamp(e.end))));
        }
        s.push_str(&fold(&format!("SUMMARY:{}", esc(&e.summary))));
        if let Some(l) = &e.location {
            s.push_str(&fold(&format!("LOCATION:{}", esc(l))));
        }
        if let Some(d) = &e.description {
            s.push_str(&fold(&format!("DESCRIPTION:{}", esc(d))));
        }
        s.push_str(&fold(&format!("URL:{}", e.url)));
        s.push_str(&fold("END:VEVENT"));
    }
    s.push_str(&fold("END:VCALENDAR"));
    s
}

async fn feed(State(state): State<AppState>, Path(file): Path<String>) -> AppResult<Response> {
    let token = file.strip_suffix(".ics").unwrap_or(&file);
    let user: Uuid = sqlx::query_scalar!("SELECT f.user_id FROM cal_feeds f JOIN users u ON u.id = f.user_id WHERE f.token_hash = $1 AND u.is_active", hash(token))
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("calendar"))?;
    let name: String = sqlx::query_scalar!(r#"SELECT trim(first_name || ' ' || last_name) AS "v!" FROM users WHERE id = $1"#, user).fetch_one(&state.db).await?;
    let (from, to) = (Utc::now() - Duration::days(60), Utc::now() + Duration::days(365));
    let base = &state.config.public_url;
    let ids: Vec<Uuid> = sqlx::query_scalar!("SELECT DISTINCT e.id FROM cal_events e LEFT JOIN cal_attendees a ON a.event_id = e.id AND a.user_id = $1
         WHERE (e.owner_id = $1 OR (a.user_id = $1 AND a.status <> 'declined')) AND e.starts_at < $3 AND (e.recurrence IS NOT NULL OR e.ends_at >= $2)", user, from, to)
    .fetch_all(&state.db)
    .await?;
    let events = load_events(&state, &ids).await?;
    let ex = exceptions(&state, &ids).await?;
    let mut entries = Vec::new();
    for (e, _) in &events {
        let excs: Vec<DateTime<Utc>> = ex.iter().filter(|(id, _)| *id == e.id).map(|(_, t)| *t).collect();
        for start in occurrences(e, &excs, from, to) {
            entries.push(Entry {
                uid: if e.recurrence.is_some() { format!("{}-{}@nerp", e.id, start.timestamp()) } else { format!("{}@nerp", e.id) },
                start,
                end: start + (e.ends_at - e.starts_at),
                all_day: e.all_day,
                summary: e.title.clone(),
                location: e.location.clone(),
                description: e.description.clone(),
                url: format!("{base}/calendar?event={}", e.id),
            });
        }
    }
    let acts: Vec<(Uuid, String, String, DateTime<Utc>, Option<DateTime<Utc>>, Uuid, String, String)> = sqlx::query!(r#"SELECT a.id AS "id!", a.kind AS "kind!", a.subject AS "subject!", COALESCE(a.starts_at, a.deadline) AS "c3!", a.ends_at AS "ends_at?", a.item_id AS "item_id!", t.code AS "code!", i.title AS "title!"
         FROM crm_activities a JOIN crm_items i ON i.id = a.item_id JOIN crm_entity_types t ON t.id = i.entity_type
         WHERE a.responsible_id = $1 AND NOT a.is_completed AND a.kind IN ('call', 'meeting')
           AND COALESCE(a.starts_at, a.deadline) >= $2 AND COALESCE(a.starts_at, a.deadline) < $3 AND i.deleted_at IS NULL"#, user, from, to).map(|r| (r.id, r.kind, r.subject, r.c3, r.ends_at, r.item_id, r.code, r.title))
    .fetch_all(&state.db)
    .await?;
    for (id, kind, subject, start, end, item, code, title) in acts {
        entries.push(Entry {
            uid: format!("act-{id}@nerp"),
            start,
            end: end.filter(|e| *e >= start).unwrap_or(start + Duration::minutes(30)),
            all_day: false,
            summary: format!("{}: {subject}", if kind == "call" { "Call" } else { "Meeting" }),
            location: None,
            description: Some(title),
            url: format!("{base}/crm/{code}/{item}"),
        });
    }
    let mut r = render(&format!("{name} (NERP)"), &entries).into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/calendar; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=300"));
    Ok(r)
}
