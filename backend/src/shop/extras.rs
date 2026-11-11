//! What changes an order's total: coupons (a percentage or an amount off; dates, a minimum
//! order, a number of uses, once per customer), discounts for loyal customers (by what they
//! bought before, by email), and ways of delivery (a price, free above an amount, with or
//! without an address). When a coupon and a loyalty discount both apply, the larger one
//! counts. Also: the shop's set-up (not its catalogue) exported to a file and imported again.

use axum::{
    extract::{Path, State},
    routing::{get, patch},
    Json, Router,
};
use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::require;
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/shop/coupons", get(coupons).post(new_coupon))
        .route("/shop/coupons/{id}", patch(change_coupon).delete(drop_coupon))
        .route("/shop/delivery", get(deliveries).post(new_delivery))
        .route("/shop/delivery/{id}", patch(change_delivery).delete(drop_delivery))
        .route("/shop/loyalty", get(get_loyalty).put(put_loyalty))
        .route("/shop/export", get(export))
        .route("/shop/import", axum::routing::post(import))
        .route("/public/shop/delivery", get(public_delivery))
}

fn round2(d: Decimal) -> Decimal {
    d.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

#[derive(Serialize, Default)]
pub struct Quote {
    pub subtotal: Decimal,
    pub discount_percent: Decimal,
    pub discount: Decimal,
    /// "Coupon SPRING" or "Loyal customer".
    pub discount_reason: Option<String>,
    #[serde(skip)]
    pub coupon_id: Option<Uuid>,
    pub coupon: Option<String>,
    pub delivery: Option<Value>,
    #[serde(skip)]
    pub delivery_name: Option<String>,
    pub delivery_price: Decimal,
    pub needs_address: bool,
    pub total: Decimal,
}

/// Works out discounts and delivery for a cart worth `subtotal` (tax included).
pub async fn price(state: &AppState, subtotal: Decimal, email: Option<&str>, coupon: Option<&str>, delivery: Option<Uuid>) -> AppResult<Quote> {
    let mut q = Quote { subtotal, needs_address: true, ..Default::default() };
    // Loyalty: what this email bought before (orders not cancelled).
    if let Some(email) = email.map(|e| e.trim().to_lowercase()).filter(|e| e.contains('@')) {
        let tiers: Value = sqlx::query_scalar("SELECT loyalty FROM shop_settings").fetch_one(&state.db).await?;
        if tiers.as_array().is_some_and(|t| !t.is_empty()) {
            let before: Decimal = sqlx::query_scalar(
                "SELECT COALESCE(SUM(i.opportunity), 0) FROM shop_orders o JOIN crm_items i ON i.id = o.item_id
                 WHERE lower(o.email) = $1 AND i.deleted_at IS NULL AND i.stage_semantics <> 'failure'",
            )
            .bind(&email)
            .fetch_one(&state.db)
            .await?;
            let pct = tiers
                .as_array()
                .unwrap()
                .iter()
                .filter(|t| t["from"].as_f64().and_then(Decimal::from_f64_retain).is_some_and(|f| before >= f))
                .filter_map(|t| t["percent"].as_f64().and_then(Decimal::from_f64_retain))
                .max()
                .unwrap_or_default();
            if pct > Decimal::ZERO {
                q.discount_percent = pct;
                q.discount_reason = Some("Loyal customer".into());
            }
        }
    }
    // A coupon.
    if let Some(code) = coupon.map(|c| c.trim().to_uppercase()).filter(|c| !c.is_empty()) {
        type C = (Uuid, String, Decimal, Option<Decimal>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<i32>, i32, bool, bool);
        let c: Option<C> = sqlx::query_as("SELECT id, kind, value, min_total, starts_at, ends_at, max_uses, uses, once_per_customer, active FROM shop_coupons WHERE code = $1")
            .bind(&code)
            .fetch_optional(&state.db)
            .await?;
        let (id, kind, value, min_total, starts, ends, max, uses, once, active) = c.filter(|c| c.9).ok_or_else(|| AppError::bad(format!("there is no coupon “{code}”")))?;
        let _ = active;
        let now = Utc::now();
        if starts.is_some_and(|s| s > now) {
            return Err(AppError::bad(format!("coupon “{code}” is not valid yet")));
        }
        if ends.is_some_and(|e| e <= now) {
            return Err(AppError::bad(format!("coupon “{code}” has expired")));
        }
        if max.is_some_and(|m| uses >= m) {
            return Err(AppError::bad(format!("coupon “{code}” has been used up")));
        }
        if let Some(m) = min_total.filter(|m| subtotal < *m) {
            return Err(AppError::bad(format!("coupon “{code}” needs an order of at least {}", m.normalize())));
        }
        if once {
            if let Some(email) = email.map(|e| e.trim().to_lowercase()) {
                let used: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM shop_coupon_uses WHERE coupon_id = $1 AND email = $2)").bind(id).bind(&email).fetch_one(&state.db).await?;
                if used {
                    return Err(AppError::bad(format!("you have already used coupon “{code}”")));
                }
            }
        }
        let pct = if kind == "percent" {
            value.min(Decimal::ONE_HUNDRED)
        } else if subtotal > Decimal::ZERO {
            (value / subtotal * Decimal::ONE_HUNDRED).min(Decimal::ONE_HUNDRED).round_dp(4)
        } else {
            Decimal::ZERO
        };
        q.coupon = Some(code.clone());
        q.coupon_id = Some(id);
        // The larger discount counts.
        if pct >= q.discount_percent {
            q.discount_percent = pct;
            q.discount_reason = Some(format!("Coupon {code}"));
        }
    }
    q.discount = round2(subtotal * q.discount_percent / Decimal::ONE_HUNDRED);
    let after = subtotal - q.discount;
    // Delivery: one of the ways, when the shop has any.
    let ways: Vec<(Uuid, String, Decimal, Option<Decimal>, bool)> =
        sqlx::query_as("SELECT id, name, price, free_from, needs_address FROM shop_delivery WHERE active ORDER BY sort, name").fetch_all(&state.db).await?;
    if !ways.is_empty() {
        let (id, name, price, free, needs) = match delivery {
            Some(d) => ways.into_iter().find(|w| w.0 == d).ok_or_else(|| AppError::bad("choose a way of delivery"))?,
            None => return Ok(Quote { total: after, ..q }),
        };
        q.delivery_price = if free.is_some_and(|f| after >= f) { Decimal::ZERO } else { price };
        q.delivery = Some(json!({ "id": id, "name": name, "price": q.delivery_price }));
        q.delivery_name = Some(name);
        q.needs_address = needs;
    }
    q.total = after + q.delivery_price;
    Ok(q)
}

