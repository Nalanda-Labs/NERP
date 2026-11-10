//! Messenger: conversations between employees, live.
//!
//! - **Direct** chats: one per pair of people.
//! - **Group** chats: a title, a description and members. Private ones are for the people
//!   invited; open ones are listed for everyone, who can read them and join.
//! - **Channels**: the owner and admins post, members read and comment under each post.
//! - Some chats keep their members in step with the company: the **general chat** (every
//!   active employee), **department** chats and channels (a department with its
//!   sub-departments), and the chats of workgroups, tasks and events (see `linked`).
//!
//! In a chat, the **owner** (whoever made it) and **admins** change its settings, add and
//! remove people and, with "only managers write", are the only ones who post. Messages carry
//! text and files (readable by the chat's members), voice and video notes, stickers and polls;
//! they get reactions, can be edited or deleted by their author, and each member's read
//! position gives unread counts. History can be cleared for oneself, or for everyone by the
//! chat's managers, and a chat can delete its messages after a set time.

mod extras;
mod linked;
mod social;

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

pub use linked::{linked_chat, linked_chat_gone, linked_chat_people, project_chat, sync_linked_chat, sync_project_chat, Link};
pub use social::{run_maintenance, sync_audiences, MAINTENANCE_JOB};

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/im/chats", get(list).post(create_group))
        .route("/im/direct/{user}", post(direct))
        .route("/im/chats/{id}", get(get_chat).patch(update_chat))
        .route("/im/chats/{id}/messages", get(messages).post(send))
        .route("/im/chats/{id}/read", post(read))
        .route("/im/chats/{id}/leave", post(leave))
        .route("/im/messages/{id}", get(get_message).patch(edit).delete(remove))
        .route("/im/unread", get(unread))
        .merge(social::router())
        .merge(extras::router())
}

// ---------------------------------------------------------------------------
// Membership and roles
// ---------------------------------------------------------------------------

/// Members of a chat (for live events).
pub(crate) async fn members(state: &AppState, chat: Uuid) -> AppResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar("SELECT user_id FROM im_members WHERE chat_id = $1").bind(chat).fetch_all(&state.db).await?)
}

/// The chat's members, or NotFound when the user isn't one.
async fn member(state: &AppState, auth: &AuthUser, chat: Uuid) -> AppResult<Vec<Uuid>> {
    let m = members(state, chat).await?;
    if !m.contains(&auth.id) {
        return Err(AppError::NotFound("chat"));
    }
    Ok(m)
}

/// What the user may do in a chat.
#[derive(Debug, Clone)]
pub(crate) struct Seat {
    pub kind: String,
    pub audience: String,
    pub visibility: String,
    pub is_general: bool,
    pub linked: bool,
    /// None: not a member (only possible for open chats, which can be read before joining).
    pub role: Option<String>,
    /// Owner, admin, or (for chats that follow the company) someone who runs that.
    pub manage: bool,
    /// May start a new message (not a comment) in it.
    pub post: bool,
}

pub(crate) async fn seat(state: &AppState, auth: &AuthUser, chat: Uuid) -> AppResult<Seat> {
    #[allow(clippy::type_complexity)]
    let row: Option<(String, String, String, String, bool, bool, Option<Uuid>, Option<String>, bool)> = sqlx::query_as(
        "SELECT c.kind, c.audience, c.visibility, c.post_policy, c.is_general,
                (c.project_id IS NOT NULL OR c.task_id IS NOT NULL OR c.event_id IS NOT NULL),
                c.department_id,
                (SELECT role FROM im_members WHERE chat_id = c.id AND user_id = $2),
                EXISTS (SELECT 1 FROM departments d WHERE d.id = c.department_id AND d.head_user_id = $2)
         FROM im_chats c WHERE c.id = $1",
    )
    .bind(chat)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;
    let Some((kind, audience, visibility, post_policy, is_general, linked, _dept, role, is_head)) = row else {
        return Err(AppError::NotFound("chat"));
    };
    if role.is_none() && visibility != "open" {
        return Err(AppError::NotFound("chat"));
    }
    let company = match audience.as_str() {
        "everyone" => auth.can(perm::SETTINGS_MANAGE),
        "department" => is_head || auth.can(perm::DEPARTMENTS_MANAGE),
        _ => false,
    };
    let manage = company || matches!(role.as_deref(), Some("owner") | Some("admin"));
    let managers_only = kind == "channel" || post_policy == "managers";
    let post = role.is_some() && (!managers_only || manage);
    Ok(Seat { kind, audience, visibility, is_general, linked, role, manage, post })
}

pub(crate) async fn tell(state: &AppState, who: Vec<Uuid>, kind: &str, data: Value) {
    crate::realtime::publish(state, Some(who), kind, data).await;
}

