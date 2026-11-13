//! E-signature for generated documents.
//!
//! "Send for signature" on a ready document freezes a copy of its PDF (and records its
//! SHA-256), and emails each signer a personal link. On the signing page a signer reads the
//! PDF, types their name, draws their signature and agrees; or declines with a reason. Each
//! signature keeps the time, IP address and browser. When everyone has signed, a signed PDF
//! is made: the document followed by a certificate page with every signature and the
//! fingerprints of the document as sent and as signed. It is emailed to all signers and
//! shown on the record. Links expire after 30 days.

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use super::{access::require, broadcast, items::load_row, timeline};
use crate::{
    auth::{tokens::random_token, AuthUser, ClientIp},
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::files::{delete_stored, read_bytes, store_bytes},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/documents/{id}/signature", post(send))
        .route("/crm/items/{id}/signatures", get(list))
        .route("/crm/signatures/{id}/cancel", post(cancel))
        .route("/crm/signatures/{id}/remind", post(remind))
        .route("/public/sign/{token}", get(signer_view).post(sign))
        .route("/public/sign/{token}/document", get(signer_document))
        .route("/public/sign/{token}/decline", post(decline))
}

fn hash(s: &[u8]) -> String {
    hex::encode(Sha256::digest(s))
}

pub(crate) async fn company(state: &AppState) -> String {
    let v: Option<Value> = sqlx::query_scalar!("SELECT value FROM settings WHERE key = 'company_name'").fetch_optional(&state.db).await.ok().flatten();
    v.and_then(|v| v.as_str().map(str::to_string)).filter(|s| !s.is_empty()).unwrap_or_else(|| "Our company".into())
}

// ---------------------------------------------------------------------------
// Staff side
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SignerInput {
    name: String,
    email: String,
    /// Optional: the link goes to this phone by text message too.
    #[serde(default)]
    phone: Option<String>,
}

#[derive(Deserialize)]
struct SendBody {
    signers: Vec<SignerInput>,
    message: Option<String>,
}

