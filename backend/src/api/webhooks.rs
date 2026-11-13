//! Outgoing event webhooks. Admins subscribe a URL to events; each event becomes a delivery
//! that is POSTed as JSON in the background:
//!
//! ```text
//! POST <url>
//! Content-Type: application/json
//! X-NERP-Event: crm.item.updated
//! X-NERP-Delivery: <delivery id>
//! X-NERP-Signature: sha256=<HMAC-SHA256 of the body with the subscription's secret>
//!
//! { "id": "<delivery id>", "event": "crm.item.updated", "occurred_at": "…", "data": { … } }
//! ```
//!
//! Network errors, 5xx and 429 are retried (after 1, 5, 30, 120 and 360 minutes); other
//! responses end the delivery. Every attempt is kept in the delivery log.

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, tokens, AuthUser},
    error::{AppError, AppResult},
    jobs,
    outbound,
    routes::required,
    state::AppState,
};

pub const JOB: &str = "webhook.deliver";
const RETRY_MINUTES: &[i64] = &[1, 5, 30, 120, 360];

pub const EVENTS: &[(&str, &str)] = &[
    ("crm.item.created", "A CRM record (lead, deal, contact, company, quote, invoice, smart process) was created"),
    ("crm.item.updated", "A CRM record's fields changed (with the changes)"),
    ("crm.item.stage_changed", "A CRM record moved to another stage"),
    ("crm.item.deleted", "A CRM record was moved to the recycle bin"),
    ("crm.activity.created", "An activity (call, meeting, to-do) was planned on a record"),
    ("crm.activity.completed", "An activity was completed"),
    ("task.created", "A task was created"),
    ("task.updated", "A task was changed (title, people, dates, status…)"),
    ("task.completed", "A task was completed"),
    ("task.deleted", "A task was deleted"),
    ("hr.document.signed", "An HR document was signed by everyone"),
    ("hr.document.revoked", "An HR document was withdrawn or declared invalid"),
];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/webhooks", get(list).post(create))
        .route("/webhooks/events", get(events))
        .route("/webhooks/{id}", patch(update).delete(remove))
        .route("/webhooks/{id}/deliveries", get(deliveries))
        .route("/webhooks/{id}/test", post(test))
        .route("/webhook-deliveries/{id}/redeliver", post(redeliver))
}

// ---------------------------------------------------------------------------
// Emitting and delivering
// ---------------------------------------------------------------------------

/// What webhooks get about a CRM record.
pub async fn crm_item_data(state: &AppState, id: Uuid) -> Option<Value> {
    let r = crate::crm::items::load_row(state, id, true).await.ok()?;
    let t = crate::crm::entity_type_by_id(state, r.entity_type).await.ok()?;
    Some(json!({
        "type": t.code, "id": r.id, "number": r.number, "title": r.title,
        "category_id": r.category_id, "stage_id": r.stage_id, "stage": r.stage_name, "stage_semantics": r.stage_semantics,
        "assigned_to": r.assigned_to, "company_id": r.company_id, "opportunity": r.opportunity, "currency": r.currency,
        "url": format!("{}/crm/{}/{}", state.config.public_url, t.code, r.id),
    }))
}

pub async fn emit_crm_item(state: &AppState, event: &str, id: Uuid, extra: Value) {
    if let Some(mut data) = crm_item_data(state, id).await {
        if let (Value::Object(d), Value::Object(e)) = (&mut data, extra) {
            d.extend(e);
        }
        emit(state, event, data).await;
    }
}

/// Queues `event` for every active subscription to it. Never fails the caller.
pub async fn emit(state: &AppState, event: &str, data: Value) {
    if let Err(e) = try_emit(state, event, data).await {
        tracing::warn!(event, error = ?e, "could not queue webhook deliveries");
    }
}

async fn try_emit(state: &AppState, event: &str, data: Value) -> AppResult<usize> {
    let subs: Vec<Uuid> = sqlx::query_scalar!("SELECT id FROM webhook_subscriptions WHERE is_active AND $1 = ANY(events)", event)
    .fetch_all(&state.db)
    .await?;
    for sub in &subs {
        queue(state, *sub, event, &data).await?;
    }
    if !subs.is_empty() {
        state.job_wakeup.notify_one();
    }
    Ok(subs.len())
}

async fn queue(state: &AppState, sub: Uuid, event: &str, data: &Value) -> AppResult<Uuid> {
    let id = Uuid::new_v4();
    let payload = json!({ "id": id, "event": event, "occurred_at": Utc::now(), "data": data });
    sqlx::query!("INSERT INTO webhook_deliveries (id, subscription_id, event, payload, next_attempt_at) VALUES ($1, $2, $3, $4, now())", id, sub, event, &payload)
        .execute(&state.db)
        .await?;
    jobs::enqueue(&state.db, JOB, json!({ "delivery_id": id }), None).await?;
    Ok(id)
}

