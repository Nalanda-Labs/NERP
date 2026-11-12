//! The visitor's side of live chat: no sign-in; a session token kept by the visitor's browser.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{announce, end, messages};
use crate::{
    auth::{tokens::random_token, ClientIp},
    crm::{automation::system_auth, broadcast, entity_type_by_id, items::{create_item, normalize}, CONTACT, LEAD},
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/public/chat/{key}", get(channel))
        .route("/public/chat/{key}/start", post(start))
        .route("/public/chat/{key}/session", get(poll))
        .route("/public/chat/{key}/messages", post(send))
        .route("/public/chat/{key}/end", post(visitor_end))
        .route("/public/chat/{key}/rate", post(rate))
}

#[derive(sqlx::FromRow)]
struct Channel {
    id: Uuid,
    name: String,
    title: String,
    greeting: String,
    color: String,
    ask_details: bool,
    operators: Vec<Uuid>,
    hours: Option<Value>,
    timezone: String,
    offline_message: Option<String>,
    auto_reply: Option<String>,
    consent_text: Option<String>,
    ask_rating: bool,
}

async fn active(state: &AppState, key: &str) -> AppResult<Channel> {
    sqlx::query_as(
        "SELECT id, name, title, greeting, color, ask_details, operators, hours, timezone, offline_message, auto_reply, consent_text, ask_rating
         FROM chat_channels WHERE key = $1 AND is_active",
    )
        .bind(key)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("chat"))
}

fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

async fn limited(state: &AppState, key: String, max: i64, secs: i64) -> AppResult<bool> {
    let mut redis = state.redis.clone();
    let n: i64 = redis.incr(&key, 1).await?;
    if n == 1 {
        let _: () = redis.expire(&key, secs).await?;
    }
    Ok(n > max)
}

async fn channel(State(state): State<AppState>, Path(key): Path<String>) -> AppResult<Json<Value>> {
    let c = active(&state, &key).await?;
    let online = super::ops::online(&c.hours, &c.timezone, Utc::now());
    Ok(Json(json!({
        "title": c.title, "greeting": c.greeting, "color": c.color, "ask_details": c.ask_details,
        "online": online, "offline_message": if online { None } else { c.offline_message.clone() },
        "consent_text": c.consent_text, "ask_rating": c.ask_rating,
    })))
}

#[derive(Deserialize)]
struct Start {
    #[serde(default)]
    name: String,
    #[serde(default)]
    email: String,
    page_url: Option<String>,
    /// Agreed to the channel's consent text.
    #[serde(default)]
    consent: bool,
    /// The website visitor id from the tracking script, if any.
    #[serde(default)]
    visitor_id: Option<String>,
}

fn opt(v: &str, max: usize) -> Option<String> {
    let t = v.trim();
    (!t.is_empty()).then(|| t.chars().take(max).collect())
}

async fn start(State(state): State<AppState>, ClientIp(ip): ClientIp, Path(key): Path<String>, Json(b): Json<Start>) -> AppResult<Json<Value>> {
    let c = active(&state, &key).await?;
    if limited(&state, format!("chat:start:{}", ip.unwrap_or_default()), 20, 600).await? {
        return Err(AppError::TooManyRequests("too many chats from here; please try again later".into()));
    }
    let name = opt(&b.name, 120);
    let email = opt(&b.email, 254).map(|e| e.to_lowercase());
    if email.as_ref().is_some_and(|e| !e.contains('@') || e.contains(char::is_whitespace)) {
        return Err(AppError::bad("please enter a valid email address"));
    }
    if c.ask_details && name.is_none() {
        return Err(AppError::bad("please tell us your name"));
    }
    if c.consent_text.is_some() && !b.consent {
        return Err(AppError::bad("please agree before starting the chat"));
    }
    let token = random_token(24);
    // Chats nobody wrote in are not kept.
    sqlx::query("DELETE FROM chat_sessions s WHERE s.started_at < now() - interval '1 day' AND NOT EXISTS (SELECT 1 FROM chat_messages m WHERE m.session_id = s.id AND m.author = 'visitor')")
        .execute(&state.db)
        .await?;
    sqlx::query(
        "INSERT INTO chat_sessions (channel_id, token_hash, visitor_name, visitor_email, page_url, consent_at, offline, visitor_id)
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $6 THEN now() END, $7, $8)",
    )
        .bind(c.id)
        .bind(hash(&token))
        .bind(name)
        .bind(email)
        .bind(b.page_url.and_then(|p| opt(&p, 2000)).filter(|p| p.starts_with("http")))
        .bind(c.consent_text.is_some())
        .bind(!super::ops::online(&c.hours, &c.timezone, Utc::now()))
        .bind(b.visitor_id.as_deref().and_then(|v| Uuid::parse_str(v.trim()).ok()))
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "token": token })))
}