/// Whether this person may send documents for signature at all: everyone with write access
/// to the record, unless the company limits it to people with "Send for e-signature".
pub(crate) async fn may_send(state: &AppState, auth: &AuthUser) -> AppResult<()> {
    let restricted: bool = sqlx::query_scalar!(r#"SELECT value = 'true'::jsonb AS "v!" FROM settings WHERE key = 'esign_restricted'"#).fetch_optional(&state.db).await?.unwrap_or(false);
    if restricted && !auth.can(crate::auth::permissions::ESIGN) && !auth.can(crate::auth::permissions::CRM_ADMIN) {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

async fn send(State(state): State<AppState>, auth: AuthUser, Path(doc): Path<Uuid>, Json(b): Json<SendBody>) -> AppResult<Json<Value>> {
    may_send(&state, &auth).await?;
    let item: Uuid = sqlx::query_scalar!("SELECT item_id FROM crm_documents WHERE id = $1", doc).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("document"))?;
    let row = load_row(&state, item, false).await?;
    require(&state, &auth, row.entity_type, "write", row.assigned_to).await?;
    let signers = b.signers.into_iter().map(|s| (s.name, s.email, s.phone.filter(|p| !p.trim().is_empty()))).collect();
    let id = send_document(&state, auth.id, doc, signers, b.message, None).await?;
    request_view(&state, id).await.map(Json)
}

/// Sends a ready document for signature (the caller checked the rights). Returns the request.
/// A signer as given: name, email and (optionally) a phone for the link by text message.
pub(crate) type SignerIn = (String, String, Option<String>);

pub(crate) async fn send_document(state: &AppState, by: Uuid, doc: Uuid, signers_in: Vec<SignerIn>, message: Option<String>, template: Option<Uuid>) -> AppResult<Uuid> {
    let (item, number, status, pdf): (Uuid, String, String, Option<Uuid>) =
        sqlx::query!(r#"SELECT item_id, number, status, pdf_file_id FROM crm_documents WHERE id = $1"#, doc).map(|r| (r.item_id, r.number, r.status, r.pdf_file_id))
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound("document"))?;
    let row = load_row(state, item, false).await?;
    let pdf = pdf.filter(|_| status == "ready").ok_or_else(|| AppError::bad("the document is not ready yet"))?;
    if signers_in.is_empty() || signers_in.len() > 10 {
        return Err(AppError::bad("add one to ten signers"));
    }
    let mut signers = Vec::new();
    let texting = signers_in.iter().any(|s| s.2.is_some());
    if texting && !crate::sms::enabled(state).await {
        return Err(AppError::bad("text messages are not set up (System → Text messages); leave the phone numbers empty"));
    }
    let cc = crate::sms::country_code(state).await;
    for (sname, semail, sphone) in &signers_in {
        let name = crate::routes::required("signer's name", sname, 200)?;
        let email = semail.trim().to_lowercase();
        if !email.contains('@') || email.contains(char::is_whitespace) || email.len() > 254 {
            return Err(AppError::bad(format!("“{semail}” is not a valid email address")));
        }
        if signers.iter().any(|(_, e, _): &(String, String, Option<String>)| *e == email) {
            return Err(AppError::bad("each signer once"));
        }
        let phone = match sphone {
            Some(p) => Some(crate::sms::normalize(p, &cc).ok_or_else(|| AppError::bad(format!("“{p}” is not a phone number with its country code")))?),
            None => None,
        };
        signers.push((name, email, phone));
    }
    let open: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM sign_requests WHERE document_id = $1 AND status = 'sent') AS "v!""#, doc).fetch_one(&state.db).await?;
    let state = state.clone();
    if open {
        return Err(AppError::bad("this document is already out for signature; cancel that first"));
    }
    // Freeze what is being signed.
    let (_, _, _, bytes) = read_bytes(&state, pdf).await?;
    let frozen = store_bytes(&state, Some(by), &format!("{number}.pdf"), "application/pdf", &bytes).await?;
    let message = message.map(|m| m.trim().chars().take(2000).collect::<String>()).filter(|m| !m.is_empty());
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar!("INSERT INTO sign_requests (document_id, item_id, title, message, original_file_id, original_sha256, created_by, template_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id", doc, item, &number, message.as_deref(), frozen, hash(&bytes), by, template)
    .fetch_one(&mut *tx)
    .await?;
    let mut links = Vec::new();
    for (i, (name, email, phone)) in signers.iter().enumerate() {
        let token = random_token(24);
        sqlx::query!("INSERT INTO sign_signers (request_id, name, email, sort, token_hash, phone) VALUES ($1, $2, $3, $4, $5, $6)", id, name, email, i as i32, hash(token.as_bytes()), phone.as_deref())
            .execute(&mut *tx)
            .await?;
        links.push((name.clone(), email.clone(), phone.clone(), token));
    }
    tx.commit().await?;
    let sender: String = sqlx::query_scalar!(r#"SELECT trim(first_name || ' ' || last_name) AS "v!" FROM users WHERE id = $1"#, by).fetch_one(&state.db).await?;
    let company = company(&state).await;
    for (name, email, phone, token) in &links {
        invite(&state, &company, &sender, &number, message.as_deref(), name, email, phone.as_deref(), Some(item), token).await;
    }
    timeline(&state.db, item, "signature", Some(by), None, json!({ "event": "sent", "request_id": id, "number": number, "signers": signers.iter().map(|(n, _, _)| n).collect::<Vec<_>>() })).await?;
    broadcast(&state, row.entity_type, item, false).await;
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
async fn invite(state: &AppState, company: &str, sender: &str, number: &str, message: Option<&str>, name: &str, email: &str, phone: Option<&str>, item: Option<Uuid>, token: &str) {
    let link = format!("{}/sign/{token}", state.config.public_url);
    if let Some(p) = phone {
        let body = format!("{name}, {sender} from {company} asks you to sign {number}: {link}");
        if let Err(e) = crate::sms::queue(state, crate::sms::NewSms { to: p, body: &body, purpose: "esign", item_id: item, ..Default::default() }).await {
            tracing::warn!(error = ?e, "could not text a signature request");
        }
    }
    let mut lines = vec![format!("Hi {name},"), format!("{sender} from {company} asks you to sign {number}.")];
    if let Some(m) = message {
        lines.push(m.to_string());
    }
    lines.push("Open the document, read it, and sign it with your name and signature. The link is personal and works for 30 days.".into());
    let mail = crate::mail::notice(email, company, &format!("Please sign {number}"), &format!("Please sign {number}"), &lines, Some(("Review and sign", &link)), "If you were not expecting this, you can ignore this email.");
    if let Err(e) = crate::jobs::send_email(state, mail).await {
        tracing::warn!(error = ?e, "could not queue a signature request");
    }
}

#[derive(Serialize, FromRow)]
struct SignerView {
    id: Uuid,
    name: String,
    email: String,
    phone: Option<String>,
    status: String,
    viewed_at: Option<DateTime<Utc>>,
    signed_at: Option<DateTime<Utc>>,
    decline_reason: Option<String>,
}

#[derive(Serialize, FromRow)]
struct RequestRow {
    id: Uuid,
    document_id: Option<Uuid>,
    item_id: Uuid,
    title: String,
    message: Option<String>,
    status: String,
    original_file_id: Option<Uuid>,
    original_sha256: String,
    signed_file_id: Option<Uuid>,
    signed_sha256: Option<String>,
    created_by_name: Option<String>,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}

const REQUEST_SELECT: &str = "SELECT r.id, r.document_id, r.item_id, r.title, r.message,
    CASE WHEN r.status = 'sent' AND r.expires_at < now() THEN 'expired' ELSE r.status END AS status,
    r.original_file_id, r.original_sha256, r.signed_file_id, r.signed_sha256, trim(u.first_name || ' ' || u.last_name) AS created_by_name,
    r.created_at, r.expires_at, r.completed_at
    FROM sign_requests r LEFT JOIN users u ON u.id = r.created_by";

async fn signers_of(state: &AppState, id: Uuid) -> AppResult<Vec<SignerView>> {
    Ok(sqlx::query_as!(SignerView, r#"SELECT id, name, email, phone, status, viewed_at AS "viewed_at?", signed_at AS "signed_at?", decline_reason AS "decline_reason?" FROM sign_signers WHERE request_id = $1 ORDER BY sort"#, id)
        .fetch_all(&state.db)
        .await?)
}

async fn request_view(state: &AppState, id: Uuid) -> AppResult<Value> {
    let r: RequestRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("{REQUEST_SELECT} WHERE r.id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("signature request"))?;
    let mut v = serde_json::to_value(r).unwrap();
    v["signers"] = json!(signers_of(state, id).await?);
    Ok(v)
}

async fn list(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = load_row(&state, item, false).await?;
    super::access::require_read(&state, &auth, row.entity_type, item, row.assigned_to).await?;
    let ids: Vec<Uuid> = sqlx::query_scalar!("SELECT id FROM sign_requests WHERE item_id = $1 ORDER BY created_at DESC", item).fetch_all(&state.db).await?;
    let mut items = Vec::new();
    for id in ids {
        items.push(request_view(&state, id).await?);
    }
    Ok(Json(json!({ "items": items })))
}

async fn writable(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<(Uuid, i16, String, String)> {
    may_send(state, auth).await?;
    let (item, status, title): (Uuid, String, String) = sqlx::query!(r#"SELECT item_id AS "item_id!", CASE WHEN status = 'sent' AND expires_at < now() THEN 'expired' ELSE status END AS "c1!", title AS "title!" FROM sign_requests WHERE id = $1"#, id).map(|r| (r.item_id, r.c1, r.title))
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("signature request"))?;
    let row = load_row(state, item, false).await?;
    require(state, auth, row.entity_type, "write", row.assigned_to).await?;
    Ok((item, row.entity_type, status, title))
}

async fn cancel(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (item, et, status, title) = writable(&state, &auth, id).await?;
    if status != "sent" {
        return Err(AppError::bad("only a request still waiting for signatures can be cancelled"));
    }
    sqlx::query!("UPDATE sign_requests SET status = 'cancelled' WHERE id = $1", id).execute(&state.db).await?;
    timeline(&state.db, item, "signature", Some(auth.id), None, json!({ "event": "cancelled", "request_id": id, "number": title })).await?;
    broadcast(&state, et, item, false).await;
    request_view(&state, id).await.map(Json)
}

/// Sends the links again to those who have not signed (new links; the old ones stop working).
async fn remind(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (_, _, status, title) = writable(&state, &auth, id).await?;
    if status != "sent" {
        return Err(AppError::bad("this request is no longer waiting for signatures"));
    }
    let message: Option<String> = sqlx::query_scalar!(r#"SELECT message AS "message?" FROM sign_requests WHERE id = $1"#, id).fetch_one(&state.db).await?;
    let waiting: Vec<(Uuid, String, String, Option<String>)> = sqlx::query!(r#"SELECT id, name, email, phone FROM sign_signers WHERE request_id = $1 AND status IN ('pending', 'viewed')"#, id).map(|r| (r.id, r.name, r.email, r.phone)).fetch_all(&state.db).await?;
    let item: Uuid = sqlx::query_scalar!("SELECT item_id FROM sign_requests WHERE id = $1", id).fetch_one(&state.db).await?;
    let sender: String = sqlx::query_scalar!(r#"SELECT trim(first_name || ' ' || last_name) AS "v!" FROM users WHERE id = $1"#, auth.id).fetch_one(&state.db).await?;
    let company = company(&state).await;
    for (sid, name, email, phone) in &waiting {
        let token = random_token(24);
        sqlx::query!("UPDATE sign_signers SET token_hash = $2 WHERE id = $1", sid, hash(token.as_bytes())).execute(&state.db).await?;
        invite(&state, &company, &sender, &title, message.as_deref(), name, email, phone.as_deref(), Some(item), &token).await;
    }
    let mut v = request_view(&state, id).await?;
    v["reminded"] = json!(waiting.len());
    Ok(Json(v))
}

// ---------------------------------------------------------------------------
// Signer side
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct Signer {
    id: Uuid,
    request_id: Uuid,
    name: String,
    email: String,
    status: String,
    request_status: String,
    title: String,
    message: Option<String>,
    original_file_id: Option<Uuid>,
    original_sha256: String,
    signed_file_id: Option<Uuid>,
    expires_at: DateTime<Utc>,
}

async fn signer(state: &AppState, token: &str) -> AppResult<Signer> {
    sqlx::query_as!(Signer, r#"SELECT s.id AS "id!", s.request_id AS "request_id!", s.name AS "name!", s.email AS "email!", s.status AS "status!",
                CASE WHEN r.status = 'sent' AND r.expires_at < now() THEN 'expired' ELSE r.status END AS "request_status!",
                r.title AS "title!", r.message AS "message?", r.original_file_id AS "original_file_id?", r.original_sha256 AS "original_sha256!", r.signed_file_id AS "signed_file_id?", r.expires_at AS "expires_at!"
         FROM sign_signers s JOIN sign_requests r ON r.id = s.request_id WHERE s.token_hash = $1"#, hash(token.as_bytes()))
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("signing link"))
}

async fn signer_view(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let s = signer(&state, &token).await?;
    if s.status == "pending" && s.request_status == "sent" {
        sqlx::query!("UPDATE sign_signers SET status = 'viewed', viewed_at = now() WHERE id = $1", s.id).execute(&state.db).await?;
    }
    let others: Vec<(String, String)> = sqlx::query!(r#"SELECT name, status FROM sign_signers WHERE request_id = $1 AND id <> $2 ORDER BY sort"#, s.request_id, s.id).map(|r| (r.name, r.status))
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!({
        "title": s.title,
        "message": s.message,
        "company": company(&state).await,
        "name": s.name,
        "email": s.email,
        "status": if s.status == "pending" { "viewed" } else { s.status.as_str() },
        "request_status": s.request_status,
        "fingerprint": s.original_sha256,
        "expires_at": s.expires_at,
        "signed_available": s.signed_file_id.is_some(),
        "others": others.into_iter().map(|(n, st)| json!({ "name": n, "status": st })).collect::<Vec<_>>(),
    })))
}

/// The PDF to sign; once everyone has signed, the signed copy.
async fn signer_document(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Response> {
    let s = signer(&state, &token).await?;
    let file = s.signed_file_id.or(s.original_file_id).ok_or(AppError::NotFound("document"))?;
    let (name, _, _, bytes) = read_bytes(&state, file).await?;
    let mut r = bytes.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    let safe: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || " .-_()".contains(*c)).collect();
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&format!("inline; filename=\"{safe}\"")).unwrap_or(HeaderValue::from_static("inline")));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    Ok(r)
}

#[derive(Deserialize)]
struct SignBody {
    signed_name: String,
    /// A PNG of the drawn signature, as a data: URL.
    signature: String,
    agree: bool,
}

fn open_for(s: &Signer) -> AppResult<()> {
    match s.request_status.as_str() {
        "sent" => {}
        "expired" => return Err(AppError::bad("this signing link has expired; ask the sender for a new one")),
        "cancelled" => return Err(AppError::bad("the sender cancelled this request")),
        _ => return Err(AppError::bad("this document is no longer waiting for signatures")),
    }
    if s.status == "signed" || s.status == "declined" {
        return Err(AppError::bad("you have already answered this request"));
    }
    Ok(())
}

async fn sign(State(state): State<AppState>, ClientIp(ip): ClientIp, headers: HeaderMap, Path(token): Path<String>, Json(b): Json<SignBody>) -> AppResult<Json<Value>> {
    let s = signer(&state, &token).await?;
    open_for(&s)?;
    if !b.agree {
        return Err(AppError::bad("please agree to sign electronically"));
    }
    let typed = crate::routes::required("your name", &b.signed_name, 200)?;
    let png = b
        .signature
        .strip_prefix("data:image/png;base64,")
        .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok())
        .filter(|p| p.starts_with(b"\x89PNG\r\n\x1a\n"))
        .ok_or_else(|| AppError::bad("please draw your signature"))?;
    if png.len() > 300_000 {
        return Err(AppError::bad("the signature image is too large"));
    }
    let file = store_bytes(&state, None, &format!("signature-{}.png", s.id), "image/png", &png).await?;
    let ua: Option<String> = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(|v| v.chars().take(300).collect());
    let n = sqlx::query!("UPDATE sign_signers SET status = 'signed', signed_at = now(), signed_name = $2, signature_file_id = $3, ip = $4, user_agent = $5
         WHERE id = $1 AND status IN ('pending', 'viewed')", s.id, &typed, file, ip.as_deref(), ua.as_deref())
    .execute(&state.db)
    .await?
    .rows_affected();
    if n == 0 {
        delete_stored(&state, file).await;
        return Err(AppError::bad("you have already answered this request"));
    }
    let (item, created_by): (Uuid, Option<Uuid>) = sqlx::query!(r#"SELECT item_id, created_by FROM sign_requests WHERE id = $1"#, s.request_id).map(|r| (r.item_id, r.created_by)).fetch_one(&state.db).await?;
    timeline(&state.db, item, "signature", None, None, json!({ "event": "signed", "request_id": s.request_id, "number": s.title, "name": s.name })).await?;
    // The last signature completes it (once, even if two arrive together).
    let left: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM sign_signers WHERE request_id = $1 AND status <> 'signed'"#, s.request_id).fetch_one(&state.db).await?;
    if left == 0 {
        let claimed = sqlx::query!("UPDATE sign_requests SET status = 'completed', completed_at = now() WHERE id = $1 AND status = 'sent'", s.request_id)
            .execute(&state.db)
            .await?
            .rows_affected();
        if claimed == 1
            && let Err(e) = finish(&state, s.request_id).await {
                tracing::warn!(request = %s.request_id, error = ?e, "could not make the signed copy");
            }
    } else {
        tell(&state, created_by, format!("{} signed {}", s.name, s.title), item).await;
    }
    signer_view(State(state), Path(token)).await
}

#[derive(Deserialize)]
struct DeclineBody {
    reason: Option<String>,
}

async fn decline(State(state): State<AppState>, Path(token): Path<String>, Json(b): Json<DeclineBody>) -> AppResult<Json<Value>> {
    let s = signer(&state, &token).await?;
    open_for(&s)?;
    let reason = b.reason.map(|r| r.trim().chars().take(1000).collect::<String>()).filter(|r| !r.is_empty());
    sqlx::query!("UPDATE sign_signers SET status = 'declined', decline_reason = $2 WHERE id = $1", s.id, reason.as_deref()).execute(&state.db).await?;
    sqlx::query!("UPDATE sign_requests SET status = 'declined' WHERE id = $1 AND status = 'sent'", s.request_id).execute(&state.db).await?;
    let (item, created_by): (Uuid, Option<Uuid>) = sqlx::query!(r#"SELECT item_id, created_by FROM sign_requests WHERE id = $1"#, s.request_id).map(|r| (r.item_id, r.created_by)).fetch_one(&state.db).await?;
    timeline(&state.db, item, "signature", None, None, json!({ "event": "declined", "request_id": s.request_id, "number": s.title, "name": s.name, "reason": reason })).await?;
    tell(&state, created_by, format!("{} declined to sign {}", s.name, s.title), item).await;
    let tpl: Option<Uuid> = sqlx::query_scalar!(r#"SELECT template_id AS "template_id?" FROM sign_requests WHERE id = $1"#, s.request_id).fetch_one(&state.db).await?;
    super::automation::fire(&state, item, "signature_declined", |p| same_template(p, tpl)).await;
    signer_view(State(state), Path(token)).await
}

async fn tell(state: &AppState, who: Option<Uuid>, title: String, item: Uuid) {
    let Some(u) = who else { return };
    let code: Option<(String, i16)> = sqlx::query!(r#"SELECT t.code, t.id FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type WHERE i.id = $1"#, item).map(|r| (r.code, r.id))
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    if let Some((_, et)) = &code {
        broadcast(state, *et, item, false).await;
    }
    notify(state, &[u], NewNotification { kind: "crm.signature", title, body: None, link: code.map(|(c, _)| format!("/crm/{c}/{item}")), actor: None }).await;
}

// ---------------------------------------------------------------------------
// The signed copy
// ---------------------------------------------------------------------------

const CERTIFICATE: &str = r##"#set page(paper: "a4", margin: 2cm)
#set text(font: "Libertinus Serif", size: 10pt)
#let d = json("/data.json")
#text(size: 18pt, weight: "bold")[Signature certificate]
#v(4pt)
#text(fill: luma(90))[#d.company · #d.title]
#v(10pt)
#table(columns: (auto, 1fr), stroke: none, column-gutter: 14pt, inset: (x: 0pt, y: 3pt),
  [*Document*], [#d.title],
  [*Request*], [#d.request],
  [*Sent*], [#d.sent],
  [*Completed*], [#d.completed],
  [*SHA-256 as sent*], [#text(size: 8pt, font: "DejaVu Sans Mono")[#d.sha]],
)
#v(6pt)
The pages before this one are the document each signer opened; its fingerprint (SHA-256) was
recorded when it was sent and has not changed.
#v(8pt)
#for s in d.signers [
  #block(width: 100%, stroke: 0.5pt + luma(200), radius: 4pt, inset: 10pt, breakable: false)[
    #grid(columns: (1fr, 6cm), gutter: 10pt,
      [
        *#s.name* #text(fill: luma(90))[(#s.email)] \
        Signed as “#s.signed_name” on #s.signed_at UTC \
        #text(size: 8pt, fill: luma(90))[IP address #s.ip · #s.agent]
      ],
      image(s.image, height: 2cm, fit: "contain"),
    )
  ]
  #v(6pt)
]
"##;

/// One signature on a certificate page.
pub(crate) struct CertSigner {
    pub name: String,
    pub email: String,
    pub signed_name: String,
    pub signed_at: Option<DateTime<Utc>>,
    pub ip: Option<String>,
    pub agent: Option<String>,
    pub signature: Option<Uuid>,
}

/// The signed copy: the original PDF with a certificate page (signers, times, addresses,
/// the original's fingerprint) added at the end.
pub(crate) async fn signed_copy(state: &AppState, title: &str, request: &str, sha: &str, sent: DateTime<Utc>, completed: DateTime<Utc>, list: &[CertSigner], original: &[u8]) -> AppResult<Vec<u8>> {
    let mut files = Vec::new();
    let mut signers = Vec::new();
    for (i, s) in list.iter().enumerate() {
        let image = format!("sig-{i}.png");
        if let Some(f) = s.signature {
            files.push((image.clone(), read_bytes(state, f).await?.3));
        }
        signers.push(json!({
            "name": s.name, "email": s.email, "signed_name": s.signed_name,
            "signed_at": s.signed_at.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()).unwrap_or_default(),
            "ip": s.ip.clone().unwrap_or_else(|| "unknown".into()), "agent": s.agent.clone().unwrap_or_default().chars().take(90).collect::<String>(),
            "image": format!("/{image}"),
        }));
    }
    let data = json!({
        "company": company(state).await, "title": title, "request": request, "sha": sha, "signers": signers,
        "sent": sent.format("%Y-%m-%d %H:%M UTC").to_string(),
        "completed": completed.format("%Y-%m-%d %H:%M UTC").to_string(),
    });
    let cert = tokio::task::spawn_blocking(move || super::docs::typst_engine::render_with(CERTIFICATE, &data, files))
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?
        .map_err(|e| AppError::Internal(anyhow::anyhow!("certificate: {e}")))?;
    let dir = std::env::temp_dir().join(format!("nerp-sign-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(dir.join("a.pdf"), original).await?;
    tokio::fs::write(dir.join("b.pdf"), &cert).await?;
    let joined = super::docs::process::run("pdfunite", &["a.pdf", "b.pdf", "signed.pdf"], &dir, &[], 60).await;
    let signed = match joined {
        Ok(_) => tokio::fs::read(dir.join("signed.pdf")).await.map_err(|e| AppError::Internal(e.into())),
        Err(_) => Err(AppError::Internal(anyhow::anyhow!("pdfunite failed"))),
    };
    let _ = tokio::fs::remove_dir_all(&dir).await;
    signed
}

pub(crate) fn sha256_hex(b: &[u8]) -> String {
    hash(b)
}

async fn finish(state: &AppState, id: Uuid) -> AppResult<()> {
    let r: RequestRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("{REQUEST_SELECT} WHERE r.id = $1"))).bind(id).fetch_one(&state.db).await?;
    let rows: Vec<(String, String, Option<String>, Option<DateTime<Utc>>, Option<String>, Option<String>, Option<Uuid>)> = sqlx::query!(r#"SELECT name, email, signed_name, signed_at, ip, user_agent, signature_file_id FROM sign_signers WHERE request_id = $1 ORDER BY sort"#, id).map(|r| (r.name, r.email, r.signed_name, r.signed_at, r.ip, r.user_agent, r.signature_file_id))
    .fetch_all(&state.db)
    .await?;
    let list: Vec<CertSigner> = rows
        .iter()
        .map(|(name, email, sn, at, ip, ua, sig)| CertSigner { name: name.clone(), email: email.clone(), signed_name: sn.clone().unwrap_or_default(), signed_at: *at, ip: ip.clone(), agent: ua.clone(), signature: *sig })
        .collect();
    let company = company(state).await;
    let original = read_bytes(state, r.original_file_id.ok_or(AppError::NotFound("document"))?).await?.3;
    let signed = signed_copy(state, &r.title, &id.to_string(), &r.original_sha256, r.created_at, r.completed_at.unwrap_or_else(Utc::now), &list, &original).await?;
    let file = store_bytes(state, None, &format!("{} (signed).pdf", r.title), "application/pdf", &signed).await?;
    sqlx::query!("UPDATE sign_requests SET signed_file_id = $2, signed_sha256 = $3 WHERE id = $1", id, file, hash(&signed)).execute(&state.db).await?;
    timeline(&state.db, r.item_id, "signature", None, None, json!({ "event": "completed", "request_id": id, "number": r.title, "file_id": file })).await?;
    // Everyone gets the signed copy.
    for (name, email, ..) in &rows {
        let mut mail = crate::mail::notice(
            email,
            &company,
            &format!("{} is signed", r.title),
            &format!("{} is signed", r.title),
            &[format!("Hi {name},"), format!("Everyone has signed {}. The signed copy, with a certificate page, is attached.", r.title)],
            None,
            "Keep this email: it is your copy of the signed document.",
        );
        mail.attachments = vec![file];
        let _ = crate::jobs::send_email(state, mail).await;
    }
    let (created_by, tpl): (Option<Uuid>, Option<Uuid>) = sqlx::query!(r#"SELECT created_by, template_id FROM sign_requests WHERE id = $1"#, id).map(|r| (r.created_by, r.template_id)).fetch_one(&state.db).await?;
    tell(state, created_by, format!("Everyone signed {}", r.title), r.item_id).await;
    super::automation::fire(state, r.item_id, "signature_completed", |p| same_template(p, tpl)).await;
    Ok(())
}

/// A trigger limited to one signing template matches only requests from it.
fn same_template(params: &Value, template: Option<Uuid>) -> bool {
    match params.get("sign_template_id").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        Some(t) => template.is_some_and(|x| x.to_string() == t),
        None => true,
    }
}
