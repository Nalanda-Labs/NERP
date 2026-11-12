//! What stock costs: every movement gets a cost in the base currency.
//!
//! - Goods coming in make a **cost layer** (receipts at their purchase price, transfers at the
//!   cost they left the other warehouse with, returns at the cost they went out with, anything
//!   else at the warehouse's current average) and update the warehouse's **moving average**.
//! - Goods going out are costed by the company's method — **FIFO** (the oldest layers first)
//!   or **average** — and use up the oldest layers either way, so the method can be changed at
//!   any time. Beyond the layers (stock sold into the negative) the average is used.
//! - With "post to the books" on (and accounting set up), each movement is posted: inventory
//!   against cost of goods sold (sales), purchases (receipts) or stock adjustments
//!   (write-offs, counts, the product form). Transfers stay inside the inventory account.

use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

#[derive(Serialize, sqlx::FromRow, Clone)]
pub struct Settings {
    pub cost_method: String,
    pub post_to_books: bool,
    pub inventory_account: Option<Uuid>,
    pub cogs_account: Option<Uuid>,
    pub receipt_account: Option<Uuid>,
    pub adjust_account: Option<Uuid>,
}

pub async fn settings<'e, E: sqlx::PgExecutor<'e>>(db: E) -> AppResult<Settings> {
    Ok(sqlx::query_as("SELECT cost_method, post_to_books, inventory_account, cogs_account, receipt_account, adjust_account FROM inv_settings WHERE id = 1")
        .fetch_one(db)
        .await?)
}

fn r6(d: Decimal) -> Decimal {
    d.round_dp(6)
}

/// A unit cost for goods coming in when nothing better is known: the warehouse's average,
/// else the product's average anywhere, else its own cost in the base currency, else 0.
async fn fallback_cost(tx: &mut Transaction<'_, Postgres>, product: Uuid, warehouse: Uuid) -> AppResult<Decimal> {
    let here: Option<Decimal> = sqlx::query_scalar("SELECT avg_cost FROM inv_stock WHERE product_id = $1 AND warehouse_id = $2 AND avg_cost > 0")
        .bind(product)
        .bind(warehouse)
        .fetch_optional(&mut **tx)
        .await?;
    if let Some(c) = here {
        return Ok(c);
    }
    let anywhere: Option<Decimal> = sqlx::query_scalar(
        "SELECT SUM(avg_cost * GREATEST(quantity, 0)) / NULLIF(SUM(GREATEST(quantity, 0)), 0) FROM inv_stock WHERE product_id = $1 AND avg_cost > 0",
    )
    .bind(product)
    .fetch_one(&mut **tx)
    .await?;
    if let Some(c) = anywhere {
        return Ok(c);
    }
    let own: Option<Decimal> = sqlx::query_scalar(
        "SELECT p.cost * COALESCE((SELECT rate FROM crm_currencies c WHERE c.code = p.currency), 1) FROM crm_products p WHERE p.id = $1",
    )
    .bind(product)
    .fetch_one(&mut **tx)
    .await?;
    Ok(own.unwrap_or_default())
}

