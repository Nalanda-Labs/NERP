//! Automation preview: before a rule is switched on (or changed), how often it would have run
//! in the past days — "would have run 312 times, on 280 deals, in the last 30 days".
//!
//! - **Robots** run when a record enters their stage: every entry into the stage in the period
//!   counts, for records that still exist and pass the robot's conditions.
//! - **Triggers** move a record when something happens: the period's events of that kind on
//!   the pipeline's records (activities done, comments, documents, emails received, web form
//!   submissions, signatures, finished tasks, bookings), with the trigger's own settings
//!   (activity kind, template, form, booking status), for records that pass its conditions.
//!   Webhook calls aren't recorded, so they can't be previewed.
//!
//! Conditions are checked against the records as they are now (their values back then aren't
//! kept), which the answer says.
use std::collections::{BTreeSet, HashMap};

use axum::{extract::State, routing::post, Json, Router};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{conditions_hold, context, require_admin};
use crate::{auth::AuthUser, error::{AppError, AppResult}, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new().route("/crm/automation/preview", post(preview))
}

#[derive(Deserialize)]
struct Input {
    /// robot | trigger
    kind: String,
    category_id: Uuid,
    /// A robot's stage.
    stage_id: Option<Uuid>,
    /// A trigger's event and its settings.
    event: Option<String>,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    conditions: Value,
    days: Option<i64>,
}

/// Records checked at most (the most recent first).
const CAP: usize = 5000;

