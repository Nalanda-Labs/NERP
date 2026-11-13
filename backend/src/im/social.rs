//! Finding and joining open chats, the general chat, chats whose members follow the company
//! (everyone, or a department with its sub-departments), roles, muting, clearing history,
//! automatic deletion of old messages, and each person's chat folders.

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{chat_view, members, names, seat, system_message, tell};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub const MAINTENANCE_JOB: &str = "im.maintenance";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/im/open", get(discover))
        .route("/im/general", get(general))
        .route("/im/chats/{id}/join", post(join))
        .route("/im/chats/{id}/members/{user}", put(set_role))
        .route("/im/chats/{id}/clear", post(clear))
        .route("/im/chats/{id}/me", patch(my_settings))
        .route("/im/folders", get(folders).post(new_folder))
        .route("/im/folders/{id}", patch(update_folder).delete(delete_folder))
}

// ---------------------------------------------------------------------------
// Members that follow the company
// ---------------------------------------------------------------------------

/// Who belongs in each chat with an audience: (chat, user). `$1`: one user, or NULL for all.
/// `$2`: one chat, or NULL for all.
const AUDIENCE_SQL: &str = "WITH RECURSIVE tree (root, id) AS (
        SELECT id, id FROM departments
        UNION ALL SELECT t.root, d.id FROM departments d JOIN tree t ON d.parent_id = t.id
    ), wanted AS (
        SELECT c.id AS chat_id, u.id AS user_id FROM im_chats c CROSS JOIN users u
        WHERE c.audience = 'everyone' AND u.is_active AND NOT u.is_guest AND ($1::uuid IS NULL OR u.id = $1) AND ($2::uuid IS NULL OR c.id = $2)
        UNION
        SELECT c.id, ud.user_id FROM im_chats c JOIN tree t ON t.root = c.department_id JOIN user_departments ud ON ud.department_id = t.id
        JOIN users u ON u.id = ud.user_id
        WHERE c.audience = 'department' AND u.is_active AND NOT u.is_guest AND ($1::uuid IS NULL OR u.id = $1) AND ($2::uuid IS NULL OR c.id = $2)
    )";

/// Adds and removes members of the general chat and of department chats (for one user, or
/// everyone). The chat's owner and admins stay whatever their department.
pub async fn sync_audiences(state: &AppState, user: Option<Uuid>) -> AppResult<()> {
    sync(state, user, None).await
}

pub(super) async fn sync_chat_audience(state: &AppState, chat: Uuid) -> AppResult<()> {
    sync(state, None, Some(chat)).await
}

async fn sync(state: &AppState, user: Option<Uuid>, chat: Option<Uuid>) -> AppResult<()> {
    let added: Vec<(Uuid, Uuid)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{AUDIENCE_SQL} INSERT INTO im_members (chat_id, user_id) SELECT chat_id, user_id FROM wanted ON CONFLICT DO NOTHING RETURNING chat_id, user_id"
    )))
    .bind(user)
    .bind(chat)
    .fetch_all(&state.db)
    .await?;
    let removed: Vec<(Uuid, Uuid)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{AUDIENCE_SQL} DELETE FROM im_members m USING im_chats c
         WHERE c.id = m.chat_id AND c.audience <> 'manual' AND m.role = 'member'
           AND ($1::uuid IS NULL OR m.user_id = $1) AND ($2::uuid IS NULL OR m.chat_id = $2)
           AND NOT EXISTS (SELECT 1 FROM wanted w WHERE w.chat_id = m.chat_id AND w.user_id = m.user_id)
         RETURNING m.chat_id, m.user_id"
    )))
    .bind(user)
    .bind(chat)
    .fetch_all(&state.db)
    .await?;
    let mut chats: Vec<Uuid> = added.iter().chain(removed.iter()).map(|(c, _)| *c).collect();
    chats.sort();
    chats.dedup();
    for c in chats {
        let mut who = members(state, c).await?;
        who.extend(removed.iter().filter(|(x, _)| *x == c).map(|(_, u)| *u));
        tell(state, who, "im.chat", json!({ "chat_id": c })).await;
    }
    Ok(())
}

