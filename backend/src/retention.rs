//! Retention rules: how long each kind of data is kept.
//!
//! Each rule (off until turned on) names a kind of data, how many days it is kept and what
//! happens after: deleted, or anonymised where the record itself must stay (orders, bookings,
//! contacts that deals and invoices point at). The `retention.run` job applies the rules
//! every day, in batches, and logs what each run did; the page shows how much each rule would
//! affect now, before it is turned on.
//!
//! Kept regardless: the audit trail (tamper-evident, never removed), the books (the law sets
//! how long), and signed documents.
use axum::{
    extract::{Path, State},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    audit,
    auth::{permissions as perm, AuthUser, ClientIp},
    error::{AppError, AppResult},
    state::AppState,
};

pub const JOB: &str = "retention.run";
/// Rows handled per rule per run, so a first run on years of data doesn't hold locks long.
const BATCH: i64 = 5000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/retention", get(list))
        .route("/admin/retention/run", post(run_now))
        .route("/admin/retention/{key}", put(save))
}

struct Rule {
    key: &'static str,
    title: &'static str,
    detail: &'static str,
    default_days: i32,
    min_days: i32,
    actions: &'static [&'static str],
    /// The ids of the rows past the cutoff ($1 = keep days), oldest first, $2 = limit.
    due: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        key: "lost_leads",
        title: "Lost leads",
        detail: "Leads closed as lost, counted from when they were closed. Deleting puts them in the CRM recycle bin, which empties itself.",
        default_days: 730,
        min_days: 30,
        actions: &["delete"],
        due: "SELECT id FROM crm_items WHERE entity_type = 1 AND deleted_at IS NULL AND stage_semantics = 'failure'
              AND COALESCE(closed_at, updated_at) < now() - make_interval(days => $1) ORDER BY COALESCE(closed_at, updated_at) LIMIT $2",
    },
    Rule {
        key: "inactive_contacts",
        title: "Inactive contacts",
        detail: "Contacts with nothing happening for this long and no open deals. Anonymising keeps what points at them (deals, invoices) without the person.",
        default_days: 1825,
        min_days: 365,
        actions: &["anonymise"],
        due: "SELECT i.id FROM crm_items i WHERE i.entity_type = 3 AND i.deleted_at IS NULL
              AND GREATEST(i.updated_at, COALESCE(i.last_activity_at, i.updated_at)) < now() - make_interval(days => $1)
              AND NOT EXISTS (SELECT 1 FROM crm_item_contacts c JOIN crm_items d ON d.id = c.item_id WHERE c.contact_id = i.id AND d.deleted_at IS NULL AND d.stage_semantics = 'process')
              ORDER BY i.updated_at LIMIT $2",
    },
    Rule {
        key: "emails",
        title: "CRM emails",
        detail: "Emails received or sent in the CRM, with their attachments, counted from when they were sent.",
        default_days: 1095,
        min_days: 90,
        actions: &["delete"],
        due: "SELECT id FROM crm_emails WHERE sent_at < now() - make_interval(days => $1) ORDER BY sent_at LIMIT $2",
    },
    Rule {
        key: "live_chats",
        title: "Live chats",
        detail: "Website live chat conversations, counted from when they were closed.",
        default_days: 365,
        min_days: 30,
        actions: &["delete"],
        due: "SELECT id FROM chat_sessions WHERE COALESCE(closed_at, last_message_at, started_at) < now() - make_interval(days => $1)
              AND (closed_at IS NOT NULL OR COALESCE(last_message_at, started_at) < now() - make_interval(days => $1 + 30)) ORDER BY started_at LIMIT $2",
    },
    Rule {
        key: "tickets",
        title: "Help desk tickets",
        detail: "Closed tickets with their messages, counted from when they were closed.",
        default_days: 1095,
        min_days: 90,
        actions: &["delete", "anonymise"],
        due: "SELECT id FROM support_tickets WHERE closed_at < now() - make_interval(days => $1) AND requester_name IS DISTINCT FROM 'Erased' ORDER BY closed_at LIMIT $2",
    },
    Rule {
        key: "site_visits",
        title: "Site visits",
        detail: "Visits recorded by the site tracking code (pages, sources, campaigns).",
        default_days: 395,
        min_days: 30,
        actions: &["delete"],
        due: "SELECT id FROM track_visits WHERE COALESCE(last_at, started_at) < now() - make_interval(days => $1) ORDER BY started_at LIMIT $2",
    },
    Rule {
        key: "form_submissions",
        title: "Web form submissions",
        detail: "What was sent through web forms (the records they created stay).",
        default_days: 365,
        min_days: 30,
        actions: &["delete"],
        due: "SELECT id FROM crm_webform_submissions WHERE created_at < now() - make_interval(days => $1) ORDER BY created_at LIMIT $2",
    },
    Rule {
        key: "marketing",
        title: "Marketing email recipients",
        detail: "Who got each campaign email and whether they opened it (the campaigns' totals go with them). Opt-outs are kept.",
        default_days: 730,
        min_days: 90,
        actions: &["delete"],
        due: "SELECT id FROM mkt_recipients WHERE COALESCE(sent_at, unsubscribed_at) < now() - make_interval(days => $1) ORDER BY sent_at LIMIT $2",
    },
    Rule {
        key: "bookings",
        title: "Bookings",
        detail: "Past bookings: anonymising keeps the booked times (the schedule's history) without who booked them.",
        default_days: 730,
        min_days: 30,
        actions: &["anonymise", "delete"],
        due: "SELECT id FROM bookings WHERE ends_at < now() - make_interval(days => $1) AND client_name <> 'Erased' ORDER BY ends_at LIMIT $2",
    },
    Rule {
        key: "orders",
        title: "Shop order contact details",
        detail: "Name, email, phone and address on shop orders; the orders and their amounts stay.",
        default_days: 1095,
        min_days: 90,
        actions: &["anonymise"],
        due: "SELECT item_id FROM shop_orders WHERE created_at < now() - make_interval(days => $1) AND name <> 'Erased' ORDER BY created_at LIMIT $2",
    },
    Rule {
        key: "call_recordings",
        title: "Call recordings",
        detail: "Recordings of video calls, with their files.",
        default_days: 180,
        min_days: 7,
        actions: &["delete"],
        due: "SELECT id FROM call_recordings WHERE COALESCE(stopped_at, started_at) < now() - make_interval(days => $1) ORDER BY started_at LIMIT $2",
    },
    Rule {
        key: "privacy_requests",
        title: "Answered personal data requests",
        detail: "Requests that were answered or rejected (what was done goes with them; the audit log keeps that it happened).",
        default_days: 1095,
        min_days: 365,
        actions: &["delete"],
        due: "SELECT id FROM privacy_requests WHERE status IN ('done', 'rejected') AND decided_at < now() - make_interval(days => $1) ORDER BY decided_at LIMIT $2",
    },
];

