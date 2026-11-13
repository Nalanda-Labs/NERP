//! Email for the help desk.
//!
//! **Incoming:** the help-desk inbox is a CRM mailbox chosen in the settings. Its mail does
//! not go to the CRM: a message that answers one of ours (by its headers) or names a ticket
//! in the subject (`[#123]`, from that ticket's requester) is added to that ticket; any other
//! message opens a new one. Automatic messages (out of office, bounces) are ignored.
//!
//! **Outgoing:** customers get the acknowledgement, support's replies and the closing note.
//! With an inbox they go out through it, threaded, so customers simply reply; without one,
//! a notice from the system address carries the link to the request page.

use anyhow::Result;
use lettre::{
    message::{Mailbox, MultiPart},
    AsyncTransport, Message,
};
use mail_parser::MessageParser;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{is_worker, tickets};
use crate::{
    crm::email::MailboxRow,
    error::AppResult,
    jobs,
    routes::files,
    state::AppState,
};

pub const SEND_JOB: &str = "support.mail.send";

/// What a customer is told.
pub enum Notice {
    /// We got the request (with its number and link).
    Received,
    /// Support's reply (the message id).
    Reply(Uuid),
    /// The ticket was closed (`auto`: because the customer did not answer).
    Closed { auto: bool },
}

