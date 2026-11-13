//! Requests from people about their personal data ("what do you have on me", "delete me").
//!
//! A request names the person by email (and optionally phone). NERP looks for them in every
//! module — CRM records (by their emails and phones), emails, live chats, help desk tickets,
//! bookings, shop orders, marketing emails, web forms, calendar invitations, signatures, site
//! visits, the books and the staff directory — and shows what it found per source.
//!
//! - **Access:** the report (JSON, every source with its rows) can be downloaded and sent.
//! - **Erasure:** a person with `privacy.manage` reviews each source and picks delete,
//!   anonymise or keep (with the reason, e.g. records the law requires), confirms with the
//!   person's email, and NERP does it in one transaction. The email goes on the marketing
//!   opt-out list so it isn't contacted again. Only counts are kept on the request.
//!
//! People can send a request themselves through the public form (/privacy-request); it
//! counts once they confirm it from the email it sends. Requests are due 30 days after.
use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    audit,
    auth::{permissions as perm, AuthUser, ClientIp},
    error::{AppError, AppResult},
    routes::normalize_email,
    state::AppState,
};

pub const PERM: &str = perm::PRIVACY_MANAGE;
const DUE_DAYS: i64 = 30;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/privacy/requests", get(list).post(create))
        .route("/privacy/requests/{id}", get(show).patch(update))
        .route("/privacy/requests/{id}/report", get(report))
        .route("/privacy/requests/{id}/erase", post(erase))
        .route("/public/privacy-request", post(public_request))
        .route("/public/privacy-request/verify", post(public_verify))
}

#[derive(Serialize, sqlx::FromRow)]
struct Request {
    id: Uuid,
    kind: String,
    email: String,
    phone: Option<String>,
    name: Option<String>,
    message: Option<String>,
    source: String,
    status: String,
    verified_at: Option<DateTime<Utc>>,
    due_at: Option<DateTime<Utc>>,
    created_by: Option<Uuid>,
    created_at: DateTime<Utc>,
    decided_by: Option<Uuid>,
    decided_by_name: Option<String>,
    decided_at: Option<DateTime<Utc>>,
    outcome: Option<Value>,
    note: Option<String>,
}

const SELECT: &str = "SELECT r.id, r.kind, r.email, r.phone, r.name, r.message, r.source, r.status, r.verified_at, r.due_at, r.created_by, r.created_at,
       r.decided_by, CASE WHEN u.id IS NULL THEN NULL ELSE trim(u.first_name || ' ' || u.last_name) END AS decided_by_name, r.decided_at, r.outcome, r.note
FROM privacy_requests r LEFT JOIN users u ON u.id = r.decided_by";

async fn load(state: &AppState, id: Uuid) -> AppResult<Request> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE r.id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("request"))
}

fn phone_key(p: &str) -> String {
    let d: String = p.chars().filter(|c| c.is_ascii_digit()).collect();
    if d.len() < 7 { String::new() } else { d[d.len().saturating_sub(10)..].to_string() }
}

// ---- finding the person ----

/// What can be done with a source on erasure.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Delete,
    Anonymise,
    Keep,
}

struct Source {
    key: &'static str,
    title: &'static str,
    /// What can be done, the first being the suggestion.
    actions: &'static [Action],
    note: &'static str,
    sql: &'static str,
}

