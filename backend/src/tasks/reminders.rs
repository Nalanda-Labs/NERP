//! Deadline reminders, run as a recurring background job (`tasks.deadlines`).
//!
//! - Due within 24 hours: remind the responsible person and assistants (once).
//! - Overdue: tell the responsible person, assistants and creator (once).
//!
//! Changing a task's deadline clears both markers, so the new deadline gets its own reminders.

use anyhow::Result;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{notify::notify, notify::NewNotification, state::AppState};

pub const JOB: &str = "tasks.deadlines";

pub async fn run(state: &AppState) -> Result<()> {
    // Claim rows by setting the marker in the same statement, so two workers never both send.
    let due_soon: Vec<(Uuid, String, Uuid, Option<DateTime<Utc>>)> = sqlx::query_as(
        "UPDATE tasks SET deadline_reminded_at = now()
         WHERE status IN ('new','in_progress') AND deadline_reminded_at IS NULL
           AND deadline > now() AND deadline <= now() + interval '24 hours'
         RETURNING id, title, responsible_id, deadline",
    )
    .fetch_all(&state.db)
    .await?;
    for (id, title, responsible, deadline) in &due_soon {
        let mut people = vec![*responsible];
        people.extend(assistants(state, *id).await);
        let when = deadline.map(|d| format!("Due {}", d.format("%Y-%m-%d %H:%M UTC")));
        send(state, &people, "task.due_soon", format!("Due soon: {title}"), when, *id).await;
    }

    let overdue: Vec<(Uuid, String, Uuid, Uuid)> = sqlx::query_as(
        "UPDATE tasks SET overdue_notified_at = now()
         WHERE status IN ('new','in_progress','review') AND overdue_notified_at IS NULL AND deadline < now()
         RETURNING id, title, responsible_id, created_by",
    )
    .fetch_all(&state.db)
    .await?;
    for (id, title, responsible, creator) in &overdue {
        let mut people = vec![*responsible, *creator];
        people.extend(assistants(state, *id).await);
        people.sort();
        people.dedup();
        send(state, &people, "task.overdue", format!("Overdue: {title}"), Some("The deadline has passed.".into()), *id).await;
        super::automation::fire(state, *id, "overdue", *creator).await;
    }
    if !due_soon.is_empty() || !overdue.is_empty() {
        tracing::info!(due_soon = due_soon.len(), overdue = overdue.len(), "sent task deadline reminders");
    }
    Ok(())
}

async fn assistants(state: &AppState, task: Uuid) -> Vec<Uuid> {
    sqlx::query_scalar("SELECT user_id FROM task_members WHERE task_id = $1 AND role = 'assistant'")
        .bind(task)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
}

/// Reminders come from the system, so they have no actor.
async fn send(state: &AppState, users: &[Uuid], kind: &'static str, title: String, body: Option<String>, task: Uuid) {
    notify(state, users, NewNotification { kind, title, body, link: Some(format!("/tasks/{task}")), actor: None }).await;
}
