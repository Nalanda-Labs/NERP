//! One client, everything: a contact's or company's history across every module, newest
//! first — comments, deals and their stages, calls, meetings and to-dos, emails, live chats,
//! help desk tickets, invoices and payments, documents and signatures, bookings, site visits,
//! shop orders, web forms, marketing emails and tasks.
//!
//! The client's records are the client itself, a company's contacts, and every record whose
//! client they are (deals, leads, quotes, invoices, smart processes). Only records the viewer
//! may read count, and module-owned history (tickets, the books) only for those who work in
//! that module.
use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{access, COMPANY, CONTACT};
use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/crm/items/{id}/everything", get(everything))
}

/// The kinds of history, in the order the filter shows them.
pub const KINDS: &[(&str, &str)] = &[
    ("comment", "Comments"),
    ("record", "Deals and records"),
    ("activity", "Calls, meetings, to-dos"),
    ("email", "Emails"),
    ("chat", "Live chats"),
    ("ticket", "Help desk"),
    ("money", "Invoices and payments"),
    ("document", "Documents and signatures"),
    ("booking", "Bookings"),
    ("visit", "Site visits"),
    ("order", "Shop orders"),
    ("form", "Web forms"),
    ("marketing", "Marketing emails"),
    ("task", "Tasks"),
];