/// Background job: one delivery attempt.
pub async fn run_job(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let id: Uuid = serde_json::from_value(payload["delivery_id"].clone())?;
    let row: Option<(String, Value, i32, String, String, bool, String)> = sqlx::query!(r#"SELECT d.status, d.payload, d.attempts, s.url, s.secret, s.is_active, d.event
         FROM webhook_deliveries d JOIN webhook_subscriptions s ON s.id = d.subscription_id WHERE d.id = $1"#, id).map(|r| (r.status, r.payload, r.attempts, r.url, r.secret, r.is_active, r.event))
    .fetch_optional(&state.db)
    .await?;
    let Some((status, body, attempts, url, secret, active, event)) = row else { return Ok(()) };
    if status != "pending" {
        return Ok(());
    }
    if !active {
        sqlx::query!("UPDATE webhook_deliveries SET status = 'failed', error = 'the webhook was turned off', next_attempt_at = NULL WHERE id = $1", id).execute(&state.db).await?;
        return Ok(());
    }
    let attempt = attempts + 1;
    let result = match outbound::check_url(&url) {
        Ok(u) => {
            outbound::send(state, outbound::Request {
                method: reqwest::Method::POST,
                url: u,
                headers: vec![("X-NERP-Event".into(), event.clone()), ("X-NERP-Delivery".into(), id.to_string())],
                body: Some((body.to_string(), "application/json")),
                secret: Some(&secret),
                timeout_secs: 10,
            })
            .await
        }
        Err(e) => Err(outbound::Failure { msg: e, retry: false, status: None, snippet: None }),
    };
    match result {
        Ok(r) => {
            sqlx::query!("UPDATE webhook_deliveries SET status = 'done', attempts = $2, response_status = $3, response_body = $4, error = NULL, delivered_at = now(), next_attempt_at = NULL WHERE id = $1", id, attempt, r.status as i32, r.snippet)
                .execute(&state.db)
                .await?;
        }
        Err(f) => {
            let next = (f.retry && (attempt as usize) <= RETRY_MINUTES.len()).then(|| Utc::now() + Duration::minutes(RETRY_MINUTES[attempt as usize - 1]));
            sqlx::query!("UPDATE webhook_deliveries SET status = $2, attempts = $3, response_status = $4, response_body = $5, error = $6, next_attempt_at = $7 WHERE id = $1", id, if next.is_some() { "pending" } else { "failed" }, attempt, f.status.map(|s| s as i32), f.snippet, f.msg, next)
                .execute(&state.db)
                .await?;
            if let Some(at) = next {
                jobs::enqueue(&state.db, JOB, json!({ "delivery_id": id }), Some(at)).await?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Subscriptions (needs "Manage webhooks")
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct Subscription {
    id: Uuid,
    name: String,
    url: String,
    secret: String,
    events: Vec<String>,
    is_active: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    delivered_24h: i64,
    failed_24h: i64,
    pending: i64,
    last_delivery_at: Option<DateTime<Utc>>,
}

const SUB_SELECT: &str = "SELECT s.id, s.name, s.url, s.secret, s.events, s.is_active, s.created_at, s.updated_at,
    (SELECT COUNT(*) FROM webhook_deliveries d WHERE d.subscription_id = s.id AND d.status = 'done' AND d.created_at > now() - interval '1 day')::int8 AS delivered_24h,
    (SELECT COUNT(*) FROM webhook_deliveries d WHERE d.subscription_id = s.id AND d.status = 'failed' AND d.created_at > now() - interval '1 day')::int8 AS failed_24h,
    (SELECT COUNT(*) FROM webhook_deliveries d WHERE d.subscription_id = s.id AND d.status = 'pending')::int8 AS pending,
    (SELECT MAX(d.created_at) FROM webhook_deliveries d WHERE d.subscription_id = s.id) AS last_delivery_at
    FROM webhook_subscriptions s";

async fn load(state: &AppState, id: Uuid) -> AppResult<Subscription> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SUB_SELECT} WHERE s.id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("webhook"))
}

async fn events(auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::WEBHOOKS_MANAGE)?;
    Ok(Json(json!({ "items": EVENTS.iter().map(|(code, label)| json!({ "code": code, "label": label })).collect::<Vec<_>>() })))
}

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::WEBHOOKS_MANAGE)?;
    let items: Vec<Subscription> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SUB_SELECT} ORDER BY s.created_at"))).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": items })))
}

#[derive(Deserialize)]
struct Body {
    name: Option<String>,
    url: Option<String>,
    events: Option<Vec<String>>,
    is_active: Option<bool>,
    /// Replace the signing secret.
    rotate_secret: Option<bool>,
}

fn check_events(list: &[String]) -> AppResult<Vec<String>> {
    let mut l = list.to_vec();
    l.sort();
    l.dedup();
    if l.is_empty() || l.iter().any(|e| !EVENTS.iter().any(|(c, _)| c == e)) {
        return Err(AppError::bad("choose events from the list"));
    }
    Ok(l)
}

