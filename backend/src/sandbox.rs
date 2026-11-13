//! Sandbox: an anonymised copy of the company to try automation, imports and settings on.
//!
//! **On the live system** (System → Sandbox, system managers), *Refresh* copies the database
//! into the one in `SANDBOX_DATABASE_URL` (pg_dump / pg_restore; the old copy is dropped) and
//! anonymises it: clients' names, contact details, addresses, notes, custom fields, emails,
//! chats, tickets, form data, bookings, orders and campaign recipients are replaced or removed;
//! credentials and connections (API keys, webhooks, mailboxes, calendar accounts, directory and
//! sign-in secrets) are removed or switched off; stored files' content is out of reach;
//! employees' hourly costs go. Amounts, dates, pipelines, stages, products, automation and
//! employees' sign-ins stay, so tests behave like the real thing.
//!
//! **The sandbox itself** is a second NERP running on that database with `SANDBOX=true`: it
//! sends no email (its outbox is listed instead), calls nothing outside, polls no mailbox or
//! calendar, can't run backups, uses its own real-time channel and shows a banner.
use std::process::Stdio;

use axum::{extract::State, routing::{get, post}, Json, Router};
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use serde_json::{json, Value};
use sqlx::Connection;

use crate::{
    audit,
    auth::{permissions as perm, AuthUser, ClientIp},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/public/instance", get(instance))
        .route("/admin/sandbox", get(status))
        .route("/admin/sandbox/refresh", post(refresh))
        .route("/admin/sandbox/outbox", get(outbox))
}

fn target() -> Option<String> {
    std::env::var("SANDBOX_DATABASE_URL").ok().filter(|s| !s.trim().is_empty())
}

fn link() -> Option<String> {
    std::env::var("SANDBOX_URL").ok().filter(|s| !s.trim().is_empty())
}

/// Whether this is a sandbox, for the banner (anyone may ask).
async fn instance(State(state): State<AppState>) -> AppResult<Json<Value>> {
    if !state.config.sandbox {
        return Ok(Json(json!({ "sandbox": false })));
    }
    let of: Option<Value> = sqlx::query_scalar!("SELECT value FROM settings WHERE key = 'sandbox_of'").fetch_optional(&state.db).await?;
    Ok(Json(json!({ "sandbox": true, "copy": of })))
}

async fn status(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    let last: Option<Value> = sqlx::query_scalar!("SELECT value FROM settings WHERE key = 'sandbox_last'").fetch_optional(&state.db).await?;
    let mut redis = state.redis.clone();
    let running: Option<String> = redis.get("sandbox:refreshing").await?;
    Ok(Json(json!({ "is_sandbox": state.config.sandbox, "configured": target().is_some(), "url": link(), "running": running.is_some(), "last": last })))
}

/// The sandbox's emails, kept instead of sent.
async fn outbox(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    if !state.config.sandbox {
        return Err(AppError::bad("only a sandbox keeps its emails"));
    }
    let rows: Vec<(Value, DateTime<Utc>)> = sqlx::query!(r#"SELECT payload, created_at FROM jobs WHERE kind = $1 ORDER BY created_at DESC LIMIT 100"#, crate::jobs::EMAIL_SEND).map(|r| (r.payload, r.created_at))
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(p, at)| json!({ "to": p["to"], "subject": p["subject"], "text": p["text"], "at": at })).collect::<Vec<_>>() })))
}

async fn refresh(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    if state.config.sandbox {
        return Err(AppError::bad("this is the sandbox; refresh it from the live system"));
    }
    let to = target().ok_or_else(|| AppError::bad("set SANDBOX_DATABASE_URL to the database the sandbox uses"))?;
    if db_name(&to) == db_name(&state.config.database_url) && host_of(&to) == host_of(&state.config.database_url) {
        return Err(AppError::bad("SANDBOX_DATABASE_URL must name another database than the live one"));
    }
    let mut redis = state.redis.clone();
    let free: bool = redis::cmd("SET").arg("sandbox:refreshing").arg("1").arg("NX").arg("EX").arg(6 * 3600).query_async::<Option<String>>(&mut redis).await?.is_some();
    if !free {
        return Err(AppError::Conflict("the sandbox is being refreshed".into()));
    }
    audit::record(&state.db, Some(auth.id), "system.sandbox_refresh", "system", None, json!({}), ip.as_deref()).await;
    let st = state.clone();
    tokio::spawn(async move {
        let started = Utc::now();
        let r = run(&st, &to).await;
        let (ok, text) = match r {
            Ok(t) => (true, t),
            Err(e) => (false, format!("{e:#}")),
        };
        let v = json!({ "started_at": started, "finished_at": Utc::now(), "ok": ok, "output": text });
        let _ = sqlx::query!("INSERT INTO settings (key, value) VALUES ('sandbox_last', $1) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()", v)
            .execute(&st.db)
            .await;
        let mut redis = st.redis.clone();
        let _: Result<(), _> = redis.del("sandbox:refreshing").await;
    });
    Ok(Json(json!({ "started": true })))
}