/// Costs a movement that has just changed `inv_stock` (from `have` to `have + delta`) and
/// records it on the movement. `unit_in`: the cost of goods coming in, when known. Returns
/// the movement's cost (signed like `delta`).
pub async fn cost_movement(
    tx: &mut Transaction<'_, Postgres>,
    movement: Uuid,
    product: Uuid,
    warehouse: Uuid,
    have: Decimal,
    delta: Decimal,
    unit_in: Option<Decimal>,
) -> AppResult<Decimal> {
    let s = settings(&mut **tx).await?;
    let avg: Decimal = sqlx::query_scalar("SELECT avg_cost FROM inv_stock WHERE product_id = $1 AND warehouse_id = $2")
        .bind(product)
        .bind(warehouse)
        .fetch_one(&mut **tx)
        .await?;
    let (unit, cost) = if delta > Decimal::ZERO {
        let unit = match unit_in {
            Some(u) if u >= Decimal::ZERO => u,
            _ => fallback_cost(tx, product, warehouse).await?,
        };
        sqlx::query("INSERT INTO inv_cost_layers (product_id, warehouse_id, movement_id, quantity, remaining, unit_cost) VALUES ($1, $2, $3, $4, $4, $5)")
            .bind(product)
            .bind(warehouse)
            .bind(movement)
            .bind(delta)
            .bind(r6(unit))
            .execute(&mut **tx)
            .await?;
        // Moving average over what is on the shelf (stock below zero doesn't count).
        let on_hand = have.max(Decimal::ZERO);
        let new_avg = if on_hand + delta > Decimal::ZERO { (on_hand * avg + delta * unit) / (on_hand + delta) } else { unit };
        sqlx::query("UPDATE inv_stock SET avg_cost = $3 WHERE product_id = $1 AND warehouse_id = $2")
            .bind(product)
            .bind(warehouse)
            .bind(r6(new_avg))
            .execute(&mut **tx)
            .await?;
        (unit, delta * unit)
    } else {
        let mut left = -delta;
        let mut fifo = Decimal::ZERO;
        // The oldest layers go first, whatever the method (so it can be switched any time).
        let layers: Vec<(Uuid, Decimal, Decimal)> = sqlx::query_as(
            "SELECT id, remaining, unit_cost FROM inv_cost_layers WHERE product_id = $1 AND warehouse_id = $2 AND remaining > 0 ORDER BY received_at, id FOR UPDATE",
        )
        .bind(product)
        .bind(warehouse)
        .fetch_all(&mut **tx)
        .await?;
        for (id, remaining, unit) in layers {
            if left <= Decimal::ZERO {
                break;
            }
            let take = remaining.min(left);
            sqlx::query("UPDATE inv_cost_layers SET remaining = remaining - $2 WHERE id = $1").bind(id).bind(take).execute(&mut **tx).await?;
            fifo += take * unit;
            left -= take;
        }
        let beyond_unit = if avg > Decimal::ZERO { avg } else { fallback_cost(tx, product, warehouse).await? };
        fifo += left * beyond_unit;
        let q = -delta;
        let total = if s.cost_method == "fifo" { fifo } else { q * beyond_unit };
        // Nothing left on the shelf: the average starts again with the next goods in.
        if have + delta <= Decimal::ZERO {
            sqlx::query("UPDATE inv_stock SET avg_cost = 0 WHERE product_id = $1 AND warehouse_id = $2 AND $3 <= 0").bind(product).bind(warehouse).bind(have + delta).execute(&mut **tx).await?;
            if have + delta == Decimal::ZERO {
                sqlx::query("UPDATE inv_cost_layers SET remaining = 0 WHERE product_id = $1 AND warehouse_id = $2 AND remaining > 0").bind(product).bind(warehouse).execute(&mut **tx).await?;
            }
        }
        (if q.is_zero() { Decimal::ZERO } else { total / q }, -total)
    };
    let cost = cost.round_dp(4);
    sqlx::query("UPDATE inv_movements SET unit_cost = $2, cost = $3 WHERE id = $1").bind(movement).bind(r6(unit)).bind(cost).execute(&mut **tx).await?;
    Ok(cost)
}

/// The cost each unit of a product went out with to an order (to bring returns back at it).
pub async fn order_out_cost(tx: &mut Transaction<'_, Postgres>, product: Uuid, item: Uuid) -> AppResult<Option<Decimal>> {
    Ok(sqlx::query_scalar(
        "SELECT unit_cost FROM inv_movements WHERE product_id = $1 AND item_id = $2 AND delta < 0 AND unit_cost IS NOT NULL ORDER BY created_at DESC LIMIT 1",
    )
    .bind(product)
    .bind(item)
    .fetch_optional(&mut **tx)
    .await?)
}