/// Files attached to messages are readable by the chat's members (and readers of open chats).
pub async fn can_view_file(state: &AppState, auth: &AuthUser, file: Uuid) -> AppResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM im_messages m JOIN im_chats c ON c.id = m.chat_id
                        WHERE $1 = ANY(m.file_ids) AND m.deleted_at IS NULL
                          AND (c.visibility = 'open' OR EXISTS (SELECT 1 FROM im_members mm WHERE mm.chat_id = m.chat_id AND mm.user_id = $2)))",
    )
    .bind(file)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?)
}

// ---------------------------------------------------------------------------
// Chats
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct ChatRow {
    id: Uuid,
    kind: String,
    /// Groups and channels: their title; direct chats: the other person's name.
    title: String,
    description: Option<String>,
    /// Direct chats: the other person.
    other_id: Option<Uuid>,
    avatar_file_id: Option<Uuid>,
    members: i64,
    unread: i64,
    last_body: Option<String>,
    last_author: Option<String>,
    last_message_at: DateTime<Utc>,
    created_by: Option<Uuid>,
    /// A workgroup's, task's or event's chat: its members are the people on it.
    project_id: Option<Uuid>,
    task_id: Option<Uuid>,
    event_id: Option<Uuid>,
    visibility: String,
    audience: String,
    department_id: Option<Uuid>,
    department_name: Option<String>,
    is_general: bool,
    post_policy: String,
    auto_delete_hours: Option<i32>,
    /// My role; None when reading an open chat I haven't joined.
    my_role: Option<String>,
    muted: bool,
    /// My folders this chat is in.
    folder_ids: Vec<Uuid>,
}

/// `$1`: the user. Chats they are in, or (`LEFT JOIN`ed) open ones they read.
const CHAT_COLUMNS: &str = "c.id, c.kind,
    CASE WHEN c.kind = 'direct' THEN COALESCE(trim(o.first_name || ' ' || o.last_name), 'Former employee') ELSE c.title END AS title,
    c.description, o.id AS other_id, o.avatar_file_id,
    (SELECT COUNT(*) FROM im_members x WHERE x.chat_id = c.id) AS members,
    CASE WHEN me.user_id IS NULL THEN 0 ELSE
      (SELECT COUNT(*) FROM im_messages m WHERE m.chat_id = c.id AND m.parent_id IS NULL AND m.created_at > me.last_read_at AND m.user_id IS DISTINCT FROM $1 AND m.deleted_at IS NULL) END AS unread,
    (SELECT CASE WHEN m.deleted_at IS NOT NULL THEN 'Message deleted' WHEN m.kind = 'poll' THEN 'Poll: ' || left(m.body, 180)
                 WHEN m.kind = 'voice' THEN 'Voice message' WHEN m.kind = 'video' THEN 'Video message' WHEN m.kind = 'sticker' THEN 'Sticker'
                 WHEN m.body = '' AND cardinality(m.file_ids) > 0 THEN 'File' ELSE left(m.body, 200) END
       FROM im_messages m WHERE m.chat_id = c.id AND m.parent_id IS NULL AND (me.cleared_at IS NULL OR m.created_at > me.cleared_at) ORDER BY m.created_at DESC LIMIT 1) AS last_body,
    (SELECT u.first_name FROM im_messages m LEFT JOIN users u ON u.id = m.user_id WHERE m.chat_id = c.id AND m.parent_id IS NULL AND (me.cleared_at IS NULL OR m.created_at > me.cleared_at) ORDER BY m.created_at DESC LIMIT 1) AS last_author,
    c.last_message_at, c.created_by, c.project_id, c.task_id, c.event_id,
    c.visibility, c.audience, c.department_id, (SELECT name FROM departments d WHERE d.id = c.department_id) AS department_name,
    c.is_general, c.post_policy, c.auto_delete_hours, me.role AS my_role, COALESCE(me.muted, FALSE) AS muted,
    ARRAY(SELECT fc.folder_id FROM im_folder_chats fc JOIN im_folders f ON f.id = fc.folder_id WHERE fc.chat_id = c.id AND f.user_id = $1) AS folder_ids";

const CHAT_FROM: &str = "FROM im_chats c LEFT JOIN im_members me ON me.chat_id = c.id AND me.user_id = $1
    LEFT JOIN LATERAL (SELECT u.* FROM im_members x JOIN users u ON u.id = x.user_id WHERE c.kind = 'direct' AND x.chat_id = c.id AND x.user_id <> $1 LIMIT 1) o ON TRUE";

