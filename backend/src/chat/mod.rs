//! Live chat: a chat button on the company's website, answered from NERP.
//!
//! A channel is one chat button (its look, greeting and operators). A visitor's conversation
//! is a session: the first message links it to the CRM (the contact with the visitor's
//! email, or a new lead) and gives it to the next operator in turn, who is notified. New
//! messages reach operators live; visitors' pages poll. Closing a chat puts its transcript
//! on the CRM record's timeline.
//!
//! Operators see the chats of their channels; people with "Manage live chat" see all chats
//! and set channels up.

mod public;

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, tokens::random_token, AuthUser},
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/chat/channels", get(list_channels).post(create_channel))
        .route("/chat/channels/{id}", patch(update_channel).delete(delete_channel))
        .route("/chat/sessions", get(list_sessions))
        .route("/chat/sessions/{id}", get(get_session))
        .route("/chat/sessions/{id}/messages", post(reply))
        .route("/chat/sessions/{id}/take", post(take))
        .route("/chat/sessions/{id}/close", post(close))
        .route("/chat/sessions/{id}/read", post(mark_read))
        .route("/chat/counters", get(counters))
        .merge(public::router())
}

fn manager(auth: &AuthUser) -> bool {
    auth.can(perm::CHAT)
}

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct Channel {
    id: Uuid,
    name: String,
    key: String,
    title: String,
    greeting: String,
    color: String,
    ask_details: bool,
    operators: Vec<Uuid>,
    is_active: bool,
    open_chats: i64,
}

const CHANNEL_SELECT: &str = "SELECT c.id, c.name, c.key, c.title, c.greeting, c.color, c.ask_details, c.operators, c.is_active,
    (SELECT COUNT(*) FROM chat_sessions s WHERE s.channel_id = c.id AND s.status = 'open') AS open_chats
    FROM chat_channels c";

/// Channels the user works in (all of them for managers).
async fn list_channels(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let items: Vec<Channel> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{CHANNEL_SELECT} WHERE $1 OR $2 = ANY(c.operators) ORDER BY c.name")))
        .bind(manager(&auth))
        .bind(auth.id)
        .fetch_all(&state.db)
        .await?;
    let operators: Vec<(Uuid, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, trim(first_name || ' ' || last_name), avatar_file_id FROM users WHERE id IN (SELECT unnest(operators) FROM chat_channels)",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "items": items,
        "people": operators.into_iter().map(|(id, name, a)| json!({ "id": id, "name": name, "avatar_file_id": a })).collect::<Vec<_>>(),
        "public_base": state.config.public_url,
        "can_manage": manager(&auth),
    })))
}

#[derive(Deserialize)]
struct ChannelInput {
    name: Option<String>,
    title: Option<String>,
    greeting: Option<String>,
    color: Option<String>,
    ask_details: Option<bool>,
    operators: Option<Vec<Uuid>>,
    is_active: Option<bool>,
}

async fn check_operators(state: &AppState, ops: &[Uuid]) -> AppResult<()> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND is_active").bind(ops).fetch_one(&state.db).await?;
    if n as usize != ops.len() {
        return Err(AppError::bad("operators must be active employees"));
    }
    Ok(())
}

fn color(c: &str) -> AppResult<String> {
    let c = c.trim();
    if c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit()) {
        Ok(c.to_lowercase())
    } else {
        Err(AppError::bad("colour must look like #1a73e8"))
    }
}

