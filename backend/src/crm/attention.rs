//! Proactive CRM: the open deals and leads that need someone to act, and why —
//!
//! - an activity is overdue;
//! - nothing is planned (no open activity);
//! - the client's last email waits for an answer (longer than the set hours);
//! - the close date has passed;
//! - the record has been in its stage longer than the stage's time limit.
//!
//! Everyone sees their own list (Insights → Needs attention; heads their team's, CRM
//! administrators anyone's). Every morning (8:00 company time) each responsible person gets one
//! notification summing theirs up — and an email too, if the setting says so.
use std::collections::BTreeMap;

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use chrono::{Timelike, Utc};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::access;
use crate::{auth::AuthUser, error::{AppError, AppResult}, state::AppState};

pub const JOB: &str = "crm.attention";

pub fn router() -> Router<AppState> {
    Router::new().route("/crm/attention", get(list)).route("/crm/attention/send-now", axum::routing::post(send_now_route))
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    code: String,
    type_name: String,
    title: String,
    stage: Option<String>,
    assigned_to: Option<Uuid>,
    overdue: i64,
    planned: i64,
    waiting_since: Option<chrono::DateTime<Utc>>,
    close_passed_days: Option<i32>,
    stage_over_minutes: Option<i64>,
}

/// Open deals and leads of these people (all, when None) with at least one reason.
async fn rows(state: &AppState, users: Option<&[Uuid]>) -> AppResult<Vec<Row>> {
    let hours: i32 = sqlx::query_scalar("SELECT reply_hours FROM crm_insight_settings WHERE id = 1").fetch_one(&state.db).await?;
    Ok(sqlx::query_as(
        "WITH open AS (
             SELECT i.id, t.code, t.name AS type_name, i.title, s.name AS stage, i.assigned_to, i.close_date, i.entity_type, i.stage_changed_at, s.sla_minutes
             FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type LEFT JOIN crm_stages s ON s.id = i.stage_id
             WHERE i.entity_type IN (1, 2) AND i.deleted_at IS NULL AND i.stage_semantics = 'process' AND ($1::uuid[] IS NULL OR i.assigned_to = ANY($1)))
         SELECT * FROM (
             SELECT o.id, o.code, o.type_name, o.title, o.stage, o.assigned_to,
                 (SELECT COUNT(*) FROM crm_activities a WHERE a.item_id = o.id AND NOT a.is_completed AND a.deadline < now()) AS overdue,
                 (SELECT COUNT(*) FROM crm_activities a WHERE a.item_id = o.id AND NOT a.is_completed AND (a.deadline IS NULL OR a.deadline >= now())) AS planned,
                 (SELECT CASE WHEN e.direction = 'in' AND e.sent_at < now() - make_interval(hours => $2) THEN e.sent_at END
                    FROM crm_emails e JOIN crm_email_links l ON l.email_id = e.id
                   WHERE (l.item_id = o.id OR l.item_id IN (SELECT contact_id FROM crm_item_contacts WHERE item_id = o.id)) AND e.status <> 'failed'
                   ORDER BY e.sent_at DESC LIMIT 1) AS waiting_since,
                 CASE WHEN o.entity_type = 2 AND o.close_date < current_date THEN (current_date - o.close_date) END AS close_passed_days,
                 CASE WHEN o.sla_minutes IS NOT NULL AND o.stage_changed_at + make_interval(mins => o.sla_minutes) < now()
                      THEN (extract(epoch FROM now() - o.stage_changed_at) / 60)::bigint - o.sla_minutes END AS stage_over_minutes
             FROM open o) x
         WHERE overdue > 0 OR planned = 0 OR waiting_since IS NOT NULL OR close_passed_days IS NOT NULL OR stage_over_minutes IS NOT NULL
         LIMIT 2000",
    )
    .bind(users)
    .bind(hours)
    .fetch_all(&state.db)
    .await?)
}

fn reasons(r: &Row) -> Vec<Value> {
    let mut out = vec![];
    if r.overdue > 0 {
        out.push(json!({ "kind": "overdue", "text": if r.overdue == 1 { "An activity is overdue".to_string() } else { format!("{} activities are overdue", r.overdue) } }));
    }
    if r.planned == 0 && r.overdue == 0 {
        out.push(json!({ "kind": "nothing_planned", "text": "Nothing is planned" }));
    }
    if let Some(t) = r.waiting_since {
        let h = (Utc::now() - t).num_hours();
        out.push(json!({ "kind": "unanswered", "text": if h < 48 { format!("The client's email has waited {h} hours") } else { format!("The client's email has waited {} days", h / 24) } }));
    }
    if let Some(d) = r.close_passed_days {
        out.push(json!({ "kind": "close_date", "text": format!("The close date passed {d} day{} ago", if d == 1 { "" } else { "s" }) }));
    }
    if let Some(m) = r.stage_over_minutes {
        let text = if m < 120 { format!("{m} minutes over the stage's time limit") } else if m < 2880 { format!("{} hours over the stage's time limit", m / 60) } else { format!("{} days over the stage's time limit", m / 1440) };
        out.push(json!({ "kind": "stage_time", "text": text }));
    }
    out
}

/// The weightier reasons first.
fn weight(kind: &str) -> i32 {
    match kind {
        "unanswered" => 5,
        "overdue" => 4,
        "stage_time" => 3,
        "close_date" => 2,
        _ => 1,
    }
}

