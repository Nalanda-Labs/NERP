//! Single sign-on with a SAML 2.0 identity provider (Microsoft Entra ID / ADFS, Okta, Google
//! Workspace, Keycloak, Shibboleth, OneLogin, …), as a service provider.
//!
//! - NERP publishes its metadata at /api/auth/saml/metadata (entity id = that address,
//!   assertion consumer service = /api/auth/saml/acs, HTTP-POST). Its own key and certificate
//!   are made on first use (sealed with the data key): sign-in requests are signed with them,
//!   and assertions encrypted for NERP can be read.
//! - The identity provider is set up from its metadata (fetched from a URL, or pasted). It
//!   must have a signing certificate: responses are only accepted when their signature checks
//!   out (xmlsec; only the signed part of the document is read, against signature-wrapping)
//!   and uses SHA-256 or stronger. The issuer, audience, destination and recipient, the time
//!   limits and the request the response answers are all checked; responses nobody asked for
//!   (IdP-initiated) are refused, and each assertion is accepted once.
//! - The person is the NameID; their email comes from an attribute (or the NameID when it is
//!   an email address). Accounts are matched and made as for OpenID Connect: existing
//!   employees by email, new ones only when allowed, limited to the allowed email domains, and
//!   the company's two-step sign-in applies.

use axum::{
    extract::State,
    http::header,
    response::{IntoResponse, Redirect, Response},
    Form, Json,
};
use openssl::{
    asn1::Asn1Time,
    bn::{BigNum, MsbOption},
    hash::MessageDigest,
    pkey::{PKey, Private},
    rsa::Rsa,
    x509::{X509NameBuilder, X509},
};
use redis::AsyncCommands;
use samael::{
    crypto::{AllowedSignatureAlgorithm, CertificateDer},
    metadata::{EntityDescriptor, HTTP_REDIRECT_BINDING},
    schema::Assertion,
    service_provider::{ServiceProvider, ServiceProviderBuilder},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::bad;
use crate::{
    audit,
    auth::{permissions as perm, tokens::{random_token, ClientMeta}, AuthUser},
    crm::email::oauth::url_encode,
    crypto,
    error::{AppError, AppResult},
    routes::normalize_email,
    state::AppState,
};

const FLOW_TTL_SECS: u64 = 600;

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct SamlConfig {
    pub enabled: bool,
    /// Where the identity provider's metadata is read from (optional: it can be pasted).
    pub metadata_url: String,
    /// The identity provider's metadata (EntityDescriptor XML).
    pub metadata_xml: String,
    pub button_label: String,
    /// Only these email domains may sign in (empty = any).
    pub allowed_domains: Vec<String>,
    /// Create an account for someone who signs in for the first time.
    pub auto_create: bool,
    /// The attribute holding the email address (empty: the usual names, then the NameID).
    pub email_attribute: String,
    /// NERP's own certificate (PEM), published in its metadata.
    pub sp_certificate: String,
}

impl Default for SamlConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            metadata_url: String::new(),
            metadata_xml: String::new(),
            button_label: "Sign in with company account (SAML)".into(),
            allowed_domains: Vec::new(),
            auto_create: false,
            email_attribute: String::new(),
            sp_certificate: String::new(),
        }
    }
}

pub fn entity_id(state: &AppState) -> String {
    format!("{}/api/auth/saml/metadata", state.config.public_url)
}

pub fn acs_url(state: &AppState) -> String {
    format!("{}/api/auth/saml/acs", state.config.public_url)
}

async fn load(state: &AppState) -> AppResult<(SamlConfig, Option<PKey<Private>>)> {
    let row: Option<(Value, Option<Vec<u8>>)> = sqlx::query_as("SELECT saml, saml_key FROM directory_config WHERE id = 1").fetch_optional(&state.db).await?;
    let (cfg, key) = row.unwrap_or((json!({}), None));
    let key = key
        .and_then(|k| crypto::open(&state.config.data_key, &k))
        .and_then(|pem| PKey::private_key_from_pem(pem.as_bytes()).ok());
    Ok((serde_json::from_value(cfg).unwrap_or_default(), key))
}

