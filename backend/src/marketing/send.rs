//! Sending a campaign: when it starts, its segments are turned into recipients (one per
//! email address, minus opt-outs); then a background job sends them in batches at a steady
//! rate. Each message gets personal tracking links (opens, clicks, unsubscribe), and links
//! carry UTM tags so leads and deals they bring in are counted for the campaign.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use hmac::{KeyInit, Mac};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use uuid::Uuid;

use super::{load_campaign, members, parse_filter, require, Campaign, CAMPAIGN_COLUMNS};
use crate::{
    auth::{tokens, AuthUser},
    crm::{automation, entity_type_by_id, timeline},
    error::{AppError, AppResult},
    jobs,
    mail::BulkEmail,
    state::AppState,
};

pub const JOB: &str = "marketing.send";
pub const DUE_JOB: &str = "marketing.due";
/// Messages per batch, and per minute.
const BATCH: i64 = 50;
const PER_MINUTE: i64 = 300;
/// Recipients a campaign may have.
const MAX_RECIPIENTS: i64 = 50_000;

// ---------------------------------------------------------------------------
// Links and rendering
// ---------------------------------------------------------------------------

/// Signs a click link so the redirect cannot be used to send people elsewhere.
pub(crate) fn link_sig(state: &AppState, token: &str, url: &str) -> String {
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&state.config.data_key).expect("any key length");
    mac.update(token.as_bytes());
    mac.update(b"\n");
    mac.update(url.as_bytes());
    hex::encode(mac.finalize().into_bytes())[..32].to_string()
}

/// http(s) links in a text, as (start, end) byte ranges.
fn find_links(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let Some(p) = [rest.find("https://"), rest.find("http://")].into_iter().flatten().min() else { break };
        let start = i + p;
        let mut end = s[start..].find(|c: char| c.is_whitespace() || "<>\"'".contains(c)).map(|e| start + e).unwrap_or(s.len());
        while end > start && ".,;:!?)".contains(s[..end].chars().last().unwrap()) {
            end -= 1;
        }
        if end > start + 8 {
            out.push((start, end));
        }
        i = end.max(start + 1);
    }
    out
}

/// Adds utm_source/medium/campaign unless the link already has its own.
fn with_utm(url: &str, slug: &str) -> String {
    let Ok(mut u) = reqwest::Url::parse(url) else { return url.to_string() };
    if u.query_pairs().any(|(k, _)| k.starts_with("utm_")) {
        return url.to_string();
    }
    u.query_pairs_mut().append_pair("utm_source", "nerp").append_pair("utm_medium", "email").append_pair("utm_campaign", slug);
    u.to_string()
}

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub(crate) struct Rendered {
    pub subject: String,
    pub text: String,
    pub html: String,
    pub unsubscribe_url: String,
}

pub(crate) async fn company_name(state: &AppState) -> String {
    let v: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'company_name'").fetch_optional(&state.db).await.ok().flatten();
    v.and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "NERP".into())
}

