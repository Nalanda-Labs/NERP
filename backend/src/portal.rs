//! Client portal: a contact signs in with a one-time link from their email and sees, in one
//! place, their invoices and quotes (with balances and PDFs), payments, signatures (and the
//! signed copies), help desk tickets (each opening its private page) and bookings.
//!
//! Staff turn the portal on for a contact and send the link from the contact's page; a
//! contact with the portal on can also ask for a new link at /portal. The link works once
//! and for 7 days, and opens a 30-day session kept in an HttpOnly cookie, separate from
//! employees' sign-in. What a contact sees: records they are on, and — when staff allow it —
//! their company's invoices and quotes.
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap},
    response::{AppendHeaders, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use redis::AsyncCommands;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    audit,
    auth::{AuthUser, ClientIp},
    crm::{access, CONTACT},
    error::{AppError, AppResult},
    routes::normalize_email,
    state::AppState,
};

const COOKIE: &str = "nerp_portal";
const LINK_DAYS: i64 = 7;
const SESSION_DAYS: i64 = 30;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/items/{id}/portal", get(staff_get).put(staff_put))
        .route("/crm/items/{id}/portal/invite", post(staff_invite))
        .route("/public/portal/request", post(request_link))
        .route("/public/portal/sign-in", post(sign_in))
        .route("/public/portal/sign-out", post(sign_out))
        .route("/public/portal/me", get(me))
        .route("/public/portal/files/{id}", get(file))
}

fn hash(t: &str) -> String {
    hex::encode(Sha256::digest(t.as_bytes()))
}

async fn emails_of(state: &AppState, contact: Uuid) -> AppResult<Vec<String>> {
    Ok(sqlx::query_scalar!("SELECT normalized FROM crm_multifields WHERE item_id = $1 AND kind = 'email' ORDER BY sort", contact).fetch_all(&state.db).await?)
}

/// Makes a sign-in link for a contact and emails it.
async fn send_link(state: &AppState, contact: Uuid, to: &str) -> AppResult<String> {
    let token = crate::auth::tokens::random_token(32);
    sqlx::query!("INSERT INTO portal_links (hash, contact_id, expires_at) VALUES ($1, $2, now() + make_interval(days => $3::int))", hash(&token), contact, LINK_DAYS as i32)
        .execute(&state.db)
        .await?;
    let url = format!("{}/portal/sign-in?token={token}", state.config.public_url);
    let company = crate::routes::auth::company_name(state).await;
    let mail = crate::mail::customer_notice(
        to,
        &company,
        &format!("Your {company} client portal"),
        "Your client portal",
        &[
            format!("See your invoices, payments, documents, tickets and bookings with {company} in one place."),
            format!("The link works once, within {LINK_DAYS} days; you can ask for a new one on the portal page."),
        ],
        Some(("Open the portal", &url)),
    );
    crate::jobs::send_email(state, mail).await?;
    Ok(url)
}

// ---- staff ----

async fn contact_for_staff(state: &AppState, auth: &AuthUser, id: Uuid, write: bool) -> AppResult<()> {
    let row = crate::crm::items::load_row(state, id, false).await?;
    if row.entity_type != CONTACT {
        return Err(AppError::bad("the client portal is for contacts"));
    }
    if write {
        access::require(state, auth, CONTACT, "write", row.assigned_to).await
    } else {
        access::require_read(state, auth, CONTACT, id, row.assigned_to).await
    }
}

