//! Single sign-on with an OpenID Connect provider (Microsoft Entra ID, Google Workspace,
//! Okta, Keycloak, Authentik, …): authorisation-code flow with PKCE and a nonce.
//!
//! The ID token comes straight from the provider's token endpoint over TLS, so its claims
//! are checked (issuer, audience, nonce, expiry) without verifying the signature again.

use axum::{
    extract::{Query, State},
    http::header,
    response::{AppendHeaders, IntoResponse, Redirect, Response},
    Json,
};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{bad, load, SecretUpdate};
use crate::{
    audit,
    auth::{
        permissions as perm,
        tokens::{self, random_token, ClientMeta},
        AuthUser,
    },
    crm::email::oauth::{http, id_claims, pkce_challenge, url_encode},
    error::AppResult,
    routes::normalize_email,
    state::AppState,
};

const FLOW_TTL_SECS: u64 = 600;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct SsoConfig {
    pub enabled: bool,
    /// The provider's issuer URL; its /.well-known/openid-configuration is read.
    pub issuer: String,
    pub client_id: String,
    pub button_label: String,
    /// Only these email domains may sign in (empty = any).
    pub allowed_domains: Vec<String>,
    /// Create an account for someone who signs in for the first time.
    pub auto_create: bool,
}

impl Default for SsoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            issuer: String::new(),
            client_id: String::new(),
            button_label: "Sign in with company account".into(),
            allowed_domains: Vec::new(),
            auto_create: false,
        }
    }
}

pub fn redirect_uri(state: &AppState) -> String {
    format!("{}/api/auth/sso/callback", state.config.public_url)
}

/// Plain http is only for a provider on this machine (development and tests).
fn issuer_ok(issuer: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(issuer) else { return false };
    match u.scheme() {
        "https" => true,
        "http" => matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        _ => false,
    }
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    userinfo_endpoint: Option<String>,
    #[serde(default)]
    token_endpoint_auth_methods_supported: Vec<String>,
}

async fn discover(issuer: &str) -> Result<Discovery, String> {
    let url = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));
    let res = http()?.get(&url).send().await.map_err(|e| format!("cannot reach the identity provider: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("the identity provider's configuration was not found ({})", res.status()));
    }
    let d: Discovery = res.json().await.map_err(|e| format!("the identity provider's configuration is not valid: {e}"))?;
    if d.issuer.trim_end_matches('/') != issuer.trim_end_matches('/') {
        return Err(format!("the provider calls itself {} rather than {issuer}", d.issuer));
    }
    for u in [&d.authorization_endpoint, &d.token_endpoint] {
        if !issuer_ok(u) {
            return Err("the provider's endpoints must use https".into());
        }
    }
    Ok(d)
}

/// For other providers: (authorization, token, userinfo) endpoints of an issuer.
pub(crate) async fn discover_pub(issuer: &str) -> Result<(String, String, Option<String>), String> {
    if !issuer_ok(issuer) {
        return Err("the issuer must be an https:// address".into());
    }
    let d = discover(issuer).await?;
    Ok((d.authorization_endpoint, d.token_endpoint, d.userinfo_endpoint))
}

pub(crate) fn issuer_ok_pub(issuer: &str) -> bool {
    issuer_ok(issuer)
}

// ---------------------------------------------------------------------------
// Sign-in
// ---------------------------------------------------------------------------

/// Public: whether the login page shows the button.
pub async fn public_info(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let s = load(&state).await?;
    let on = s.sso.enabled && s.sso_secret.is_some();
    Ok(Json(json!({ "enabled": on, "label": if on { Some(s.sso.button_label) } else { None }, "saml": super::saml::public_info(&state).await })))
}

fn to_login(error: &str) -> Response {
    Redirect::to(&format!("/login?{}", url_encode(&[("sso_error", error)]))).into_response()
}

pub async fn start(State(state): State<AppState>) -> Response {
    match begin(&state).await {
        Ok(url) => Redirect::to(&url).into_response(),
        Err(e) => to_login(&e),
    }
}

