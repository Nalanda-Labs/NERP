//! Receiving: fetch new messages from a mailbox's inbox and sent folder over IMAP, and
//! file the ones that concern CRM records.
//!
//! A message is filed when (a) it belongs to a conversation already in the CRM, or
//! (b) another party's address matches a lead, contact or company. Messages from unknown
//! senders create a lead if the mailbox asks for it; everything else is left alone, so
//! private mail never enters the CRM.

use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use mail_parser::{AddressList, MessageParser};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use uuid::Uuid;

use super::{imap, MailboxRow};
use crate::{
    crm::{
        automation::{fire, system_auth},
        broadcast, entity_type,
    },
    error::AppResult,
    routes::files,
    state::AppState,
};

/// Messages handled per folder per run (the rest come next time).
const BATCH: usize = 300;
const MAX_MESSAGE_BYTES: u32 = 30 * 1024 * 1024;

fn addr_list(l: Option<AddressList<'_>>) -> Vec<(String, String)> {
    l.map(|l| l.mailboxes().filter_map(|m| m.address().map(|a| (a.trim().to_lowercase(), m.name().unwrap_or("").trim().to_string()))).collect())
        .unwrap_or_default()
}

fn as_json(v: &[(String, String)]) -> Value {
    json!(v.iter().map(|(a, n)| json!({ "address": a, "name": n })).collect::<Vec<_>>())
}

/// Sanitises HTML for display: no scripts, frames, forms or event handlers; inline
/// styles are kept (the viewer renders in a sandboxed frame that loads nothing).
pub fn sanitize_html(html: &str) -> String {
    ammonia::Builder::default().add_generic_attributes(["style", "align", "valign", "bgcolor", "width", "height"]).link_rel(Some("noopener noreferrer")).clean(html).to_string()
}

pub fn snippet(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect()
}

/// What `ingest` did with a message.
pub enum Ingested {
    Filed,
    Duplicate,
    NotCrm,
}

