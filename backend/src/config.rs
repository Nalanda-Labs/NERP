use anyhow::{bail, Context, Result};
use std::{env, net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_addr: SocketAddr,
    pub database_url: String,
    pub database_max_connections: u32,
    pub redis_url: String,
    pub jwt_secret: String,
    pub access_token_ttl_secs: i64,
    pub refresh_token_ttl_secs: i64,
    pub upload_dir: PathBuf,
    pub max_upload_bytes: usize,
    /// Directory with the built frontend. When set, the backend serves the SPA too.
    pub static_dir: Option<PathBuf>,
    /// Origins allowed by CORS (only needed when the frontend is served from another origin).
    pub cors_origins: Vec<String>,
    /// Set the `Secure` flag on the refresh cookie. Turn off only for plain-HTTP local development.
    pub cookie_secure: bool,
    /// Trust X-Forwarded-For / X-Real-IP (only enable behind a reverse proxy you control).
    pub trust_proxy: bool,
    /// Address people use to open NERP; used to build links in emails.
    pub public_url: String,
    pub smtp: SmtpConfig,
    /// Run the background job worker in this process.
    pub worker_enabled: bool,
    pub worker_concurrency: usize,
    pub docs: DocsConfig,
    /// Key for secrets stored in the database (mailbox passwords).
    pub data_key: [u8; 32],
    pub oauth: OAuthConfig,
    /// Let automation webhooks call addresses in private networks (loopback, LAN). Off by
    /// default so a robot cannot be used to probe the server's own network.
    pub webhook_allow_private: bool,
    /// ICE servers for video calls, as the browser's RTCIceServer list in JSON, e.g.
    /// `[{"urls":"turn:turn.example.com:3478","username":"u","credential":"p"}]`. Empty: calls
    /// connect directly, which works within one network.
    pub webrtc_ice_servers: serde_json::Value,
    /// LDAP_ENABLED: whether this server offers LDAP at all (sign-in, sync, its settings).
    pub ldap_enabled: bool,
    /// A LiveKit media server for calls (and recording them); None: browsers connect directly.
    pub livekit: Option<LiveKitConfig>,
}

#[derive(Clone, Debug)]
pub struct LiveKitConfig {
    /// What browsers connect to (ws:// or wss://).
    pub url: String,
    /// What NERP calls for the server API (http:// or https://); derived from `url` by default.
    pub api_url: String,
    pub api_key: String,
    pub api_secret: String,
    /// Where LiveKit Egress leaves recordings, as NERP sees it; None: no recording.
    pub recordings_dir: Option<PathBuf>,
    /// The same folder as Egress sees it (when Egress runs in a container); defaults to recordings_dir.
    pub egress_dir: Option<String>,
}

