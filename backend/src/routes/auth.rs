use axum::{
    extract::{Path, State},
    http::{header, HeaderMap},
    response::{AppendHeaders, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{normalize_email, users};
use crate::{
    audit,
    auth::{
        invalidate_authz, load_permissions, password,
        tokens::{self, ClientMeta},
        two_factor, AuthUser,
    },
    error::{AppError, AppResult},
    jobs, mail,
    notify::{notify, NewNotification},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/login/2fa", post(login_two_factor))
        .route("/auth/login/2fa-setup/start", post(enroll_start))
        .route("/auth/login/2fa-setup", post(enroll_finish))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/auth/change-password", post(change_password))
        .route("/auth/forgot-password", post(forgot_password))
        .route("/auth/reset-password", post(reset_password))
        .route("/auth/invite/{token}", get(invite_info))
        .route("/auth/accept-invite", post(accept_invite))
}

const MAX_FAILED_LOGINS: i64 = 10;
/// Failed sign-ins from one address (any accounts) before it waits: stops trying one
/// password on many accounts ("password spraying"), which the per-account limit cannot see.
const MAX_FAILED_LOGINS_PER_IP: i64 = 100;
const FAILED_LOGIN_WINDOW_SECS: i64 = 15 * 60;
const TWO_FACTOR_CHALLENGE_SECS: u64 = 5 * 60;
const TWO_FACTOR_MAX_ATTEMPTS: i64 = 5;
const RESET_TTL_SECS: u64 = 3600;
const MAX_RESET_REQUESTS_PER_HOUR: i64 = 3;
pub const INVITE_TTL_SECS: u64 = 7 * 24 * 3600;
pub const RESET_SCOPE: &str = "pwreset";
pub const INVITE_SCOPE: &str = "invite";

#[derive(Serialize)]
pub struct Me {
    #[serde(flatten)]
    pub user: users::User,
    pub permissions: Vec<String>,
}

#[derive(Serialize)]
pub struct Session {
    pub access_token: String,
    pub expires_in: i64,
    pub user: Me,
}

pub async fn load_me(state: &AppState, user_id: Uuid) -> AppResult<Me> {
    let mut permissions = load_permissions(state, user_id).await?;
    permissions.sort();
    Ok(Me { user: users::fetch_user(state, user_id).await?, permissions })
}

/// Starts a new session for a device: access token in the body, refresh token as a cookie.
pub async fn start_session(state: &AppState, user_id: Uuid, meta: &ClientMeta) -> AppResult<Response> {
    let (sid, refresh) = tokens::create_session(state, user_id, meta).await?;
    session_response(state, user_id, sid, Some(&refresh)).await
}

async fn session_response(
    state: &AppState,
    user_id: Uuid,
    sid: Uuid,
    refresh: Option<&str>,
) -> AppResult<Response> {
    let (access_token, expires_in) = tokens::issue_access_token(state, user_id, sid)?;
    let body = Json(Session { access_token, expires_in, user: load_me(state, user_id).await? });
    Ok(match refresh {
        Some(r) => (AppendHeaders([(header::SET_COOKIE, tokens::refresh_cookie(state, r))]), body).into_response(),
        None => body.into_response(),
    })
}

pub async fn company_name(state: &AppState) -> String {
    sqlx::query_scalar::<_, Value>("SELECT value FROM settings WHERE key = 'company_name'")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "NERP".into())
}

// ---------------------------------------------------------------------------
// Sign-in
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginRequest {
    email: String,
    password: String,
}

async fn login(
    State(state): State<AppState>,
    meta: ClientMeta,
    Json(body): Json<LoginRequest>,
) -> AppResult<Response> {
    let invalid = || AppError::InvalidCredentials("incorrect email or password".into());
    let email = normalize_email(&body.email).map_err(|_| invalid())?;
    let ip = meta.ip.as_deref();

    let fail_key = format!("auth:fail:{email}");
    let mut redis = state.redis.clone();
    let failures: Option<i64> = redis.get(&fail_key).await?;
    if failures.unwrap_or(0) >= MAX_FAILED_LOGINS {
        return Err(AppError::TooManyRequests("too many failed attempts; try again in 15 minutes".into()));
    }
    let ip_key = format!("auth:failip:{}", ip.unwrap_or("unknown"));
    let ip_failures: Option<i64> = redis.get(&ip_key).await?;
    if ip_failures.unwrap_or(0) >= MAX_FAILED_LOGINS_PER_IP {
        return Err(AppError::TooManyRequests("too many failed sign-ins from your network; try again in 15 minutes".into()));
    }

    let db = &state.db;
    let find = |email: String| async move {
        sqlx::query_as::<_, (Uuid, String, bool, bool, bool, String)>(
            "SELECT id, password_hash, is_active, invite_pending, totp_secret IS NOT NULL, auth_source FROM users WHERE email = $1",
        )
        .bind(email)
        .fetch_optional(db)
        .await
    };
    let mut row = find(email.clone()).await?;

    // Directory accounts (and people in the directory without an account yet) are checked
    // against the directory; everyone else against their NERP password. Unknown emails
    // still verify a hash so they take as long as wrong passwords.
    let directory = match &row {
        Some(r) => r.5 == "ldap",
        None => crate::directory::ldap::enabled(&state).await?,
    };
    let password_ok = if directory {
        let ok = crate::directory::ldap::sign_in(&state, &email, &body.password).await?.is_some();
        if ok {
            row = find(email.clone()).await?;
        }
        ok
    } else {
        let hash = row.as_ref().map(|r| r.1.clone()).unwrap_or_else(|| password::dummy_hash().to_string());
        password::verify_password(body.password, hash).await?
    };
    let row = row.map(|(a, b, c, d, e, _)| (a, b, c, d, e));

    let (user_id, has_2fa) = match row {
        Some((id, _, true, false, two_fa)) if password_ok => (id, two_fa),
        Some((id, _, false, _, _)) if password_ok => {
            audit::record(&state.db, Some(id), "auth.login_blocked", "user", Some(id.to_string()), json!({}), ip).await;
            return Err(AppError::InvalidCredentials("this account has been deactivated".into()));
        }
        other => {
            let _: () = redis::pipe()
                .incr(&fail_key, 1)
                .ignore()
                .expire(&fail_key, FAILED_LOGIN_WINDOW_SECS)
                .ignore()
                .incr(&ip_key, 1)
                .ignore()
                .expire(&ip_key, FAILED_LOGIN_WINDOW_SECS)
                .ignore()
                .query_async(&mut redis)
                .await?;
            audit::record(&state.db, other.map(|r| r.0), "auth.login_failed", "user", None, json!({ "email": email }), ip).await;
            return Err(invalid());
        }
    };
    let _: () = redis.del(&fail_key).await?;

    if let Some(next) = second_step(&state, user_id, has_2fa).await? {
        return Ok(Json(next).into_response());
    }
    complete_login(&state, user_id, &meta, "password").await
}

/// After the password (or the identity provider): a code from the authenticator app if the
/// person has one, or setting one up if the company requires it. None: no second step.
pub async fn second_step(state: &AppState, user_id: Uuid, has_2fa: bool) -> AppResult<Option<Value>> {
    let mut redis = state.redis.clone();
    let challenge = tokens::random_token(24);
    if has_2fa {
        let _: () = redis.set_ex(format!("auth:2fa_challenge:{challenge}"), user_id.to_string(), TWO_FACTOR_CHALLENGE_SECS).await?;
        return Ok(Some(json!({ "two_factor_required": true, "challenge": challenge })));
    }
    if super::settings::two_factor_required(state).await? {
        let _: () = redis.set_ex(format!("auth:2fa_enroll:{challenge}"), user_id.to_string(), ENROLL_SECS).await?;
        return Ok(Some(json!({ "two_factor_setup_required": true, "challenge": challenge })));
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Setting up two-step sign-in while signing in (when the company requires it)
// ---------------------------------------------------------------------------

const ENROLL_SECS: u64 = 15 * 60;

async fn enroll_user(state: &AppState, challenge: &str) -> AppResult<Uuid> {
    let mut redis = state.redis.clone();
    let user: Option<String> = redis.get(format!("auth:2fa_enroll:{challenge}")).await?;
    user.and_then(|u| Uuid::parse_str(&u).ok())
        .ok_or_else(|| AppError::InvalidCredentials("this sign-in attempt has expired; start again".into()))
}

#[derive(Deserialize)]
pub struct EnrollStart {
    challenge: String,
}

/// The secret to add to the authenticator app (as a QR code), and the mobile number on file.
async fn enroll_start(State(state): State<AppState>, Json(b): Json<EnrollStart>) -> AppResult<Json<Value>> {
    let user = enroll_user(&state, &b.challenge).await?;
    let (email, mobile): (String, Option<String>) = sqlx::query_as("SELECT email, mobile FROM users WHERE id = $1 AND is_active")
        .bind(user)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::Unauthorized)?;
    let key = format!("auth:2fa_enroll_secret:{}", b.challenge);
    let mut redis = state.redis.clone();
    let existing: Option<String> = redis.get(&key).await?;
    let secret = match existing {
        Some(s) => s,
        None => {
            let s = two_factor::new_secret();
            let _: () = redis.set_ex(&key, &s, ENROLL_SECS).await?;
            s
        }
    };
    let totp = two_factor::build(&secret, &company_name(&state).await, &email)?;
    let qr = totp.to_qr_base64().map_err(|e| anyhow::anyhow!("QR generation failed: {e:?}"))?;
    let url = totp.to_url().map_err(|e| anyhow::anyhow!("otpauth URL failed: {e:?}"))?;
    Ok(Json(json!({ "secret": secret, "otpauth_url": url, "qr_png_base64": qr, "email": email, "mobile": mobile })))
}

/// A mobile number as people write it: +, digits, spaces, dashes, dots, brackets; 7–15 digits.
fn clean_mobile(m: &str) -> AppResult<String> {
    let m = m.trim();
    let digits = m.chars().filter(|c| c.is_ascii_digit()).count();
    if !(7..=15).contains(&digits) || !m.chars().all(|c| c.is_ascii_digit() || " +-().".contains(c)) || m[1..].contains('+') {
        return Err(AppError::bad("enter your mobile number, e.g. +44 7700 900123"));
    }
    Ok(m.to_string())
}

#[derive(Deserialize)]
pub struct EnrollFinish {
    challenge: String,
    mobile: String,
    code: String,
}

/// Saves the mobile number and the authenticator, and signs in. The recovery codes are in
/// the answer (shown once).
async fn enroll_finish(State(state): State<AppState>, meta: ClientMeta, Json(b): Json<EnrollFinish>) -> AppResult<Response> {
    let user_id = enroll_user(&state, &b.challenge).await?;
    // Deactivated since the password: no way in.
    let active: bool = sqlx::query_scalar("SELECT is_active FROM users WHERE id = $1").bind(user_id).fetch_optional(&state.db).await?.unwrap_or(false);
    if !active {
        return Err(AppError::InvalidCredentials("this account has been deactivated".into()));
    }
    let mobile = clean_mobile(&b.mobile)?;
    let mut redis = state.redis.clone();
    let key = format!("auth:2fa_enroll:{}", b.challenge);
    let secret_key = format!("auth:2fa_enroll_secret:{}", b.challenge);
    let attempts_key = format!("{key}:attempts");
    let secret: Option<String> = redis.get(&secret_key).await?;
    let secret = secret.ok_or_else(|| AppError::bad("scan the QR code first"))?;
    if !two_factor::verify_code(&state, user_id, &secret, &b.code).await? {
        let (attempts,): (i64,) = redis::pipe().incr(&attempts_key, 1).expire(&attempts_key, ENROLL_SECS as i64).ignore().query_async(&mut redis).await?;
        audit::record(&state.db, Some(user_id), "auth.2fa_failed", "user", Some(user_id.to_string()), json!({ "setup": true }), meta.ip.as_deref()).await;
        if attempts >= TWO_FACTOR_MAX_ATTEMPTS {
            let _: () = redis.del(&[&key, &secret_key, &attempts_key]).await?;
            return Err(AppError::InvalidCredentials("too many wrong codes; sign in again".into()));
        }
        return Err(AppError::bad("that code is not valid; check the time on your phone and try again"));
    }
    let _: () = redis.del(&[&key, &secret_key, &attempts_key]).await?;
    sqlx::query("UPDATE users SET totp_secret = $1, totp_enabled_at = now(), mobile = $2, updated_at = now() WHERE id = $3")
        .bind(&secret)
        .bind(&mobile)
        .bind(user_id)
        .execute(&state.db)
        .await?;
    let codes = two_factor::regenerate_recovery_codes(&state.db, user_id).await?;
    invalidate_authz(&state, &[user_id]).await?;
    audit::record(&state.db, Some(user_id), "auth.2fa_enabled", "user", Some(user_id.to_string()), json!({ "at_sign_in": true }), meta.ip.as_deref()).await;

    let res = complete_login(&state, user_id, &meta, "totp_setup").await?;
    // Add the recovery codes to the session in the body.
    let (parts, body) = res.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20).await.map_err(|e| anyhow::anyhow!(e))?;
    let mut v: Value = serde_json::from_slice(&bytes).map_err(|e| anyhow::anyhow!(e))?;
    v["recovery_codes"] = json!(codes);
    let mut out = Json(v).into_response();
    for (k, val) in parts.headers.iter() {
        if k == header::SET_COOKIE {
            out.headers_mut().append(k, val.clone());
        }
    }
    Ok(out)
}

async fn complete_login(state: &AppState, user_id: Uuid, meta: &ClientMeta, method: &str) -> AppResult<Response> {
    sqlx::query("UPDATE users SET last_login_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(&state.db)
        .await?;
    audit::record(
        &state.db,
        Some(user_id),
        "auth.login",
        "user",
        Some(user_id.to_string()),
        json!({ "method": method, "user_agent": meta.user_agent }),
        meta.ip.as_deref(),
    )
    .await;
    start_session(state, user_id, meta).await
}

#[derive(Deserialize)]
pub struct TwoFactorLogin {
    challenge: String,
    /// A 6-digit authenticator code or a recovery code.
    code: String,
}

async fn login_two_factor(
    State(state): State<AppState>,
    meta: ClientMeta,
    Json(body): Json<TwoFactorLogin>,
) -> AppResult<Response> {
    let key = format!("auth:2fa_challenge:{}", body.challenge);
    let attempts_key = format!("{key}:attempts");
    let mut redis = state.redis.clone();
    let user: Option<String> = redis.get(&key).await?;
    let Some(user_id) = user.and_then(|u| Uuid::parse_str(&u).ok()) else {
        return Err(AppError::InvalidCredentials("this sign-in attempt has expired; start again".into()));
    };

    let secret: Option<String> = sqlx::query_scalar("SELECT totp_secret FROM users WHERE id = $1 AND is_active")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?
        .flatten();
    let Some(secret) = secret else {
        return Err(AppError::Unauthorized);
    };

    let code = body.code.trim();
    let digits = code.chars().filter(|c| c.is_ascii_digit()).count();
    let (ok, method) = if digits == 6 && code.len() <= 7 {
        (two_factor::verify_code(&state, user_id, &secret, code).await?, "totp")
    } else {
        (two_factor::use_recovery_code(&state, user_id, code).await?, "recovery_code")
    };

    if !ok {
        let (attempts,): (i64,) = redis::pipe()
            .incr(&attempts_key, 1)
            .expire(&attempts_key, TWO_FACTOR_CHALLENGE_SECS as i64)
            .ignore()
            .query_async(&mut redis)
            .await?;
        audit::record(&state.db, Some(user_id), "auth.2fa_failed", "user", Some(user_id.to_string()), json!({}), meta.ip.as_deref()).await;
        if attempts >= TWO_FACTOR_MAX_ATTEMPTS {
            let _: () = redis.del(&[&key, &attempts_key]).await?;
            return Err(AppError::InvalidCredentials("too many wrong codes; sign in again".into()));
        }
        return Err(AppError::InvalidCredentials("that code is not valid".into()));
    }

    let _: () = redis.del(&[&key, &attempts_key]).await?;
    if method == "recovery_code" {
        let left = two_factor::remaining_recovery_codes(&state, user_id).await?;
        notify(
            &state,
            &[user_id],
            NewNotification {
                kind: "security.recovery_code_used",
                title: "A recovery code was used to sign in".into(),
                body: Some(format!("{left} recovery codes left. Generate new ones from your profile if you are running low.")),
                link: Some("/profile#security".into()),
                actor: None,
            },
        )
        .await;
    }
    complete_login(&state, user_id, &meta, method).await
}

async fn refresh(State(state): State<AppState>, meta: ClientMeta, headers: HeaderMap) -> AppResult<Response> {
    let token = tokens::read_cookie(&headers, tokens::REFRESH_COOKIE).ok_or(AppError::Unauthorized)?;
    let (user_id, sid, new_token) = tokens::rotate_refresh_token(&state, &token, &meta)
        .await?
        .ok_or(AppError::Unauthorized)?;
    let active: Option<bool> = sqlx::query_scalar("SELECT is_active FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;
    if active != Some(true) {
        tokens::revoke_session(&state, sid).await?;
        return Err(AppError::Unauthorized);
    }
    session_response(&state, user_id, sid, Some(&new_token)).await
}

async fn logout(State(state): State<AppState>, headers: HeaderMap, meta: ClientMeta) -> AppResult<Response> {
    // Find the session from the refresh cookie, so logout works even with an expired access token.
    if let Some(token) = tokens::read_cookie(&headers, tokens::REFRESH_COOKIE)
        && let Some((_, sid, _)) = tokens::rotate_refresh_token(&state, &token, &meta).await?
    {
        tokens::revoke_session(&state, sid).await?;
    }
    Ok((
        AppendHeaders([(header::SET_COOKIE, tokens::clear_refresh_cookie(&state))]),
        Json(json!({ "ok": true })),
    )
        .into_response())
}

async fn me(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Me>> {
    Ok(Json(load_me(&state, auth.id).await?))
}

// ---------------------------------------------------------------------------
// Passwords
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ChangePassword {
    current_password: String,
    new_password: String,
}

async fn change_password(
    State(state): State<AppState>,
    auth: AuthUser,
    meta: ClientMeta,
    Json(body): Json<ChangePassword>,
) -> AppResult<Response> {
    if crate::directory::external_source(&state, auth.id).await?.is_some() {
        return Err(AppError::bad("your password is managed by the company directory or sign-on provider"));
    }
    password::validate_password(&body.new_password)?;
    if !verify_user_password(&state, auth.id, body.current_password).await? {
        return Err(AppError::bad("current password is incorrect"));
    }
    set_password(&state, auth.id, body.new_password).await?;
    // Sign out every other device; keep this one with a fresh access token
    // (tokens issued before the change are no longer accepted).
    tokens::revoke_all_sessions(&state, auth.id, Some(auth.session_id)).await?;
    audit::record(&state.db, Some(auth.id), "auth.password_change", "user", Some(auth.id.to_string()), json!({}), meta.ip.as_deref()).await;
    session_response(&state, auth.id, auth.session_id, None).await
}

pub async fn verify_user_password(state: &AppState, user: Uuid, password_plain: String) -> AppResult<bool> {
    let (hash, source, email): (String, String, String) = sqlx::query_as("SELECT password_hash, auth_source, email FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(&state.db)
        .await?;
    if source == "ldap" {
        return Ok(crate::directory::ldap::sign_in(state, &email, &password_plain).await? == Some(user));
    }
    password::verify_password(password_plain, hash).await
}

/// Stores a new password and invalidates the user's cached authorization.
pub async fn set_password(state: &AppState, user: Uuid, new_password: String) -> AppResult<()> {
    let hash = password::hash_password(new_password).await?;
    sqlx::query(
        "UPDATE users SET password_hash = $1, password_changed_at = date_trunc('second', now()),
             updated_at = now() WHERE id = $2",
    )
    .bind(hash)
    .bind(user)
    .execute(&state.db)
    .await?;
    invalidate_authz(state, &[user]).await
}

#[derive(Deserialize)]
pub struct ForgotPassword {
    email: String,
}

/// Always answers the same way, so it cannot be used to discover which emails exist.
async fn forgot_password(
    State(state): State<AppState>,
    meta: ClientMeta,
    Json(body): Json<ForgotPassword>,
) -> AppResult<Json<Value>> {
    let done = Json(json!({ "ok": true }));
    let Ok(email) = normalize_email(&body.email) else {
        return Ok(done);
    };

    let rate_key = format!("auth:forgot:{email}");
    let mut redis = state.redis.clone();
    let (count,): (i64,) = redis::pipe()
        .incr(&rate_key, 1)
        .expire(&rate_key, 3600)
        .ignore()
        .query_async(&mut redis)
        .await?;
    if count > MAX_RESET_REQUESTS_PER_HOUR {
        return Ok(done);
    }

    let user: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, first_name FROM users WHERE email = $1 AND is_active AND NOT invite_pending AND auth_source = 'local'",
    )
    .bind(&email)
    .fetch_optional(&state.db)
    .await?;
    let Some((user_id, first_name)) = user else {
        return Ok(done);
    };

    let token = tokens::issue_link_token(&state, RESET_SCOPE, user_id, &json!({ "user_id": user_id }), RESET_TTL_SECS).await?;
    let link = format!("{}/reset-password?token={token}", state.config.public_url);
    let company = company_name(&state).await;
    jobs::send_email(&state, mail::password_reset(&email, &first_name, &company, &link)).await?;
    audit::record(&state.db, Some(user_id), "auth.password_reset_requested", "user", Some(user_id.to_string()), json!({}), meta.ip.as_deref()).await;
    Ok(done)
}

#[derive(Deserialize)]
pub struct ResetPassword {
    token: String,
    password: String,
}

async fn reset_password(
    State(state): State<AppState>,
    meta: ClientMeta,
    Json(body): Json<ResetPassword>,
) -> AppResult<Json<Value>> {
    password::validate_password(&body.password)?;
    let data = tokens::consume_link_token(&state, RESET_SCOPE, &body.token)
        .await?
        .ok_or_else(|| AppError::bad("this reset link is invalid or has expired; request a new one"))?;
    let user_id: Uuid = serde_json::from_value(data["user_id"].clone()).map_err(|e| anyhow::anyhow!(e))?;
    tokens::revoke_link_tokens(&state, RESET_SCOPE, user_id).await?;

    set_password(&state, user_id, body.password).await?;
    tokens::revoke_all_sessions(&state, user_id, None).await?;
    audit::record(&state.db, Some(user_id), "auth.password_reset", "user", Some(user_id.to_string()), json!({}), meta.ip.as_deref()).await;
    notify(
        &state,
        &[user_id],
        NewNotification {
            kind: "security.password_reset",
            title: "Your password was reset".into(),
            body: Some("If this wasn't you, contact your administrator straight away.".into()),
            link: Some("/profile#security".into()),
            actor: None,
        },
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Invitations
// ---------------------------------------------------------------------------

/// Creates (or replaces) the invitation link for a pending user and emails it.
pub async fn send_invitation(state: &AppState, user_id: Uuid, inviter: Uuid) -> AppResult<()> {
    let (email, first_name): (String, String) =
        sqlx::query_as("SELECT email, first_name FROM users WHERE id = $1 AND invite_pending")
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::bad("this employee has already accepted their invitation"))?;
    let inviter_name: String = sqlx::query_scalar("SELECT first_name || ' ' || last_name FROM users WHERE id = $1")
        .bind(inviter)
        .fetch_one(&state.db)
        .await?;
    let token = tokens::issue_link_token(
        state,
        INVITE_SCOPE,
        user_id,
        &json!({ "user_id": user_id, "invited_by": inviter }),
        INVITE_TTL_SECS,
    )
    .await?;
    sqlx::query("UPDATE users SET invited_at = now() WHERE id = $1").bind(user_id).execute(&state.db).await?;
    let link = format!("{}/accept-invite?token={token}", state.config.public_url);
    let company = company_name(state).await;
    jobs::send_email(state, mail::invitation(&email, &first_name, &company, &inviter_name, &link)).await?;
    Ok(())
}

async fn invite_info(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let data = tokens::peek_link_token(&state, INVITE_SCOPE, &token)
        .await?
        .ok_or_else(|| AppError::bad("this invitation is invalid or has expired; ask for a new one"))?;
    let user_id: Uuid = serde_json::from_value(data["user_id"].clone()).map_err(|e| anyhow::anyhow!(e))?;
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT email, first_name, last_name FROM users WHERE id = $1 AND invite_pending AND is_active",
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    let (email, first_name, last_name) = row.ok_or_else(|| AppError::bad("this invitation is no longer valid"))?;
    Ok(Json(json!({
        "email": email,
        "first_name": first_name,
        "last_name": last_name,
        "company_name": company_name(&state).await,
    })))
}

#[derive(Deserialize)]
pub struct AcceptInvite {
    token: String,
    password: String,
}

async fn accept_invite(
    State(state): State<AppState>,
    meta: ClientMeta,
    Json(body): Json<AcceptInvite>,
) -> AppResult<Response> {
    password::validate_password(&body.password)?;
    let data = tokens::consume_link_token(&state, INVITE_SCOPE, &body.token)
        .await?
        .ok_or_else(|| AppError::bad("this invitation is invalid or has expired; ask for a new one"))?;
    let user_id: Uuid = serde_json::from_value(data["user_id"].clone()).map_err(|e| anyhow::anyhow!(e))?;
    let invited_by: Option<Uuid> = serde_json::from_value(data["invited_by"].clone()).ok();
    tokens::revoke_link_tokens(&state, INVITE_SCOPE, user_id).await?;

    let hash = password::hash_password(body.password).await?;
    let updated = sqlx::query(
        "UPDATE users SET password_hash = $1, invite_pending = FALSE, password_changed_at = date_trunc('second', now()),
             last_login_at = now(), updated_at = now()
         WHERE id = $2 AND invite_pending AND is_active",
    )
    .bind(hash)
    .bind(user_id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(AppError::bad("this invitation is no longer valid"));
    }
    invalidate_authz(&state, &[user_id]).await?;
    audit::record(&state.db, Some(user_id), "auth.invite_accepted", "user", Some(user_id.to_string()), json!({}), meta.ip.as_deref()).await;

    if let Some(inviter) = invited_by {
        let name: String = sqlx::query_scalar("SELECT first_name || ' ' || last_name FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&state.db)
            .await?;
        notify(
            &state,
            &[inviter],
            NewNotification {
                kind: "user.invite_accepted",
                title: format!("{name} accepted your invitation"),
                body: None,
                link: Some(format!("/employees/{user_id}")),
                actor: Some(user_id),
            },
        )
        .await;
    }
    start_session(&state, user_id, &meta).await
}
