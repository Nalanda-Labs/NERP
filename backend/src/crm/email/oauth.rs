//! Gmail and Microsoft 365 mailboxes: OAuth2 authorisation-code flow with PKCE, then
//! XOAUTH2 for IMAP and SMTP using short-lived access tokens refreshed on demand.
//!
//! Flow: the app asks for an authorisation URL (state + PKCE verifier kept in Redis for 10
//! minutes, bound to the user), the browser signs in at the provider, the provider
//! redirects to `/api/crm/mailboxes/oauth/callback`, the code is exchanged for tokens, and
//! the mailbox is created (or reconnected) with the encrypted refresh token.

use axum::{
    extract::{Path, Query, State},
    response::Redirect,
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Duration, Utc};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{imap, load_mailbox, queue_sync, MailboxRow};
use crate::{
    auth::{tokens::random_token, AuthUser},
    config::OAuthClient,
    crypto,
    error::{AppError, AppResult},
    state::AppState,
};

pub struct Provider<'a> {
    pub kind: &'static str,
    pub client: &'a OAuthClient,
    auth_url: String,
    token_url: String,
    pub(crate) scope: &'static str,
    extra: &'static [(&'static str, &'static str)],
    pub imap: (&'static str, i32, &'static str),
    pub smtp: (&'static str, i32, &'static str),
}

pub fn provider<'a>(state: &'a AppState, kind: &str) -> Option<Provider<'a>> {
    let c = &state.config.oauth;
    match kind {
        "google" => c.google.as_ref().map(|client| Provider {
            kind: "google",
            client,
            auth_url: Some(client.auth_url.clone()).filter(|u| !u.is_empty()).unwrap_or_else(|| "https://accounts.google.com/o/oauth2/v2/auth".into()),
            token_url: Some(client.token_url.clone()).filter(|u| !u.is_empty()).unwrap_or_else(|| "https://oauth2.googleapis.com/token".into()),
            scope: "https://mail.google.com/ openid email profile",
            // A refresh token is only issued with offline access and an explicit consent.
            extra: &[("access_type", "offline"), ("prompt", "consent")],
            imap: ("imap.gmail.com", 993, "tls"),
            smtp: ("smtp.gmail.com", 465, "tls"),
        }),
        "microsoft" => c.microsoft.as_ref().map(|client| {
            let base = format!("https://login.microsoftonline.com/{}/oauth2/v2.0", c.microsoft_tenant);
            Provider {
                kind: "microsoft",
                client,
                auth_url: Some(client.auth_url.clone()).filter(|u| !u.is_empty()).unwrap_or_else(|| format!("{base}/authorize")),
                token_url: Some(client.token_url.clone()).filter(|u| !u.is_empty()).unwrap_or_else(|| format!("{base}/token")),
                scope: "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access openid email profile",
                extra: &[("prompt", "select_account")],
                imap: ("outlook.office365.com", 993, "tls"),
                smtp: ("smtp.office365.com", 587, "starttls"),
            }
        }),
        _ => None,
    }
}

pub fn redirect_uri(state: &AppState) -> String {
    format!("{}/api/crm/mailboxes/oauth/callback", state.config.public_url)
}

pub(crate) fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .tls_backend_preconfigured(imap::client_config(false)?)
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())
}

/// Which providers this server offers, and the redirect URI to register with them.
pub async fn providers(State(state): State<AppState>, _auth: AuthUser) -> Json<Value> {
    Json(json!({
        "google": state.config.oauth.google.is_some(),
        "microsoft": state.config.oauth.microsoft.is_some(),
        "redirect_uri": redirect_uri(&state),
    }))
}

#[derive(Deserialize, Default)]
pub struct StartBody {
    /// Reconnect this mailbox (e.g. after access was revoked).
    mailbox_id: Option<Uuid>,
}

pub(crate) fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub fn authorize_url(p: &Provider<'_>, redirect: &str, state_token: &str, challenge: &str) -> String {
    let mut q = url_encode(&[
        ("client_id", p.client.client_id.as_str()),
        ("response_type", "code"),
        ("redirect_uri", redirect),
        ("scope", p.scope),
        ("state", state_token),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ]);
    for (k, v) in p.extra {
        q.push('&');
        q.push_str(&url_encode(&[(k, v)]));
    }
    format!("{}?{q}", p.auth_url)
}

pub(crate) fn url_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| {
            let enc = |s: &str| s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>();
            format!("{}={}", enc(k), enc(v))
        })
        .collect::<Vec<_>>()
        .join("&")
}

