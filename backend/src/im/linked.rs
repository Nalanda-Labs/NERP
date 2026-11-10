//! Chats that belong to a workgroup, task or calendar event: their name and members follow it.

use serde_json::json;
use uuid::Uuid;

use super::{members, names, system_message, tell};
use crate::{
    error::{AppError, AppResult},
    state::AppState,
};

// ---------------------------------------------------------------------------
// Workgroup chats
// ---------------------------------------------------------------------------

/// A workgroup's chat follows the group: its name and members change with the group's.
pub(super) async fn workgroup_guard(state: &AppState, chat: Uuid) -> AppResult<()> {
    let linked: bool = sqlx::query_scalar("SELECT project_id IS NOT NULL OR task_id IS NOT NULL OR event_id IS NOT NULL FROM im_chats WHERE id = $1")
        .bind(chat)
        .fetch_one(&state.db)
        .await?;
    if linked {
        return Err(AppError::bad("this chat belongs to a workgroup, task or event: its name and members follow it"));
    }
    Ok(())
}

/// What a linked chat belongs to: its name and members follow that.
#[derive(Clone, Copy)]
pub enum Link {
    Project(Uuid),
    Task(Uuid),
    Event(Uuid),
}

impl Link {
    fn id(self) -> Uuid {
        match self {
            Link::Project(id) | Link::Task(id) | Link::Event(id) => id,
        }
    }
    fn column(self) -> &'static str {
        match self {
            Link::Project(_) => "project_id",
            Link::Task(_) => "task_id",
            Link::Event(_) => "event_id",
        }
    }
    /// Everyone on it (`$2` is its id), active or not.
    fn people(self) -> &'static str {
        match self {
            Link::Project(_) => "SELECT user_id FROM project_members WHERE project_id = $2",
            Link::Task(_) => "SELECT created_by AS user_id FROM tasks WHERE id = $2 UNION SELECT responsible_id FROM tasks WHERE id = $2
                UNION SELECT user_id FROM task_members WHERE task_id = $2",
            Link::Event(_) => "SELECT owner_id AS user_id FROM cal_events WHERE id = $2 UNION SELECT user_id FROM cal_attendees WHERE event_id = $2",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Link::Project(_) => "SELECT name FROM projects WHERE id = $1",
            Link::Task(_) => "SELECT 'Task #' || number || ': ' || title FROM tasks WHERE id = $1",
            Link::Event(_) => "SELECT title FROM cal_events WHERE id = $1",
        }
    }
    fn welcome(self, name: &str) -> String {
        match self {
            Link::Project(_) => format!("The chat of “{name}”"),
            Link::Task(_) => format!("The chat of {name}"),
            Link::Event(_) => format!("The chat of the event “{name}”"),
        }
    }
    fn left(self) -> &'static str {
        match self {
            Link::Project(_) => "left the group",
            Link::Task(_) => "left the task",
            Link::Event(_) => "left the event",
        }
    }
    /// Whether `user` is on it.
    pub async fn has(self, state: &AppState, user: Uuid) -> AppResult<bool> {
        Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT EXISTS (SELECT 1 FROM ({}) p WHERE p.user_id = $1)", self.people())))
            .bind(user)
            .bind(self.id())
            .fetch_one(&state.db)
            .await?)
    }
}

/// The workgroup's chat, made on first use.
pub async fn project_chat(state: &AppState, project: Uuid) -> AppResult<Uuid> {
    linked_chat(state, Link::Project(project)).await
}

/// Brings the workgroup's chat (if it has one) in line with the group's name and members.
pub async fn sync_project_chat(state: &AppState, project: Uuid) -> AppResult<()> {
    sync_linked_chat(state, Link::Project(project)).await
}