async fn staff_get(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    contact_for_staff(&state, &auth, id, false).await?;
    let a: Option<(bool, bool, Option<DateTime<Utc>>, Option<DateTime<Utc>>)> =
        sqlx::query!(r#"SELECT enabled, company_wide, invited_at, last_seen_at FROM portal_access WHERE contact_id = $1"#, id).map(|r| (r.enabled, r.company_wide, r.invited_at, r.last_seen_at)).fetch_optional(&state.db).await?;
    Ok(Json(json!({
        "enabled": a.as_ref().is_some_and(|a| a.0), "company_wide": a.as_ref().is_some_and(|a| a.1),
        "invited_at": a.as_ref().and_then(|a| a.2), "last_seen_at": a.as_ref().and_then(|a| a.3), "emails": emails_of(&state, id).await?,
    })))
}

#[derive(Deserialize)]
struct AccessInput {
    enabled: bool,
    #[serde(default)]
    company_wide: bool,
}

async fn staff_put(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(id): Path<Uuid>, Json(b): Json<AccessInput>) -> AppResult<Json<Value>> {
    contact_for_staff(&state, &auth, id, true).await?;
    sqlx::query!("INSERT INTO portal_access (contact_id, enabled, company_wide) VALUES ($1, $2, $3)
         ON CONFLICT (contact_id) DO UPDATE SET enabled = EXCLUDED.enabled, company_wide = EXCLUDED.company_wide", id, b.enabled, b.company_wide)
    .execute(&state.db)
    .await?;
    if !b.enabled {
        // Turning it off signs them out at once.
        sqlx::query!("DELETE FROM portal_sessions WHERE contact_id = $1", id).execute(&state.db).await?;
        sqlx::query!("DELETE FROM portal_links WHERE contact_id = $1", id).execute(&state.db).await?;
    }
    audit::record(&state.db, Some(auth.id), "portal.access", "crm_item", Some(id.to_string()), json!({ "enabled": b.enabled, "company_wide": b.company_wide }), ip.as_deref()).await;
    staff_get(State(state), auth, Path(id)).await
}

#[derive(Deserialize, Default)]
struct InviteInput {
    email: Option<String>,
}

/// Turns the portal on (if it isn't) and emails the contact a sign-in link. The link is in the
/// answer too, for sending another way.
async fn staff_invite(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(id): Path<Uuid>, body: Option<Json<InviteInput>>) -> AppResult<Json<Value>> {
    contact_for_staff(&state, &auth, id, true).await?;
    let emails = emails_of(&state, id).await?;
    let to = match body.and_then(|b| b.0.email) {
        Some(e) => {
            let e = normalize_email(&e)?;
            if !emails.contains(&e) {
                return Err(AppError::bad("send it to one of the contact's own email addresses"));
            }
            e
        }
        None => emails.first().cloned().ok_or_else(|| AppError::bad("the contact has no email address"))?,
    };
    sqlx::query!("INSERT INTO portal_access (contact_id, enabled, invited_by, invited_at) VALUES ($1, TRUE, $2, now())
         ON CONFLICT (contact_id) DO UPDATE SET enabled = TRUE, invited_by = EXCLUDED.invited_by, invited_at = now()", id, auth.id)
    .execute(&state.db)
    .await?;
    let url = send_link(&state, id, &to).await?;
    audit::record(&state.db, Some(auth.id), "portal.invite", "crm_item", Some(id.to_string()), json!({ "to": to }), ip.as_deref()).await;
    Ok(Json(json!({ "sent_to": to, "url": url })))
}

// ---- the client's side ----

async fn limited(state: &AppState, key: String, max: i64, secs: i64) -> AppResult<bool> {
    let mut redis = state.redis.clone();
    let n: i64 = redis.incr(&key, 1).await?;
    if n == 1 {
        let _: () = redis.expire(&key, secs).await?;
    }
    Ok(n > max)
}

#[derive(Deserialize)]
struct RequestInput {
    email: String,
}

/// A new link for a contact with the portal on. The answer is the same for any address.
async fn request_link(State(state): State<AppState>, ClientIp(ip): ClientIp, Json(b): Json<RequestInput>) -> AppResult<Json<Value>> {
    let email = normalize_email(&b.email)?;
    if limited(&state, format!("portal:ip:{}", ip.as_deref().unwrap_or("unknown")), 10, 3600).await? || limited(&state, format!("portal:email:{email}"), 3, 3600).await? {
        return Err(AppError::TooManyRequests("too many requests; try again later".into()));
    }
    let contact: Option<Uuid> = sqlx::query_scalar!("SELECT a.contact_id FROM portal_access a JOIN crm_multifields m ON m.item_id = a.contact_id JOIN crm_items i ON i.id = a.contact_id
         WHERE a.enabled AND m.kind = 'email' AND m.normalized = $1 AND i.deleted_at IS NULL ORDER BY a.last_seen_at DESC NULLS LAST LIMIT 1", &email)
    .fetch_optional(&state.db)
    .await?;
    if let Some(c) = contact {
        send_link(&state, c, &email).await?;
    }
    Ok(Json(json!({ "ok": true })))
}

fn session_cookie(state: &AppState, token: &str, max_age: i64) -> String {
    format!(
        "{COOKIE}={token}; Path=/api/public/portal; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
        if state.config.cookie_secure { "; Secure" } else { "" }
    )
}

#[derive(Deserialize)]
struct SignIn {
    token: String,
}

async fn sign_in(State(state): State<AppState>, ClientIp(ip): ClientIp, Json(b): Json<SignIn>) -> AppResult<Response> {
    if limited(&state, format!("portal:signin:{}", ip.as_deref().unwrap_or("unknown")), 30, 3600).await? {
        return Err(AppError::TooManyRequests("too many attempts; try again later".into()));
    }
    let contact: Option<Uuid> = sqlx::query_scalar!("UPDATE portal_links l SET used_at = now() FROM portal_access a
         WHERE l.hash = $1 AND l.used_at IS NULL AND l.expires_at > now() AND a.contact_id = l.contact_id AND a.enabled RETURNING l.contact_id", hash(b.token.trim()))
    .fetch_optional(&state.db)
    .await?;
    let contact = contact.ok_or(AppError::NotFound("link"))?;
    let token = crate::auth::tokens::random_token(32);
    sqlx::query!("INSERT INTO portal_sessions (hash, contact_id, expires_at) VALUES ($1, $2, now() + make_interval(days => $3::int))", hash(&token), contact, SESSION_DAYS as i32)
        .execute(&state.db)
        .await?;
    sqlx::query!("UPDATE portal_access SET last_seen_at = now() WHERE contact_id = $1", contact).execute(&state.db).await?;
    audit::record(&state.db, None, "portal.sign_in", "crm_item", Some(contact.to_string()), json!({}), ip.as_deref()).await;
    Ok((AppendHeaders([(header::SET_COOKIE, session_cookie(&state, &token, SESSION_DAYS * 86400))]), Json(json!({ "ok": true }))).into_response())
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
        .filter(|v| !v.is_empty())
}

async fn sign_out(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    if let Some(t) = cookie_token(&headers) {
        sqlx::query!("DELETE FROM portal_sessions WHERE hash = $1", hash(&t)).execute(&state.db).await?;
    }
    Ok((AppendHeaders([(header::SET_COOKIE, session_cookie(&state, "", 0))]), Json(json!({ "ok": true }))).into_response())
}

/// The signed-in contact (and whether they see their company's records).
async fn client(state: &AppState, headers: &HeaderMap) -> AppResult<(Uuid, bool)> {
    let t = cookie_token(headers).ok_or(AppError::Unauthorized)?;
    let row: Option<(Uuid, bool)> = sqlx::query!(r#"UPDATE portal_sessions s SET last_seen_at = now() FROM portal_access a, crm_items i
         WHERE s.hash = $1 AND s.expires_at > now() AND a.contact_id = s.contact_id AND a.enabled AND i.id = s.contact_id AND i.deleted_at IS NULL
         RETURNING s.contact_id, a.company_wide"#, hash(&t)).map(|r| (r.contact_id, r.company_wide))
    .fetch_optional(&state.db)
    .await?;
    let (contact, wide) = row.ok_or(AppError::Unauthorized)?;
    sqlx::query!("UPDATE portal_access SET last_seen_at = now() WHERE contact_id = $1 AND (last_seen_at IS NULL OR last_seen_at < now() - interval '5 minutes')", contact)
        .execute(&state.db)
        .await?;
    Ok((contact, wide))
}

/// Invoices and quotes the contact may see: those they are on, and with company-wide access
/// their company's.
const VISIBLE: &str = "SELECT i.id FROM crm_items i WHERE i.deleted_at IS NULL AND i.entity_type IN (5, 6)
    AND (i.id IN (SELECT item_id FROM crm_item_contacts WHERE contact_id = $1)
         OR ($2 AND i.company_id IS NOT NULL AND i.company_id = (SELECT company_id FROM crm_items WHERE id = $1)))";

async fn me(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Json<Value>> {
    let (contact, wide) = client(&state, &headers).await?;
    let emails = emails_of(&state, contact).await?;
    let who: (String, Option<String>) = sqlx::query!(r#"SELECT COALESCE(NULLIF(trim(concat_ws(' ', c.first_name, c.last_name)), ''), c.title) AS "c0!", co.title AS "title?"
         FROM crm_items c LEFT JOIN crm_items co ON co.id = c.company_id WHERE c.id = $1"#, contact).map(|r| (r.c0, r.title))
    .fetch_one(&state.db)
    .await?;
    let base = crate::crm::money::base_currency(&state).await.unwrap_or_default();
    type Doc = (Uuid, i16, i64, String, Option<String>, Decimal, Option<NaiveDate>, Option<String>, Option<Decimal>, Option<Decimal>, Option<Decimal>, Option<NaiveDate>, Option<Uuid>);
    let docs: Vec<Doc> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT i.id, i.entity_type, i.number, i.title, i.currency, i.opportunity, i.close_date,
                a.status, a.total, a.credited,
                (SELECT COALESCE(SUM(al.amount), 0) FROM acc_allocations al JOIN acc_payments p ON p.id = al.payment_id WHERE al.doc_kind = 'invoice' AND al.doc_id = i.id AND p.status = 'posted'),
                COALESCE(a.date, i.created_at::date),
                (SELECT d.pdf_file_id FROM crm_documents d WHERE d.item_id = i.id AND d.status = 'ready' AND d.pdf_file_id IS NOT NULL ORDER BY d.created_at DESC LIMIT 1)
         FROM crm_items i LEFT JOIN acc_invoices a ON a.item_id = i.id AND a.status <> 'void'
         WHERE i.id IN ({VISIBLE}) ORDER BY COALESCE(a.date, i.created_at::date) DESC LIMIT 200"
    )))
    .bind(contact)
    .bind(wide)
    .fetch_all(&state.db)
    .await?;
    let doc = |d: &Doc| {
        let cur = d.4.clone().unwrap_or_else(|| base.clone());
        let balance = d.8.map(|t| t - d.9.unwrap_or_default() - d.10.unwrap_or_default());
        json!({
            "id": d.0, "number": d.2, "title": d.3, "currency": cur, "amount": d.8.unwrap_or(d.5), "date": d.11, "due_date": d.6,
            "status": d.7.clone().unwrap_or_else(|| "issued".into()), "balance": balance, "pdf": d.12.map(|f| format!("/api/public/portal/files/{f}")),
        })
    };
    let invoices: Vec<Value> = docs.iter().filter(|d| d.1 == 6).map(doc).collect();
    let quotes: Vec<Value> = docs.iter().filter(|d| d.1 == 5).map(doc).collect();
    let payments: Vec<(i64, NaiveDate, Decimal, String, Option<String>)> = sqlx::query!(r#"SELECT p.number, p.date, p.amount, p.currency, p.reference FROM acc_payments p
         WHERE p.direction = 'in' AND p.status = 'posted'
           AND (p.party_id = $1 OR ($2 AND p.party_id = (SELECT company_id FROM crm_items WHERE id = $1)))
         ORDER BY p.date DESC LIMIT 200"#, contact, wide).map(|r| (r.number, r.date, r.amount, r.currency, r.reference))
    .fetch_all(&state.db)
    .await?;
    let signatures: Vec<(Uuid, String, String, DateTime<Utc>, Option<DateTime<Utc>>, Option<Uuid>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT r.id, r.title, r.status, r.created_at, r.completed_at, r.signed_file_id FROM sign_requests r
         WHERE r.item_id IN ({VISIBLE}) OR r.item_id = $1 OR r.id IN (SELECT request_id FROM sign_signers WHERE lower(email) = ANY($3))
         ORDER BY r.created_at DESC LIMIT 100"
    )))
    .bind(contact)
    .bind(wide)
    .bind(&emails)
    .fetch_all(&state.db)
    .await?;
    let tickets: Vec<(i64, String, Option<DateTime<Utc>>, DateTime<Utc>, Option<DateTime<Utc>>, Option<Vec<u8>>)> = sqlx::query!(r#"SELECT number AS "number!", title AS "title!", closed_at AS "closed_at?", created_at AS "created_at!", last_message_at AS "last_message_at?", access_secret AS "access_secret?" FROM support_tickets
         WHERE NOT is_spam AND (contact_id = $1 OR lower(requester_email) = ANY($2)) ORDER BY created_at DESC LIMIT 100"#, contact, &emails).map(|r| (r.number, r.title, r.closed_at, r.created_at, r.last_message_at, r.access_secret))
    .fetch_all(&state.db)
    .await?;
    let bookings: Vec<(i64, DateTime<Utc>, DateTime<Utc>, String, Option<String>)> = sqlx::query!(r#"SELECT b.number AS "number!", b.starts_at AS "starts_at!", b.ends_at AS "ends_at!", b.status AS "status!", s.name AS "name?" FROM bookings b LEFT JOIN booking_services s ON s.id = b.service_id
         WHERE b.contact_id = $1 OR lower(b.client_email) = ANY($2) ORDER BY b.starts_at DESC LIMIT 100"#, contact, &emails).map(|r| (r.number, r.starts_at, r.ends_at, r.status, r.name))
    .fetch_all(&state.db)
    .await?;
    let company = crate::routes::auth::company_name(&state).await;
    Ok(Json(json!({
        "client": { "name": who.0, "company": who.1, "sees_company": wide }, "company": company,
        "invoices": invoices, "quotes": quotes,
        "payments": payments.into_iter().map(|(n, d, a, c, r)| json!({ "number": n, "date": d, "amount": a, "currency": c, "reference": r })).collect::<Vec<_>>(),
        "signatures": signatures.into_iter().map(|(id, t, st, at, done, f)| json!({ "id": id, "title": t, "status": st, "sent_at": at, "completed_at": done, "signed_copy": f.map(|f| format!("/api/public/portal/files/{f}")) })).collect::<Vec<_>>(),
        "tickets": tickets.into_iter().map(|(n, t, closed, at, last, secret)| {
            let link = secret.and_then(|s| crate::crypto::open(&state.config.data_key, &s)).map(|tok| format!("/help/t/{tok}"));
            json!({ "number": n, "title": t, "open": closed.is_none(), "created_at": at, "last_message_at": last, "link": link })
        }).collect::<Vec<_>>(),
        "bookings": bookings.into_iter().map(|(n, s, e, st, svc)| json!({ "number": n, "starts_at": s, "ends_at": e, "status": st, "service": svc })).collect::<Vec<_>>(),
    })))
}

/// A document's PDF or a signed copy, if it belongs to what the contact sees.
async fn file(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<Uuid>) -> AppResult<Response> {
    let (contact, wide) = client(&state, &headers).await?;
    let emails = emails_of(&state, contact).await?;
    let ok: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS (SELECT 1 FROM crm_documents d WHERE d.pdf_file_id = $3 AND d.status = 'ready' AND d.item_id IN ({VISIBLE}))
             OR EXISTS (SELECT 1 FROM sign_requests r WHERE r.signed_file_id = $3
                        AND (r.item_id IN ({VISIBLE}) OR r.item_id = $1 OR r.id IN (SELECT request_id FROM sign_signers WHERE lower(email) = ANY($4))))"
    )))
    .bind(contact)
    .bind(wide)
    .bind(id)
    .bind(&emails)
    .fetch_one(&state.db)
    .await?;
    if !ok {
        return Err(AppError::NotFound("file"));
    }
    let name: String = sqlx::query_scalar!("SELECT original_name FROM files WHERE id = $1", id).fetch_one(&state.db).await?;
    crate::routes::files::stream_id(&state, id, &name, &headers).await
}
