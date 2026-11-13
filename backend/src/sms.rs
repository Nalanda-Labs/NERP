//! Text messages (SMS) through Twilio.
//!
//! The account comes from the environment (`TWILIO_ACCOUNT_SID`, `TWILIO_AUTH_TOKEN`; never
//! from the database), the sender from System → Text messages (a Twilio number, or a
//! Messaging Service SID `MG…`), falling back to `TWILIO_FROM`. `TWILIO_API_URL` points the
//! client somewhere else (the test suites' stand-in).
//!
//! Every message is a row in `sms_messages`: sending queues it and a job hands it to Twilio,
//! which reports delivery to `/api/public/twilio/sms/status`. Replies arrive at
//! `/api/public/twilio/sms/inbound` and land on the CRM record with that phone number. Both
//! webhooks check Twilio's signature (HMAC-SHA1 of the URL and parameters with the auth token).

use std::{collections::BTreeMap, sync::OnceLock, time::Duration};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
    Form, Json, Router,
};
use base64::Engine;
use hmac::{KeyInit, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::types::chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    crm::{self, access, automation::SYSTEM_USER},
    error::{AppError, AppResult},
    jobs,
    notify::{notify, NewNotification},
    state::AppState,
};

pub const SEND_JOB: &str = "sms.send";
/// Longest message accepted (Twilio's limit; it splits it into segments).
pub const MAX_LEN: usize = 1600;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sms/available", get(available))
        .route("/sms/setup", get(get_setup).put(put_setup))
        .route("/sms/test", post(send_test))
        .route("/sms/messages", get(list_messages))
        .route("/crm/items/{id}/sms", get(item_thread).post(send_from_item))
        .route("/public/twilio/sms/status", post(status_hook))
        .route("/public/twilio/sms/inbound", post(inbound_hook))
}

// ---------------------------------------------------------------------------
// Account
// ---------------------------------------------------------------------------

struct Account {
    sid: String,
    token: String,
    api: String,
}

fn account() -> Option<&'static Account> {
    static A: OnceLock<Option<Account>> = OnceLock::new();
    A.get_or_init(|| {
        let sid = std::env::var("TWILIO_ACCOUNT_SID").ok().filter(|s| !s.trim().is_empty())?;
        let token = std::env::var("TWILIO_AUTH_TOKEN").ok().filter(|s| !s.trim().is_empty())?;
        let api = std::env::var("TWILIO_API_URL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "https://api.twilio.com".into());
        Some(Account { sid: sid.trim().into(), token: token.trim().into(), api: api.trim_end_matches('/').into() })
    })
    .as_ref()
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        let tls = crate::crm::email::imap::client_config(false).expect("TLS set-up");
        reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("NERP")
            .build()
            .expect("HTTP client")
    })
}

/// Sender and number format, from System → Text messages.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Setup {
    /// A Twilio phone number (+…) or a Messaging Service SID (MG…). Empty: TWILIO_FROM.
    #[serde(default)]
    pub from: String,
    /// Calling code for numbers written without one ("+91"). Empty: numbers need a "+".
    #[serde(default)]
    pub country_code: String,
}

async fn setup(state: &AppState) -> Setup {
    let v: Option<Value> = sqlx::query_scalar!("SELECT value FROM settings WHERE key = 'sms'").fetch_optional(&state.db).await.ok().flatten();
    let mut s: Setup = v.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    if s.from.is_empty() {
        s.from = std::env::var("TWILIO_FROM").unwrap_or_default().trim().to_string();
    }
    s
}

/// The calling code added to numbers written without one (System → Text messages).
pub async fn country_code(state: &AppState) -> String {
    setup(state).await.country_code
}

/// Whether text messages can be sent at all (account and sender set).
pub async fn enabled(state: &AppState) -> bool {
    account().is_some() && !setup(state).await.from.is_empty()
}