/// The chat of a workgroup, task or event, made on first use.
pub async fn linked_chat(state: &AppState, link: Link) -> AppResult<Uuid> {
    let col = link.column();
    if let Some(id) = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT id FROM im_chats WHERE {col} = $1"))).bind(link.id()).fetch_optional(&state.db).await? {
        sync_linked_chat(state, link).await?;
        return Ok(id);
    }
    let name: String = sqlx::query_scalar(sqlx::AssertSqlSafe(link.title().to_string())).bind(link.id()).fetch_one(&state.db).await?;
    let made: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "INSERT INTO im_chats (kind, title, {col}) VALUES ('group', $1, $2) ON CONFLICT ({col}) DO NOTHING RETURNING id"
    )))
    .bind(&name)
    .bind(link.id())
    .fetch_optional(&state.db)
    .await?;
    // Someone else made it a moment ago.
    let Some(id) = made else {
        return Box::pin(linked_chat(state, link)).await;
    };
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO im_members (chat_id, user_id) SELECT $1, p.user_id FROM ({}) p JOIN users u ON u.id = p.user_id WHERE u.is_active ON CONFLICT DO NOTHING",
        link.people()
    )))
    .bind(id)
    .bind(link.id())
    .execute(&state.db)
    .await?;
    system_message(state, id, link.welcome(&name)).await?;
    let all = members(state, id).await?;
    tell(state, all, "im.chat", json!({ "chat_id": id })).await;
    Ok(id)
}

/// Who to tell once a task or event (and with it, its chat) is deleted: call before deleting
/// it, then hand the result to `linked_chat_gone`.
pub async fn linked_chat_people(state: &AppState, link: Link) -> AppResult<Option<(Uuid, Vec<Uuid>)>> {
    let id: Option<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT id FROM im_chats WHERE {} = $1", link.column()))).bind(link.id()).fetch_optional(&state.db).await?;
    Ok(match id {
        Some(id) => Some((id, members(state, id).await?)),
        None => None,
    })
}

pub async fn linked_chat_gone(state: &AppState, gone: Option<(Uuid, Vec<Uuid>)>) {
    if let Some((id, people)) = gone {
        tell(state, people, "im.chat", json!({ "chat_id": id })).await;
    }
}

/// Brings a linked chat (if there is one) in line with the name and people of what it belongs to.
pub async fn sync_linked_chat(state: &AppState, link: Link) -> AppResult<()> {
    let col = link.column();
    let Some((id, title)): Option<(Uuid, Option<String>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT id, title FROM im_chats WHERE {col} = $1"))).bind(link.id()).fetch_optional(&state.db).await?
    else {
        return Ok(());
    };
    let before = members(state, id).await?;
    let name: String = sqlx::query_scalar(sqlx::AssertSqlSafe(link.title().to_string())).bind(link.id()).fetch_one(&state.db).await?;
    if title.as_deref() != Some(name.as_str()) {
        sqlx::query("UPDATE im_chats SET title = $2 WHERE id = $1").bind(id).bind(&name).execute(&state.db).await?;
    }
    let added: Vec<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "INSERT INTO im_members (chat_id, user_id) SELECT $1, p.user_id FROM ({}) p JOIN users u ON u.id = p.user_id
         WHERE u.is_active ON CONFLICT DO NOTHING RETURNING user_id",
        link.people()
    )))
    .bind(id)
    .bind(link.id())
    .fetch_all(&state.db)
    .await?;
    let gone: Vec<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "DELETE FROM im_members WHERE chat_id = $1 AND user_id NOT IN ({}) RETURNING user_id",
        link.people()
    )))
    .bind(id)
    .bind(link.id())
    .fetch_all(&state.db)
    .await?;
    if !added.is_empty() {
        system_message(state, id, format!("{} joined", names(state, &added).await?)).await?;
    }
    if !gone.is_empty() {
        system_message(state, id, format!("{} {}", names(state, &gone).await?, link.left())).await?;
    }
    if !added.is_empty() || !gone.is_empty() || title.as_deref() != Some(name.as_str()) {
        let mut all = members(state, id).await?;
        all.extend(before);
        all.sort();
        all.dedup();
        tell(state, all, "im.chat", json!({ "chat_id": id })).await;
    }
    Ok(())
}