/// Whether the shop has ways of delivery (then a way must be chosen).
pub async fn has_delivery(state: &AppState) -> AppResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM shop_delivery WHERE active)").fetch_one(&state.db).await?)
}

/// Takes one use of a coupon (all or nothing), for this email.
pub async fn use_coupon(state: &AppState, id: Uuid, email: &str) -> AppResult<()> {
    let email = email.trim().to_lowercase();
    let mut tx = state.db.begin().await?;
    // Orders from one customer at the same moment take turns, so "once per customer" holds.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('coupon:' || $1::text || ':' || $2, 0))").bind(id).bind(&email).execute(&mut *tx).await?;
    let once: bool = sqlx::query_scalar("SELECT once_per_customer FROM shop_coupons WHERE id = $1").bind(id).fetch_one(&mut *tx).await?;
    if once {
        let used: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM shop_coupon_uses WHERE coupon_id = $1 AND email = $2)").bind(id).bind(&email).fetch_one(&mut *tx).await?;
        if used {
            return Err(AppError::bad("you have already used this coupon"));
        }
    }
    let n = sqlx::query("UPDATE shop_coupons SET uses = uses + 1 WHERE id = $1 AND (max_uses IS NULL OR uses < max_uses)").bind(id).execute(&mut *tx).await?.rows_affected();
    if n == 0 {
        return Err(AppError::bad("the coupon has just been used up"));
    }
    sqlx::query("INSERT INTO shop_coupon_uses (coupon_id, email) VALUES ($1, $2)").bind(id).bind(&email).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Gives a use back when the order did not go through.