/// A phone number in E.164 (+ and 8–15 digits), adding the default calling code to numbers
/// written without one. None: not a usable number.
pub fn normalize(number: &str, country_code: &str) -> Option<String> {
    let t = number.trim();
    let plus = t.starts_with('+') || t.starts_with("00");
    let mut digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    if t.starts_with("00") {
        digits = digits[2..].to_string();
    }
    if !plus {
        let cc: String = country_code.chars().filter(|c| c.is_ascii_digit()).collect();
        if cc.is_empty() {
            return None;
        }
        // A national number: drop its trunk prefix ("0…").
        digits = format!("{cc}{}", digits.trim_start_matches('0'));
    }
    (8..=15).contains(&digits.len()).then(|| format!("+{digits}"))
}

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct NewSms<'a> {
    pub to: &'a str,
    pub body: &'a str,
    pub purpose: &'a str,
    pub item_id: Option<Uuid>,
    pub booking_id: Option<Uuid>,
    pub campaign_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
}

/// Queues a message: checks the number and text, stores it and wakes the worker.
pub async fn queue(state: &AppState, m: NewSms<'_>) -> AppResult<Uuid> {
    let s = setup(state).await;
    let to = normalize(m.to, &s.country_code).ok_or_else(|| {
        AppError::bad(if s.country_code.is_empty() {
            format!("“{}” is not a phone number with its country code (such as +44 20 7946 0000)", m.to.trim())
        } else {
            format!("“{}” is not a phone number", m.to.trim())
        })
    })?;
    let body = m.body.trim();
    if body.is_empty() {
        return Err(AppError::bad("write a message"));
    }
    if body.chars().count() > MAX_LEN {
        return Err(AppError::bad(format!("a text message can be at most {MAX_LEN} characters")));
    }
    if account().is_none() || s.from.is_empty() {
        return Err(AppError::bad("text messages are not set up (System → Text messages)"));
    }
    let id = sqlx::query_scalar!(
        "INSERT INTO sms_messages (direction, to_number, from_number, body, purpose, item_id, booking_id, campaign_id, user_id)
         VALUES ('out', $1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
        to, s.from, body, m.purpose, m.item_id, m.booking_id, m.campaign_id, m.user_id
    )
    .fetch_one(&state.db)
    .await?;
    jobs::enqueue(&state.db, SEND_JOB, json!({ "id": id }), None).await?;
    state.job_wakeup.notify_one();
    Ok(id)
}

/// The job: hands one queued message to Twilio. Network trouble and Twilio's own errors
/// (5xx) are retried by the job queue; a refusal (4xx) fails the message with Twilio's reason.
pub async fn run_job(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let Some(id) = payload["id"].as_str().and_then(|s| Uuid::parse_str(s).ok()) else { return Ok(()) };
    let row = sqlx::query!("SELECT to_number, from_number, body, status FROM sms_messages WHERE id = $1", id).fetch_optional(&state.db).await?;
    let Some(row) = row else { return Ok(()) };
    if row.status != "queued" {
        return Ok(());
    }
    let fail = |e: String| async move {
        sqlx::query!("UPDATE sms_messages SET status = 'failed', error = $2, updated_at = now() WHERE id = $1", id, e).execute(&state.db).await.map(|_| ())
    };
    if state.config.sandbox {
        return Ok(fail("not sent: this is the sandbox".into()).await?);
    }
    let Some(acc) = account() else { return Ok(fail("text messages are not set up".into()).await?) };
    let from = row.from_number.unwrap_or_default();
    let mut form = vec![("To", row.to_number.clone()), ("Body", row.body.clone()), ("StatusCallback", format!("{}/api/public/twilio/sms/status", state.config.public_url))];
    form.push(if from.starts_with("MG") { ("MessagingServiceSid", from) } else { ("From", from) });
    let url = format!("{}/2010-04-01/Accounts/{}/Messages.json", acc.api, acc.sid);
    let r = http().post(&url).basic_auth(&acc.sid, Some(&acc.token)).form(&form).send().await?;
    let code = r.status();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    if code.is_server_error() {
        anyhow::bail!("Twilio answered {code}");
    }
    if !code.is_success() {
        let msg = v["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("Twilio refused the message ({code})"));
        return Ok(fail(msg).await?);
    }
    let sid = v["sid"].as_str().unwrap_or_default().to_string();
    let segments = v["num_segments"].as_str().and_then(|s| s.parse::<i32>().ok());
    let status = map_status(v["status"].as_str().unwrap_or("queued"));
    sqlx::query!(
        "UPDATE sms_messages SET status = $2, provider_sid = NULLIF($3, ''), segments = $4, from_number = COALESCE($5, from_number), updated_at = now() WHERE id = $1",
        id, status, sid, segments, v["from"].as_str()
    )
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Twilio's statuses in NERP's words.
fn map_status(s: &str) -> &'static str {
    match s {
        "delivered" | "read" => "delivered",
        "sent" => "sent",
        "undelivered" => "undelivered",
        "failed" | "canceled" => "failed",
        _ => "sending",
    }
}

// ---------------------------------------------------------------------------
// Webhooks
// ---------------------------------------------------------------------------

/// Twilio's request signature: base64(HMAC-SHA1(token, url + each key and value, sorted)).
pub fn signature(token: &str, url: &str, params: &BTreeMap<String, String>) -> String {
    let mut data = url.to_string();
    for (k, v) in params {
        data.push_str(k);
        data.push_str(v);
    }
    let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(token.as_bytes()).expect("HMAC takes any key length");
    mac.update(data.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

fn verify(state: &AppState, headers: &HeaderMap, path: &str, params: &BTreeMap<String, String>) -> AppResult<()> {
    let acc = account().ok_or(AppError::NotFound("page"))?;
    let given = headers.get("x-twilio-signature").and_then(|v| v.to_str().ok()).unwrap_or("");
    let want = signature(&acc.token, &format!("{}{path}", state.config.public_url), params);
    if given.is_empty() || !bool::from(subtle_eq(given.as_bytes(), want.as_bytes())) || params.get("AccountSid").map(String::as_str) != Some(acc.sid.as_str()) {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

fn subtle_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn status_hook(State(state): State<AppState>, headers: HeaderMap, Form(p): Form<BTreeMap<String, String>>) -> AppResult<StatusCode> {
    verify(&state, &headers, "/api/public/twilio/sms/status", &p)?;
    let sid = p.get("MessageSid").or_else(|| p.get("SmsSid")).cloned().unwrap_or_default();
    let status = map_status(p.get("MessageStatus").or_else(|| p.get("SmsStatus")).map(String::as_str).unwrap_or(""));
    let error = p.get("ErrorCode").map(|c| format!("Twilio error {c}{}", p.get("ErrorMessage").map(|m| format!(": {m}")).unwrap_or_default()));
    // Statuses can arrive out of order; never go back from a final one.
    let row = sqlx::query!(
        "UPDATE sms_messages SET status = $2, error = COALESCE($3, error), updated_at = now()
         WHERE provider_sid = $1 AND status NOT IN ('delivered', 'undelivered', 'failed') RETURNING id, item_id, status",
        sid, status, error
    )
    .fetch_optional(&state.db)
    .await?;
    if let Some(r) = row
        && let Some(item) = r.item_id
        && matches!(r.status.as_str(), "undelivered" | "failed") {
            let who: Option<Uuid> = sqlx::query_scalar!("SELECT assigned_to FROM crm_items WHERE id = $1", item).fetch_optional(&state.db).await?.flatten();
            let title: String = sqlx::query_scalar!("SELECT title FROM crm_items WHERE id = $1", item).fetch_optional(&state.db).await?.unwrap_or_default();
            if let Some(u) = who {
                notify(&state, &[u], NewNotification {
                    kind: "sms.failed",
                    title: format!("A text message to {title} was not delivered"),
                    body: error.clone(),
                    link: Some(item_link(&state, item).await),
                    actor: None,
                })
                .await;
            }
        }
    Ok(StatusCode::NO_CONTENT)
}

async fn item_link(state: &AppState, item: Uuid) -> String {
    let code: Option<String> = sqlx::query_scalar!("SELECT t.code FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type WHERE i.id = $1", item).fetch_optional(&state.db).await.ok().flatten();
    format!("/crm/{}/{item}", code.unwrap_or_else(|| "lead".into()))
}

/// The record a phone number belongs to: a contact first, then a company, then the newest
/// open lead or deal with that number.
pub async fn item_for_number(state: &AppState, number: &str) -> AppResult<Option<Uuid>> {
    let n = crm::items::normalize("phone", number);
    if n.len() < 6 {
        return Ok(None);
    }
    Ok(sqlx::query_scalar!(
        "SELECT i.id FROM crm_multifields m JOIN crm_items i ON i.id = m.item_id
         WHERE m.kind = 'phone' AND m.normalized = $1 AND i.deleted_at IS NULL
         ORDER BY CASE i.entity_type WHEN 3 THEN 0 WHEN 4 THEN 1 ELSE 2 END, i.updated_at DESC LIMIT 1",
        n
    )
    .fetch_optional(&state.db)
    .await?)
}

async fn inbound_hook(State(state): State<AppState>, headers: HeaderMap, Form(p): Form<BTreeMap<String, String>>) -> AppResult<Response> {
    verify(&state, &headers, "/api/public/twilio/sms/inbound", &p)?;
    let from = p.get("From").cloned().unwrap_or_default();
    let to = p.get("To").cloned().unwrap_or_default();
    let body: String = p.get("Body").cloned().unwrap_or_default().chars().take(MAX_LEN).collect();
    let sid = p.get("MessageSid").or_else(|| p.get("SmsSid")).cloned();
    let item = item_for_number(&state, &from).await?;
    // Opting out of (and back into) marketing texts, with the usual keywords.
    let word = body.trim().to_uppercase();
    if ["STOP", "STOPALL", "UNSUBSCRIBE", "CANCEL", "END", "QUIT"].contains(&word.as_str()) {
        sqlx::query!("INSERT INTO sms_optouts (number) VALUES ($1) ON CONFLICT DO NOTHING", from).execute(&state.db).await?;
    } else if ["START", "UNSTOP", "YES"].contains(&word.as_str()) {
        sqlx::query!("DELETE FROM sms_optouts WHERE number = $1", from).execute(&state.db).await?;
    }
    // Twilio retries a webhook it thinks failed: the message SID keeps it to one row.
    let id = sqlx::query_scalar!(
        "INSERT INTO sms_messages (direction, to_number, from_number, body, status, provider_sid, purpose, item_id)
         VALUES ('in', $1, $2, $3, 'received', $4, 'reply', $5) ON CONFLICT (provider_sid) DO NOTHING RETURNING id",
        to, from, body, sid, item
    )
    .fetch_optional(&state.db)
    .await?;
    if let (Some(id), Some(item)) = (id, item) {
        let assigned: Option<Uuid> = sqlx::query_scalar!("SELECT assigned_to FROM crm_items WHERE id = $1", item).fetch_one(&state.db).await?;
        log_activity(&state, item, id, "incoming", &body, assigned, None).await?;
        if let Some(u) = assigned {
            notify(&state, &[u], NewNotification {
                kind: "sms.received",
                title: format!("Text message from {from}"),
                body: Some(body.chars().take(200).collect()),
                link: Some(item_link(&state, item).await),
                actor: None,
            })
            .await;
        }
    }
    // An empty TwiML answer: no automatic reply.
    Ok(([(header::CONTENT_TYPE, "text/xml")], "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Response></Response>").into_response())
}

/// Records a message on the CRM record as a completed activity and a timeline entry.
pub async fn log_activity(state: &AppState, item: Uuid, sms: Uuid, direction: &str, body: &str, responsible: Option<Uuid>, author: Option<Uuid>) -> AppResult<Uuid> {
    let subject: String = if direction == "incoming" { "Text message received".into() } else { "Text message sent".into() };
    let by = author.unwrap_or(SYSTEM_USER);
    let id = sqlx::query_scalar!(
        "INSERT INTO crm_activities (item_id, kind, subject, description, direction, responsible_id, created_by, is_completed, completed_at, sms_id)
         VALUES ($1, 'sms', $2, $3, $4, COALESCE($5::uuid, $6::uuid), $6, TRUE, now(), $7) RETURNING id",
        item, subject, body, direction, responsible, by, sms
    )
    .fetch_one(&state.db)
    .await?;
    crm::timeline(&state.db, item, "activity", Some(by), Some(subject), json!({ "activity_id": id, "kind": "sms", "event": "completed", "result": body.chars().take(300).collect::<String>() })).await?;
    sqlx::query!("UPDATE crm_items SET last_activity_at = now() WHERE id = $1", item).execute(&state.db).await?;
    let et: Option<i16> = sqlx::query_scalar!("SELECT entity_type FROM crm_items WHERE id = $1", item).fetch_optional(&state.db).await?;
    if let Some(et) = et {
        crm::broadcast(state, et, item, false).await;
    }
    Ok(id)
}

// ---------------------------------------------------------------------------
// CRM
// ---------------------------------------------------------------------------

/// The client's phone numbers for a record: its own, its primary contact's, its company's.
pub async fn client_phones(state: &AppState, item: Uuid) -> AppResult<Vec<String>> {
    Ok(sqlx::query_scalar!(
        "SELECT value FROM crm_multifields WHERE kind = 'phone' AND item_id = ANY(ARRAY[
             $1,
             (SELECT contact_id FROM crm_item_contacts WHERE item_id = $1 ORDER BY is_primary DESC, sort LIMIT 1),
             (SELECT company_id FROM crm_items WHERE id = $1)
         ]) ORDER BY item_id = $1 DESC, sort",
        item
    )
    .fetch_all(&state.db)
    .await?)
}

#[derive(Deserialize)]
struct SendBody {
    /// Empty: the client's first phone number.
    #[serde(default)]
    to: Option<String>,
    body: String,
}

async fn send_from_item(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<SendBody>) -> AppResult<Json<Value>> {
    let row = crm::items::load_row(&state, id, false).await?;
    access::require_read(&state, &auth, row.entity_type, id, row.assigned_to).await?;
    access::require(&state, &auth, row.entity_type, "write", row.assigned_to).await?;
    let phones = client_phones(&state, id).await?;
    let to = match b.to.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(t) => t.to_string(),
        None => phones.first().cloned().ok_or_else(|| AppError::bad("this record has no phone number"))?,
    };
    let sms = queue(&state, NewSms { to: &to, body: &b.body, purpose: "crm", item_id: Some(id), user_id: Some(auth.id), ..Default::default() }).await?;
    let activity = log_activity(&state, id, sms, "outgoing", b.body.trim(), Some(auth.id), Some(auth.id)).await?;
    Ok(Json(json!({ "id": sms, "activity_id": activity })))
}

#[derive(Serialize)]
struct Message {
    id: Uuid,
    direction: String,
    to_number: String,
    from_number: Option<String>,
    body: String,
    status: String,
    error: Option<String>,
    purpose: String,
    item_id: Option<Uuid>,
    user_name: Option<String>,
    created_at: DateTime<Utc>,
}

async fn item_thread(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = crm::items::load_row(&state, id, false).await?;
    access::require_read(&state, &auth, row.entity_type, id, row.assigned_to).await?;
    let messages = sqlx::query_as!(
        Message,
        r#"SELECT s.id, s.direction, s.to_number, s.from_number, s.body, s.status, s.error, s.purpose, s.item_id,
                  trim(u.first_name || ' ' || u.last_name) AS "user_name?", s.created_at
           FROM sms_messages s LEFT JOIN users u ON u.id = s.user_id WHERE s.item_id = $1 ORDER BY s.created_at LIMIT 500"#,
        id
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "enabled": enabled(&state).await, "phones": client_phones(&state, id).await?, "messages": messages })))
}