async fn begin(state: &AppState) -> Result<String, String> {
    let s = load(state).await.map_err(|e| e.to_string())?;
    if !s.sso.enabled || s.sso_secret.is_none() {
        return Err("single sign-on is not set up".into());
    }
    let d = discover(&s.sso.issuer).await?;
    let (flow, verifier, nonce) = (random_token(24), random_token(48), random_token(24));
    let mut redis = state.redis.clone();
    let _: () = redis
        .set_ex(format!("auth:sso:{flow}"), json!({ "verifier": verifier, "nonce": nonce }).to_string(), FLOW_TTL_SECS)
        .await
        .map_err(|e| e.to_string())?;
    let q = url_encode(&[
        ("client_id", s.sso.client_id.as_str()),
        ("response_type", "code"),
        ("redirect_uri", &redirect_uri(state)),
        ("scope", "openid email profile"),
        ("state", &flow),
        ("nonce", &nonce),
        ("code_challenge", &pkce_challenge(&verifier)),
        ("code_challenge_method", "S256"),
    ]);
    let sep = if d.authorization_endpoint.contains('?') { '&' } else { '?' };
    Ok(format!("{}{sep}{q}", d.authorization_endpoint))
}

#[derive(Deserialize)]
pub struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// The provider sends the browser here. On success the refresh cookie is set and the app
/// opens; it then gets an access token with /auth/refresh as on any page load.
pub async fn callback(State(state): State<AppState>, meta: ClientMeta, Query(q): Query<Callback>) -> Response {
    if let Some(e) = q.error {
        return to_login(&q.error_description.unwrap_or(e));
    }
    match finish(&state, &meta, q.code.unwrap_or_default(), q.state.unwrap_or_default()).await {
        Ok(user) => complete_sign_in(&state, &meta, user).await,
        Err(e) => {
            audit::record(&state.db, None, "auth.sso_failed", "user", None, json!({ "error": e }), meta.ip.as_deref()).await;
            to_login(&e)
        }
    }
}