pub async fn start(State(state): State<AppState>, auth: AuthUser, Path(kind): Path<String>, body: Option<Json<StartBody>>) -> AppResult<Json<Value>> {
    let p = provider(&state, &kind).ok_or_else(|| AppError::bad(format!("{kind} sign-in is not configured on this server")))?;
    let b = body.map(|b| b.0).unwrap_or_default();
    if let Some(id) = b.mailbox_id {
        let m = load_mailbox(&state, id).await?;
        if m.owner_id != auth.id {
            return Err(AppError::NotFound("mailbox"));
        }
    }
    let token = random_token(24);
    let verifier = random_token(48);
    let mut redis = state.redis.clone();
    let _: () = redis
        .set_ex(format!("crm:oauth:{token}"), json!({ "user": auth.id, "kind": p.kind, "verifier": verifier, "mailbox_id": b.mailbox_id }).to_string(), 600)
        .await?;
    Ok(Json(json!({ "url": authorize_url(&p, &redirect_uri(&state), &token, &pkce_challenge(&verifier)) })))
}

#[derive(Deserialize)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
    pub id_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    error_description: Option<String>,
}

pub(crate) async fn token_request(p: &Provider<'_>, form: &[(&str, &str)]) -> Result<TokenResponse, String> {
    let mut all = vec![("client_id", p.client.client_id.as_str()), ("client_secret", p.client.client_secret.as_str())];
    all.extend_from_slice(form);
    let res = http()?.post(&p.token_url).form(&all).send().await.map_err(|e| format!("cannot reach {}: {e}", p.kind))?;
    let status = res.status();
    let text = res.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let e: Option<TokenError> = serde_json::from_str(&text).ok();
        return Err(match e {
            Some(e) if e.error == "invalid_grant" => "access was revoked or expired; connect the account again".into(),
            Some(e) => format!("{}: {}", e.error, e.error_description.unwrap_or_default()),
            None => format!("token request failed ({status})"),
        });
    }
    serde_json::from_str(&text).map_err(|e| format!("unexpected token response: {e}"))
}

/// Claims of an ID token received directly from the provider's token endpoint over TLS
/// (so its signature need not be checked again).
pub fn id_claims(id_token: &str) -> Option<Value> {
    let payload = id_token.split('.').nth(1)?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?).ok()
}

#[derive(Deserialize)]
pub struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// The provider sends the browser here (for mailboxes and for calendars, one redirect URI);
/// the result goes back to the page the sign-in started from.
pub async fn callback(State(state): State<AppState>, Query(q): Query<Callback>) -> Redirect {
    // One use only.
    let saved: Option<Value> = match q.state.as_deref().filter(|t| !t.is_empty()) {
        Some(t) => {
            let mut redis = state.redis.clone();
            let v: Option<String> = redis.get_del(format!("crm:oauth:{t}")).await.unwrap_or(None);
            v.and_then(|v| serde_json::from_str(&v).ok())
        }
        None => None,
    };
    let calendar = saved.as_ref().is_some_and(|s| s["purpose"] == "calendar");
    let base = format!("{}{}", state.config.public_url, if calendar { "/calendar" } else { "/crm/mailboxes" });
    let back = |k: &str, v: &str| Redirect::to(&format!("{base}?{}", url_encode(&[(k, v)])));
    if let Some(e) = q.error {
        return back("error", &q.error_description.unwrap_or(e));
    }
    let code = q.code.unwrap_or_default();
    let result = match saved {
        None => Err("the sign-in took too long or was already used; try again".to_string()),
        Some(_) if code.is_empty() => Err("the sign-in was not completed".to_string()),
        Some(s) if calendar => crate::calendar::sync::oauth_finish(&state, s, code).await,
        Some(s) => finish(&state, code, s).await,
    };
    match result {
        Ok(email) => back("connected", &email),
        Err(e) => back("error", &e),
    }
}

