pub mod app_passwords;
pub mod ip;
pub mod password;
pub mod permissions;
pub mod tokens;
pub mod two_factor;

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{header, request::Parts},
};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, net::SocketAddr};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    state::AppState,
};
use tokens::ClientMeta;

/// What the request handlers know about the caller. Built from the JWT plus the
/// cached authorization entry, so most requests never touch the database for auth.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Uuid,
    /// The signed-in device this request came from.
    pub session_id: Uuid,
    pub permissions: HashSet<String>,
}

impl AuthUser {
    /// Someone from outside the company (see `GUEST`).
    pub fn is_guest(&self) -> bool {
        self.permissions.contains(GUEST)
    }

    pub fn can(&self, permission: &str) -> bool {
        self.permissions.contains(permissions::ALL) || self.permissions.contains(permission)
    }

    pub fn require(&self, permission: &str) -> AppResult<()> {
        if self.can(permission) {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
}

/// Cached in Redis under `auth:authz:{user_id}`.
#[derive(Serialize, Deserialize)]
struct AuthzEntry {
    active: bool,
    /// Tokens issued before this unix time are rejected (set on password change).
    valid_after: i64,
    permissions: Vec<String>,
}

const AUTHZ_TTL_SECS: u64 = 300;

fn authz_key(user_id: Uuid) -> String {
    format!("auth:authz:{user_id}")
}

/// Drop the cached authorization entry. Call after any change to a user's
/// roles, status or password, or to a role's permissions.
pub async fn invalidate_authz(state: &AppState, user_ids: &[Uuid]) -> AppResult<()> {
    if user_ids.is_empty() {
        return Ok(());
    }
    let keys: Vec<String> = user_ids.iter().map(|id| authz_key(*id)).collect();
    let mut redis = state.redis.clone();
    let _: () = redis.del(keys).await?;
    Ok(())
}

/// An active person acting without a request of their own (automation acting for them).
pub async fn load_auth_user(state: &AppState, user_id: Uuid) -> AppResult<AuthUser> {
    let active: bool = sqlx::query_scalar("SELECT is_active FROM users WHERE id = $1").bind(user_id).fetch_optional(&state.db).await?.unwrap_or(false);
    if !active {
        return Err(AppError::bad("that person is no longer active"));
    }
    let perms = load_permissions(state, user_id).await?;
    Ok(AuthUser { id: user_id, session_id: Uuid::nil(), permissions: perms.into_iter().collect() })
}

pub async fn load_permissions(state: &AppState, user_id: Uuid) -> AppResult<Vec<String>> {
    let mut perms = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT rp.permission FROM user_roles ur
         JOIN role_permissions rp ON rp.role_id = ur.role_id
         WHERE ur.user_id = $1",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;
    // A guest from outside the company carries a marker (never a grantable permission).
    let guest: bool = sqlx::query_scalar("SELECT is_guest FROM users WHERE id = $1").bind(user_id).fetch_optional(&state.db).await?.unwrap_or(false);
    if guest {
        perms.retain(|p| p != permissions::ALL);
        perms.push(GUEST.into());
    }
    Ok(perms)
}

/// The marker of a guest's permission set.
pub const GUEST: &str = "@guest";

/// What a guest may reach: their chats, calls, workgroups and tasks, files, boards shared with
/// them by name, notifications, and their own account. Everything else answers 403.
const GUEST_PATHS: &[&str] = &[
    "/auth/", "/me/", "/profile", "/notifications", "/realtime/", "/files", "/download-links", "/im/", "/calls", "/projects", "/tasks", "/feed", "/boards",
    // Their workgroups' files (the drive itself keeps them to those), and editing them.
    "/drive", "/office/status",
];

fn guest_may(path: &str, method: &axum::http::Method) -> bool {
    let path = path.strip_prefix("/api").unwrap_or(path);
    let under = |base: &str| {
        let base = base.trim_end_matches('/');
        path == base || path.strip_prefix(base).is_some_and(|rest| rest.starts_with('/'))
    };
    GUEST_PATHS.iter().any(|p| under(p)) || (path == "/settings" && *method == axum::http::Method::GET)
}

async fn load_authz(state: &AppState, user_id: Uuid) -> AppResult<Option<AuthzEntry>> {
    let mut redis = state.redis.clone();
    let cached: Option<String> = redis.get(authz_key(user_id)).await?;
    if let Some(entry) = cached.and_then(|c| serde_json::from_str(&c).ok()) {
        return Ok(Some(entry));
    }

    let row: Option<(bool, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as("SELECT is_active, password_changed_at FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((active, changed_at)) = row else {
        return Ok(None);
    };
    let entry = AuthzEntry {
        active,
        valid_after: changed_at.timestamp(),
        permissions: load_permissions(state, user_id).await?,
    };
    let _: () = redis
        .set_ex(authz_key(user_id), serde_json::to_string(&entry).unwrap(), AUTHZ_TTL_SECS)
        .await?;
    Ok(Some(entry))
}

/// Acts on behalf of a user outside a request (e.g. a scheduled job). `None` if the
/// user no longer exists or is deactivated.
pub async fn acting_as(state: &AppState, user_id: Uuid) -> AppResult<Option<AuthUser>> {
    let active: Option<bool> = sqlx::query_scalar("SELECT is_active FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;
    if active != Some(true) {
        return Ok(None);
    }
    Ok(Some(AuthUser {
        id: user_id,
        session_id: Uuid::nil(),
        permissions: load_permissions(state, user_id).await?.into_iter().collect(),
    }))
}

fn bearer_token(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let user = identify(parts, state).await?;
        ip::check(state, &user, client_ip(parts, state).as_deref()).await?;
        if user.is_guest() {
            let path = parts.extensions.get::<axum::extract::OriginalUri>().map(|u| u.0.path().to_string()).unwrap_or_else(|| parts.uri.path().to_string());
            if !guest_may(&path, &parts.method) {
                return Err(AppError::Forbidden);
            }
        }
        Ok(user)
    }
}

/// Who is calling: a session's access token, an API key or a download link.
async fn identify(parts: &mut Parts, state: &AppState) -> Result<AuthUser, AppError> {
    {
        let Some(token) = bearer_token(parts) else {
            // A download link (GET only, for the one path it was made for).
            return match download_token(parts) {
                Some(t) => download_link_user(state, parts, &t).await,
                None => Err(AppError::Unauthorized),
            };
        };
        if token.starts_with(crate::api::keys::PREFIX) {
            let token = token.to_string();
            return crate::api::keys::authenticate(state, parts, &token).await;
        }
        let claims = tokens::decode_access_token(state, token)?;

        let mut redis = state.redis.clone();
        let revoked: bool = redis.exists(tokens::revoked_sid_key(claims.sid)).await?;
        if revoked {
            return Err(AppError::Unauthorized);
        }

        let entry = load_authz(state, claims.sub).await?.ok_or(AppError::Unauthorized)?;
        if !entry.active || claims.iat < entry.valid_after {
            return Err(AppError::Unauthorized);
        }
        // When someone was last active (written at most every two minutes).
        let fresh: Option<String> = redis::cmd("SET").arg(format!("seen:{}", claims.sub)).arg(1).arg("NX").arg("EX").arg(120).query_async(&mut redis).await.unwrap_or(None);
        if fresh.is_some() {
            let _ = sqlx::query("UPDATE users SET last_seen_at = now() WHERE id = $1").bind(claims.sub).execute(&state.db).await;
        }

        Ok(AuthUser {
            id: claims.sub,
            session_id: claims.sid,
            permissions: entry.permissions.into_iter().collect(),
        })
    }
}

/// The `dl` query parameter of a GET request: a short-lived download link.
fn download_token(parts: &Parts) -> Option<String> {
    if parts.method != axum::http::Method::GET {
        return None;
    }
    parts.uri.query()?.split('&').find_map(|kv| kv.strip_prefix("dl=")).filter(|t| !t.is_empty() && t.len() <= 64).map(String::from)
}

pub const DOWNLOAD_LINK_SECS: u64 = 10 * 60;

/// Who a download link was made for, if it is for this path and they may still sign in.
async fn download_link_user(state: &AppState, parts: &Parts, token: &str) -> Result<AuthUser, AppError> {
    let mut redis = state.redis.clone();
    let saved: Option<String> = redis.get(format!("auth:dl:{token}")).await?;
    let v: serde_json::Value = saved.and_then(|s| serde_json::from_str(&s).ok()).ok_or(AppError::Unauthorized)?;
    // The full path: routers nested under /api see it without that prefix.
    let path = parts.extensions.get::<axum::extract::OriginalUri>().map(|u| u.0.path().to_string()).unwrap_or_else(|| parts.uri.path().to_string());
    if v["path"].as_str() != Some(path.as_str()) {
        return Err(AppError::Unauthorized);
    }
    let user: Uuid = v["user"].as_str().and_then(|s| Uuid::parse_str(s).ok()).ok_or(AppError::Unauthorized)?;
    let sid: Uuid = v["sid"].as_str().and_then(|s| Uuid::parse_str(s).ok()).ok_or(AppError::Unauthorized)?;
    let at = v["at"].as_i64().unwrap_or(0);
    // Signed out, or deactivated / password changed since: the link dies too.
    let revoked: bool = redis.exists(tokens::revoked_sid_key(sid)).await?;
    let entry = load_authz(state, user).await?.ok_or(AppError::Unauthorized)?;
    if revoked || !entry.active || at < entry.valid_after {
        return Err(AppError::Unauthorized);
    }
    Ok(AuthUser { id: user, session_id: sid, permissions: entry.permissions.into_iter().collect() })
}

/// Makes a download link for `path` (for this user's session), valid for 10 minutes and for
/// as many requests as a video player needs.
pub async fn make_download_link(state: &AppState, auth: &AuthUser, path: &str) -> AppResult<String> {
    let token = tokens::random_token(24);
    let mut redis = state.redis.clone();
    let _: () = redis
        .set_ex(
            format!("auth:dl:{token}"),
            serde_json::json!({ "user": auth.id, "sid": auth.session_id, "path": path, "at": chrono::Utc::now().timestamp() }).to_string(),
            DOWNLOAD_LINK_SECS,
        )
        .await?;
    Ok(token)
}

/// Like `AuthUser`, but yields `None` instead of rejecting anonymous requests.
pub struct MaybeAuthUser(pub Option<AuthUser>);

impl FromRequestParts<AppState> for MaybeAuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if bearer_token(parts).is_none() && download_token(parts).is_none() {
            return Ok(MaybeAuthUser(None));
        }
        match AuthUser::from_request_parts(parts, state).await {
            Ok(u) => Ok(MaybeAuthUser(Some(u))),
            Err(AppError::Unauthorized) => Ok(MaybeAuthUser(None)),
            Err(e) => Err(e),
        }
    }
}

/// Best-effort client address for rate limiting and the audit log.
pub struct ClientIp(pub Option<String>);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        Ok(ClientIp(client_ip(parts, state)))
    }
}

