//! CRM email: employees connect their own mailboxes (IMAP to receive, SMTP to send).
//! Messages with CRM clients are filed on their leads, contacts, companies and deals
//! (timeline + an email activity), and can be written and answered from the record.

pub(crate) mod imap;
pub(crate) mod oauth;
mod actions;
mod sync;

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use lettre::{
    message::{header::ContentType, Attachment, Mailbox, MultiPart, SinglePart},
    transport::smtp::{
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{
    access::{is_crm_admin, require},
    broadcast,
    items::load_row,
    timeline,
};
use crate::{
    auth::AuthUser,
    crypto,
    error::{AppError, AppResult},
    jobs,
    routes::{files, required},
    state::AppState,
};

pub const SYNC_JOB: &str = "crm.mail.sync";
pub const POLL_JOB: &str = "crm.mail.poll";
pub const SEND_JOB: &str = "crm.mail.send";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/mailboxes", get(list_mailboxes).post(create_mailbox))
        .route("/crm/mailboxes/test", post(test_mailbox))
        .route("/crm/mailboxes/oauth/providers", get(oauth::providers))
        .route("/crm/mailboxes/oauth/{kind}/start", post(oauth::start))
        .route("/crm/mailboxes/oauth/callback", get(oauth::callback))
        .route("/crm/mailboxes/{id}", axum::routing::patch(update_mailbox).delete(delete_mailbox))
        .route("/crm/mailboxes/{id}/sync", post(sync_now))
        .route("/crm/items/{id}/emails", get(item_emails).post(send_email))
        .route("/crm/items/{id}/email-recipients", get(recipients))
        .route("/crm/emails/{id}", get(get_email))
        .route("/crm/emails/{id}/resend", post(resend))
        .merge(actions::router())
}

// ---------------------------------------------------------------------------
// Mailboxes
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow, Clone)]
pub struct MailboxRow {
    pub id: Uuid,
    pub owner_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub imap_host: String,
    pub imap_port: i32,
    pub imap_security: String,
    pub smtp_host: String,
    pub smtp_port: i32,
    pub smtp_security: String,
    pub smtp_auth: bool,
    pub username: String,
    #[serde(skip)]
    pub secret: Vec<u8>,
    pub allow_invalid_certs: bool,
    pub shared_with: Vec<Uuid>,
    pub create_leads: bool,
    pub sync_days: i32,
    pub sync_enabled: bool,
    #[serde(skip)]
    pub folders: Value,
    pub sent_folder: Option<String>,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    /// password | google | microsoft
    pub auth_kind: String,
    #[serde(skip)]
    pub access_token: Option<Vec<u8>>,
    #[serde(skip)]
    pub access_expires_at: Option<DateTime<Utc>>,
}

const MAILBOX_COLUMNS: &str = "id, owner_id, email, display_name, imap_host, imap_port, imap_security, smtp_host, smtp_port, smtp_security, smtp_auth,
    username, secret, allow_invalid_certs, shared_with, create_leads, sync_days, sync_enabled, folders, sent_folder, last_sync_at, last_error, created_at,
    auth_kind, access_token, access_expires_at";

impl MailboxRow {
    pub fn password(&self, state: &AppState) -> Option<String> {
        crypto::open(&state.config.data_key, &self.secret)
    }
    fn imap<'a>(&'a self, secret: &'a str) -> imap::Settings<'a> {
        imap::Settings {
            host: &self.imap_host,
            port: self.imap_port as u16,
            security: &self.imap_security,
            username: &self.username,
            password: secret,
            allow_invalid_certs: self.allow_invalid_certs,
            xoauth2: self.is_oauth(),
        }
    }
    fn is_oauth(&self) -> bool {
        self.auth_kind != "password"
    }
    /// What to sign in with: the password, or a fresh OAuth access token.
    pub(crate) async fn credential(&self, state: &AppState) -> Result<String, String> {
        if self.is_oauth() {
            oauth::access_token(state, self).await
        } else {
            self.password(state).ok_or_else(|| "the stored password cannot be read; enter it again".to_string())
        }
    }
    pub(crate) fn smtp(&self, secret: &str) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
        let auth = (self.smtp_auth || self.is_oauth()).then_some((self.username.as_str(), secret));
        smtp_transport(&self.smtp_host, self.smtp_port as u16, &self.smtp_security, auth, self.allow_invalid_certs, self.is_oauth())
    }
    pub(crate) fn can_send(&self, user: Uuid) -> bool {
        self.owner_id == user || self.shared_with.contains(&user)
    }
}

pub(crate) async fn load_mailbox(state: &AppState, id: Uuid) -> AppResult<MailboxRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {MAILBOX_COLUMNS} FROM crm_mailboxes WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("mailbox"))
}

fn smtp_transport(host: &str, port: u16, security: &str, auth: Option<(&str, &str)>, allow_invalid: bool, xoauth2: bool) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
    let params = || TlsParameters::builder(host.to_string()).dangerous_accept_invalid_certs(allow_invalid).build().map_err(|e| e.to_string());
    let tls = match security {
        "tls" => Tls::Wrapper(params()?),
        "starttls" => Tls::Required(params()?),
        _ => Tls::None,
    };
    let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host).port(port).tls(tls).timeout(Some(std::time::Duration::from_secs(30)));
    if let Some((u, p)) = auth {
        b = b.credentials(Credentials::new(u.to_string(), p.to_string()));
        if xoauth2 {
            b = b.authentication(vec![lettre::transport::smtp::authentication::Mechanism::Xoauth2]);
        }
    }
    Ok(b.build())
}