async fn finish(state: &AppState, code: String, saved: Value) -> Result<String, String> {
    let user: Uuid = serde_json::from_value(saved["user"].clone()).map_err(|e| e.to_string())?;
    let kind = saved["kind"].as_str().unwrap_or_default().to_string();
    let p = provider(state, &kind).ok_or("this provider is no longer configured")?;
    let t = token_request(&p, &[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &redirect_uri(state)),
        ("code_verifier", saved["verifier"].as_str().unwrap_or_default()),
    ])
    .await?;
    let refresh = t.refresh_token.ok_or("the provider did not grant offline access; try again and accept all permissions")?;
    let claims = t.id_token.as_deref().and_then(id_claims).unwrap_or(json!({}));
    let email = ["email", "preferred_username", "upn"]
        .iter()
        .find_map(|k| claims[k].as_str().filter(|s| s.contains('@')))
        .ok_or("the provider did not share the email address")?
        .to_lowercase();
    let name = claims["name"].as_str().unwrap_or("").to_string();
    let key = &state.config.data_key;
    let expires = Utc::now() + Duration::seconds(t.expires_in.unwrap_or(3600));
    let reconnect: Option<Uuid> = serde_json::from_value(saved["mailbox_id"].clone()).unwrap_or(None);
    // Reconnecting, or the same account connected again: update in place.
    let existing: Option<Uuid> = match reconnect {
        Some(id) => Some(id),
        None => sqlx::query_scalar!("SELECT id FROM crm_mailboxes WHERE owner_id = $1 AND lower(email) = $2 AND auth_kind = $3", user, &email, p.kind)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| e.to_string())?,
    };
    let id = match existing {
        Some(id) => {
            sqlx::query!("UPDATE crm_mailboxes SET secret = $3, access_token = $4, access_expires_at = $5, last_error = NULL, updated_at = now()
                 WHERE id = $1 AND owner_id = $2 AND auth_kind = $6", id, user, crypto::seal(key, &refresh), crypto::seal(key, &t.access_token), expires, p.kind)
            .execute(&state.db)
            .await
            .map_err(|e| e.to_string())?;
            id
        }
        None => {
            let n: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM crm_mailboxes WHERE owner_id = $1"#, user).fetch_one(&state.db).await.map_err(|e| e.to_string())?;
            if n >= 10 {
                return Err("at most 10 mailboxes per person".into());
            }
            sqlx::query_scalar!("INSERT INTO crm_mailboxes (owner_id, email, display_name, imap_host, imap_port, imap_security, smtp_host, smtp_port, smtp_security, smtp_auth,
                     username, secret, auth_kind, access_token, access_expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, TRUE, $2, $10, $11, $12, $13) RETURNING id", user, &email, &name, p.imap.0, p.imap.1, p.imap.2, p.smtp.0, p.smtp.1, p.smtp.2, crypto::seal(key, &refresh), p.kind, crypto::seal(key, &t.access_token), expires)
            .fetch_one(&state.db)
            .await
            .map_err(|e| e.to_string())?
        }
    };
    crate::audit::record(&state.db, Some(user), "crm.mailbox.connect", "crm_mailbox", Some(id.to_string()), json!({ "email": email, "via": p.kind }), None).await;
    let _ = queue_sync(state, id).await;
    Ok(email)
}

/// A valid access token for an OAuth mailbox, refreshing it when it is about to expire.
pub async fn access_token(state: &AppState, mb: &MailboxRow) -> Result<String, String> {
    let key = &state.config.data_key;
    if let (Some(tok), Some(exp)) = (&mb.access_token, mb.access_expires_at)
        && exp > Utc::now() + Duration::seconds(90)
            && let Some(t) = crypto::open(key, tok) {
                return Ok(t);
            }
    let p = provider(state, &mb.auth_kind).ok_or("this provider is no longer configured on the server")?;
    let refresh = crypto::open(key, &mb.secret).ok_or("the stored sign-in cannot be read; reconnect the mailbox")?;
    let t = token_request(&p, &[("grant_type", "refresh_token"), ("refresh_token", &refresh)]).await?;
    // Microsoft rotates refresh tokens; keep the newest.
    sqlx::query!("UPDATE crm_mailboxes SET access_token = $2, access_expires_at = $3, secret = COALESCE($4, secret) WHERE id = $1", mb.id, crypto::seal(key, &t.access_token), Utc::now() + Duration::seconds(t.expires_in.unwrap_or(3600)), t.refresh_token.as_deref().map(|r| crypto::seal(key, r)))
        .execute(&state.db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(t.access_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_and_claims() {
        // RFC 7636 appendix B.
        assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let payload = URL_SAFE_NO_PAD.encode(br#"{"email":"Ann@Example.com","name":"Ann"}"#);
        let c = id_claims(&format!("h.{payload}.s")).unwrap();
        assert_eq!(c["email"], "Ann@Example.com");
        assert!(id_claims("garbage").is_none());
        assert_eq!(url_encode(&[("scope", "a b/c"), ("x", "é")]), "scope=a%20b%2Fc&x=%C3%A9");
    }

    #[test]
    fn authorize_url_has_pkce_and_offline() {
        let client = OAuthClient { client_id: "cid".into(), client_secret: "s".into(), auth_url: String::new(), token_url: String::new() };
        let p = Provider {
            kind: "google",
            client: &client,
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: String::new(),
            scope: "https://mail.google.com/ openid email profile",
            extra: &[("access_type", "offline"), ("prompt", "consent")],
            imap: ("imap.gmail.com", 993, "tls"),
            smtp: ("smtp.gmail.com", 465, "tls"),
        };
        let u = authorize_url(&p, "https://crm.example/api/crm/mailboxes/oauth/callback", "st", "ch");
        assert!(u.starts_with("https://accounts.google.com/o/oauth2/v2/auth?client_id=cid&response_type=code"));
        assert!(u.contains("redirect_uri=https%3A%2F%2Fcrm.example%2Fapi%2Fcrm%2Fmailboxes%2Foauth%2Fcallback"));
        assert!(u.contains("code_challenge=ch&code_challenge_method=S256") && u.contains("access_type=offline") && u.contains("scope=https%3A%2F%2Fmail.google.com%2F%20openid"));
    }
}
