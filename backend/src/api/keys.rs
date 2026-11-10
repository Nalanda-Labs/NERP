//! Personal API keys. A key acts as the employee who made it, with their access rights, but
//! only within its scopes:
//! * `crm` / `crm:read` — /api/crm/…
//! * `tasks` / `tasks:read` — /api/tasks…, /api/projects…, /api/task-templates…
//! * `users:read` — /api/users…, /api/departments… (reading only)
//! * `files` — /api/files…
//!
//! `…:read` scopes allow GET only. Everything else (sign-in, settings, roles, keys
//! themselves) is out of reach of keys. Keys are rate-limited and stop working when revoked,
//! when they expire, or when their owner is deactivated or loses the "Use the API" permission.

use axum::{
    extract::{Path, Query, State},
    http::{request::Parts, Method},
    routing::{delete, get},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, tokens, AuthUser},
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub const PREFIX: &str = "nerp_";
pub const SCOPES: &[(&str, &str)] = &[
    ("crm", "CRM: read and change records, activities, products and documents"),
    ("crm:read", "CRM: read only"),
    ("tasks", "Tasks and projects: read and change"),
    ("tasks:read", "Tasks and projects: read only"),
    ("users:read", "Employees and company structure: read only"),
    ("files", "Files: upload and download"),
];
/// Requests per key per minute.
const RATE_PER_MINUTE: i64 = 600;

pub fn router() -> Router<AppState> {
    Router::new().route("/api-keys", get(list).post(create)).route("/api-keys/{id}", delete(revoke))
}

/// The scope a request needs, or None when keys may not call it at all.
pub fn needed_scope(method: &Method, path: &str) -> Option<String> {
    let path = path.strip_prefix("/api").unwrap_or(path);
    let first = path.trim_start_matches('/').split('/').next().unwrap_or("");
    let area = match first {
        "crm" => "crm",
        "tasks" | "projects" | "task-templates" => "tasks",
        "users" | "departments" => "users",
        "files" => "files",
        _ => return None,
    };
    let read = matches!(*method, Method::GET | Method::HEAD);
    if area == "users" && !read {
        return None;
    }
    Some(if read { format!("{area}:read") } else { area.to_string() })
}

/// Whether a key's scopes cover what a request needs (a full scope covers its `:read`).
pub fn allows(scopes: &[String], needed: &str) -> bool {
    let full = needed.trim_end_matches(":read");
    scopes.iter().any(|s| s == needed || s == full)
}

#[derive(Serialize, Deserialize, FromRow, Clone)]
struct KeyEntry {
    id: Uuid,
    user_id: Uuid,
    scopes: Vec<String>,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

fn cache_key(hash: &str) -> String {
    format!("api:key:{hash}")
}

/// Authenticates a request carrying an API key (called by the `AuthUser` extractor).
pub async fn authenticate(state: &AppState, parts: &Parts, token: &str) -> AppResult<AuthUser> {
    let hash = tokens::hash_token(token);
    let mut redis = state.redis.clone();
    let cached: Option<String> = redis.get(cache_key(&hash)).await?;
    let entry: KeyEntry = match cached.and_then(|c| serde_json::from_str(&c).ok()) {
        Some(e) => e,
        None => {
            let e: KeyEntry = sqlx::query_as("SELECT id, user_id, scopes, expires_at, revoked_at FROM api_keys WHERE secret_hash = $1")
                .bind(&hash)
                .fetch_optional(&state.db)
                .await?
                .ok_or(AppError::Unauthorized)?;
            let _: () = redis.set_ex(cache_key(&hash), serde_json::to_string(&e).unwrap(), 60).await?;
            e
        }
    };
    if entry.revoked_at.is_some() || entry.expires_at.is_some_and(|t| t < Utc::now()) {
        return Err(AppError::Unauthorized);
    }
    let needed = needed_scope(&parts.method, parts.uri.path()).ok_or(AppError::Forbidden)?;
    if !allows(&entry.scopes, &needed) {
        return Err(AppError::Forbidden);
    }
    let minute = Utc::now().timestamp() / 60;
    let rate_key = format!("api:rate:{}:{minute}", entry.id);
    let n: i64 = redis.incr(&rate_key, 1).await?;
    if n == 1 {
        let _: () = redis.expire(&rate_key, 120).await?;
    }
    if n > RATE_PER_MINUTE {
        return Err(AppError::TooManyRequests(format!("this API key is limited to {RATE_PER_MINUTE} requests a minute")));
    }
    let user = crate::auth::acting_as(state, entry.user_id).await?.ok_or(AppError::Unauthorized)?;
    if !user.can(perm::API_KEYS) {
        return Err(AppError::Unauthorized);
    }
    // Note the use at most once a minute.
    let first: bool = redis::cmd("SET").arg(format!("api:used:{}", entry.id)).arg(1).arg("NX").arg("EX").arg(60).query_async::<Option<String>>(&mut redis).await?.is_some();
    if first {
        let ip = crate::auth::client_ip(parts, state);
        sqlx::query("UPDATE api_keys SET last_used_at = now(), last_used_ip = $2 WHERE id = $1").bind(entry.id).bind(ip).execute(&state.db).await?;
    }
    Ok(AuthUser { id: user.id, session_id: entry.id, permissions: user.permissions })
}

#[derive(Serialize, FromRow)]
struct KeyRow {
    id: Uuid,
    user_id: Uuid,
    owner_name: String,
    name: String,
    prefix: String,
    scopes: Vec<String>,
    expires_at: Option<DateTime<Utc>>,
    last_used_at: Option<DateTime<Utc>>,
    last_used_ip: Option<String>,
    revoked_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

const KEY_SELECT: &str = "SELECT k.id, k.user_id, u.first_name || ' ' || u.last_name AS owner_name, k.name, k.prefix, k.scopes, k.expires_at,
    k.last_used_at, k.last_used_ip, k.revoked_at, k.created_at FROM api_keys k JOIN users u ON u.id = k.user_id";

#[derive(Deserialize)]
struct ListQuery {
    /// Everyone's keys (needs "Manage employees").
    all: Option<bool>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let all = q.all.unwrap_or(false);
    if all {
        auth.require(perm::USERS_MANAGE)?;
    }
    let items: Vec<KeyRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{KEY_SELECT} WHERE ($1 OR k.user_id = $2) ORDER BY k.revoked_at IS NOT NULL, k.created_at DESC LIMIT 500"
    )))
    .bind(all)
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    let scopes: Vec<Value> = SCOPES.iter().map(|(code, label)| json!({ "code": code, "label": label })).collect();
    Ok(Json(json!({ "items": items, "scopes": scopes, "can_create": auth.can(perm::API_KEYS), "can_manage_all": auth.can(perm::USERS_MANAGE) })))
}