#[derive(Deserialize)]
struct ListQuery {
    q: Option<String>,
    /// direct | group | channel
    kind: Option<String>,
    /// A folder of mine.
    folder_id: Option<Uuid>,
    /// "true": only chats with unread messages.
    unread: Option<String>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    // The general chat and department chats follow the company; bring mine up to date.
    social::sync_audiences(&state, Some(auth.id)).await?;
    let term = q.q.map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).map(|t| format!("%{}%", t.replace('%', "\\%").replace('_', "\\_")));
    let items: Vec<ChatRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT * FROM (SELECT {CHAT_COLUMNS} {CHAT_FROM} WHERE me.user_id IS NOT NULL) x
         WHERE ($2::text IS NULL OR lower(x.title) LIKE $2) AND ($3::text IS NULL OR x.kind = $3)
           AND ($4::uuid IS NULL OR $4 = ANY(x.folder_ids)) AND (NOT $5 OR x.unread > 0)
         ORDER BY x.last_message_at DESC LIMIT 300"
    )))
    .bind(auth.id)
    .bind(term)
    .bind(q.kind)
    .bind(q.folder_id)
    .bind(q.unread.as_deref() == Some("true"))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items })))
}

async fn chat_row(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<ChatRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {CHAT_COLUMNS} {CHAT_FROM} WHERE c.id = $2 AND (me.user_id IS NOT NULL OR c.visibility = 'open')"
    )))
    .bind(auth.id)
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("chat"))
}