async fn create_channel(State(state): State<AppState>, auth: AuthUser, Json(b): Json<ChannelInput>) -> AppResult<Json<Value>> {
    auth.require(perm::CHAT)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 120)?;
    let mut ops = b.operators.unwrap_or_else(|| vec![auth.id]);
    ops.dedup();
    check_operators(&state, &ops).await?;
    sqlx::query(
        "INSERT INTO chat_channels (name, key, title, greeting, color, ask_details, operators, created_by)
         VALUES ($1, $2, COALESCE($3, 'Chat with us'), COALESCE($4, 'Hi! How can we help?'), COALESCE($5, '#1a73e8'), COALESCE($6, TRUE), $7, $8)",
    )
    .bind(&name)
    .bind(random_token(12))
    .bind(b.title.as_deref().map(|t| required("title", t, 80)).transpose()?)
    .bind(b.greeting.as_deref().map(|t| required("greeting", t, 500)).transpose()?)
    .bind(b.color.as_deref().map(color).transpose()?)
    .bind(b.ask_details)
    .bind(&ops)
    .bind(auth.id)
    .execute(&state.db)
    .await?;
    list_channels(State(state), auth).await
}

async fn update_channel(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ChannelInput>) -> AppResult<Json<Value>> {
    auth.require(perm::CHAT)?;
    if let Some(ops) = &b.operators {
        check_operators(&state, ops).await?;
    }
    let n = sqlx::query(
        "UPDATE chat_channels SET name = COALESCE($2, name), title = COALESCE($3, title), greeting = COALESCE($4, greeting),
             color = COALESCE($5, color), ask_details = COALESCE($6, ask_details), operators = COALESCE($7, operators), is_active = COALESCE($8, is_active)
         WHERE id = $1",
    )
    .bind(id)
    .bind(b.name.as_deref().map(|t| required("name", t, 120)).transpose()?)
    .bind(b.title.as_deref().map(|t| required("title", t, 80)).transpose()?)
    .bind(b.greeting.as_deref().map(|t| required("greeting", t, 500)).transpose()?)
    .bind(b.color.as_deref().map(color).transpose()?)
    .bind(b.ask_details)
    .bind(b.operators)
    .bind(b.is_active)
    .execute(&state.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("channel"));
    }
    list_channels(State(state), auth).await
}

async fn delete_channel(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    auth.require(perm::CHAT)?;
    sqlx::query("DELETE FROM chat_channels WHERE id = $1").bind(id).execute(&state.db).await?;
    list_channels(State(state), auth).await
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct Session {
    id: Uuid,
    channel_id: Uuid,
    channel_name: String,
    visitor_name: Option<String>,
    visitor_email: Option<String>,
    page_url: Option<String>,
    status: String,
    assigned_to: Option<Uuid>,
    assigned_name: Option<String>,
    item_id: Option<Uuid>,
    item_code: Option<String>,
    item_title: Option<String>,
    unread: i32,
    last_message: Option<String>,
    started_at: DateTime<Utc>,
    last_message_at: DateTime<Utc>,
    closed_at: Option<DateTime<Utc>>,
}

const SESSION_SELECT: &str = "SELECT s.id, s.channel_id, c.name AS channel_name, s.visitor_name, s.visitor_email, s.page_url, s.status,
    s.assigned_to, trim(u.first_name || ' ' || u.last_name) AS assigned_name, s.item_id, t.code AS item_code, i.title AS item_title, s.unread,
    (SELECT m.body FROM chat_messages m WHERE m.session_id = s.id AND m.author <> 'system' ORDER BY m.created_at DESC LIMIT 1) AS last_message,
    s.started_at, s.last_message_at, s.closed_at
    FROM chat_sessions s JOIN chat_channels c ON c.id = s.channel_id
    LEFT JOIN users u ON u.id = s.assigned_to
    LEFT JOIN crm_items i ON i.id = s.item_id LEFT JOIN crm_entity_types t ON t.id = i.entity_type";

/// Who may see a channel's chats.
const VISIBLE: &str = "($1 OR $2 = ANY(c.operators))";

#[derive(Deserialize)]
struct SessionQuery {
    /// open (default) | closed
    status: Option<String>,
    /// "mine": assigned to me; "waiting": nobody's yet.
    who: Option<String>,
    q: Option<String>,
}

async fn list_sessions(State(state): State<AppState>, auth: AuthUser, Query(q): Query<SessionQuery>) -> AppResult<Json<Value>> {
    let status = if q.status.as_deref() == Some("closed") { "closed" } else { "open" };
    let term = clean(q.q).map(|t| format!("%{}%", t.to_lowercase().replace('%', "\\%").replace('_', "\\_")));
    let items: Vec<Session> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SESSION_SELECT} WHERE {VISIBLE} AND s.status = $3
           AND (CASE $4 WHEN 'mine' THEN s.assigned_to = $2 WHEN 'waiting' THEN s.assigned_to IS NULL ELSE TRUE END)
           AND ($5::text IS NULL OR lower(s.visitor_name) LIKE $5 OR lower(s.visitor_email) LIKE $5)
           AND EXISTS (SELECT 1 FROM chat_messages m WHERE m.session_id = s.id AND m.author = 'visitor')
         ORDER BY s.last_message_at DESC LIMIT 200"
    )))
    .bind(manager(&auth))
    .bind(auth.id)
    .bind(status)
    .bind(q.who.unwrap_or_default())
    .bind(term)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items })))
}