#[derive(sqlx::FromRow)]
struct Session {
    id: Uuid,
    status: String,
    item_id: Option<Uuid>,
    assigned_to: Option<Uuid>,
    visitor_name: Option<String>,
    visitor_email: Option<String>,
    page_url: Option<String>,
    offline: bool,
    rating: Option<i16>,
}

async fn find(state: &AppState, channel: Uuid, token: &str) -> AppResult<Session> {
    sqlx::query_as("SELECT id, status, item_id, assigned_to, visitor_name, visitor_email, page_url, offline, rating FROM chat_sessions WHERE channel_id = $1 AND token_hash = $2")
        .bind(channel)
        .bind(hash(token))
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("chat"))
}

#[derive(Deserialize)]
struct PollQuery {
    token: String,
    after: Option<DateTime<Utc>>,
}

/// What the visitor sees: messages (operators by first name only) and whether it is open.
async fn poll(State(state): State<AppState>, Path(key): Path<String>, Query(q): Query<PollQuery>) -> AppResult<Json<Value>> {
    let c = active(&state, &key).await?;
    let s = find(&state, c.id, &q.token).await?;
    let m = messages(&state, s.id, q.after).await?;
    let operator: Option<String> = match s.assigned_to {
        Some(u) => sqlx::query_scalar("SELECT first_name FROM users WHERE id = $1").bind(u).fetch_optional(&state.db).await?,
        None => None,
    };
    let list: Vec<Value> = m
        .iter()
        .map(|x| {
            let v = serde_json::to_value(x).unwrap();
            json!({
                "id": v["id"], "author": v["author"], "body": v["body"], "created_at": v["created_at"],
                "name": v["user_name"].as_str().and_then(|n| n.split_whitespace().next()),
            })
        })
        .collect();
    Ok(Json(json!({ "status": s.status, "operator": operator, "messages": list, "ask_rating": c.ask_rating, "rating": s.rating })))
}

#[derive(Deserialize)]
struct SendBody {
    token: String,
    body: String,
}

async fn send(State(state): State<AppState>, Path(key): Path<String>, Json(b): Json<SendBody>) -> AppResult<Json<Value>> {
    let c = active(&state, &key).await?;
    let s = find(&state, c.id, &b.token).await?;
    if s.status != "open" {
        return Err(AppError::bad("this chat has ended; start a new one"));
    }
    let body = b.body.trim();
    if body.is_empty() {
        return Err(AppError::bad("the message is empty"));
    }
    if body.chars().count() > 4000 {
        return Err(AppError::bad("messages can be at most 4000 characters"));
    }
    if limited(&state, format!("chat:msg:{}", s.id), 30, 60).await? {
        return Err(AppError::TooManyRequests("you are sending messages too fast".into()));
    }
    let first: bool = !sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM chat_messages WHERE session_id = $1 AND author = 'visitor')")
        .bind(s.id)
        .fetch_one(&state.db)
        .await?;
    sqlx::query("INSERT INTO chat_messages (session_id, author, body) VALUES ($1, 'visitor', $2)").bind(s.id).bind(body).execute(&state.db).await?;
    let unread: i32 = sqlx::query_scalar("UPDATE chat_sessions SET unread = unread + 1, last_message_at = now() WHERE id = $1 RETURNING unread")
        .bind(s.id)
        .fetch_one(&state.db)
        .await?;
    if first {
        if let Err(e) = welcome(&state, &c, &s, body).await {
            tracing::warn!(session = %s.id, error = ?e, "could not link a chat to the CRM");
        }
        // The channel's automatic answer (or its away message, out of hours).
        let reply = if s.offline { c.offline_message.clone().or(c.auto_reply.clone()) } else { c.auto_reply.clone() };
        if let Some(r) = reply {
            super::ops::auto(&state, s.id, &r).await?;
        }
    } else if let Some(u) = s.assigned_to.filter(|_| unread == 1) {
        // Once per batch: further messages before the operator reads them only update the chat.
        notify(&state, &[u], NewNotification {
            kind: "chat.message",
            title: format!("{}: {}", s.visitor_name.clone().unwrap_or_else(|| "Visitor".into()), body.chars().take(80).collect::<String>()),
            body: None,
            link: Some(format!("/chats?session={}", s.id)),
            actor: None,
        })
        .await;
    }
    announce(&state, s.id, "chat.message").await;
    Ok(Json(json!({ "ok": true })))
}