async fn preview(State(state): State<AppState>, auth: AuthUser, Json(b): Json<Input>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let days = b.days.unwrap_or(30).clamp(1, 365);
    let since = Utc::now() - Duration::days(days);
    let str_param = |k: &str| b.params.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    // (record, when) for every time the rule would have been considered.
    let hits: Vec<(Uuid, DateTime<Utc>)> = match b.kind.as_str() {
        "robot" => {
            let stage = b.stage_id.ok_or_else(|| AppError::bad("choose the robot's stage"))?;
            sqlx::query_as(
                "SELECT h.item_id, h.entered_at FROM crm_stage_history h JOIN crm_items i ON i.id = h.item_id
                 WHERE h.stage_id = $1 AND h.category_id = $2 AND h.entered_at >= $3 AND i.deleted_at IS NULL ORDER BY h.entered_at DESC",
            )
            .bind(stage)
            .bind(b.category_id)
            .bind(since)
            .fetch_all(&state.db)
            .await?
        }
        "trigger" => {
            let event = b.event.as_deref().ok_or_else(|| AppError::bad("choose the trigger's event"))?;
            let q = match event {
                "activity_done" => "SELECT a.item_id, a.completed_at FROM crm_activities a WHERE a.is_completed AND a.completed_at >= $2 AND ($3::text IS NULL OR a.kind = $3)",
                "comment_added" => "SELECT t.item_id, t.created_at FROM crm_timeline t WHERE t.kind = 'comment' AND t.created_at >= $2 AND ($3::text IS NULL OR TRUE)",
                "document_created" => "SELECT d.item_id, d.created_at FROM crm_documents d WHERE d.created_at >= $2 AND ($3::text IS NULL OR d.template_id::text = $3)",
                "email_received" => "SELECT l.item_id, e.sent_at FROM crm_emails e JOIN crm_email_links l ON l.email_id = e.id WHERE e.direction = 'in' AND e.sent_at >= $2 AND ($3::text IS NULL OR TRUE)",
                "webform_submitted" => "SELECT s.item_id, s.created_at FROM crm_webform_submissions s WHERE s.item_id IS NOT NULL AND s.created_at >= $2 AND ($3::text IS NULL OR s.form_id::text = $3)",
                "signature_completed" => "SELECT r.item_id, r.completed_at FROM sign_requests r WHERE r.status = 'completed' AND r.completed_at >= $2 AND ($3::text IS NULL OR r.template_id::text = $3)",
                "signature_declined" => "SELECT r.item_id, r.created_at FROM sign_requests r WHERE r.status = 'declined' AND r.created_at >= $2 AND ($3::text IS NULL OR r.template_id::text = $3)",
                "tasks_done" => "SELECT l.item_id, max(t.completed_at) FROM task_crm_links l JOIN tasks t ON t.id = l.task_id GROUP BY l.item_id
                                 HAVING bool_and(t.status = 'done') AND max(t.completed_at) >= $2 AND ($3::text IS NULL OR TRUE)",
                "booking" => "SELECT b.deal_id, b.created_at FROM bookings b WHERE b.deal_id IS NOT NULL AND b.created_at >= $2 AND ($3::text IS NULL OR b.status = $3)",
                "webhook" => return Ok(Json(json!({ "days": days, "unknown": true, "note": "Webhook calls aren't recorded, so this trigger can't be previewed." }))),
                _ => return Err(AppError::bad("unknown event")),
            };
            let param = match event {
                "activity_done" => str_param("kind"),
                "document_created" => str_param("template_id"),
                "webform_submitted" => str_param("form_id"),
                "signature_completed" | "signature_declined" => str_param("sign_template_id"),
                "booking" => str_param("status"),
                _ => None,
            };
            let rows: Vec<(Uuid, DateTime<Utc>)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT x.item_id, x.at FROM ({q}) AS x(item_id, at) JOIN crm_items i ON i.id = x.item_id
                 WHERE i.category_id = $1 AND i.deleted_at IS NULL AND x.at IS NOT NULL ORDER BY x.at DESC"
            )))
            .bind(b.category_id)
            .bind(since)
            .bind(param)
            .fetch_all(&state.db)
            .await?;
            rows
        }
        _ => return Err(AppError::bad("kind is robot or trigger")),
    };
    // Conditions, once per record (they are read as the record is now).
    let mut verdict: HashMap<Uuid, bool> = HashMap::new();
    let mut checked = 0;
    for (item, _) in &hits {
        if verdict.contains_key(item) {
            continue;
        }
        if checked >= CAP {
            break;
        }
        checked += 1;
        let ok = if b.conditions.as_array().is_none_or(|a| a.is_empty()) {
            true
        } else {
            match context(&state, *item).await {
                Ok(c) => conditions_hold(&b.conditions, &c.value),
                Err(_) => false,
            }
        };
        verdict.insert(*item, ok);
    }
    let runs: Vec<&(Uuid, DateTime<Utc>)> = hits.iter().filter(|h| verdict.get(&h.0).copied().unwrap_or(false)).collect();
    let records: BTreeSet<Uuid> = runs.iter().map(|h| h.0).collect();
    let sample_ids: Vec<Uuid> = runs.iter().map(|h| h.0).fold(vec![], |mut v, id| {
        if !v.contains(&id) && v.len() < 10 {
            v.push(id);
        }
        v
    });
    let sample: Vec<(Uuid, String, String)> = sqlx::query!(r#"SELECT i.id, i.title, t.code FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type WHERE i.id = ANY($1)"#, &sample_ids).map(|r| (r.id, r.title, r.code))
    .fetch_all(&state.db)
    .await?;
    // Per week, oldest first, for a small chart.
    let mut weeks: Vec<i64> = vec![0; (days as usize).div_ceil(7)];
    for (_, at) in &runs {
        let w = ((Utc::now() - *at).num_days() / 7) as usize;
        if w < weeks.len() {
            let n = weeks.len();
            weeks[n - 1 - w] += 1;
        }
    }
    Ok(Json(json!({
        "days": days,
        "considered": hits.len(),
        "would_run": runs.len(),
        "records": records.len(),
        "filtered_out": verdict.values().filter(|v| !**v).count(),
        "per_week": weeks,
        "capped": checked >= CAP,
        "note": if b.conditions.as_array().is_none_or(|a| a.is_empty()) { Value::Null } else { json!("Conditions are checked on the records as they are now.") },
        "sample": sample_ids.iter().filter_map(|id| sample.iter().find(|s| s.0 == *id)).map(|(id, t, c)| json!({ "id": id, "title": t, "link": format!("/crm/{c}/{id}") })).collect::<Vec<_>>(),
    })))
}
