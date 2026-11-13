//! Signing in with other accounts: Google, Microsoft, Apple, Facebook, X, or any OpenID
//! Connect provider, each a button on the login page.
//!
//! Only existing, active employees sign in this way. The first time, NERP matches the account
//! by an email address the provider has verified (Google, Apple, OpenID Connect providers
//! saying `email_verified`, Microsoft personal accounts, Facebook and X, which share only
//! confirmed addresses); otherwise the person links the account from their profile while
//! signed in. After that the account's own id is what counts, whatever its email.
//!
//! Tokens come straight from the provider's token endpoint over TLS, so ID token claims are
//! checked (issuer, audience, nonce, expiry) without verifying the signature again. The
//! callback takes GET and POST (Apple posts its answer).

use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{AppendHeaders, IntoResponse, Redirect, Response},
    Form, Json,
};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::bad;
use crate::{
    audit,
    auth::{
        permissions as perm,
        tokens::{self, random_token, ClientMeta},
        AuthUser,
    },
    crm::email::oauth::{http, id_claims, pkce_challenge, url_encode},
    error::{AppError, AppResult},
    routes::normalize_email,
    state::AppState,
};

const FLOW_TTL_SECS: u64 = 600;
const KINDS: &[&str] = &["google", "microsoft", "apple", "facebook", "x", "oidc"];
/// Microsoft's tenant for personal accounts (whose email is the account itself).
const MS_PERSONAL_TENANT: &str = "9188040d-6c67-4c5b-b112-36a304b66dad";

#[derive(sqlx::FromRow, Clone)]
pub struct Provider {
    pub id: Uuid,
    pub kind: String,
    pub label: String,
    pub client_id: String,
    pub secret: Option<Vec<u8>>,
    pub issuer: Option<String>,
    pub team_id: Option<String>,
    pub key_id: Option<String>,
    pub enabled: bool,
}
const COLS: &str = "id, kind, label, client_id, secret, issuer, team_id, key_id, enabled";

pub fn redirect_uri(state: &AppState) -> String {
    format!("{}/api/auth/providers/callback", state.config.public_url)
}

/// A provider's address; tests point every provider at a stand-in with NERP_TEST_SIGNIN_BASE
/// (`https://graph.facebook.com/x` becomes `<base>/graph.facebook.com/x`).
fn at(url: &str) -> String {
    match std::env::var("NERP_TEST_SIGNIN_BASE") {
        Ok(base) if !base.is_empty() => format!("{}/{}", base.trim_end_matches('/'), url.trim_start_matches("https://")),
        _ => url.to_string(),
    }
}

struct Endpoints {
    authorize: String,
    token: String,
    /// The ID token's expected issuer (None: no ID token; Microsoft: checked by pattern).
    issuer: Option<String>,
    userinfo: Option<String>,
    scope: &'static str,
}

async fn endpoints(p: &Provider) -> Result<Endpoints, String> {
    Ok(match p.kind.as_str() {
        "google" | "oidc" => {
            let issuer = if p.kind == "google" { at("https://accounts.google.com") } else { p.issuer.clone().unwrap_or_default() };
            let d = super::sso::discover_pub(&issuer).await?;
            Endpoints { authorize: d.0, token: d.1, issuer: Some(issuer), userinfo: d.2, scope: "openid email profile" }
        }
        "microsoft" => Endpoints {
            authorize: at("https://login.microsoftonline.com/common/oauth2/v2.0/authorize"),
            token: at("https://login.microsoftonline.com/common/oauth2/v2.0/token"),
            issuer: None,
            userinfo: None,
            scope: "openid email profile",
        },
        "apple" => Endpoints { authorize: at("https://appleid.apple.com/auth/authorize"), token: at("https://appleid.apple.com/auth/token"), issuer: Some(at("https://appleid.apple.com")), userinfo: None, scope: "name email" },
        "facebook" => Endpoints {
            authorize: at("https://www.facebook.com/v23.0/dialog/oauth"),
            token: at("https://graph.facebook.com/v23.0/oauth/access_token"),
            issuer: None,
            userinfo: Some(at("https://graph.facebook.com/v23.0/me?fields=id,name,first_name,last_name,email")),
            scope: "email public_profile",
        },
        "x" => Endpoints {
            authorize: at("https://x.com/i/oauth2/authorize"),
            token: at("https://api.x.com/2/oauth2/token"),
            issuer: None,
            userinfo: Some(at("https://api.x.com/2/users/me?user.fields=confirmed_email")),
            scope: "users.read tweet.read users.email",
        },
        k => return Err(format!("unknown provider kind “{k}”")),
    })
}

