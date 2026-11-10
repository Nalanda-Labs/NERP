//! Recurring records: a deal, invoice or smart-process record that repeats on a schedule
//! (monthly retainers, subscriptions, regular orders).
//!
//! The record set to repeat is the pattern. Each time the schedule comes round, a copy of it
//! as it is then (title, responsible person, client, currency, amount, custom fields,
//! comment, products) starts in the first stage of the same pipeline, with its date set some
//! days ahead, and is listed under the pattern's "Made from this record". Robots of that
//! first stage run as usual, e.g. to send the new invoice. The schedule uses the same rules
//! as recurring tasks: daily, weekly or monthly at a local time, with an optional end.

use anyhow::Result;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::{
    access::require,
    automation::system_auth,
    broadcast, entity_type_by_id,
    items::{create_item, load_row},
    money::totals,
    timeline, EntityType, COMPANY, CONTACT, LEAD,
};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    state::AppState,
    tasks::recurrence::Recurrence,
};

pub const JOB: &str = "crm.recurring";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/items/{id}/recurring", get(get_rule).put(put_rule).delete(delete_rule))
        .route("/crm/items/{id}/recurring/now", post(run_now))
}

/// "Create one now": an extra copy, outside the schedule (which stays as it is).
async fn run_now(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = load_row(&state, id, false).await?;
    require(&state, &auth, row.entity_type, "add", None).await?;
    let (rule, due_days): (Value, Option<i32>) = sqlx::query_as("SELECT rule, due_days FROM crm_recurring WHERE item_id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::bad("this record does not repeat"))?;
    let rule: Recurrence = serde_json::from_value(rule).map_err(|_| AppError::bad("the schedule is damaged; set it again"))?;
    let new = copy(&state, id, due_days, &rule, Some(auth.id)).await?;
    let (number, code): (i64, String) = sqlx::query_as("SELECT i.number, t.code FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type WHERE i.id = $1")
        .bind(new)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(json!({ "id": new, "number": number, "code": code })))
}

/// Contacts, companies and leads describe someone, not something that happens again.
fn can_repeat(t: &EntityType) -> bool {
    ![LEAD, CONTACT, COMPANY].contains(&t.id) && t.has_stages
}

#[derive(sqlx::FromRow)]
struct RuleRow {
    rule: Value,
    due_days: Option<i32>,
    next_run_at: Option<DateTime<Utc>>,
    last_run_at: Option<DateTime<Utc>>,
    run_count: i32,
    is_active: bool,
}

async fn view(state: &AppState, item: Uuid) -> AppResult<Value> {
    let r: Option<RuleRow> = sqlx::query_as("SELECT rule, due_days, next_run_at, last_run_at, run_count, is_active FROM crm_recurring WHERE item_id = $1")
        .bind(item)
        .fetch_optional(&state.db)
        .await?;
    Ok(match r {
        None => json!({ "rule": null }),
        Some(r) => {
            let rule: Option<Recurrence> = serde_json::from_value(r.rule.clone()).ok();
            json!({
                "rule": r.rule,
                "summary": rule.map(|x| x.describe()),
                "due_days": r.due_days,
                "next_run_at": r.next_run_at,
                "last_run_at": r.last_run_at,
                "run_count": r.run_count,
                "is_active": r.is_active,
            })
        }
    })
}

async fn get_rule(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = load_row(&state, id, false).await?;
    super::access::require_read(&state, &auth, row.entity_type, id, row.assigned_to).await?;
    view(&state, id).await.map(Json)
}

#[derive(Deserialize)]
struct RuleInput {
    rule: Recurrence,
    /// The copy's date (due date, close date…) this many days after it is made.
    due_days: Option<i32>,
}