#[derive(Deserialize, Clone)]
struct MailboxBody {
    email: Option<String>,
    display_name: Option<String>,
    imap_host: Option<String>,
    imap_port: Option<i32>,
    imap_security: Option<String>,
    smtp_host: Option<String>,
    smtp_port: Option<i32>,
    smtp_security: Option<String>,
    smtp_auth: Option<bool>,
    username: Option<String>,
    password: Option<String>,
    allow_invalid_certs: Option<bool>,
    shared_with: Option<Vec<Uuid>>,
    create_leads: Option<bool>,
    sync_days: Option<i32>,
    sync_enabled: Option<bool>,
}

/// Validated settings, merged over an existing mailbox when editing.
struct Settings {
    email: String,
    display_name: String,
    imap_host: String,
    imap_port: i32,
    imap_security: String,
    smtp_host: String,
    smtp_port: i32,
    smtp_security: String,
    smtp_auth: bool,
    username: String,
    password: String,
    allow_invalid_certs: bool,
    shared_with: Vec<Uuid>,
    create_leads: bool,
    sync_days: i32,
    sync_enabled: bool,
}

fn security(v: &str) -> AppResult<String> {
    if ["tls", "starttls", "none"].contains(&v) { Ok(v.into()) } else { Err(AppError::bad("security must be tls, starttls or none")) }
}
fn port(p: i32) -> AppResult<i32> {
    if (1..=65535).contains(&p) { Ok(p) } else { Err(AppError::bad("port must be between 1 and 65535")) }
}
fn host(h: &str) -> AppResult<String> {
    let h = h.trim();
    if h.is_empty() || h.len() > 253 || !h.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c)) {
        return Err(AppError::bad("enter a server name like imap.example.com"));
    }
    Ok(h.to_string())
}

fn settings(b: MailboxBody, cur: Option<(&MailboxRow, String)>) -> AppResult<Settings> {
    let c = cur.as_ref().map(|x| x.0);
    let email = b.email.or(c.map(|c| c.email.clone())).unwrap_or_default().trim().to_lowercase();
    if email.parse::<lettre::Address>().is_err() {
        return Err(AppError::bad("enter a valid email address"));
    }
    let imap_host = host(&b.imap_host.or(c.map(|c| c.imap_host.clone())).unwrap_or_default())?;
    let smtp_host = host(&b.smtp_host.or(c.map(|c| c.smtp_host.clone())).unwrap_or_default())?;
    let password = match b.password.filter(|p| !p.is_empty()) {
        Some(p) => p,
        None => cur.as_ref().map(|x| x.1.clone()).ok_or_else(|| AppError::bad("enter the password"))?,
    };
    let days = b.sync_days.or(c.map(|c| c.sync_days)).unwrap_or(7);
    if !(0..=365).contains(&days) {
        return Err(AppError::bad("import between 0 and 365 days of mail"));
    }
    Ok(Settings {
        display_name: b.display_name.or(c.map(|c| c.display_name.clone())).unwrap_or_default().trim().chars().take(200).collect(),
        username: required("username", &b.username.or(c.map(|c| c.username.clone())).unwrap_or_else(|| email.clone()), 320)?,
        email,
        imap_host,
        imap_port: port(b.imap_port.or(c.map(|c| c.imap_port)).unwrap_or(993))?,
        imap_security: security(&b.imap_security.or(c.map(|c| c.imap_security.clone())).unwrap_or_else(|| "tls".into()))?,
        smtp_host,
        smtp_port: port(b.smtp_port.or(c.map(|c| c.smtp_port)).unwrap_or(587))?,
        smtp_security: security(&b.smtp_security.or(c.map(|c| c.smtp_security.clone())).unwrap_or_else(|| "starttls".into()))?,
        smtp_auth: b.smtp_auth.or(c.map(|c| c.smtp_auth)).unwrap_or(true),
        password,
        allow_invalid_certs: b.allow_invalid_certs.or(c.map(|c| c.allow_invalid_certs)).unwrap_or(false),
        shared_with: b.shared_with.or(c.map(|c| c.shared_with.clone())).unwrap_or_default().into_iter().take(100).collect(),
        create_leads: b.create_leads.or(c.map(|c| c.create_leads)).unwrap_or(false),
        sync_days: days,
        sync_enabled: b.sync_enabled.or(c.map(|c| c.sync_enabled)).unwrap_or(true),
    })
}