// ---------------------------------------------------------------------------
// Administration
// ---------------------------------------------------------------------------

/// For everyone: whether text messages can be sent (to show the options that use them).
async fn available(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "enabled": enabled(&state).await })))
}

async fn get_setup(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    let s = setup(&state).await;
    let saved: Option<Value> = sqlx::query_scalar!("SELECT value FROM settings WHERE key = 'sms'").fetch_optional(&state.db).await?;
    Ok(Json(json!({
        "account": account().map(|a| format!("{}…{}", &a.sid[..a.sid.len().min(6)], &a.sid[a.sid.len().saturating_sub(4)..])),
        "from": saved.as_ref().and_then(|v| v["from"].as_str()).unwrap_or(""),
        "from_env": std::env::var("TWILIO_FROM").unwrap_or_default(),
        "country_code": s.country_code,
        "enabled": enabled(&state).await,
        "status_url": format!("{}/api/public/twilio/sms/status", state.config.public_url),
        "inbound_url": format!("{}/api/public/twilio/sms/inbound", state.config.public_url),
    })))
}

async fn put_setup(State(state): State<AppState>, auth: AuthUser, Json(b): Json<Setup>) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    let from = b.from.trim().to_string();
    if !from.is_empty() && !(from.starts_with("MG") && from.len() == 34 && from[2..].chars().all(|c| c.is_ascii_hexdigit())) && normalize(&from, "").is_none() {
        return Err(AppError::bad("the sender is a Twilio number with its country code (+…) or a Messaging Service SID (MG…)"));
    }
    let cc = b.country_code.trim().to_string();
    if !cc.is_empty() && !(cc.starts_with('+') && (2..=4).contains(&cc.len()) && cc[1..].chars().all(|c| c.is_ascii_digit())) {
        return Err(AppError::bad("the calling code is + and 1–3 digits, such as +91"));
    }
    let from = if from.starts_with("MG") { from } else { normalize(&from, "").unwrap_or_default() };
    sqlx::query!(
        "INSERT INTO settings (key, value) VALUES ('sms', $1) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
        json!({ "from": from, "country_code": cc })
    )
    .execute(&state.db)
    .await?;
    crate::audit::record(&state.db, Some(auth.id), "sms.setup", "settings", Some("sms".into()), json!({ "from": from, "country_code": cc }), None).await;
    get_setup(State(state), auth).await
}

