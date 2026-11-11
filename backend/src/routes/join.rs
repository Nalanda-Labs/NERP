//! Links to join the company. Whoever may invite makes a link (into a department, only for
//! some email domains, a number of times, until a date, with or without an administrator's
//! approval) and shares it however they like. Someone opening it gives their name and work
//! email; NERP then emails them the usual invitation to set a password — at once, or once
//! their request is approved. People waiting for approval are not employees yet: they are
//! inactive and outside the directory, and a rejected request is removed.

use axum::{
    extract::{Path, State},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{auth::send_invitation, normalize_email, required};
use crate::{
    audit,
    auth::{permissions as perm, tokens::random_token, AuthUser, ClientIp},
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/join-links", get(list).post(create))
        .route("/join-links/{id}", patch(change).delete(remove))
        .route("/join-requests", get(requests))
        .route("/join-requests/{id}/approve", post(approve))
        .route("/join-requests/{id}/reject", post(reject))
        .route("/public/join/{token}", get(info).post(join))
}

fn may_invite(auth: &AuthUser) -> AppResult<()> {
    if auth.can(perm::USERS_INVITE) { Ok(()) } else { auth.require(perm::USERS_MANAGE) }
}

fn hash(t: &str) -> String {
    hex::encode(Sha256::digest(t.as_bytes()))
}

type LinkRow = (Uuid, Option<String>, Option<Uuid>, Option<String>, Vec<String>, bool, Option<i32>, i32, Option<DateTime<Utc>>, bool, Option<String>, DateTime<Utc>, Option<Uuid>);

fn link_json(r: LinkRow) -> Value {
    let (id, note, dep, dep_name, domains, approval, max, uses, expires, disabled, by, at, creator) = r;
    let usable = !disabled && creator.is_some() && expires.is_none_or(|e| e > Utc::now()) && max.is_none_or(|m| uses < m);
    json!({ "id": id, "note": note, "department_id": dep, "department": dep_name, "email_domains": domains, "needs_approval": approval,
        "max_uses": max, "uses": uses, "expires_at": expires, "disabled": disabled, "created_by": by, "created_at": at, "usable": usable })
}

const LINK_SELECT: &str = "SELECT l.id, l.note, l.department_id, d.name, l.email_domains, l.needs_approval, l.max_uses, l.uses, l.expires_at, l.disabled,
    trim(u.first_name || ' ' || u.last_name), l.created_at, l.created_by
    FROM join_links l LEFT JOIN departments d ON d.id = l.department_id LEFT JOIN users u ON u.id = l.created_by";

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    may_invite(&auth)?;
    let rows: Vec<LinkRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{LINK_SELECT} ORDER BY l.created_at DESC LIMIT 200"))).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.into_iter().map(link_json).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct LinkInput {
    note: Option<String>,
    department_id: Option<Uuid>,
    #[serde(default)]
    email_domains: Vec<String>,
    #[serde(default = "yes")]
    needs_approval: bool,
    max_uses: Option<i32>,
    expires_at: Option<DateTime<Utc>>,
}

fn yes() -> bool {
    true
}

fn domains(list: &[String]) -> AppResult<Vec<String>> {
    if list.len() > 20 {
        return Err(AppError::bad("at most 20 email domains"));
    }
    list.iter()
        .map(|d| d.trim().trim_start_matches('@').to_lowercase())
        .filter(|d| !d.is_empty())
        .map(|d| {
            if d.len() <= 253 && d.contains('.') && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
                Ok(d)
            } else {
                Err(AppError::bad(format!("“{d}” is not an email domain (such as example.com)")))
            }
        })
        .collect()
}

