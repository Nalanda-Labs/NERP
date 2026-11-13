//! Insights from the company's own records — no outside service.
//!
//! **Deal health** (open deals): a score from 100 down, with the reasons, compared with how the
//! pipeline's won deals went:
//! - contact: days since anything happened on the deal;
//! - pace: days in the current stage against how long won deals stayed there;
//! - people: contacts on the deal against how many won deals had;
//! - replies: an email to the client left unanswered;
//! - dates and next steps: a close date passed or missing, overdue activities, nothing planned;
//! - payment: the client usually pays late (from the books).
//!
//! Healthy is 70 and up, at risk 40 to 69, poor below.
//!
//! **Next steps that worked** (an open deal): for its current stage, the steps that went with
//! winning — among the pipeline's closed deals that passed through the stage, how often those
//! that took a step there (a call, a meeting, an email, a to-do, a document) were won, against
//! those that didn't — suggested when the deal hasn't taken them in this stage yet, with when
//! won deals usually took them.
//!
//! **Silent clients**: clients (a company, or a contact without one) with at least three won
//! deals or completed orders whose usual rhythm broke — no order for well over the usual gap
//! (more than 1.5 × the median gap and 14 days past it). Optionally (a setting) a task to check
//! in goes to the client's responsible person, once per silence.

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use super::{access, DEAL};
use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    state::AppState,
};

pub const SILENT_JOB: &str = "crm.silent_clients";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/deal/health", get(list_health))
        .route("/crm/items/{id}/health", get(one_health))
        .route("/crm/items/{id}/next-steps", get(next_steps))
        .route("/crm/silent-clients", get(silent_list))
        .route("/crm/insights/settings", get(get_settings).put(put_settings))
}

#[derive(sqlx::FromRow)]
struct DealFacts {
    id: Uuid,
    title: String,
    category_id: Option<Uuid>,
    stage_id: Option<Uuid>,
    stage_name: Option<String>,
    assigned_to: Option<Uuid>,
    assigned_name: Option<String>,
    opportunity_base: rust_decimal::Decimal,
    close_date: Option<NaiveDate>,
    last_activity_at: DateTime<Utc>,
    in_stage_since: Option<DateTime<Utc>>,
    contacts: i64,
    overdue: i64,
    planned: i64,
    unanswered_since: Option<DateTime<Utc>>,
    client_late_days: Option<f64>,
}

const FACTS: &str = "
SELECT i.id, i.title, i.category_id, i.stage_id, s.name AS stage_name, i.assigned_to, trim(u.first_name || ' ' || u.last_name) AS assigned_name,
       i.opportunity_base, i.close_date, i.last_activity_at,
       (SELECT MAX(h.entered_at) FROM crm_stage_history h WHERE h.item_id = i.id AND h.stage_id = i.stage_id) AS in_stage_since,
       (SELECT COUNT(*) FROM crm_item_contacts c WHERE c.item_id = i.id) AS contacts,
       (SELECT COUNT(*) FROM crm_activities a WHERE a.item_id = i.id AND NOT a.is_completed AND a.deadline < now()) AS overdue,
       (SELECT COUNT(*) FROM crm_activities a WHERE a.item_id = i.id AND NOT a.is_completed AND (a.deadline IS NULL OR a.deadline >= now())) AS planned,
       -- The newest email of the client's conversations, when it is ours and still unanswered.
       (SELECT CASE WHEN e.direction = 'out' THEN e.sent_at END FROM crm_emails e JOIN crm_email_links l ON l.email_id = e.id
         WHERE (l.item_id = i.id OR l.item_id IN (SELECT contact_id FROM crm_item_contacts WHERE item_id = i.id)) AND e.status <> 'failed'
         ORDER BY e.sent_at DESC LIMIT 1) AS unanswered_since,
       (SELECT AVG(GREATEST(pay.d - COALESCE(a.due_date, a.date), 0))::float8
          FROM acc_invoices a CROSS JOIN LATERAL (SELECT MAX(p.date) AS d FROM acc_allocations al JOIN acc_payments p ON p.id = al.payment_id
                                                 WHERE al.doc_kind = 'invoice' AND al.doc_id = a.item_id AND p.status = 'posted') pay
         WHERE a.status = 'paid' AND a.party_id = i.company_id AND pay.d IS NOT NULL) AS client_late_days