#[derive(Deserialize)]
struct Q {
    /// Comma-separated kinds (default: all).
    kinds: Option<String>,
    /// Only history before this moment (paging).
    before: Option<DateTime<Utc>>,
    limit: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct Ev {
    at: DateTime<Utc>,
    kind: String,
    title: String,
    text: Option<String>,
    link: Option<String>,
    record_id: Option<Uuid>,
}

async fn everything(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<Q>) -> AppResult<Json<Value>> {
    let row = super::items::load_row(&state, id, false).await?;
    access::require_read(&state, &auth, row.entity_type, id, row.assigned_to).await?;
    if row.entity_type != CONTACT && row.entity_type != COMPANY {
        return Err(AppError::bad("the full history is for contacts and companies"));
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let before = q.before.unwrap_or_else(|| Utc::now() + chrono::Duration::days(3660));
    let wanted: Vec<&str> = match q.kinds.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(k) => k.split(',').map(str::trim).filter(|k| KINDS.iter().any(|x| x.0 == *k)).collect(),
        None => KINDS.iter().map(|k| k.0).collect(),
    };

    // The client: a company and its contacts, or a contact.
    let clients: Vec<Uuid> = if row.entity_type == COMPANY {
        let mut v: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM crm_items WHERE company_id = $1 AND entity_type = $2 AND deleted_at IS NULL")
            .bind(id)
            .bind(CONTACT)
            .fetch_all(&state.db)
            .await?;
        v.insert(0, id);
        v
    } else {
        vec![id]
    };
    // Every record of theirs, kept to those the viewer may read.
    let linked: Vec<(Uuid, i16, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT i.id, i.entity_type, i.assigned_to, t.code FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type
         WHERE i.deleted_at IS NULL AND (i.id = ANY($1) OR i.company_id = ANY($1) OR i.id IN (SELECT item_id FROM crm_item_contacts WHERE contact_id = ANY($1)))
         LIMIT 2000",
    )
    .bind(&clients)
    .fetch_all(&state.db)
    .await?;
    let observing: Vec<Uuid> = sqlx::query_scalar("SELECT item_id FROM crm_item_observers WHERE user_id = $1").bind(auth.id).fetch_all(&state.db).await?;
    let mut vis: HashMap<i16, access::Visible> = HashMap::new();
    let mut items = vec![];
    let mut codes: HashMap<Uuid, String> = HashMap::new();
    for (iid, et, assigned, code) in linked {
        if !vis.contains_key(&et) {
            vis.insert(et, access::visible(&state, &auth, et, "read").await?);
        }
        if iid == id || vis[&et].allows(assigned) || observing.contains(&iid) {
            items.push(iid);
            codes.insert(iid, code);
        }
    }
    let clients: Vec<Uuid> = clients.into_iter().filter(|c| items.contains(c)).collect();

    let support = auth.can(perm::SUPPORT_WORK) || auth.can(perm::SUPPORT_MANAGE);
    let books = auth.can(perm::ACCOUNTING_VIEW);
    let mut events: Vec<Ev> = vec![];
    let mut add = |mut v: Vec<Ev>| events.append(&mut v);
    macro_rules! source {
        ($kind:expr, $sql:expr) => {
            if wanted.contains(&$kind) {
                add(sqlx::query_as::<_, Ev>($sql).bind(&items).bind(&clients).bind(before).bind(limit).fetch_all(&state.db).await?);
            }
        };
    }
    source!("comment", "SELECT t.created_at AS at, 'comment' AS kind, 'Comment by ' || COALESCE(trim(u.first_name || ' ' || u.last_name), 'someone') AS title,
                left(t.body, 400) AS text, NULL AS link, t.item_id AS record_id
         FROM crm_timeline t LEFT JOIN users u ON u.id = t.author_id
         WHERE t.item_id = ANY($1) AND t.kind = 'comment' AND t.created_at < $3 ORDER BY t.created_at DESC LIMIT $4");
    source!("record", "SELECT * FROM (
           SELECT i.created_at AS at, 'record' AS kind, et.name || ' created: ' || COALESCE(i.title, '') AS title, NULL AS text, NULL AS link, i.id AS record_id
           FROM crm_items i JOIN crm_entity_types et ON et.id = i.entity_type WHERE i.id = ANY($1) AND NOT (i.id = ANY($2)) AND i.created_at < $3
           UNION ALL
           SELECT h.entered_at, 'record', COALESCE(i.title, '') || ': ' || CASE h.semantics WHEN 'success' THEN 'won (' || s.name || ')' WHEN 'failure' THEN 'lost (' || s.name || ')' ELSE 'moved to ' || s.name END,
                  NULL, NULL, i.id
           FROM crm_stage_history h JOIN crm_items i ON i.id = h.item_id JOIN crm_stages s ON s.id = h.stage_id
           WHERE h.item_id = ANY($1) AND NOT (h.item_id = ANY($2)) AND h.entered_at < $3
             AND h.entered_at > i.created_at + interval '2 seconds'
         ) x ORDER BY at DESC LIMIT $4");
    source!("activity", "SELECT COALESCE(a.completed_at, a.starts_at, a.created_at) AS at, 'activity' AS kind,
                initcap(replace(a.kind, '_', ' ')) || CASE WHEN a.is_completed THEN '' ELSE ' (planned)' END || ': ' || a.subject AS title,
                NULLIF(left(COALESCE(a.result, a.description, ''), 300), '') AS text, NULL AS link, a.item_id AS record_id
         FROM crm_activities a WHERE a.item_id = ANY($1) AND a.kind <> 'email' AND COALESCE(a.completed_at, a.starts_at, a.created_at) < $3
         ORDER BY 1 DESC LIMIT $4");
    source!("email", "SELECT DISTINCT ON (e.sent_at, e.id) e.sent_at AS at, 'email' AS kind,
                CASE e.direction WHEN 'in' THEN 'Email from ' || COALESCE(NULLIF(e.from_name, ''), e.from_addr) ELSE 'Email sent' END || ': ' || COALESCE(NULLIF(e.subject, ''), '(no subject)') AS title,
                left(e.snippet, 300) AS text, NULL AS link, l.item_id AS record_id
         FROM crm_emails e JOIN crm_email_links l ON l.email_id = e.id
         WHERE l.item_id = ANY($1) AND e.status <> 'failed' AND e.sent_at < $3 ORDER BY e.sent_at DESC, e.id LIMIT $4");
    source!("chat", "SELECT c.started_at AS at, 'chat' AS kind, 'Live chat' || COALESCE(' with ' || NULLIF(c.visitor_name, ''), '') AS title,
                CASE WHEN c.rating IS NOT NULL THEN 'Rated ' || c.rating ELSE NULL END AS text, NULL AS link, c.item_id AS record_id
         FROM chat_sessions c WHERE c.item_id = ANY($1) AND c.started_at < $3 ORDER BY c.started_at DESC LIMIT $4");
    if support {
        source!("ticket", "SELECT t.created_at AS at, 'ticket' AS kind, 'Ticket #' || t.number || ': ' || t.title AS title,
                CASE WHEN t.closed_at IS NOT NULL THEN 'Closed' ELSE 'Open' END AS text, '/support/tickets/' || t.id AS link, t.contact_id AS record_id
             FROM support_tickets t WHERE t.contact_id = ANY($2) AND NOT t.is_spam AND t.created_at < $3 ORDER BY t.created_at DESC LIMIT $4");
    }
    if books {
        source!("money", "SELECT * FROM (
               SELECT a.date::timestamp AT TIME ZONE 'UTC' AS at, 'money' AS kind, 'Invoice ' || COALESCE(i.title, '') || ' — ' || a.total || ' ' || a.currency AS title,
                      initcap(a.status) AS text, NULL AS link, a.item_id AS record_id
               FROM acc_invoices a JOIN crm_items i ON i.id = a.item_id WHERE (a.party_id = ANY($2) OR a.item_id = ANY($1))
               UNION ALL
               SELECT p.date::timestamp AT TIME ZONE 'UTC', 'money', CASE p.direction WHEN 'in' THEN 'Payment received' ELSE 'Payment made' END || ' — ' || p.amount || ' ' || p.currency,
                      NULLIF(p.reference, ''), NULL, p.party_id
               FROM acc_payments p WHERE p.party_id = ANY($2) AND p.status = 'posted'
             ) x WHERE at < $3 ORDER BY at DESC LIMIT $4");
    }
    source!("document", "SELECT * FROM (
           SELECT d.created_at AS at, 'document' AS kind, 'Document ' || COALESCE(d.number, '') || ' (' || d.template_name || ')' AS title, NULL AS text, NULL AS link, d.item_id AS record_id
           FROM crm_documents d WHERE d.item_id = ANY($1)
           UNION ALL
           SELECT s.created_at, 'document', 'Sent for signature: ' || s.title, NULL, NULL, s.item_id FROM sign_requests s WHERE s.item_id = ANY($1)
           UNION ALL
           SELECT s.completed_at, 'document', 'Signed: ' || s.title, NULL, NULL, s.item_id FROM sign_requests s WHERE s.item_id = ANY($1) AND s.completed_at IS NOT NULL AND s.status = 'completed'
         ) x WHERE at < $3 ORDER BY at DESC LIMIT $4");
    source!("booking", "SELECT b.starts_at AS at, 'booking' AS kind, 'Booking: ' || COALESCE(sv.name, 'appointment') || CASE WHEN b.status <> 'confirmed' THEN ' (' || b.status || ')' ELSE '' END AS title,
                NULLIF(b.note, '') AS text, NULL AS link, COALESCE(b.deal_id, b.contact_id) AS record_id
         FROM bookings b LEFT JOIN booking_services sv ON sv.id = b.service_id
         WHERE (b.contact_id = ANY($2) OR b.deal_id = ANY($1)) AND b.starts_at < $3 ORDER BY b.starts_at DESC LIMIT $4");
    source!("visit", "SELECT v.started_at AS at, 'visit' AS kind, 'Visited the site' || COALESCE(' from ' || NULLIF(v.source, ''), COALESCE(' (' || NULLIF(v.channel, '') || ')', '')) AS title,
                v.pages || ' pages' || COALESCE(', landed on ' || NULLIF(v.landing, ''), '') AS text, NULL AS link, tv.item_id AS record_id
         FROM track_visits v JOIN track_visitors tv ON tv.id = v.visitor_id WHERE tv.item_id = ANY($1) AND v.started_at < $3 ORDER BY v.started_at DESC LIMIT $4");
    source!("order", "SELECT o.created_at AS at, 'order' AS kind, 'Shop order: ' || COALESCE(i.title, '') AS title, NULL AS text, NULL AS link, o.item_id AS record_id
         FROM shop_orders o JOIN crm_items i ON i.id = o.item_id WHERE o.item_id = ANY($1) AND o.created_at < $3 ORDER BY o.created_at DESC LIMIT $4");
    source!("form", "SELECT s.created_at AS at, 'form' AS kind, 'Filled in the form ' || f.name AS title, NULLIF(s.page_url, '') AS text, NULL AS link, COALESCE(s.item_id, s.contact_id) AS record_id
         FROM crm_webform_submissions s JOIN crm_webforms f ON f.id = s.form_id
         WHERE (s.item_id = ANY($1) OR s.contact_id = ANY($2)) AND s.created_at < $3 ORDER BY s.created_at DESC LIMIT $4");
    source!("marketing", "SELECT r.sent_at AS at, 'marketing' AS kind, 'Campaign email: ' || c.name AS title,
                concat_ws(', ', CASE WHEN r.opened_at IS NOT NULL THEN 'opened' END, CASE WHEN r.clicked_at IS NOT NULL THEN 'clicked' END, CASE WHEN r.unsubscribed_at IS NOT NULL THEN 'unsubscribed' END) AS text,
                NULL AS link, r.item_id AS record_id
         FROM mkt_recipients r JOIN mkt_campaigns c ON c.id = r.campaign_id WHERE r.item_id = ANY($1) AND r.sent_at IS NOT NULL AND r.sent_at < $3 ORDER BY r.sent_at DESC LIMIT $4");
    source!("task", "SELECT * FROM (
           SELECT t.created_at AS at, 'task' AS kind, 'Task: ' || t.title AS title, NULL AS text, '/tasks/' || t.id AS link, l.item_id AS record_id
           FROM task_crm_links l JOIN tasks t ON t.id = l.task_id WHERE l.item_id = ANY($1)
           UNION ALL
           SELECT t.completed_at, 'task', 'Task done: ' || t.title, NULL, '/tasks/' || t.id, l.item_id
           FROM task_crm_links l JOIN tasks t ON t.id = l.task_id WHERE l.item_id = ANY($1) AND t.completed_at IS NOT NULL
         ) x WHERE at < $3 ORDER BY at DESC LIMIT $4");

    events.sort_by(|a, b| b.at.cmp(&a.at));
    let more = events.len() as i64 > limit;
    events.truncate(limit as usize);
    let titles: HashMap<Uuid, String> = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, COALESCE(NULLIF(title, ''), NULLIF(trim(concat_ws(' ', first_name, last_name)), ''), company_name, '') FROM crm_items WHERE id = ANY($1)",
    )
    .bind(events.iter().filter_map(|e| e.record_id).collect::<Vec<_>>())
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .collect();
    let out: Vec<Value> = events
        .iter()
        .map(|e| {
            let rec = e.record_id.filter(|r| codes.contains_key(r));
            json!({
                "at": e.at, "kind": e.kind, "title": e.title, "text": e.text,
                "link": e.link.clone().or_else(|| rec.map(|r| format!("/crm/{}/{r}", codes[&r]))),
                "record": rec.map(|r| json!({ "id": r, "type": codes[&r], "title": titles.get(&r) })),
            })
        })
        .collect();
    let available: Vec<Value> = KINDS
        .iter()
        .filter(|(k, _)| (*k != "ticket" || support) && (*k != "money" || books))
        .map(|(k, l)| json!({ "kind": k, "label": l }))
        .collect();
    Ok(Json(json!({
        "items": out,
        "next_before": if more { events.last().map(|e| e.at) } else { None },
        "kinds": available,
        "records": items.len(),
    })))
}