async fn put_rule(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<RuleInput>) -> AppResult<Json<Value>> {
    let row = load_row(&state, id, false).await?;
    require(&state, &auth, row.entity_type, "write", row.assigned_to).await?;
    require(&state, &auth, row.entity_type, "add", None).await?;
    let t = entity_type_by_id(&state, row.entity_type).await?;
    if !can_repeat(&t) {
        return Err(AppError::bad(format!("{} cannot repeat", t.name_plural.to_lowercase())));
    }
    b.rule.validate()?;
    if b.due_days.is_some_and(|d| !(0..=3650).contains(&d)) {
        return Err(AppError::bad("days until the date must be between 0 and 3650"));
    }
    let next = b.rule.next_after(Utc::now()).ok_or_else(|| AppError::bad("this schedule never comes round again"))?;
    // A changed schedule starts counting again.
    sqlx::query(
        "INSERT INTO crm_recurring (item_id, rule, due_days, next_run_at, is_active, created_by)
         VALUES ($1, $2, $3, $4, TRUE, $5)
         ON CONFLICT (item_id) DO UPDATE SET rule = $2, due_days = $3, next_run_at = $4, is_active = TRUE, run_count = 0, updated_at = now()",
    )
    .bind(id)
    .bind(serde_json::to_value(&b.rule).unwrap())
    .bind(b.due_days)
    .bind(next)
    .bind(auth.id)
    .execute(&state.db)
    .await?;
    timeline(&state.db, id, "recurring", Some(auth.id), None, json!({ "event": "set", "summary": b.rule.describe() })).await?;
    broadcast(&state, t.id, id, false).await;
    view(&state, id).await.map(Json)
}

async fn delete_rule(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = load_row(&state, id, false).await?;
    require(&state, &auth, row.entity_type, "write", row.assigned_to).await?;
    let n = sqlx::query("DELETE FROM crm_recurring WHERE item_id = $1").bind(id).execute(&state.db).await?.rows_affected();
    if n > 0 {
        timeline(&state.db, id, "recurring", Some(auth.id), None, json!({ "event": "stopped" })).await?;
        broadcast(&state, row.entity_type, id, false).await;
    }
    view(&state, id).await.map(Json)
}

// ---------------------------------------------------------------------------
// The job
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct Due {
    item_id: Uuid,
    rule: Value,
    due_days: Option<i32>,
    next_run_at: Option<DateTime<Utc>>,
    run_count: i32,
    created_by: Option<Uuid>,
}

/// Every minute: makes the copies that are due.
pub async fn run_due(state: &AppState) -> Result<()> {
    let due: Vec<Due> = sqlx::query_as(
        "SELECT r.item_id, r.rule, r.due_days, r.next_run_at, r.run_count, r.created_by
         FROM crm_recurring r JOIN crm_items i ON i.id = r.item_id
         WHERE r.is_active AND r.next_run_at <= now() AND i.deleted_at IS NULL ORDER BY r.next_run_at LIMIT 100",
    )
    .fetch_all(&state.db)
    .await?;
    for d in due {
        let Ok(rule) = serde_json::from_value::<Recurrence>(d.rule.clone()) else { continue };
        let scheduled = d.next_run_at.unwrap_or_else(Utc::now);
        // After downtime: one copy for the missed time, then on from now.
        let next = rule.next_after(Utc::now().max(scheduled));
        let runs = d.run_count + 1;
        let finished = next.is_none() || rule.max_runs.is_some_and(|m| runs >= m);
        let claimed = sqlx::query(
            "UPDATE crm_recurring SET next_run_at = $1, last_run_at = now(), run_count = run_count + 1, is_active = $2, updated_at = now()
             WHERE item_id = $3 AND next_run_at = $4",
        )
        .bind(if finished { None } else { next })
        .bind(!finished)
        .bind(d.item_id)
        .bind(scheduled)
        .execute(&state.db)
        .await?
        .rows_affected();
        if claimed == 0 {
            continue;
        }
        if let Err(e) = copy(state, d.item_id, d.due_days, &rule, None).await {
            tracing::warn!(item = %d.item_id, error = ?e, "recurring record could not be copied");
            let who: Vec<Uuid> = d.created_by.into_iter().collect();
            notify(state, &who, NewNotification {
                kind: "crm.recurring_failed",
                title: "A recurring record could not be repeated".into(),
                body: Some(format!("{e:?}")),
                link: Some(link(state, d.item_id).await),
                actor: None,
            })
            .await;
        }
    }
    Ok(())
}