FROM crm_items i JOIN crm_stages s ON s.id = i.stage_id LEFT JOIN users u ON u.id = i.assigned_to
WHERE i.entity_type = 2 AND i.deleted_at IS NULL AND s.semantics = 'process'";

/// How the pipeline's won deals went: (category, stage) → median days in that stage, and
/// category → median contacts on a won deal.
struct Norms {
    stage_days: HashMap<Uuid, f64>,
    contacts: HashMap<Uuid, f64>,
}

async fn norms(state: &AppState) -> AppResult<Norms> {
    let stage_days: HashMap<Uuid, f64> = sqlx::query_as::<_, (Uuid, f64)>(
        "WITH won AS (SELECT DISTINCT item_id FROM crm_stage_history WHERE semantics = 'success'),
              spans AS (SELECT h.stage_id, h.semantics, EXTRACT(EPOCH FROM (LEAD(h.entered_at) OVER (PARTITION BY h.item_id ORDER BY h.entered_at) - h.entered_at)) / 86400 AS days
                        FROM crm_stage_history h JOIN won w ON w.item_id = h.item_id)
         SELECT stage_id, percentile_cont(0.5) WITHIN GROUP (ORDER BY days)::float8 FROM spans WHERE days IS NOT NULL AND semantics = 'process' GROUP BY stage_id HAVING COUNT(*) >= 3",
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .collect();
    let contacts: HashMap<Uuid, f64> = sqlx::query_as::<_, (Uuid, f64)>(
        "SELECT i.category_id, percentile_cont(0.5) WITHIN GROUP (ORDER BY (SELECT COUNT(*) FROM crm_item_contacts c WHERE c.item_id = i.id))::float8
         FROM crm_items i JOIN crm_stages s ON s.id = i.stage_id
         WHERE i.entity_type = 2 AND i.deleted_at IS NULL AND s.semantics = 'success' AND i.category_id IS NOT NULL
         GROUP BY i.category_id HAVING COUNT(*) >= 3",
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .collect();
    Ok(Norms { stage_days, contacts })
}

fn score(f: &DealFacts, n: &Norms, now: DateTime<Utc>) -> Value {
    let mut points: i64 = 100;
    let mut reasons: Vec<Value> = Vec::new();
    let mut hit = |penalty: i64, kind: &str, text: String| {
        points -= penalty;
        reasons.push(json!({ "kind": kind, "penalty": penalty, "text": text }));
    };
    let today = now.date_naive();

    let quiet = (now - f.last_activity_at).num_days();
    if quiet > 14 {
        hit(25, "contact", format!("Nothing has happened for {quiet} days"));
    } else if quiet > 7 {
        hit(10, "contact", format!("Nothing has happened for {quiet} days"));
    }

    if let (Some(since), Some(stage)) = (f.in_stage_since, f.stage_id) {
        let days = (now - since).num_hours() as f64 / 24.0;
        let stage_name = f.stage_name.clone().unwrap_or_default();
        match n.stage_days.get(&stage) {
            Some(usual) if days > 3.0 && days > usual * 2.0 => hit(20, "pace", format!("In “{stage_name}” for {} days; won deals usually move on in {}", days.round(), usual.round().max(1.0))),
            Some(usual) if days > 3.0 && days > usual * 1.5 => hit(10, "pace", format!("In “{stage_name}” for {} days; won deals usually move on in {}", days.round(), usual.round().max(1.0))),
            None if days > 30.0 => hit(10, "pace", format!("In “{stage_name}” for {} days", days.round())),
            _ => {}
        }
    }

    if f.contacts == 0 {
        hit(15, "people", "No contact person on the deal".into());
    } else if let Some(usual) = f.category_id.and_then(|c| n.contacts.get(&c))
        && *usual >= 2.0 && (f.contacts as f64) < *usual {
            hit(10, "people", format!("{} contact{}; won deals usually have {}", f.contacts, if f.contacts == 1 { "" } else { "s" }, usual.round()));
        }

    if let Some(sent) = f.unanswered_since {
        let days = (now - sent).num_days();
        if days > 5 {
            hit(15, "replies", format!("Your last email ({} days ago) is unanswered", days));
        }
    }

    match f.close_date {
        Some(d) if d < today => hit(20, "dates", format!("The close date passed {} days ago", (today - d).num_days())),
        None => hit(5, "dates", "No close date".into()),
        _ => {}
    }
    if f.overdue > 0 {
        hit(10, "next_step", format!("{} overdue activit{}", f.overdue, if f.overdue == 1 { "y" } else { "ies" }));
    } else if f.planned == 0 {
        hit(10, "next_step", "No next step planned".into());
    }

    if let Some(late) = f.client_late_days.filter(|d| *d > 15.0) {
        hit(10, "payment", format!("The client pays {} days late on average", late.round()));
    }

    let points = points.clamp(0, 100);
    json!({
        "id": f.id, "title": f.title, "stage": f.stage_name, "assigned_to": f.assigned_to, "assigned_name": f.assigned_name,
        "amount": f.opportunity_base.round_dp(2), "score": points,
        "grade": if points >= 70 { "healthy" } else if points >= 40 { "at_risk" } else { "poor" },
        "reasons": reasons,
    })
}

#[derive(Deserialize)]
struct HealthQuery {
    category_id: Option<Uuid>,
    /// healthy | at_risk | poor
    grade: Option<String>,
    /// Only my deals.
    mine: Option<bool>,
}

/// Open deals the person can see, least healthy first.
async fn list_health(State(state): State<AppState>, auth: AuthUser, Query(q): Query<HealthQuery>) -> AppResult<Json<Value>> {
    let scope = access::scope(&state, &auth, DEAL, "read").await?;
    if scope == access::Scope::None {
        return Err(AppError::Forbidden);
    }
    let visible: Option<Vec<Uuid>> = match scope {
        access::Scope::All => None,
        access::Scope::Own => Some(vec![auth.id]),
        s => Some(access::team(&state, auth.id, s).await?),
    };
    let facts: Vec<DealFacts> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{FACTS} AND ($1::uuid IS NULL OR i.category_id = $1) AND ($2::uuid[] IS NULL OR i.assigned_to = ANY($2)) AND (NOT $3 OR i.assigned_to = $4)
         ORDER BY i.updated_at DESC LIMIT 2000"
    )))
    .bind(q.category_id)
    .bind(visible)
    .bind(q.mine.unwrap_or(false))
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    let n = norms(&state).await?;
    let now = Utc::now();
    let mut items: Vec<Value> = facts.iter().map(|f| score(f, &n, now)).filter(|v| q.grade.as_deref().is_none_or(|g| v["grade"] == g)).collect();
    items.sort_by_key(|v| v["score"].as_i64().unwrap_or(0));
    let count = |g: &str| facts.iter().map(|f| score(f, &n, now)).filter(|v| v["grade"] == g).count();
    Ok(Json(json!({ "items": items, "counts": { "healthy": count("healthy"), "at_risk": count("at_risk"), "poor": count("poor") } })))
}

