//! Sessions and tokens.
//!
//! A *session* is one signed-in device. It lives in Redis and records when and where it
//! was used. Each session has:
//! - a refresh token: opaque, stored hashed, rotated on every use, sent as an httpOnly cookie;
//! - short-lived JWT access tokens that carry the session id (`sid`).
//!
//! Revoking a session deletes it and marks its id revoked for the lifetime of an access
//! token, so the device loses access immediately rather than at the next refresh.
//!
//! Redis keys:
//!   auth:rt:{sha256(token)}   -> session id
//!   auth:sess:{sid}           -> SessionData (JSON)
//!   auth:user_sess:{user}     -> set of session ids
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

fn rt_key(hash: &str) -> String {
    format!("auth:rt:{hash}")
}
fn sess_key(sid: Uuid) -> String {
    format!("auth:sess:{sid}")
}
fn user_sess_key(user: Uuid) -> String {
    format!("auth:user_sess:{user}")
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
    let ttl = state.config.refresh_token_ttl_secs as u64;
    let mut redis = state.redis.clone();
    redis::pipe()
        .set_ex(rt_key(&data.refresh_hash), sid.to_string(), ttl)
        .set_ex(sess_key(sid), serde_json::to_string(data).unwrap(), ttl)
        .sadd(user_sess_key(data.user_id), sid.to_string())
        .expire(user_sess_key(data.user_id), ttl as i64)
        .query_async::<()>(&mut redis)
        .await?;
    Ok(())
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

pub async fn get_session(state: &AppState, sid: Uuid) -> AppResult<Option<SessionData>> {
    let mut redis = state.redis.clone();
    let raw: Option<String> = redis.get(sess_key(sid)).await?;
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
}

/// Exchanges a refresh token for a new one (single use). Returns (user, session, new token).
pub async fn rotate_refresh_token(
    state: &AppState,
    token: &str,
    meta: &ClientMeta,
) -> AppResult<Option<(Uuid, Uuid, String)>> {
    let mut redis = state.redis.clone();
    let sid: Option<String> = redis.get_del(rt_key(&hash_token(token))).await?;
    let Some(sid) = sid.and_then(|s| Uuid::parse_str(&s).ok()) else {
        return Ok(None);
    };
    let Some(mut data) = get_session(state, sid).await? else {
        return Ok(None);
    };
    let new_token = random_token(32);
    data.refresh_hash = hash_token(&new_token);
    data.last_used_at = Utc::now();
    if meta.ip.is_some() {
        data.ip = meta.ip.clone();
    }
    if meta.user_agent.is_some() {
        data.user_agent = meta.user_agent.clone();
    }
    store_session(state, sid, &data).await?;
    Ok(Some((data.user_id, sid, new_token)))
}

/// Ends one session: its refresh token stops working and its access tokens are rejected.
pub async fn revoke_session(state: &AppState, sid: Uuid) -> AppResult<()> {
    let data = get_session(state, sid).await?;
    let mut redis = state.redis.clone();
    let mut pipe = redis::pipe();
    pipe.del(sess_key(sid)).set_ex(
        revoked_sid_key(sid),
        1,
        (state.config.access_token_ttl_secs + 60) as u64,
    );
    if let Some(d) = &data {
        pipe.del(rt_key(&d.refresh_hash)).srem(user_sess_key(d.user_id), sid.to_string());
    }
    pipe.query_async::<()>(&mut redis).await?;
    if let Some(d) = data {
        // Close that device's live connections and tell it to sign out.
        realtime::publish_envelope(
            state,
            Envelope {
                users: Some(vec![d.user_id]),
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
    let mut redis = state.redis.clone();
    let sids: Vec<String> = redis.smembers(user_sess_key(user_id)).await?;
    for sid in sids.iter().filter_map(|s| Uuid::parse_str(s).ok()) {
        if Some(sid) != keep {
            revoke_session(state, sid).await?;
        }
    }
    Ok(())
}

/// Lists live sessions, pruning ids whose data has expired.
pub async fn list_sessions(state: &AppState, user_id: Uuid) -> AppResult<Vec<(Uuid, SessionData)>> {
    let mut redis = state.redis.clone();
    let sids: Vec<String> = redis.smembers(user_sess_key(user_id)).await?;
    let mut out = Vec::new();
    for raw in sids {
        let Ok(sid) = Uuid::parse_str(&raw) else { continue };
        match get_session(state, sid).await? {
            Some(d) => out.push((sid, d)),
            None => {
                let _: () = redis.srem(user_sess_key(user_id), &raw).await?;
            }
        }
    }
    out.sort_by_key(|(_, d)| std::cmp::Reverse(d.last_used_at));
    Ok(out)
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

/// One-time tokens for emailed links (password reset, invitations), stored hashed.
/// `scope` keeps different purposes apart; `owner_key` lets a newer token replace an older one.
pub async fn issue_link_token(
    state: &AppState,
    scope: &str,
    owner: Uuid,
    value: &serde_json::Value,
    ttl_secs: u64,
) -> AppResult<String> {
    let token = random_token(32);
    let hash = hash_token(&token);
    let mut redis = state.redis.clone();
    let owner_key = format!("auth:{scope}_owner:{owner}");
    let previous: Option<String> = redis.get(&owner_key).await?;
    let mut pipe = redis::pipe();
    if let Some(prev) = previous {
        pipe.del(format!("auth:{scope}:{prev}"));
    }
    pipe.set_ex(format!("auth:{scope}:{hash}"), value.to_string(), ttl_secs)
        .set_ex(&owner_key, &hash, ttl_secs);
    pipe.query_async::<()>(&mut redis).await?;
    Ok(token)
}

pub async fn peek_link_token(state: &AppState, scope: &str, token: &str) -> AppResult<Option<serde_json::Value>> {
    let mut redis = state.redis.clone();
    let raw: Option<String> = redis.get(format!("auth:{scope}:{}", hash_token(token))).await?;
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
}

pub async fn consume_link_token(state: &AppState, scope: &str, token: &str) -> AppResult<Option<serde_json::Value>> {
    let mut redis = state.redis.clone();
    let raw: Option<String> = redis.get_del(format!("auth:{scope}:{}", hash_token(token))).await?;
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
}

pub async fn revoke_link_tokens(state: &AppState, scope: &str, owner: Uuid) -> AppResult<()> {
    let mut redis = state.redis.clone();
    let owner_key = format!("auth:{scope}_owner:{owner}");
    let previous: Option<String> = redis.get_del(&owner_key).await?;
    if let Some(prev) = previous {
        let _: () = redis.del(format!("auth:{scope}:{prev}")).await?;
    }
    Ok(())
}