fn rule(key: &str) -> AppResult<&'static Rule> {
    RULES.iter().find(|r| r.key == key).ok_or(AppError::NotFound("rule"))
}

async fn config(state: &AppState) -> AppResult<Vec<(String, bool, i32, String, DateTime<Utc>)>> {
    Ok(sqlx::query!(r#"SELECT key, enabled, keep_days, action, updated_at FROM retention_rules"#).map(|r| (r.key, r.enabled, r.keep_days, r.action, r.updated_at)).fetch_all(&state.db).await?)
}

/// How many rows are past the cutoff now (counted up to 100 000).
async fn due_count(state: &AppState, r: &Rule, days: i32) -> AppResult<i64> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM ({}) x", r.due))).bind(days).bind(100_000i64).fetch_one(&state.db).await?)
}

/// Applies one rule to a batch. Returns how many rows it changed.
async fn apply(state: &AppState, r: &Rule, days: i32, action: &str) -> anyhow::Result<u64> {
    let mut tx = state.db.begin().await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(r.due.to_string())).bind(days).bind(BATCH).fetch_all(&mut *tx).await?;
    if ids.is_empty() {
        return Ok(0);
    }
    let mut files: Vec<(Uuid, String)> = vec![];
    let q = |sql: &'static str| sqlx::query(sql).bind(&ids);
    let n = match (r.key, action) {
        ("lost_leads", _) => q("UPDATE crm_items SET deleted_at = now(), deleted_by = NULL WHERE id = ANY($1)").execute(&mut *tx).await?.rows_affected(),
        ("inactive_contacts", _) => {
            let (n, f) = crate::privacy::anonymise_crm(&mut tx, &ids).await?;
            files = f;
            n
        }
        ("emails", _) => {
            files = sqlx::query_as("SELECT f.id, f.storage_path FROM files f JOIN crm_email_files ef ON ef.file_id = f.id WHERE ef.email_id = ANY($1)").bind(&ids).fetch_all(&mut *tx).await?;
            let n = q("DELETE FROM crm_emails WHERE id = ANY($1)").execute(&mut *tx).await?.rows_affected();
            discard_rows(&mut tx, &mut files).await?;
            n
        }
        ("live_chats", _) => q("DELETE FROM chat_sessions WHERE id = ANY($1)").execute(&mut *tx).await?.rows_affected(),
        ("tickets", "delete") => q("DELETE FROM support_tickets WHERE id = ANY($1)").execute(&mut *tx).await?.rows_affected(),
        ("tickets", _) => {
            q("UPDATE support_messages SET body = 'Erased', author_name = NULL, author_email = NULL WHERE ticket_id = ANY($1) AND NOT by_support").execute(&mut *tx).await?;
            q("UPDATE support_tickets SET requester_name = 'Erased', requester_email = NULL, contact_id = NULL WHERE id = ANY($1)").execute(&mut *tx).await?.rows_affected()
        }
        ("site_visits", _) => q("DELETE FROM track_visits WHERE id = ANY($1)").execute(&mut *tx).await?.rows_affected(),
        ("form_submissions", _) => q("DELETE FROM crm_webform_submissions WHERE id = ANY($1)").execute(&mut *tx).await?.rows_affected(),
        ("marketing", _) => q("DELETE FROM mkt_recipients WHERE id = ANY($1)").execute(&mut *tx).await?.rows_affected(),
        ("bookings", "delete") => q("DELETE FROM bookings WHERE id = ANY($1)").execute(&mut *tx).await?.rows_affected(),
        ("bookings", _) => q("UPDATE bookings SET client_name = 'Erased', client_email = NULL, client_phone = NULL, note = NULL, contact_id = NULL WHERE id = ANY($1)").execute(&mut *tx).await?.rows_affected(),
        ("orders", _) => q("UPDATE shop_orders SET name = 'Erased', email = 'erased@invalid', phone = NULL, address = '', note = NULL WHERE item_id = ANY($1)").execute(&mut *tx).await?.rows_affected(),
        ("call_recordings", _) => {
            files = sqlx::query_as("SELECT f.id, f.storage_path FROM files f JOIN call_recordings r ON r.file_id = f.id WHERE r.id = ANY($1)").bind(&ids).fetch_all(&mut *tx).await?;
            let n = q("DELETE FROM call_recordings WHERE id = ANY($1)").execute(&mut *tx).await?.rows_affected();
            discard_rows(&mut tx, &mut files).await?;
            n
        }
        ("privacy_requests", _) => q("DELETE FROM privacy_requests WHERE id = ANY($1)").execute(&mut *tx).await?.rows_affected(),
        _ => 0,
    };
    tx.commit().await?;
    for (_, rel) in files {
        crate::blob::discard(state, &rel).await;
    }
    Ok(n)
}