#[derive(Deserialize)]
struct Q {
    user: Option<Uuid>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<Q>) -> AppResult<Json<Value>> {
    let user = q.user.unwrap_or(auth.id);
    if user != auth.id && !access::is_crm_admin(&auth) && !crate::hr::manages(&state, &auth, user).await? {
        return Err(AppError::Forbidden);
    }
    let rs = rows(&state, Some(&[user])).await?;
    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    let mut items: Vec<(i32, Value)> = rs
        .iter()
        .map(|r| {
            let why = reasons(r);
            for w in &why {
                *counts.entry(w["kind"].as_str().unwrap_or("").to_string()).or_default() += 1;
            }
            let score: i32 = why.iter().map(|w| weight(w["kind"].as_str().unwrap_or(""))).sum();
            (score, json!({ "id": r.id, "type": r.code, "type_name": r.type_name, "title": r.title, "stage": r.stage, "link": format!("/crm/{}/{}", r.code, r.id), "reasons": why }))
        })
        .collect();
    items.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(Json(json!({ "items": items.into_iter().map(|i| i.1).collect::<Vec<_>>(), "counts": counts })))
}

/// Hourly: at 8:00 company time, each responsible person gets their summary (once a day).
pub async fn run(state: &AppState) -> anyhow::Result<()> {
    let (digest, email): (bool, bool) = sqlx::query_as("SELECT attention_digest, attention_email FROM crm_insight_settings WHERE id = 1").fetch_one(&state.db).await?;
    if !digest {
        return Ok(());
    }
    let tz: chrono_tz::Tz = sqlx::query_scalar::<_, Value>("SELECT value FROM settings WHERE key = 'default_timezone'")
        .fetch_optional(&state.db)
        .await?
        .and_then(|v| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(chrono_tz::UTC);
    let now = Utc::now().with_timezone(&tz);
    if now.hour() != 8 {
        return Ok(());
    }
    send_digests(state, email, &now.date_naive().to_string()).await
}

async fn send_digests(state: &AppState, email: bool, day: &str) -> anyhow::Result<()> {
    let rs = rows(state, None).await?;
    let mut per: BTreeMap<Uuid, Vec<&Row>> = BTreeMap::new();
    for r in &rs {
        if let Some(u) = r.assigned_to {
            per.entry(u).or_default().push(r);
        }
    }
    let company = crate::routes::auth::company_name(state).await;
    let mut redis = state.redis.clone();
    for (user, list) in per {
        // Once a day per person, across instances.
        let fresh: bool = redis::cmd("SET").arg(format!("crm:attention:{user}:{day}")).arg("1").arg("NX").arg("EX").arg(86400 * 2).query_async::<Option<String>>(&mut redis).await?.is_some();
        if !fresh {
            continue;
        }
        let active: Option<String> = sqlx::query_scalar("SELECT email FROM users WHERE id = $1 AND is_active AND NOT is_guest").bind(user).fetch_optional(&state.db).await?;
        let Some(addr) = active else { continue };
        let mut kinds: BTreeMap<&str, i64> = BTreeMap::new();
        for r in &list {
            for w in reasons(r) {
                *kinds.entry(match w["kind"].as_str().unwrap_or("") {
                    "overdue" => "with overdue activities",
                    "nothing_planned" => "with nothing planned",
                    "unanswered" => "waiting for your answer",
                    "close_date" => "past their close date",
                    _ => "over their stage's time limit",
                }).or_default() += 1;
            }
        }
        let summary = kinds.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(", ");
        let title = format!("{} {} need{} attention", list.len(), if list.len() == 1 { "deal or lead" } else { "deals and leads" }, if list.len() == 1 { "s" } else { "" });
        crate::notify::notify(state, &[user], crate::notify::NewNotification { kind: "crm.attention", title: title.clone(), body: Some(summary.clone()), link: Some("/crm/insights?tab=attention".into()), actor: None }).await;
        if email {
            let link = format!("{}/crm/insights?tab=attention", state.config.public_url);
            let mut lines = vec![summary];
            lines.extend(list.iter().take(10).map(|r| format!("• {} — {}", r.title, reasons(r).iter().filter_map(|w| w["text"].as_str().map(String::from)).collect::<Vec<_>>().join("; "))));
            let m = crate::mail::notice(&addr, &company, &title, &title, &lines, Some(("See them", &link)), "You get this because these records are yours in the CRM.");
            let _ = crate::jobs::send_email(state, m).await;
        }
    }
    Ok(())
}

/// Today's summaries now, whatever the hour (CRM administrators).
async fn send_now_route(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    if !access::is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    send_now(&state).await.map_err(AppError::Internal)?;
    Ok(Json(json!({ "sent": true })))
}

async fn send_now(state: &AppState) -> anyhow::Result<()> {
    let email: bool = sqlx::query_scalar("SELECT attention_email FROM crm_insight_settings WHERE id = 1").fetch_one(&state.db).await?;
    let day = Utc::now().date_naive().to_string();
    let mut redis = state.redis.clone();
    // A manual run sends again even if the morning's went out.
    let keys: Vec<String> = redis.keys(format!("crm:attention:*:{day}")).await.unwrap_or_default();
    for k in keys {
        let _: Result<(), _> = redis.del(k).await;
    }
    send_digests(state, email, &day).await
}