/// NERP's key and certificate, made once (RSA 3072, valid ten years).
async fn own_keys(state: &AppState) -> AppResult<(PKey<Private>, X509)> {
    let (cfg, key) = load(state).await?;
    if let (Some(k), Ok(c)) = (key, X509::from_pem(cfg.sp_certificate.as_bytes())) {
        return Ok((k, c));
    }
    let made = (|| -> Result<(PKey<Private>, X509), openssl::error::ErrorStack> {
        let key = PKey::from_rsa(Rsa::generate(3072)?)?;
        let host = reqwest::Url::parse(&state.config.public_url).ok().and_then(|u| u.host_str().map(String::from)).unwrap_or_else(|| "nerp".into());
        let mut name = X509NameBuilder::new()?;
        name.append_entry_by_text("CN", &format!("NERP SAML {host}"))?;
        let name = name.build();
        let mut b = X509::builder()?;
        b.set_version(2)?;
        let mut serial = BigNum::new()?;
        serial.rand(128, MsbOption::MAYBE_ZERO, false)?;
        let serial = serial.to_asn1_integer()?;
        b.set_serial_number(&serial)?;
        b.set_subject_name(&name)?;
        b.set_issuer_name(&name)?;
        b.set_pubkey(&key)?;
        let (from, to): (Asn1Time, Asn1Time) = (Asn1Time::days_from_now(0)?, Asn1Time::days_from_now(3650)?);
        b.set_not_before(&from)?;
        b.set_not_after(&to)?;
        b.sign(&key, MessageDigest::sha256())?;
        Ok((key, b.build()))
    })()
    .map_err(|e| AppError::Internal(e.into()))?;
    let key_pem = String::from_utf8(made.0.private_key_to_pem_pkcs8().map_err(|e| AppError::Internal(e.into()))?).map_err(|e| AppError::Internal(e.into()))?;
    let cert_pem = String::from_utf8(made.1.to_pem().map_err(|e| AppError::Internal(e.into()))?).map_err(|e| AppError::Internal(e.into()))?;
    // Only if no other request made them meanwhile.
    let saved = sqlx::query(
        "UPDATE directory_config SET saml = jsonb_set(COALESCE(saml, '{}'), '{sp_certificate}', to_jsonb($1::text)), saml_key = $2
         WHERE id = 1 AND saml_key IS NULL",
    )
    .bind(&cert_pem)
    .bind(crypto::seal(&state.config.data_key, &key_pem))
    .execute(&state.db)
    .await?
    .rows_affected();
    if saved == 0 {
        let (cfg, key) = load(state).await?;
        if let (Some(k), Ok(c)) = (key, X509::from_pem(cfg.sp_certificate.as_bytes())) {
            return Ok((k, c));
        }
    }
    Ok(made)
}

/// The identity provider from its metadata, checked for what sign-in needs.
fn parse_idp(xml: &str) -> Result<EntityDescriptor, String> {
    let md: EntityDescriptor = xml.trim().parse().map_err(|e| format!("the identity provider's metadata can't be read: {e}"))?;
    if md.entity_id.as_deref().unwrap_or("").is_empty() {
        return Err("the metadata names no entity ID".into());
    }
    let sp = ServiceProvider { idp_metadata: md.clone(), ..Default::default() };
    if sp.sso_binding_location(HTTP_REDIRECT_BINDING).is_none() {
        return Err("the identity provider offers no single sign-on address with the HTTP-Redirect binding".into());
    }
    match sp.idp_signing_certs() {
        Ok(Some(c)) if !c.is_empty() => {}
        _ => return Err("the metadata has no signing certificate: NERP only accepts signed responses".into()),
    }
    Ok(md)
}