async fn one_health(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = super::items::load_row(&state, id, false).await?;
    access::require_read(&state, &auth, row.entity_type, id, row.assigned_to).await?;
    if row.entity_type != DEAL {
        return Err(AppError::bad("health is for deals"));
    }
    let f: Option<DealFacts> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{FACTS} AND i.id = $1"))).bind(id).fetch_optional(&state.db).await?;
    match f {
        Some(f) => Ok(Json(score(&f, &norms(&state).await?, Utc::now()))),
        // Won or lost: nothing to judge.
        None => Ok(Json(json!({ "id": id, "score": null, "grade": null, "reasons": [] }))),
    }
}

// ---------------------------------------------------------------------------
// Next steps that worked
// ---------------------------------------------------------------------------

/// The steps of a stage window, as (step kind, at) for activities created, emails sent and
/// documents made while the deal was in the stage.
const STEPS: &str = "
    SELECT 'call' AS step, a.item_id, a.created_at AS at FROM crm_activities a WHERE a.kind = 'call'
    UNION ALL SELECT 'meeting', a.item_id, a.created_at FROM crm_activities a WHERE a.kind = 'meeting'
    UNION ALL SELECT 'todo', a.item_id, a.created_at FROM crm_activities a WHERE a.kind = 'todo'
    UNION ALL SELECT 'email', l.item_id, e.sent_at FROM crm_emails e JOIN crm_email_links l ON l.email_id = e.id WHERE e.direction = 'out'
    UNION ALL SELECT 'document', d.item_id, d.created_at FROM crm_documents d";