async fn session(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Session> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SESSION_SELECT} WHERE {VISIBLE} AND s.id = $3")))
        .bind(manager(auth))
        .bind(auth.id)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("chat"))
}

#[derive(Serialize, FromRow)]
pub(crate) struct Message {
    id: Uuid,
    author: String,
    user_id: Option<Uuid>,
    user_name: Option<String>,
    body: String,
    created_at: DateTime<Utc>,
}

pub(crate) async fn messages(state: &AppState, session: Uuid, after: Option<DateTime<Utc>>) -> AppResult<Vec<Message>> {
    Ok(sqlx::query_as(
        "SELECT m.id, m.author, m.user_id, trim(u.first_name || ' ' || u.last_name) AS user_name, m.body, m.created_at
         FROM chat_messages m LEFT JOIN users u ON u.id = m.user_id
         WHERE m.session_id = $1 AND ($2::timestamptz IS NULL OR m.created_at > $2) ORDER BY m.created_at LIMIT 1000",
    )
    .bind(session)
    .bind(after)
    .fetch_all(&state.db)
    .await?)
}

async fn get_session(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let s = session(&state, &auth, id).await?;
    let m = messages(&state, id, None).await?;
    Ok(Json(json!({ "session": s, "messages": m })))
}

/// Tells a channel's operators (and the chat's operator) that something changed.
pub(crate) async fn announce(state: &AppState, session: Uuid, kind: &str) {
    let who: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT u FROM chat_sessions s JOIN chat_channels c ON c.id = s.channel_id, LATERAL unnest(c.operators || COALESCE(ARRAY[s.assigned_to], '{}')) u
         WHERE s.id = $1 AND u IS NOT NULL",
    )
    .bind(session)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    // Managers watch every chat.
    let managers: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT ur.user_id FROM user_roles ur JOIN role_permissions rp ON rp.role_id = ur.role_id WHERE rp.permission IN ($1, '*')",
    )
    .bind(perm::CHAT)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let mut all = who;
    all.extend(managers);
    all.sort();
    all.dedup();
    crate::realtime::publish(state, Some(all), kind, json!({ "session_id": session })).await;
}

#[derive(Deserialize)]
struct ReplyBody {
    body: String,
}