#[derive(Deserialize)]
struct CreateBody {
    name: String,
    scopes: Vec<String>,
    /// Days until the key stops working; none = until revoked.
    expires_in_days: Option<i64>,
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<CreateBody>) -> AppResult<Json<Value>> {
    auth.require(perm::API_KEYS)?;
    let name = required("name", &b.name, 100)?;
    let mut scopes = b.scopes.clone();
    scopes.sort();
    scopes.dedup();
    if scopes.is_empty() || scopes.iter().any(|s| !SCOPES.iter().any(|(c, _)| c == s)) {
        return Err(AppError::bad(format!("choose scopes from: {}", SCOPES.iter().map(|s| s.0).collect::<Vec<_>>().join(", "))));
    }
    let expires = match b.expires_in_days {
        Some(d) if (1..=730).contains(&d) => Some(Utc::now() + Duration::days(d)),
        Some(_) => return Err(AppError::bad("a key can last from 1 day to 2 years")),
        None => None,
    };
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM api_keys WHERE user_id = $1 AND revoked_at IS NULL").bind(auth.id).fetch_one(&state.db).await?;
    if n >= 20 {
        return Err(AppError::bad("at most 20 keys at a time; revoke one you no longer use"));
    }
    let token = format!("{PREFIX}{}", tokens::random_token(30));
    let prefix: String = token.chars().take(PREFIX.len() + 6).collect();
    let id: Uuid = sqlx::query_scalar("INSERT INTO api_keys (user_id, name, prefix, secret_hash, scopes, expires_at) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id")
        .bind(auth.id)
        .bind(&name)
        .bind(&prefix)
        .bind(tokens::hash_token(&token))
        .bind(&scopes)
        .bind(expires)
        .fetch_one(&state.db)
        .await?;
    crate::audit::record(&state.db, Some(auth.id), "api.key.create", "api_key", Some(id.to_string()), json!({ "name": name, "scopes": scopes }), None).await;
    let row: KeyRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("{KEY_SELECT} WHERE k.id = $1"))).bind(id).fetch_one(&state.db).await?;
    // The secret is shown once; only its hash is kept.
    Ok(Json(json!({ "key": row, "secret": token })))
}

/// Revokes one's own key, or anyone's with "Manage employees".
async fn revoke(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let row: Option<(Uuid, String)> = sqlx::query_as("SELECT user_id, secret_hash FROM api_keys WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
    let Some((owner, hash)) = row else { return Err(AppError::NotFound("API key")) };
    if owner != auth.id && !auth.can(perm::USERS_MANAGE) {
        return Err(AppError::NotFound("API key"));
    }
    sqlx::query("UPDATE api_keys SET revoked_at = COALESCE(revoked_at, now()) WHERE id = $1").bind(id).execute(&state.db).await?;
    let mut redis = state.redis.clone();
    let _: () = redis.del(cache_key(&hash)).await?;
    crate::audit::record(&state.db, Some(auth.id), "api.key.revoke", "api_key", Some(id.to_string()), json!({}), None).await;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes() {
        assert_eq!(needed_scope(&Method::GET, "/api/crm/deal/items").as_deref(), Some("crm:read"));
        assert_eq!(needed_scope(&Method::PATCH, "/crm/deal/items/x").as_deref(), Some("crm"));
        assert_eq!(needed_scope(&Method::POST, "/api/tasks").as_deref(), Some("tasks"));
        assert_eq!(needed_scope(&Method::GET, "/api/projects/1").as_deref(), Some("tasks:read"));
        assert_eq!(needed_scope(&Method::GET, "/api/users").as_deref(), Some("users:read"));
        assert_eq!(needed_scope(&Method::POST, "/api/users"), None);
        assert_eq!(needed_scope(&Method::GET, "/api/api-keys"), None);
        assert_eq!(needed_scope(&Method::POST, "/api/auth/logout"), None);
        assert_eq!(needed_scope(&Method::GET, "/api/settings"), None);
        let s = vec!["crm".to_string(), "tasks:read".to_string()];
        assert!(allows(&s, "crm:read") && allows(&s, "crm") && allows(&s, "tasks:read"));
        assert!(!allows(&s, "tasks") && !allows(&s, "files"));
    }
}