const STEP_TEXT: &[(&str, &str)] = &[
    ("call", "Call the client"),
    ("meeting", "Hold a meeting"),
    ("email", "Email the client"),
    ("todo", "Plan a to-do"),
    ("document", "Send a document (quote, offer)"),
];

async fn next_steps(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = super::items::load_row(&state, id, false).await?;
    access::require_read(&state, &auth, row.entity_type, id, row.assigned_to).await?;
    if row.entity_type != DEAL {
        return Err(AppError::bad("next steps are for deals"));
    }
    let Some(stage) = row.stage_id else { return Ok(Json(json!({ "items": [] }))) };
    // Closed deals that passed through this stage: their window in it, how they ended, and
    // which steps they took there (with days after entering).
    let rows: Vec<(Uuid, bool, String, f64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH win AS (
            SELECT h.item_id, h.entered_at AS from_at,
                   COALESCE(LEAD(h.entered_at) OVER (PARTITION BY h.item_id ORDER BY h.entered_at), now()) AS to_at, h.stage_id
            FROM crm_stage_history h),
          closed AS (
            SELECT i.id, s.semantics = 'success' AS won FROM crm_items i JOIN crm_stages s ON s.id = i.stage_id
            WHERE i.entity_type = 2 AND i.deleted_at IS NULL AND s.semantics IN ('success', 'failure') AND i.id <> $2),
          windows AS (SELECT w.item_id, w.from_at, w.to_at, c.won FROM win w JOIN closed c ON c.id = w.item_id WHERE w.stage_id = $1)
         SELECT w.item_id, w.won, COALESCE(x.step, ''), COALESCE(EXTRACT(EPOCH FROM (x.at - w.from_at)) / 86400, -1)::float8
         FROM windows w LEFT JOIN ({STEPS}) x ON x.item_id = w.item_id AND x.at >= w.from_at AND x.at < w.to_at"
    )))
    .bind(stage)
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    // Per closed deal: won?, and the steps it took (first time each, days in).
    let mut deals: HashMap<Uuid, (bool, HashMap<String, f64>)> = HashMap::new();
    for (item, won, step, days) in rows {
        let e = deals.entry(item).or_insert_with(|| (won, HashMap::new()));
        if !step.is_empty() {
            let d = e.1.entry(step).or_insert(days);
            *d = d.min(days);
        }
    }
    // What this deal did in its current stage so far.
    let since: Option<DateTime<Utc>> = sqlx::query_scalar!(r#"SELECT MAX(entered_at) AS "v?" FROM crm_stage_history WHERE item_id = $1 AND stage_id = $2"#, id, stage).fetch_one(&state.db).await?;
    let done: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT DISTINCT x.step FROM ({STEPS}) x WHERE x.item_id = $1 AND x.at >= $2")))
        .bind(id)
        .bind(since.unwrap_or_else(|| Utc::now() - Duration::days(3650)))
        .fetch_all(&state.db)
        .await?;
    let total = deals.len();
    let mut out: Vec<(f64, Value)> = Vec::new();
    for (step, text) in STEP_TEXT {
        let with: Vec<&(bool, HashMap<String, f64>)> = deals.values().filter(|d| d.1.contains_key(*step)).collect();
        let without: Vec<&(bool, HashMap<String, f64>)> = deals.values().filter(|d| !d.1.contains_key(*step)).collect();
        if with.len() < 3 {
            continue;
        }
        let rate = |v: &[&(bool, HashMap<String, f64>)]| if v.is_empty() { 0.0 } else { v.iter().filter(|d| d.0).count() as f64 / v.len() as f64 };
        let (r_with, r_without) = (rate(&with), rate(&without));
        // Worth suggesting: clearly more often won with it.
        if r_with < 0.5 || r_with < r_without + 0.15 {
            continue;
        }
        let mut days: Vec<f64> = with.iter().filter(|d| d.0).filter_map(|d| d.1.get(*step).copied()).collect();
        days.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = days.get(days.len() / 2).copied().unwrap_or(0.0);
        out.push((
            r_with - r_without,
            json!({
                "step": step, "text": text, "done": done.iter().any(|d| d == step),
                "won_with": (r_with * 100.0).round(), "won_without": (r_without * 100.0).round(),
                "deals_with": with.len(), "deals": total, "usual_day": median.round(),
            }),
        ));
    }
    out.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    Ok(Json(json!({ "stage_id": stage, "based_on": total, "items": out.into_iter().map(|x| x.1).collect::<Vec<_>>() })))
}