pub async fn unuse_coupon(state: &AppState, id: Uuid, email: &str) {
    let _ = sqlx::query("UPDATE shop_coupons SET uses = GREATEST(uses - 1, 0) WHERE id = $1").bind(id).execute(&state.db).await;
    let _ = sqlx::query("DELETE FROM shop_coupon_uses WHERE ctid IN (SELECT ctid FROM shop_coupon_uses WHERE coupon_id = $1 AND email = $2 AND item_id IS NULL LIMIT 1)")
        .bind(id)
        .bind(email.to_lowercase())
        .execute(&state.db)
        .await;
}

// ---------------------------------------------------------------------------
// Coupons
// ---------------------------------------------------------------------------

type CouponRow = (Uuid, String, String, Decimal, Option<Decimal>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, Option<i32>, i32, bool, bool, DateTime<Utc>);
const COUPON_COLS: &str = "id, code, kind, value, min_total, starts_at, ends_at, max_uses, uses, once_per_customer, active, created_at";

fn coupon_json(r: CouponRow) -> Value {
    let (id, code, kind, value, min, starts, ends, max, uses, once, active, at) = r;
    json!({ "id": id, "code": code, "kind": kind, "value": value, "min_total": min, "starts_at": starts, "ends_at": ends, "max_uses": max, "uses": uses, "once_per_customer": once, "active": active, "created_at": at })
}

async fn coupons(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require(&auth)?;
    let rows: Vec<CouponRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COUPON_COLS} FROM shop_coupons ORDER BY created_at DESC"))).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.into_iter().map(coupon_json).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct CouponInput {
    code: Option<String>,
    kind: Option<String>,
    value: Option<Decimal>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    min_total: Option<Option<Decimal>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    starts_at: Option<Option<DateTime<Utc>>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    ends_at: Option<Option<DateTime<Utc>>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    max_uses: Option<Option<i32>>,
    once_per_customer: Option<bool>,
    active: Option<bool>,
}

fn check_coupon(kind: &str, value: Decimal, max: Option<i32>, min: Option<Decimal>) -> AppResult<()> {
    if !["percent", "amount"].contains(&kind) {
        return Err(AppError::bad("a coupon takes a percentage or an amount off"));
    }
    if value <= Decimal::ZERO || (kind == "percent" && value > Decimal::ONE_HUNDRED) {
        return Err(AppError::bad("a percentage is above 0 and at most 100; an amount above 0"));
    }
    if max.is_some_and(|m| m < 1) || min.is_some_and(|m| m < Decimal::ZERO) {
        return Err(AppError::bad("uses are at least 1 and the minimum order at least 0"));
    }
    Ok(())
}

async fn new_coupon(State(state): State<AppState>, auth: AuthUser, Json(b): Json<CouponInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let code = required("code", b.code.as_deref().unwrap_or(""), 40)?.to_uppercase();
    if !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(AppError::bad("a code is letters, digits, - and _"));
    }
    let kind = b.kind.unwrap_or_else(|| "percent".into());
    let value = b.value.ok_or_else(|| AppError::bad("how much off?"))?;
    check_coupon(&kind, value, b.max_uses.flatten(), b.min_total.flatten())?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO shop_coupons (code, kind, value, min_total, starts_at, ends_at, max_uses, once_per_customer, active) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
    )
    .bind(&code)
    .bind(&kind)
    .bind(value)
    .bind(b.min_total.flatten())
    .bind(b.starts_at.flatten())
    .bind(b.ends_at.flatten())
    .bind(b.max_uses.flatten())
    .bind(b.once_per_customer.unwrap_or(false))
    .bind(b.active.unwrap_or(true))
    .fetch_one(&state.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => AppError::bad(format!("there is already a coupon “{code}”")),
        e => e.into(),
    })?;
    let r: CouponRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COUPON_COLS} FROM shop_coupons WHERE id = $1"))).bind(id).fetch_one(&state.db).await?;
    Ok(Json(coupon_json(r)))
}

