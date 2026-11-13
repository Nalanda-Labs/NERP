//! Polls, reactions and stickers in chats, and tasks and calendar events made from a chat
//! or one of its messages.

use std::collections::HashMap;

use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{broadcast_message, members, message_view, names, seat, system_message, tell};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

/// Stickers NERP ships with (the app draws each one).
pub const STICKERS: &[&str] = &["thumbs-up", "party", "heart", "laugh", "clap", "fire", "ok", "thinking", "coffee", "rocket", "sad", "wave"];
/// Reactions a message can get.
pub const REACTIONS: &[&str] = &["👍", "❤️", "😂", "🎉", "😮", "😢", "👏", "🙏"];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/im/chats/{id}/polls", post(new_poll))
        .route("/im/messages/{id}/vote", post(vote))
        .route("/im/messages/{id}/close-poll", post(close_poll))
        .route("/im/messages/{id}/react", post(react))
        .route("/im/chats/{id}/task", post(make_task))
        .route("/im/chats/{id}/event", post(make_event))
}

/// Adds reactions and polls to messages. `viewer`: whose votes and reactions are "mine"
/// (nil when the same data goes to everyone).
pub async fn enrich(state: &AppState, viewer: Uuid, items: &mut [Value]) -> AppResult<()> {
    let ids: Vec<Uuid> = items.iter().filter_map(|m| m["id"].as_str().and_then(|s| s.parse().ok())).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let reactions: Vec<(Uuid, String, Vec<Uuid>, Vec<String>)> = sqlx::query!(r#"SELECT r.message_id AS "message_id!", r.emoji AS "emoji!", array_agg(r.user_id ORDER BY r.created_at) AS "c2!", array_agg(u.first_name ORDER BY r.created_at) AS "c3!"
         FROM im_reactions r JOIN users u ON u.id = r.user_id WHERE r.message_id = ANY($1)
         GROUP BY r.message_id, r.emoji ORDER BY MIN(r.created_at)"#, &ids).map(|r| (r.message_id, r.emoji, r.c2, r.c3))
    .fetch_all(&state.db)
    .await?;
    let mut by_msg: HashMap<Uuid, Vec<Value>> = HashMap::new();
    for (m, emoji, users, names) in reactions {
        by_msg.entry(m).or_default().push(json!({ "emoji": emoji, "count": users.len(), "mine": users.contains(&viewer), "names": names }));
    }
    let polls: Vec<(Uuid, String, Vec<String>, bool, bool, Option<DateTime<Utc>>)> =
        sqlx::query!(r#"SELECT message_id, question, options, multiple, anonymous, closed_at FROM im_polls WHERE message_id = ANY($1)"#, &ids).map(|r| (r.message_id, r.question, r.options, r.multiple, r.anonymous, r.closed_at))
            .fetch_all(&state.db)
            .await?;
    let mut poll_by: HashMap<Uuid, Value> = HashMap::new();
    if !polls.is_empty() {
        let pids: Vec<Uuid> = polls.iter().map(|p| p.0).collect();
        let votes: Vec<(Uuid, i32, Uuid, String)> = sqlx::query!(r#"SELECT v.message_id AS "message_id!", v.option AS "option!", v.user_id AS "user_id!", trim(u.first_name || ' ' || u.last_name) AS "c3!" FROM im_poll_votes v JOIN users u ON u.id = v.user_id
             WHERE v.message_id = ANY($1) ORDER BY v.voted_at"#, &pids).map(|r| (r.message_id, r.option, r.user_id, r.c3))
        .fetch_all(&state.db)
        .await?;
        for (id, question, options, multiple, anonymous, closed) in polls {
            let mine: Vec<i32> = votes.iter().filter(|v| v.0 == id && v.2 == viewer).map(|v| v.1).collect();
            let mut voters: Vec<Uuid> = votes.iter().filter(|v| v.0 == id).map(|v| v.2).collect();
            voters.sort();
            voters.dedup();
            let opts: Vec<Value> = options
                .iter()
                .enumerate()
                .map(|(i, text)| {
                    let these: Vec<&(Uuid, i32, Uuid, String)> = votes.iter().filter(|v| v.0 == id && v.1 == i as i32).collect();
                    json!({
                        "text": text,
                        "votes": these.len(),
                        // Who voted, unless the poll is anonymous.
                        "voters": if anonymous { json!([]) } else { json!(these.iter().map(|v| v.3.clone()).collect::<Vec<_>>()) },
                    })
                })
                .collect();
            poll_by.insert(id, json!({ "question": question, "multiple": multiple, "anonymous": anonymous, "closed": closed.is_some(), "options": opts, "voters": voters.len(), "my_votes": mine }));
        }
    }
    for m in items.iter_mut() {
        let Some(id) = m["id"].as_str().and_then(|s| s.parse::<Uuid>().ok()) else { continue };
        let deleted = m["deleted"].as_bool().unwrap_or(false);
        m["reactions"] = if deleted { json!([]) } else { json!(by_msg.remove(&id).unwrap_or_default()) };
        m["poll"] = if deleted { Value::Null } else { poll_by.remove(&id).unwrap_or(Value::Null) };
    }
    Ok(())
}

/// Tells the chat that a message's votes or reactions changed; each client fetches its own view.
async fn changed(state: &AppState, chat: Uuid, message: Uuid) -> AppResult<()> {
    let all = members(state, chat).await?;
    tell(state, all, "im.message.changed", json!({ "chat_id": chat, "message_id": message })).await;
    Ok(())
}

async fn message_chat(state: &AppState, id: Uuid) -> AppResult<(Uuid, Option<Uuid>, String, bool)> {
    sqlx::query!(r#"SELECT chat_id AS "chat_id!", user_id AS "user_id?", kind AS "kind!", deleted_at IS NOT NULL AS "c3!" FROM im_messages WHERE id = $1"#, id).map(|r| (r.chat_id, r.user_id, r.kind, r.c3))
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("message"))
}

// ---------------------------------------------------------------------------
// Polls
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PollInput {
    question: String,
    options: Vec<String>,
    #[serde(default)]
    multiple: bool,
    #[serde(default)]
    anonymous: bool,
    parent_id: Option<Uuid>,
}

async fn new_poll(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PollInput>) -> AppResult<Json<Value>> {
    let s = seat(&state, &auth, id).await?;
    if s.role.is_none() || (b.parent_id.is_none() && !s.post) {
        return Err(AppError::bad("you can't post in this chat"));
    }
    let question = required("question", &b.question, 300)?;
    let options: Vec<String> = b.options.into_iter().filter_map(|o| clean(Some(o))).map(|o| o.chars().take(200).collect()).collect();
    if options.len() < 2 || options.len() > 10 {
        return Err(AppError::bad("a poll has 2 to 10 answers"));
    }
    let mut tx = state.db.begin().await?;
    let mid: Uuid = sqlx::query_scalar!("INSERT INTO im_messages (chat_id, user_id, body, kind, parent_id) VALUES ($1, $2, $3, 'poll', $4) RETURNING id", id, auth.id, &question, b.parent_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query!("INSERT INTO im_polls (message_id, question, options, multiple, anonymous) VALUES ($1, $2, $3, $4, $5)", mid, &question, &options, b.multiple, b.anonymous)
        .execute(&mut *tx)
        .await?;
    if b.parent_id.is_none() {
        sqlx::query!("UPDATE im_chats SET last_message_at = now() WHERE id = $1", id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    broadcast_message(&state, mid).await?;
    Ok(Json(message_view(&state, auth.id, mid).await?))
}

#[derive(Deserialize)]
struct Vote {
    /// Indexes of the chosen answers; empty takes the vote back.
    options: Vec<i32>,
}

async fn vote(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Vote>) -> AppResult<Json<Value>> {
    let (chat, _, kind, deleted) = message_chat(&state, id).await?;
    if seat(&state, &auth, chat).await?.role.is_none() {
        return Err(AppError::bad("join the chat to vote"));
    }
    if kind != "poll" || deleted {
        return Err(AppError::bad("that isn't a poll"));
    }
    let (n, multiple, closed): (i32, bool, bool) =
        sqlx::query!(r#"SELECT cardinality(options) AS "c0!", multiple AS "multiple!", closed_at IS NOT NULL AS "c2!" FROM im_polls WHERE message_id = $1"#, id).map(|r| (r.c0, r.multiple, r.c2)).fetch_one(&state.db).await?;
    if closed {
        return Err(AppError::bad("the poll is closed"));
    }
    let mut chosen = b.options;
    chosen.sort();
    chosen.dedup();
    if chosen.iter().any(|o| *o < 0 || *o >= n) {
        return Err(AppError::bad("no such answer"));
    }
    if chosen.len() > 1 && !multiple {
        return Err(AppError::bad("choose one answer"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query!("DELETE FROM im_poll_votes WHERE message_id = $1 AND user_id = $2", id, auth.id).execute(&mut *tx).await?;
    for o in &chosen {
        sqlx::query!("INSERT INTO im_poll_votes (message_id, user_id, option) VALUES ($1, $2, $3)", id, auth.id, o).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    changed(&state, chat, id).await?;
    Ok(Json(message_view(&state, auth.id, id).await?))
}

async fn close_poll(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (chat, author, kind, _) = message_chat(&state, id).await?;
    let s = seat(&state, &auth, chat).await?;
    if kind != "poll" {
        return Err(AppError::bad("that isn't a poll"));
    }
    if author != Some(auth.id) && !s.manage {
        return Err(AppError::bad("only whoever asked closes the poll"));
    }
    sqlx::query!("UPDATE im_polls SET closed_at = COALESCE(closed_at, now()) WHERE message_id = $1", id).execute(&state.db).await?;
    changed(&state, chat, id).await?;
    Ok(Json(message_view(&state, auth.id, id).await?))
}

// ---------------------------------------------------------------------------
// Reactions
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct React {
    emoji: String,
}

/// Adds my reaction, or takes it back if I had it.
async fn react(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<React>) -> AppResult<Json<Value>> {
    let (chat, author, _, deleted) = message_chat(&state, id).await?;
    if seat(&state, &auth, chat).await?.role.is_none() {
        return Err(AppError::bad("join the chat to react"));
    }
    if deleted || author.is_none() {
        return Err(AppError::bad("you can't react to this message"));
    }
    if !REACTIONS.contains(&b.emoji.as_str()) {
        return Err(AppError::bad("unknown reaction"));
    }
    let removed = sqlx::query!("DELETE FROM im_reactions WHERE message_id = $1 AND user_id = $2 AND emoji = $3", id, auth.id, &b.emoji)
        .execute(&state.db)
        .await?
        .rows_affected();
    if removed == 0 {
        sqlx::query!("INSERT INTO im_reactions (message_id, user_id, emoji) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING", id, auth.id, &b.emoji)
            .execute(&state.db)
            .await?;
        if let Some(a) = author.filter(|a| *a != auth.id) {
            let who = names(&state, &[auth.id]).await?;
            crate::notify::notify(&state, &[a], crate::notify::NewNotification {
                kind: "im.reaction",
                title: format!("{who} reacted {} to your message", b.emoji),
                body: None,
                link: Some(format!("/messenger?chat={chat}")),
                actor: Some(auth.id),
            })
            .await;
        }
    }
    changed(&state, chat, id).await?;
    Ok(Json(message_view(&state, auth.id, id).await?))
}

// ---------------------------------------------------------------------------
// Tasks and events from a chat
// ---------------------------------------------------------------------------

/// The text of a message, quoted with who wrote it, for a task or event made from it.
async fn quote(state: &AppState, chat: Uuid, message: Option<Uuid>) -> AppResult<Option<(String, String)>> {
    let Some(m) = message else { return Ok(None) };
    let row: Option<(String, Option<String>, Option<Uuid>)> = sqlx::query!(r#"SELECT m.body AS "body!", trim(u.first_name || ' ' || u.last_name) AS "c1?", m.parent_id AS "parent_id?" FROM im_messages m LEFT JOIN users u ON u.id = m.user_id
         WHERE m.id = $1 AND m.chat_id = $2 AND m.deleted_at IS NULL"#, m, chat).map(|r| (r.body, r.c1, r.parent_id))
    .fetch_optional(&state.db)
    .await?;
    let Some((body, author, _)) = row else { return Err(AppError::bad("that message isn't in this chat")) };
    Ok(Some((body, author.unwrap_or_else(|| "NERP".into()))))
}

async fn chat_title(state: &AppState, auth: &AuthUser, chat: Uuid) -> AppResult<String> {
    Ok(super::chat_view(state, auth, chat).await?["title"].as_str().unwrap_or("chat").to_string())
}

#[derive(Deserialize)]
struct TaskFromChat {
    title: Option<String>,
    description: Option<String>,
    responsible_id: Option<Uuid>,
    deadline: Option<DateTime<Utc>>,
    /// Made from this message: its text goes into the task.
    message_id: Option<Uuid>,
}

async fn make_task(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<TaskFromChat>) -> AppResult<Json<Value>> {
    if seat(&state, &auth, id).await?.role.is_none() {
        return Err(AppError::NotFound("chat"));
    }
    let q = quote(&state, id, b.message_id).await?;
    let title = match clean(b.title) {
        Some(t) => t,
        None => match &q {
            Some((body, _)) => body.lines().next().unwrap_or("").chars().take(200).collect(),
            None => return Err(AppError::bad("give the task a title")),
        },
    };
    let from = chat_title(&state, &auth, id).await?;
    let mut description = clean(b.description).unwrap_or_default();
    if let Some((body, author)) = &q {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str(&format!("From the chat “{from}”, {author} wrote:\n\n> {}", body.replace('\n', "\n> ")));
    }
    let task = crate::tasks::items::create_task(&state, &auth, crate::tasks::items::CreateTask {
        title,
        description: Some(description).filter(|d| !d.is_empty()),
        responsible_id: b.responsible_id,
        deadline: b.deadline,
        ..Default::default()
    })
    .await?;
    let (number, title): (i64, String) = sqlx::query!(r#"SELECT number, title FROM tasks WHERE id = $1"#, task).map(|r| (r.number, r.title)).fetch_one(&state.db).await?;
    let who = names(&state, &[auth.id]).await?;
    system_message(&state, id, format!("{who} made a task: #{number} {title}")).await?;
    let all = members(&state, id).await?;
    tell(&state, all, "im.chat", json!({ "chat_id": id })).await;
    Ok(Json(json!({ "task_id": task, "number": number })))
}

#[derive(Deserialize)]
struct EventFromChat {
    title: Option<String>,
    description: Option<String>,
    start: String,
    end: String,
    #[serde(default)]
    all_day: bool,
    timezone: Option<String>,
    location: Option<String>,
    message_id: Option<Uuid>,
    /// Invite the chat's members (default yes).
    invite: Option<bool>,
}

async fn make_event(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<EventFromChat>) -> AppResult<Json<Value>> {
    if seat(&state, &auth, id).await?.role.is_none() {
        return Err(AppError::NotFound("chat"));
    }
    let q = quote(&state, id, b.message_id).await?;
    let title = match clean(b.title) {
        Some(t) => t,
        None => match &q {
            Some((body, _)) => body.lines().next().unwrap_or("").chars().take(200).collect(),
            None => return Err(AppError::bad("give the event a title")),
        },
    };
    let from = chat_title(&state, &auth, id).await?;
    let mut description = clean(b.description).unwrap_or_default();
    if let Some((body, author)) = &q {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str(&format!("From the chat “{from}”, {author} wrote: {body}"));
    }
    let guests: Vec<Uuid> = if b.invite.unwrap_or(true) { members(&state, id).await?.into_iter().filter(|u| *u != auth.id).collect() } else { vec![] };
    if guests.len() > 100 {
        return Err(AppError::bad("this chat has more than 100 people; make the event without inviting everyone"));
    }
    let ev = crate::calendar::create_event(&state, &auth, json!({
        "title": title,
        "description": Some(description).filter(|d| !d.is_empty()),
        "start": b.start,
        "end": b.end,
        "all_day": b.all_day,
        "timezone": b.timezone,
        "location": b.location,
        "attendees": guests,
    }))
    .await?;
    let who = names(&state, &[auth.id]).await?;
    system_message(&state, id, format!("{who} planned an event: {}", ev["title"].as_str().unwrap_or(""))).await?;
    let all = members(&state, id).await?;
    tell(&state, all, "im.chat", json!({ "chat_id": id })).await;
    Ok(Json(ev))
}