/// After an outside sign-in (OpenID Connect, SAML): a code from the authenticator app (or
/// setting one up) may still come next, which the login page takes from here; otherwise the
/// refresh cookie is set and the app opens.
pub(crate) async fn complete_sign_in(state: &AppState, meta: &ClientMeta, user: Uuid) -> Response {
    match second_step(state, user).await {
        Ok(Some(next)) => {
            let step = if next["two_factor_required"] == json!(true) { "code" } else { "setup" };
            Redirect::to(&format!("/login?{}", url_encode(&[("step", step), ("challenge", next["challenge"].as_str().unwrap_or_default())]))).into_response()
        }
        Ok(None) => match tokens::create_session(state, user, meta).await {
            Ok((_, refresh)) => (AppendHeaders([(header::SET_COOKIE, tokens::refresh_cookie(state, &refresh))]), Redirect::to("/")).into_response(),
            Err(e) => to_login(&e.to_string()),
        },
        Err(e) => to_login(&e.to_string()),
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
    access_token: Option<String>,
}

async fn second_step(state: &AppState, user: Uuid) -> AppResult<Option<Value>> {
    let has_2fa: bool = sqlx::query_scalar("SELECT totp_secret IS NOT NULL FROM users WHERE id = $1").bind(user).fetch_one(&state.db).await?;
    crate::routes::auth::second_step(state, user, has_2fa).await
}

async fn finish(state: &AppState, meta: &ClientMeta, code: String, flow: String) -> Result<Uuid, String> {
    if code.is_empty() || flow.is_empty() {
        return Err("the sign-in was not completed".into());
    }
    let mut redis = state.redis.clone();
    let saved: Option<String> = redis.get_del(format!("auth:sso:{flow}")).await.map_err(|e| e.to_string())?;
    let saved: Value = serde_json::from_str(&saved.ok_or("the sign-in took too long or was already used; try again")?).map_err(|e| e.to_string())?;
    let s = load(state).await.map_err(|e| e.to_string())?;
    let secret = s.sso_secret.as_deref().filter(|_| s.sso.enabled).ok_or("single sign-on is not set up")?;
    let d = discover(&s.sso.issuer).await?;

    let redirect = redirect_uri(state);
    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("code_verifier", saved["verifier"].as_str().unwrap_or_default()),
    ];
    // client_secret_basic is the default; use the form only when that is all the provider takes.
    let basic = d.token_endpoint_auth_methods_supported.is_empty() || d.token_endpoint_auth_methods_supported.iter().any(|m| m == "client_secret_basic");
    let mut req = http()?.post(&d.token_endpoint);
    if basic {
        req = req.basic_auth(url_encode(&[("", &s.sso.client_id)])[1..].to_string(), Some(url_encode(&[("", secret)])[1..].to_string()));
    } else {
        form.push(("client_id", s.sso.client_id.as_str()));
        form.push(("client_secret", secret));
    }
    let res = req.form(&form).send().await.map_err(|e| format!("cannot reach the identity provider: {e}"))?;
    let status = res.status();
    let text = res.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let e: Value = serde_json::from_str(&text).unwrap_or_default();
        return Err(format!(
            "the identity provider refused the sign-in: {}",
            e["error_description"].as_str().or(e["error"].as_str()).unwrap_or(status.as_str())
        ));
    }
    let t: TokenResponse = serde_json::from_str(&text).map_err(|e| format!("unexpected answer from the identity provider: {e}"))?;
    let claims = t.id_token.as_deref().and_then(id_claims).ok_or("the identity provider sent no ID token")?;

    // The ID token must be for us, from this provider, for this sign-in, and current.
    if claims["iss"].as_str().map(|i| i.trim_end_matches('/')) != Some(s.sso.issuer.trim_end_matches('/')) {
        return Err("the ID token is from a different issuer".into());
    }
    let aud_ok = match &claims["aud"] {
        Value::String(a) => a == &s.sso.client_id,
        Value::Array(a) => a.iter().any(|v| v.as_str() == Some(&s.sso.client_id)),
        _ => false,
    };
    if !aud_ok {
        return Err("the ID token is for a different application".into());
    }
    if claims["nonce"].as_str() != saved["nonce"].as_str() {
        return Err("the ID token does not belong to this sign-in".into());
    }
    if claims["exp"].as_i64().is_none_or(|e| e < chrono::Utc::now().timestamp() - 60) {
        return Err("the ID token has expired".into());
    }
    let sub = claims["sub"].as_str().filter(|s| !s.is_empty()).ok_or("the ID token names no one")?.to_string();

    // Email and name from the token, else from the userinfo endpoint.
    let mut info = claims.clone();
    if info["email"].as_str().is_none()
        && let (Some(u), Some(at)) = (&d.userinfo_endpoint, &t.access_token) {
            let ui: Value = http()?.get(u).bearer_auth(at).send().await.map_err(|e| e.to_string())?.json().await.unwrap_or_default();
            if ui["sub"].as_str() == Some(sub.as_str()) {
                info = ui;
            }
        }
    if info["email_verified"] == json!(false) || info["email_verified"] == json!("false") {
        return Err("your email address is not verified with the identity provider".into());
    }
    let email = info["email"]
        .as_str()
        .or(info["preferred_username"].as_str().filter(|u| u.contains('@')))
        .and_then(|e| normalize_email(e).ok())
        .ok_or("the identity provider did not share your email address")?;
    let domain = email.rsplit('@').next().unwrap_or_default();
    if !s.sso.allowed_domains.is_empty() && !s.sso.allowed_domains.iter().any(|d| d.eq_ignore_ascii_case(domain)) {
        return Err(format!("accounts from {domain} cannot sign in here"));
    }

    let found: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT id, is_active FROM users WHERE (auth_source = 'sso' AND external_id = $1) OR email = $2
         ORDER BY (auth_source = 'sso' AND external_id = $1) DESC LIMIT 1",
    )
    .bind(&sub)
    .bind(&email)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| e.to_string())?;
    let user = match found {
        Some((_, false)) => return Err("this account has been deactivated".into()),
        Some((id, true)) => {
            // Signing in through the provider accepts a pending invitation.
            sqlx::query("UPDATE users SET invite_pending = FALSE, last_login_at = now() WHERE id = $1").bind(id).execute(&state.db).await.map_err(|e| e.to_string())?;
            id
        }
        None if s.sso.auto_create => match create(state, &sub, &email, &info).await {
            Ok(id) => id,
            // Made by another sign-in at the same moment.
            Err(crate::error::AppError::Database(e)) if crate::error::is_unique_violation(&e) => sqlx::query_scalar(
                "SELECT id FROM users WHERE (auth_source = 'sso' AND external_id = $1) OR email = $2 LIMIT 1",
            )
            .bind(&sub)
            .bind(&email)
            .fetch_one(&state.db)
            .await
            .map_err(|e| e.to_string())?,
            Err(e) => return Err(e.to_string()),
        },
        None => return Err(format!("there is no employee account for {email}; ask an administrator to add you")),
    };
    audit::record(&state.db, Some(user), "auth.login", "user", Some(user.to_string()), json!({ "method": "sso", "user_agent": meta.user_agent }), meta.ip.as_deref()).await;
    Ok(user)
}