async fn change_coupon(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<CouponInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let old: CouponRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COUPON_COLS} FROM shop_coupons WHERE id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("coupon"))?;
    let kind = b.kind.unwrap_or(old.2);
    let value = b.value.unwrap_or(old.3);
    let min = b.min_total.unwrap_or(old.4);
    let max = b.max_uses.unwrap_or(old.7);
    check_coupon(&kind, value, max, min)?;
    sqlx::query(
        "UPDATE shop_coupons SET kind = $2, value = $3, min_total = $4, starts_at = $5, ends_at = $6, max_uses = $7, once_per_customer = $8, active = $9 WHERE id = $1",
    )
    .bind(id)
    .bind(kind)
    .bind(value)
    .bind(min)
    .bind(b.starts_at.unwrap_or(old.5))
    .bind(b.ends_at.unwrap_or(old.6))
    .bind(max)
    .bind(b.once_per_customer.unwrap_or(old.9))
    .bind(b.active.unwrap_or(old.10))
    .execute(&state.db)
    .await?;
    let r: CouponRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COUPON_COLS} FROM shop_coupons WHERE id = $1"))).bind(id).fetch_one(&state.db).await?;
    Ok(Json(coupon_json(r)))
}

async fn drop_coupon(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    sqlx::query("DELETE FROM shop_coupons WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

type DeliveryRow = (Uuid, String, Option<String>, Decimal, Option<Decimal>, bool, i32, bool);
const DELIVERY_COLS: &str = "id, name, description, price, free_from, needs_address, sort, active";

fn delivery_json(r: DeliveryRow) -> Value {
    let (id, name, d, price, free, needs, sort, active) = r;
    json!({ "id": id, "name": name, "description": d, "price": price, "free_from": free, "needs_address": needs, "sort": sort, "active": active })
}

async fn deliveries(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require(&auth)?;
    let rows: Vec<DeliveryRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {DELIVERY_COLS} FROM shop_delivery ORDER BY sort, name"))).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.into_iter().map(delivery_json).collect::<Vec<_>>() })))
}

async fn public_delivery(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let rows: Vec<DeliveryRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {DELIVERY_COLS} FROM shop_delivery WHERE active ORDER BY sort, name"))).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.into_iter().map(delivery_json).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct DeliveryInput {
    name: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    price: Option<Decimal>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    free_from: Option<Option<Decimal>>,
    needs_address: Option<bool>,
    sort: Option<i32>,
    active: Option<bool>,
}

async fn new_delivery(State(state): State<AppState>, auth: AuthUser, Json(b): Json<DeliveryInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 120)?;
    let price = b.price.unwrap_or_default();
    if price < Decimal::ZERO || b.free_from.flatten().is_some_and(|f| f < Decimal::ZERO) {
        return Err(AppError::bad("prices are 0 or more"));
    }
    sqlx::query("INSERT INTO shop_delivery (name, description, price, free_from, needs_address, sort, active) VALUES ($1, $2, $3, $4, $5, $6, $7)")
        .bind(name)
        .bind(b.description.flatten().map(|d| d.chars().take(500).collect::<String>()))
        .bind(price)
        .bind(b.free_from.flatten())
        .bind(b.needs_address.unwrap_or(true))
        .bind(b.sort.unwrap_or(0))
        .bind(b.active.unwrap_or(true))
        .execute(&state.db)
        .await?;
    deliveries(State(state), auth).await
}

async fn change_delivery(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<DeliveryInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let o: DeliveryRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {DELIVERY_COLS} FROM shop_delivery WHERE id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("delivery"))?;
    let name = match &b.name {
        Some(n) => required("name", n, 120)?,
        None => o.1,
    };
    let price = b.price.unwrap_or(o.3);
    let free = b.free_from.unwrap_or(o.4);
    if price < Decimal::ZERO || free.is_some_and(|f| f < Decimal::ZERO) {
        return Err(AppError::bad("prices are 0 or more"));
    }
    sqlx::query("UPDATE shop_delivery SET name = $2, description = $3, price = $4, free_from = $5, needs_address = $6, sort = $7, active = $8 WHERE id = $1")
        .bind(id)
        .bind(name)
        .bind(b.description.unwrap_or(o.2))
        .bind(price)
        .bind(free)
        .bind(b.needs_address.unwrap_or(o.5))
        .bind(b.sort.unwrap_or(o.6))
        .bind(b.active.unwrap_or(o.7))
        .execute(&state.db)
        .await?;
    deliveries(State(state), auth).await
}