pub(crate) async fn chat_view(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Value> {
    let c = chat_row(state, auth, id).await?;
    let s = seat(state, auth, id).await?;
    let people: Vec<(Uuid, String, Option<Uuid>, DateTime<Utc>, String)> = sqlx::query_as(
        "SELECT u.id, trim(u.first_name || ' ' || u.last_name), u.avatar_file_id, m.last_read_at, m.role FROM im_members m JOIN users u ON u.id = m.user_id
         WHERE m.chat_id = $1 ORDER BY CASE m.role WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 ELSE 2 END, u.first_name, u.last_name LIMIT 1000",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let mut v = serde_json::to_value(c).unwrap();
    v["people"] = json!(people.into_iter().map(|(id, name, a, read, role)| json!({ "id": id, "name": name, "avatar_file_id": a, "last_read_at": read, "role": role })).collect::<Vec<_>>());
    v["can"] = json!({
        "manage": s.manage,
        "post": s.post,
        "join": s.role.is_none(),
        // Members of chats that follow something else are not edited here.
        "edit_people": s.manage && s.audience == "manual" && !s.linked && s.kind != "direct",
        "add_people": s.role.is_some() && s.audience == "manual" && !s.linked && s.kind == "group" || (s.manage && s.audience == "manual" && !s.linked && s.kind == "channel"),
        "leave": s.role.is_some() && s.kind != "direct" && s.audience == "manual" && !s.linked,
    });
    Ok(v)
}

async fn get_chat(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    Ok(Json(chat_view(&state, &auth, id).await?))
}

async fn active_users(state: &AppState, ids: &[Uuid]) -> AppResult<()> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND is_active").bind(ids).fetch_one(&state.db).await?;
    if n as usize != ids.len() {
        return Err(AppError::bad("you can only chat with active employees"));
    }
    Ok(())
}

/// The direct chat with someone, made the first time.
async fn direct(State(state): State<AppState>, auth: AuthUser, Path(user): Path<Uuid>) -> AppResult<Json<Value>> {
    if user == auth.id {
        return Err(AppError::bad("pick someone else"));
    }
    active_users(&state, &[user]).await?;
    let mut pair = [auth.id, user];
    pair.sort();
    let key = format!("{}:{}", pair[0], pair[1]);
    let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM im_chats WHERE direct_key = $1").bind(&key).fetch_optional(&state.db).await?;
    let id = match existing {
        Some(id) => id,
        None => {
            let mut tx = state.db.begin().await?;
            let id: Option<Uuid> = sqlx::query_scalar("INSERT INTO im_chats (kind, direct_key, created_by) VALUES ('direct', $1, $2) ON CONFLICT (direct_key) DO NOTHING RETURNING id")
                .bind(&key)
                .bind(auth.id)
                .fetch_optional(&mut *tx)
                .await?;
            let id = match id {
                Some(id) => {
                    for u in pair {
                        sqlx::query("INSERT INTO im_members (chat_id, user_id) VALUES ($1, $2)").bind(id).bind(u).execute(&mut *tx).await?;
                    }
                    id
                }
                // Made by the other person at the same moment.
                None => sqlx::query_scalar("SELECT id FROM im_chats WHERE direct_key = $1").bind(&key).fetch_one(&mut *tx).await?,
            };
            tx.commit().await?;
            id
        }
    };
    get_chat(State(state), auth, Path(id)).await
}

/// A note from NERP in the chat ("Ann added Bob"), shown at once to its members.
pub(crate) async fn system_message(state: &AppState, chat: Uuid, text: String) -> AppResult<()> {
    let id: Uuid = sqlx::query_scalar("INSERT INTO im_messages (chat_id, body) VALUES ($1, $2) RETURNING id").bind(chat).bind(text).fetch_one(&state.db).await?;
    sqlx::query("UPDATE im_chats SET last_message_at = now() WHERE id = $1").bind(chat).execute(&state.db).await?;
    broadcast_message(state, id).await
}

pub(crate) async fn names(state: &AppState, ids: &[Uuid]) -> AppResult<String> {
    let n: Vec<String> = sqlx::query_scalar("SELECT first_name FROM users WHERE id = ANY($1) ORDER BY first_name").bind(ids).fetch_all(&state.db).await?;
    Ok(n.join(", "))
}

#[derive(Deserialize)]
struct GroupInput {
    title: Option<String>,
    description: Option<String>,
    #[serde(default)]
    members: Vec<Uuid>,
    /// group (default) | channel
    kind: Option<String>,
    /// private (default) | open
    visibility: Option<String>,
    /// manual (default) | everyone | department
    audience: Option<String>,
    department_id: Option<Uuid>,
    /// all | managers
    post_policy: Option<String>,
}

fn check_choice(field: &str, v: &str, allowed: &[&str]) -> AppResult<()> {
    if allowed.contains(&v) { Ok(()) } else { Err(AppError::bad(format!("{field} must be one of: {}", allowed.join(", ")))) }
}

async fn create_group(State(state): State<AppState>, auth: AuthUser, Json(b): Json<GroupInput>) -> AppResult<Json<Value>> {
    let title = required("title", b.title.as_deref().unwrap_or(""), 120)?;
    let kind = b.kind.unwrap_or_else(|| "group".into());
    check_choice("kind", &kind, &["group", "channel"])?;
    let visibility = b.visibility.unwrap_or_else(|| "private".into());
    check_choice("visibility", &visibility, &["private", "open"])?;
    let audience = b.audience.unwrap_or_else(|| "manual".into());
    check_choice("audience", &audience, &["manual", "everyone", "department"])?;
    let post_policy = b.post_policy.unwrap_or_else(|| if kind == "channel" { "managers".into() } else { "all".into() });
    check_choice("post_policy", &post_policy, &["all", "managers"])?;
    let department = match audience.as_str() {
        "everyone" => {
            if !auth.can(perm::SETTINGS_MANAGE) {
                return Err(AppError::bad("only administrators make chats for the whole company"));
            }
            None
        }
        "department" => {
            let d = b.department_id.ok_or_else(|| AppError::bad("choose the department"))?;
            let head: Option<Option<Uuid>> = sqlx::query_scalar("SELECT head_user_id FROM departments WHERE id = $1").bind(d).fetch_optional(&state.db).await?;
            let Some(head) = head else { return Err(AppError::NotFound("department")) };
            if head != Some(auth.id) && !auth.can(perm::DEPARTMENTS_MANAGE) {
                return Err(AppError::bad("only the department's head or whoever manages the structure makes its chats"));
            }
            Some(d)
        }
        _ => None,
    };
    let mut m = b.members;
    m.retain(|u| *u != auth.id);
    m.sort();
    m.dedup();
    if audience == "manual" {
        // A channel or an open chat may start with just its owner; others join.
        if (kind == "group" && visibility == "private" && m.is_empty()) || m.len() > 1000 {
            return Err(AppError::bad("add one to 1000 people"));
        }
        active_users(&state, &m).await?;
    } else {
        m.clear();
    }
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO im_chats (kind, title, description, created_by, visibility, audience, department_id, post_policy)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(&kind)
    .bind(&title)
    .bind(clean(b.description).map(|d| d.chars().take(2000).collect::<String>()))
    .bind(auth.id)
    .bind(&visibility)
    .bind(&audience)
    .bind(department)
    .bind(&post_policy)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO im_members (chat_id, user_id, role) VALUES ($1, $2, 'owner')").bind(id).bind(auth.id).execute(&mut *tx).await?;
    for u in &m {
        sqlx::query("INSERT INTO im_members (chat_id, user_id) VALUES ($1, $2)").bind(id).bind(u).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    if audience != "manual" {
        social::sync_chat_audience(&state, id).await?;
    }
    let me = names(&state, &[auth.id]).await?;
    let what = if kind == "channel" { "channel" } else { "chat" };
    system_message(&state, id, format!("{me} made the {what} “{title}”")).await?;
    let all = members(&state, id).await?;
    tell(&state, all, "im.chat", json!({ "chat_id": id })).await;
    get_chat(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct ChatPatch {
    title: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    visibility: Option<String>,
    post_policy: Option<String>,
    /// Hours; null switches automatic deletion off.
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    auto_delete_hours: Option<Option<i32>>,
    add: Option<Vec<Uuid>>,
    remove: Option<Vec<Uuid>>,
}

async fn update_chat(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ChatPatch>) -> AppResult<Json<Value>> {
    let s = seat(&state, &auth, id).await?;
    if s.role.is_none() && !s.manage {
        return Err(AppError::NotFound("chat"));
    }
    let before = members(&state, id).await?;
    if s.kind == "direct" {
        return Err(AppError::bad("a one-to-one chat can't be changed; start a group instead"));
    }
    let me = names(&state, &[auth.id]).await?;
    let settings = b.title.is_some() || b.description.is_some() || b.visibility.is_some() || b.post_policy.is_some();
    if settings {
        linked::workgroup_guard(&state, id).await?;
    }
    if (settings || b.auto_delete_hours.is_some()) && !s.manage {
        return Err(AppError::bad("only the chat's owner and admins change its settings"));
    }
    if let Some(t) = &b.title {
        let t = required("title", t, 120)?;
        sqlx::query("UPDATE im_chats SET title = $2 WHERE id = $1").bind(id).bind(&t).execute(&state.db).await?;
        system_message(&state, id, format!("{me} renamed the chat to “{t}”")).await?;
    }
    if let Some(d) = b.description {
        sqlx::query("UPDATE im_chats SET description = $2 WHERE id = $1").bind(id).bind(clean(d)).execute(&state.db).await?;
    }
    if let Some(v) = &b.visibility {
        check_choice("visibility", v, &["private", "open"])?;
        if s.is_general {
            return Err(AppError::bad("the general chat is for everyone already"));
        }
        sqlx::query("UPDATE im_chats SET visibility = $2 WHERE id = $1").bind(id).bind(v).execute(&state.db).await?;
    }
    if let Some(p) = &b.post_policy {
        check_choice("post_policy", p, &["all", "managers"])?;
        if s.kind == "channel" && p == "all" {
            return Err(AppError::bad("in a channel only its managers post; members comment"));
        }
        sqlx::query("UPDATE im_chats SET post_policy = $2 WHERE id = $1").bind(id).bind(p).execute(&state.db).await?;
        system_message(&state, id, if p == "managers" { format!("{me} let only the chat's managers write") } else { format!("{me} let everyone write") }).await?;
    }
    if let Some(h) = b.auto_delete_hours {
        if h.is_some_and(|h| !(1..=24 * 365).contains(&h)) {
            return Err(AppError::bad("messages can be deleted after 1 hour to 1 year"));
        }
        sqlx::query("UPDATE im_chats SET auto_delete_hours = $2 WHERE id = $1").bind(id).bind(h).execute(&state.db).await?;
        let note = match h {
            None => format!("{me} switched off automatic deletion"),
            Some(h) if h % 24 == 0 => format!("{me} set messages to be deleted after {} days", h / 24),
            Some(h) => format!("{me} set messages to be deleted after {h} hours"),
        };
        system_message(&state, id, note).await?;
    }
    if b.add.is_some() || b.remove.is_some() {
        linked::workgroup_guard(&state, id).await?;
        if s.audience != "manual" {
            return Err(AppError::bad("this chat's members follow the company structure"));
        }
    }
    if let Some(add) = b.add {
        if s.role.is_none() || (s.kind == "channel" && !s.manage) {
            return Err(AppError::bad("only the channel's managers add people"));
        }
        let new: Vec<Uuid> = add.into_iter().filter(|u| !before.contains(u)).collect();
        if !new.is_empty() {
            active_users(&state, &new).await?;
            for u in &new {
                // They see the conversation from now on.
                sqlx::query("INSERT INTO im_members (chat_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(id).bind(u).execute(&state.db).await?;
            }
            system_message(&state, id, format!("{me} added {}", names(&state, &new).await?)).await?;
        }
    }
    if let Some(remove) = b.remove {
        if !s.manage {
            return Err(AppError::bad("only the chat's owner and admins remove people; others can leave"));
        }
        let owner: Option<Uuid> = sqlx::query_scalar("SELECT user_id FROM im_members WHERE chat_id = $1 AND role = 'owner'").bind(id).fetch_optional(&state.db).await?;
        let gone: Vec<Uuid> = remove.into_iter().filter(|u| before.contains(u) && *u != auth.id && Some(*u) != owner).collect();
        if !gone.is_empty() {
            sqlx::query("DELETE FROM im_members WHERE chat_id = $1 AND user_id = ANY($2)").bind(id).bind(&gone).execute(&state.db).await?;
            system_message(&state, id, format!("{me} removed {}", names(&state, &gone).await?)).await?;
        }
    }
    let mut all = members(&state, id).await?;
    all.extend(before);
    all.sort();
    all.dedup();
    tell(&state, all, "im.chat", json!({ "chat_id": id })).await;
    get_chat(State(state), auth, Path(id)).await
}

async fn leave(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let s = seat(&state, &auth, id).await?;
    if s.role.is_none() {
        return Err(AppError::NotFound("chat"));
    }
    if s.kind == "direct" {
        return Err(AppError::bad("you can't leave a one-to-one chat"));
    }
    linked::workgroup_guard(&state, id).await?;
    if s.audience != "manual" {
        return Err(AppError::bad("this chat's members follow the company structure; mute it instead"));
    }
    sqlx::query("DELETE FROM im_members WHERE chat_id = $1 AND user_id = $2").bind(id).bind(auth.id).execute(&state.db).await?;
    let left = members(&state, id).await?;
    if left.is_empty() {
        sqlx::query("DELETE FROM im_chats WHERE id = $1").bind(id).execute(&state.db).await?;
    } else {
        // An owner who leaves hands the chat to an admin, or else to the longest-standing member.
        if s.role.as_deref() == Some("owner") {
            sqlx::query(
                "UPDATE im_members SET role = 'owner' WHERE chat_id = $1 AND user_id = (
                   SELECT user_id FROM im_members WHERE chat_id = $1 ORDER BY role = 'admin' DESC, joined_at LIMIT 1)",
            )
            .bind(id)
            .execute(&state.db)
            .await?;
        }
        let me = names(&state, &[auth.id]).await?;
        system_message(&state, id, format!("{me} left")).await?;
        tell(&state, left, "im.chat", json!({ "chat_id": id })).await;
    }
    Ok(Json(json!({ "ok": true })))
}

/// A message NERP posts itself (call notes, recordings): shown at once in open chats.
/// `user`: who it is from (None: a note from NERP).
pub async fn post_message(state: &AppState, chat: Uuid, user: Option<Uuid>, body: &str, file_ids: &[Uuid]) -> AppResult<()> {
    let mid: Uuid = sqlx::query_scalar("INSERT INTO im_messages (chat_id, user_id, body, file_ids) VALUES ($1, $2, $3, $4) RETURNING id")
        .bind(chat)
        .bind(user)
        .bind(body)
        .bind(file_ids)
        .fetch_one(&state.db)
        .await?;
    sqlx::query("UPDATE im_chats SET last_message_at = now() WHERE id = $1").bind(chat).execute(&state.db).await?;
    broadcast_message(state, mid).await
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
pub(crate) struct Message {
    id: Uuid,
    chat_id: Uuid,
    user_id: Option<Uuid>,
    user_name: Option<String>,
    avatar_file_id: Option<Uuid>,
    body: String,
    kind: String,
    parent_id: Option<Uuid>,
    files: Value,
    edited_at: Option<DateTime<Utc>>,
    deleted: bool,
    created_at: DateTime<Utc>,
    /// Comments under a channel post (or replies to a message).
    comment_count: i64,
    last_comment_at: Option<DateTime<Utc>>,
}

const MESSAGE_SELECT: &str = "SELECT m.id, m.chat_id, m.user_id, trim(u.first_name || ' ' || u.last_name) AS user_name, u.avatar_file_id,
    CASE WHEN m.deleted_at IS NULL THEN m.body ELSE '' END AS body, m.kind, m.parent_id,
    CASE WHEN m.deleted_at IS NULL THEN COALESCE((SELECT jsonb_agg(jsonb_build_object('id', f.id, 'name', f.original_name, 'size', f.size_bytes, 'mime', f.mime_type))
        FROM files f WHERE f.id = ANY(m.file_ids)), '[]') ELSE '[]' END AS files,
    m.edited_at, m.deleted_at IS NOT NULL AS deleted, m.created_at,
    (SELECT COUNT(*) FROM im_messages r WHERE r.parent_id = m.id AND r.deleted_at IS NULL) AS comment_count,
    (SELECT MAX(r.created_at) FROM im_messages r WHERE r.parent_id = m.id AND r.deleted_at IS NULL) AS last_comment_at
    FROM im_messages m LEFT JOIN users u ON u.id = m.user_id";

#[derive(Deserialize)]
struct MessagesQuery {
    before: Option<DateTime<Utc>>,
    limit: Option<i64>,
    /// The comments under this message instead of the chat's messages.
    parent_id: Option<Uuid>,
}

/// Newest first, in pages going back; each page in time order.
async fn messages(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<MessagesQuery>) -> AppResult<Json<Value>> {
    seat(&state, &auth, id).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let cleared: Option<DateTime<Utc>> = sqlx::query_scalar("SELECT cleared_at FROM im_members WHERE chat_id = $1 AND user_id = $2")
        .bind(id)
        .bind(auth.id)
        .fetch_optional(&state.db)
        .await?
        .flatten();
    let items: Vec<Message> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{MESSAGE_SELECT} WHERE m.chat_id = $1 AND ($2::timestamptz IS NULL OR m.created_at < $2)
           AND ($4::timestamptz IS NULL OR m.created_at > $4)
           AND (($5::uuid IS NULL AND m.parent_id IS NULL) OR m.parent_id = $5)
         ORDER BY m.created_at DESC LIMIT $3"
    )))
    .bind(id)
    .bind(q.before)
    .bind(limit + 1)
    .bind(cleared)
    .bind(q.parent_id)
    .fetch_all(&state.db)
    .await?;
    let more = items.len() as i64 > limit;
    let mut items: Vec<Value> = items.into_iter().take(limit as usize).map(|m| serde_json::to_value(m).unwrap()).collect();
    items.reverse();
    extras::enrich(&state, auth.id, &mut items).await?;
    Ok(Json(json!({ "items": items, "more": more })))
}

pub(crate) async fn message(state: &AppState, id: Uuid) -> AppResult<Message> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{MESSAGE_SELECT} WHERE m.id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("message"))
}