async fn link(state: &AppState, item: Uuid) -> String {
    let code: Option<String> = sqlx::query_scalar("SELECT t.code FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type WHERE i.id = $1")
        .bind(item)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    format!("/crm/{}/{item}", code.unwrap_or_default())
}

#[derive(sqlx::FromRow)]
struct Pattern {
    entity_type: i16,
    title: String,
    assigned_to: Option<Uuid>,
    assignee_active: Option<bool>,
    company_id: Option<Uuid>,
    currency: Option<String>,
    opportunity: Decimal,
    is_manual_amount: bool,
    category_id: Option<Uuid>,
    uf: Value,
    comments: Option<String>,
}

/// Makes one copy of the pattern record. Returns the new record.
pub async fn copy(state: &AppState, source: Uuid, due_days: Option<i32>, rule: &Recurrence, actor: Option<Uuid>) -> AppResult<Uuid> {
    let p: Pattern = sqlx::query_as(
        "SELECT i.entity_type, i.title, i.assigned_to, u.is_active AS assignee_active, i.company_id, i.currency, i.opportunity,
                i.is_manual_amount, i.category_id, i.uf, i.comments
         FROM crm_items i LEFT JOIN users u ON u.id = i.assigned_to WHERE i.id = $1",
    )
    .bind(source)
    .fetch_one(&state.db)
    .await?;
    let t = entity_type_by_id(state, p.entity_type).await?;
    let contacts: Vec<Uuid> = sqlx::query_scalar("SELECT contact_id FROM crm_item_contacts WHERE item_id = $1 ORDER BY is_primary DESC, sort")
        .bind(source)
        .fetch_all(&state.db)
        .await?;
    let mut m = Map::new();
    m.insert("title".into(), json!(p.title));
    // Someone who left: the copy waits for a new responsible person.
    m.insert("assigned_to".into(), json!(p.assigned_to.filter(|_| p.assignee_active == Some(true))));
    if t.has_client {
        m.insert("company_id".into(), json!(p.company_id));
        m.insert("contact_ids".into(), json!(contacts));
    }
    if t.has_amount {
        m.insert("currency".into(), json!(p.currency));
        m.insert("opportunity".into(), json!(p.opportunity.to_string()));
        m.insert("is_manual_amount".into(), json!(p.is_manual_amount));
    }
    if let Some(c) = p.category_id.filter(|_| t.has_pipelines) {
        m.insert("category_id".into(), json!(c)); // its first open stage
    }
    if let Some(days) = due_days {
        let today = Utc::now().with_timezone(&rule.timezone.parse::<chrono_tz::Tz>().unwrap_or(chrono_tz::UTC)).date_naive();
        m.insert("close_date".into(), json!((today + chrono::Duration::days(days as i64)).to_string()));
    }
    if p.uf.as_object().is_some_and(|o| !o.is_empty()) {
        m.insert("uf".into(), p.uf.clone());
    }
    if let Some(c) = p.comments {
        m.insert("comments".into(), json!(c));
    }
    let id = create_item(state, &system_auth(), &t, m, Some(source)).await?;
    sqlx::query(
        "INSERT INTO crm_item_products (item_id, product_id, name, price, quantity, unit, discount_kind, discount_value, tax_rate, tax_included, sort)
         SELECT $1, product_id, name, price, quantity, unit, discount_kind, discount_value, tax_rate, tax_included, sort
         FROM crm_item_products WHERE item_id = $2",
    )
    .bind(id)
    .bind(source)
    .execute(&state.db)
    .await?;
    totals(state, id).await?;
    let number: i64 = sqlx::query_scalar("SELECT number FROM crm_items WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    timeline(&state.db, id, "recurring", actor, None, json!({ "event": "copy", "from": source })).await?;
    timeline(&state.db, source, "recurring", actor, None, json!({ "event": "made", "item_id": id, "number": number, "code": t.code, "manual": actor.is_some() })).await?;
    broadcast(state, t.id, id, false).await;
    broadcast(state, t.id, source, false).await;
    Ok(id)
}