impl FromRequestParts<AppState> for ClientMeta {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        Ok(ClientMeta {
            ip: client_ip(parts, state),
            user_agent: parts
                .headers
                .get(header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.chars().take(400).collect()),
        })
    }
}

pub(crate) fn client_ip(parts: &Parts, state: &AppState) -> Option<String> {
    if state.config.trust_proxy {
            let forwarded = parts
                .headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .map(|v| v.trim().to_string())
                .or_else(|| {
                    parts.headers.get("x-real-ip").and_then(|v| v.to_str().ok()).map(String::from)
                });
            if forwarded.is_some() {
                return forwarded;
            }
    }
    parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(a)| a.ip().to_string())
}

#[cfg(test)]
mod guest_tests {
    use axum::http::Method;

    #[test]
    fn guests_reach_only_their_areas() {
        for p in ["/api/im/chats", "/api/calls/1/join", "/api/projects", "/api/tasks/5", "/api/auth/me", "/api/files/x/content", "/api/realtime/ticket", "/api/notifications"] {
            assert!(super::guest_may(p, &Method::GET), "{p}");
        }
        for p in ["/api/users", "/api/crm/deal/items", "/api/hr/my-day", "/api/bi/dashboards", "/api/kb/spaces", "/api/drive/nodes", "/api/feedback", "/api/imx", "/api/tasksX"] {
            assert!(!super::guest_may(p, &Method::GET), "{p}");
        }
        assert!(super::guest_may("/api/settings", &Method::GET) && !super::guest_may("/api/settings", &Method::PUT));
    }
}
