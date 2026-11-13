//! Sessions and tokens.
//!
//! A *session* is one signed-in device. It lives in PostgreSQL (`auth_sessions`, so losing
//! Redis signs nobody out) with a copy in Redis that requests check; it records when and
//! where it was used. Each session has:
//! - a refresh token: opaque, stored hashed, rotated on every use, sent as an httpOnly cookie;
//! - short-lived JWT access tokens that carry the session id (`sid`).
//!
//! Revoking a session deletes it and marks its id revoked for the lifetime of an access
//! token, so the device loses access immediately rather than at the next refresh.
//!
//! Refresh tokens are exchanged in one database statement; exchanged ones are remembered in
//! `auth_used_refresh` (reuse = theft). Emailed link tokens live in `auth_link_tokens`.
//!
//! Redis keys (a cache; refilled from the database when missing):
//!   auth:sess:{sid}           -> SessionData (JSON)
//!   auth:revoked_sid:{sid}    -> 1 (short TTL)

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, Header, Validation};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    realtime::{self, Envelope},
    state::AppState,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub sid: Uuid,
    pub iat: i64,
    pub exp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub user_id: Uuid,
    pub refresh_hash: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: DateTime<Utc>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

/// Who/where a request came from; recorded on the session.
#[derive(Debug, Clone, Default)]
pub struct ClientMeta {
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::fill(&mut buf[..]);
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}


/// Two tabs of the same browser may refresh at the same moment; a token reused this soon
/// after it was exchanged is refused but not treated as theft.
const REUSE_GRACE_SECS: i64 = 30;

fn sess_key(sid: Uuid) -> String {
    format!("auth:sess:{sid}")
}
pub fn revoked_sid_key(sid: Uuid) -> String {
    format!("auth:revoked_sid:{sid}")
}

pub fn issue_access_token(state: &AppState, user_id: Uuid, sid: Uuid) -> AppResult<(String, i64)> {
    let now = Utc::now().timestamp();
    let ttl = state.config.access_token_ttl_secs;
    let claims = Claims { sub: user_id, sid, iat: now, exp: now + ttl };
    let token = jsonwebtoken::encode(&Header::new(Algorithm::HS256), &claims, &state.jwt.encoding)
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok((token, ttl))
}

pub fn decode_access_token(state: &AppState, token: &str) -> AppResult<Claims> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 5;
    jsonwebtoken::decode::<Claims>(token, &state.jwt.decoding, &validation)
        .map(|d| d.claims)
        .map_err(|_| AppError::Unauthorized)
}