/// Logs in to IMAP and opens an SMTP session; returns a message per side.
async fn check(s: &Settings) -> (Result<(), String>, Result<(), String>) {
    let imap = async {
        let mut session = imap::connect(&imap::Settings {
            host: &s.imap_host,
            port: s.imap_port as u16,
            security: &s.imap_security,
            username: &s.username,
            password: &s.password,
            allow_invalid_certs: s.allow_invalid_certs,
            xoauth2: false,
        })
        .await?;
        session.select("INBOX").await.map_err(|e| format!("cannot open INBOX: {e}"))?;
        let _ = session.logout().await;
        Ok(())
    };
    let smtp = async {
        let auth = s.smtp_auth.then_some((s.username.as_str(), s.password.as_str()));
        let t = smtp_transport(&s.smtp_host, s.smtp_port as u16, &s.smtp_security, auth, s.allow_invalid_certs, false)?;
        match t.test_connection().await {
            Ok(true) => Ok(()),
            Ok(false) => Err("the SMTP server refused the connection".to_string()),
            Err(e) => Err(format!("SMTP: {e}")),
        }
    };
    tokio::join!(imap, smtp)
}

fn check_json(r: &(Result<(), String>, Result<(), String>)) -> Value {
    json!({
        "imap": { "ok": r.0.is_ok(), "error": r.0.as_ref().err() },
        "smtp": { "ok": r.1.is_ok(), "error": r.1.as_ref().err() },
    })
}

async fn test_mailbox(auth: AuthUser, Json(b): Json<MailboxBody>) -> AppResult<Json<Value>> {
    let _ = auth;
    let s = settings(b, None)?;
    Ok(Json(check_json(&check(&s).await)))
}

async fn list_mailboxes(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let rows: Vec<MailboxRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {MAILBOX_COLUMNS} FROM crm_mailboxes WHERE owner_id = $1 OR $1 = ANY(shared_with) ORDER BY email"
    )))
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|m| {
            let mut v = serde_json::to_value(m).unwrap();
            v["is_owner"] = json!(m.owner_id == auth.id);
            v
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

async fn create_mailbox(State(state): State<AppState>, auth: AuthUser, Json(b): Json<MailboxBody>) -> AppResult<Json<Value>> {
    let s = settings(b, None)?;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crm_mailboxes WHERE owner_id = $1").bind(auth.id).fetch_one(&state.db).await?;
    if n >= 10 {
        return Err(AppError::bad("at most 10 mailboxes per person"));
    }
    let result = check(&s).await;
    if result.0.is_err() || result.1.is_err() {
        return Err(AppError::bad(format!(
            "could not connect: {}",
            [result.0.err().map(|e| format!("IMAP {e}")), result.1.err()].into_iter().flatten().collect::<Vec<_>>().join("; ")
        )));
    }
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO crm_mailboxes (owner_id, email, display_name, imap_host, imap_port, imap_security, smtp_host, smtp_port, smtp_security, smtp_auth,
             username, secret, allow_invalid_certs, shared_with, create_leads, sync_days, sync_enabled)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17) RETURNING id",
    )
    .bind(auth.id)
    .bind(&s.email)
    .bind(&s.display_name)
    .bind(&s.imap_host)
    .bind(s.imap_port)
    .bind(&s.imap_security)
    .bind(&s.smtp_host)
    .bind(s.smtp_port)
    .bind(&s.smtp_security)
    .bind(s.smtp_auth)
    .bind(&s.username)
    .bind(crypto::seal(&state.config.data_key, &s.password))
    .bind(s.allow_invalid_certs)
    .bind(&s.shared_with)
    .bind(s.create_leads)
    .bind(s.sync_days)
    .bind(s.sync_enabled)
    .fetch_one(&state.db)
    .await?;
    crate::audit::record(&state.db, Some(auth.id), "crm.mailbox.connect", "crm_mailbox", Some(id.to_string()), json!({ "email": s.email }), None).await;
    queue_sync(&state, id).await?;
    let m = load_mailbox(&state, id).await?;
    Ok(Json(serde_json::to_value(m).unwrap()))
}