/// The company's general chat, made the first time anyone asks for it.
async fn general(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    if auth.is_guest() {
        return Err(AppError::NotFound("chat"));
    }
    let id: Option<Uuid> = sqlx::query_scalar!("SELECT id FROM im_chats WHERE is_general").fetch_optional(&state.db).await?;
    let id = match id {
        Some(id) => id,
        None => {
            let made: Option<Uuid> = sqlx::query_scalar!("INSERT INTO im_chats (kind, title, description, audience, is_general)
                 VALUES ('group', 'General', 'Everyone in the company', 'everyone', TRUE)
                 ON CONFLICT (is_general) WHERE is_general DO NOTHING RETURNING id")
            .fetch_optional(&state.db)
            .await?;
            match made {
                Some(id) => {
                    system_message(&state, id, "The company's general chat: everyone is in it".into()).await?;
                    id
                }
                None => sqlx::query_scalar!("SELECT id FROM im_chats WHERE is_general").fetch_one(&state.db).await?,
            }
        }
    };
    sync_chat_audience(&state, id).await?;
    Ok(Json(chat_view(&state, &auth, id).await?))
}

// ---------------------------------------------------------------------------
// Open chats
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DiscoverQuery {
    q: Option<String>,
}

/// Open chats and channels I'm not in.
async fn discover(State(state): State<AppState>, auth: AuthUser, Query(q): Query<DiscoverQuery>) -> AppResult<Json<Value>> {
    // Guests see only the chats they were invited to.
    if auth.is_guest() {
        return Ok(Json(json!({ "items": [] })));
    }
    let term = q.q.map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).map(|t| format!("%{}%", t.replace('%', "\\%").replace('_', "\\_")));
    let rows: Vec<(Uuid, String, String, Option<String>, i64)> = sqlx::query!(r#"SELECT c.id AS "id!", c.kind AS "kind!", c.title AS "title!", c.description AS "description?", (SELECT COUNT(*) FROM im_members x WHERE x.chat_id = c.id) AS "c4!"
         FROM im_chats c WHERE c.visibility = 'open'
           AND NOT EXISTS (SELECT 1 FROM im_members m WHERE m.chat_id = c.id AND m.user_id = $1)
           AND ($2::text IS NULL OR lower(c.title) LIKE $2 OR lower(COALESCE(c.description, '')) LIKE $2)
         ORDER BY c.last_message_at DESC LIMIT 200"#, auth.id, term).map(|r| (r.id, r.kind, r.title, r.description, r.c4))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, kind, title, d, n)| json!({ "id": id, "kind": kind, "title": title, "description": d, "members": n })).collect::<Vec<_>>() })))
}

async fn join(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let s = seat(&state, &auth, id).await?;
    if s.role.is_none() {
        if s.visibility != "open" || s.audience != "manual" || s.linked {
            return Err(AppError::NotFound("chat"));
        }
        sqlx::query!("INSERT INTO im_members (chat_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING", id, auth.id).execute(&state.db).await?;
        if s.kind != "channel" {
            system_message(&state, id, format!("{} joined", names(&state, &[auth.id]).await?)).await?;
        }
        let all = members(&state, id).await?;
        tell(&state, all, "im.chat", json!({ "chat_id": id })).await;
    }
    Ok(Json(chat_view(&state, &auth, id).await?))
}

// ---------------------------------------------------------------------------
// Roles, muting, clearing
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RoleBody {
    /// owner (hands the chat over) | admin | member
    role: String,
}

async fn set_role(State(state): State<AppState>, auth: AuthUser, Path((id, user)): Path<(Uuid, Uuid)>, Json(b): Json<RoleBody>) -> AppResult<Json<Value>> {
    let s = seat(&state, &auth, id).await?;
    if s.kind == "direct" || s.linked {
        return Err(AppError::bad("this chat has no roles"));
    }
    let is_owner = s.role.as_deref() == Some("owner");
    if !(is_owner || (s.manage && s.audience != "manual")) {
        return Err(AppError::bad("only the chat's owner gives roles"));
    }
    if !["owner", "admin", "member"].contains(&b.role.as_str()) {
        return Err(AppError::bad("role must be owner, admin or member"));
    }
    let current: Option<String> = sqlx::query_scalar!("SELECT role FROM im_members WHERE chat_id = $1 AND user_id = $2", id, user).fetch_optional(&state.db).await?;
    let Some(current) = current else { return Err(AppError::bad("they aren't in the chat")) };
    if current == "owner" && b.role != "owner" {
        return Err(AppError::bad("hand the chat over to someone else first"));
    }
    let mut tx = state.db.begin().await?;
    if b.role == "owner" {
        sqlx::query!("UPDATE im_members SET role = 'admin' WHERE chat_id = $1 AND role = 'owner'", id).execute(&mut *tx).await?;
    }
    sqlx::query!("UPDATE im_members SET role = $3 WHERE chat_id = $1 AND user_id = $2", id, user, &b.role).execute(&mut *tx).await?;
    tx.commit().await?;
    let (me, them) = (names(&state, &[auth.id]).await?, names(&state, &[user]).await?);
    let note = match b.role.as_str() {
        "owner" => format!("{me} handed the chat over to {them}"),
        "admin" => format!("{me} made {them} an admin"),
        _ => format!("{them} is no longer an admin"),
    };
    system_message(&state, id, note).await?;
    let all = members(&state, id).await?;
    tell(&state, all, "im.chat", json!({ "chat_id": id })).await;
    Ok(Json(chat_view(&state, &auth, id).await?))
}

#[derive(Deserialize)]
struct MySettings {
    muted: Option<bool>,
}

async fn my_settings(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<MySettings>) -> AppResult<Json<Value>> {
    if seat(&state, &auth, id).await?.role.is_none() {
        return Err(AppError::NotFound("chat"));
    }
    if let Some(m) = b.muted {
        sqlx::query!("UPDATE im_members SET muted = $3 WHERE chat_id = $1 AND user_id = $2", id, auth.id, m).execute(&state.db).await?;
    }
    tell(&state, vec![auth.id], "im.chat", json!({ "chat_id": id })).await;
    Ok(Json(chat_view(&state, &auth, id).await?))
}

#[derive(Deserialize, Default)]
struct ClearBody {
    /// Delete the history for everyone (the chat's managers); otherwise hide it for me.
    #[serde(default)]
    everyone: bool,
}

async fn clear(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, body: Option<Json<ClearBody>>) -> AppResult<Json<Value>> {
    let b = body.map(|b| b.0).unwrap_or_default();
    let s = seat(&state, &auth, id).await?;
    if s.role.is_none() {
        return Err(AppError::NotFound("chat"));
    }
    if !b.everyone {
        sqlx::query!("UPDATE im_members SET cleared_at = now(), last_read_at = now() WHERE chat_id = $1 AND user_id = $2", id, auth.id).execute(&state.db).await?;
        tell(&state, vec![auth.id], "im.chat", json!({ "chat_id": id, "cleared": true })).await;
        return Ok(Json(json!({ "ok": true })));
    }
    if s.kind == "direct" || !s.manage {
        return Err(AppError::bad("only the chat's owner and admins delete its history for everyone"));
    }
    sqlx::query!("DELETE FROM im_messages WHERE chat_id = $1", id).execute(&state.db).await?;
    system_message(&state, id, format!("{} deleted the chat's history", names(&state, &[auth.id]).await?)).await?;
    let all = members(&state, id).await?;
    tell(&state, all, "im.chat", json!({ "chat_id": id, "cleared": true })).await;
    Ok(Json(json!({ "ok": true })))
}

/// Deletes messages past their chat's time limit (or the company's), and keeps company-wide and department
/// chats' members in step with the company.
pub async fn run_maintenance(state: &AppState) -> Result<()> {
    let mut gone: Vec<Uuid> = sqlx::query_scalar!("DELETE FROM im_messages m USING im_chats c
         WHERE c.id = m.chat_id AND c.auto_delete_hours IS NOT NULL AND m.created_at < now() - make_interval(hours => c.auto_delete_hours)
           AND m.parent_id IS NULL
         RETURNING m.chat_id")
    .fetch_all(&state.db)
    .await?;
    // The company's limit, for every chat.
    let days: Option<serde_json::Value> = sqlx::query_scalar!("SELECT value FROM settings WHERE key = 'chat_retention_days'").fetch_optional(&state.db).await?;
    if let Some(d) = days.and_then(|v| v.as_i64()).filter(|d| *d > 0) {
        gone.extend(
            sqlx::query_scalar!("DELETE FROM im_messages WHERE created_at < now() - make_interval(days => $1) RETURNING chat_id", d as i32)
                .fetch_all(&state.db)
                .await?,
        );
    }
    gone.sort();
    gone.dedup();
    for c in gone {
        let all = members(state, c).await?;
        tell(state, all, "im.chat", json!({ "chat_id": c, "cleared": true })).await;
    }
    sync_audiences(state, None).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Folders
// ---------------------------------------------------------------------------

async fn folder_list(state: &AppState, user: Uuid) -> AppResult<Value> {
    let rows: Vec<(Uuid, String, Vec<Uuid>, i64)> = sqlx::query!(r#"SELECT f.id AS "id!", f.name AS "name!", ARRAY(SELECT fc.chat_id FROM im_folder_chats fc WHERE fc.folder_id = f.id) AS "c2!",
            (SELECT COUNT(*) FROM im_folder_chats fc JOIN im_members me ON me.chat_id = fc.chat_id AND me.user_id = $1
             JOIN im_messages m ON m.chat_id = fc.chat_id AND m.parent_id IS NULL AND m.created_at > me.last_read_at AND m.user_id IS DISTINCT FROM $1 AND m.deleted_at IS NULL
             WHERE fc.folder_id = f.id) AS "c3!"
         FROM im_folders f WHERE f.user_id = $1 ORDER BY f.sort, f.created_at"#, user).map(|r| (r.id, r.name, r.c2, r.c3))
    .fetch_all(&state.db)
    .await?;
    Ok(json!({ "items": rows.into_iter().map(|(id, name, chats, unread)| json!({ "id": id, "name": name, "chat_ids": chats, "unread": unread })).collect::<Vec<_>>() }))
}

async fn folders(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    Ok(Json(folder_list(&state, auth.id).await?))
}

#[derive(Deserialize)]
struct FolderInput {
    name: Option<String>,
    /// Replaces the folder's chats (only chats I'm in count).
    chat_ids: Option<Vec<Uuid>>,
    sort: Option<i32>,
}

async fn set_folder_chats(state: &AppState, user: Uuid, folder: Uuid, chats: &[Uuid]) -> AppResult<()> {
    let mut tx = state.db.begin().await?;
    sqlx::query!("DELETE FROM im_folder_chats WHERE folder_id = $1", folder).execute(&mut *tx).await?;
    sqlx::query!("INSERT INTO im_folder_chats (folder_id, chat_id) SELECT $1, chat_id FROM im_members WHERE user_id = $2 AND chat_id = ANY($3)", folder, user, chats)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn new_folder(State(state): State<AppState>, auth: AuthUser, Json(b): Json<FolderInput>) -> AppResult<Json<Value>> {
    let name = required("folder name", b.name.as_deref().unwrap_or(""), 60)?;
    let n: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM im_folders WHERE user_id = $1"#, auth.id).fetch_one(&state.db).await?;
    if n >= 20 {
        return Err(AppError::bad("at most 20 folders"));
    }
    let id: Uuid = sqlx::query_scalar!("INSERT INTO im_folders (user_id, name, sort) VALUES ($1, $2, $3) RETURNING id", auth.id, name, b.sort.unwrap_or(n as i32))
        .fetch_one(&state.db)
        .await?;
    if let Some(c) = b.chat_ids {
        set_folder_chats(&state, auth.id, id, &c).await?;
    }
    Ok(Json(folder_list(&state, auth.id).await?))
}

async fn own_folder(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<()> {
    let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM im_folders WHERE id = $1 AND user_id = $2) AS "v!""#, id, auth.id).fetch_one(&state.db).await?;
    if ok { Ok(()) } else { Err(AppError::NotFound("folder")) }
}

async fn update_folder(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<FolderInput>) -> AppResult<Json<Value>> {
    own_folder(&state, &auth, id).await?;
    if let Some(n) = &b.name {
        sqlx::query!("UPDATE im_folders SET name = $2 WHERE id = $1", id, required("folder name", n, 60)?).execute(&state.db).await?;
    }
    if let Some(s) = b.sort {
        sqlx::query!("UPDATE im_folders SET sort = $2 WHERE id = $1", id, s).execute(&state.db).await?;
    }
    if let Some(c) = b.chat_ids {
        set_folder_chats(&state, auth.id, id, &c).await?;
    }
    Ok(Json(folder_list(&state, auth.id).await?))
}

async fn delete_folder(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    own_folder(&state, &auth, id).await?;
    sqlx::query!("DELETE FROM im_folders WHERE id = $1", id).execute(&state.db).await?;
    Ok(Json(folder_list(&state, auth.id).await?))
}
