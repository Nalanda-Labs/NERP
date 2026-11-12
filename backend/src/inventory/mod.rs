//! Inventory: warehouses, stock per warehouse, stock documents and the movement history.
//!
//! Every change of stock goes through [`adjust`], which updates the warehouse's quantity,
//! records a movement (with the balance after it) and keeps the product's total
//! (`crm_products.stock`) in step. Changes come from stock documents, shop orders (taken at
//! checkout, back when cancelled) and the product form.
//!
//! Documents are drafts until posted; posting applies them all or nothing, and a posted
//! document can be cancelled, which undoes it (if the stock is still there):
//! - receipt: goods arrive in a warehouse (from a supplier, at a purchase price);
//! - write-off: goods leave a warehouse (damaged, lost, used);
//! - transfer: goods move from one warehouse to another;
//! - count: a stock count; each listed product is set to the quantity found.

pub mod costing;
mod documents;

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch},
    Json, Router,
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/inventory/warehouses", get(list_warehouses).post(create_warehouse))
        .route("/inventory/warehouses/{id}", patch(update_warehouse).delete(delete_warehouse))
        .route("/inventory/stock", get(stock))
        .route("/inventory/settings", get(costing::get_settings).put(costing::put_settings))
        .route("/inventory/valuation", get(costing::valuation))
        .route("/inventory/cogs", get(costing::cogs))
        .route("/inventory/products/{id}", get(product_stock))
        .merge(documents::router())
}

fn require(auth: &AuthUser) -> AppResult<()> {
    if auth.can(perm::INVENTORY) || auth.can(perm::CRM_ADMIN) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

// ---------------------------------------------------------------------------
// Moving stock
// ---------------------------------------------------------------------------

/// Why stock changed, and what changed it.
pub struct Reason<'a> {
    pub kind: &'a str,
    pub document: Option<Uuid>,
    pub item: Option<Uuid>,
    pub user: Option<Uuid>,
}

/// Changes how many units of a product a warehouse holds, inside the caller's transaction.
/// Refuses to go below zero unless `allow_negative`. Returns the movement's id.
pub async fn adjust(tx: &mut Transaction<'_, Postgres>, product: Uuid, warehouse: Uuid, delta: Decimal, allow_negative: bool, why: Reason<'_>) -> AppResult<Uuid> {
    Ok(adjust_costed(tx, product, warehouse, delta, allow_negative, why, None).await?.0)
}

