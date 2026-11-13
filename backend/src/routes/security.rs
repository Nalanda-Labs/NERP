//! Self-service account security: signed-in devices and two-factor authentication.

use axum::{
    extract::{Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::auth::{company_name, verify_user_password};
use crate::{
    audit,
    auth::{
        invalidate_authz,
        tokens::{self, ClientMeta},
        two_factor, AuthUser,
    },
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/sessions", get(list_sessions))
        .route("/auth/sessions/revoke-others", post(revoke_others))
        .route("/auth/sessions/{id}", delete(revoke_one))
        .route("/auth/2fa", get(two_factor_status))
        .route("/auth/2fa/setup", post(two_factor_setup))
        .route("/auth/2fa/enable", post(two_factor_enable))
        .route("/auth/2fa/disable", post(two_factor_disable))
        .route("/auth/2fa/recovery-codes", post(regenerate_codes))
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct SessionInfo {
    id: Uuid,
    current: bool,
    created_at: DateTime<Utc>,
    last_used_at: DateTime<Utc>,
    ip: Option<String>,
    user_agent: Option<String>,
}

pub async fn sessions_for(state: &AppState, user: Uuid, current: Option<Uuid>) -> AppResult<Vec<SessionInfo>> {
    Ok(tokens::list_sessions(state, user)
        .await?
        .into_iter()
        .map(|(id, d)| SessionInfo {
            id,
            current: Some(id) == current,
            created_at: d.created_at,
            last_used_at: d.last_used_at,
            ip: d.ip,
            user_agent: d.user_agent,
        })
        .collect())
}

async fn list_sessions(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Vec<SessionInfo>>> {
    Ok(Json(sessions_for(&state, auth.id, Some(auth.session_id)).await?))
}

async fn revoke_one(
    State(state): State<AppState>,
    auth: AuthUser,
    meta: ClientMeta,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let owned = tokens::get_session(&state, id).await?.is_some_and(|s| s.user_id == auth.id);
    if !owned {
        return Err(AppError::NotFound("session"));
    }
    tokens::revoke_session(&state, id).await?;
    audit::record(&state.db, Some(auth.id), "auth.session_revoked", "user", Some(auth.id.to_string()), json!({ "session": id }), meta.ip.as_deref()).await;
    Ok(Json(json!({ "ok": true })))
}

async fn revoke_others(State(state): State<AppState>, auth: AuthUser, meta: ClientMeta) -> AppResult<Json<Value>> {
    tokens::revoke_all_sessions(&state, auth.id, Some(auth.session_id)).await?;
    audit::record(&state.db, Some(auth.id), "auth.sessions_revoked", "user", Some(auth.id.to_string()), json!({}), meta.ip.as_deref()).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Two-factor authentication
// ---------------------------------------------------------------------------

fn setup_key(user: Uuid) -> String {
    format!("auth:2fa_setup:{user}")
}

async fn two_factor_status(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let enabled_at: Option<DateTime<Utc>> =
        sqlx::query_scalar!("SELECT totp_enabled_at FROM users WHERE id = $1 AND totp_secret IS NOT NULL", auth.id)
            .fetch_optional(&state.db)
            .await?
            .flatten();
    Ok(Json(json!({
        "enabled": enabled_at.is_some(),
        "enabled_at": enabled_at,
        "recovery_codes_remaining": if enabled_at.is_some() { two_factor::remaining_recovery_codes(&state, auth.id).await? } else { 0 },
    })))
}

#[derive(Deserialize)]
struct PasswordConfirm {
    password: String,
}

async fn require_password(state: &AppState, user: Uuid, password: String) -> AppResult<()> {
    if verify_user_password(state, user, password).await? {
        Ok(())
    } else {
        Err(AppError::bad("password is incorrect"))
    }
}

/// Step 1: create a secret (held in Redis for 10 minutes) and show it as a QR code.
async fn two_factor_setup(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<PasswordConfirm>,
) -> AppResult<Json<Value>> {
    require_password(&state, auth.id, body.password).await?;
    let email: String = sqlx::query_scalar!("SELECT email FROM users WHERE id = $1", auth.id)
        .fetch_one(&state.db)
        .await?;
    let secret = two_factor::new_secret();
    let totp = two_factor::build(&secret, &company_name(&state).await, &email)?;
    let qr = totp.to_qr_base64().map_err(|e| anyhow::anyhow!("QR generation failed: {e:?}"))?;
    let url = totp.to_url().map_err(|e| anyhow::anyhow!("otpauth URL failed: {e:?}"))?;
    let mut redis = state.redis.clone();
    let _: () = redis.set_ex(setup_key(auth.id), &secret, 600).await?;
    Ok(Json(json!({ "secret": secret, "otpauth_url": url, "qr_png_base64": qr })))
}

#[derive(Deserialize)]
struct CodeConfirm {
    code: String,
}

/// Step 2: confirm with a code from the app. Returns recovery codes (shown only once).
async fn two_factor_enable(
    State(state): State<AppState>,
    auth: AuthUser,
    meta: ClientMeta,
    Json(body): Json<CodeConfirm>,
) -> AppResult<Json<Value>> {
    let mut redis = state.redis.clone();
    let secret: Option<String> = redis.get(setup_key(auth.id)).await?;
    let secret = secret.ok_or_else(|| AppError::bad("setup expired; start again"))?;
    if !two_factor::verify_code(&state, auth.id, &secret, &body.code).await? {
        return Err(AppError::bad("that code is not valid; check the time on your phone and try again"));
    }
    sqlx::query!("UPDATE users SET totp_secret = $1, totp_enabled_at = now(), updated_at = now() WHERE id = $2", &secret, auth.id)
        .execute(&state.db)
        .await?;
    let codes = two_factor::regenerate_recovery_codes(&state.db, auth.id).await?;
    let _: () = redis.del(setup_key(auth.id)).await?;
    invalidate_authz(&state, &[auth.id]).await?;
    audit::record(&state.db, Some(auth.id), "auth.2fa_enabled", "user", Some(auth.id.to_string()), json!({}), meta.ip.as_deref()).await;
    Ok(Json(json!({ "recovery_codes": codes })))
}

async fn two_factor_disable(
    State(state): State<AppState>,
    auth: AuthUser,
    meta: ClientMeta,
    Json(body): Json<PasswordConfirm>,
) -> AppResult<Json<Value>> {
    if super::settings::two_factor_required(&state).await? {
        return Err(AppError::bad("your company requires two-step sign-in, so it can't be turned off"));
    }
    require_password(&state, auth.id, body.password).await?;
    disable_two_factor(&state, auth.id).await?;
    audit::record(&state.db, Some(auth.id), "auth.2fa_disabled", "user", Some(auth.id.to_string()), json!({}), meta.ip.as_deref()).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn disable_two_factor(state: &AppState, user: Uuid) -> AppResult<()> {
    sqlx::query!("UPDATE users SET totp_secret = NULL, totp_enabled_at = NULL, updated_at = now() WHERE id = $1", user)
        .execute(&state.db)
        .await?;
    sqlx::query!("DELETE FROM user_recovery_codes WHERE user_id = $1", user)
        .execute(&state.db)
        .await?;
    invalidate_authz(state, &[user]).await
}

async fn regenerate_codes(
    State(state): State<AppState>,
    auth: AuthUser,
    meta: ClientMeta,
    Json(body): Json<PasswordConfirm>,
) -> AppResult<Json<Value>> {
    require_password(&state, auth.id, body.password).await?;
    let enabled: bool = sqlx::query_scalar!(r#"SELECT totp_secret IS NOT NULL AS "v!" FROM users WHERE id = $1"#, auth.id)
        .fetch_one(&state.db)
        .await?;
    if !enabled {
        return Err(AppError::bad("two-factor authentication is not turned on"));
    }
    let codes = two_factor::regenerate_recovery_codes(&state.db, auth.id).await?;
    audit::record(&state.db, Some(auth.id), "auth.recovery_codes_regenerated", "user", Some(auth.id.to_string()), json!({}), meta.ip.as_deref()).await;
    Ok(Json(json!({ "recovery_codes": codes })))
}
