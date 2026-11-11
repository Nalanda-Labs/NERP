//! Running live chat: handing chats out, working hours, automatic replies and closing idle
//! chats, saved replies, passing a chat on, reviews, figures and export.

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use chrono_tz::Tz;
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{announce, end, manager, session};
use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::required,
    state::AppState,
};

pub const MAINTENANCE_JOB: &str = "chat.maintenance";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/chat/canned", get(list_canned).post(create_canned))
        .route("/chat/canned/{id}", patch(update_canned).delete(delete_canned))
        .route("/chat/sessions/{id}/transfer", post(transfer))
        .route("/chat/sessions/{id}/review", post(review))
        .route("/chat/stats", get(stats))
        .route("/chat/export.csv", get(export))
}

// ---------------------------------------------------------------------------
// Handing out
// ---------------------------------------------------------------------------

/// Whether the channel is within its working hours now.
pub(crate) fn online(hours: &Option<Value>, timezone: &str, at: DateTime<Utc>) -> bool {
    let Some(h) = hours.as_ref().and_then(|h| serde_json::from_value::<Vec<(u32, i32, i32)>>(h.clone()).ok()) else {
        return true;
    };
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let local = at.with_timezone(&tz);
    let wd = local.weekday().number_from_monday();
    let m = (local.hour() * 60 + local.minute()) as i32;
    h.iter().any(|(d, from, to)| *d == wd && *from <= m && m < *to)
}

/// The next operator for a channel's chat: active ones in turn, skipping those with their
/// fill of open chats (and `skip`). None when the channel offers chats to everyone, or nobody
/// is free.
pub(crate) async fn pick(state: &AppState, channel: Uuid, skip: &[Uuid]) -> AppResult<Option<Uuid>> {
    let (ops, distribution, max): (Vec<Uuid>, String, Option<i32>) = sqlx::query_as("SELECT operators, distribution, max_chats FROM chat_channels WHERE id = $1").bind(channel).fetch_one(&state.db).await?;
    if distribution == "all" {
        return Ok(None);
    }
    let active: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT u.h, (SELECT COUNT(*) FROM chat_sessions s WHERE s.assigned_to = u.h AND s.status = 'open')
         FROM unnest($1::uuid[]) WITH ORDINALITY u(h, n) JOIN users ON users.id = u.h AND users.is_active ORDER BY u.n",
    )
    .bind(&ops)
    .fetch_all(&state.db)
    .await?;
    let free: Vec<Uuid> = active.into_iter().filter(|(u, n)| !skip.contains(u) && max.is_none_or(|m| *n < m as i64)).map(|(u, _)| u).collect();
    if free.is_empty() {
        return Ok(None);
    }
    let mut redis = state.redis.clone();
    let i: i64 = redis.incr(format!("chat:rr:{channel}"), 1).await?;
    Ok(Some(free[((i - 1) as usize) % free.len()]))
}

async fn system(state: &AppState, session: Uuid, body: &str) -> AppResult<()> {
    sqlx::query("INSERT INTO chat_messages (session_id, author, body) VALUES ($1, 'system', $2)").bind(session).bind(body).execute(&state.db).await?;
    Ok(())
}

/// A message to the visitor from the channel (no operator's name on it).
pub(crate) async fn auto(state: &AppState, session: Uuid, body: &str) -> AppResult<()> {
    sqlx::query("INSERT INTO chat_messages (session_id, author, body) VALUES ($1, 'operator', $2)").bind(session).bind(body).execute(&state.db).await?;
    sqlx::query("UPDATE chat_sessions SET last_message_at = now() WHERE id = $1").bind(session).execute(&state.db).await?;
    Ok(())
}

async fn first_name(state: &AppState, u: Uuid) -> AppResult<String> {
    Ok(sqlx::query_scalar("SELECT first_name FROM users WHERE id = $1").bind(u).fetch_one(&state.db).await?)
}