fn check_webhook_url(url: &str) -> AppResult<String> {
    let u = required("URL", url, 2000)?;
    outbound::check_url(&u).map_err(AppError::bad)?;
    Ok(u)
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<Body>) -> AppResult<Json<Subscription>> {
    auth.require(perm::WEBHOOKS_MANAGE)?;
    let n: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM webhook_subscriptions"#).fetch_one(&state.db).await?;
    if n >= 50 {
        return Err(AppError::bad("at most 50 webhooks"));
    }
    let id: Uuid = sqlx::query_scalar!("INSERT INTO webhook_subscriptions (name, url, secret, events, is_active, created_by) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id", required("name", b.name.as_deref().unwrap_or(""), 100)?, check_webhook_url(b.url.as_deref().unwrap_or(""))?, tokens::random_token(24), &(check_events(&b.events.unwrap_or_default())?), b.is_active.unwrap_or(true), auth.id)
        .fetch_one(&state.db)
        .await?;
    crate::audit::record(&state.db, Some(auth.id), "webhook.create", "webhook", Some(id.to_string()), json!({}), None).await;
    Ok(Json(load(&state, id).await?))
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Body>) -> AppResult<Json<Subscription>> {
    auth.require(perm::WEBHOOKS_MANAGE)?;
    let cur = load(&state, id).await?;
    sqlx::query!("UPDATE webhook_subscriptions SET name = $2, url = $3, events = $4, is_active = $5, secret = $6, updated_at = now() WHERE id = $1", id, match &b.name { Some(n) => required("name", n, 100)?, None => cur.name }, match &b.url { Some(u) => check_webhook_url(u)?, None => cur.url }, &(match &b.events { Some(e) => check_events(e)?, None => cur.events }), b.is_active.unwrap_or(cur.is_active), if b.rotate_secret == Some(true) { tokens::random_token(24) } else { cur.secret })
        .execute(&state.db)
        .await?;
    Ok(Json(load(&state, id).await?))
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    auth.require(perm::WEBHOOKS_MANAGE)?;
    let n = sqlx::query!("DELETE FROM webhook_subscriptions WHERE id = $1", id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("webhook"));
    }
    crate::audit::record(&state.db, Some(auth.id), "webhook.delete", "webhook", Some(id.to_string()), json!({}), None).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Serialize, FromRow)]
struct Delivery {
    id: Uuid,
    event: String,
    status: String,
    attempts: i32,
    response_status: Option<i32>,
    response_body: Option<String>,
    error: Option<String>,
    next_attempt_at: Option<DateTime<Utc>>,
    delivered_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    payload: Value,
}

#[derive(Deserialize)]
struct DeliveriesQuery {
    status: Option<String>,
    limit: Option<i64>,
}

async fn deliveries(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<DeliveriesQuery>) -> AppResult<Json<Value>> {
    auth.require(perm::WEBHOOKS_MANAGE)?;
    load(&state, id).await?;
    let items: Vec<Delivery> = sqlx::query_as!(Delivery, r#"SELECT id, event, status, attempts, response_status AS "response_status?", response_body AS "response_body?", error AS "error?", next_attempt_at AS "next_attempt_at?", delivered_at AS "delivered_at?", created_at, payload
         FROM webhook_deliveries WHERE subscription_id = $1 AND ($2::text IS NULL OR status = $2) ORDER BY created_at DESC LIMIT $3"#, id, q.status.filter(|s| !s.is_empty()), q.limit.unwrap_or(50).clamp(1, 200))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items })))
}

/// Sends a `ping` event to this webhook only.
async fn test(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    auth.require(perm::WEBHOOKS_MANAGE)?;
    let sub = load(&state, id).await?;
    if !sub.is_active {
        return Err(AppError::bad("turn the webhook on first"));
    }
    let delivery = queue(&state, id, "ping", &json!({ "message": "Test from NERP", "webhook": sub.name })).await?;
    state.job_wakeup.notify_one();
    Ok(Json(json!({ "delivery_id": delivery })))
}

/// Sends a delivery's payload again, as a new delivery.
async fn redeliver(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    auth.require(perm::WEBHOOKS_MANAGE)?;
    let row: Option<(Uuid, String, Value)> = sqlx::query!(r#"SELECT subscription_id, event, payload FROM webhook_deliveries WHERE id = $1"#, id).map(|r| (r.subscription_id, r.event, r.payload)).fetch_optional(&state.db).await?;
    let Some((sub, event, payload)) = row else { return Err(AppError::NotFound("delivery")) };
    let new = queue(&state, sub, &event, &payload["data"]).await?;
    state.job_wakeup.notify_one();
    Ok(Json(json!({ "delivery_id": new })))
}
