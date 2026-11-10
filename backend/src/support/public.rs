//! The public help-desk page (`/help`): customers send a request without an account, and
//! follow it with the private link they get (`/help/t/<token>`): read the answers, write
//! again, close it, rate the answer.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{hash, tickets};
use crate::{
    auth::ClientIp,
    error::{AppError, AppResult},
    routes::{clean, files, normalize_email},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/public/support", get(portal))
        .route("/public/support/tickets", post(send))
        .route("/public/support/t/{token}", get(view))
        .route("/public/support/t/{token}/messages", post(write))
        .route("/public/support/t/{token}/close", post(close))
        .route("/public/support/t/{token}/rate", post(rate))
        .route("/public/support/t/{token}/files/{file}", get(file))
}

async fn limited(state: &AppState, key: String, max: i64, secs: i64) -> AppResult<bool> {
    let mut redis = state.redis.clone();
    let n: i64 = redis.incr(&key, 1).await?;
    if n == 1 {
        let _: () = redis.expire(&key, secs).await?;
    }
    Ok(n > max)
}

async fn enabled(state: &AppState) -> AppResult<(bool, Option<String>)> {
    Ok(sqlx::query_as("SELECT portal_enabled, portal_intro FROM support_settings WHERE id").fetch_one(&state.db).await?)
}

async fn choices(state: &AppState, kind: &str) -> AppResult<Vec<Value>> {
    let rows: Vec<(Uuid, String, bool)> = sqlx::query_as("SELECT id, name, is_default FROM support_dictionary WHERE kind = $1 ORDER BY sort, name").bind(kind).fetch_all(&state.db).await?;
    Ok(rows.into_iter().map(|(id, name, d)| json!({ "id": id, "name": name, "is_default": d })).collect())
}

async fn portal(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let (on, intro) = enabled(&state).await?;
    if !on {
        return Err(AppError::NotFound("help desk"));
    }
    Ok(Json(json!({
        "company": crate::routes::auth::company_name(&state).await,
        "intro": intro,
        "categories": choices(&state, "category").await?,
        "criticalities": choices(&state, "criticality").await?,
    })))
}

#[derive(Deserialize)]
struct SendInput {
    name: String,
    email: String,
    title: String,
    body: String,
    category_id: Option<Uuid>,
    criticality_id: Option<Uuid>,
    /// A field people do not see: robots fill it in.
    #[serde(default)]
    website: String,
}

async fn send(State(state): State<AppState>, ClientIp(ip): ClientIp, Json(b): Json<SendInput>) -> AppResult<Json<Value>> {
    if !enabled(&state).await?.0 {
        return Err(AppError::NotFound("help desk"));
    }
    if !b.website.is_empty() {
        return Err(AppError::bad("something went wrong; please try again"));
    }
    let email = normalize_email(&b.email).map_err(|_| AppError::bad("please enter your email address"))?;
    let name = clean(Some(b.name)).ok_or_else(|| AppError::bad("please enter your name"))?;
    if name.chars().count() > 200 {
        return Err(AppError::bad("the name is too long"));
    }
    let who = ip.unwrap_or_else(|| "unknown".into());
    if limited(&state, format!("support:ip:{who}"), 10, 3600).await? || limited(&state, format!("support:email:{email}"), 5, 3600).await? {
        return Err(AppError::TooManyRequests("too many requests in a short time; please try again later".into()));
    }
    let c = tickets::create(&state, tickets::NewTicket {
        title: b.title,
        body: b.body,
        channel: "portal",
        requester_id: None,
        requester_email: Some(email),
        requester_name: Some(name),
        category_id: b.category_id,
        criticality_id: b.criticality_id,
        file_ids: vec![],
        created_by: None,
        email_message_id: None,
    })
    .await?;
    Ok(Json(json!({ "number": c.number, "token": c.token })))
}

async fn by_token(state: &AppState, token: &str) -> AppResult<tickets::TicketRow> {
    let id: Uuid = sqlx::query_scalar("SELECT id FROM support_tickets WHERE access_hash = $1 AND NOT is_spam")
        .bind(hash(token))
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("request"))?;
    tickets::load(state, id).await
}