/// Every minute: passes on chats nobody answered, tells waiting visitors, closes idle chats.
pub async fn maintenance(state: &AppState) -> Result<()> {
    // Unanswered since given to someone: to the next operator.
    let stale: Vec<(Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT s.id, s.channel_id, s.assigned_to FROM chat_sessions s JOIN chat_channels c ON c.id = s.channel_id
         WHERE s.status = 'open' AND s.assigned_to IS NOT NULL AND c.requeue_minutes IS NOT NULL AND c.distribution = 'queue'
           AND s.offered_at < now() - make_interval(mins => c.requeue_minutes)
           AND NOT EXISTS (SELECT 1 FROM chat_messages m WHERE m.session_id = s.id AND m.author = 'operator' AND m.user_id = s.assigned_to AND m.created_at >= s.offered_at)",
    )
    .fetch_all(&state.db)
    .await?;
    for (id, channel, current) in stale {
        let Some(next) = pick(state, channel, &[current]).await? else {
            // Nobody else: keep it, and don't look again until the next round of minutes.
            sqlx::query("UPDATE chat_sessions SET offered_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
            continue;
        };
        sqlx::query("UPDATE chat_sessions SET assigned_to = $2, offered_at = now() WHERE id = $1").bind(id).bind(next).execute(&state.db).await?;
        let (from, to) = (first_name(state, current).await?, first_name(state, next).await?);
        system(state, id, &format!("Nobody answered: passed from {from} to {to}")).await?;
        notify(state, &[next], NewNotification { kind: "chat.new", title: format!("A chat waiting for an answer was passed to you"), body: None, link: Some(format!("/chats?session={id}")), actor: None }).await;
        announce(state, id, "chat.changed").await;
    }
    // Waiting visitors hear something.
    let waiting: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT s.id, c.no_answer_message FROM chat_sessions s JOIN chat_channels c ON c.id = s.channel_id
         WHERE s.status = 'open' AND NOT s.no_answer_sent AND s.first_reply_at IS NULL AND c.no_answer_minutes IS NOT NULL AND c.no_answer_message IS NOT NULL
           AND EXISTS (SELECT 1 FROM chat_messages m WHERE m.session_id = s.id AND m.author = 'visitor' AND m.created_at < now() - make_interval(mins => c.no_answer_minutes))",
    )
    .fetch_all(&state.db)
    .await?;
    for (id, msg) in waiting {
        auto(state, id, &msg).await?;
        sqlx::query("UPDATE chat_sessions SET no_answer_sent = TRUE WHERE id = $1").bind(id).execute(&state.db).await?;
        announce(state, id, "chat.message").await;
    }
    // Idle chats close (with the closing message).
    let idle: Vec<Uuid> = sqlx::query_scalar(
        "SELECT s.id FROM chat_sessions s JOIN chat_channels c ON c.id = s.channel_id
         WHERE s.status = 'open' AND c.idle_close_minutes IS NOT NULL AND s.last_message_at < now() - make_interval(mins => c.idle_close_minutes)
           AND EXISTS (SELECT 1 FROM chat_messages m WHERE m.session_id = s.id AND m.author = 'visitor')",
    )
    .fetch_all(&state.db)
    .await?;
    for id in idle {
        end(state, id, None).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Saved replies
// ---------------------------------------------------------------------------

async fn list_canned(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    let rows: Vec<(Uuid, Option<Uuid>, String, String, String)> = sqlx::query_as("SELECT id, channel_id, shortcut, title, body FROM chat_canned ORDER BY lower(shortcut)").fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, c, s, t, b)| json!({ "id": id, "channel_id": c, "shortcut": s, "title": t, "body": b })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct CannedInput {
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    channel_id: Option<Option<Uuid>>,
    shortcut: Option<String>,
    title: Option<String>,
    body: Option<String>,
}

fn shortcut(s: &str) -> AppResult<String> {
    let s = s.trim().trim_start_matches('/').to_lowercase();
    if s.is_empty() || s.len() > 30 || !s.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err(AppError::bad("a short code is letters, digits, - and _ (up to 30)"));
    }
    Ok(s)
}

async fn create_canned(State(state): State<AppState>, auth: AuthUser, Json(b): Json<CannedInput>) -> AppResult<Json<Value>> {
    auth.require(perm::CHAT)?;
    let sc = shortcut(b.shortcut.as_deref().unwrap_or(""))?;
    let title = required("title", b.title.as_deref().unwrap_or(""), 120)?;
    let body = required("text", b.body.as_deref().unwrap_or(""), 4000)?;
    sqlx::query("INSERT INTO chat_canned (channel_id, shortcut, title, body, created_by) VALUES ($1, $2, $3, $4, $5)")
        .bind(b.channel_id.flatten())
        .bind(sc)
        .bind(title)
        .bind(body)
        .bind(auth.id)
        .execute(&state.db)
        .await
        .map_err(|_| AppError::Conflict("that short code is taken".into()))?;
    list_canned(State(state), auth).await
}

async fn update_canned(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<CannedInput>) -> AppResult<Json<Value>> {
    auth.require(perm::CHAT)?;
    let sc = b.shortcut.as_deref().map(shortcut).transpose()?;
    let title = b.title.as_deref().map(|t| required("title", t, 120)).transpose()?;
    let body = b.body.as_deref().map(|t| required("text", t, 4000)).transpose()?;
    sqlx::query("UPDATE chat_canned SET shortcut = COALESCE($2, shortcut), title = COALESCE($3, title), body = COALESCE($4, body), channel_id = CASE WHEN $5 THEN $6 ELSE channel_id END WHERE id = $1")
        .bind(id)
        .bind(sc)
        .bind(title)
        .bind(body)
        .bind(b.channel_id.is_some())
        .bind(b.channel_id.flatten())
        .execute(&state.db)
        .await
        .map_err(|_| AppError::Conflict("that short code is taken".into()))?;
    list_canned(State(state), auth).await
}

async fn delete_canned(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    auth.require(perm::CHAT)?;
    sqlx::query("DELETE FROM chat_canned WHERE id = $1").bind(id).execute(&state.db).await?;
    list_canned(State(state), auth).await
}

// ---------------------------------------------------------------------------
// Passing a chat on, reviewing it
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TransferInput {
    /// To a colleague (an operator of the chat's channel, or of the new one).
    user_id: Option<Uuid>,
    /// To another channel (its next operator, unless a colleague is named).
    channel_id: Option<Uuid>,
}

async fn transfer(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<TransferInput>) -> AppResult<Json<Value>> {
    let s = session(&state, &auth, id).await?;
    if s.status != "open" {
        return Err(AppError::bad("this chat is closed"));
    }
    if b.user_id.is_none() && b.channel_id.is_none() {
        return Err(AppError::bad("pass it to a colleague or a channel"));
    }
    let channel = b.channel_id.unwrap_or(s.channel_id);
    let (ops, cname): (Vec<Uuid>, String) = sqlx::query_as("SELECT operators, name FROM chat_channels WHERE id = $1 AND is_active").bind(channel).fetch_optional(&state.db).await?.ok_or_else(|| AppError::bad("no such channel"))?;
    let to = match b.user_id {
        Some(u) => {
            let active: bool = sqlx::query_scalar("SELECT is_active FROM users WHERE id = $1").bind(u).fetch_optional(&state.db).await?.unwrap_or(false);
            if !active || !ops.contains(&u) {
                return Err(AppError::bad("pass it to an operator of the channel"));
            }
            Some(u)
        }
        None => pick(&state, channel, s.assigned_to.as_slice()).await?,
    };
    sqlx::query("UPDATE chat_sessions SET channel_id = $2, assigned_to = $3, offered_at = now() WHERE id = $1").bind(id).bind(channel).bind(to).execute(&state.db).await?;
    let me = first_name(&state, auth.id).await?;
    let msg = match (to, b.channel_id) {
        (Some(u), Some(_)) => format!("{me} passed the chat to {} in “{cname}”", first_name(&state, u).await?),
        (Some(u), None) => format!("{me} passed the chat to {}", first_name(&state, u).await?),
        (None, _) => format!("{me} passed the chat to “{cname}”"),
    };
    system(&state, id, &msg).await?;
    let tell: Vec<Uuid> = match to {
        Some(u) => vec![u],
        None => ops,
    };
    notify(&state, &tell, NewNotification { kind: "chat.new", title: format!("{me} passed you a chat"), body: s.visitor_name.clone(), link: Some(format!("/chats?session={id}")), actor: Some(auth.id) }).await;
    announce(&state, id, "chat.changed").await;
    Ok(Json(json!({ "ok": true, "assigned_to": to, "channel_id": channel })))
}

#[derive(Deserialize)]
struct ReviewInput {
    score: i16,
    comment: Option<String>,
}

/// A supervisor's mark for how the chat was handled.
async fn review(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ReviewInput>) -> AppResult<Json<Value>> {
    auth.require(perm::CHAT)?;
    session(&state, &auth, id).await?;
    if !(1..=5).contains(&b.score) {
        return Err(AppError::bad("score 1 to 5"));
    }
    sqlx::query("UPDATE chat_sessions SET review_score = $2, review_comment = $3, reviewed_by = $4, reviewed_at = now() WHERE id = $1")
        .bind(id)
        .bind(b.score)
        .bind(crate::routes::clean(b.comment).map(|c| c.chars().take(2000).collect::<String>()))
        .bind(auth.id)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Figures and export
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Range {
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    channel_id: Option<Uuid>,
}

fn range(r: &Range) -> (DateTime<Utc>, DateTime<Utc>) {
    let to = r.to.unwrap_or_else(Utc::now);
    (r.from.unwrap_or(to - Duration::days(30)), to)
}

/// Chats, answers, waiting times, length and marks: overall, per channel, per operator, per day.
async fn stats(State(state): State<AppState>, auth: AuthUser, Query(q): Query<Range>) -> AppResult<Json<Value>> {
    if !manager(&auth) {
        return Err(AppError::Forbidden);
    }
    let (from, to) = range(&q);
    let base = "FROM chat_sessions s JOIN chat_channels c ON c.id = s.channel_id
        WHERE s.started_at >= $1 AND s.started_at < $2 AND ($3::uuid IS NULL OR s.channel_id = $3)
          AND EXISTS (SELECT 1 FROM chat_messages m WHERE m.session_id = s.id AND m.author = 'visitor')";
    let cols = "COUNT(*) AS chats, COUNT(s.first_reply_at) AS answered, COUNT(*) FILTER (WHERE s.offline) AS offline,
        AVG(EXTRACT(EPOCH FROM (s.first_reply_at - s.started_at)))::float8 AS first_reply_secs,
        AVG(EXTRACT(EPOCH FROM (s.closed_at - s.started_at)))::float8 AS duration_secs,
        AVG(s.rating)::float8 AS rating, COUNT(s.rating) AS ratings, AVG(s.review_score)::float8 AS review";
    type Row = (i64, i64, i64, Option<f64>, Option<f64>, Option<f64>, i64, Option<f64>);
    let fig = |r: &Row| json!({ "chats": r.0, "answered": r.1, "missed": r.0 - r.1, "offline": r.2, "first_reply_secs": r.3, "duration_secs": r.4, "rating": r.5, "ratings": r.6, "review": r.7 });
    let total: Row = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {cols} {base}"))).bind(from).bind(to).bind(q.channel_id).fetch_one(&state.db).await?;
    let by_channel: Vec<(String, i64, i64, i64, Option<f64>, Option<f64>, Option<f64>, i64, Option<f64>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT c.name, {cols} {base} GROUP BY c.name ORDER BY c.name"))).bind(from).bind(to).bind(q.channel_id).fetch_all(&state.db).await?;
    let by_operator: Vec<(Option<String>, i64, i64, i64, Option<f64>, Option<f64>, Option<f64>, i64, Option<f64>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT trim(u.first_name || ' ' || u.last_name), {cols} {base} AND s.assigned_to IS NOT NULL GROUP BY 1 ORDER BY 1"
    ).replace("JOIN chat_channels c ON c.id = s.channel_id", "JOIN chat_channels c ON c.id = s.channel_id LEFT JOIN users u ON u.id = s.assigned_to")))
    .bind(from)
    .bind(to)
    .bind(q.channel_id)
    .fetch_all(&state.db)
    .await?;
    let by_day: Vec<(chrono::NaiveDate, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT (s.started_at AT TIME ZONE 'UTC')::date, COUNT(*), COUNT(s.first_reply_at) {base} GROUP BY 1 ORDER BY 1")))
        .bind(from)
        .bind(to)
        .bind(q.channel_id)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!({
        "from": from, "to": to, "total": fig(&total),
        "channels": by_channel.iter().map(|r| { let mut v = fig(&(r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8)); v["name"] = json!(r.0); v }).collect::<Vec<_>>(),
        "operators": by_operator.iter().map(|r| { let mut v = fig(&(r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8)); v["name"] = json!(r.0); v }).collect::<Vec<_>>(),
        "days": by_day.iter().map(|(d, n, a)| json!({ "day": d, "chats": n, "answered": a })).collect::<Vec<_>>(),
    })))
}