/// Parses and files one raw message. `sent` is true for messages from the sent folder.
pub async fn ingest(state: &AppState, mb: &MailboxRow, sent: bool, raw: &[u8]) -> AppResult<Ingested> {
    let Some(msg) = MessageParser::default().parse(raw) else { return Ok(Ingested::NotCrm) };
    let message_id = match msg.message_id() {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => format!("{}@nerp.generated", hex::encode(&Sha256::digest(raw)[..16])),
    };
    let dup: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_emails WHERE mailbox_id = $1 AND message_id = $2)")
        .bind(mb.id)
        .bind(&message_id)
        .fetch_one(&state.db)
        .await?;
    if dup {
        return Ok(Ingested::Duplicate);
    }
    let own = mb.email.to_lowercase();
    let from = addr_list(msg.from());
    let to = addr_list(msg.to());
    let cc = addr_list(msg.cc());
    let (from_addr, from_name) = from.first().cloned().unwrap_or_default();
    let outgoing = sent || from_addr == own;
    let others: BTreeSet<String> = from.iter().chain(&to).chain(&cc).map(|(a, _)| a.clone()).filter(|a| *a != own && !a.is_empty()).collect();
    let others: Vec<String> = others.into_iter().collect();

    // Conversation: any referenced message already in the CRM.
    let mut refs: Vec<String> = msg.references().map(|r| r.iter().map(String::from).collect()).unwrap_or_default();
    let in_reply_to = msg.in_reply_to().and_then(|r| r.first().map(String::from));
    if let Some(r) = &in_reply_to
        && !refs.contains(r) {
            refs.push(r.clone());
        }
    let thread: Option<Uuid> = if refs.is_empty() {
        None
    } else {
        sqlx::query_scalar("SELECT thread_id FROM crm_emails WHERE message_id = ANY($1) ORDER BY sent_at LIMIT 1").bind(&refs).fetch_optional(&state.db).await?
    };

    let mut links: Vec<Uuid> = match thread {
        Some(t) => sqlx::query_scalar("SELECT DISTINCT l.item_id FROM crm_email_links l JOIN crm_emails e ON e.id = l.email_id WHERE e.thread_id = $1")
            .bind(t)
            .fetch_all(&state.db)
            .await?,
        None => vec![],
    };
    // Records with one of the other parties' addresses.
    let matched: Vec<(Uuid, i16)> = sqlx::query_as(
        "SELECT DISTINCT i.id, i.entity_type FROM crm_multifields m JOIN crm_items i ON i.id = m.item_id
         WHERE m.kind = 'email' AND m.normalized = ANY($1) AND i.deleted_at IS NULL LIMIT 50",
    )
    .bind(&others)
    .fetch_all(&state.db)
    .await?;
    for (id, _) in &matched {
        if !links.contains(id) {
            links.push(*id);
        }
    }
    // A new conversation with a known contact/company also goes on their latest open deal
    // (or smart-process record linked to them).
    if thread.is_none() && !matched.is_empty() {
        let people: Vec<Uuid> = matched.iter().filter(|(_, t)| *t == crate::crm::CONTACT || *t == crate::crm::COMPANY).map(|(i, _)| *i).collect();
        if !people.is_empty() {
            let open: Option<Uuid> = sqlx::query_scalar(
                "SELECT i.id FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type
                 WHERE i.deleted_at IS NULL AND NOT i.is_closed AND t.has_client AND t.id <> 1
                   AND (i.company_id = ANY($1) OR EXISTS (SELECT 1 FROM crm_item_contacts c WHERE c.item_id = i.id AND c.contact_id = ANY($1)))
                 ORDER BY i.updated_at DESC LIMIT 1",
            )
            .bind(&people)
            .fetch_optional(&state.db)
            .await?;
            if let Some(o) = open.filter(|o| !links.contains(o)) {
                links.push(o);
            }
        }
    }
    let subject = msg.subject().unwrap_or("").trim().chars().take(500).collect::<String>();
    if links.is_empty() && !outgoing && mb.create_leads && !from_addr.is_empty() {
        links.push(create_lead(state, mb, &from_addr, &from_name, &subject).await?);
    }
    if links.is_empty() {
        return Ok(Ingested::NotCrm);
    }

    let text = msg.body_text(0).map(|t| t.chars().take(200_000).collect::<String>());
    let html = msg.html_body().next().filter(|p| p.is_content_type("text", "html")).and_then(|_| msg.body_html(0)).map(|h| sanitize_html(&h.chars().take(1_000_000).collect::<String>()));
    let sent_at: DateTime<Utc> = msg.date().and_then(|d| DateTime::from_timestamp(d.to_timestamp(), 0)).unwrap_or_else(Utc::now);
    let id = Uuid::new_v4();
    let thread_id = thread.unwrap_or(id);

    // Attachments become files owned by the mailbox owner.
    let mut atts = Vec::new();
    let mut file_ids = Vec::new();
    for part in msg.attachments().take(20) {
        let data = part.decoded();
        if data.is_empty() || data.len() > 25 * 1024 * 1024 {
            continue;
        }
        let name = part.attachment_name().unwrap_or("attachment").to_string();
        let mime = part.content_type().map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream"))).unwrap_or_else(|| "application/octet-stream".into());
        let f = files::store_bytes(state, Some(mb.owner_id), &name, &mime, &data).await?;
        atts.push(json!({ "file_id": f, "name": name, "size": data.len(), "mime": mime }));
        file_ids.push(f);
    }

    let inserted = sqlx::query(
        "INSERT INTO crm_emails (id, mailbox_id, direction, status, message_id, in_reply_to, refs, thread_id, from_addr, from_name, to_addrs, cc_addrs,
             subject, body_text, body_html, snippet, attachments, sent_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(mb.id)
    .bind(if outgoing { "out" } else { "in" })
    .bind(if outgoing { "sent" } else { "received" })
    .bind(&message_id)
    .bind(&in_reply_to)
    .bind(&refs)
    .bind(thread_id)
    .bind(&from_addr)
    .bind(&from_name)
    .bind(as_json(&to))
    .bind(as_json(&cc))
    .bind(&subject)
    .bind(&text)
    .bind(&html)
    .bind(snippet(text.as_deref().unwrap_or("")))
    .bind(Value::Array(atts))
    .bind(sent_at)
    .execute(&state.db)
    .await?
    .rows_affected();
    if inserted == 0 {
        for f in file_ids {
            files::delete_stored(state, f).await;
        }
        return Ok(Ingested::Duplicate);
    }
    for f in &file_ids {
        sqlx::query("INSERT INTO crm_email_files (file_id, email_id) VALUES ($1, $2)").bind(f).bind(id).execute(&state.db).await?;
    }
    super::file_on_records(state, id, &links, if outgoing { Some(mb.owner_id) } else { None }).await?;
    if !outgoing {
        for item in &links {
            fire(state, *item, "email_received", |_| true).await;
        }
    }
    Ok(Ingested::Filed)
}

async fn create_lead(state: &AppState, mb: &MailboxRow, addr: &str, name: &str, subject: &str) -> AppResult<Uuid> {
    let lead = entity_type(state, "lead").await?;
    let mut parts = name.split_whitespace();
    let (first, last) = (parts.next().unwrap_or("").to_string(), parts.collect::<Vec<_>>().join(" "));
    let mut body = Map::new();
    body.insert("title".into(), json!(if subject.is_empty() { format!("Email from {addr}") } else { subject.chars().take(300).collect::<String>() }));
    if !first.is_empty() {
        body.insert("first_name".into(), json!(first));
    }
    if !last.is_empty() {
        body.insert("last_name".into(), json!(last));
    }
    body.insert("source".into(), json!("email"));
    body.insert("assigned_to".into(), json!(mb.owner_id));
    body.insert("multifields".into(), json!([{ "kind": "email", "value_type": "work", "value": addr }]));
    let id = crate::crm::items::create_item(state, &system_auth(), &lead, body, None).await?;
    broadcast(state, lead.id, id, false).await;
    Ok(id)
}

fn folder_state(mb: &MailboxRow, folder: &str) -> (Option<u32>, u32) {
    let f = mb.folders.get(folder);
    (f.and_then(|f| f["validity"].as_u64()).map(|v| v as u32), f.and_then(|f| f["last_uid"].as_u64()).unwrap_or(0) as u32)
}