fn provider(state: &AppState, cfg: &SamlConfig, key: PKey<Private>, cert: &X509) -> AppResult<ServiceProvider> {
    let idp = parse_idp(&cfg.metadata_xml).map_err(bad)?;
    ServiceProviderBuilder::default()
        .entity_id(entity_id(state))
        .metadata_url(entity_id(state))
        .acs_url(acs_url(state))
        .slo_url(None::<String>)
        .idp_metadata(idp)
        .key(key)
        .certificate(CertificateDer::from(cert.to_der().map_err(|e| AppError::Internal(e.into()))?))
        .allow_idp_initiated(false)
        .max_issue_delay(chrono::Duration::minutes(5))
        .max_clock_skew(chrono::Duration::minutes(3))
        .allowed_signature_algorithms(Some(vec![
            AllowedSignatureAlgorithm::RsaSha256,
            AllowedSignatureAlgorithm::RsaSha384,
            AllowedSignatureAlgorithm::RsaSha512,
            AllowedSignatureAlgorithm::EcdsaSha256,
            AllowedSignatureAlgorithm::EcdsaSha384,
            AllowedSignatureAlgorithm::EcdsaSha512,
        ]))
        .build()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))
}

// ---------------------------------------------------------------------------
// Public: metadata and sign-in
// ---------------------------------------------------------------------------

/// NERP's service-provider metadata, for the identity provider's administrator. (Written
/// here rather than by the library, which would announce a single-logout address NERP
/// doesn't have.)
pub async fn metadata(State(state): State<AppState>) -> AppResult<Response> {
    let (_, cert) = own_keys(&state).await?;
    let der = cert.to_der().map_err(|e| AppError::Internal(e.into()))?;
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let key_info = |usage: &str| {
        format!(r#"<md:KeyDescriptor use="{usage}"><ds:KeyInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#"><ds:X509Data><ds:X509Certificate>{b64}</ds:X509Certificate></ds:X509Data></ds:KeyInfo></md:KeyDescriptor>"#)
    };
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" entityID="{entity}">
  <md:SPSSODescriptor AuthnRequestsSigned="true" WantAssertionsSigned="true" protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    {signing}
    {encryption}
    <md:NameIDFormat>urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress</md:NameIDFormat>
    <md:NameIDFormat>urn:oasis:names:tc:SAML:2.0:nameid-format:persistent</md:NameIDFormat>
    <md:AssertionConsumerService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST" Location="{acs}" index="0" isDefault="true"/>
  </md:SPSSODescriptor>
</md:EntityDescriptor>
"#,
        entity = xml_escape(&entity_id(&state)),
        acs = xml_escape(&acs_url(&state)),
        signing = key_info("signing"),
        encryption = key_info("encryption"),
    );
    Ok(([(header::CONTENT_TYPE, "application/samlmetadata+xml"), (header::CONTENT_DISPOSITION, "inline; filename=\"nerp-saml-metadata.xml\"")], xml).into_response())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub async fn public_info(state: &AppState) -> Value {
    match load(state).await {
        Ok((c, _)) if c.enabled && !c.metadata_xml.is_empty() => json!({ "enabled": true, "label": c.button_label }),
        _ => json!({ "enabled": false }),
    }
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
    let (cfg, _) = load(state).await.map_err(|e| e.to_string())?;
    if !cfg.enabled || cfg.metadata_xml.is_empty() {
        return Err("SAML sign-on is not set up".into());
    }
    let (key, cert) = own_keys(state).await.map_err(|e| e.to_string())?;
    let sp = provider(state, &cfg, key.clone(), &cert).map_err(|e| e.to_string())?;
    let to = sp.sso_binding_location(HTTP_REDIRECT_BINDING).ok_or("the identity provider has no sign-in address")?;
    let req = sp.make_authentication_request(&to).map_err(|e| e.to_string())?;
    let relay = random_token(24);
    let mut redis = state.redis.clone();
    let _: () = redis.set_ex(format!("auth:saml:{relay}"), &req.id, FLOW_TTL_SECS).await.map_err(|e| e.to_string())?;
    let url = req.signed_redirect(&relay, &key).map_err(|e| e.to_string())?.ok_or("the identity provider has no sign-in address")?;
    Ok(url.to_string())
}

#[derive(Deserialize)]
pub struct AcsForm {
    #[serde(rename = "SAMLResponse")]
    saml_response: Option<String>,
    #[serde(rename = "RelayState")]
    relay_state: Option<String>,
}

/// The identity provider posts the browser here with its signed answer.
pub async fn acs(State(state): State<AppState>, meta: ClientMeta, Form(f): Form<AcsForm>) -> Response {
    match finish(&state, &meta, f).await {
        Ok(user) => super::sso::complete_sign_in(&state, &meta, user).await,
        Err(e) => {
            audit::record(&state.db, None, "auth.saml_failed", "user", None, json!({ "error": e }), meta.ip.as_deref()).await;
            to_login(&e)
        }
    }
}

/// Values of the first attribute among these names (by Name or FriendlyName).
fn attribute(a: &Assertion, names: &[&str]) -> Option<String> {
    let statements = a.attribute_statements.as_ref()?;
    for n in names {
        for s in statements {
            for at in &s.attributes {
                let hit = at.name.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(n)) || at.friendly_name.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(n));
                if hit
                    && let Some(v) = at.values.iter().find_map(|v| v.value.clone()).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
                        return Some(v);
                    }
            }
        }
    }
    None
}