/// Posts a movement to the books, when that is on and accounting is set up. A failure to
/// post doesn't stop the stock from moving (it is logged, and the movement shows no entry).
pub async fn book(tx: &mut Transaction<'_, Postgres>, movement: Uuid, reason: &str, document: Option<Uuid>, cost: Decimal, user: Option<Uuid>) -> AppResult<()> {
    let s = settings(&mut **tx).await?;
    let amount = cost.abs().round_dp(2);
    if !s.post_to_books || amount.is_zero() {
        return Ok(());
    }
    let ready: bool = sqlx::query_scalar("SELECT ready FROM acc_settings WHERE id = 1").fetch_one(&mut **tx).await?;
    let Some(inventory) = s.inventory_account.filter(|_| ready) else { return Ok(()) };
    let doc_kind: Option<String> = match document {
        Some(d) => sqlx::query_scalar("SELECT kind FROM inv_documents WHERE id = $1").bind(d).fetch_optional(&mut **tx).await?,
        None => None,
    };
    let (other, what) = match (reason, doc_kind.as_deref()) {
        (_, Some("transfer")) | ("opening", _) => return Ok(()),
        ("document" | "document_cancelled", Some("receipt")) => (s.receipt_account, "Goods received"),
        ("document" | "document_cancelled", _) | ("product_form", _) => (s.adjust_account, "Stock adjustment"),
        _ => (s.cogs_account, "Cost of goods sold"),
    };
    let Some(other) = other else { return Ok(()) };
    let line = |account: Uuid, debit: Decimal, credit: Decimal| crate::accounting::ledger::Line {
        account,
        debit,
        credit,
        currency: None,
        amount_currency: None,
        description: Some(what.to_string()),
        party: None,
        tax: None,
        tax_base: None,
    };
    // Goods in: inventory up; goods out: inventory down.
    let lines = if cost > Decimal::ZERO {
        vec![line(inventory, amount, Decimal::ZERO), line(other, Decimal::ZERO, amount)]
    } else {
        vec![line(other, amount, Decimal::ZERO), line(inventory, Decimal::ZERO, amount)]
    };
    let today: NaiveDate = Utc::now().date_naive();
    let mut sp = sqlx::Acquire::begin(&mut **tx).await?;
    match crate::accounting::ledger::post(&mut sp, today, Some(what), "inventory", Some(movement), &lines, user).await {
        Ok(entry) => {
            sp.commit().await?;
            sqlx::query("UPDATE inv_movements SET entry_id = $2 WHERE id = $1").bind(movement).bind(entry).execute(&mut **tx).await?;
        }
        Err(e) => {
            sp.rollback().await?;
            tracing::warn!(movement = %movement, error = %e, "a stock movement could not be posted to the books");
        }
    }
    Ok(())
}

/// What a product costs now, per unit, in the base currency: the next unit out (FIFO) or the
/// average, over the warehouses that hold it. None when its stock isn't counted.
pub async fn current_unit_cost(state: &AppState, product: Uuid) -> AppResult<Option<Decimal>> {
    let s = settings(&state.db).await?;
    let v: Option<Decimal> = if s.cost_method == "fifo" {
        sqlx::query_scalar(
            "SELECT unit_cost FROM inv_cost_layers WHERE product_id = $1 AND remaining > 0 ORDER BY received_at, id LIMIT 1",
        )
        .bind(product)
        .fetch_optional(&state.db)
        .await?
    } else {
        sqlx::query_scalar("SELECT SUM(avg_cost * quantity) / NULLIF(SUM(quantity), 0) FROM inv_stock WHERE product_id = $1 AND quantity > 0 AND avg_cost > 0")
            .bind(product)
            .fetch_one(&state.db)
            .await?
    };
    Ok(v.map(|v| v.round_dp(4)))
}

// ---------------------------------------------------------------------------
// Settings and reports
// ---------------------------------------------------------------------------