fn db_name(url: &str) -> String {
    url.split('?').next().unwrap_or(url).rsplit('/').next().unwrap_or("").to_string()
}

fn host_of(url: &str) -> String {
    url.split("://").nth(1).and_then(|r| r.split('/').next()).map(|h| h.rsplit('@').next().unwrap_or(h).to_string()).unwrap_or_default()
}

/// The same server, the maintenance database.
fn maintenance_url(url: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, format!("?{q}")),
        None => (url, String::new()),
    };
    match base.rsplit_once('/') {
        Some((head, _)) => format!("{head}/postgres{query}"),
        None => url.to_string(),
    }
}

fn tool(name: &str) -> String {
    match std::env::var("PG_BIN").ok().filter(|s| !s.is_empty()) {
        Some(dir) => format!("{dir}/{name}"),
        None => name.to_string(),
    }
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

async fn run(state: &AppState, to: &str) -> anyhow::Result<String> {
    let mut log = vec![];
    let dump = std::env::temp_dir().join(format!("nerp-sandbox-{}.dump", uuid::Uuid::new_v4()));
    let out = tokio::process::Command::new(tool("pg_dump"))
        .args(["--format=custom", "--no-owner", "--no-privileges", "--file"])
        .arg(&dump)
        .arg(&state.config.database_url)
        .stdin(Stdio::null())
        .output()
        .await?;
    anyhow::ensure!(out.status.success(), "pg_dump failed: {}", String::from_utf8_lossy(&out.stderr));
    log.push(format!("copied the live database ({} MB)", std::fs::metadata(&dump).map(|m| m.len() / 1_048_576).unwrap_or(0)));
    // A fresh database for the copy.
    let name = db_name(to);
    anyhow::ensure!(!name.is_empty() && name.len() < 64, "SANDBOX_DATABASE_URL has no database name");
    let mut admin = sqlx::PgConnection::connect(&maintenance_url(to)).await?;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", quote_ident(&name)))).execute(&mut admin).await?;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE DATABASE {}", quote_ident(&name)))).execute(&mut admin).await?;
    admin.close().await?;
    let out = tokio::process::Command::new(tool("pg_restore"))
        .args(["--no-owner", "--no-privileges", "--exit-on-error", "--dbname", to])
        .arg(&dump)
        .stdin(Stdio::null())
        .output()
        .await?;
    let _ = tokio::fs::remove_file(&dump).await;
    if !out.status.success() {
        drop_copy(to, &name).await;
        anyhow::bail!("pg_restore failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    log.push("restored it into the sandbox database".into());
    // All of it or nothing: a copy that isn't fully anonymised doesn't stay.
    let company = crate::routes::auth::company_name(state).await;
    let anonymised: anyhow::Result<()> = async {
        let mut db = sqlx::PgConnection::connect(to).await?;
        let mut tx = db.begin().await?;
        for (what, sql) in ANONYMISE {
            let n = sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string())).execute(&mut *tx).await.map_err(|e| anyhow::anyhow!("{what}: {e}"))?.rows_affected();
            log.push(format!("{what}: {n}"));
        }
        sqlx::query!("INSERT INTO settings (key, value) VALUES ('sandbox_of', $1) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value", json!({ "company": company, "copied_at": Utc::now() }))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        db.close().await?;
        Ok(())
    }
    .await;
    if let Err(e) = anonymised {
        drop_copy(to, &name).await;
        return Err(e.context("anonymising failed, so the copy was removed"));
    }
    Ok(log.join("\n"))
}

/// Removes the sandbox database (after a failed refresh).
async fn drop_copy(to: &str, name: &str) {
    if let Ok(mut c) = sqlx::PgConnection::connect(&maintenance_url(to)).await {
        let _ = sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", quote_ident(name)))).execute(&mut c).await;
        let _ = c.close().await;
    }
}