const EMAIL: &[&str] = &[
    "email", "mail", "emailaddress", "Email",
    "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress",
    "urn:oid:0.9.2342.19200300.100.1.3",
    "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/upn",
];
const FIRST: &[&str] = &["givenName", "firstName", "first_name", "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/givenname", "urn:oid:2.5.4.42"];
const LAST: &[&str] = &["sn", "surname", "lastName", "last_name", "http://schemas.xmlsoap.org/ws/2005/05/identity/claims/surname", "urn:oid:2.5.4.4"];

async fn finish(state: &AppState, meta: &ClientMeta, f: AcsForm) -> Result<Uuid, String> {
    let (relay, response) = match (f.relay_state, f.saml_response) {
        (Some(r), Some(s)) if !r.is_empty() && !s.is_empty() => (r, s),
        // Sign-ins started at the identity provider (no request of ours) aren't accepted.
        _ => return Err("the sign-in was not started here; use the sign-in button".into()),
    };
    let mut redis = state.redis.clone();
    let request_id: Option<String> = redis.get_del(format!("auth:saml:{relay}")).await.map_err(|e| e.to_string())?;
    let request_id = request_id.ok_or("the sign-in took too long or was already used; try again")?;
    let (cfg, _) = load(state).await.map_err(|e| e.to_string())?;
    if !cfg.enabled {
        return Err("SAML sign-on is not set up".into());
    }
    let (key, cert) = own_keys(state).await.map_err(|e| e.to_string())?;
    let sp = provider(state, &cfg, key, &cert).map_err(|e| e.to_string())?;
    let assertion = tokio::task::spawn_blocking(move || {
        sp.parse_base64_response(response.trim(), Some(&[request_id.as_str()])).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| {
        tracing::warn!(error = %e, "SAML response refused");
        "the identity provider's answer could not be verified".to_string()
    })?;
    // Each assertion once (it stays valid for minutes).
    let fresh: bool = redis::cmd("SET").arg(format!("auth:saml:used:{}", assertion.id)).arg(1).arg("NX").arg("EX").arg(3600)
        .query_async::<Option<String>>(&mut redis).await.map_err(|e| e.to_string())?.is_some();
    if !fresh {
        return Err("this sign-in answer was already used".into());
    }
    let name_id = assertion.subject.as_ref().and_then(|s| s.name_id.as_ref()).map(|n| (n.value.trim().to_string(), n.format.clone().unwrap_or_default()));
    let (subject, format) = name_id.filter(|(v, _)| !v.is_empty()).ok_or("the identity provider named no one")?;
    let mut names: Vec<&str> = Vec::new();
    if !cfg.email_attribute.is_empty() {
        names.push(cfg.email_attribute.as_str());
    }
    names.extend_from_slice(EMAIL);
    let email = attribute(&assertion, &names)
        .or_else(|| (format.ends_with("emailAddress") || subject.contains('@')).then(|| subject.clone()))
        .and_then(|e| normalize_email(&e).ok())
        .ok_or("the identity provider did not share your email address")?;
    let domain = email.rsplit('@').next().unwrap_or_default().to_string();
    if !cfg.allowed_domains.is_empty() && !cfg.allowed_domains.iter().any(|d| d.eq_ignore_ascii_case(&domain)) {
        return Err(format!("accounts from {domain} cannot sign in here"));
    }
    let found: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT id, is_active FROM users WHERE (auth_source = 'saml' AND external_id = $1) OR email = $2
         ORDER BY (auth_source = 'saml' AND external_id = $1) DESC LIMIT 1",
    )
    .bind(&subject)
    .bind(&email)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| e.to_string())?;
    let user = match found {
        Some((_, false)) => return Err("this account has been deactivated".into()),
        Some((id, true)) => {
            sqlx::query("UPDATE users SET invite_pending = FALSE, last_login_at = now() WHERE id = $1").bind(id).execute(&state.db).await.map_err(|e| e.to_string())?;
            id
        }
        None if cfg.auto_create => {
            let first = attribute(&assertion, FIRST).unwrap_or_else(|| email.split('@').next().unwrap_or_default().to_string());
            let last = attribute(&assertion, LAST).unwrap_or_else(|| "-".into());
            match create(state, &subject, &email, &first, &last).await {
                Ok(id) => id,
                Err(AppError::Database(e)) if crate::error::is_unique_violation(&e) => sqlx::query_scalar(
                    "SELECT id FROM users WHERE (auth_source = 'saml' AND external_id = $1) OR email = $2 LIMIT 1",
                )
                .bind(&subject)
                .bind(&email)
                .fetch_one(&state.db)
                .await
                .map_err(|e| e.to_string())?,
                Err(e) => return Err(e.to_string()),
            }
        }
        None => return Err(format!("there is no employee account for {email}; ask an administrator to add you")),
    };
    audit::record(&state.db, Some(user), "auth.login", "user", Some(user.to_string()), json!({ "method": "saml", "user_agent": meta.user_agent }), meta.ip.as_deref()).await;
    Ok(user)
}