/// The first message: links the chat to the CRM and gives it to the next operator.
async fn welcome(state: &AppState, c: &Channel, s: &Session, body: &str) -> AppResult<()> {
    let active_ops: Vec<Uuid> = sqlx::query_scalar("SELECT h FROM unnest($1::uuid[]) WITH ORDINALITY u(h, n) JOIN users ON users.id = h AND users.is_active ORDER BY n")
        .bind(&c.operators)
        .fetch_all(&state.db)
        .await?;
    // In turn (skipping those with their fill of chats), or offered to everyone.
    let operator = super::ops::pick(state, c.id, &[]).await?;
    let auth = system_auth();
    let mut item = s.item_id;
    if item.is_none()
        && let Some(email) = &s.visitor_email {
            item = sqlx::query_scalar(
                "SELECT i.id FROM crm_multifields m JOIN crm_items i ON i.id = m.item_id
                 WHERE m.kind = 'email' AND m.normalized = $1 AND i.entity_type IN ($2, $3) AND i.deleted_at IS NULL
                 ORDER BY (i.entity_type = $2) DESC, i.updated_at DESC LIMIT 1",
            )
            .bind(normalize("email", email))
            .bind(CONTACT)
            .bind(LEAD)
            .fetch_optional(&state.db)
            .await?;
        }
    if item.is_none() {
        let who = s.visitor_name.clone().or(s.visitor_email.clone()).unwrap_or_else(|| "website visitor".into());
        let mut m = Map::new();
        m.insert("title".into(), json!(format!("Chat: {who}")));
        if let Some(n) = &s.visitor_name {
            let (first, last) = match n.rsplit_once(' ') {
                Some((f, l)) => (f.to_string(), Some(l.to_string())),
                None => (n.clone(), None),
            };
            m.insert("first_name".into(), json!(first));
            if let Some(l) = last {
                m.insert("last_name".into(), json!(l));
            }
        }
        if let Some(e) = &s.visitor_email {
            m.insert("multifields".into(), json!([{ "kind": "email", "value_type": "work", "value": e }]));
        }
        m.insert("source".into(), json!("website"));
        m.insert("source_details".into(), json!(s.page_url.clone().unwrap_or_else(|| c.name.clone())));
        m.insert("comments".into(), json!(body.chars().take(2000).collect::<String>()));
        m.insert("assigned_to".into(), json!(operator));
        let t = entity_type_by_id(state, LEAD).await?;
        let id = create_item(state, &auth, &t, m, None).await?;
        // Their earlier emails (and chats) belong with the new lead.
        crate::crm::matching::adopt_history(state, id).await?;
        broadcast(state, LEAD, id, false).await;
        item = Some(id);
    }
    sqlx::query("UPDATE chat_sessions SET item_id = $2, assigned_to = COALESCE(assigned_to, $3), offered_at = CASE WHEN $3 IS NULL THEN offered_at ELSE now() END WHERE id = $1")
        .bind(s.id)
        .bind(item)
        .bind(operator)
        .execute(&state.db)
        .await?;
    // The visitor's journey on the website leads to this record.
    if let Some(i) = item {
        let vid: Option<Uuid> = sqlx::query_scalar("SELECT visitor_id FROM chat_sessions WHERE id = $1").bind(s.id).fetch_one(&state.db).await?;
        crate::tracking::link_visitor(state, vid.map(|v| v.to_string()).as_deref(), i).await;
    }
    let who = s.visitor_name.clone().unwrap_or_else(|| "A website visitor".into());
    let notify_to: Vec<Uuid> = operator.map(|o| vec![o]).unwrap_or(active_ops);
    notify(state, &notify_to, NewNotification {
        kind: "chat.new",
        title: format!("{who} started a chat on “{}”", c.name),
        body: Some(body.chars().take(200).collect()),
        link: Some(format!("/chats?session={}", s.id)),
        actor: None,
    })
    .await;
    Ok(())
}

#[derive(Deserialize)]
struct EndBody {
    token: String,
}

async fn visitor_end(State(state): State<AppState>, Path(key): Path<String>, Json(b): Json<EndBody>) -> AppResult<Json<Value>> {
    let c = active(&state, &key).await?;
    let s = find(&state, c.id, &b.token).await?;
    end(&state, s.id, None).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct RateBody {
    token: String,
    rating: i16,
    #[serde(default)]
    comment: Option<String>,
}

/// The visitor's mark for the chat (once it has started; can be changed).
async fn rate(State(state): State<AppState>, Path(key): Path<String>, Json(b): Json<RateBody>) -> AppResult<Json<Value>> {
    let c = active(&state, &key).await?;
    if !c.ask_rating {
        return Err(AppError::bad("this chat doesn't take ratings"));
    }
    let s = find(&state, c.id, &b.token).await?;
    if !(1..=5).contains(&b.rating) {
        return Err(AppError::bad("rate 1 to 5"));
    }
    sqlx::query("UPDATE chat_sessions SET rating = $2, rating_comment = $3 WHERE id = $1")
        .bind(s.id)
        .bind(b.rating)
        .bind(b.comment.and_then(|x| opt(&x, 2000)))
        .execute(&state.db)
        .await?;
    announce(&state, s.id, "chat.changed").await;
    Ok(Json(json!({ "ok": true })))
}