/// What anonymising does, in order (each a statement on the sandbox database).
const ANONYMISE: &[(&str, &str)] = &[
    ("contacts renamed", "UPDATE crm_items SET honorific = NULL, first_name = 'Contact', middle_name = NULL, last_name = number::text, title = 'Contact ' || number,
        job_title = NULL, birthdate = NULL, comments = NULL, uf = '{}'::jsonb, source_details = NULL, utm = NULL, search_text = 'contact ' || number WHERE entity_type = 3"),
    ("companies renamed", "UPDATE crm_items SET company_name = 'Company ' || number, title = 'Company ' || number, comments = NULL, uf = '{}'::jsonb,
        source_details = NULL, utm = NULL, search_text = 'company ' || number WHERE entity_type = 4"),
    ("leads renamed", "UPDATE crm_items SET honorific = NULL, first_name = CASE WHEN first_name IS NOT NULL THEN 'Lead' END, middle_name = NULL,
        last_name = CASE WHEN last_name IS NOT NULL THEN number::text END, company_name = CASE WHEN company_name IS NOT NULL THEN 'Company of lead ' || number END,
        title = 'Lead ' || number, job_title = NULL, birthdate = NULL, comments = NULL, uf = '{}'::jsonb, source_details = NULL, utm = NULL, search_text = 'lead ' || number
        WHERE entity_type = 1"),
    ("other records retitled", "UPDATE crm_items i SET title = t.name || ' ' || i.number, comments = NULL, search_text = lower(t.name) || ' ' || i.number
        FROM crm_entity_types t WHERE t.id = i.entity_type AND i.entity_type NOT IN (1, 3, 4)"),
    ("addresses reduced to city and country", "UPDATE crm_items SET address = jsonb_strip_nulls(jsonb_build_object('city', address->>'city', 'country', address->>'country'))
        WHERE address IS NOT NULL AND address <> 'null'::jsonb"),
    ("contact details replaced", "UPDATE crm_multifields m SET value = CASE m.kind WHEN 'email' THEN 'client' || i.number || '-' || m.sort || '@example.invalid'
        WHEN 'phone' THEN '+1555' || lpad((i.number % 10000000)::text, 7, '0') ELSE 'https://example.invalid/' || i.number END,
        normalized = CASE m.kind WHEN 'email' THEN 'client' || i.number || '-' || m.sort || '@example.invalid' WHEN 'phone' THEN '555' || lpad((i.number % 10000000)::text, 7, '0')
        ELSE 'example.invalid/' || i.number END FROM crm_items i WHERE i.id = m.item_id"),
    ("requisites removed", "DELETE FROM crm_requisites"),
    ("notes blanked", "UPDATE crm_timeline SET body = 'Comment', file_ids = '{}' WHERE kind = 'comment'"),
    ("activities blanked", "UPDATE crm_activities SET subject = initcap(kind) || ' ' || substr(id::text, 1, 6), description = NULL, result = NULL"),
    ("emails removed", "DELETE FROM crm_emails"),
    ("live chats removed", "DELETE FROM chat_sessions"),
    ("tickets anonymised", "UPDATE support_tickets SET title = 'Ticket ' || number, requester_name = 'Client', requester_email = 'ticket' || number || '@example.invalid', access_hash = NULL, access_secret = NULL"),
    ("ticket messages blanked", "UPDATE support_messages SET body = 'Message', author_name = NULL, author_email = NULL, file_ids = '{}'"),
    ("bookings anonymised", "UPDATE bookings SET client_name = 'Client ' || number, client_email = 'booking' || number || '@example.invalid', client_phone = NULL, note = NULL, token_hash = NULL"),
    ("orders anonymised", "UPDATE shop_orders SET name = 'Client', email = 'order@example.invalid', phone = NULL, address = '', note = NULL"),
    ("campaign recipients anonymised", "UPDATE mkt_recipients SET email = 'recipient-' || substr(id::text, 1, 8) || '@example.invalid'"),
    ("opt-outs removed", "DELETE FROM mkt_optouts"),
    ("text messages removed", "DELETE FROM sms_messages"),
    ("text opt-outs removed", "DELETE FROM sms_optouts"),
    ("campaign recipients' phones removed", "UPDATE mkt_recipients SET phone = NULL"),
    ("web form data removed", "UPDATE crm_webform_submissions SET data = '{}'::jsonb, page_url = NULL, referrer = NULL"),
    ("signers anonymised", "UPDATE sign_signers SET name = 'Signer', email = 'signer@example.invalid', phone = NULL, signed_name = NULL, ip = NULL, user_agent = NULL, token_hash = 'sandbox-' || id"),
    ("calendar guests anonymised", "UPDATE cal_email_guests SET name = 'Guest', email = 'guest-' || substr(id::text, 1, 8) || '@example.invalid'"),
    ("site visitors unlinked", "DELETE FROM track_visits"),
    ("personal data requests removed", "DELETE FROM privacy_requests"),
    ("portal access removed", "DELETE FROM portal_access"),
    ("hourly costs removed", "DELETE FROM user_costs"),
    ("API keys removed", "DELETE FROM api_keys"),
    ("app passwords removed", "DELETE FROM app_passwords"),
    ("webhook subscriptions removed", "DELETE FROM webhook_subscriptions"),
    ("mailboxes disconnected", "UPDATE crm_mailboxes SET secret = ''::bytea, access_token = NULL, sync_enabled = FALSE, last_error = 'Disconnected in the sandbox'"),
    ("calendar accounts removed", "DELETE FROM cal_ext_accounts"),
    ("directory secrets removed", "UPDATE directory_config SET ldap_secret = NULL, sso_secret = NULL, saml_key = NULL"),
    ("sign-in providers' secrets removed", "UPDATE sign_in_providers SET secret = NULL"),
    ("BI connections' passwords removed", "UPDATE bi_sources SET password = NULL"),
    ("sign-in sessions ended", "DELETE FROM auth_sessions"),
    ("files out of reach", "UPDATE files SET storage_path = '.sandbox/' || id, enc_key = NULL"),
    ("job queue emptied", "DELETE FROM jobs"),
];