// ---------------------------------------------------------------------------
// Silent clients
// ---------------------------------------------------------------------------

/// Clients whose ordering rhythm broke: (client, name, code, responsible, orders, median gap
/// days, last order, days since).
async fn silent(state: &AppState) -> AppResult<Vec<(Uuid, String, String, Option<Uuid>, i64, f64, DateTime<Utc>, i64)>> {
    let rows: Vec<(Uuid, String, String, Option<Uuid>, i64, f64, DateTime<Utc>)> = sqlx::query_as(
        "WITH orders AS (
            SELECT COALESCE(i.company_id, (SELECT c.contact_id FROM crm_item_contacts c WHERE c.item_id = i.id ORDER BY c.is_primary DESC, c.sort LIMIT 1)) AS client,
                   COALESCE((SELECT MAX(h.entered_at) FROM crm_stage_history h WHERE h.item_id = i.id AND h.semantics = 'success'), i.updated_at) AS at
            FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type JOIN crm_stages s ON s.id = i.stage_id
            WHERE (i.entity_type = 2 OR t.kind = 'order') AND i.deleted_at IS NULL AND s.semantics = 'success'),
          gaps AS (
            SELECT client, at, EXTRACT(EPOCH FROM (at - LAG(at) OVER (PARTITION BY client ORDER BY at))) / 86400 AS gap FROM orders WHERE client IS NOT NULL)
         SELECT g.client, c.title, t.code, c.assigned_to, COUNT(*) AS n, percentile_cont(0.5) WITHIN GROUP (ORDER BY g.gap)::float8 AS median, MAX(g.at) AS last
         FROM gaps g JOIN crm_items c ON c.id = g.client AND c.deleted_at IS NULL JOIN crm_entity_types t ON t.id = c.entity_type
         GROUP BY g.client, c.title, t.code, c.assigned_to
         HAVING COUNT(*) >= 3",
    )
    .fetch_all(&state.db)
    .await?;
    let now = Utc::now();
    Ok(rows
        .into_iter()
        .filter_map(|(id, name, code, owner, n, median, last)| {
            let since = (now - last).num_days();
            let median = median.max(1.0);
            (since as f64 > (median * 1.5).max(median + 14.0)).then_some((id, name, code, owner, n, median, last, since))
        })
        .collect())
}