async fn view_json(state: &AppState, t: &tickets::TicketRow) -> AppResult<Value> {
    let rows: Vec<(Uuid, bool, Option<String>, String, Vec<Uuid>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT m.id, m.by_support, CASE WHEN m.by_support THEN u.first_name || ' ' || u.last_name ELSE COALESCE(m.author_name, m.author_email) END,
                m.body, m.file_ids, m.created_at
         FROM support_messages m LEFT JOIN users u ON u.id = m.author_id
         WHERE m.ticket_id = $1 AND m.kind = 'message' ORDER BY m.created_at, m.id",
    )
    .bind(t.id)
    .fetch_all(&state.db)
    .await?;
    let mut messages = Vec::new();
    for (id, by_support, name, body, file_ids, at) in rows {
        let files: Vec<(Uuid, String, i64)> = sqlx::query_as("SELECT id, original_name, size_bytes FROM files WHERE id = ANY($1)").bind(&file_ids).fetch_all(&state.db).await?;
        messages.push(json!({ "id": id, "by_support": by_support, "author_name": name.unwrap_or_else(|| "Support".into()), "body": body, "created_at": at,
                              "files": files.into_iter().map(|(id, name, size)| json!({ "id": id, "name": name, "size": size })).collect::<Vec<_>>() }));
    }
    let allow_reopen: bool = sqlx::query_scalar("SELECT allow_reopen FROM support_settings WHERE id").fetch_one(&state.db).await?;
    Ok(json!({
        "number": t.number, "title": t.title, "requester_name": t.requester_name, "created_at": t.created_at,
        "status": super::entry_name(state, t.status_id).await?, "category": super::entry_name(state, t.category_id).await?,
        "closed_at": t.closed_at, "waiting_for": t.waiting_for, "mark_id": t.mark_id,
        "can_write": t.closed_at.is_none() || allow_reopen,
        "marks": choices(state, "mark").await?,
        "company": crate::routes::auth::company_name(state).await,
        "messages": messages,
    }))
}

async fn view(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let t = by_token(&state, &token).await?;
    Ok(Json(view_json(&state, &t).await?))
}

#[derive(Deserialize)]
struct WriteInput {
    body: String,
}

async fn write(State(state): State<AppState>, ClientIp(ip): ClientIp, Path(token): Path<String>, Json(b): Json<WriteInput>) -> AppResult<Json<Value>> {
    let t = by_token(&state, &token).await?;
    let who = ip.unwrap_or_else(|| "unknown".into());
    if limited(&state, format!("support:write:{who}"), 30, 3600).await? {
        return Err(AppError::TooManyRequests("too many messages in a short time; please try again later".into()));
    }
    let email = t.requester_email.clone().unwrap_or_default();
    let author = tickets::Author::Customer { email, name: t.requester_name.clone() };
    tickets::add_message(&state, &t, author, false, tickets::NewMessage { body: b.body, note: false, file_ids: vec![], minutes_spent: 0, email_message_id: None }).await?;
    let t = tickets::load(&state, t.id).await?;
    Ok(Json(view_json(&state, &t).await?))
}

async fn close(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let t = by_token(&state, &token).await?;
    if t.closed_at.is_none() {
        sqlx::query("UPDATE support_tickets SET closed_at = now(), auto_closed = FALSE, deadline_at = NULL, notice_at = NULL, updated_at = now() WHERE id = $1")
            .bind(t.id)
            .execute(&state.db)
            .await?;
        tickets::log(&state, t.id, None, "Closed by the customer".into()).await?;
        if let Some(r) = t.responsible_id {
            crate::notify::notify(&state, &[r], crate::notify::NewNotification {
                kind: "support.closed",
                title: format!("#{} was closed by the customer: {}", t.number, t.title),
                body: None,
                link: Some(format!("/support/tickets/{}", t.id)),
                actor: None,
            })
            .await;
        }
    }
    let t = tickets::load(&state, t.id).await?;
    Ok(Json(view_json(&state, &t).await?))
}

async fn rate(State(state): State<AppState>, Path(token): Path<String>, Json(b): Json<tickets::RateInput>) -> AppResult<Json<Value>> {
    let t = by_token(&state, &token).await?;
    tickets::do_rate(&state, &t, b.mark_id, None).await?;
    let t = tickets::load(&state, t.id).await?;
    Ok(Json(view_json(&state, &t).await?))
}

/// A file on one of the request's messages (not on support's notes).
async fn file(State(state): State<AppState>, Path((token, file)): Path<(String, Uuid)>, headers: HeaderMap) -> AppResult<Response> {
    let t = by_token(&state, &token).await?;
    let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM support_messages WHERE ticket_id = $1 AND kind = 'message' AND $2 = ANY(file_ids))")
        .bind(t.id)
        .bind(file)
        .fetch_one(&state.db)
        .await?;
    if !ok {
        return Err(AppError::NotFound("file"));
    }
    let name: String = sqlx::query_scalar("SELECT original_name FROM files WHERE id = $1").bind(file).fetch_one(&state.db).await?;
    files::stream_id(&state, file, &name, &headers).await
}