async fn reply(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ReplyBody>) -> AppResult<Json<Value>> {
    let s = session(&state, &auth, id).await?;
    if s.status != "open" {
        return Err(AppError::bad("this chat is closed"));
    }
    let body = required("message", &b.body, 4000)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("INSERT INTO chat_messages (session_id, author, user_id, body) VALUES ($1, 'operator', $2, $3)").bind(id).bind(auth.id).bind(&body).execute(&mut *tx).await?;
    // Answering takes an unassigned chat, and means the operator has read it.
    sqlx::query("UPDATE chat_sessions SET assigned_to = COALESCE(assigned_to, $2), unread = 0, last_message_at = now() WHERE id = $1")
        .bind(id)
        .bind(auth.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    announce(&state, id, "chat.message").await;
    get_session(State(state), auth, Path(id)).await
}

async fn take(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    session(&state, &auth, id).await?;
    sqlx::query("UPDATE chat_sessions SET assigned_to = $2 WHERE id = $1").bind(id).bind(auth.id).execute(&state.db).await?;
    let name: String = sqlx::query_scalar("SELECT first_name FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await?;
    sqlx::query("INSERT INTO chat_messages (session_id, author, body) VALUES ($1, 'system', $2)").bind(id).bind(format!("{name} joined the chat")).execute(&state.db).await?;
    announce(&state, id, "chat.changed").await;
    get_session(State(state), auth, Path(id)).await
}

async fn mark_read(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    session(&state, &auth, id).await?;
    sqlx::query("UPDATE chat_sessions SET unread = 0 WHERE id = $1").bind(id).execute(&state.db).await?;
    announce(&state, id, "chat.changed").await;
    Ok(Json(json!({ "ok": true })))
}

/// Ends the chat and puts its transcript on the CRM record.
async fn close(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let s = session(&state, &auth, id).await?;
    if s.status == "closed" {
        return Err(AppError::bad("this chat is already closed"));
    }
    end(&state, id, Some(auth.id)).await?;
    get_session(State(state), auth, Path(id)).await
}

pub(crate) async fn end(state: &AppState, id: Uuid, by: Option<Uuid>) -> AppResult<()> {
    let n = sqlx::query("UPDATE chat_sessions SET status = 'closed', closed_at = now(), unread = 0 WHERE id = $1 AND status = 'open'")
        .bind(id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Ok(());
    }
    let who = match by {
        Some(u) => sqlx::query_scalar::<_, String>("SELECT first_name FROM users WHERE id = $1").bind(u).fetch_one(&state.db).await?,
        None => "The visitor".into(),
    };
    sqlx::query("INSERT INTO chat_messages (session_id, author, user_id, body) VALUES ($1, 'system', $2, $3)")
        .bind(id)
        .bind(by)
        .bind(format!("{who} ended the chat"))
        .execute(&state.db)
        .await?;
    let (item, channel, visitor): (Option<Uuid>, String, Option<String>) = sqlx::query_as(
        "SELECT s.item_id, c.name, s.visitor_name FROM chat_sessions s JOIN chat_channels c ON c.id = s.channel_id WHERE s.id = $1",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if let Some(item) = item {
        let m = messages(state, id, None).await?;
        let lines: Vec<Value> = m
            .iter()
            .filter(|x| x.author != "system")
            .map(|x| json!({ "who": if x.author == "visitor" { visitor.clone().unwrap_or_else(|| "Visitor".into()) } else { x.user_name.clone().unwrap_or_default() }, "visitor": x.author == "visitor", "body": x.body, "at": x.created_at }))
            .collect();
        crate::crm::timeline(&state.db, item, "chat", by, None, json!({ "session_id": id, "channel": channel, "count": lines.len(), "messages": lines })).await?;
        let et: Option<i16> = sqlx::query_scalar("SELECT entity_type FROM crm_items WHERE id = $1").bind(item).fetch_optional(&state.db).await?;
        if let Some(et) = et {
            crate::crm::broadcast(state, et, item, false).await;
        }
    }
    announce(state, id, "chat.changed").await;
    Ok(())
}

/// Open chats waiting for the user: unread, or nobody's yet in their channels.
async fn counters(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let (unread, waiting): (i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT COUNT(*) FILTER (WHERE s.unread > 0 AND (s.assigned_to = $2 OR s.assigned_to IS NULL)),
                COUNT(*) FILTER (WHERE s.assigned_to IS NULL)
         FROM chat_sessions s JOIN chat_channels c ON c.id = s.channel_id
         WHERE {VISIBLE} AND s.status = 'open' AND EXISTS (SELECT 1 FROM chat_messages m WHERE m.session_id = s.id AND m.author = 'visitor')"
    )))
    .bind(manager(&auth))
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(json!({ "unread": unread, "waiting": waiting })))
}