async fn update_mailbox(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<MailboxBody>) -> AppResult<Json<Value>> {
    let cur = load_mailbox(&state, id).await?;
    if cur.owner_id != auth.id {
        return Err(AppError::NotFound("mailbox"));
    }
    if cur.is_oauth() {
        // Servers and sign-in come from the provider; only the options can change.
        let days = b.sync_days.unwrap_or(cur.sync_days);
        if !(0..=365).contains(&days) {
            return Err(AppError::bad("import between 0 and 365 days of mail"));
        }
        sqlx::query(
            "UPDATE crm_mailboxes SET display_name = $2, shared_with = $3, create_leads = $4, sync_days = $5, sync_enabled = $6, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(b.display_name.unwrap_or(cur.display_name).trim().chars().take(200).collect::<String>())
        .bind(b.shared_with.unwrap_or(cur.shared_with).into_iter().take(100).collect::<Vec<_>>())
        .bind(b.create_leads.unwrap_or(cur.create_leads))
        .bind(days)
        .bind(b.sync_enabled.unwrap_or(cur.sync_enabled))
        .execute(&state.db)
        .await?;
        return Ok(Json(serde_json::to_value(load_mailbox(&state, id).await?).unwrap()));
    }
    let connection_changed = b.password.as_ref().is_some_and(|p| !p.is_empty())
        || b.imap_host.is_some() || b.smtp_host.is_some() || b.imap_port.is_some() || b.smtp_port.is_some()
        || b.imap_security.is_some() || b.smtp_security.is_some() || b.username.is_some() || b.smtp_auth.is_some();
    let old_password = cur.password(&state).unwrap_or_default();
    let s = settings(b, Some((&cur, old_password)))?;
    if connection_changed {
        let r = check(&s).await;
        if r.0.is_err() || r.1.is_err() {
            return Err(AppError::bad(format!("could not connect: {}", [r.0.err().map(|e| format!("IMAP {e}")), r.1.err()].into_iter().flatten().collect::<Vec<_>>().join("; "))));
        }
    }
    // A different server or account means a fresh start for folder positions.
    let reset = s.imap_host != cur.imap_host || s.username != cur.username;
    sqlx::query(
        "UPDATE crm_mailboxes SET email = $2, display_name = $3, imap_host = $4, imap_port = $5, imap_security = $6, smtp_host = $7, smtp_port = $8,
             smtp_security = $9, smtp_auth = $10, username = $11, secret = $12, allow_invalid_certs = $13, shared_with = $14, create_leads = $15,
             sync_days = $16, sync_enabled = $17, last_error = NULL,
             folders = CASE WHEN $18 THEN '{}'::jsonb ELSE folders END, sent_folder = CASE WHEN $18 THEN NULL ELSE sent_folder END, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(&s.email)
    .bind(&s.display_name)
    .bind(&s.imap_host)
    .bind(s.imap_port)
    .bind(&s.imap_security)
    .bind(&s.smtp_host)
    .bind(s.smtp_port)
    .bind(&s.smtp_security)
    .bind(s.smtp_auth)
    .bind(&s.username)
    .bind(crypto::seal(&state.config.data_key, &s.password))
    .bind(s.allow_invalid_certs)
    .bind(&s.shared_with)
    .bind(s.create_leads)
    .bind(s.sync_days)
    .bind(s.sync_enabled)
    .bind(reset)
    .execute(&state.db)
    .await?;
    Ok(Json(serde_json::to_value(load_mailbox(&state, id).await?).unwrap()))
}

async fn delete_mailbox(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let cur = load_mailbox(&state, id).await?;
    if cur.owner_id != auth.id && !is_crm_admin(&auth) {
        return Err(AppError::NotFound("mailbox"));
    }
    // Filed emails stay on their records.
    sqlx::query("DELETE FROM crm_mailboxes WHERE id = $1").bind(id).execute(&state.db).await?;
    crate::audit::record(&state.db, Some(auth.id), "crm.mailbox.disconnect", "crm_mailbox", Some(id.to_string()), json!({ "email": cur.email }), None).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Syncing (background jobs)
// ---------------------------------------------------------------------------

/// Queues a sync unless one is already queued or running for this mailbox.
async fn queue_sync(state: &AppState, id: Uuid) -> AppResult<bool> {
    let mut redis = state.redis.clone();
    let got: Option<String> = redis::cmd("SET").arg(format!("crm:mail:sync:{id}")).arg("1").arg("NX").arg("EX").arg(600).query_async(&mut redis).await?;
    if got.is_some() {
        jobs::enqueue(&state.db, SYNC_JOB, json!({ "mailbox_id": id }), None).await?;
        state.job_wakeup.notify_one();
    } else {
        // A sync is already running: run once more when it ends, so nothing waits for
        // the next poll.
        let _: () = redis.set_ex(format!("crm:mail:again:{id}"), 1, 600).await?;
    }
    Ok(got.is_some())
}

async fn sync_now(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let m = load_mailbox(&state, id).await?;
    if !m.can_send(auth.id) {
        return Err(AppError::NotFound("mailbox"));
    }
    Ok(Json(json!({ "queued": queue_sync(&state, id).await? })))
}

/// Recurring: queue a sync for every enabled mailbox.
pub async fn run_poll(state: &AppState) -> anyhow::Result<()> {
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM crm_mailboxes WHERE sync_enabled").fetch_all(&state.db).await?;
    for id in ids {
        queue_sync(state, id).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    }
    Ok(())
}

pub async fn run_sync(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let id: Uuid = serde_json::from_value(payload["mailbox_id"].clone())?;
    let result = async {
        let mb = match load_mailbox(state, id).await {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };
        let r = sync::sync(state, &mb).await;
        sqlx::query("UPDATE crm_mailboxes SET last_sync_at = now(), last_error = $2 WHERE id = $1")
            .bind(id)
            .bind(r.as_ref().err())
            .execute(&state.db)
            .await?;
        match r {
            Ok(rep) if rep.filed > 0 => tracing::info!(mailbox = %id, fetched = rep.fetched, filed = rep.filed, "mail synced"),
            Err(e) => tracing::warn!(mailbox = %id, error = e, "mail sync failed"),
            _ => {}
        }
        anyhow::Ok(())
    }
    .await;
    let mut redis = state.redis.clone();
    let _: Result<(), _> = redis.del(format!("crm:mail:sync:{id}")).await;
    let again: Option<i64> = redis.get_del(format!("crm:mail:again:{id}")).await.unwrap_or(None);
    if again.is_some() {
        queue_sync(state, id).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    }
    result
}

// ---------------------------------------------------------------------------
// Filing on records
// ---------------------------------------------------------------------------

/// Links an email to records: timeline entries, and one activity on the main record
/// (a to-do for incoming mail; done for outgoing, which also closes the conversation's
/// open incoming email activities).
async fn file_on_records(state: &AppState, email: Uuid, items: &[Uuid], author: Option<Uuid>) -> AppResult<()> {
    let e: (String, String, String, String, Uuid, Option<Uuid>) = sqlx::query_as(
        "SELECT direction, subject, from_addr, snippet, thread_id, mailbox_id FROM crm_emails WHERE id = $1",
    )
    .bind(email)
    .fetch_one(&state.db)
    .await?;
    let (direction, subject, from, snippet, thread, mailbox) = e;
    let owner: Option<Uuid> = match mailbox {
        Some(m) => sqlx::query_scalar("SELECT owner_id FROM crm_mailboxes WHERE id = $1").bind(m).fetch_optional(&state.db).await?,
        None => None,
    };
    let mut primary: Option<(Uuid, i16, Option<Uuid>, i32)> = None;
    for item in items {
        sqlx::query("INSERT INTO crm_email_links (email_id, item_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(email).bind(item).execute(&state.db).await?;
        timeline(&state.db, *item, "email", author, Some(subject.clone()), json!({ "email_id": email, "direction": direction, "from": from, "snippet": snippet })).await?;
        sqlx::query("UPDATE crm_items SET last_activity_at = now() WHERE id = $1").bind(item).execute(&state.db).await?;
        let row = load_row(state, *item, false).await.ok();
        if let Some(r) = row {
            // Prefer an open pipeline record (deal, lead, smart process) over a contact/company.
            let rank = if r.stage_id.is_some() && !r.is_closed { 2 } else { 1 };
            if primary.is_none_or(|p| rank > p.3) {
                primary = Some((r.id, r.entity_type, r.assigned_to, rank));
            }
            broadcast(state, r.entity_type, r.id, false).await;
        }
    }
    if let Some((item, _, assigned, _)) = primary {
        let incoming = direction == "in";
        if let Some(responsible) = assigned.or(owner).or(author) {
            sqlx::query(
                "INSERT INTO crm_activities (item_id, kind, subject, direction, responsible_id, created_by, email_id, is_completed, completed_at, deadline)
                 VALUES ($1, 'email', $2, $3, $4, $5, $6, $7, CASE WHEN $7 THEN now() END, CASE WHEN NOT $7 THEN now() + interval '1 day' END)",
            )
            .bind(item)
            .bind(if subject.is_empty() { "(no subject)".to_string() } else { subject.clone() })
            .bind(if incoming { "incoming" } else { "outgoing" })
            .bind(responsible)
            .bind(author.unwrap_or(super::automation::SYSTEM_USER))
            .bind(email)
            .bind(!incoming)
            .execute(&state.db)
            .await?;
        }
        if !incoming {
            sqlx::query(
                "UPDATE crm_activities SET is_completed = TRUE, completed_at = now(), result = 'replied', updated_at = now()
                 WHERE NOT is_completed AND email_id IN (SELECT id FROM crm_emails WHERE thread_id = $1 AND direction = 'in')",
            )
            .bind(thread)
            .execute(&state.db)
            .await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct EmailSummary {
    id: Uuid,
    mailbox_id: Option<Uuid>,
    direction: String,
    status: String,
    thread_id: Uuid,
    from_addr: String,
    from_name: String,
    to_addrs: Value,
    cc_addrs: Value,
    subject: String,
    snippet: String,
    attachments: Value,
    sent_at: DateTime<Utc>,
    error: Option<String>,
    created_by: Option<Uuid>,
}

const SUMMARY: &str = "e.id, e.mailbox_id, e.direction, e.status, e.thread_id, e.from_addr, e.from_name, e.to_addrs, e.cc_addrs, e.subject, e.snippet, e.attachments, e.sent_at, e.error, e.created_by";

async fn item_emails(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = load_row(&state, item, false).await?;
    require(&state, &auth, row.entity_type, "read", row.assigned_to).await?;
    let items: Vec<EmailSummary> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {SUMMARY} FROM crm_emails e JOIN crm_email_links l ON l.email_id = e.id WHERE l.item_id = $1 ORDER BY e.sent_at DESC LIMIT 200"
    )))
    .bind(item)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items })))
}