/// The message for one recipient: placeholders filled from the record, links tracked.
pub(crate) async fn render(state: &AppState, c: &Campaign, item: Option<Uuid>, token: &str, company: &str) -> AppResult<Rendered> {
    let ctx = match item {
        Some(id) => automation::context(state, id).await?.value,
        None => json!({}),
    };
    let subject = automation::fill(&c.subject, &ctx);
    let body = automation::fill(&c.body, &ctx);
    let base = &state.config.public_url;
    let track = |url: &str| -> String {
        let target = with_utm(url, &c.slug);
        let sig = link_sig(state, token, &target);
        reqwest::Url::parse_with_params(&format!("{base}/api/public/m/c/{token}"), &[("u", target.as_str()), ("s", sig.as_str())])
            .map(|u| u.to_string())
            .unwrap_or(target)
    };
    let unsubscribe_page = format!("{base}/unsubscribe/{token}");
    if let Some(d) = &c.design {
        let footer_html = format!("You get this email from {}. <a href=\"{}\" style=\"color:#64748b\">Unsubscribe</a>", esc(company), esc(&unsubscribe_page));
        let footer_text = format!("You get this email from {company}. Unsubscribe: {unsubscribe_page}");
        let (html, text) = super::design::render(state, d, &|t| automation::fill(t, &ctx), &|u| if u.starts_with("http") { track(u) } else { u.to_string() }, (&footer_html, &footer_text));
        let pixel = format!("<img src=\"{base}/api/public/m/o/{token}\" width=\"1\" height=\"1\" alt=\"\" style=\"display:block;border:0\"></body>");
        return Ok(Rendered { subject, text, html: html.replacen("</body>", &pixel, 1), unsubscribe_url: format!("{base}/api/public/m/u/{token}") });
    }
    let (mut text, mut html) = (String::new(), String::new());
    let mut last = 0;
    for (a, b) in find_links(&body) {
        let url = &body[a..b];
        let t = track(url);
        text.push_str(&body[last..a]);
        text.push_str(&t);
        html.push_str(&esc(&body[last..a]));
        html.push_str(&format!("<a href=\"{}\">{}</a>", esc(&t), esc(url)));
        last = b;
    }
    text.push_str(&body[last..]);
    html.push_str(&esc(&body[last..]));
    text.push_str(&format!("\n\n--\nYou get this email from {company}. Unsubscribe: {unsubscribe_page}\n"));
    let html = format!(
        "<div style=\"font-family:sans-serif;font-size:15px;line-height:1.55;color:#1e293b;max-width:640px\">{}</div>\
         <p style=\"font-family:sans-serif;font-size:12px;color:#64748b;margin-top:32px\">You get this email from {}. <a href=\"{}\" style=\"color:#64748b\">Unsubscribe</a></p>\
         <img src=\"{base}/api/public/m/o/{token}\" width=\"1\" height=\"1\" alt=\"\" style=\"display:block;border:0\">",
        html.replace('\n', "<br>"),
        esc(company),
        esc(&unsubscribe_page),
    );
    Ok(Rendered { subject, text, html, unsubscribe_url: format!("{base}/api/public/m/u/{token}") })
}

// ---------------------------------------------------------------------------
// Starting and sending
// ---------------------------------------------------------------------------

fn ready(c: &Campaign) -> AppResult<()> {
    if c.subject.trim().is_empty() || c.body.trim().is_empty() {
        return Err(AppError::bad("write a subject and a message first"));
    }
    if c.segment_ids.is_empty() {
        return Err(AppError::bad("choose who gets it: add a segment"));
    }
    Ok(())
}

/// Turns the segments into recipients and queues sending. Returns how many.
async fn start(state: &AppState, c: &Campaign) -> AppResult<i64> {
    let optouts: HashSet<String> = sqlx::query_scalar::<_, String>("SELECT email FROM mkt_optouts").fetch_all(&state.db).await?.into_iter().collect();
    let segments: Vec<(i16, Value, Option<Vec<Uuid>>)> = sqlx::query_as("SELECT entity_type, filter, item_ids FROM mkt_segments WHERE id = ANY($1)").bind(&c.segment_ids).fetch_all(&state.db).await?;
    let mut seen: HashSet<String> = HashSet::new();
    let (mut items, mut emails, mut toks) = (Vec::new(), Vec::new(), Vec::new());
    let mut considered = 0i64;
    for (et, filter, ids) in segments {
        let t = entity_type_by_id(state, et).await?;
        for (id, _, email) in members(state, &t, &parse_filter(&filter)?, ids.as_deref(), MAX_RECIPIENTS).await? {
            considered += 1;
            let Some(email) = email.map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty()) else { continue };
            if optouts.contains(&email) || !seen.insert(email.clone()) || items.len() as i64 >= MAX_RECIPIENTS {
                continue;
            }
            items.push(id);
            emails.push(email);
            toks.push(tokens::random_token(24));
        }
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("INSERT INTO mkt_recipients (campaign_id, item_id, email, token) SELECT $1, x.i, x.e, x.t FROM unnest($2::uuid[], $3::text[], $4::text[]) AS x(i, e, t)")
        .bind(c.id)
        .bind(&items)
        .bind(&emails)
        .bind(&toks)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE mkt_campaigns SET status = 'sending', started_at = now(), skipped = $2, updated_at = now() WHERE id = $1")
        .bind(c.id)
        .bind((considered - items.len() as i64) as i32)
        .execute(&mut *tx)
        .await?;
    jobs::enqueue(&mut *tx, JOB, json!({ "campaign_id": c.id }), None).await?;
    tx.commit().await?;
    state.job_wakeup.notify_one();
    Ok(items.len() as i64)
}

#[derive(Deserialize, Default)]
pub(crate) struct ScheduleBody {
    /// Later; none = now.
    send_at: Option<DateTime<Utc>>,
}