async fn drop_delivery(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    sqlx::query("DELETE FROM shop_delivery WHERE id = $1").bind(id).execute(&state.db).await?;
    deliveries(State(state), auth).await
}

// ---------------------------------------------------------------------------
// Loyalty
// ---------------------------------------------------------------------------

async fn get_loyalty(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require(&auth)?;
    let v: Value = sqlx::query_scalar("SELECT loyalty FROM shop_settings").fetch_one(&state.db).await?;
    Ok(Json(json!({ "tiers": v })))
}

#[derive(Deserialize)]
struct Tier {
    from: f64,
    percent: f64,
}

#[derive(Deserialize)]
struct LoyaltyInput {
    tiers: Vec<Tier>,
}

async fn put_loyalty(State(state): State<AppState>, auth: AuthUser, Json(b): Json<LoyaltyInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    if b.tiers.len() > 10 || b.tiers.iter().any(|t| !(t.from >= 0.0 && t.from.is_finite()) || !(t.percent > 0.0 && t.percent <= 100.0)) {
        return Err(AppError::bad("up to 10 levels, each from an amount of 0 or more, for 0–100%"));
    }
    let mut tiers: Vec<Value> = b.tiers.iter().map(|t| json!({ "from": t.from, "percent": t.percent })).collect();
    tiers.sort_by(|a, b| a["from"].as_f64().partial_cmp(&b["from"].as_f64()).unwrap_or(std::cmp::Ordering::Equal));
    sqlx::query("UPDATE shop_settings SET loyalty = $1").bind(json!(tiers)).execute(&state.db).await?;
    get_loyalty(State(state), auth).await
}

// ---------------------------------------------------------------------------
// Export and import (the set-up, not the catalogue)
// ---------------------------------------------------------------------------

const FORMAT: &str = "nerp-shop/1";

async fn export(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require(&auth)?;
    let s = super::settings(&state).await?;
    let loyalty: Value = sqlx::query_scalar("SELECT loyalty FROM shop_settings").fetch_one(&state.db).await?;
    let cats: Vec<(String, String, i32)> = sqlx::query_as("SELECT name, slug, sort FROM shop_categories ORDER BY sort, name").fetch_all(&state.db).await?;
    let del: Vec<DeliveryRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {DELIVERY_COLS} FROM shop_delivery ORDER BY sort, name"))).fetch_all(&state.db).await?;
    let cps: Vec<CouponRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COUPON_COLS} FROM shop_coupons ORDER BY code"))).fetch_all(&state.db).await?;
    Ok(Json(json!({
        "format": FORMAT,
        "settings": { "name": s.name, "intro": s.intro, "currency": s.currency, "payment_instructions": s.payment_instructions, "terms": s.terms },
        "loyalty": loyalty,
        "categories": cats.into_iter().map(|(n, sl, so)| json!({ "name": n, "slug": sl, "sort": so })).collect::<Vec<_>>(),
        "delivery": del.into_iter().map(|r| { let mut v = delivery_json(r); v.as_object_mut().map(|o| o.remove("id")); v }).collect::<Vec<_>>(),
        "coupons": cps.into_iter().map(|r| { let mut v = coupon_json(r); if let Some(o) = v.as_object_mut() { for k in ["id", "uses", "created_at"] { o.remove(k); } } v }).collect::<Vec<_>>(),
    })))
}

