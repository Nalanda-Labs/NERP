//! Guests: people from outside the company, invited by email into a chat or a workgroup.
//!
//! A guest gets the usual invitation to set a password, has the "guest" role (no
//! permissions), and reaches only a short list of areas (see `auth::GUEST_PATHS`): the chats
//! and workgroups they were invited to, with their messages, calls, tasks, files and posts.
//! "Open" chats and groups, anything shared with everyone, the company feed and the employee
//! directory stay closed to them, and company-wide live events never reach them.
//!
//! Chat owners and admins invite guests into their chats; workgroup owners into their groups
//! (also whoever may invite employees). Inviting someone who already has an account adds
//! them instead.

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    audit,
    auth::{permissions as perm, AuthUser, ClientIp},
    error::{AppError, AppResult},
    routes::{normalize_email, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/im/chats/{id}/guests", post(invite_to_chat))
        .route("/projects/{id}/guests", post(invite_to_project))
        .route("/guests", get(list))
}

/// Whether two people share a chat or a workgroup (guests write privately only to them).
pub async fn shares_with(state: &AppState, a: Uuid, b: Uuid) -> AppResult<bool> {
    Ok(sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM im_members x JOIN im_members y ON y.chat_id = x.chat_id WHERE x.user_id = $1 AND y.user_id = $2)
             OR EXISTS (SELECT 1 FROM project_members x JOIN project_members y ON y.project_id = x.project_id WHERE x.user_id = $1 AND y.user_id = $2) AS "v!""#, a, b)
    .fetch_one(&state.db)
    .await?)
}

#[derive(Deserialize)]
struct Invite {
    email: String,
    first_name: Option<String>,
    last_name: Option<String>,
}

/// The person with this email: an existing account, or a new guest (invited by email).
async fn guest_for(state: &AppState, auth: &AuthUser, b: &Invite, ip: Option<&str>) -> AppResult<(Uuid, bool)> {
    let email = normalize_email(&b.email)?;
    let found: Option<(Uuid, bool)> = sqlx::query!(r#"SELECT id, is_active FROM users WHERE email = $1"#, &email).map(|r| (r.id, r.is_active)).fetch_optional(&state.db).await?;
    if let Some((id, active)) = found {
        if !active {
            return Err(AppError::bad("that account is deactivated"));
        }
        return Ok((id, false));
    }
    let first = required("first name", b.first_name.as_deref().unwrap_or(""), 100)?;
    let last = required("last name", b.last_name.as_deref().unwrap_or(""), 100)?;
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar!("INSERT INTO users (email, password_hash, first_name, last_name, invite_pending, is_guest) VALUES ($1, '!invited', $2, $3, TRUE, TRUE) RETURNING id", &email, &first, &last)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!("INSERT INTO user_roles (user_id, role_id) SELECT $1, id FROM roles WHERE code = 'guest'", id).execute(&mut *tx).await?;
    audit::record(&mut *tx, Some(auth.id), "user.guest_invite", "user", Some(id.to_string()), json!({ "email": email }), ip).await;
    tx.commit().await?;
    Ok((id, true))
}

async fn invite_to_chat(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(chat): Path<Uuid>, Json(b): Json<Invite>) -> AppResult<Json<Value>> {
    let row: Option<(Option<String>, String, bool)> = sqlx::query!(r#"SELECT (SELECT role FROM im_members WHERE chat_id = c.id AND user_id = $2) AS "c0?", c.kind AS "kind!",
                (c.audience <> 'manual' OR c.is_general OR c.project_id IS NOT NULL OR c.task_id IS NOT NULL OR c.event_id IS NOT NULL OR c.board_id IS NOT NULL OR c.crm_item_id IS NOT NULL) AS "c2!"
         FROM im_chats c WHERE c.id = $1"#, chat, auth.id).map(|r| (r.c0, r.kind, r.c2))
    .fetch_optional(&state.db)
    .await?;
    let (role, kind, managed) = row.ok_or(AppError::NotFound("chat"))?;
    if !matches!(role.as_deref(), Some("owner" | "admin")) && !auth.can(perm::USERS_INVITE) {
        return Err(AppError::Forbidden);
    }
    if role.is_none() {
        return Err(AppError::NotFound("chat"));
    }
    if kind == "direct" || managed {
        return Err(AppError::bad("guests join group chats you manage yourself (not direct, company, department or linked chats)"));
    }
    let (user, new) = guest_for(&state, &auth, &b, ip.as_deref()).await?;
    sqlx::query!("INSERT INTO im_members (chat_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING", chat, user).execute(&state.db).await?;
    if new {
        crate::routes::auth::send_invitation(&state, user, auth.id).await?;
    }
    let name: String = sqlx::query_scalar!(r#"SELECT trim(first_name || ' ' || last_name) AS "v!" FROM users WHERE id = $1"#, user).fetch_one(&state.db).await?;
    crate::im::system_message(&state, chat, format!("{name} was invited as a guest")).await?;
    let all = crate::im::members(&state, chat).await?;
    crate::im::tell(&state, all, "im.chat", json!({ "chat_id": chat })).await;
    Ok(Json(json!({ "user_id": user, "invited": new })))
}

async fn invite_to_project(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(project): Path<Uuid>, Json(b): Json<Invite>) -> AppResult<Json<Value>> {
    let access = crate::tasks::access::project_access(&state, &auth, project).await?;
    if !access.manage && !auth.can(perm::USERS_INVITE) {
        return Err(AppError::Forbidden);
    }
    let (user, new) = guest_for(&state, &auth, &b, ip.as_deref()).await?;
    sqlx::query!("INSERT INTO project_members (project_id, user_id, role) VALUES ($1, $2, 'member') ON CONFLICT DO NOTHING", project, user).execute(&state.db).await?;
    crate::im::sync_project_chat(&state, project).await?;
    if new {
        crate::routes::auth::send_invitation(&state, user, auth.id).await?;
    }
    Ok(Json(json!({ "user_id": user, "invited": new })))
}

/// The guests, for whoever may invite people (with where they are).
async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    if !auth.can(perm::USERS_INVITE) {
        auth.require(perm::USERS_MANAGE)?;
    }
    let rows: Vec<(Uuid, String, String, bool, bool, Option<String>, Option<String>)> = sqlx::query!(r#"SELECT u.id AS "id!", trim(u.first_name || ' ' || u.last_name) AS "c1!", u.email AS "email!", u.is_active AS "is_active!", u.invite_pending AS "invite_pending!",
            (SELECT string_agg(COALESCE(c.title, 'Chat'), ', ') FROM im_members m JOIN im_chats c ON c.id = m.chat_id WHERE m.user_id = u.id AND c.kind <> 'direct') AS "c5?",
            (SELECT string_agg(p.name, ', ') FROM project_members pm JOIN projects p ON p.id = pm.project_id WHERE pm.user_id = u.id) AS "c6?"
         FROM users u WHERE u.is_guest ORDER BY u.created_at DESC LIMIT 1000"#).map(|r| (r.id, r.c1, r.email, r.is_active, r.invite_pending, r.c5, r.c6))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, n, e, a, p, c, g)| json!({ "id": id, "name": n, "email": e, "active": a, "invite_pending": p, "chats": c, "groups": g })).collect::<Vec<_>>() })))
}