pub async fn get_settings(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    super::require(&auth)?;
    let s = settings(&state.db).await?;
    let ready: bool = sqlx::query_scalar("SELECT ready FROM acc_settings WHERE id = 1").fetch_one(&state.db).await?;
    Ok(Json(json!({ "settings": s, "accounting_ready": ready })))
}

#[derive(Deserialize)]
pub struct SettingsInput {
    cost_method: Option<String>,
    post_to_books: Option<bool>,
    inventory_account: Option<Uuid>,
    cogs_account: Option<Uuid>,
    receipt_account: Option<Uuid>,
    adjust_account: Option<Uuid>,
}

pub async fn put_settings(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SettingsInput>) -> AppResult<Json<Value>> {
    super::require(&auth)?;
    if let Some(m) = &b.cost_method
        && !["fifo", "average"].contains(&m.as_str()) {
            return Err(AppError::bad("the cost method is fifo or average"));
        }
    let mut tx = state.db.begin().await?;
    let mut s = settings(&mut *tx).await?;
    if let Some(m) = b.cost_method {
        s.cost_method = m;
    }
    for (slot, v) in [(&mut s.inventory_account, b.inventory_account), (&mut s.cogs_account, b.cogs_account), (&mut s.receipt_account, b.receipt_account), (&mut s.adjust_account, b.adjust_account)] {
        if v.is_some() {
            *slot = v;
        }
    }
    if let Some(p) = b.post_to_books {
        s.post_to_books = p;
    }
    if s.post_to_books {
        let ready: bool = sqlx::query_scalar("SELECT ready FROM acc_settings WHERE id = 1").fetch_one(&mut *tx).await?;
        if !ready {
            return Err(AppError::bad("set up accounting first"));
        }
        let accounts = [s.inventory_account, s.cogs_account, s.receipt_account, s.adjust_account];
        if accounts.iter().any(Option::is_none) {
            return Err(AppError::bad("choose the inventory, cost of goods sold, goods received and adjustment accounts"));
        }
        let ids: Vec<Uuid> = accounts.iter().flatten().copied().collect();
        let ok: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT id) FROM acc_accounts WHERE id = ANY($1) AND is_active").bind(&ids).fetch_one(&mut *tx).await?;
        if ok as usize != ids.iter().collect::<std::collections::HashSet<_>>().len() {
            return Err(AppError::bad("an account doesn't exist or is inactive"));
        }
    }
    sqlx::query(
        "UPDATE inv_settings SET cost_method = $1, post_to_books = $2, inventory_account = $3, cogs_account = $4, receipt_account = $5, adjust_account = $6 WHERE id = 1",
    )
    .bind(&s.cost_method)
    .bind(s.post_to_books)
    .bind(s.inventory_account)
    .bind(s.cogs_account)
    .bind(s.receipt_account)
    .bind(s.adjust_account)
    .execute(&mut *tx)
    .await?;
    crate::audit::record(&mut *tx, Some(auth.id), "inventory.settings", "settings", None, json!({ "cost_method": s.cost_method, "post_to_books": s.post_to_books }), None).await;
    tx.commit().await?;
    get_settings(State(state), auth).await
}

#[derive(Deserialize)]
pub struct ValuationQuery {
    warehouse_id: Option<Uuid>,
}