/// A new link; its address is shown only now.
async fn create(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Json(b): Json<LinkInput>) -> AppResult<Json<Value>> {
    may_invite(&auth)?;
    if b.max_uses.is_some_and(|m| !(1..=10_000).contains(&m)) {
        return Err(AppError::bad("a link can be used 1 to 10,000 times"));
    }
    if b.expires_at.is_some_and(|e| e <= Utc::now()) {
        return Err(AppError::bad("the expiry date has passed"));
    }
    let token = random_token(24);
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO join_links (token_hash, note, department_id, email_domains, needs_approval, max_uses, expires_at, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(hash(&token))
    .bind(b.note.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(200).collect::<String>()))
    .bind(b.department_id)
    .bind(domains(&b.email_domains)?)
    .bind(b.needs_approval)
    .bind(b.max_uses)
    .bind(b.expires_at)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| if crate::error::is_unique_violation(&e) { e.into() } else { AppError::bad("no such department") })?;
    audit::record(&state.db, Some(auth.id), "user.join_link", "join_link", Some(id.to_string()), json!({ "approval": b.needs_approval }), ip.as_deref()).await;
    let row: LinkRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("{LINK_SELECT} WHERE l.id = $1"))).bind(id).fetch_one(&state.db).await?;
    let mut v = link_json(row);
    v["url"] = json!(format!("{}/join/{token}", state.config.public_url));
    Ok(Json(v))
}

#[derive(Deserialize)]
struct LinkChange {
    disabled: Option<bool>,
    note: Option<String>,
}