/// Addresses to suggest when writing from a record: its own, its contacts' and its
/// company's emails.
async fn recipients(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = load_row(&state, item, false).await?;
    require(&state, &auth, row.entity_type, "read", row.assigned_to).await?;
    let rows: Vec<(String, String, i32)> = sqlx::query_as(
        "SELECT m.value, i.title, CASE WHEN i.id = $1 THEN 0 WHEN i.entity_type = 3 THEN 1 ELSE 2 END AS rank
         FROM crm_multifields m JOIN crm_items i ON i.id = m.item_id
         WHERE m.kind = 'email' AND i.deleted_at IS NULL AND (
             i.id = $1
             OR i.id IN (SELECT contact_id FROM crm_item_contacts WHERE item_id = $1)
             OR i.id = (SELECT company_id FROM crm_items WHERE id = $1))
         ORDER BY rank, m.sort LIMIT 50",
    )
    .bind(item)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.iter().map(|(a, n, _)| json!({ "address": a, "name": n })).collect::<Vec<_>>() })))
}

/// Readable by whoever can read a record it is filed on, its sender, or the mailbox owner.
async fn can_read_email(state: &AppState, auth: &AuthUser, email: Uuid) -> AppResult<bool> {
    let (created_by, owner): (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT e.created_by, m.owner_id FROM crm_emails e LEFT JOIN crm_mailboxes m ON m.id = e.mailbox_id WHERE e.id = $1",
    )
    .bind(email)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("email"))?;
    if created_by == Some(auth.id) || owner == Some(auth.id) {
        return Ok(true);
    }
    let items: Vec<Uuid> = sqlx::query_scalar("SELECT item_id FROM crm_email_links WHERE email_id = $1").bind(email).fetch_all(&state.db).await?;
    for i in items {
        if let Ok(r) = load_row(state, i, false).await
            && require(state, auth, r.entity_type, "read", r.assigned_to).await.is_ok() {
                return Ok(true);
            }
    }
    Ok(false)
}

