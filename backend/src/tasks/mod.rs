//! Tasks & Projects module.

pub mod access;
pub mod automation;
pub mod depth;
mod deps;
mod flows;
pub mod items;
mod parts;
mod projects;
mod reports;
pub mod recurrence;
pub mod reminders;
mod scrum;
pub mod stages;
pub mod templates;

use axum::Router;
use serde_json::json;
use uuid::Uuid;

use crate::{
    notify::{notify, NewNotification},
    realtime,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(projects::router())
        .merge(items::router())
        .merge(parts::router())
        .merge(stages::router())
        .merge(deps::router())
        .merge(templates::router())
        .merge(scrum::router())
        .merge(depth::router())
        .merge(automation::router())
        .merge(reports::router())
        .merge(flows::router())
}

/// Creator, responsible person, assistants and watchers.
pub async fn participants(state: &AppState, task: Uuid) -> Vec<Uuid> {
    sqlx::query_scalar(
        "SELECT created_by FROM tasks WHERE id = $1
         UNION SELECT responsible_id FROM tasks WHERE id = $1
         UNION SELECT user_id FROM task_members WHERE task_id = $1",
    )
    .bind(task)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
}

/// Tells everyone who might be looking at a task (participants and project members) that it
/// changed, so open lists, boards and detail pages refresh themselves.
pub async fn broadcast_changed(state: &AppState, task: Uuid, project: Option<Uuid>, deleted: bool) {
    // The task's chat follows its title and people.
    if !deleted {
        if let Err(e) = crate::im::sync_linked_chat(state, crate::im::Link::Task(task)).await {
            tracing::warn!("task chat sync: {e:?}");
        }
    }
    let mut users = participants(state, task).await;
    if let Some(p) = project {
        let members: Vec<Uuid> = sqlx::query_scalar("SELECT user_id FROM project_members WHERE project_id = $1")
            .bind(p)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
        users.extend(members);
    }
    users.sort();
    users.dedup();
    realtime::publish(
        state,
        Some(users),
        "task.changed",
        json!({ "id": task, "project_id": project, "deleted": deleted }),
    )
    .await;
}

/// Records one change for the task's History tab.
pub async fn record_history<'e, E: sqlx::PgExecutor<'e>>(
    executor: E,
    task: Uuid,
    actor: Uuid,
    field: &str,
    old: Option<String>,
    new: Option<String>,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO task_history (task_id, actor_id, field, old_value, new_value) VALUES ($1, $2, $3, $4, $5)")
        .bind(task)
        .bind(actor)
        .bind(field)
        .bind(old)
        .bind(new)
        .execute(executor)
        .await?;
    Ok(())
}

pub async fn user_name(state: &AppState, user: Uuid) -> String {
    sqlx::query_scalar("SELECT first_name || ' ' || last_name FROM users WHERE id = $1")
        .bind(user)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "Someone".into())
}

/// Sends a task notification to `users` (the actor is skipped automatically).
pub async fn notify_task(state: &AppState, users: &[Uuid], kind: &'static str, title: String, body: Option<String>, task: Uuid, actor: Uuid) {
    notify(
        state,
        users,
        NewNotification { kind, title, body, link: Some(format!("/tasks/{task}")), actor: Some(actor) },
    )
    .await;
}

/// Bumps the activity timestamp used for "recently active" sorting.
pub async fn touch(state: &AppState, task: Uuid) {
    let _ = sqlx::query("UPDATE tasks SET last_activity_at = now() WHERE id = $1")
        .bind(task)
        .execute(&state.db)
        .await;
}