/// Removes the rows of files nothing else uses any more; those still used elsewhere are
/// dropped from the list so their content stays.
async fn discard_rows(tx: &mut sqlx::PgConnection, files: &mut Vec<(Uuid, String)>) -> sqlx::Result<()> {
    let mut gone = vec![];
    for (id, rel) in files.drain(..) {
        sqlx::query!("SAVEPOINT f").execute(&mut *tx).await?;
        match sqlx::query!("DELETE FROM files WHERE id = $1", id).execute(&mut *tx).await {
            Ok(_) => {
                sqlx::query!("RELEASE SAVEPOINT f").execute(&mut *tx).await?;
                gone.push((id, rel));
            }
            Err(_) => {
                sqlx::query!("ROLLBACK TO SAVEPOINT f").execute(&mut *tx).await?;
            }
        }
    }
    *files = gone;
    Ok(())
}

/// Runs every rule that is on; logs each.
pub async fn run(state: &AppState, by: Option<Uuid>) -> anyhow::Result<Vec<Value>> {
    let rules: Vec<(String, i32, String)> = sqlx::query!(r#"SELECT key, keep_days, action FROM retention_rules WHERE enabled ORDER BY key"#).map(|r| (r.key, r.keep_days, r.action)).fetch_all(&state.db).await?;
    let mut out = vec![];
    for (key, days, action) in rules {
        let Ok(r) = rule(&key) else { continue };
        let (affected, error) = match apply(state, r, days, &action).await {
            Ok(n) => (n as i64, None),
            Err(e) => {
                tracing::warn!(error = ?e, rule = key, "retention rule failed");
                (0, Some(format!("{e:#}")))
            }
        };
        sqlx::query!("INSERT INTO retention_runs (key, action, keep_days, affected, error, run_by) VALUES ($1, $2, $3, $4, $5, $6)", &key, &action, days, affected, error.as_deref(), by)
            .execute(&state.db)
            .await?;
        if affected > 0 || error.is_some() {
            audit::record(&state.db, by, "retention.applied", "retention_rule", Some(key.clone()), json!({ "action": action, "keep_days": days, "affected": affected, "error": error }), None).await;
        }
        out.push(json!({ "key": key, "action": action, "affected": affected, "error": error }));
    }
    sqlx::query!("DELETE FROM retention_runs WHERE ran_at < now() - interval '2 years'").execute(&state.db).await?;
    Ok(out)
}

pub async fn run_job(state: &AppState) -> anyhow::Result<()> {
    run(state, None).await.map(|_| ())
}

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::PRIVACY_MANAGE)?;
    let cfg = config(&state).await?;
    let mut rules = vec![];
    for r in RULES {
        let c = cfg.iter().find(|c| c.0 == r.key);
        let days = c.map(|c| c.2).unwrap_or(r.default_days);
        let last: Option<(DateTime<Utc>, i64, Option<String>)> = sqlx::query!(r#"SELECT ran_at, affected, error FROM retention_runs WHERE key = $1 ORDER BY ran_at DESC LIMIT 1"#, r.key).map(|r| (r.ran_at, r.affected, r.error))
            .fetch_optional(&state.db)
            .await?;
        rules.push(json!({
            "key": r.key, "title": r.title, "detail": r.detail, "actions": r.actions, "min_days": r.min_days, "default_days": r.default_days,
            "enabled": c.is_some_and(|c| c.1), "keep_days": days, "action": c.map(|c| c.3.clone()).unwrap_or_else(|| r.actions[0].to_string()),
            "due_now": due_count(&state, r, days).await?,
            "last_run": last.map(|(at, n, e)| json!({ "at": at, "affected": n, "error": e })),
        }));
    }
    let log: Vec<(String, String, i32, i64, Option<String>, DateTime<Utc>, Option<String>)> = sqlx::query!(r#"SELECT r.key, r.action, r.keep_days, r.affected, r.error, r.ran_at, trim(u.first_name || ' ' || u.last_name) AS "c6"
         FROM retention_runs r LEFT JOIN users u ON u.id = r.run_by WHERE r.affected > 0 OR r.error IS NOT NULL ORDER BY r.ran_at DESC LIMIT 50"#).map(|r| (r.key, r.action, r.keep_days, r.affected, r.error, r.ran_at, r.c6))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "rules": rules,
        "log": log.into_iter().map(|(k, a, d, n, e, at, by)| json!({ "key": k, "action": a, "keep_days": d, "affected": n, "error": e, "at": at, "by": by })).collect::<Vec<_>>(),
        "always_kept": ["The audit trail (tamper-evident; never removed)", "Invoices, payments and the books (kept as the law requires)", "Signed documents (evidence of what was agreed)"],
    })))
}