async fn get_email(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    if !can_read_email(&state, &auth, id).await? {
        return Err(AppError::NotFound("email"));
    }
    let (summary, text, html, message_id): (Value, Option<String>, Option<String>, String) = {
        let s: EmailSummary = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {SUMMARY} FROM crm_emails e WHERE e.id = $1"))).bind(id).fetch_one(&state.db).await?;
        let (t, h, m): (Option<String>, Option<String>, String) = sqlx::query_as("SELECT body_text, body_html, message_id FROM crm_emails WHERE id = $1").bind(id).fetch_one(&state.db).await?;
        (serde_json::to_value(s).unwrap(), t, h, m)
    };
    let mut v = summary;
    v["body_text"] = json!(text);
    v["body_html"] = json!(html);
    v["message_id"] = json!(message_id);
    Ok(Json(v))
}

/// Files module hook: attachments are readable like their email.
pub async fn can_view_file(state: &AppState, auth: &AuthUser, file: Uuid) -> AppResult<bool> {
    let email: Option<Uuid> = sqlx::query_scalar("SELECT email_id FROM crm_email_files WHERE file_id = $1").bind(file).fetch_optional(&state.db).await?;
    match email {
        Some(e) => can_read_email(state, auth, e).await,
        None => Ok(false),
    }
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SendBody {
    mailbox_id: Uuid,
    #[serde(default)]
    to: Vec<String>,
    #[serde(default)]
    cc: Vec<String>,
    subject: String,
    body: String,
    reply_to: Option<Uuid>,
    #[serde(default)]
    file_ids: Vec<Uuid>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn parse_addrs(list: &[String]) -> AppResult<Vec<(String, String)>> {
    list.iter()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .map(|a| {
            let m: Mailbox = a.parse().map_err(|_| AppError::bad(format!("{a} is not a valid email address")))?;
            Ok((m.email.to_string().to_lowercase(), m.name.unwrap_or_default()))
        })
        .collect()
}

/// Queues an email from a mailbox, filed on `item` (and on the conversation's records when
/// replying). Used by the API and automation robots.
pub async fn queue_send(
    state: &AppState,
    sender: Uuid,
    mailbox: &MailboxRow,
    item: Uuid,
    to: &[String],
    cc: &[String],
    subject: &str,
    body: &str,
    reply_to: Option<Uuid>,
    file_ids: &[Uuid],
) -> AppResult<Uuid> {
    let to = parse_addrs(to)?;
    let cc = parse_addrs(cc)?;
    if to.is_empty() {
        return Err(AppError::bad("add at least one recipient"));
    }
    if to.len() + cc.len() > 50 {
        return Err(AppError::bad("at most 50 recipients"));
    }
    let subject = required("subject", subject, 500)?;
    if body.chars().count() > 200_000 {
        return Err(AppError::bad("the message is too long"));
    }
    let id = Uuid::new_v4();
    let domain = mailbox.email.rsplit('@').next().unwrap_or("nerp.local").to_string();
    let message_id = format!("{}@{domain}", id.simple());
    let (thread_id, in_reply_to, refs, mut links, quoted) = match reply_to {
        Some(r) => {
            let (t, mid, mut refs, from, sent, text): (Uuid, String, Vec<String>, String, DateTime<Utc>, Option<String>) =
                sqlx::query_as("SELECT thread_id, message_id, refs, from_addr, sent_at, body_text FROM crm_emails WHERE id = $1")
                    .bind(r)
                    .fetch_optional(&state.db)
                    .await?
                    .ok_or_else(|| AppError::bad("the message you reply to does not exist"))?;
            refs.push(mid.clone());
            let links: Vec<Uuid> = sqlx::query_scalar("SELECT item_id FROM crm_email_links WHERE email_id = $1").bind(r).fetch_all(&state.db).await?;
            let q = text.map(|t| format!("\n\nOn {}, {from} wrote:\n{}", sent.format("%Y-%m-%d %H:%M UTC"), t.lines().take(200).map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n")));
            (t, Some(mid), refs, links, q)
        }
        None => (id, None, vec![], vec![], None),
    };
    if !links.contains(&item) {
        links.insert(0, item);
    }
    let text = format!("{body}{}", quoted.unwrap_or_default());
    let html = format!(
        "<div style=\"font-family:Arial,sans-serif;font-size:14px;line-height:1.5\">{}</div>",
        esc(&text).replace('\n', "<br>")
    );
    // Attachments: the sender's own uploads, or documents generated for this record.
    let mut atts = Vec::new();
    for f in file_ids.iter().take(20) {
        let (name, mime, owner, bytes) = files::read_bytes(state, *f).await?;
        let doc: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_documents WHERE item_id = $1 AND (pdf_file_id = $2 OR source_file_id = $2))")
            .bind(item)
            .bind(f)
            .fetch_one(&state.db)
            .await?;
        if owner != Some(sender) && !doc {
            return Err(AppError::NotFound("file"));
        }
        atts.push(json!({ "file_id": f, "name": name, "size": bytes.len(), "mime": mime }));
    }
    let from_name = if mailbox.display_name.is_empty() {
        sqlx::query_scalar("SELECT first_name || ' ' || last_name FROM users WHERE id = $1").bind(sender).fetch_optional(&state.db).await?.unwrap_or_default()
    } else {
        mailbox.display_name.clone()
    };
    sqlx::query(
        "INSERT INTO crm_emails (id, mailbox_id, direction, status, message_id, in_reply_to, refs, thread_id, from_addr, from_name, to_addrs, cc_addrs,
             subject, body_text, body_html, snippet, attachments, created_by)
         VALUES ($1, $2, 'out', 'queued', $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
    )
    .bind(id)
    .bind(mailbox.id)
    .bind(&message_id)
    .bind(&in_reply_to)
    .bind(&refs)
    .bind(thread_id)
    .bind(&mailbox.email)
    .bind(&from_name)
    .bind(json!(to.iter().map(|(a, n)| json!({ "address": a, "name": n })).collect::<Vec<_>>()))
    .bind(json!(cc.iter().map(|(a, n)| json!({ "address": a, "name": n })).collect::<Vec<_>>()))
    .bind(&subject)
    .bind(&text)
    .bind(&html)
    .bind(sync::snippet(&text))
    .bind(Value::Array(atts))
    .bind(sender)
    .execute(&state.db)
    .await?;
    for l in &links {
        sqlx::query("INSERT INTO crm_email_links (email_id, item_id) VALUES ($1, $2) ON CONFLICT DO NOTHING").bind(id).bind(l).execute(&state.db).await?;
    }
    jobs::enqueue(&state.db, SEND_JOB, json!({ "email_id": id }), None).await?;
    state.job_wakeup.notify_one();
    Ok(id)
}

async fn send_email(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>, Json(b): Json<SendBody>) -> AppResult<Json<Value>> {
    let row = load_row(&state, item, false).await?;
    require(&state, &auth, row.entity_type, "read", row.assigned_to).await?;
    let mb = load_mailbox(&state, b.mailbox_id).await?;
    if !mb.can_send(auth.id) {
        return Err(AppError::NotFound("mailbox"));
    }
    let id = queue_send(&state, auth.id, &mb, item, &b.to, &b.cc, &b.subject, &b.body, b.reply_to, &b.file_ids).await?;
    get_email(State(state), auth, Path(id)).await
}

async fn resend(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (status, created_by): (String, Option<Uuid>) = sqlx::query_as("SELECT status, created_by FROM crm_emails WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("email"))?;
    if created_by != Some(auth.id) {
        return Err(AppError::NotFound("email"));
    }
    if status != "failed" {
        return Err(AppError::bad("only failed messages can be sent again"));
    }
    sqlx::query("UPDATE crm_emails SET status = 'queued', error = NULL WHERE id = $1").bind(id).execute(&state.db).await?;
    jobs::enqueue(&state.db, SEND_JOB, json!({ "email_id": id }), None).await?;
    state.job_wakeup.notify_one();
    get_email(State(state), auth, Path(id)).await
}

/// Background job: deliver a queued email through its mailbox's SMTP server, then keep a
/// copy in the mailbox's sent folder.
pub async fn run_send(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let id: Uuid = serde_json::from_value(payload["email_id"].clone())?;
    let row: Option<(Option<Uuid>, String, String, String, Option<String>, Vec<String>, String, String, Value, Value, String, String, Value, Option<Uuid>)> = sqlx::query_as(
        "SELECT mailbox_id, status, from_addr, from_name, in_reply_to, refs, message_id, subject, to_addrs, cc_addrs, body_text, body_html, attachments, created_by
         FROM crm_emails WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let Some((mailbox, status, from, from_name, in_reply_to, refs, message_id, subject, to, cc, text, html, atts, created_by)) = row else { return Ok(()) };
    if status != "queued" {
        return Ok(());
    }
    let result: Result<(), String> = async {
        let mb = load_mailbox(state, mailbox.ok_or("the mailbox was removed")?).await.map_err(|_| "the mailbox was removed".to_string())?;
        let password = mb.credential(state).await?;
        let mbox = |v: &Value| -> Result<Mailbox, String> {
            let a = v["address"].as_str().unwrap_or("");
            let n = v["name"].as_str().filter(|n| !n.is_empty()).map(String::from);
            Ok(Mailbox::new(n, a.parse().map_err(|_| format!("{a} is not a valid address"))?))
        };
        let mut b = Message::builder()
            .from(Mailbox::new(Some(from_name.clone()).filter(|n| !n.is_empty()), from.parse().map_err(|_| "invalid sender address".to_string())?))
            .subject(&subject)
            .message_id(Some(format!("<{message_id}>")));
        for t in to.as_array().into_iter().flatten() {
            b = b.to(mbox(t)?);
        }
        for c in cc.as_array().into_iter().flatten() {
            b = b.cc(mbox(c)?);
        }
        if let Some(r) = &in_reply_to {
            b = b.in_reply_to(format!("<{r}>"));
        }
        if !refs.is_empty() {
            b = b.references(refs.iter().map(|r| format!("<{r}>")).collect::<Vec<_>>().join(" "));
        }
        let alt = MultiPart::alternative_plain_html(text.clone(), html.clone());
        let mut files_list = Vec::new();
        for a in atts.as_array().into_iter().flatten() {
            let fid: Uuid = serde_json::from_value(a["file_id"].clone()).map_err(|e| e.to_string())?;
            let (name, mime, _, bytes) = files::read_bytes(state, fid).await.map_err(|_| "an attachment is missing".to_string())?;
            files_list.push((name, mime, bytes));
        }
        let message = if files_list.is_empty() {
            b.multipart(alt)
        } else {
            let mut mixed = MultiPart::mixed().multipart(alt);
            for (name, mime, bytes) in files_list {
                let ct = ContentType::parse(&mime).unwrap_or(ContentType::parse("application/octet-stream").unwrap());
                let part: SinglePart = Attachment::new(name).body(bytes, ct);
                mixed = mixed.singlepart(part);
            }
            b.multipart(mixed)
        }
        .map_err(|e| e.to_string())?;
        let raw = message.formatted();
        mb.smtp(&password)?
            .send(message)
            .await
            .map_err(|e| format!("SMTP: {e}"))?;
        // Best effort: a copy in the sent folder, like a mail program would keep.
        if let Ok(mut session) = imap::connect(&mb.imap(&password)).await {
            let folder = mb.sent_folder.clone().unwrap_or_else(|| "Sent".into());
            if mb.sent_folder.is_none() {
                let _ = session.create(&folder).await;
                let _ = sqlx::query("UPDATE crm_mailboxes SET sent_folder = $2 WHERE id = $1").bind(mb.id).bind(&folder).execute(&state.db).await;
            }
            if let Err(e) = session.append(&folder, Some("(\\Seen)"), None, &raw).await {
                tracing::debug!(error = %e, "could not keep a copy in the sent folder");
            }
            let _ = session.logout().await;
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            sqlx::query("UPDATE crm_emails SET status = 'sent', sent_at = now(), error = NULL WHERE id = $1").bind(id).execute(&state.db).await?;
            let links: Vec<Uuid> = sqlx::query_scalar("SELECT item_id FROM crm_email_links WHERE email_id = $1").bind(id).fetch_all(&state.db).await?;
            file_on_records(state, id, &links, created_by).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
        }
        Err(e) => {
            sqlx::query("UPDATE crm_emails SET status = 'failed', error = $2 WHERE id = $1").bind(id).bind(&e).execute(&state.db).await?;
            let links: Vec<Uuid> = sqlx::query_scalar("SELECT item_id FROM crm_email_links WHERE email_id = $1").bind(id).fetch_all(&state.db).await?;
            for l in links {
                if let Ok(r) = load_row(state, l, false).await {
                    broadcast(state, r.entity_type, l, false).await;
                }
            }
            if let Some(u) = created_by {
                crate::notify::notify(state, &[u], crate::notify::NewNotification {
                    kind: "crm.email_failed",
                    title: format!("Your email “{subject}” was not sent"),
                    body: Some(e.clone()),
                    link: None,
                    actor: None,
                })
                .await;
            }
        }
    }
    Ok(())
}

/// Sends from a mailbox as its owner (web form auto-replies).
pub async fn send_from_mailbox(state: &AppState, mailbox: Uuid, item: Uuid, to: &str, subject: &str, body: &str) -> AppResult<Uuid> {
    let mb = load_mailbox(state, mailbox).await?;
    queue_send(state, mb.owner_id, &mb, item, &[to.to_string()], &[], subject, body, None, &[]).await
}

/// The first mailbox a user may send from (for automation robots).
pub async fn mailbox_of(state: &AppState, user: Uuid) -> AppResult<Option<MailboxRow>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {MAILBOX_COLUMNS} FROM crm_mailboxes WHERE owner_id = $1 ORDER BY created_at LIMIT 1"
    )))
    .bind(user)
    .fetch_optional(&state.db)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::sync::{sanitize_html, snippet};

    #[test]
    fn sanitizes() {
        let h = sanitize_html(r#"<p style="color:red" onclick="x()">Hi<script>alert(1)</script><img src="https://t.example/p.gif"><a href="javascript:x()">j</a><a href="https://ok.example">ok</a></p>"#);
        assert!(h.contains("style=\"color:red\"") && !h.contains("onclick") && !h.contains("script") && !h.contains("javascript"), "{h}");
        assert!(h.contains("rel=\"noopener noreferrer\""), "{h}");
        assert_eq!(snippet("  Hello \n\n  world  "), "Hello world");
    }
}