/// The same, with the unit cost (base currency) of goods coming in when it is known (a
/// receipt's price, a transfer's or return's cost out). Returns the movement and its cost.
pub async fn adjust_costed(
    tx: &mut Transaction<'_, Postgres>,
    product: Uuid,
    warehouse: Uuid,
    delta: Decimal,
    allow_negative: bool,
    why: Reason<'_>,
    unit_in: Option<Decimal>,
) -> AppResult<(Uuid, Decimal)> {
    sqlx::query("INSERT INTO inv_stock (product_id, warehouse_id, quantity) VALUES ($1, $2, 0) ON CONFLICT DO NOTHING")
        .bind(product)
        .bind(warehouse)
        .execute(&mut **tx)
        .await?;
    let have: Decimal = sqlx::query_scalar("SELECT quantity FROM inv_stock WHERE product_id = $1 AND warehouse_id = $2 FOR UPDATE")
        .bind(product)
        .bind(warehouse)
        .fetch_one(&mut **tx)
        .await?;
    let after = have + delta;
    if after < Decimal::ZERO && !allow_negative {
        let (name, wh): (String, String) = sqlx::query_as("SELECT p.name, w.name FROM crm_products p, inv_warehouses w WHERE p.id = $1 AND w.id = $2")
            .bind(product)
            .bind(warehouse)
            .fetch_one(&mut **tx)
            .await?;
        return Err(AppError::bad(if have <= Decimal::ZERO {
            format!("“{name}” is out of stock in {wh}")
        } else {
            format!("only {} of “{name}” left in {wh}", have.normalize())
        }));
    }
    sqlx::query("UPDATE inv_stock SET quantity = $3 WHERE product_id = $1 AND warehouse_id = $2")
        .bind(product)
        .bind(warehouse)
        .bind(after)
        .execute(&mut **tx)
        .await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO inv_movements (product_id, warehouse_id, delta, balance, reason, document_id, item_id, user_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(product)
    .bind(warehouse)
    .bind(delta)
    .bind(after)
    .bind(why.kind)
    .bind(why.document)
    .bind(why.item)
    .bind(why.user)
    .fetch_one(&mut **tx)
    .await?;
    sqlx::query("UPDATE crm_products SET stock = (SELECT COALESCE(SUM(quantity), 0) FROM inv_stock WHERE product_id = $1), updated_at = now() WHERE id = $1")
        .bind(product)
        .execute(&mut **tx)
        .await?;
    let cost = costing::cost_movement(tx, id, product, warehouse, have, delta, unit_in).await?;
    costing::book(tx, id, why.kind, why.document, cost, why.user).await?;
    Ok((id, cost))
}

pub async fn default_warehouse<'e, E: sqlx::PgExecutor<'e>>(db: E) -> AppResult<Uuid> {
    sqlx::query_scalar("SELECT id FROM inv_warehouses WHERE is_default")
        .fetch_optional(db)
        .await?
        .ok_or_else(|| AppError::bad("there is no default warehouse"))
}

/// Sets a product's total from the product form: the difference goes in or out of the
/// default warehouse. `None` stops counting stock for it.
pub async fn set_total(state: &AppState, auth: &AuthUser, product: Uuid, total: Option<Decimal>) -> AppResult<()> {
    let mut tx = state.db.begin().await?;
    let current: Option<Decimal> = sqlx::query_scalar("SELECT stock FROM crm_products WHERE id = $1 FOR UPDATE").bind(product).fetch_one(&mut *tx).await?;
    match total {
        None => {
            let held: Decimal = sqlx::query_scalar("SELECT COALESCE(SUM(abs(quantity)), 0) FROM inv_stock WHERE product_id = $1").bind(product).fetch_one(&mut *tx).await?;
            if held > Decimal::ZERO {
                return Err(AppError::bad("warehouses still hold this product; write it off before you stop counting it"));
            }
            sqlx::query("DELETE FROM inv_stock WHERE product_id = $1").bind(product).execute(&mut *tx).await?;
            sqlx::query("UPDATE crm_products SET stock = NULL WHERE id = $1").bind(product).execute(&mut *tx).await?;
        }
        Some(t) if Some(t) != current => {
            let delta = t - current.unwrap_or(Decimal::ZERO);
            let wh = default_warehouse(&mut *tx).await?;
            adjust(&mut tx, product, wh, delta, false, Reason { kind: "product_form", document: None, item: None, user: Some(auth.id) })
                .await
                .map_err(|e| match e {
                    AppError::BadRequest(m) => AppError::bad(format!("{m}; other warehouses hold the rest, so change it with an inventory document")),
                    e => e,
                })?;
        }
        Some(_) => {}
    }
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Warehouses
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct Warehouse {
    id: Uuid,
    name: String,
    address: Option<String>,
    is_default: bool,
    is_active: bool,
    /// Products with stock here, and their units.
    products: i64,
    units: Decimal,
}

async fn warehouses(state: &AppState) -> AppResult<Vec<Warehouse>> {
    Ok(sqlx::query_as(
        "SELECT w.id, w.name, w.address, w.is_default, w.is_active,
                (SELECT COUNT(*) FROM inv_stock s WHERE s.warehouse_id = w.id AND s.quantity <> 0) AS products,
                (SELECT COALESCE(SUM(quantity), 0) FROM inv_stock s WHERE s.warehouse_id = w.id) AS units
         FROM inv_warehouses w ORDER BY w.is_active DESC, w.sort, w.name",
    )
    .fetch_all(&state.db)
    .await?)
}

async fn list_warehouses(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "items": warehouses(&state).await? })))
}