/// Sends now, or schedules for later.
pub(crate) async fn schedule(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, body: Option<Json<ScheduleBody>>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let c = load_campaign(&state, id).await?;
    if c.status != "draft" {
        return Err(AppError::bad("this campaign was already sent or scheduled"));
    }
    ready(&c)?;
    let at = body.and_then(|b| b.0.send_at);
    if let Some(at) = at.filter(|t| *t > Utc::now() + Duration::seconds(30)) {
        if at > Utc::now() + Duration::days(365) {
            return Err(AppError::bad("schedule it within a year"));
        }
        sqlx::query("UPDATE mkt_campaigns SET status = 'scheduled', send_at = $2, updated_at = now() WHERE id = $1 AND status = 'draft'").bind(id).bind(at).execute(&state.db).await?;
        crate::audit::record(&state.db, Some(auth.id), "marketing.campaign.schedule", "campaign", Some(id.to_string()), json!({ "send_at": at }), None).await;
        return Ok(Json(json!({ "status": "scheduled", "send_at": at })));
    }
    let claimed = sqlx::query("UPDATE mkt_campaigns SET status = 'sending', updated_at = now() WHERE id = $1 AND status = 'draft'").bind(id).execute(&state.db).await?.rows_affected();
    if claimed == 0 {
        return Err(AppError::bad("this campaign was already sent or scheduled"));
    }
    let n = start(&state, &c).await?;
    crate::audit::record(&state.db, Some(auth.id), "marketing.campaign.send", "campaign", Some(id.to_string()), json!({ "recipients": n }), None).await;
    Ok(Json(json!({ "status": "sending", "recipients": n })))
}

/// Stops a scheduled campaign (back to a draft) or one being sent (what went out stays).
pub(crate) async fn cancel(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let c = load_campaign(&state, id).await?;
    match c.status.as_str() {
        "scheduled" => {
            sqlx::query("UPDATE mkt_campaigns SET status = 'draft', send_at = NULL, updated_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
        }
        "sending" => {
            sqlx::query("UPDATE mkt_campaigns SET status = 'cancelled', finished_at = now(), updated_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
            sqlx::query("DELETE FROM mkt_recipients WHERE campaign_id = $1 AND status = 'pending'").bind(id).execute(&state.db).await?;
        }
        _ => return Err(AppError::bad("only scheduled campaigns or ones being sent can be cancelled")),
    }
    Ok(Json(json!({ "ok": true })))
}

/// Recurring job: starts scheduled campaigns that are due.
pub async fn run_due(state: &AppState) -> anyhow::Result<()> {
    let due: Vec<Campaign> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {CAMPAIGN_COLUMNS} FROM mkt_campaigns WHERE status = 'scheduled' AND send_at <= now() ORDER BY send_at LIMIT 10"
    )))
    .fetch_all(&state.db)
    .await?;
    for c in due {
        let claimed = sqlx::query("UPDATE mkt_campaigns SET status = 'sending' WHERE id = $1 AND status = 'scheduled'").bind(c.id).execute(&state.db).await?.rows_affected();
        if claimed == 1
            && let Err(e) = start(state, &c).await {
                tracing::warn!(campaign = %c.id, error = ?e, "could not start a scheduled campaign");
                sqlx::query("UPDATE mkt_campaigns SET status = 'draft' WHERE id = $1").bind(c.id).execute(&state.db).await?;
            }
    }
    Ok(())
}