pub async fn is_inbox(state: &AppState, mailbox: Uuid) -> AppResult<bool> {
    Ok(sqlx::query_scalar!(r#"SELECT mailbox_id = $1 AS "v?" FROM support_settings WHERE id"#, mailbox).fetch_one(&state.db).await?.unwrap_or(false))
}

async fn inbox(state: &AppState) -> AppResult<Option<Uuid>> {
    Ok(sqlx::query_scalar!(r#"SELECT mailbox_id AS "mailbox_id?" FROM support_settings WHERE id"#).fetch_one(&state.db).await?)
}

/// The customer's link to their request.
async fn link(state: &AppState, ticket: Uuid) -> AppResult<Option<String>> {
    let secret: Option<Vec<u8>> = sqlx::query_scalar!("SELECT access_secret FROM support_tickets WHERE id = $1", ticket).fetch_optional(&state.db).await?.flatten();
    Ok(secret.and_then(|s| crate::crypto::open(&state.config.data_key, &s)).map(|t| format!("{}/help/t/{t}", state.config.public_url)))
}

pub async fn tell_customer(state: &AppState, ticket: Uuid, notice: Notice) -> AppResult<()> {
    let t = tickets::load(state, ticket).await?;
    if t.requester_id.is_some() || t.requester_email.is_none() || t.is_spam {
        return Ok(());
    }
    let (kind, message) = match notice {
        Notice::Received => ("received", None),
        Notice::Reply(m) => ("reply", Some(m)),
        Notice::Closed { auto } => (if auto { "closed_auto" } else { "closed" }, None),
    };
    if let Some(m) = message {
        // The Message-ID is fixed now, so the customer's answer finds this ticket.
        let domain = match inbox(state).await? {
            Some(mb) => sqlx::query_scalar!("SELECT email FROM crm_mailboxes WHERE id = $1", mb).fetch_optional(&state.db).await?,
            None => None,
        }
        .and_then(|e| e.rsplit('@').next().map(String::from))
        .unwrap_or_else(|| "nerp.local".into());
        sqlx::query!("UPDATE support_messages SET email_status = 'queued', email_message_id = COALESCE(email_message_id, $2) WHERE id = $1", m, format!("{}@{domain}", m.simple()))
            .execute(&state.db)
            .await?;
    }
    jobs::enqueue(&state.db, SEND_JOB, json!({ "ticket_id": ticket, "kind": kind, "message_id": message }), None).await?;
    state.job_wakeup.notify_one();
    Ok(())
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Background job: write and send one email to a customer.
pub async fn run_send(state: &AppState, payload: &Value) -> Result<()> {
    let ticket: Uuid = serde_json::from_value(payload["ticket_id"].clone())?;
    let kind = payload["kind"].as_str().unwrap_or("");
    let message: Option<Uuid> = serde_json::from_value(payload["message_id"].clone()).unwrap_or(None);
    let Ok(t) = tickets::load(state, ticket).await else { return Ok(()) };
    let Some(to) = t.requester_email.clone() else { return Ok(()) };
    let company = crate::routes::auth::company_name(state).await;
    let link = link(state, ticket).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let allow_reopen: bool = sqlx::query_scalar!("SELECT allow_reopen FROM support_settings WHERE id").fetch_one(&state.db).await?;
    let name = t.requester_name.clone().filter(|n| !n.contains('@')).unwrap_or_else(|| "there".into());
    let tag = format!("[#{}]", t.number);
    let (subject, paragraphs, own_id): (String, Vec<String>, Option<String>) = match kind {
        "received" => (
            format!("{tag} {}", t.title),
            vec![
                format!("Hello {name},"),
                format!("Thank you for writing to {company}. Your request number is #{}; we will answer as soon as we can.", t.number),
                "To add anything, reply to this email or open your request.".into(),
            ],
            None,
        ),
        "reply" => {
            let Some(m) = message else { return Ok(()) };
            let row: Option<(String, Option<String>, Option<String>)> = sqlx::query!(r#"SELECT m.body AS "body!", m.email_message_id AS "email_message_id?", u.first_name || ' ' || u.last_name AS "c2?" FROM support_messages m LEFT JOIN users u ON u.id = m.author_id WHERE m.id = $1"#, m).map(|r| (r.body, r.email_message_id, r.c2))
            .fetch_optional(&state.db)
            .await?;
            let Some((body, mid, author)) = row else { return Ok(()) };
            let mut p: Vec<String> = body.split("\n\n").map(String::from).collect();
            if let Some(a) = author {
                p.push(format!("{a}, {company}"));
            }
            (format!("Re: {tag} {}", t.title), p, mid)
        }
        "closed" | "closed_auto" => (
            format!("{tag} {}: closed", t.title),
            vec![
                format!("Hello {name},"),
                if kind == "closed_auto" {
                    format!("We closed your request #{} because we did not hear back from you.", t.number)
                } else {
                    format!("Your request #{} is closed.", t.number)
                },
                if allow_reopen {
                    "If it is not solved, just reply to this email. You can also rate our answer on the request page.".into()
                } else {
                    "You can rate our answer on the request page. For anything new, please send a new request.".into()
                },
            ],
            None,
        ),
        _ => return Ok(()),
    };
    let button = link.as_deref().map(|l| ("Open your request", l));

    let mailbox = match inbox(state).await? {
        Some(id) => crate::crm::email::load_mailbox(state, id).await.ok(),
        None => None,
    };
    let Some(mb) = mailbox else {
        // No inbox: a notice from the system address (customers answer on the page).
        let footer = format!("You get this email because you asked {company} for help.");
        jobs::send_email(state, crate::mail::notice(&to, &company, &subject, &subject, &paragraphs, button, &footer)).await?;
        if let Some(m) = message {
            sqlx::query!("UPDATE support_messages SET email_status = 'sent' WHERE id = $1", m).execute(&state.db).await?;
        }
        return Ok(());
    };
    let result = send_through(state, &mb, &t, &to, &subject, &paragraphs, link.as_deref(), own_id).await;
    if let Some(m) = message {
        sqlx::query!("UPDATE support_messages SET email_status = $2 WHERE id = $1", m, if result.is_ok() { "sent" } else { "failed" }).execute(&state.db).await?;
    }
    if let Err(e) = result {
        tracing::warn!(ticket = %ticket, error = %e, "help-desk email not sent");
        tickets::log(state, ticket, None, format!("The email to {to} was not sent: {e}")).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
        if let (Some(_), Some(r)) = (message, t.responsible_id) {
            crate::notify::notify(state, &[r], crate::notify::NewNotification {
                kind: "support.email_failed",
                title: format!("Your reply on #{} did not reach the customer", t.number),
                body: Some(e),
                link: Some(format!("/support/tickets/{ticket}")),
                actor: None,
            })
            .await;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn send_through(state: &AppState, mb: &MailboxRow, t: &tickets::TicketRow, to: &str, subject: &str, paragraphs: &[String], link: Option<&str>, own_id: Option<String>) -> Result<(), String> {
    mb.guard(state).await?;
    let password = mb.credential(state).await?;
    // Threading: answer the customer's latest email, and list the conversation so far.
    let ids: Vec<(String, bool)> = sqlx::query!(r#"SELECT email_message_id AS "email_message_id!", by_support AS "by_support!" FROM support_messages WHERE ticket_id = $1 AND email_message_id IS NOT NULL ORDER BY created_at DESC LIMIT 20"#, t.id).map(|r| (r.email_message_id, r.by_support))
    .fetch_all(&state.db)
    .await
    .map_err(|e| e.to_string())?;
    let mut text = paragraphs.join("\n\n");
    if let Some(l) = link {
        text.push_str(&format!("\n\n--\nReply to this email, or open your request: {l}"));
    }
    let html = format!(
        "<div style=\"font-family:Arial,sans-serif;font-size:14px;line-height:1.5\">{}{}</div>",
        paragraphs.iter().map(|p| format!("<p>{}</p>", esc(p).replace('\n', "<br>"))).collect::<String>(),
        link.map(|l| format!("<p style=\"color:#666;font-size:12px\">Reply to this email, or <a href=\"{}\">open your request</a>.</p>", esc(l))).unwrap_or_default()
    );
    let domain = mb.email.rsplit('@').next().unwrap_or("nerp.local").to_string();
    let message_id = own_id.unwrap_or_else(|| format!("{}@{domain}", Uuid::new_v4().simple()));
    let from_name = if mb.display_name.is_empty() { crate::routes::auth::company_name(state).await } else { mb.display_name.clone() };
    let mut b = Message::builder()
        .from(Mailbox::new(Some(from_name), mb.email.parse().map_err(|_| "the inbox address is not valid".to_string())?))
        .to(Mailbox::new(t.requester_name.clone().filter(|n| !n.contains('@')), to.parse().map_err(|_| format!("{to} is not a valid address"))?))
        .subject(subject)
        .message_id(Some(format!("<{message_id}>")));
    if let Some((last, _)) = ids.iter().find(|(_, by_support)| !by_support) {
        b = b.in_reply_to(format!("<{last}>"));
    }
    if !ids.is_empty() {
        b = b.references(ids.iter().rev().map(|(i, _)| format!("<{i}>")).collect::<Vec<_>>().join(" "));
    }
    let message = b.multipart(MultiPart::alternative_plain_html(text, html)).map_err(|e| e.to_string())?;
    mb.smtp(&password)?.send(message).await.map_err(|e| format!("SMTP: {e}"))?;
    Ok(())
}

/// Removes the quoted earlier conversation from an emailed answer.
fn strip_quoted(text: &str) -> String {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l == "-----Original Message-----" || (l.starts_with("On ") && l.ends_with("wrote:")) || l.starts_with("--\nReply to this email") {
            break;
        }
        if l.starts_with('>') {
            continue;
        }
        out.push(line);
    }
    let s = out.join("\n").trim().to_string();
    if s.is_empty() { text.trim().to_string() } else { s }
}

/// The ticket number in a subject like "Re: [#123] Printer".
fn subject_number(subject: &str) -> Option<i64> {
    let start = subject.find("[#")? + 2;
    let rest = &subject[start..];
    let end = rest.find(']')?;
    rest[..end].trim().parse().ok()
}

fn clean_subject(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_lowercase();
        let cut = ["re:", "fw:", "fwd:", "aw:"].iter().find(|p| lower.starts_with(*p)).map(|p| p.len());
        match cut {
            Some(n) => s = s[n..].trim_start(),
            None => break,
        }
    }
    s.to_string()
}

/// Files one incoming message of the help-desk inbox. Returns whether it was filed.
pub async fn ingest(state: &AppState, mb: &MailboxRow, sent: bool, raw: &[u8]) -> AppResult<bool> {
    if sent {
        return Ok(false);
    }
    let Some(msg) = MessageParser::default().parse(raw) else { return Ok(false) };
    let message_id = match msg.message_id() {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => format!("{}@nerp.generated", hex::encode(&Sha256::digest(raw)[..16])),
    };
    let dup: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM support_messages WHERE email_message_id = $1) AS "v!""#, &message_id).fetch_one(&state.db).await?;
    if dup {
        return Ok(false);
    }
    // Out-of-office replies, bounces and lists would answer each other forever.
    let header = |name: &str| msg.headers().value(name).and_then(|v| v.as_text().map(|t| t.trim().to_lowercase()));
    let auto = header("Auto-Submitted").is_some_and(|v| v != "no") || header("Precedence").is_some_and(|v| ["bulk", "junk", "list"].contains(&v.as_str()));
    let (from, from_name) = msg
        .from()
        .and_then(|l| l.first())
        .and_then(|m| m.address().map(|a| (a.trim().to_lowercase(), m.name().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()))))
        .unwrap_or_default();
    let local = from.split('@').next().unwrap_or("");
    if auto || from.is_empty() || from == mb.email.to_lowercase() || ["mailer-daemon", "postmaster", "noreply", "no-reply"].contains(&local) {
        return Ok(false);
    }
    let subject: String = msg.subject().unwrap_or("").trim().chars().take(300).collect();
    let text = msg
        .body_text(0)
        .map(|t| t.to_string())
        .or_else(|| msg.body_html(0).map(|h| ammonia::Builder::empty().clean(&h).to_string()))
        .unwrap_or_default();
    let body: String = strip_quoted(&text).chars().take(100_000).collect();
    let body = if body.trim().is_empty() { "(no text)".to_string() } else { body };

    let mut file_ids = Vec::new();
    for part in msg.attachments().take(20) {
        let data = part.decoded();
        if data.is_empty() || data.len() > 25 * 1024 * 1024 {
            continue;
        }
        let name = part.attachment_name().unwrap_or("attachment").to_string();
        let mime = part.content_type().map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream"))).unwrap_or_else(|| "application/octet-stream".into());
        file_ids.push(files::store_bytes(state, None, &name, &mime, &data).await?);
    }

    // An employee writing in is asked for by their account.
    let user: Option<Uuid> = sqlx::query_scalar!("SELECT id FROM users WHERE lower(email) = $1 AND is_active AND NOT is_system", &from).fetch_optional(&state.db).await?;

    // Which ticket: one of our messages it answers, else the number in the subject.
    let mut refs: Vec<String> = msg.references().map(|r| r.iter().map(String::from).collect()).unwrap_or_default();
    if let Some(r) = msg.in_reply_to().and_then(|r| r.first().map(String::from)) {
        refs.push(r);
    }
    let mut ticket: Option<Uuid> = if refs.is_empty() {
        None
    } else {
        sqlx::query_scalar!("SELECT ticket_id FROM support_messages WHERE email_message_id = ANY($1) ORDER BY created_at DESC LIMIT 1", &refs).fetch_optional(&state.db).await?
    };
    if ticket.is_none()
        && let Some(n) = subject_number(&subject) {
            ticket = sqlx::query_scalar!(r#"SELECT t.id AS "id!" FROM support_tickets t LEFT JOIN users u ON u.id = t.requester_id
                 WHERE t.number = $1 AND (t.requester_email = $2 OR lower(u.email) = $2)"#, n, &from)
            .fetch_optional(&state.db)
            .await?;
        }

    match ticket {
        Some(id) => {
            let t = tickets::load(state, id).await?;
            if t.is_spam {
                return Ok(false);
            }
            // Support may answer by email too.
            let by_support = match user {
                Some(u) => t.requester_id != Some(u) && is_worker(state, u).await?,
                None => false,
            };
            let author = match user {
                Some(u) => tickets::Author::User(u),
                None => tickets::Author::Customer { email: from.clone(), name: from_name.clone() },
            };
            let r = tickets::add_message(state, &t, author, by_support, tickets::NewMessage { body, note: false, file_ids: file_ids.clone(), minutes_spent: 0, email_message_id: Some(message_id) }).await;
            if let Err(e) = r {
                for f in file_ids {
                    files::delete_stored(state, f).await;
                }
                // A closed ticket that may not be reopened: the message is not filed.
                tracing::info!(ticket = %id, error = ?e, "help-desk email not added to its ticket");
                return Ok(false);
            }
        }
        None => {
            let title = clean_subject(&subject);
            tickets::create(state, tickets::NewTicket {
                title: if title.is_empty() { "(no subject)".into() } else { title },
                body,
                channel: "email",
                requester_id: user,
                requester_email: Some(from.clone()),
                requester_name: from_name,
                category_id: None,
                criticality_id: None,
                file_ids,
                created_by: None,
                email_message_id: Some(message_id),
            })
            .await?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects() {
        assert_eq!(subject_number("Re: [#123] Printer"), Some(123));
        assert_eq!(subject_number("[# 7 ] x"), Some(7));
        assert_eq!(subject_number("Printer"), None);
        assert_eq!(clean_subject("Re: Fwd: RE: Printer"), "Printer");
    }

    #[test]
    fn quotes_are_removed() {
        let t = "Thanks, it works now.\n\nOn Mon, 9 Mar 2026, Help <help@x.test> wrote:\n> Try again\n> please";
        assert_eq!(strip_quoted(t), "Thanks, it works now.");
        assert_eq!(strip_quoted("> only a quote"), "> only a quote");
    }
}