/// What the stock is worth: per product (and warehouse), quantity, unit cost and value.
pub async fn valuation(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ValuationQuery>) -> AppResult<Json<Value>> {
    super::require(&auth)?;
    let s = settings(&state.db).await?;
    // FIFO: what the remaining layers cost; average: quantity × average.
    let rows: Vec<(Uuid, String, Option<String>, String, Decimal, Decimal)> = sqlx::query_as(
        "SELECT p.id, p.name, p.sku, p.unit, SUM(st.quantity),
                SUM(CASE WHEN $2 = 'fifo'
                         THEN COALESCE((SELECT SUM(l.remaining * l.unit_cost) FROM inv_cost_layers l WHERE l.product_id = st.product_id AND l.warehouse_id = st.warehouse_id AND l.remaining > 0), 0)
                              + LEAST(st.quantity - COALESCE((SELECT SUM(l.remaining) FROM inv_cost_layers l WHERE l.product_id = st.product_id AND l.warehouse_id = st.warehouse_id AND l.remaining > 0), 0), 0) * st.avg_cost
                         ELSE st.quantity * st.avg_cost END)
         FROM inv_stock st JOIN crm_products p ON p.id = st.product_id
         WHERE st.quantity <> 0 AND ($1::uuid IS NULL OR st.warehouse_id = $1)
         GROUP BY p.id, p.name, p.sku, p.unit ORDER BY p.name",
    )
    .bind(q.warehouse_id)
    .bind(&s.cost_method)
    .fetch_all(&state.db)
    .await?;
    let total: Decimal = rows.iter().map(|r| r.5).sum();
    let base: String = sqlx::query_scalar("SELECT code FROM crm_currencies WHERE is_base LIMIT 1").fetch_optional(&state.db).await?.unwrap_or_default();
    Ok(Json(json!({
        "method": s.cost_method,
        "currency": base.trim(),
        "total": total.round_dp(2),
        "items": rows.into_iter().map(|(id, name, sku, unit, qty, value)| json!({
            "product_id": id, "name": name, "sku": sku, "unit": unit, "quantity": qty.normalize(),
            "unit_cost": if qty.is_zero() { Decimal::ZERO } else { (value / qty).round_dp(4) }, "value": value.round_dp(2),
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct PeriodQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

/// Cost of goods sold (orders, net of returns), write-offs and count differences in a period.
pub async fn cogs(State(state): State<AppState>, auth: AuthUser, Query(q): Query<PeriodQuery>) -> AppResult<Json<Value>> {
    super::require(&auth)?;
    let from: DateTime<Utc> = q.from.unwrap_or_else(|| Utc::now().date_naive() - chrono::Duration::days(30)).and_hms_opt(0, 0, 0).unwrap().and_utc();
    let to: DateTime<Utc> = (q.to.unwrap_or_else(|| Utc::now().date_naive()) + chrono::Duration::days(1)).and_hms_opt(0, 0, 0).unwrap().and_utc();
    let rows: Vec<(Uuid, String, String, Decimal, Decimal)> = sqlx::query_as(
        "SELECT p.id, p.name,
                CASE WHEN m.reason LIKE 'order%' THEN 'sold'
                     WHEN d.kind = 'writeoff' THEN 'written_off'
                     WHEN d.kind = 'count' OR m.reason = 'product_form' THEN 'counted'
                     ELSE 'other' END AS what,
                -SUM(m.delta), -SUM(COALESCE(m.cost, 0))
         FROM inv_movements m JOIN crm_products p ON p.id = m.product_id LEFT JOIN inv_documents d ON d.id = m.document_id
         WHERE m.created_at >= $1 AND m.created_at < $2 AND (m.reason LIKE 'order%' OR d.kind IN ('writeoff', 'count') OR m.reason = 'product_form')
         GROUP BY p.id, p.name, 3 ORDER BY 3, p.name",
    )
    .bind(from)
    .bind(to)
    .fetch_all(&state.db)
    .await?;
    let sum = |w: &str| rows.iter().filter(|r| r.2 == w).map(|r| r.4).sum::<Decimal>().round_dp(2);
    Ok(Json(json!({
        "from": from.date_naive(), "to": (to - chrono::Duration::days(1)).date_naive(),
        "sold": sum("sold"), "written_off": sum("written_off"), "counted": sum("counted"),
        "items": rows.into_iter().map(|(id, name, what, qty, cost)| json!({ "product_id": id, "name": name, "kind": what, "quantity": qty.normalize(), "cost": cost.round_dp(2) })).collect::<Vec<_>>(),
    })))
}