/// $1 the email, $2 the phone key ('' for none), $3 the matched CRM records.
const SOURCES: &[Source] = &[
    Source {
        key: "crm",
        title: "CRM records (contacts, leads, companies)",
        actions: &[Action::Anonymise, Action::Keep],
        note: "Anonymising clears the names, emails, phones, address, comments, custom fields and notes and moves the record out of the lists; deals and invoices keep pointing at it.",
        sql: "SELECT i.id, t.name AS type, i.honorific, i.first_name, i.middle_name, i.last_name, i.company_name, i.job_title, i.birthdate, i.address, i.comments, i.uf, i.source, i.created_at,
                     (SELECT jsonb_agg(jsonb_build_object('kind', m.kind, 'type', m.value_type, 'value', m.value)) FROM crm_multifields m WHERE m.item_id = i.id) AS contact_details,
                     (SELECT jsonb_agg(jsonb_build_object('at', c.created_at, 'text', c.body) ORDER BY c.created_at) FROM crm_timeline c WHERE c.item_id = i.id AND c.kind = 'comment') AS notes,
                     (SELECT jsonb_agg(jsonb_build_object('at', a.created_at, 'kind', a.kind, 'subject', a.subject, 'description', a.description) ORDER BY a.created_at) FROM crm_activities a WHERE a.item_id = i.id) AS activities
              FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type WHERE i.id = ANY($3)",
    },
    Source {
        key: "emails",
        title: "Emails to and from them",
        actions: &[Action::Delete, Action::Keep],
        note: "",
        sql: "SELECT e.id, e.direction, e.from_addr, e.to_addrs, e.cc_addrs, e.subject, e.body_text, e.sent_at FROM crm_emails e
              WHERE lower(e.from_addr) = $1 OR e.to_addrs @> to_jsonb(ARRAY[$1]) OR e.cc_addrs @> to_jsonb(ARRAY[$1])
                 OR EXISTS (SELECT 1 FROM jsonb_array_elements(e.to_addrs || e.cc_addrs) x WHERE lower(COALESCE(x->>'email', x #>> '{}')) = $1)
              ORDER BY e.sent_at",
    },
    Source {
        key: "chats",
        title: "Live chats",
        actions: &[Action::Delete, Action::Keep],
        note: "",
        sql: "SELECT c.id, c.visitor_name, c.visitor_email, c.page_url, c.started_at, c.rating, c.rating_comment,
                     (SELECT jsonb_agg(jsonb_build_object('at', m.created_at, 'text', m.body) ORDER BY m.created_at) FROM chat_messages m WHERE m.session_id = c.id) AS messages
              FROM chat_sessions c WHERE lower(c.visitor_email) = $1 OR c.item_id = ANY($3) ORDER BY c.started_at",
    },
    Source {
        key: "tickets",
        title: "Help desk tickets",
        actions: &[Action::Delete, Action::Anonymise, Action::Keep],
        note: "Anonymising removes their name and email from the tickets and the text of their messages.",
        sql: "SELECT t.id, t.number, t.title, t.requester_name, t.requester_email, t.created_at, t.closed_at,
                     (SELECT jsonb_agg(jsonb_build_object('at', m.created_at, 'from', COALESCE(m.author_name, m.author_email), 'text', m.body) ORDER BY m.created_at)
                        FROM support_messages m WHERE m.ticket_id = t.id AND (lower(m.author_email) = $1 OR m.author_id IS NULL AND NOT m.by_support)) AS their_messages
              FROM support_tickets t WHERE lower(t.requester_email) = $1 OR t.contact_id = ANY($3) ORDER BY t.created_at",
    },
    Source {
        key: "bookings",
        title: "Bookings",
        actions: &[Action::Anonymise, Action::Delete, Action::Keep],
        note: "Anonymising keeps the booked time (for the schedule's history) without who booked it.",
        sql: "SELECT b.id, b.number, b.starts_at, b.ends_at, b.status, b.client_name, b.client_email, b.client_phone, b.note FROM bookings b
              WHERE lower(b.client_email) = $1 OR ($2 <> '' AND right(regexp_replace(COALESCE(b.client_phone, ''), '\\D', '', 'g'), 10) = $2) OR b.contact_id = ANY($3)
              ORDER BY b.starts_at",
    },
    Source {
        key: "orders",
        title: "Shop orders",
        actions: &[Action::Anonymise, Action::Keep],
        note: "Orders are sales records: anonymising keeps the order and its amounts without name, email, phone or address.",
        sql: "SELECT o.item_id AS id, o.name, o.email, o.phone, o.address, o.note, o.created_at FROM shop_orders o
              WHERE lower(o.email) = $1 OR ($2 <> '' AND right(regexp_replace(COALESCE(o.phone, ''), '\\D', '', 'g'), 10) = $2) ORDER BY o.created_at",
    },
    Source {
        key: "marketing",
        title: "Marketing emails",
        actions: &[Action::Delete, Action::Keep],
        note: "Their address stays on the opt-out list either way, so they aren't emailed again.",
        sql: "SELECT r.id, c.name AS campaign, r.email, r.status, r.sent_at, r.opened_at, r.clicked_at, r.unsubscribed_at FROM mkt_recipients r JOIN mkt_campaigns c ON c.id = r.campaign_id
              WHERE lower(r.email) = $1 ORDER BY r.sent_at",
    },
    Source {
        key: "forms",
        title: "Web form submissions",
        actions: &[Action::Delete, Action::Keep],
        note: "",
        sql: "SELECT s.id, f.name AS form, s.data, s.page_url, s.created_at FROM crm_webform_submissions s JOIN crm_webforms f ON f.id = s.form_id
              WHERE s.contact_id = ANY($3) OR s.item_id = ANY($3) OR EXISTS (SELECT 1 FROM jsonb_path_query(s.data, 'strict $.**') v WHERE jsonb_typeof(v) = 'string' AND lower(trim(v #>> '{}')) = $1) ORDER BY s.created_at",
    },
    Source {
        key: "calendar",
        title: "Calendar invitations",
        actions: &[Action::Delete, Action::Keep],
        note: "",
        sql: "SELECT g.id, e.title AS event, e.starts_at, g.name, g.email, g.status FROM cal_email_guests g JOIN cal_events e ON e.id = g.event_id WHERE lower(g.email) = $1 ORDER BY e.starts_at",
    },
    Source {
        key: "visits",
        title: "Site visits",
        actions: &[Action::Delete, Action::Keep],
        note: "",
        sql: "SELECT v.id, s.name AS site, v.started_at, v.channel, v.source, v.referrer, v.landing, v.pages FROM track_visits v JOIN track_visitors tv ON tv.id = v.visitor_id
              JOIN track_sites s ON s.id = v.site_id WHERE tv.item_id = ANY($3) ORDER BY v.started_at",
    },
    Source {
        key: "signatures",
        title: "Signatures",
        actions: &[Action::Keep, Action::Anonymise],
        note: "Signed documents are usually kept as evidence of the agreement.",
        sql: "SELECT g.id, r.title AS document, g.name, g.email, g.status, g.signed_at, g.signed_name, g.ip FROM sign_signers g JOIN sign_requests r ON r.id = g.request_id
              WHERE lower(g.email) = $1 ORDER BY g.signed_at",
    },
    Source {
        key: "books",
        title: "Invoices and payments in the books",
        actions: &[Action::Keep],
        note: "Accounting records are kept for as long as the law requires; their CRM record is anonymised instead.",
        sql: "SELECT a.item_id AS id, 'invoice' AS kind, a.date, a.total, a.currency, a.status FROM acc_invoices a WHERE a.party_id = ANY($3)
              UNION ALL SELECT p.id, 'payment', p.date, p.amount, p.currency, p.status FROM acc_payments p WHERE p.party_id = ANY($3)",
    },
    Source {
        key: "staff",
        title: "Employee account",
        actions: &[Action::Keep],
        note: "An employee's own data is handled through HR (deactivate or delete the employee there).",
        sql: "SELECT u.id, u.first_name, u.last_name, u.email, u.phone, u.is_active, u.created_at FROM users u WHERE lower(u.email) = $1 AND NOT u.is_guest",
    },
    Source {
        key: "optout",
        title: "Marketing opt-out",
        actions: &[Action::Keep],
        note: "Kept so they aren't emailed again.",
        sql: "SELECT o.email, o.source, o.created_at FROM mkt_optouts o WHERE lower(o.email) = $1",
    },
];

async fn matched_items(state: &AppState, email: &str, phone: &str) -> AppResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar!("SELECT DISTINCT m.item_id FROM crm_multifields m JOIN crm_items i ON i.id = m.item_id
         WHERE (m.kind = 'email' AND m.normalized = $1) OR (m.kind = 'phone' AND $2 <> '' AND m.normalized = $2)", email, phone)
    .fetch_all(&state.db)
    .await?)
}

/// Every source with what it holds on the person.
async fn find(state: &AppState, r: &Request) -> AppResult<(Vec<Uuid>, Vec<Value>)> {
    let email = r.email.to_lowercase();
    let phone = r.phone.as_deref().map(phone_key).unwrap_or_default();
    let items = matched_items(state, &email, &phone).await?;
    let mut out = vec![];
    for s in SOURCES {
        let rows: Vec<Value> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT to_jsonb(x) FROM ({}) x", s.sql)))
            .bind(&email)
            .bind(&phone)
            .bind(&items)
            .fetch_all(&state.db)
            .await?;
        out.push(json!({ "key": s.key, "title": s.title, "count": rows.len(), "actions": s.actions, "note": s.note, "rows": rows }));
    }
    Ok((items, out))
}

// ---- staff routes ----

#[derive(Deserialize)]
struct ListQ {
    status: Option<String>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQ>) -> AppResult<Json<Value>> {
    auth.require(PERM)?;
    let items: Vec<Request> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE ($1::text IS NULL OR r.status = $1) ORDER BY (r.status = 'open') DESC, r.due_at NULLS LAST, r.created_at DESC LIMIT 200"
    )))
    .bind(q.status.filter(|s| !s.is_empty()))
    .fetch_all(&state.db)
    .await?;
    let overdue = items.iter().filter(|r| r.status == "open" && r.due_at.is_some_and(|d| d < Utc::now())).count();
    Ok(Json(json!({ "items": items, "overdue": overdue })))
}

#[derive(Deserialize)]
struct NewRequest {
    kind: String,
    email: String,
    phone: Option<String>,
    name: Option<String>,
    message: Option<String>,
}

fn check_new(b: &NewRequest) -> AppResult<String> {
    if !["access", "erasure"].contains(&b.kind.as_str()) {
        return Err(AppError::bad("kind must be access or erasure"));
    }
    if b.name.as_deref().is_some_and(|n| n.len() > 200) || b.message.as_deref().is_some_and(|m| m.len() > 5000) || b.phone.as_deref().is_some_and(|p| p.len() > 40) {
        return Err(AppError::bad("too long"));
    }
    normalize_email(&b.email)
}

async fn create(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Json(b): Json<NewRequest>) -> AppResult<Json<Request>> {
    auth.require(PERM)?;
    let email = check_new(&b)?;
    let id: Uuid = sqlx::query_scalar!("INSERT INTO privacy_requests (kind, email, phone, name, message, source, status, verified_at, due_at, created_by)
         VALUES ($1, $2, NULLIF(trim($3), ''), NULLIF(trim($4), ''), NULLIF(trim($5), ''), 'staff', 'open', now(), now() + make_interval(days => $6::int), $7) RETURNING id", &b.kind, &email, b.phone.as_deref(), b.name.as_deref(), b.message.as_deref(), DUE_DAYS as i32, auth.id)
    .fetch_one(&state.db)
    .await?;
    audit::record(&state.db, Some(auth.id), "privacy.request", "privacy_request", Some(id.to_string()), json!({ "kind": b.kind }), ip.as_deref()).await;
    Ok(Json(load(&state, id).await?))
}

async fn show(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    auth.require(PERM)?;
    let r = load(&state, id).await?;
    // What was found, without the rows (the report has them): counts and a few labels.
    let (_, sources) = if r.status == "done" { (vec![], vec![]) } else { find(&state, &r).await? };
    let summary: Vec<Value> = sources
        .into_iter()
        .map(|mut s| {
            let rows = s["rows"].take();
            s["sample"] = json!(rows.as_array().map(|a| a.iter().take(5).map(label).collect::<Vec<_>>()).unwrap_or_default());
            s
        })
        .collect();
    Ok(Json(json!({ "request": r, "sources": summary })))
}

/// A short label for a found row.
fn label(v: &Value) -> String {
    let pick = |k: &str| v.get(k).and_then(|x| x.as_str()).filter(|s| !s.is_empty()).map(String::from);
    let name = [pick("first_name"), pick("last_name")].into_iter().flatten().collect::<Vec<_>>().join(" ");
    let parts: Vec<String> = [
        (!name.is_empty()).then_some(name),
        pick("company_name"),
        pick("subject"),
        pick("title"),
        pick("campaign"),
        pick("form"),
        pick("event"),
        pick("document"),
        pick("site"),
        pick("type"),
        pick("kind"),
        v.get("number").map(|n| format!("#{n}")),
        pick("started_at").or_else(|| pick("sent_at")).or_else(|| pick("starts_at")).or_else(|| pick("created_at")).or_else(|| pick("date")).map(|d| d.chars().take(10).collect()),
    ]
    .into_iter()
    .flatten()
    .take(3)
    .collect();
    parts.join(" · ")
}

#[derive(Deserialize)]
struct Patch {
    status: Option<String>,
    note: Option<String>,
}

/// Closes an access request (report sent) or rejects a request, with a note.
async fn update(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(id): Path<Uuid>, Json(b): Json<Patch>) -> AppResult<Json<Request>> {
    auth.require(PERM)?;
    let r = load(&state, id).await?;
    if let Some(st) = b.status.as_deref() {
        if !["done", "rejected", "open"].contains(&st) {
            return Err(AppError::bad("status must be open, done or rejected"));
        }
        if st == "done" && r.kind == "erasure" {
            return Err(AppError::bad("an erasure request is done by erasing"));
        }
        if r.status == "unverified" && st != "rejected" {
            return Err(AppError::bad("the person hasn't confirmed the request yet"));
        }
    }
    sqlx::query!("UPDATE privacy_requests SET status = COALESCE($2, status), note = COALESCE($3, note),
                decided_by = CASE WHEN $2 IN ('done', 'rejected') THEN $4 ELSE decided_by END,
                decided_at = CASE WHEN $2 IN ('done', 'rejected') THEN now() ELSE decided_at END WHERE id = $1", id, b.status.as_deref(), b.note.as_deref().map(str::trim), auth.id)
    .execute(&state.db)
    .await?;
    if let Some(st) = &b.status {
        audit::record(&state.db, Some(auth.id), &format!("privacy.{st}"), "privacy_request", Some(id.to_string()), json!({}), ip.as_deref()).await;
    }
    Ok(Json(load(&state, id).await?))
}

/// The full report: every source with every row, as a JSON download.
async fn report(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(id): Path<Uuid>) -> AppResult<Response> {
    auth.require(PERM)?;
    let r = load(&state, id).await?;
    if r.status == "unverified" {
        return Err(AppError::bad("the person hasn't confirmed the request yet"));
    }
    if r.status == "done" && r.kind == "erasure" {
        return Err(AppError::bad("the data was erased"));
    }
    let (_, sources) = find(&state, &r).await?;
    let company = crate::routes::auth::company_name(&state).await;
    let body = json!({
        "about": { "email": r.email, "phone": r.phone, "name": r.name },
        "controller": company,
        "generated_at": Utc::now(),
        "request": { "id": r.id, "received": r.created_at, "kind": r.kind },
        "sources": sources.into_iter().filter(|s| s["count"].as_u64().unwrap_or(0) > 0).map(|mut s| { s.as_object_mut().map(|o| { o.remove("actions"); }); s }).collect::<Vec<_>>(),
    });
    audit::record(&state.db, Some(auth.id), "privacy.report", "privacy_request", Some(id.to_string()), json!({}), ip.as_deref()).await;
    let name = format!("personal-data-{}.json", Utc::now().format("%Y-%m-%d"));
    Ok((
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
        ],
        serde_json::to_vec_pretty(&body).unwrap_or_default(),
    )
        .into_response())
}

#[derive(Deserialize)]
struct EraseInput {
    /// source key → action
    actions: HashMap<String, Action>,
    /// The person's email again, as a confirmation.
    confirm: String,
    note: Option<String>,
}

async fn erase(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(id): Path<Uuid>, Json(b): Json<EraseInput>) -> AppResult<Json<Request>> {
    auth.require(PERM)?;
    let r = load(&state, id).await?;
    if r.kind != "erasure" || r.status != "open" {
        return Err(AppError::bad("only an open erasure request can be carried out"));
    }
    if b.confirm.trim().to_lowercase() != r.email.to_lowercase() {
        return Err(AppError::bad("type the person's email to confirm"));
    }
    let email = r.email.to_lowercase();
    let phone = r.phone.as_deref().map(phone_key).unwrap_or_default();
    let items = matched_items(&state, &email, &phone).await?;
    let mut chosen: HashMap<&str, Action> = HashMap::new();
    for s in SOURCES {
        let a = b.actions.get(s.key).copied().unwrap_or(s.actions[0]);
        if !s.actions.contains(&a) {
            return Err(AppError::bad(format!("{}: can't {:?}", s.title, a).to_lowercase()));
        }
        chosen.insert(s.key, a);
    }
    let mut tx = state.db.begin().await?;
    let mut outcome = serde_json::Map::new();
    let mut done = |k: &str, a: Action, n: u64| {
        outcome.insert(k.into(), json!({ "action": a, "count": n }));
    };
    macro_rules! run {
        ($sql:expr) => {
            sqlx::query($sql).bind(&email).bind(&phone).bind(&items).execute(&mut *tx).await?.rows_affected()
        };
    }
    // Files attached to their records' comments go with them.
    let mut files: Vec<(Uuid, String)> = vec![];
    match chosen["crm"] {
        Action::Anonymise => {
            let (n, f) = anonymise_crm(&mut tx, &items).await?;
            files = f;
            done("crm", Action::Anonymise, n);
        }
        a => done("crm", a, 0),
    }
    let emails_sql = "DELETE FROM crm_emails e WHERE lower(e.from_addr) = $1 OR e.to_addrs @> to_jsonb(ARRAY[$1]) OR e.cc_addrs @> to_jsonb(ARRAY[$1])
                      OR EXISTS (SELECT 1 FROM jsonb_array_elements(e.to_addrs || e.cc_addrs) x WHERE lower(COALESCE(x->>'email', x #>> '{}')) = $1) AND $2 IS NOT NULL AND $3 IS NOT NULL";
    let n = if chosen["emails"] == Action::Delete { run!(emails_sql) } else { 0 };
    done("emails", chosen["emails"], n);
    let n = if chosen["chats"] == Action::Delete { run!("DELETE FROM chat_sessions WHERE (lower(visitor_email) = $1 OR item_id = ANY($3)) AND $2 IS NOT NULL") } else { 0 };
    done("chats", chosen["chats"], n);
    let n = match chosen["tickets"] {
        Action::Delete => run!("DELETE FROM support_tickets WHERE (lower(requester_email) = $1 OR contact_id = ANY($3)) AND $2 IS NOT NULL"),
        Action::Anonymise => {
            run!("UPDATE support_messages SET body = 'Erased', author_name = NULL, author_email = NULL, file_ids = '{}'
                  WHERE ticket_id IN (SELECT id FROM support_tickets WHERE lower(requester_email) = $1 OR contact_id = ANY($3)) AND NOT by_support AND $2 IS NOT NULL");
            run!("UPDATE support_tickets SET requester_name = 'Erased', requester_email = NULL, title = 'Erased', contact_id = NULL
                  WHERE (lower(requester_email) = $1 OR contact_id = ANY($3)) AND $2 IS NOT NULL")
        }
        Action::Keep => 0,
    };
    done("tickets", chosen["tickets"], n);
    let bookings_where = "(lower(client_email) = $1 OR ($2 <> '' AND right(regexp_replace(COALESCE(client_phone, ''), '\\D', '', 'g'), 10) = $2) OR contact_id = ANY($3))";
    let n = match chosen["bookings"] {
        Action::Delete => run!(sqlx::AssertSqlSafe(format!("DELETE FROM bookings WHERE {bookings_where}"))),
        Action::Anonymise => run!(sqlx::AssertSqlSafe(format!("UPDATE bookings SET client_name = 'Erased', client_email = NULL, client_phone = NULL, note = NULL, contact_id = NULL WHERE {bookings_where}"))),
        Action::Keep => 0,
    };
    done("bookings", chosen["bookings"], n);
    let n = if chosen["orders"] == Action::Anonymise {
        run!("UPDATE shop_orders SET name = 'Erased', email = 'erased@invalid', phone = NULL, address = '', note = NULL
              WHERE lower(email) = $1 OR ($2 <> '' AND right(regexp_replace(COALESCE(phone, ''), '\\D', '', 'g'), 10) = $2) AND $3 IS NOT NULL")
    } else {
        0
    };
    done("orders", chosen["orders"], n);
    let n = if chosen["marketing"] == Action::Delete { run!("DELETE FROM mkt_recipients WHERE lower(email) = $1 AND $2 IS NOT NULL AND $3 IS NOT NULL") } else { 0 };
    done("marketing", chosen["marketing"], n);
    let n = if chosen["forms"] == Action::Delete {
        run!("DELETE FROM crm_webform_submissions s WHERE (s.contact_id = ANY($3) OR s.item_id = ANY($3) OR EXISTS (SELECT 1 FROM jsonb_path_query(s.data, 'strict $.**') v WHERE jsonb_typeof(v) = 'string' AND lower(trim(v #>> '{}')) = $1)) AND $2 IS NOT NULL")
    } else {
        0
    };
    done("forms", chosen["forms"], n);
    let n = if chosen["calendar"] == Action::Delete { run!("DELETE FROM cal_email_guests WHERE lower(email) = $1 AND $2 IS NOT NULL AND $3 IS NOT NULL") } else { 0 };
    done("calendar", chosen["calendar"], n);
    let n = if chosen["visits"] == Action::Delete { run!("DELETE FROM track_visitors WHERE item_id = ANY($3) AND $1 IS NOT NULL AND $2 IS NOT NULL") } else { 0 };
    done("visits", chosen["visits"], n);
    let n = if chosen["signatures"] == Action::Anonymise {
        run!("UPDATE sign_signers SET name = 'Erased', email = 'erased@invalid', signed_name = CASE WHEN signed_name IS NULL THEN NULL ELSE 'Erased' END, ip = NULL, user_agent = NULL
              WHERE lower(email) = $1 AND $2 IS NOT NULL AND $3 IS NOT NULL")
    } else {
        0
    };
    done("signatures", chosen["signatures"], n);
    done("books", Action::Keep, 0);
    done("staff", Action::Keep, 0);
    // Not to be emailed again.
    sqlx::query!("INSERT INTO mkt_optouts (email, source, created_by) SELECT $1, 'erasure', $2 WHERE NOT EXISTS (SELECT 1 FROM mkt_optouts WHERE lower(email) = $1)", &email, auth.id)
        .execute(&mut *tx)
        .await?;
    done("optout", Action::Keep, 1);
    sqlx::query!("UPDATE privacy_requests SET status = 'done', outcome = $2, decided_by = $3, decided_at = now(), note = COALESCE(NULLIF(trim($4), ''), note) WHERE id = $1", id, Value::Object(outcome.clone()), auth.id, b.note.as_deref())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    // The removed files' content goes once the change is committed.
    for (_, rel) in files {
        crate::blob::discard(&state, &rel).await;
    }
    audit::record(&state.db, Some(auth.id), "privacy.erase", "privacy_request", Some(id.to_string()), json!({ "outcome": outcome }), ip.as_deref()).await;
    Ok(Json(load(&state, id).await?))
}

/// Anonymises CRM records: names, contact details, address, notes and their files, custom
/// fields, requisites; activities keep only that they happened. The records leave the lists
/// (deals and invoices keep pointing at them). Returns how many and the files to discard
/// once committed (their rows are already gone).
pub async fn anonymise_crm(tx: &mut sqlx::PgConnection, items: &[Uuid]) -> sqlx::Result<(u64, Vec<(Uuid, String)>)> {
    let files: Vec<(Uuid, String)> = sqlx::query!(r#"SELECT f.id, f.storage_path FROM files f WHERE f.id IN (SELECT unnest(file_ids) FROM crm_timeline WHERE item_id = ANY($1) AND kind = 'comment' AND file_ids IS NOT NULL)"#, items).map(|r| (r.id, r.storage_path))
    .fetch_all(&mut *tx)
    .await?;
    for q in [
        "DELETE FROM crm_multifields WHERE item_id = ANY($1)",
        "DELETE FROM crm_timeline WHERE item_id = ANY($1) AND kind = 'comment'",
        "UPDATE crm_activities SET subject = 'Erased', description = NULL, result = NULL WHERE item_id = ANY($1)",
        "DELETE FROM crm_requisites WHERE item_id = ANY($1)",
    ] {
        sqlx::query(q).bind(items).execute(&mut *tx).await?;
    }
    let n = sqlx::query!("UPDATE crm_items SET honorific = NULL, first_name = CASE WHEN entity_type = 3 OR first_name IS NOT NULL THEN 'Erased' END, middle_name = NULL, last_name = NULL,
                company_name = CASE WHEN company_name IS NOT NULL THEN 'Erased' END, title = CASE WHEN entity_type IN (1, 3, 4) THEN 'Erased' ELSE title END,
                job_title = NULL, birthdate = NULL, address = NULL, comments = NULL, uf = '{}'::jsonb, source_details = NULL, utm = NULL, search_text = '',
                deleted_at = COALESCE(deleted_at, now()), updated_at = now()
         WHERE id = ANY($1)", items)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let ids: Vec<Uuid> = files.iter().map(|f| f.0).collect();
    sqlx::query!("DELETE FROM files WHERE id = ANY($1)", &ids).execute(&mut *tx).await?;
    Ok((n, files))
}

// ---- the public form ----

#[derive(Deserialize)]
struct PublicRequest {
    kind: String,
    email: String,
    name: Option<String>,
    message: Option<String>,
}

async fn limited(state: &AppState, key: String, max: i64, secs: i64) -> AppResult<bool> {
    let mut redis = state.redis.clone();
    let n: i64 = redis.incr(&key, 1).await?;
    if n == 1 {
        let _: () = redis.expire(&key, secs).await?;
    }
    Ok(n > max)
}

/// Anyone can ask; the request counts once confirmed from the email. The answer is the same
/// whatever the address, so it tells nothing about who is on file.
async fn public_request(State(state): State<AppState>, ClientIp(ip): ClientIp, Json(b): Json<PublicRequest>) -> AppResult<Json<Value>> {
    let email = check_new(&NewRequest { kind: b.kind.clone(), email: b.email.clone(), phone: None, name: b.name.clone(), message: b.message.clone() })?;
    if limited(&state, format!("privacy:ip:{}", ip.as_deref().unwrap_or("unknown")), 5, 3600).await? || limited(&state, format!("privacy:email:{email}"), 3, 86400).await? {
        return Err(AppError::TooManyRequests("too many requests; try again later".into()));
    }
    let token = crate::auth::tokens::random_token(32);
    let hash = hex::encode(Sha256::digest(token.as_bytes()));
    sqlx::query!("INSERT INTO privacy_requests (kind, email, name, message, source, status, verify_hash) VALUES ($1, $2, NULLIF(trim($3), ''), NULLIF(trim($4), ''), 'public', 'unverified', $5)", &b.kind, &email, b.name.as_deref(), b.message.as_deref(), &hash)
    .execute(&state.db)
    .await?;
    let company = crate::routes::auth::company_name(&state).await;
    let link = format!("{}/privacy-request/confirm?token={token}", state.config.public_url);
    let what = if b.kind == "erasure" { "delete the personal data we hold about you" } else { "send you a copy of the personal data we hold about you" };
    let mail = crate::mail::notice(
        &email,
        &company,
        &format!("Confirm your request to {company}"),
        "Confirm your request",
        &[format!("We received a request to {what}. To make sure it comes from you, confirm it within 7 days."), "If you didn't ask for this, ignore this email.".into()],
        Some(("Confirm the request", &link)),
        "You get this email because someone entered this address in our personal data request form.",
    );
    crate::jobs::send_email(&state, mail).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct VerifyInput {
    token: String,
}

async fn public_verify(State(state): State<AppState>, Json(b): Json<VerifyInput>) -> AppResult<Json<Value>> {
    let hash = hex::encode(Sha256::digest(b.token.trim().as_bytes()));
    let row: Option<(Uuid, String)> = sqlx::query!(r#"UPDATE privacy_requests SET status = 'open', verified_at = now(), due_at = now() + make_interval(days => $2::int), verify_hash = NULL
         WHERE verify_hash = $1 AND status = 'unverified' AND created_at > now() - interval '7 days' RETURNING id, kind"#, &hash, DUE_DAYS as i32).map(|r| (r.id, r.kind))
    .fetch_optional(&state.db)
    .await?;
    let Some((id, kind)) = row else { return Err(AppError::NotFound("request")) };
    // Tell those who handle requests.
    let people: Vec<Uuid> = sqlx::query_scalar!("SELECT DISTINCT ur.user_id FROM user_roles ur JOIN role_permissions rp ON rp.role_id = ur.role_id JOIN users u ON u.id = ur.user_id
         WHERE rp.permission IN ('*', $1) AND u.is_active AND NOT u.is_guest", PERM)
    .fetch_all(&state.db)
    .await?;
    crate::notify::notify(
        &state,
        &people,
        crate::notify::NewNotification {
            kind: "privacy.request",
            title: format!("New personal data request ({})", if kind == "erasure" { "erasure" } else { "access" }),
            body: Some(format!("Due within {DUE_DAYS} days.")),
            link: Some(format!("/privacy/{id}")),
            actor: None,
        },
    )
    .await;
    audit::record(&state.db, None, "privacy.request_confirmed", "privacy_request", Some(id.to_string()), json!({ "kind": kind }), None).await;
    Ok(Json(json!({ "ok": true, "kind": kind, "due_days": DUE_DAYS })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phones() {
        assert_eq!(phone_key("+44 20 7946 0000"), "2079460000");
        assert_eq!(phone_key("12"), "");
    }

    #[test]
    fn every_source_suggests_an_action() {
        assert!(SOURCES.iter().all(|s| !s.actions.is_empty()));
    }
}