fn livekit_config() -> Result<Option<LiveKitConfig>> {
    let Some(url) = env::var("LIVEKIT_URL").ok().map(|s| s.trim().trim_end_matches('/').to_string()).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if !(url.starts_with("ws://") || url.starts_with("wss://")) {
        bail!("LIVEKIT_URL must start with ws:// or wss://");
    }
    let key = env::var("LIVEKIT_API_KEY").ok().filter(|s| !s.trim().is_empty()).context("LIVEKIT_API_KEY must be set with LIVEKIT_URL")?;
    let secret = env::var("LIVEKIT_API_SECRET").ok().filter(|s| !s.trim().is_empty()).context("LIVEKIT_API_SECRET must be set with LIVEKIT_URL")?;
    let api_url = env::var("LIVEKIT_API_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| url.replacen("ws", "http", 1));
    let recordings_dir: Option<PathBuf> = env::var("LIVEKIT_RECORDINGS_DIR").ok().filter(|s| !s.trim().is_empty()).map(|s| s.trim().into());
    let egress_dir = env::var("LIVEKIT_EGRESS_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .or_else(|| recordings_dir.as_ref().map(|d| d.to_string_lossy().into_owned()));
    Ok(Some(LiveKitConfig { url, api_url, api_key: key.trim().into(), api_secret: secret.trim().into(), recordings_dir, egress_dir }))
}

/// OAuth apps for connecting Gmail and Microsoft 365 mailboxes. A provider is offered only
/// when its client id and secret are set. Register the redirect URI
/// `{PUBLIC_URL}/api/crm/mailboxes/oauth/callback` with the provider.
#[derive(Clone, Debug, Default)]
pub struct OAuthConfig {
    pub google: Option<OAuthClient>,
    pub microsoft: Option<OAuthClient>,
    /// Azure tenant: "common" (any work or personal account), "organizations", or a tenant id.
    pub microsoft_tenant: String,
}

#[derive(Clone, Debug)]
pub struct OAuthClient {
    pub client_id: String,
    pub client_secret: String,
    /// Endpoint overrides (tests, or proxies); empty means the provider's own.
    pub auth_url: String,
    pub token_url: String,
}

fn oauth_client(prefix: &str) -> Option<OAuthClient> {
    let id = env::var(format!("{prefix}_CLIENT_ID")).ok().filter(|s| !s.trim().is_empty())?;
    let secret = env::var(format!("{prefix}_CLIENT_SECRET")).ok().filter(|s| !s.trim().is_empty())?;
    Some(OAuthClient {
        client_id: id.trim().to_string(),
        client_secret: secret.trim().to_string(),
        auth_url: var_or(&format!("{prefix}_OAUTH_AUTH_URL"), ""),
        token_url: var_or(&format!("{prefix}_OAUTH_TOKEN_URL"), ""),
    })
}

/// External document renderers (Typst is built in).
#[derive(Clone, Debug)]
pub struct DocsConfig {
    pub lualatex_bin: String,
    pub soffice_bin: String,
    pub timeout_secs: u64,
    /// External renderer processes allowed at once.
    pub concurrency: usize,
}

#[derive(Clone, Debug)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    /// none | starttls | tls
    pub security: String,
    /// Accept self-signed/invalid certificates (local development only).
    pub accept_invalid_certs: bool,
    pub from: String,
}

fn var_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn parse<T: std::str::FromStr>(name: &str, default: &str) -> Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    var_or(name, default)
        .parse()
        .with_context(|| format!("invalid value for {name}"))
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let jwt_secret = env::var("JWT_SECRET").context("JWT_SECRET must be set")?;
        if jwt_secret.len() < 32 {
            bail!("JWT_SECRET must be at least 32 characters");
        }
        let data_key = crate::crypto::key_from(&env::var("DATA_ENCRYPTION_KEY").ok().filter(|s| s.len() >= 32).unwrap_or_else(|| jwt_secret.clone()));
        Ok(Self {
            bind_addr: parse("BIND_ADDR", "127.0.0.1:8080")?,
            database_url: env::var("DATABASE_URL").context("DATABASE_URL must be set")?,
            database_max_connections: parse("DATABASE_MAX_CONNECTIONS", "20")?,
            redis_url: var_or("REDIS_URL", "redis://127.0.0.1:6379"),
            jwt_secret,
            access_token_ttl_secs: parse("ACCESS_TOKEN_TTL_SECS", "900")?,
            refresh_token_ttl_secs: parse("REFRESH_TOKEN_TTL_SECS", "2592000")?,
            upload_dir: var_or("UPLOAD_DIR", "./data/uploads").into(),
            max_upload_bytes: parse("MAX_UPLOAD_BYTES", "52428800")?,
            static_dir: env::var("STATIC_DIR").ok().filter(|s| !s.is_empty()).map(Into::into),
            cors_origins: var_or("CORS_ORIGINS", "")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
            cookie_secure: parse("COOKIE_SECURE", "true")?,
            trust_proxy: parse("TRUST_PROXY", "false")?,
            public_url: var_or("PUBLIC_URL", "http://localhost:5173").trim_end_matches('/').to_string(),
            smtp: SmtpConfig {
                host: var_or("SMTP_HOST", "127.0.0.1"),
                port: parse("SMTP_PORT", "1025")?,
                username: env::var("SMTP_USERNAME").ok().filter(|s| !s.is_empty()),
                password: env::var("SMTP_PASSWORD").ok().filter(|s| !s.is_empty()),
                security: {
                    let s = var_or("SMTP_SECURITY", "none").to_lowercase();
                    if !["none", "starttls", "tls"].contains(&s.as_str()) {
                        bail!("SMTP_SECURITY must be none, starttls or tls");
                    }
                    s
                },
                accept_invalid_certs: parse("SMTP_ACCEPT_INVALID_CERTS", "false")?,
                from: var_or("MAIL_FROM", "NERP <no-reply@nerp.local>"),
            },
            worker_enabled: parse("WORKER_ENABLED", "true")?,
            worker_concurrency: parse("WORKER_CONCURRENCY", "4")?,
            data_key,
            webhook_allow_private: parse("CRM_WEBHOOK_ALLOW_PRIVATE", "false")?,
            ldap_enabled: parse("LDAP_ENABLED", "false")?,
            livekit: livekit_config()?,
            webrtc_ice_servers: {
                let raw = var_or("WEBRTC_ICE_SERVERS", "[]");
                let v: serde_json::Value = serde_json::from_str(&raw).context("WEBRTC_ICE_SERVERS must be a JSON list")?;
                anyhow::ensure!(v.is_array(), "WEBRTC_ICE_SERVERS must be a JSON list");
                v
            },
            oauth: OAuthConfig {
                google: oauth_client("GOOGLE"),
                microsoft: oauth_client("MICROSOFT"),
                microsoft_tenant: var_or("MICROSOFT_TENANT", "common"),
            },
            docs: DocsConfig {
                lualatex_bin: var_or("LUALATEX_BIN", "lualatex"),
                soffice_bin: var_or("SOFFICE_BIN", "soffice"),
                timeout_secs: parse("DOC_RENDER_TIMEOUT_SECS", "60")?,
                concurrency: parse("DOC_RENDER_CONCURRENCY", "2")?,
            },
        })
    }
}