#[derive(Deserialize)]
struct TestBody {
    to: String,
}

async fn send_test(State(state): State<AppState>, auth: AuthUser, Json(b): Json<TestBody>) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    let company = crate::routes::auth::company_name(&state).await;
    let id = queue(&state, NewSms { to: &b.to, body: &format!("A test message from {company}'s NERP."), purpose: "test", user_id: Some(auth.id), ..Default::default() }).await?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

async fn list_messages(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    auth.require(perm::SETTINGS_MANAGE)?;
    let term = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| format!("%{}%", s.replace('%', "\\%").replace('_', "\\_")));
    let messages = sqlx::query_as!(
        Message,
        r#"SELECT s.id, s.direction, s.to_number, s.from_number, s.body, s.status, s.error, s.purpose, s.item_id,
                  trim(u.first_name || ' ' || u.last_name) AS "user_name?", s.created_at
           FROM sms_messages s LEFT JOIN users u ON u.id = s.user_id
           WHERE ($1::text IS NULL OR s.to_number ILIKE $1 OR s.from_number ILIKE $1 OR s.body ILIKE $1)
           ORDER BY s.created_at DESC LIMIT $2"#,
        term, q.limit.unwrap_or(200).clamp(1, 1000)
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "messages": messages })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(normalize("+44 20 7946 0000", "").as_deref(), Some("+442079460000"));
        assert_eq!(normalize("0044 20 7946 0000", "").as_deref(), Some("+442079460000"));
        assert_eq!(normalize("098765 43210", "+91").as_deref(), Some("+919876543210"));
        assert_eq!(normalize("098765 43210", ""), None);
        assert_eq!(normalize("+12", ""), None);
    }

    #[test]
    fn twilio_signature() {
        // Checked against an independent implementation (Python hmac + hashlib.sha1).
        let mut p = BTreeMap::new();
        for (k, v) in [("CallSid", "CA1234567890ABCDE"), ("Caller", "+12349013030"), ("Digits", "1234"), ("From", "+12349013030"), ("To", "+18005551212")] {
            p.insert(k.to_string(), v.to_string());
        }
        assert_eq!(signature("12345", "https://example.com/myapp.php?foo=1&bar=2", &p), "vNe7KK2kJwCsxc9K3OLkkKB3qqI=");
    }
}