#[derive(Deserialize)]
struct WarehouseInput {
    name: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    address: Option<Option<String>>,
    is_default: Option<bool>,
    is_active: Option<bool>,
}

async fn create_warehouse(State(state): State<AppState>, auth: AuthUser, Json(b): Json<WarehouseInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 120)?;
    let sort: i32 = sqlx::query_scalar("SELECT COALESCE(MAX(sort), 0) + 10 FROM inv_warehouses").fetch_one(&state.db).await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO inv_warehouses (name, address, sort) VALUES ($1, $2, $3) RETURNING id")
        .bind(&name)
        .bind(clean(b.address.flatten()))
        .bind(sort)
        .fetch_one(&state.db)
        .await?;
    if b.is_default == Some(true) {
        make_default(&state, id).await?;
    }
    Ok(Json(json!({ "items": warehouses(&state).await? })))
}

async fn make_default(state: &AppState, id: Uuid) -> AppResult<()> {
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE inv_warehouses SET is_default = FALSE WHERE is_default AND id <> $1").bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE inv_warehouses SET is_default = TRUE, is_active = TRUE WHERE id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn update_warehouse(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<WarehouseInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let (is_default,): (bool,) = sqlx::query_as("SELECT is_default FROM inv_warehouses WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("warehouse"))?;
    let name = b.name.as_deref().map(|n| required("name", n, 120)).transpose()?;
    if b.is_active == Some(false) {
        if is_default {
            return Err(AppError::bad("make another warehouse the default first"));
        }
        let held: Decimal = sqlx::query_scalar("SELECT COALESCE(SUM(abs(quantity)), 0) FROM inv_stock WHERE warehouse_id = $1").bind(id).fetch_one(&state.db).await?;
        if held > Decimal::ZERO {
            return Err(AppError::bad("it still holds stock; move or write it off first"));
        }
    }
    sqlx::query(
        "UPDATE inv_warehouses SET name = COALESCE($2, name), address = CASE WHEN $3 THEN $4 ELSE address END, is_active = COALESCE($5, is_active) WHERE id = $1",
    )
    .bind(id)
    .bind(name)
    .bind(b.address.is_some())
    .bind(clean(b.address.flatten()))
    .bind(b.is_active)
    .execute(&state.db)
    .await?;
    if b.is_default == Some(true) {
        make_default(&state, id).await?;
    }
    Ok(Json(json!({ "items": warehouses(&state).await? })))
}

/// Only a warehouse that was never used; others are deactivated.
async fn delete_warehouse(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let used: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM inv_movements WHERE warehouse_id = $1)
             OR EXISTS (SELECT 1 FROM inv_documents WHERE from_warehouse = $1 OR to_warehouse = $1)
             OR EXISTS (SELECT 1 FROM inv_warehouses WHERE id = $1 AND is_default)",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if used {
        return Err(AppError::bad("this warehouse has history or is the default; deactivate it instead"));
    }
    sqlx::query("DELETE FROM inv_warehouses WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "items": warehouses(&state).await? })))
}

// ---------------------------------------------------------------------------
// Stock
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct StockQuery {
    q: Option<String>,
    /// all | low (5 or fewer) | out (none or below)
    state: Option<String>,
    warehouse: Option<Uuid>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Serialize, FromRow)]
struct StockRow {
    product_id: Uuid,
    name: String,
    sku: Option<String>,
    unit: String,
    total: Decimal,
    /// warehouse id → quantity
    per: Value,
}

