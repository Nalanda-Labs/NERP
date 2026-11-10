//! Working with an email beyond replying: make a task or a calendar event from it, or bring it
//! to colleagues in a chat or the news feed. Whoever can read the email can do these.

use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::can_read_email;
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::clean,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/emails/{id}/task", post(make_task))
        .route("/crm/emails/{id}/event", post(make_event))
        .route("/crm/emails/{id}/share", post(share))
}

struct Mail {
    subject: String,
    from: String,
    text: String,
    /// The records it is filed on (that the user can open), and a link to the first.
    items: Vec<Uuid>,
    link: Option<String>,
}

async fn mail(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Mail> {
    if !can_read_email(state, auth, id).await? {
        return Err(AppError::NotFound("email"));
    }
    let (subject, from_name, from_addr, text, snippet): (String, String, String, Option<String>, String) =
        sqlx::query_as("SELECT subject, from_name, from_addr, body_text, snippet FROM crm_emails WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let items: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT l.item_id, t.code FROM crm_email_links l JOIN crm_items i ON i.id = l.item_id JOIN crm_entity_types t ON t.id = i.entity_type WHERE l.email_id = $1",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let mut visible = Vec::new();
    for (i, code) in items {
        if let Ok(r) = crate::crm::items::load_row(state, i, false).await
            && crate::crm::access::require(state, auth, r.entity_type, "read", r.assigned_to).await.is_ok()
        {
            visible.push((i, code));
        }
    }
    let from = if from_name.trim().is_empty() { from_addr.clone() } else { format!("{from_name} <{from_addr}>") };
    let text: String = text.unwrap_or(snippet).chars().take(20_000).collect();
    Ok(Mail {
        subject: if subject.trim().is_empty() { "(no subject)".into() } else { subject },
        from,
        text,
        link: visible.first().map(|(i, c)| format!("{}/crm/{c}/{i}", state.config.public_url)),
        items: visible.into_iter().map(|(i, _)| i).collect(),
    })
}

#[derive(Deserialize, Default)]
struct TaskBody {
    title: Option<String>,
    responsible_id: Option<Uuid>,
    deadline: Option<DateTime<Utc>>,
}

/// A task from the email: its subject is the title, the message the description, and the task
/// is linked to the email's CRM records.
async fn make_task(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, body: Option<Json<TaskBody>>) -> AppResult<Json<Value>> {
    let b = body.map(|b| b.0).unwrap_or_default();
    let m = mail(&state, &auth, id).await?;
    let task = crate::tasks::items::create_task(&state, &auth, crate::tasks::items::CreateTask {
        title: clean(b.title).unwrap_or_else(|| m.subject.chars().take(500).collect()),
        description: Some(format!("Email from {}:\n\n{}", m.from, m.text)),
        responsible_id: b.responsible_id,
        deadline: b.deadline,
        crm_item_ids: m.items,
        ..Default::default()
    })
    .await?;
    let number: i64 = sqlx::query_scalar("SELECT number FROM tasks WHERE id = $1").bind(task).fetch_one(&state.db).await?;
    Ok(Json(json!({ "task_id": task, "number": number })))
}

#[derive(Deserialize)]
struct EventBody {
    title: Option<String>,
    start: String,
    end: String,
    timezone: Option<String>,
    #[serde(default)]
    attendees: Vec<Uuid>,
}

async fn make_event(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<EventBody>) -> AppResult<Json<Value>> {
    let m = mail(&state, &auth, id).await?;
    let ev = crate::calendar::create_event(&state, &auth, json!({
        "title": clean(b.title).unwrap_or_else(|| m.subject.chars().take(300).collect()),
        "description": format!("Email from {}:\n\n{}", m.from, m.text.chars().take(5000).collect::<String>()),
        "start": b.start,
        "end": b.end,
        "timezone": b.timezone,
        "attendees": b.attendees,
        "item_id": m.items.first(),
    }))
    .await?;
    Ok(Json(ev))
}

#[derive(Deserialize)]
struct ShareBody {
    /// A chat to post it in (one I'm in).
    chat_id: Option<Uuid>,
    /// Or the news feed: to everyone, or to these people.
    #[serde(default)]
    feed: bool,
    #[serde(default)]
    users: Vec<Uuid>,
    /// A few words of my own.
    comment: Option<String>,
}

/// Brings the email to colleagues: its sender, subject and text, with a link to its record.
async fn share(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ShareBody>) -> AppResult<Json<Value>> {
    let m = mail(&state, &auth, id).await?;
    let excerpt: String = m.text.chars().take(1500).collect();
    let mut body = String::new();
    if let Some(c) = clean(b.comment) {
        body.push_str(&c);
        body.push_str("\n\n");
    }
    body.push_str(&format!("📧 {} — “{}”\n\n{}", m.from, m.subject, excerpt));
    if m.text.chars().count() > 1500 {
        body.push('…');
    }
    if let Some(l) = &m.link {
        body.push_str(&format!("\n\n{l}"));
    }
    match (b.chat_id, b.feed) {
        (Some(chat), false) => {
            let s = crate::im::seat(&state, &auth, chat).await?;
            if !s.post {
                return Err(AppError::bad("you can't write in that chat"));
            }
            crate::im::post_message(&state, chat, Some(auth.id), &body, &[]).await?;
            Ok(Json(json!({ "chat_id": chat })))
        }
        (None, true) => {
            let post = crate::feed::create_post(&state, &auth, json!({ "body": body, "to_all": b.users.is_empty(), "users": b.users })).await?;
            Ok(Json(json!({ "post_id": post["id"] })))
        }
        _ => Err(AppError::bad("share to a chat or to the feed")),
    }
}