async fn save_folder(state: &AppState, mb: Uuid, folder: &str, validity: u32, last_uid: u32) -> AppResult<()> {
    sqlx::query("UPDATE crm_mailboxes SET folders = jsonb_set(folders, ARRAY[$2], $3::jsonb, TRUE) WHERE id = $1")
        .bind(mb)
        .bind(folder)
        .bind(json!({ "validity": validity, "last_uid": last_uid }))
        .execute(&state.db)
        .await?;
    Ok(())
}

/// The sent folder: the one marked \Sent, else a usual name.
async fn find_sent(session: &mut async_imap::Session<imap::Stream>) -> Option<String> {
    let names: Vec<(String, bool)> = session
        .list(Some(""), Some("*"))
        .await
        .ok()?
        .map_ok(|n| (n.name().to_string(), n.attributes().iter().any(|a| matches!(a, async_imap::types::NameAttribute::Sent))))
        .try_collect()
        .await
        .ok()?;
    if let Some((n, _)) = names.iter().find(|(_, s)| *s) {
        return Some(n.clone());
    }
    ["Sent", "Sent Items", "Sent Messages", "INBOX.Sent", "INBOX/Sent", "[Gmail]/Sent Mail"]
        .iter()
        .find(|c| names.iter().any(|(n, _)| n.eq_ignore_ascii_case(c)))
        .map(|c| names.iter().find(|(n, _)| n.eq_ignore_ascii_case(c)).unwrap().0.clone())
}

#[derive(Default, serde::Serialize)]
pub struct Report {
    pub fetched: usize,
    pub filed: usize,
}

/// One sync run for a mailbox.
pub async fn sync(state: &AppState, mb: &MailboxRow) -> Result<Report, String> {
    let password = mb.credential(state).await?;
    let mut session = imap::connect(&mb.imap(&password)).await?;
    let sent = match &mb.sent_folder {
        Some(s) => Some(s.clone()),
        None => {
            let s = find_sent(&mut session).await;
            if let Some(s) = &s {
                let _ = sqlx::query("UPDATE crm_mailboxes SET sent_folder = $2 WHERE id = $1").bind(mb.id).bind(s).execute(&state.db).await;
            }
            s
        }
    };
    let mut report = Report::default();
    let mut folders = vec![("INBOX".to_string(), false)];
    if let Some(s) = sent {
        folders.push((s, true));
    }
    for (folder, is_sent) in folders {
        let mailbox = match session.select(&folder).await {
            Ok(m) => m,
            Err(e) if is_sent => {
                tracing::debug!(error = %e, folder, "sent folder not selectable");
                continue;
            }
            Err(e) => return Err(format!("cannot open {folder}: {e}")),
        };
        let validity = mailbox.uid_validity.unwrap_or(0);
        let (known_validity, last_uid) = folder_state(mb, &folder);
        // First run (or the server renumbered the folder): start from `sync_days` ago.
        let mut uids: Vec<u32> = if known_validity != Some(validity) {
            let since = (Utc::now() - chrono::Duration::days(mb.sync_days as i64)).format("%d-%b-%Y").to_string();
            session.uid_search(format!("SINCE {since}")).await.map_err(|e| e.to_string())?.into_iter().collect()
        } else {
            session.uid_search(format!("UID {}:*", last_uid + 1)).await.map_err(|e| e.to_string())?.into_iter().filter(|u| *u > last_uid).collect()
        };
        uids.sort_unstable();
        if known_validity != Some(validity) && uids.is_empty() {
            // Nothing recent: remember where the folder ends so only new mail is read.
            let next = mailbox.uid_next.unwrap_or(1).saturating_sub(1);
            save_folder(state, mb.id, &folder, validity, next).await.map_err(|e| format!("{e:?}"))?;
            continue;
        }
        uids.truncate(BATCH);
        let mut high = if known_validity == Some(validity) { last_uid } else { 0 };
        for chunk in uids.chunks(25) {
            let set = chunk.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
            let fetched: Vec<(u32, Option<Vec<u8>>)> = session
                .uid_fetch(&set, "(UID RFC822.SIZE BODY.PEEK[])")
                .await
                .map_err(|e| e.to_string())?
                .map_ok(|f| (f.uid.unwrap_or(0), if f.size.unwrap_or(0) > MAX_MESSAGE_BYTES { None } else { f.body().map(<[u8]>::to_vec) }))
                .try_collect()
                .await
                .map_err(|e| e.to_string())?;
            for (uid, body) in fetched {
                report.fetched += 1;
                if let Some(body) = body {
                    match ingest(state, mb, is_sent, &body).await {
                        Ok(Ingested::Filed) => report.filed += 1,
                        Ok(_) => {}
                        Err(e) => tracing::warn!(mailbox = %mb.id, uid, error = ?e, "could not file a message"),
                    }
                }
                high = high.max(uid);
            }
            save_folder(state, mb.id, &folder, validity, high).await.map_err(|e| format!("{e:?}"))?;
        }
    }
    let _ = session.logout().await;
    Ok(report)
}