async fn create(state: &AppState, sub: &str, email: &str, info: &Value) -> AppResult<Uuid> {
    let text = |k: &str| info[k].as_str().map(|s| s.trim().chars().take(100).collect::<String>()).filter(|s| !s.is_empty());
    let name = text("name");
    let first = text("given_name")
        .or_else(|| name.as_ref().and_then(|n| n.split_whitespace().next().map(String::from)))
        .unwrap_or_else(|| email.split('@').next().unwrap_or_default().to_string());
    let last = text("family_name")
        .or_else(|| name.as_ref().map(|n| n.split_whitespace().skip(1).collect::<Vec<_>>().join(" ")).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "-".into());
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash, first_name, last_name, auth_source, external_id, last_login_at)
         VALUES ($1, '!sso', $2, $3, 'sso', $4, now()) RETURNING id",
    )
    .bind(email)
    .bind(first)
    .bind(last)
    .bind(sub)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO user_roles (user_id, role_id) SELECT $1, id FROM roles WHERE code = 'employee'").bind(id).execute(&mut *tx).await?;
    audit::record(&mut *tx, None, "user.create", "user", Some(id.to_string()), json!({ "email": email, "source": "sso" }), None).await;
    tx.commit().await?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SsoUpdate {
    #[serde(flatten)]
    cfg: SsoConfig,
    #[serde(default)]
    client_secret: SecretUpdate,
}

pub async fn put_config(State(state): State<AppState>, auth: AuthUser, Json(mut b): Json<SsoUpdate>) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    let c = &mut b.cfg;
    c.issuer = c.issuer.trim().trim_end_matches('/').to_string();
    c.client_id = c.client_id.trim().to_string();
    c.button_label = c.button_label.trim().chars().take(60).collect();
    if c.button_label.is_empty() {
        c.button_label = SsoConfig::default().button_label;
    }
    c.allowed_domains = c
        .allowed_domains
        .iter()
        .map(|d| d.trim().trim_start_matches('@').to_lowercase())
        .filter(|d| !d.is_empty())
        .collect();
    if c.allowed_domains.iter().any(|d| !d.contains('.') || d.contains(char::is_whitespace)) {
        return Err(bad("allowed domains look like example.com"));
    }
    if !c.issuer.is_empty() && !issuer_ok(&c.issuer) {
        return Err(bad("the issuer must be an https:// address"));
    }
    if c.issuer.len() > 500 || c.client_id.len() > 500 {
        return Err(bad("a setting is too long"));
    }
    let secret = b.client_secret.sealed(&state);
    let has_secret = match &secret {
        Some(s) => s.is_some(),
        None => load(&state).await?.sso_secret.is_some(),
    };
    if c.enabled && (c.issuer.is_empty() || c.client_id.is_empty() || !has_secret) {
        return Err(bad("set the issuer, client ID and client secret before turning sign-on on"));
    }
    sqlx::query("UPDATE directory_config SET sso = $1, sso_secret = CASE WHEN $2 THEN $3 ELSE sso_secret END WHERE id = 1")
        .bind(serde_json::to_value(&b.cfg).map_err(anyhow::Error::from)?)
        .bind(secret.is_some())
        .bind(secret.flatten())
        .execute(&state.db)
        .await?;
    audit::record(&state.db, Some(auth.id), "directory.sso_update", "settings", None, json!({ "enabled": b.cfg.enabled, "issuer": b.cfg.issuer }), None).await;
    super::get_config(State(state), auth).await
}

/// Reads the provider's configuration to check the issuer.
pub async fn test(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    let s = load(&state).await?;
    if s.sso.issuer.is_empty() {
        return Err(bad("set the issuer first"));
    }
    Ok(Json(match discover(&s.sso.issuer).await {
        Ok(d) => json!({ "ok": true, "authorization_endpoint": d.authorization_endpoint }),
        Err(e) => json!({ "ok": false, "error": e }),
    }))
}