/// One message as the user sees it (with their votes and reactions).
pub(crate) async fn message_view(state: &AppState, user: Uuid, id: Uuid) -> AppResult<Value> {
    let mut v = vec![serde_json::to_value(message(state, id).await?).unwrap()];
    extras::enrich(state, user, &mut v).await?;
    Ok(v.remove(0))
}

async fn get_message(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let chat: Uuid = sqlx::query_scalar("SELECT chat_id FROM im_messages WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("message"))?;
    seat(&state, &auth, chat).await?;
    Ok(Json(message_view(&state, auth.id, id).await?))
}

/// Tells the chat's members (and readers of an open chat, through the chat's channel of
/// events) about a new or changed message. A comment also changes its post's count.
pub(crate) async fn broadcast_message(state: &AppState, id: Uuid) -> AppResult<()> {
    let m = message(state, id).await?;
    let all = members(state, m.chat_id).await?;
    let (chat, parent) = (m.chat_id, m.parent_id);
    let mut v = vec![serde_json::to_value(m).unwrap()];
    // Votes and reactions per viewer are fetched by each client (im.message.changed).
    extras::enrich(state, Uuid::nil(), &mut v).await?;
    tell(state, all.clone(), "im.message", json!({ "chat_id": chat, "message": v.remove(0) })).await;
    if let Some(p) = parent {
        tell(state, all, "im.message.changed", json!({ "chat_id": chat, "message_id": p })).await;
    }
    Ok(())
}

#[derive(Deserialize)]
struct SendBody {
    #[serde(default)]
    body: String,
    #[serde(default)]
    file_ids: Vec<Uuid>,
    /// text (default) | voice | video | sticker
    kind: Option<String>,
    /// A comment under this message (channel posts) or a reply in its thread.
    parent_id: Option<Uuid>,
}

async fn send(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<SendBody>) -> AppResult<Json<Value>> {
    let s = seat(&state, &auth, id).await?;
    if s.role.is_none() {
        return Err(AppError::bad("join the chat to write in it"));
    }
    if b.parent_id.is_none() && !s.post {
        return Err(AppError::bad(if s.kind == "channel" { "only the channel's managers post; you can comment" } else { "only the chat's managers write here" }));
    }
    if let Some(p) = b.parent_id {
        let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM im_messages WHERE id = $1 AND chat_id = $2 AND parent_id IS NULL AND deleted_at IS NULL)")
            .bind(p)
            .bind(id)
            .fetch_one(&state.db)
            .await?;
        if !ok {
            return Err(AppError::bad("comment on a message of this chat"));
        }
    }
    let kind = b.kind.unwrap_or_else(|| "text".into());
    check_choice("kind", &kind, &["text", "voice", "video", "sticker"])?;
    let body = b.body.trim().to_string();
    if body.chars().count() > 10_000 {
        return Err(AppError::bad("messages can be at most 10,000 characters"));
    }
    if b.file_ids.len() > 10 {
        return Err(AppError::bad("at most 10 files per message"));
    }
    // Only files the sender uploaded can be shared this way.
    let own: Vec<String> = sqlx::query_scalar("SELECT mime_type FROM files WHERE id = ANY($1) AND owner_id = $2").bind(&b.file_ids).bind(auth.id).fetch_all(&state.db).await?;
    if own.len() != b.file_ids.len() {
        return Err(AppError::bad("attach files you uploaded"));
    }
    match kind.as_str() {
        "text" if body.is_empty() && b.file_ids.is_empty() => return Err(AppError::bad("the message is empty")),
        "voice" | "video" => {
            let want = if kind == "voice" { "audio/" } else { "video/" };
            // Browsers record voice as audio/webm or audio/ogg, video notes as video/webm or video/mp4.
            if own.len() != 1 || !(own[0].starts_with(want) || (kind == "voice" && own[0] == "video/webm")) {
                return Err(AppError::bad(format!("a {kind} message is one recording")));
            }
        }
        "sticker" if !extras::STICKERS.contains(&body.as_str()) || !b.file_ids.is_empty() => return Err(AppError::bad("unknown sticker")),
        _ => {}
    }
    let mid: Uuid = sqlx::query_scalar("INSERT INTO im_messages (chat_id, user_id, body, file_ids, kind, parent_id) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id")
        .bind(id)
        .bind(auth.id)
        .bind(&body)
        .bind(&b.file_ids)
        .bind(&kind)
        .bind(b.parent_id)
        .fetch_one(&state.db)
        .await?;
    if b.parent_id.is_none() {
        // Writing means having read everything before.
        sqlx::query("UPDATE im_chats SET last_message_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
        sqlx::query("UPDATE im_members SET last_read_at = now() WHERE chat_id = $1 AND user_id = $2").bind(id).bind(auth.id).execute(&state.db).await?;
    }
    broadcast_message(&state, mid).await?;
    Ok(Json(message_view(&state, auth.id, mid).await?))
}

/// The message, its chat and the chat's members, when the user may change it: its author, or
/// (deleting only) a manager of the chat.
async fn changeable(state: &AppState, auth: &AuthUser, id: Uuid, deleting: bool) -> AppResult<(Uuid, Vec<Uuid>)> {
    let (chat, author, deleted): (Uuid, Option<Uuid>, bool) = sqlx::query_as("SELECT chat_id, user_id, deleted_at IS NOT NULL FROM im_messages WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("message"))?;
    let all = member(state, auth, chat).await?;
    let manager = deleting && author.is_some() && seat(state, auth, chat).await?.manage;
    if (author != Some(auth.id) && !manager) || deleted {
        return Err(AppError::bad("you can only change your own messages"));
    }
    Ok((chat, all))
}

#[derive(Deserialize)]
struct EditBody {
    body: String,
}

async fn edit(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<EditBody>) -> AppResult<Json<Value>> {
    changeable(&state, &auth, id, false).await?;
    let body = b.body.trim();
    if body.chars().count() > 10_000 {
        return Err(AppError::bad("messages can be at most 10,000 characters"));
    }
    let (files, kind): (i32, String) = sqlx::query_as("SELECT cardinality(file_ids), kind FROM im_messages WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    if kind != "text" {
        return Err(AppError::bad("only text messages can be edited"));
    }
    if body.is_empty() && files == 0 {
        return Err(AppError::bad("the message is empty; delete it instead"));
    }
    sqlx::query("UPDATE im_messages SET body = $2, edited_at = now() WHERE id = $1").bind(id).bind(body).execute(&state.db).await?;
    broadcast_message(&state, id).await?;
    Ok(Json(message_view(&state, auth.id, id).await?))
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    changeable(&state, &auth, id, true).await?;
    sqlx::query("UPDATE im_messages SET deleted_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
    broadcast_message(&state, id).await?;
    Ok(Json(message_view(&state, auth.id, id).await?))
}

async fn read(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let all = member(&state, &auth, id).await?;
    sqlx::query("UPDATE im_members SET last_read_at = now() WHERE chat_id = $1 AND user_id = $2").bind(id).bind(auth.id).execute(&state.db).await?;
    // Others see "read"; my own sidebar count goes down.
    tell(&state, all, "im.read", json!({ "chat_id": id, "user_id": auth.id, "at": Utc::now() })).await;
    Ok(Json(json!({ "ok": true })))
}

/// Unread messages over all chats not muted (for the sidebar).
async fn unread(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let (n, chats): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(DISTINCT m.chat_id) FROM im_members me JOIN im_messages m ON m.chat_id = me.chat_id
         WHERE me.user_id = $1 AND NOT me.muted AND m.parent_id IS NULL AND m.created_at > me.last_read_at AND m.user_id IS DISTINCT FROM $1 AND m.deleted_at IS NULL",
    )
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(json!({ "messages": n, "chats": chats })))
}