async fn create(state: &AppState, subject: &str, email: &str, first: &str, last: &str) -> AppResult<Uuid> {
    let clip = |s: &str| s.trim().chars().take(100).collect::<String>();
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash, first_name, last_name, auth_source, external_id, last_login_at)
         VALUES ($1, '!sso', $2, $3, 'saml', $4, now()) RETURNING id",
    )
    .bind(email)
    .bind(clip(first))
    .bind(clip(last))
    .bind(subject)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO user_roles (user_id, role_id) SELECT $1, id FROM roles WHERE code = 'employee'").bind(id).execute(&mut *tx).await?;
    audit::record(&mut *tx, None, "user.create", "user", Some(id.to_string()), json!({ "email": email, "source": "saml" }), None).await;
    tx.commit().await?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// For the settings page: the configuration, NERP's addresses, and the identity provider.
pub async fn settings(state: &AppState) -> AppResult<Value> {
    let (cfg, _) = load(state).await?;
    let idp = (!cfg.metadata_xml.is_empty()).then(|| parse_idp(&cfg.metadata_xml));
    Ok(json!({
        "config": cfg,
        "entity_id": entity_id(state),
        "acs_url": acs_url(state),
        "metadata_url": entity_id(state),
        "idp": match idp {
            Some(Ok(md)) => json!({ "entity_id": md.entity_id, "ok": true }),
            Some(Err(e)) => json!({ "ok": false, "error": e }),
            None => Value::Null,
        },
    }))
}