/// Background job: sends the next batch of a campaign, then queues the following one.
pub async fn run_job(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let id: Uuid = serde_json::from_value(payload["campaign_id"].clone())?;
    let ae = |e: AppError| anyhow::anyhow!("{e:?}");
    let c = load_campaign(state, id).await.map_err(ae)?;
    if c.status != "sending" {
        return Ok(());
    }
    let batch: Vec<(Uuid, Option<Uuid>, String, String)> =
        sqlx::query_as("SELECT id, item_id, email, token FROM mkt_recipients WHERE campaign_id = $1 AND status = 'pending' ORDER BY email LIMIT $2")
            .bind(id)
            .bind(BATCH)
            .fetch_all(&state.db)
            .await?;
    let company = company_name(state).await;
    for (rid, item, email, token) in &batch {
        // An address may have opted out since sending started.
        let out: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM mkt_optouts WHERE email = $1)").bind(email).fetch_one(&state.db).await?;
        if out {
            sqlx::query("UPDATE mkt_recipients SET status = 'failed', error = 'unsubscribed before it was sent' WHERE id = $1").bind(rid).execute(&state.db).await?;
            continue;
        }
        let result = match render(state, &c, *item, token, &company).await {
            Ok(m) => state
                .mailer
                .send_bulk(&BulkEmail {
                    to: email,
                    from_name: c.from_name.as_deref(),
                    reply_to: c.reply_to.as_deref(),
                    subject: &m.subject,
                    text: m.text,
                    html: m.html,
                    unsubscribe_url: &m.unsubscribe_url,
                })
                .await
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        match result {
            Ok(()) => {
                sqlx::query("UPDATE mkt_recipients SET status = 'sent', sent_at = now(), error = NULL WHERE id = $1").bind(rid).execute(&state.db).await?;
                if let Some(item) = item {
                    let _ = timeline(&state.db, *item, "campaign", None, None, json!({ "campaign_id": c.id, "name": c.name, "event": "sent" })).await;
                }
            }
            Err(e) => {
                sqlx::query("UPDATE mkt_recipients SET status = 'failed', error = $2 WHERE id = $1").bind(rid).bind(e.chars().take(500).collect::<String>()).execute(&state.db).await?;
            }
        }
    }
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mkt_recipients WHERE campaign_id = $1 AND status = 'pending'").bind(id).fetch_one(&state.db).await?;
    if left > 0 {
        let pause = Duration::milliseconds((batch.len() as i64 * 60_000) / PER_MINUTE);
        jobs::enqueue(&state.db, JOB, json!({ "campaign_id": id }), Some(Utc::now() + pause)).await?;
    } else {
        sqlx::query("UPDATE mkt_campaigns SET status = 'sent', finished_at = now(), updated_at = now() WHERE id = $1 AND status = 'sending'").bind(id).execute(&state.db).await?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct TestBody {
    email: String,
    /// The record to fill placeholders from; default: the first in the campaign's segments.
    item_id: Option<Uuid>,
}

/// Sends the campaign to one address, marked as a test (links work; nothing is counted).
pub(crate) async fn test_send(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<TestBody>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let c = load_campaign(&state, id).await?;
    if c.subject.trim().is_empty() || c.body.trim().is_empty() {
        return Err(AppError::bad("write a subject and a message first"));
    }
    let to = b.email.trim().to_string();
    if to.parse::<lettre::Address>().is_err() {
        return Err(AppError::bad("enter a valid email address"));
    }
    let item = match b.item_id {
        Some(i) => Some(i),
        None => {
            let first: Option<(i16, Value, Option<Vec<Uuid>>)> = sqlx::query_as("SELECT entity_type, filter, item_ids FROM mkt_segments WHERE id = ANY($1) LIMIT 1").bind(&c.segment_ids).fetch_optional(&state.db).await?;
            match first {
                Some((et, f, ids)) => members(&state, &entity_type_by_id(&state, et).await?, &parse_filter(&f)?, ids.as_deref(), 1).await?.first().map(|m| m.0),
                None => None,
            }
        }
    };
    let m = render(&state, &c, item, "test", &company_name(&state).await).await?;
    state
        .mailer
        .send_bulk(&BulkEmail {
            to: &to,
            from_name: c.from_name.as_deref(),
            reply_to: c.reply_to.as_deref(),
            subject: &format!("[Test] {}", m.subject),
            text: m.text,
            html: m.html,
            unsubscribe_url: &m.unsubscribe_url,
        })
        .await
        .map_err(|e| AppError::bad(format!("could not send: {e}")))?;
    Ok(Json(json!({ "ok": true, "filled_from": item })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links() {
        let s = "Visit https://example.com/a?b=1. Or (http://x.io/path), not ftp://y.";
        let found: Vec<&str> = find_links(s).into_iter().map(|(a, b)| &s[a..b]).collect();
        assert_eq!(found, vec!["https://example.com/a?b=1", "http://x.io/path"]);
        assert!(find_links("no links here").is_empty());
    }

    #[test]
    fn utm() {
        assert_eq!(with_utm("https://ex.com/p?a=1", "spring"), "https://ex.com/p?a=1&utm_source=nerp&utm_medium=email&utm_campaign=spring");
        assert_eq!(with_utm("https://ex.com/?utm_source=x", "spring"), "https://ex.com/?utm_source=x");
    }
}