async fn provider(state: &AppState, id: Uuid) -> AppResult<Provider> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM sign_in_providers WHERE id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("provider"))
}

fn secret_of(state: &AppState, p: &Provider) -> Option<String> {
    p.secret.as_deref().and_then(|s| crate::crypto::open(&state.config.data_key, s))
}

/// Apple's client secret: a short-lived JWT signed with the team's private key.
fn apple_secret(p: &Provider, pem: &str) -> Result<String, String> {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    let now = chrono::Utc::now().timestamp();
    let mut h = Header::new(Algorithm::ES256);
    h.kid = p.key_id.clone();
    let claims = json!({ "iss": p.team_id, "iat": now, "exp": now + 300, "aud": "https://appleid.apple.com", "sub": p.client_id });
    let key = EncodingKey::from_ec_pem(pem.as_bytes()).map_err(|_| "the Apple private key is not valid (paste the .p8 file's text)".to_string())?;
    encode(&h, &claims, &key).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Sign-in
// ---------------------------------------------------------------------------

/// Public: the login page's buttons.
pub async fn public_list(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let rows: Vec<(Uuid, String, String)> = sqlx::query!(r#"SELECT id, kind, label FROM sign_in_providers WHERE enabled AND secret IS NOT NULL ORDER BY position, created_at"#).map(|r| (r.id, r.kind, r.label)).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, k, l)| json!({ "id": id, "kind": k, "label": l })).collect::<Vec<_>>() })))
}

fn to_login(error: &str) -> Response {
    Redirect::to(&format!("/login?{}", url_encode(&[("sso_error", error)]))).into_response()
}

/// The address to send the browser to; `link` ties the result to someone signed in.
async fn begin(state: &AppState, id: Uuid, link: Option<Uuid>) -> Result<String, String> {
    let p = provider(state, id).await.map_err(|e| e.to_string())?;
    if !p.enabled || p.secret.is_none() {
        return Err("this sign-in is not available".into());
    }
    let e = endpoints(&p).await?;
    let (flow, verifier, nonce) = (random_token(24), random_token(48), random_token(24));
    let mut redis = state.redis.clone();
    let _: () = redis
        .set_ex(format!("auth:social:{flow}"), json!({ "provider": p.id, "verifier": verifier, "nonce": nonce, "link": link }).to_string(), FLOW_TTL_SECS)
        .await
        .map_err(|e| e.to_string())?;
    let redirect = redirect_uri(state);
    let challenge = pkce_challenge(&verifier);
    let mut q: Vec<(&str, &str)> = vec![("client_id", &p.client_id), ("response_type", "code"), ("redirect_uri", &redirect), ("scope", e.scope), ("state", &flow)];
    match p.kind.as_str() {
        "apple" => {
            q.push(("response_mode", "form_post"));
            q.push(("nonce", &nonce));
        }
        "facebook" => {}
        "x" => {
            q.push(("code_challenge", &challenge));
            q.push(("code_challenge_method", "S256"));
        }
        _ => {
            q.push(("nonce", &nonce));
            q.push(("code_challenge", &challenge));
            q.push(("code_challenge_method", "S256"));
        }
    }
    let sep = if e.authorize.contains('?') { '&' } else { '?' };
    Ok(format!("{}{sep}{}", e.authorize, url_encode(&q)))
}

pub async fn start(State(state): State<AppState>, Path(id): Path<Uuid>) -> Response {
    match begin(&state, id, None).await {
        Ok(url) => Redirect::to(&url).into_response(),
        Err(e) => to_login(&e),
    }
}

/// Signed in: link an outside account to oneself (the browser is then sent to the address).
pub async fn link_start(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let url = begin(&state, id, Some(auth.id)).await.map_err(bad)?;
    Ok(Json(json!({ "url": url })))
}

#[derive(Deserialize, Default)]
pub struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
    /// Apple, the first time: {"name": {"firstName", "lastName"}, "email"}.
    user: Option<String>,
}

pub async fn callback_get(State(state): State<AppState>, meta: ClientMeta, Query(q): Query<Callback>) -> Response {
    callback(state, meta, q).await
}

pub async fn callback_post(State(state): State<AppState>, meta: ClientMeta, Form(q): Form<Callback>) -> Response {
    callback(state, meta, q).await
}

async fn callback(state: AppState, meta: ClientMeta, q: Callback) -> Response {
    if let Some(e) = q.error {
        return to_login(&q.error_description.unwrap_or(e));
    }
    let mut redis = state.redis.clone();
    let saved: Option<String> = redis.get_del(format!("auth:social:{}", q.state.as_deref().unwrap_or(""))).await.unwrap_or(None);
    let Some(saved) = saved.and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
        return to_login("the sign-in took too long or was already used; try again");
    };
    let link: Option<Uuid> = saved["link"].as_str().and_then(|s| s.parse().ok());
    let outcome = async {
        let pid: Uuid = saved["provider"].as_str().and_then(|s| s.parse().ok()).ok_or("the sign-in was not completed")?;
        let p = provider(&state, pid).await.map_err(|e| e.to_string())?;
        let who = identify(&state, &p, q.code.as_deref().unwrap_or(""), &saved, q.user.as_deref()).await?;
        let user = match link {
            Some(u) => {
                sqlx::query!("INSERT INTO user_identities (provider_id, subject, user_id, email) VALUES ($1, $2, $3, $4)
                     ON CONFLICT (provider_id, subject) DO UPDATE SET user_id = $3, email = $4 WHERE user_identities.user_id = $3", p.id, &who.subject, u, who.email.as_deref())
                .execute(&state.db)
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| if r.rows_affected() == 0 { Err("that account is already linked to someone else".to_string()) } else { Ok(()) })?;
                audit::record(&state.db, Some(u), "auth.identity_linked", "user", Some(u.to_string()), json!({ "provider": p.label }), meta.ip.as_deref()).await;
                return Ok((u, true));
            }
            None => find(&state, &p, &who).await?,
        };
        Ok::<_, String>((user, false))
    }
    .await;
    match outcome {
        Ok((_, true)) => Redirect::to("/profile?linked=1").into_response(),
        Ok((user, false)) => {
            audit::record(&state.db, Some(user), "auth.login", "user", Some(user.to_string()), json!({ "method": "provider", "user_agent": meta.user_agent }), meta.ip.as_deref()).await;
            if let Err(e) = crate::auth::ip::check_user(&state, user, meta.ip.as_deref()).await {
                return to_login(&e.to_string());
            }
            let has_2fa: bool = sqlx::query_scalar!(r#"SELECT totp_secret IS NOT NULL AS "v!" FROM users WHERE id = $1"#, user).fetch_one(&state.db).await.unwrap_or(false);
            match crate::routes::auth::second_step(&state, user, has_2fa).await {
                Ok(Some(next)) => {
                    let step = if next["two_factor_required"] == json!(true) { "code" } else { "setup" };
                    Redirect::to(&format!("/login?{}", url_encode(&[("step", step), ("challenge", next["challenge"].as_str().unwrap_or_default())]))).into_response()
                }
                Ok(None) => match tokens::create_session(&state, user, &meta).await {
                    Ok((_, refresh)) => (AppendHeaders([(header::SET_COOKIE, tokens::refresh_cookie(&state, &refresh))]), Redirect::to("/")).into_response(),
                    Err(e) => to_login(&e.to_string()),
                },
                Err(e) => to_login(&e.to_string()),
            }
        }
        Err(e) => {
            audit::record(&state.db, link, "auth.provider_failed", "user", None, json!({ "error": e }), meta.ip.as_deref()).await;
            if link.is_some() {
                Redirect::to(&format!("/profile?{}", url_encode(&[("link_error", &e)]))).into_response()
            } else {
                to_login(&e)
            }
        }
    }
}

/// Who the provider says this is.
struct Who {
    subject: String,
    email: Option<String>,
    /// The provider vouches for the email address.
    verified: bool,
}

async fn identify(state: &AppState, p: &Provider, code: &str, saved: &Value, apple_user: Option<&str>) -> Result<Who, String> {
    if code.is_empty() {
        return Err("the sign-in was not completed".into());
    }
    let e = endpoints(p).await?;
    let secret = match p.kind.as_str() {
        "apple" => apple_secret(p, &secret_of(state, p).ok_or("the provider has no key")?)?,
        _ => secret_of(state, p).ok_or("the provider has no secret")?,
    };
    let redirect = redirect_uri(state);
    let verifier = saved["verifier"].as_str().unwrap_or_default();
    let client = http()?;
    let res = match p.kind.as_str() {
        "facebook" => {
            let sep = if e.token.contains('?') { '&' } else { '?' };
            client.get(format!("{}{sep}{}", e.token, url_encode(&[("client_id", &p.client_id), ("client_secret", &secret), ("redirect_uri", &redirect), ("code", code)]))).send().await
        }
        "x" => client.post(&e.token).basic_auth(&p.client_id, Some(&secret)).form(&[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", &redirect), ("code_verifier", verifier)]).send().await,
        "apple" => client.post(&e.token).form(&[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", &redirect), ("client_id", &p.client_id), ("client_secret", &secret)]).send().await,
        _ => {
            client
                .post(&e.token)
                .form(&[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", &redirect), ("code_verifier", verifier), ("client_id", &p.client_id), ("client_secret", &secret)])
                .send()
                .await
        }
    }
    .map_err(|e| format!("cannot reach {}: {e}", p.label))?;
    let status = res.status();
    let t: Value = res.json().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("{} refused the sign-in: {}", p.label, t["error_description"].as_str().or(t["error"]["message"].as_str()).or(t["error"].as_str()).unwrap_or(status.as_str())));
    }

    // Accounts with an ID token (OpenID Connect, Google, Microsoft, Apple).
    if let Some(claims) = t["id_token"].as_str().and_then(id_claims) {
        let iss = claims["iss"].as_str().unwrap_or_default().trim_end_matches('/');
        let iss_ok = match (&e.issuer, p.kind.as_str()) {
            (Some(want), _) => iss == want.trim_end_matches('/'),
            (None, "microsoft") => claims["tid"].as_str().is_some_and(|tid| iss.ends_with(&format!("/{tid}/v2.0"))),
            _ => false,
        };
        if !iss_ok {
            return Err("the ID token is from a different issuer".into());
        }
        let aud_ok = match &claims["aud"] {
            Value::String(a) => a == &p.client_id,
            Value::Array(a) => a.iter().any(|v| v.as_str() == Some(&p.client_id)),
            _ => false,
        };
        if !aud_ok {
            return Err("the ID token is for a different application".into());
        }
        if claims["nonce"].as_str() != saved["nonce"].as_str() {
            return Err("the ID token does not belong to this sign-in".into());
        }
        if claims["exp"].as_f64().is_none_or(|x| x < (chrono::Utc::now().timestamp() - 60) as f64) {
            return Err("the ID token has expired".into());
        }
        let subject = claims["sub"].as_str().filter(|s| !s.is_empty()).ok_or("the ID token names no one")?.to_string();
        let mut info = claims.clone();
        if info["email"].as_str().is_none() {
            if let (Some(u), Some(at)) = (&e.userinfo, t["access_token"].as_str()) {
                let ui: Value = client.get(u).bearer_auth(at).send().await.map_err(|e| e.to_string())?.json().await.unwrap_or_default();
                if ui["sub"].as_str() == Some(subject.as_str()) {
                    info = ui;
                }
            }
        }
        // Apple's very first answer may carry the address separately.
        if info["email"].as_str().is_none() {
            if let Some(u) = apple_user.and_then(|u| serde_json::from_str::<Value>(u).ok()) {
                info["email"] = u["email"].clone();
            }
        }
        let flag = |v: &Value| v == &json!(true) || v == &json!("true");
        let verified = match p.kind.as_str() {
            "microsoft" => claims["tid"].as_str() == Some(MS_PERSONAL_TENANT) || flag(&claims["xms_edov"]),
            _ => flag(&info["email_verified"]),
        };
        let email = info["email"].as_str().and_then(|x| normalize_email(x).ok());
        return Ok(Who { subject, email, verified });
    }

    // Facebook and X: the account from their API.
    let at = t["access_token"].as_str().ok_or("no access token")?;
    let me: Value = client.get(e.userinfo.as_deref().unwrap_or_default()).bearer_auth(at).send().await.map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
    let me = if p.kind == "x" { me["data"].clone() } else { me };
    let subject = me["id"].as_str().filter(|s| !s.is_empty()).ok_or("the account has no id")?.to_string();
    let email = me[if p.kind == "x" { "confirmed_email" } else { "email" }].as_str().and_then(|x| normalize_email(x).ok());
    // Both share only addresses their owner confirmed.
    Ok(Who { subject, verified: email.is_some(), email })
}

/// The employee: linked before, or found by a verified email address (and linked now).
async fn find(state: &AppState, p: &Provider, who: &Who) -> Result<Uuid, String> {
    let linked: Option<(Uuid, bool)> = sqlx::query!(r#"SELECT u.id, u.is_active FROM user_identities i JOIN users u ON u.id = i.user_id WHERE i.provider_id = $1 AND i.subject = $2"#, p.id, &who.subject).map(|r| (r.id, r.is_active))
        .fetch_optional(&state.db)
        .await
        .map_err(|e| e.to_string())?;
    let user = match linked {
        Some((_, false)) => return Err("this account has been deactivated".into()),
        Some((id, true)) => id,
        None => {
            let email = who.email.as_deref().ok_or_else(|| format!("{} did not share your email address; sign in with your password and link the account from your profile", p.label))?;
            if !who.verified {
                return Err(format!("{} has not verified {email}; sign in with your password and link the account from your profile", p.label));
            }
            let found: Option<(Uuid, bool, bool)> = sqlx::query!(r#"SELECT id, is_active, join_pending FROM users WHERE email = $1 AND NOT is_system"#, email).map(|r| (r.id, r.is_active, r.join_pending)).fetch_optional(&state.db).await.map_err(|e| e.to_string())?;
            match found {
                Some((_, _, true)) => return Err("your request to join is still waiting for approval".into()),
                Some((_, false, _)) => return Err("this account has been deactivated".into()),
                Some((id, true, _)) => {
                    sqlx::query!("INSERT INTO user_identities (provider_id, subject, user_id, email) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING", p.id, &who.subject, id, email)
                        .execute(&state.db)
                        .await
                        .map_err(|e| e.to_string())?;
                    id
                }
                None => return Err(format!("there is no employee account for {email}")),
            }
        }
    };
    // Signing in this way also accepts a pending invitation.
    sqlx::query!("UPDATE users SET invite_pending = FALSE, last_login_at = now() WHERE id = $1", user).execute(&state.db).await.map_err(|e| e.to_string())?;
    sqlx::query!("UPDATE user_identities SET last_used_at = now() WHERE provider_id = $1 AND subject = $2", p.id, &who.subject).execute(&state.db).await.map_err(|e| e.to_string())?;
    Ok(user)
}

// ---------------------------------------------------------------------------
// One's own linked accounts
// ---------------------------------------------------------------------------

pub async fn my_identities(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let rows: Vec<(Uuid, String, String, Option<String>, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query!(r#"SELECT p.id, p.kind, p.label, i.email, i.created_at, i.last_used_at FROM user_identities i JOIN sign_in_providers p ON p.id = i.provider_id WHERE i.user_id = $1 ORDER BY p.position"#, auth.id).map(|r| (r.id, r.kind, r.label, r.email, r.created_at, r.last_used_at))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, k, l, e, c, u)| json!({ "provider_id": id, "kind": k, "label": l, "email": e, "created_at": c, "last_used_at": u })).collect::<Vec<_>>() })))
}

pub async fn unlink(State(state): State<AppState>, auth: AuthUser, Path(provider): Path<Uuid>) -> AppResult<Json<Value>> {
    sqlx::query!("DELETE FROM user_identities WHERE user_id = $1 AND provider_id = $2", auth.id, provider).execute(&state.db).await?;
    my_identities(State(state), auth).await
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

fn admin_json(p: &Provider, linked: i64) -> Value {
    json!({ "id": p.id, "kind": p.kind, "label": p.label, "client_id": p.client_id, "has_secret": p.secret.is_some(), "issuer": p.issuer,
        "team_id": p.team_id, "key_id": p.key_id, "enabled": p.enabled, "linked": linked })
}

pub async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    let rows: Vec<Provider> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM sign_in_providers ORDER BY position, created_at"))).fetch_all(&state.db).await?;
    let counts: Vec<(Uuid, i64)> = sqlx::query!(r#"SELECT provider_id AS "provider_id!", COUNT(*) AS "c1!" FROM user_identities GROUP BY 1"#).map(|r| (r.provider_id, r.c1)).fetch_all(&state.db).await?;
    Ok(Json(json!({
        "redirect_uri": redirect_uri(&state),
        "items": rows.iter().map(|p| admin_json(p, counts.iter().find(|(i, _)| *i == p.id).map(|(_, n)| *n).unwrap_or(0))).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct ProviderInput {
    kind: Option<String>,
    label: Option<String>,
    client_id: Option<String>,
    /// Absent keeps it; Apple: the .p8 private key's text.
    secret: Option<String>,
    issuer: Option<String>,
    team_id: Option<String>,
    key_id: Option<String>,
    enabled: Option<bool>,
}

fn default_label(kind: &str) -> &'static str {
    match kind {
        "google" => "Sign in with Google",
        "microsoft" => "Sign in with Microsoft",
        "apple" => "Sign in with Apple",
        "facebook" => "Sign in with Facebook",
        "x" => "Sign in with X",
        _ => "Sign in with your account",
    }
}

async fn save(state: &AppState, auth: &AuthUser, id: Option<Uuid>, b: ProviderInput) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    let old = match id {
        Some(i) => Some(provider(state, i).await?),
        None => None,
    };
    let kind = b.kind.or(old.as_ref().map(|o| o.kind.clone())).unwrap_or_default();
    if !KINDS.contains(&kind.as_str()) {
        return Err(bad("Google, Microsoft, Apple, Facebook, X or OpenID Connect"));
    }
    let text = |v: Option<String>, o: Option<String>| v.or(o).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let client_id = text(b.client_id, old.as_ref().map(|o| o.client_id.clone())).ok_or_else(|| bad("the client ID is needed"))?;
    let label = text(b.label, old.as_ref().map(|o| o.label.clone())).unwrap_or_else(|| default_label(&kind).into()).chars().take(60).collect::<String>();
    let issuer = text(b.issuer, old.as_ref().and_then(|o| o.issuer.clone())).map(|i| i.trim_end_matches('/').to_string());
    let team_id = text(b.team_id, old.as_ref().and_then(|o| o.team_id.clone()));
    let key_id = text(b.key_id, old.as_ref().and_then(|o| o.key_id.clone()));
    if kind == "oidc" && issuer.as_deref().is_none_or(|i| !super::sso::issuer_ok_pub(i)) {
        return Err(bad("the issuer must be an https:// address"));
    }
    if kind == "apple" && (team_id.is_none() || key_id.is_none()) {
        return Err(bad("Apple needs the team ID and the key ID"));
    }
    if client_id.len() > 500 || issuer.as_ref().is_some_and(|i| i.len() > 500) {
        return Err(bad("a setting is too long"));
    }
    let secret = match b.secret.map(|s| s.trim().to_string()) {
        Some(s) if s.is_empty() => None,
        Some(s) => {
            if s.len() > 10_000 {
                return Err(bad("the secret is too long"));
            }
            let p = Provider { id: Uuid::nil(), kind: kind.clone(), label: label.clone(), client_id: client_id.clone(), secret: None, issuer: issuer.clone(), team_id: team_id.clone(), key_id: key_id.clone(), enabled: true };
            if kind == "apple" {
                apple_secret(&p, &s).map_err(bad)?;
            }
            Some(crate::crypto::seal(&state.config.data_key, &s))
        }
        None => old.as_ref().and_then(|o| o.secret.clone()),
    };
    let enabled = b.enabled.or(old.as_ref().map(|o| o.enabled)).unwrap_or(true);
    if enabled && secret.is_none() {
        return Err(bad(if kind == "apple" { "paste the private key (.p8) before turning it on" } else { "set the client secret before turning it on" }));
    }
    let pid = id.unwrap_or_else(Uuid::new_v4);
    sqlx::query!("INSERT INTO sign_in_providers (id, kind, label, client_id, secret, issuer, team_id, key_id, enabled, position)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, (SELECT COALESCE(MAX(position), 0) + 1 FROM sign_in_providers))
         ON CONFLICT (id) DO UPDATE SET kind = $2, label = $3, client_id = $4, secret = $5, issuer = $6, team_id = $7, key_id = $8, enabled = $9", pid, &kind, &label, &client_id, secret.as_deref(), issuer.as_deref(), team_id.as_deref(), key_id.as_deref(), enabled)
    .execute(&state.db)
    .await?;
    audit::record(&state.db, Some(auth.id), "directory.provider_update", "settings", Some(pid.to_string()), json!({ "kind": kind, "enabled": enabled }), None).await;
    let p = provider(state, pid).await?;
    Ok(Json(admin_json(&p, 0)))
}

pub async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<ProviderInput>) -> AppResult<Json<Value>> {
    save(&state, &auth, None, b).await
}

pub async fn change(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ProviderInput>) -> AppResult<Json<Value>> {
    save(&state, &auth, Some(id), b).await
}

pub async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    sqlx::query!("DELETE FROM sign_in_providers WHERE id = $1", id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}