async fn silent_list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    if !auth.can(perm::CRM_ADMIN) && access::scope(&state, &auth, DEAL, "read").await? == access::Scope::None {
        return Err(AppError::Forbidden);
    }
    let mut rows = silent(&state).await?;
    rows.sort_by_key(|r| std::cmp::Reverse(r.7));
    let names: HashMap<Uuid, String> = sqlx::query!(r#"SELECT id AS "id!", trim(first_name || ' ' || last_name) AS "c1!" FROM users WHERE id = ANY($1)"#, &(rows.iter().filter_map(|r| r.3).collect::<Vec<_>>())).map(|r| (r.id, r.c1))
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .collect();
    let tasks: HashMap<Uuid, Uuid> = sqlx::query!(r#"SELECT client_id, task_id FROM crm_silent_alerts WHERE client_id = ANY($1)"#, &(rows.iter().map(|r| r.0).collect::<Vec<_>>())).map(|r| (r.client_id, r.task_id))
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .filter_map(|(c, t)| t.map(|t| (c, t)))
        .collect();
    Ok(Json(json!({
        "items": rows.into_iter().map(|(id, name, code, owner, n, median, last, since)| json!({
            "id": id, "name": name, "link": format!("/crm/{code}/{id}"), "responsible_id": owner, "responsible_name": owner.and_then(|o| names.get(&o)),
            "orders": n, "usual_gap_days": median.round(), "last_order_at": last, "days_since": since,
            "expected_by": (last + Duration::days(median.round() as i64)).date_naive(), "task_id": tasks.get(&id),
        })).collect::<Vec<_>>(),
    })))
}

/// Daily: a task to check in with each newly silent client (when the setting is on).
pub async fn run_silent(state: &AppState) -> anyhow::Result<()> {
    let on: bool = sqlx::query_scalar!("SELECT silent_tasks FROM crm_insight_settings WHERE id = 1").fetch_one(&state.db).await?;
    if !on {
        return Ok(());
    }
    for (id, name, _code, owner, n, median, last, since) in silent(state).await.map_err(|e| anyhow::anyhow!(e.to_string()))? {
        let fresh = sqlx::query!("INSERT INTO crm_silent_alerts (client_id, last_order_at) VALUES ($1, $2) ON CONFLICT DO NOTHING", id, last)
            .execute(&state.db)
            .await?
            .rows_affected();
        if fresh == 0 {
            continue;
        }
        let who = match owner {
            Some(o) => Some(o),
            None => sqlx::query_scalar!("SELECT u.id FROM users u JOIN user_roles ur ON ur.user_id = u.id JOIN role_permissions rp ON rp.role_id = ur.role_id WHERE u.is_active AND rp.permission IN ('*', 'crm.admin') ORDER BY u.created_at LIMIT 1")
                .fetch_optional(&state.db)
                .await?,
        };
        let Some(who) = who else { continue };
        let body = crate::tasks::items::CreateTask {
            title: format!("Check in with {name}"),
            description: Some(format!(
                "{name} usually orders every {} days ({n} orders so far); the last order was {since} days ago.",
                median.round()
            )),
            responsible_id: Some(who),
            deadline: Some(Utc::now() + Duration::days(3)),
            crm_item_ids: vec![id],
            ..Default::default()
        };
        match crate::tasks::items::create_task(state, &super::automation::system_auth(), body).await {
            Ok(task) => {
                sqlx::query!("UPDATE crm_silent_alerts SET task_id = $3 WHERE client_id = $1 AND last_order_at = $2", id, last, task).execute(&state.db).await?;
            }
            Err(e) => tracing::warn!(client = %id, error = ?e, "could not make the check-in task"),
        }
    }
    Ok(())
}

async fn get_settings(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    let (on, digest, email, hours): (bool, bool, bool, i32) =
        sqlx::query!(r#"SELECT silent_tasks, attention_digest, attention_email, reply_hours FROM crm_insight_settings WHERE id = 1"#).map(|r| (r.silent_tasks, r.attention_digest, r.attention_email, r.reply_hours)).fetch_one(&state.db).await?;
    Ok(Json(json!({ "silent_tasks": on, "attention_digest": digest, "attention_email": email, "reply_hours": hours })))
}

#[derive(Deserialize)]
struct SettingsInput {
    silent_tasks: Option<bool>,
    attention_digest: Option<bool>,
    attention_email: Option<bool>,
    reply_hours: Option<i32>,
}

async fn put_settings(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SettingsInput>) -> AppResult<Json<Value>> {
    if !auth.can(perm::CRM_ADMIN) {
        return Err(AppError::Forbidden);
    }
    if b.reply_hours.is_some_and(|h| !(1..=720).contains(&h)) {
        return Err(AppError::bad("an answer is due within 1 to 720 hours"));
    }
    sqlx::query!("UPDATE crm_insight_settings SET silent_tasks = COALESCE($1, silent_tasks), attention_digest = COALESCE($2, attention_digest),
                attention_email = COALESCE($3, attention_email), reply_hours = COALESCE($4, reply_hours) WHERE id = 1", b.silent_tasks, b.attention_digest, b.attention_email, b.reply_hours)
    .execute(&state.db)
    .await?;
    if b.silent_tasks == Some(true) {
        crate::jobs::enqueue(&state.db, SILENT_JOB, json!({}), None).await?;
        state.job_wakeup.notify_one();
    }
    get_settings(State(state), auth).await
}