#[derive(Deserialize)]
struct SaveInput {
    enabled: bool,
    keep_days: i32,
    action: String,
}

async fn save(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(key): Path<String>, Json(b): Json<SaveInput>) -> AppResult<Json<Value>> {
    auth.require(perm::PRIVACY_MANAGE)?;
    let r = rule(&key)?;
    if b.keep_days < r.min_days || b.keep_days > 36500 {
        return Err(AppError::bad(format!("{}: keep at least {} days", r.title, r.min_days)));
    }
    if !r.actions.contains(&b.action.as_str()) {
        return Err(AppError::bad(format!("{}: can't {}", r.title, b.action)));
    }
    sqlx::query!("INSERT INTO retention_rules (key, enabled, keep_days, action, updated_by, updated_at) VALUES ($1, $2, $3, $4, $5, now())
         ON CONFLICT (key) DO UPDATE SET enabled = EXCLUDED.enabled, keep_days = EXCLUDED.keep_days, action = EXCLUDED.action, updated_by = EXCLUDED.updated_by, updated_at = now()", &key, b.enabled, b.keep_days, &b.action, auth.id)
    .execute(&state.db)
    .await?;
    audit::record(&state.db, Some(auth.id), "retention.rule", "retention_rule", Some(key.clone()), json!({ "enabled": b.enabled, "keep_days": b.keep_days, "action": b.action }), ip.as_deref()).await;
    Ok(Json(json!({ "key": key, "enabled": b.enabled, "keep_days": b.keep_days, "action": b.action, "due_now": due_count(&state, r, b.keep_days).await? })))
}

async fn run_now(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::PRIVACY_MANAGE)?;
    let out = run(&state, Some(auth.id)).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "results": out })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_are_sane() {
        for r in RULES {
            assert!(!r.actions.is_empty() && r.default_days >= r.min_days, "{}", r.key);
        }
    }
}