/// Sets the shop up from an exported file: texts, loyalty, categories, delivery and coupons
/// (added or updated by name and code; nothing is deleted). The shop stays closed or open as
/// it is.
async fn import(State(state): State<AppState>, auth: AuthUser, Json(d): Json<Value>) -> AppResult<Json<Value>> {
    require(&auth)?;
    if d["format"] != FORMAT {
        return Err(AppError::bad("this is not a NERP shop file"));
    }
    let st = &d["settings"];
    let text = |k: &str, max: usize| st[k].as_str().map(|s| s.chars().take(max).collect::<String>());
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE shop_settings SET name = COALESCE($1, name), intro = $2, payment_instructions = COALESCE($3, payment_instructions), terms = $4, updated_at = now()")
        .bind(text("name", 120))
        .bind(text("intro", 2000))
        .bind(text("payment_instructions", 4000))
        .bind(text("terms", 20000))
        .execute(&mut *tx)
        .await?;
    if d["loyalty"].is_array() {
        sqlx::query("UPDATE shop_settings SET loyalty = $1").bind(&d["loyalty"]).execute(&mut *tx).await?;
    }
    let mut counts = (0, 0, 0);
    for c in d["categories"].as_array().cloned().unwrap_or_default().iter().take(500) {
        let (Some(n), Some(sl)) = (c["name"].as_str(), c["slug"].as_str()) else { continue };
        sqlx::query("INSERT INTO shop_categories (name, slug, sort) VALUES ($1, $2, $3) ON CONFLICT (slug) DO UPDATE SET name = $1, sort = $3")
            .bind(n.chars().take(120).collect::<String>())
            .bind(sl.chars().take(80).collect::<String>())
            .bind(c["sort"].as_i64().unwrap_or(0) as i32)
            .execute(&mut *tx)
            .await?;
        counts.0 += 1;
    }
    for w in d["delivery"].as_array().cloned().unwrap_or_default().iter().take(100) {
        let Some(n) = w["name"].as_str() else { continue };
        let price = w["price"].as_str().and_then(|p| p.parse::<Decimal>().ok()).or_else(|| w["price"].as_f64().and_then(Decimal::from_f64_retain)).unwrap_or_default();
        let free = w["free_from"].as_str().and_then(|p| p.parse::<Decimal>().ok()).or_else(|| w["free_from"].as_f64().and_then(Decimal::from_f64_retain));
        sqlx::query("DELETE FROM shop_delivery WHERE name = $1").bind(n).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO shop_delivery (name, description, price, free_from, needs_address, sort, active) VALUES ($1, $2, $3, $4, $5, $6, $7)")
            .bind(n)
            .bind(w["description"].as_str())
            .bind(price.max(Decimal::ZERO))
            .bind(free)
            .bind(w["needs_address"].as_bool().unwrap_or(true))
            .bind(w["sort"].as_i64().unwrap_or(0) as i32)
            .bind(w["active"].as_bool().unwrap_or(true))
            .execute(&mut *tx)
            .await?;
        counts.1 += 1;
    }
    for c in d["coupons"].as_array().cloned().unwrap_or_default().iter().take(1000) {
        let Some(code) = c["code"].as_str().map(str::to_uppercase) else { continue };
        let num = |k: &str| c[k].as_str().and_then(|p| p.parse::<Decimal>().ok()).or_else(|| c[k].as_f64().and_then(Decimal::from_f64_retain));
        let kind = c["kind"].as_str().unwrap_or("percent");
        let Some(value) = num("value") else { continue };
        if check_coupon(kind, value, c["max_uses"].as_i64().map(|m| m as i32), num("min_total")).is_err() {
            continue;
        }
        sqlx::query(
            "INSERT INTO shop_coupons (code, kind, value, min_total, starts_at, ends_at, max_uses, once_per_customer, active)
             VALUES ($1, $2, $3, $4, $5::timestamptz, $6::timestamptz, $7, $8, $9)
             ON CONFLICT (code) DO UPDATE SET kind = $2, value = $3, min_total = $4, starts_at = $5::timestamptz, ends_at = $6::timestamptz, max_uses = $7, once_per_customer = $8, active = $9",
        )
        .bind(&code)
        .bind(kind)
        .bind(value)
        .bind(num("min_total"))
        .bind(c["starts_at"].as_str())
        .bind(c["ends_at"].as_str())
        .bind(c["max_uses"].as_i64().map(|m| m as i32))
        .bind(c["once_per_customer"].as_bool().unwrap_or(false))
        .bind(c["active"].as_bool().unwrap_or(true))
        .execute(&mut *tx)
        .await?;
        counts.2 += 1;
    }
    tx.commit().await?;
    Ok(Json(json!({ "categories": counts.0, "delivery": counts.1, "coupons": counts.2 })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn rounding() {
        use rust_decimal::Decimal;
        assert_eq!(super::round2(Decimal::new(12345, 3)), Decimal::new(1235, 2));
    }
}