#[derive(Deserialize)]
pub struct SamlUpdate {
    enabled: bool,
    #[serde(default)]
    metadata_url: String,
    #[serde(default)]
    metadata_xml: String,
    #[serde(default)]
    button_label: String,
    #[serde(default)]
    allowed_domains: Vec<String>,
    #[serde(default)]
    auto_create: bool,
    #[serde(default)]
    email_attribute: String,
}

/// Reads metadata from a URL (https, or http on this machine), at most 2 MB.
async fn fetch_metadata(url: &str) -> Result<String, String> {
    if !super::sso::issuer_ok_pub(url) {
        return Err("the metadata address must be https://".into());
    }
    let res = crate::crm::email::oauth::http()?.get(url).send().await.map_err(|e| format!("cannot read the metadata: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("the metadata address answered {}", res.status()));
    }
    let bytes = res.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("the metadata is larger than 2 MB".into());
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| "the metadata is not text".to_string())
}

pub async fn put_config(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SamlUpdate>) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    let (mut cfg, _) = load(&state).await?;
    cfg.metadata_url = b.metadata_url.trim().to_string();
    cfg.metadata_xml = b.metadata_xml.trim().to_string();
    if !cfg.metadata_url.is_empty() && cfg.metadata_xml.is_empty() {
        cfg.metadata_xml = fetch_metadata(&cfg.metadata_url).await.map_err(bad)?;
    }
    if cfg.metadata_xml.len() > 2 * 1024 * 1024 {
        return Err(bad("the metadata is larger than 2 MB"));
    }
    if !cfg.metadata_xml.is_empty() {
        parse_idp(&cfg.metadata_xml).map_err(bad)?;
    }
    cfg.button_label = b.button_label.trim().chars().take(60).collect();
    if cfg.button_label.is_empty() {
        cfg.button_label = SamlConfig::default().button_label;
    }
    cfg.allowed_domains = b.allowed_domains.iter().map(|d| d.trim().trim_start_matches('@').to_lowercase()).filter(|d| !d.is_empty()).collect();
    if cfg.allowed_domains.iter().any(|d| !d.contains('.') || d.contains(char::is_whitespace)) {
        return Err(bad("allowed domains look like example.com"));
    }
    cfg.auto_create = b.auto_create;
    cfg.email_attribute = b.email_attribute.trim().chars().take(200).collect();
    cfg.enabled = b.enabled;
    if cfg.enabled && cfg.metadata_xml.is_empty() {
        return Err(bad("add the identity provider's metadata before turning SAML sign-on on"));
    }
    own_keys(&state).await?;
    let (stored, _) = load(&state).await?;
    cfg.sp_certificate = stored.sp_certificate;
    sqlx::query("UPDATE directory_config SET saml = $1 WHERE id = 1").bind(serde_json::to_value(&cfg).map_err(anyhow::Error::from)?).execute(&state.db).await?;
    audit::record(&state.db, Some(auth.id), "directory.saml_update", "settings", None, json!({ "enabled": cfg.enabled }), None).await;
    Ok(Json(settings(&state).await?))
}

/// Reads the metadata again from its URL (identity providers change certificates).
pub async fn refresh(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    let (mut cfg, _) = load(&state).await?;
    if cfg.metadata_url.is_empty() {
        return Err(bad("there is no metadata address to read"));
    }
    let xml = fetch_metadata(&cfg.metadata_url).await.map_err(bad)?;
    parse_idp(&xml).map_err(bad)?;
    cfg.metadata_xml = xml;
    sqlx::query("UPDATE directory_config SET saml = $1 WHERE id = 1").bind(serde_json::to_value(&cfg).map_err(anyhow::Error::from)?).execute(&state.db).await?;
    Ok(Json(settings(&state).await?))
}