/// Counted products and how many each warehouse holds.
async fn stock(State(state): State<AppState>, _auth: AuthUser, Query(q): Query<StockQuery>) -> AppResult<Json<Value>> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let term = clean(q.q).map(|t| format!("%{}%", t.to_lowercase().replace('%', "\\%").replace('_', "\\_")));
    let level = q.state.unwrap_or_default();
    // The quantity filtered on: one warehouse's, or the total.
    let where_sql = "p.stock IS NOT NULL AND p.is_active
        AND ($1::text IS NULL OR lower(p.name) LIKE $1 OR lower(p.sku) LIKE $1)
        AND (CASE $2 WHEN 'low' THEN qty > 0 AND qty <= 5 WHEN 'out' THEN qty <= 0 ELSE TRUE END)";
    let from_sql = "FROM (SELECT p.*, CASE WHEN $3::uuid IS NULL THEN p.stock ELSE COALESCE((SELECT quantity FROM inv_stock s WHERE s.product_id = p.id AND s.warehouse_id = $3), 0) END AS qty
                    FROM crm_products p) p";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) {from_sql} WHERE {where_sql}")))
        .bind(&term)
        .bind(&level)
        .bind(q.warehouse)
        .fetch_one(&state.db)
        .await?;
    let items: Vec<StockRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT p.id AS product_id, p.name, p.sku, p.unit, p.stock AS total,
                COALESCE((SELECT jsonb_object_agg(s.warehouse_id, s.quantity) FROM inv_stock s WHERE s.product_id = p.id), '{{}}') AS per
         {from_sql} WHERE {where_sql} ORDER BY p.name LIMIT $4 OFFSET $5"
    )))
    .bind(&term)
    .bind(&level)
    .bind(q.warehouse)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items, "total": total, "limit": limit, "offset": offset, "warehouses": warehouses(&state).await? })))
}

#[derive(Serialize, FromRow)]
struct Movement {
    id: Uuid,
    warehouse: String,
    delta: Decimal,
    balance: Decimal,
    reason: String,
    document_id: Option<Uuid>,
    document_number: Option<String>,
    item_id: Option<Uuid>,
    item_number: Option<i64>,
    item_code: Option<String>,
    user_name: Option<String>,
    /// Per unit and in all (signed like the quantity), in the base currency.
    unit_cost: Option<Decimal>,
    cost: Option<Decimal>,
    entry_id: Option<Uuid>,
    created_at: DateTime<Utc>,
}

/// One product: stock per warehouse and its movement history.
async fn product_stock(State(state): State<AppState>, _auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (name, unit, total): (String, String, Option<Decimal>) = sqlx::query_as("SELECT name, unit, stock FROM crm_products WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("product"))?;
    let per: Vec<(Uuid, String, Decimal)> = sqlx::query_as(
        "SELECT w.id, w.name, s.quantity FROM inv_stock s JOIN inv_warehouses w ON w.id = s.warehouse_id WHERE s.product_id = $1 ORDER BY w.sort, w.name",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let movements: Vec<Movement> = sqlx::query_as(
        "SELECT m.id, w.name AS warehouse, m.delta, m.balance, m.reason, m.document_id, d.number AS document_number,
                m.item_id, i.number AS item_number, t.code AS item_code,
                trim(u.first_name || ' ' || u.last_name) AS user_name, round(m.unit_cost, 4) AS unit_cost, m.cost, m.entry_id, m.created_at
         FROM inv_movements m JOIN inv_warehouses w ON w.id = m.warehouse_id
         LEFT JOIN inv_documents d ON d.id = m.document_id
         LEFT JOIN crm_items i ON i.id = m.item_id LEFT JOIN crm_entity_types t ON t.id = i.entity_type
         LEFT JOIN users u ON u.id = m.user_id
         WHERE m.product_id = $1 ORDER BY m.created_at DESC LIMIT 200",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "name": name, "unit": unit, "total": total,
        "warehouses": per.into_iter().map(|(id, name, q)| json!({ "id": id, "name": name, "quantity": q })).collect::<Vec<_>>(),
        "movements": movements,
    })))
}
