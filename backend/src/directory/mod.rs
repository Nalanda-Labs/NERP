//! Company directory and single sign-on.
//!
//! - LDAP (Active Directory, OpenLDAP, …): employees are synced from the directory, and
//!   those accounts sign in with their directory password (checked by binding as them).
//!   Someone in the directory who has no NERP account yet gets one on first sign-in.
//! - OpenID Connect: a "Sign in with …" button on the login page.
//! - SAML 2.0: the same for identity providers that speak SAML (see `saml`).
//! - Other accounts (Google, Microsoft, Apple, Facebook, X, more OpenID Connect providers)
//!   for existing employees (see `social`).
//!
//! Settings live in the one-row `directory_config` table; secrets are sealed with the data key.

pub mod ldap;
pub mod saml;
pub mod social;
pub mod sso;

use axum::{
    extract::State,
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    auth::{permissions as perm, AuthUser},
    crypto,
    error::{AppError, AppResult},
    state::AppState,
};

pub use ldap::SYNC_JOB;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/directory", get(get_config))
        .route("/directory/ldap", put(ldap::put_config))
        .route("/directory/ldap/test", post(ldap::test))
        .route("/directory/ldap/sync", post(ldap::sync_now))
        .route("/directory/sso", put(sso::put_config))
        .route("/directory/sso/test", post(sso::test))
        // Public: the login page's button and the sign-in round trip.
        .route("/auth/sso", get(sso::public_info))
        .route("/auth/sso/start", get(sso::start))
        .route("/auth/sso/callback", get(sso::callback))
        .route("/directory/saml", put(saml::put_config))
        .route("/directory/saml/refresh", post(saml::refresh))
        .route("/auth/saml/metadata", get(saml::metadata))
        .route("/auth/saml/start", get(saml::start))
        .route("/auth/saml/acs", post(saml::acs))
        // Other accounts (Google, Microsoft, Apple, Facebook, X, OpenID Connect).
        .route("/directory/providers", get(social::list).post(social::create))
        .route("/directory/providers/{id}", put(social::change).delete(social::remove))
        .route("/auth/providers", get(social::public_list))
        .route("/auth/providers/{id}/start", get(social::start))
        .route("/auth/providers/{id}/link", post(social::link_start))
        .route("/auth/providers/callback", get(social::callback_get).post(social::callback_post))
        .route("/auth/identities", get(social::my_identities))
        .route("/auth/identities/{provider}", axum::routing::delete(social::unlink))
}

pub(crate) struct Stored {
    pub ldap: ldap::LdapConfig,
    pub ldap_secret: Option<String>,
    pub sso: sso::SsoConfig,
    pub sso_secret: Option<String>,
    pub ldap_synced_at: Option<chrono::DateTime<chrono::Utc>>,
    pub ldap_sync_result: Option<Value>,
}

pub(crate) async fn load(state: &AppState) -> AppResult<Stored> {
    type Row = (Value, Option<Vec<u8>>, Value, Option<Vec<u8>>, Option<chrono::DateTime<chrono::Utc>>, Option<Value>);
    let row: Option<Row> = sqlx::query!(r#"SELECT ldap, ldap_secret, sso, sso_secret, ldap_synced_at, ldap_sync_result FROM directory_config WHERE id = 1"#).map(|r| (r.ldap, r.ldap_secret, r.sso, r.sso_secret, r.ldap_synced_at, r.ldap_sync_result))
    .fetch_optional(&state.db)
    .await?;
    let (l, ls, s, ss, at, res) = row.unwrap_or((json!({}), None, json!({}), None, None, None));
    let open = |b: Option<Vec<u8>>| b.and_then(|b| crypto::open(&state.config.data_key, &b));
    Ok(Stored {
        ldap: serde_json::from_value(l).unwrap_or_default(),
        ldap_secret: open(ls),
        sso: serde_json::from_value(s).unwrap_or_default(),
        sso_secret: open(ss),
        ldap_synced_at: at,
        ldap_sync_result: res,
    })
}

/// A secret field in a settings update: absent keeps it, "" clears it, anything else replaces it.
#[derive(Deserialize, Default)]
#[serde(transparent)]
pub(crate) struct SecretUpdate(pub Option<String>);

impl SecretUpdate {
    /// The column update: None = keep; Some(None) = clear; Some(Some(bytes)) = set.
    pub fn sealed(&self, state: &AppState) -> Option<Option<Vec<u8>>> {
        self.0.as_ref().map(|s| (!s.is_empty()).then(|| crypto::seal(&state.config.data_key, s)))
    }
}

async fn get_config(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    let s = load(&state).await?;
    let counts: Vec<(String, i64)> =
        sqlx::query!(r#"SELECT auth_source AS "auth_source!", count(*) AS "c1!" FROM users WHERE is_active GROUP BY auth_source"#).map(|r| (r.auth_source, r.c1)).fetch_all(&state.db).await?;
    Ok(Json(json!({
        "ldap_available": state.config.ldap_enabled,
        "ldap": s.ldap,
        "ldap_has_password": s.ldap_secret.is_some(),
        "ldap_synced_at": s.ldap_synced_at,
        "ldap_sync_result": s.ldap_sync_result,
        "sso": s.sso,
        "sso_has_secret": s.sso_secret.is_some(),
        "sso_redirect_uri": sso::redirect_uri(&state),
        "saml": saml::settings(&state).await?,
        "accounts": counts.into_iter().map(|(k, v)| (k, json!(v))).collect::<serde_json::Map<_, _>>(),
    })))
}

/// Accounts whose password lives elsewhere cannot change or reset it here.
pub async fn external_source(state: &AppState, user: uuid::Uuid) -> AppResult<Option<String>> {
    let s: Option<String> = sqlx::query_scalar!("SELECT auth_source FROM users WHERE id = $1", user).fetch_optional(&state.db).await?;
    Ok(s.filter(|s| s != "local"))
}

pub(crate) fn bad(msg: impl Into<String>) -> AppError {
    AppError::bad(msg.into())
}