async fn change(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<LinkChange>) -> AppResult<Json<Value>> {
    may_invite(&auth)?;
    let n = sqlx::query("UPDATE join_links SET disabled = COALESCE($2, disabled), note = COALESCE($3, note) WHERE id = $1")
        .bind(id)
        .bind(b.disabled)
        .bind(b.note.as_deref().map(str::trim))
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("link"));
    }
    let row: LinkRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("{LINK_SELECT} WHERE l.id = $1"))).bind(id).fetch_one(&state.db).await?;
    Ok(Json(link_json(row)))
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    may_invite(&auth)?;
    sqlx::query("DELETE FROM join_links WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Requests waiting for approval
// ---------------------------------------------------------------------------

async fn requests(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    may_invite(&auth)?;
    let rows: Vec<(Uuid, String, String, DateTime<Utc>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT u.id, trim(u.first_name || ' ' || u.last_name), u.email, u.created_at, l.note,
            (SELECT string_agg(d.name, ', ') FROM user_departments ud JOIN departments d ON d.id = ud.department_id WHERE ud.user_id = u.id)
         FROM users u LEFT JOIN join_links l ON l.id = u.joined_via WHERE u.join_pending ORDER BY u.created_at DESC LIMIT 500",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, n, e, at, note, dep)| json!({ "id": id, "name": n, "email": e, "created_at": at, "link_note": note, "department": dep })).collect::<Vec<_>>() })))
}

async fn approve(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    may_invite(&auth)?;
    let n = sqlx::query("UPDATE users SET join_pending = FALSE, is_active = TRUE, updated_at = now() WHERE id = $1 AND join_pending").bind(id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("request"));
    }
    crate::auth::invalidate_authz(&state, &[id]).await?;
    let _ = crate::im::sync_audiences(&state, Some(id)).await;
    send_invitation(&state, id, auth.id).await?;
    audit::record(&state.db, Some(auth.id), "user.join_approved", "user", Some(id.to_string()), json!({}), ip.as_deref()).await;
    requests(State(state), auth).await
}

async fn reject(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    may_invite(&auth)?;
    let email: Option<String> = sqlx::query_scalar("DELETE FROM users WHERE id = $1 AND join_pending RETURNING email").bind(id).fetch_optional(&state.db).await?;
    let email = email.ok_or(AppError::NotFound("request"))?;
    audit::record(&state.db, Some(auth.id), "user.join_rejected", "user", None, json!({ "email": email }), ip.as_deref()).await;
    requests(State(state), auth).await
}

// ---------------------------------------------------------------------------
// Public
// ---------------------------------------------------------------------------

struct Link {
    id: Uuid,
    department_id: Option<Uuid>,
    email_domains: Vec<String>,
    needs_approval: bool,
    created_by: Uuid,
}

async fn usable(state: &AppState, token: &str) -> AppResult<Link> {
    let row: Option<(Uuid, Option<Uuid>, Vec<String>, bool, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, department_id, email_domains, needs_approval, created_by FROM join_links
         WHERE token_hash = $1 AND NOT disabled AND (expires_at IS NULL OR expires_at > now()) AND (max_uses IS NULL OR uses < max_uses)",
    )
    .bind(hash(token))
    .fetch_optional(&state.db)
    .await?;
    match row {
        Some((id, department_id, email_domains, needs_approval, Some(created_by))) => Ok(Link { id, department_id, email_domains, needs_approval, created_by }),
        _ => Err(AppError::NotFound("link")),
    }
}

async fn info(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let l = usable(&state, &token).await?;
    Ok(Json(json!({ "company_name": super::auth::company_name(&state).await, "needs_approval": l.needs_approval, "email_domains": l.email_domains })))
}

#[derive(Deserialize)]
struct JoinInput {
    email: String,
    first_name: String,
    last_name: String,
}

async fn join(State(state): State<AppState>, ClientIp(ip): ClientIp, Path(token): Path<String>, Json(b): Json<JoinInput>) -> AppResult<Json<Value>> {
    // A few requests per link and address per hour.
    let mut redis = state.redis.clone();
    let key = format!("join:{}:{}", &hash(&token)[..16], ip.as_deref().unwrap_or("unknown"));
    let n: i64 = redis.incr(&key, 1).await?;
    if n == 1 {
        let _: () = redis.expire(&key, 3600).await?;
    }
    if n > 10 {
        return Err(AppError::TooManyRequests("too many requests from your network; try again later".into()));
    }
    let l = usable(&state, &token).await?;
    let email = normalize_email(&b.email)?;
    let first_name = required("first name", &b.first_name, 100)?;
    let last_name = required("last name", &b.last_name, 100)?;
    let domain = email.rsplit('@').next().unwrap_or("");
    if !l.email_domains.is_empty() && !l.email_domains.iter().any(|d| d == domain) {
        return Err(AppError::bad(format!("use your work email (@{})", l.email_domains.join(", @"))));
    }
    let answer = json!({ "ok": true, "needs_approval": l.needs_approval });
    // Someone who already has an account hears the same (nobody learns who works here).
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE email = $1)").bind(&email).fetch_one(&state.db).await?;
    if exists {
        return Ok(Json(answer));
    }
    let mut tx = state.db.begin().await?;
    let used = sqlx::query("UPDATE join_links SET uses = uses + 1 WHERE id = $1 AND (max_uses IS NULL OR uses < max_uses)").bind(l.id).execute(&mut *tx).await?.rows_affected();
    if used == 0 {
        return Err(AppError::NotFound("link"));
    }
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash, first_name, last_name, invite_pending, is_active, join_pending, joined_via)
         VALUES ($1, '!invited', $2, $3, TRUE, $4, $5, $6) RETURNING id",
    )
    .bind(&email)
    .bind(&first_name)
    .bind(&last_name)
    .bind(!l.needs_approval)
    .bind(l.needs_approval)
    .bind(l.id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| if crate::error::is_unique_violation(&e) { AppError::Conflict("already asked".into()) } else { e.into() })?;
    sqlx::query("INSERT INTO user_roles (user_id, role_id) SELECT $1, id FROM roles WHERE code = 'employee'").bind(id).execute(&mut *tx).await?;
    if let Some(d) = l.department_id {
        sqlx::query("INSERT INTO user_departments (user_id, department_id) SELECT $1, id FROM departments WHERE id = $2").bind(id).bind(d).execute(&mut *tx).await?;
    }
    audit::record(&mut *tx, None, "user.join_request", "user", Some(id.to_string()), json!({ "email": email, "link": l.id }), ip.as_deref()).await;
    tx.commit().await?;
    if l.needs_approval {
        notify(
            &state,
            &[l.created_by],
            NewNotification {
                kind: "user.join_request",
                title: format!("{first_name} {last_name} asks to join"),
                body: Some(email.clone()),
                link: Some("/employees/invitations".into()),
                actor: None,
            },
        )
        .await;
    } else {
        let _ = crate::im::sync_audiences(&state, Some(id)).await;
        send_invitation(&state, id, l.created_by).await?;
    }
    Ok(Json(answer))
}