async fn store_session(state: &AppState, sid: Uuid, data: &SessionData) -> AppResult<()> {
    let ttl = state.config.refresh_token_ttl_secs;
    sqlx::query!("INSERT INTO auth_sessions (id, user_id, refresh_hash, created_at, last_used_at, ip, user_agent, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now() + make_interval(secs => $8))
         ON CONFLICT (id) DO UPDATE SET refresh_hash = EXCLUDED.refresh_hash, last_used_at = EXCLUDED.last_used_at,
             ip = EXCLUDED.ip, user_agent = EXCLUDED.user_agent, expires_at = EXCLUDED.expires_at", sid, data.user_id, &data.refresh_hash, data.created_at, data.last_used_at, data.ip.as_deref(), data.user_agent.as_deref(), ttl as f64)
    .execute(&state.db)
    .await?;
    cache_session(state, sid, data).await;
    Ok(())
}

/// How long Redis's copy of a session lives: a copy that outlived its session (a sign-out
/// while Redis couldn't be reached) is gone within this, and a missing one is refilled from
/// the database.
const CACHE_SECS: u64 = 300;

/// Keeps a copy in Redis for the requests that check it.
async fn cache_session(state: &AppState, sid: Uuid, data: &SessionData) {
    let mut redis = state.redis.clone();
    let _: Result<(), _> = redis.set_ex(sess_key(sid), serde_json::to_string(data).unwrap(), CACHE_SECS).await;
}

/// Starts a new session. Returns (session id, refresh token).
pub async fn create_session(state: &AppState, user_id: Uuid, meta: &ClientMeta) -> AppResult<(Uuid, String)> {
    let sid = Uuid::new_v4();
    let token = random_token(32);
    let now = Utc::now();
    let data = SessionData {
        user_id,
        refresh_hash: hash_token(&token),
        created_at: now,
        last_used_at: now,
        ip: meta.ip.clone(),
        user_agent: meta.user_agent.as_ref().map(|u| u.chars().take(400).collect()),
    };
    store_session(state, sid, &data).await?;
    Ok((sid, token))
}

type SessionRow = (Uuid, String, DateTime<Utc>, DateTime<Utc>, Option<String>, Option<String>);

fn from_row(r: SessionRow) -> SessionData {
    SessionData { user_id: r.0, refresh_hash: r.1, created_at: r.2, last_used_at: r.3, ip: r.4, user_agent: r.5 }
}

pub async fn get_session(state: &AppState, sid: Uuid) -> AppResult<Option<SessionData>> {
    let mut redis = state.redis.clone();
    let raw: Option<String> = redis.get(sess_key(sid)).await.unwrap_or(None);
    if let Some(d) = raw.and_then(|r| serde_json::from_str(&r).ok()) {
        return Ok(Some(d));
    }
    let row: Option<SessionRow> = sqlx::query!(r#"SELECT user_id, refresh_hash, created_at, last_used_at, ip, user_agent FROM auth_sessions WHERE id = $1 AND expires_at > now()"#, sid).map(|r| (r.user_id, r.refresh_hash, r.created_at, r.last_used_at, r.ip, r.user_agent))
    .fetch_optional(&state.db)
    .await?;
    let data = row.map(from_row);
    if let Some(d) = &data {
        cache_session(state, sid, d).await;
    }
    Ok(data)
}

/// Whether a session is still signed in (for every request: Redis, else the database).
pub async fn session_alive(state: &AppState, sid: Uuid) -> AppResult<bool> {
    let mut redis = state.redis.clone();
    if redis.exists::<_, bool>(sess_key(sid)).await.unwrap_or(false) {
        return Ok(true);
    }
    Ok(get_session(state, sid).await?.is_some())
}

/// Exchanges a refresh token for a new one (single use). Returns (user, session, new token).
pub async fn rotate_refresh_token(
    state: &AppState,
    token: &str,
    meta: &ClientMeta,
) -> AppResult<Option<(Uuid, Uuid, String)>> {
    let hash = hash_token(token);
    let new_token = random_token(32);
    let ttl = state.config.refresh_token_ttl_secs;
    // One statement: of two requests with the same token, only one gets the session.
    let row: Option<(Uuid, SessionRow)> = sqlx::query!(r#"UPDATE auth_sessions SET refresh_hash = $2, last_used_at = now(), ip = COALESCE($3, ip), user_agent = COALESCE($4, user_agent),
             expires_at = now() + make_interval(secs => $5)
         WHERE refresh_hash = $1 AND expires_at > now()
         RETURNING id, user_id, refresh_hash, created_at, last_used_at, ip, user_agent"#, &hash, hash_token(&new_token), meta.ip.as_deref(), meta.user_agent.as_ref().map(|u| u.chars().take(400).collect::<String>()), ttl as f64).map(|r| (r.id, r.user_id, r.refresh_hash, r.created_at, r.last_used_at, r.ip, r.user_agent))
    .fetch_optional(&state.db)
    .await?
    .map(|r| (r.0, (r.1, r.2, r.3, r.4, r.5, r.6)));
    let Some((sid, row)) = row else {
        // An exchanged token presented again: past the grace period, assume it was stolen.
        let used: Option<(Uuid, DateTime<Utc>)> = sqlx::query!(r#"SELECT session_id, used_at FROM auth_used_refresh WHERE hash = $1 AND expires_at > now()"#, &hash).map(|r| (r.session_id, r.used_at))
            .fetch_optional(&state.db)
            .await?;
        if let Some((sid, at)) = used
            && (Utc::now() - at).num_seconds() > REUSE_GRACE_SECS {
                let user = get_session(state, sid).await?.map(|d| d.user_id);
                revoke_session(state, sid).await?;
                crate::audit::record(&state.db, user, "auth.refresh_reuse", "session", Some(sid.to_string()), serde_json::json!({}), meta.ip.as_deref()).await;
                if let Some(u) = user {
                    crate::notify::notify(state, &[u], crate::notify::NewNotification {
                        kind: "security.session_ended",
                        title: "A sign-in on one of your devices was ended for safety".into(),
                        body: Some("An old sign-in token was used again, which can mean it was copied. Sign in again on that device; change your password if this wasn't you.".into()),
                        link: Some("/profile#security".into()),
                        actor: None,
                    })
                    .await;
                }
            }
        return Ok(None);
    };
    sqlx::query!("INSERT INTO auth_used_refresh (hash, session_id, used_at, expires_at) VALUES ($1, $2, now(), now() + make_interval(secs => $3)) ON CONFLICT (hash) DO NOTHING", &hash, sid, ttl as f64)
        .execute(&state.db)
        .await?;
    let data = from_row(row);
    cache_session(state, sid, &data).await;
    Ok(Some((data.user_id, sid, new_token)))
}

/// Ends one session: its refresh token stops working and its access tokens are rejected.
pub async fn revoke_session(state: &AppState, sid: Uuid) -> AppResult<()> {
    let user: Option<Uuid> = sqlx::query_scalar!("DELETE FROM auth_sessions WHERE id = $1 RETURNING user_id", sid).fetch_optional(&state.db).await?;
    // Redis's copy goes too (tried a few times; if Redis can't be reached, the copy runs out
    // within CACHE_SECS and the session is gone from the database already).
    let mut redis = state.redis.clone();
    let mut done = false;
    for attempt in 0..3 {
        let r = redis::pipe()
            .del(sess_key(sid))
            .set_ex(revoked_sid_key(sid), 1, (state.config.access_token_ttl_secs + 60) as u64)
            .query_async::<()>(&mut redis)
            .await;
        if r.is_ok() {
            done = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200 * (attempt + 1))).await;
    }
    if !done {
        tracing::warn!(session = %sid, "signed out, but Redis could not be told; its copy runs out within five minutes");
    }
    if let Some(u) = user {
        // Close that device's live connections and tell it to sign out.
        realtime::publish_envelope(
            state,
            Envelope {
                users: Some(vec![u]),
                session: Some(sid),
                kind: "session.revoked".into(),
                data: serde_json::json!({ "session_id": sid }),
            },
        )
        .await;
    }
    Ok(())
}

/// Signs a user out everywhere, optionally keeping one session.
pub async fn revoke_all_sessions(state: &AppState, user_id: Uuid, keep: Option<Uuid>) -> AppResult<()> {
    let sids: Vec<Uuid> = sqlx::query_scalar!("SELECT id FROM auth_sessions WHERE user_id = $1", user_id).fetch_all(&state.db).await?;
    for sid in sids {
        if Some(sid) != keep {
            revoke_session(state, sid).await?;
        }
    }
    Ok(())
}

/// Lists live sessions, newest use first.
pub async fn list_sessions(state: &AppState, user_id: Uuid) -> AppResult<Vec<(Uuid, SessionData)>> {
    let rows: Vec<(Uuid, Uuid, String, DateTime<Utc>, DateTime<Utc>, Option<String>, Option<String>)> = sqlx::query!(r#"SELECT id, user_id, refresh_hash, created_at, last_used_at, ip, user_agent FROM auth_sessions
         WHERE user_id = $1 AND expires_at > now() ORDER BY last_used_at DESC"#, user_id).map(|r| (r.id, r.user_id, r.refresh_hash, r.created_at, r.last_used_at, r.ip, r.user_agent))
    .fetch_all(&state.db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.0, from_row((r.1, r.2, r.3, r.4, r.5, r.6)))).collect())
}

/// At start: sessions that so far lived only in Redis are written to the database (once,
/// after the upgrade that moved them), so nobody is signed out by it.
pub async fn adopt_redis_sessions(state: &AppState) -> anyhow::Result<()> {
    let mut redis = state.redis.clone();
    let keys: Vec<String> = {
        let mut iter: redis::AsyncIter<String> = redis.scan_match("auth:sess:*").await?;
        let mut out = Vec::new();
        while let Some(k) = futures_util::StreamExt::next(&mut iter).await {
            out.push(k?);
        }
        out
    };
    let mut adopted = 0;
    for k in keys {
        let Some(sid) = k.strip_prefix("auth:sess:").and_then(|s| Uuid::parse_str(s).ok()) else { continue };
        let raw: Option<String> = redis.get(&k).await?;
        let ttl: i64 = redis.ttl(&k).await.unwrap_or(-1);
        let Some(d) = raw.and_then(|r| serde_json::from_str::<SessionData>(&r).ok()) else { continue };
        if ttl <= 0 {
            continue;
        }
        let done = sqlx::query!("INSERT INTO auth_sessions (id, user_id, refresh_hash, created_at, last_used_at, ip, user_agent, expires_at)
             SELECT $1, $2, $3, $4, $5, $6, $7, now() + make_interval(secs => $8) WHERE EXISTS (SELECT 1 FROM users WHERE id = $2)
             ON CONFLICT DO NOTHING", sid, d.user_id, &d.refresh_hash, d.created_at, d.last_used_at, d.ip.as_deref(), d.user_agent.as_deref(), ttl as f64)
        .execute(&state.db)
        .await?
        .rows_affected();
        adopted += done;
    }
    if adopted > 0 {
        tracing::info!(adopted, "sessions from Redis written to the database");
    }
    // Keys only the Redis-only sessions used (the database holds that now).
    for pattern in ["auth:rt:*", "auth:rt_used:*", "auth:user_sess:*"] {
        let keys: Vec<String> = {
            let mut iter: redis::AsyncIter<String> = redis.scan_match(pattern).await?;
            let mut out = Vec::new();
            while let Some(k) = futures_util::StreamExt::next(&mut iter).await {
                out.push(k?);
            }
            out
        };
        for chunk in keys.chunks(500) {
            let _: () = redis::cmd("UNLINK").arg(chunk).query_async(&mut redis).await?;
        }
    }
    Ok(())
}

pub const CLEANUP_JOB: &str = "auth.cleanup";

/// Daily: expired sessions and tokens go.
pub async fn cleanup(state: &AppState) -> anyhow::Result<()> {
    sqlx::query!("DELETE FROM auth_sessions WHERE expires_at < now()").execute(&state.db).await?;
    sqlx::query!("DELETE FROM auth_used_refresh WHERE expires_at < now()").execute(&state.db).await?;
    sqlx::query!("DELETE FROM auth_link_tokens WHERE expires_at < now()").execute(&state.db).await?;
    Ok(())
}

pub const REFRESH_COOKIE: &str = "nerp_rt";

pub fn refresh_cookie(state: &AppState, token: &str) -> String {
    format!(
        "{REFRESH_COOKIE}={token}; Path=/api/auth; HttpOnly; SameSite=Strict; Max-Age={}{}",
        state.config.refresh_token_ttl_secs,
        if state.config.cookie_secure { "; Secure" } else { "" }
    )
}

pub fn clear_refresh_cookie(state: &AppState) -> String {
    format!(
        "{REFRESH_COOKIE}=; Path=/api/auth; HttpOnly; SameSite=Strict; Max-Age=0{}",
        if state.config.cookie_secure { "; Secure" } else { "" }
    )
}

pub fn read_cookie(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// One-time tokens for emailed links (password reset, invitations), stored hashed in the
/// database. `scope` keeps different purposes apart; a newer token for the same person and
/// purpose replaces the older one.
pub async fn issue_link_token(
    state: &AppState,
    scope: &str,
    owner: Uuid,
    value: &serde_json::Value,
    ttl_secs: u64,
) -> AppResult<String> {
    let token = random_token(32);
    let mut tx = state.db.begin().await?;
    sqlx::query!("DELETE FROM auth_link_tokens WHERE scope = $1 AND owner_id = $2", scope, owner).execute(&mut *tx).await?;
    sqlx::query!("INSERT INTO auth_link_tokens (hash, scope, owner_id, value, expires_at) VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))", hash_token(&token), scope, owner, value, ttl_secs as f64)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(token)
}

pub async fn peek_link_token(state: &AppState, scope: &str, token: &str) -> AppResult<Option<serde_json::Value>> {
    Ok(sqlx::query_scalar!("SELECT value FROM auth_link_tokens WHERE hash = $1 AND scope = $2 AND expires_at > now()", hash_token(token), scope)
        .fetch_optional(&state.db)
        .await?)
}

pub async fn consume_link_token(state: &AppState, scope: &str, token: &str) -> AppResult<Option<serde_json::Value>> {
    Ok(sqlx::query_scalar!("DELETE FROM auth_link_tokens WHERE hash = $1 AND scope = $2 AND expires_at > now() RETURNING value", hash_token(token), scope)
        .fetch_optional(&state.db)
        .await?)
}

pub async fn revoke_link_tokens(state: &AppState, scope: &str, owner: Uuid) -> AppResult<()> {
    sqlx::query!("DELETE FROM auth_link_tokens WHERE scope = $1 AND owner_id = $2", scope, owner).execute(&state.db).await?;
    Ok(())
}