fn csv_cell(s: &str) -> String {
    // Cells a spreadsheet would run as a formula are made plain text.
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
}

/// The chats (and what was said) as CSV.
async fn export(State(state): State<AppState>, auth: AuthUser, Query(q): Query<Range>) -> AppResult<Response> {
    if !manager(&auth) {
        return Err(AppError::Forbidden);
    }
    let (from, to) = range(&q);
    type Row = (Uuid, String, Option<String>, Option<String>, Option<String>, String, DateTime<Utc>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<i16>, Option<String>, Option<i16>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT s.id, c.name, s.visitor_name, s.visitor_email, trim(u.first_name || ' ' || u.last_name), s.status, s.started_at, s.first_reply_at, s.closed_at,
                s.rating, s.rating_comment, s.review_score
         FROM chat_sessions s JOIN chat_channels c ON c.id = s.channel_id LEFT JOIN users u ON u.id = s.assigned_to
         WHERE s.started_at >= $1 AND s.started_at < $2 AND ($3::uuid IS NULL OR s.channel_id = $3)
           AND EXISTS (SELECT 1 FROM chat_messages m WHERE m.session_id = s.id AND m.author = 'visitor')
         ORDER BY s.started_at LIMIT 50000",
    )
    .bind(from)
    .bind(to)
    .bind(q.channel_id)
    .fetch_all(&state.db)
    .await?;
    let mut out = String::from("\u{feff}Chat,Channel,Visitor,Email,Operator,Status,Started,First answer,Closed,Rating,Visitor comment,Review,Transcript\r\n");
    for r in rows {
        let msgs = super::messages(&state, r.0, None).await?;
        let transcript: Vec<String> = msgs
            .iter()
            .filter(|m| m.author != "system")
            .map(|m| format!("{}: {}", if m.author == "visitor" { r.2.clone().unwrap_or_else(|| "Visitor".into()) } else { m.user_name.clone().unwrap_or_else(|| "Operator".into()) }, m.body))
            .collect();
        let t = |x: Option<DateTime<Utc>>| x.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default();
        let cells = [
            r.0.to_string(), r.1, r.2.unwrap_or_default(), r.3.unwrap_or_default(), r.4.unwrap_or_default(), r.5, t(Some(r.6)), t(r.7), t(r.8),
            r.9.map(|x| x.to_string()).unwrap_or_default(), r.10.unwrap_or_default(), r.11.map(|x| x.to_string()).unwrap_or_default(), transcript.join("\n"),
        ];
        out.push_str(&cells.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    let mut res = out.into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/csv; charset=utf-8"));
    res.headers_mut().insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"chats.csv\""));
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn working_hours() {
        let h = Some(json!([[1, 540, 1020]])); // Mondays 9–17
        let mon = |hh: u32| Utc.with_ymd_and_hms(2026, 11, 16, hh, 0, 0).unwrap();
        assert!(online(&h, "UTC", mon(10)));
        assert!(!online(&h, "UTC", mon(18)));
        assert!(!online(&h, "UTC", mon(10) + Duration::days(1)));
        assert!(online(&h, "Asia/Kolkata", mon(4))); // 09:30 in Kolkata
        assert!(online(&None, "UTC", mon(3)));
        assert_eq!(csv_cell("=SUM(A1)"), "'=SUM(A1)");
        assert_eq!(csv_cell("a,\"b\""), "\"a,\"\"b\"\"\"");
    }
}
