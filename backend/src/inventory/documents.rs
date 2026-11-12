//! Stock documents: receipts, write-offs, transfers and stock counts.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{adjust, adjust_costed, require, Reason};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::clean,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/inventory/documents", get(list).post(create))
        .route("/inventory/documents/{id}", get(get_one).patch(update).delete(remove))
        .route("/inventory/documents/{id}/post", post(post_document))
        .route("/inventory/documents/{id}/cancel", post(cancel))
}

const KINDS: &[(&str, &str)] = &[("receipt", "RC"), ("writeoff", "WO"), ("transfer", "TR"), ("count", "IC")];

#[derive(Serialize, FromRow)]
struct Summary {
    id: Uuid,
    kind: String,
    number: String,
    status: String,
    doc_date: NaiveDate,
    from_warehouse: Option<Uuid>,
    from_name: Option<String>,
    to_warehouse: Option<Uuid>,
    to_name: Option<String>,
    supplier_id: Option<Uuid>,
    supplier_name: Option<String>,
    reference: Option<String>,
    currency: Option<String>,
    comment: Option<String>,
    lines: i64,
    units: Decimal,
    /// Receipts: what the goods cost.
    cost: Option<Decimal>,
    created_by_name: Option<String>,
    posted_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

const SUMMARY: &str = "SELECT d.id, d.kind, d.number, d.status, d.doc_date, d.from_warehouse, fw.name AS from_name, d.to_warehouse, tw.name AS to_name,
    d.supplier_id, s.title AS supplier_name, d.reference, d.currency, d.comment,
    (SELECT COUNT(*) FROM inv_document_lines l WHERE l.document_id = d.id) AS lines,
    (SELECT COALESCE(SUM(quantity), 0) FROM inv_document_lines l WHERE l.document_id = d.id) AS units,
    (SELECT SUM(quantity * price) FROM inv_document_lines l WHERE l.document_id = d.id AND d.kind = 'receipt') AS cost,
    trim(u.first_name || ' ' || u.last_name) AS created_by_name, d.posted_at, d.created_at
    FROM inv_documents d
    LEFT JOIN inv_warehouses fw ON fw.id = d.from_warehouse LEFT JOIN inv_warehouses tw ON tw.id = d.to_warehouse
    LEFT JOIN crm_items s ON s.id = d.supplier_id LEFT JOIN users u ON u.id = d.created_by";

#[derive(Deserialize)]
struct ListQuery {
    kind: Option<String>,
    status: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list(State(state): State<AppState>, _auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    let term = clean(q.q).map(|t| format!("%{}%", t.to_lowercase().replace('%', "\\%").replace('_', "\\_")));
    let filter = "($1::text IS NULL OR d.kind = $1) AND ($2::text IS NULL OR d.status = $2)
        AND ($3::text IS NULL OR lower(d.number) LIKE $3 OR lower(d.reference) LIKE $3 OR lower(d.comment) LIKE $3)";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM inv_documents d WHERE {filter}")))
        .bind(clean(q.kind.clone()))
        .bind(clean(q.status.clone()))
        .bind(&term)
        .fetch_one(&state.db)
        .await?;
    let items: Vec<Summary> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SUMMARY} WHERE {filter} ORDER BY d.doc_date DESC, d.created_at DESC LIMIT $4 OFFSET $5")))
        .bind(clean(q.kind))
        .bind(clean(q.status))
        .bind(&term)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!({ "items": items, "total": total, "limit": limit, "offset": offset })))
}

#[derive(Serialize, FromRow)]
struct LineView {
    id: Uuid,
    product_id: Uuid,
    name: String,
    sku: Option<String>,
    unit: String,
    quantity: Decimal,
    price: Option<Decimal>,
    applied: Option<Decimal>,
    /// What the source warehouse (or, for receipts and counts, the target) holds now.
    in_stock: Option<Decimal>,
}

async fn view(state: &AppState, id: Uuid) -> AppResult<Value> {
    let d: Summary = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SUMMARY} WHERE d.id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("document"))?;
    let lines: Vec<LineView> = sqlx::query_as(
        "SELECT l.id, l.product_id, p.name, p.sku, p.unit, l.quantity, l.price, l.applied,
                (SELECT quantity FROM inv_stock s WHERE s.product_id = l.product_id AND s.warehouse_id = COALESCE(d.from_warehouse, d.to_warehouse)) AS in_stock
         FROM inv_document_lines l JOIN crm_products p ON p.id = l.product_id JOIN inv_documents d ON d.id = l.document_id
         WHERE l.document_id = $1 ORDER BY l.sort",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let mut v = serde_json::to_value(d).unwrap();
    v["items"] = json!(lines);
    Ok(v)
}

async fn get_one(State(state): State<AppState>, _auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    view(&state, id).await.map(Json)
}

#[derive(Deserialize)]
struct LineInput {
    product_id: Uuid,
    quantity: f64,
    price: Option<f64>,
}

#[derive(Deserialize)]
struct DocInput {
    kind: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    from_warehouse: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    to_warehouse: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    supplier_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    reference: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    currency: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    comment: Option<Option<String>>,
    doc_date: Option<NaiveDate>,
    items: Option<Vec<LineInput>>,
}

fn dec(v: f64, what: &str) -> AppResult<Decimal> {
    Decimal::try_from(v).map(|d| d.round_dp(4)).map_err(|_| AppError::bad(format!("{what} must be a number")))
}

async fn save_lines(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, id: Uuid, kind: &str, items: &[LineInput]) -> AppResult<()> {
    if items.len() > 1000 {
        return Err(AppError::bad("at most 1000 lines in one document"));
    }
    let mut seen = std::collections::HashSet::new();
    sqlx::query("DELETE FROM inv_document_lines WHERE document_id = $1").bind(id).execute(&mut **tx).await?;
    for (i, l) in items.iter().enumerate() {
        let q = dec(l.quantity, "quantity")?;
        if q < Decimal::ZERO || (kind != "count" && q == Decimal::ZERO) {
            return Err(AppError::bad(if kind == "count" { "quantities found cannot be negative" } else { "quantities must be more than zero" }));
        }
        if kind == "count" && !seen.insert(l.product_id) {
            return Err(AppError::bad("a stock count lists each product once"));
        }
        let price = match (kind, l.price) {
            ("receipt", Some(p)) => {
                let p = dec(p, "price")?;
                if p < Decimal::ZERO {
                    return Err(AppError::bad("prices cannot be negative"));
                }
                Some(p)
            }
            _ => None,
        };
        let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_products WHERE id = $1)").bind(l.product_id).fetch_one(&mut **tx).await?;
        if !exists {
            return Err(AppError::bad("a product on the document does not exist"));
        }
        sqlx::query("INSERT INTO inv_document_lines (document_id, product_id, quantity, price, sort) VALUES ($1, $2, $3, $4, $5)")
            .bind(id)
            .bind(l.product_id)
            .bind(q)
            .bind(price)
            .bind(i as i32)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

async fn active_warehouse(state: &AppState, id: Option<Uuid>) -> AppResult<()> {
    if let Some(w) = id {
        let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM inv_warehouses WHERE id = $1 AND is_active)").bind(w).fetch_one(&state.db).await?;
        if !ok {
            return Err(AppError::bad("that warehouse does not exist or is inactive"));
        }
    }
    Ok(())
}

async fn check_supplier(state: &AppState, id: Option<Uuid>) -> AppResult<()> {
    if let Some(s) = id {
        let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_items WHERE id = $1 AND entity_type = $2 AND deleted_at IS NULL)")
            .bind(s)
            .bind(crate::crm::COMPANY)
            .fetch_one(&state.db)
            .await?;
        if !ok {
            return Err(AppError::bad("the supplier must be a company in the CRM"));
        }
    }
    Ok(())
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<DocInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let kind = b.kind.clone().unwrap_or_default();
    let prefix = KINDS.iter().find(|(k, _)| *k == kind).map(|(_, p)| *p).ok_or_else(|| AppError::bad("kind must be receipt, writeoff, transfer or count"))?;
    let default = super::default_warehouse(&state.db).await?;
    // Sensible starting warehouses: the default one where goods come from or go to.
    let (from, to) = match kind.as_str() {
        "receipt" | "count" => (None, Some(b.to_warehouse.flatten().unwrap_or(default))),
        "writeoff" => (Some(b.from_warehouse.flatten().unwrap_or(default)), None),
        _ => (Some(b.from_warehouse.flatten().unwrap_or(default)), b.to_warehouse.flatten()),
    };
    active_warehouse(&state, from).await?;
    active_warehouse(&state, to).await?;
    let supplier = b.supplier_id.flatten().filter(|_| kind == "receipt");
    check_supplier(&state, supplier).await?;
    let currency = match b.currency.flatten().filter(|_| kind == "receipt") {
        Some(c) => Some(c.trim().to_uppercase()),
        None if kind == "receipt" => Some(crate::crm::money::base_currency(&state).await?),
        None => None,
    };
    let mut tx = state.db.begin().await?;
    let n: i64 = sqlx::query_scalar("UPDATE inv_counters SET last = last + 1 WHERE kind = $1 RETURNING last").bind(&kind).fetch_one(&mut *tx).await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO inv_documents (kind, number, from_warehouse, to_warehouse, supplier_id, reference, currency, comment, doc_date, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, COALESCE($9, CURRENT_DATE), $10) RETURNING id",
    )
    .bind(&kind)
    .bind(format!("{prefix}-{n:04}"))
    .bind(from)
    .bind(to)
    .bind(supplier)
    .bind(clean(b.reference.flatten()))
    .bind(currency)
    .bind(clean(b.comment.flatten()))
    .bind(b.doc_date)
    .bind(auth.id)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(items) = &b.items {
        save_lines(&mut tx, id, &kind, items).await?;
    }
    tx.commit().await?;
    view(&state, id).await.map(Json)
}

async fn draft(state: &AppState, id: Uuid) -> AppResult<(String, String)> {
    let (kind, status): (String, String) = sqlx::query_as("SELECT kind, status FROM inv_documents WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("document"))?;
    Ok((kind, status))
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<DocInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let (kind, status) = draft(&state, id).await?;
    if status != "draft" {
        return Err(AppError::bad("a posted document cannot be changed; cancel it and make a new one"));
    }
    if let Some(w) = b.from_warehouse {
        active_warehouse(&state, w).await?;
    }
    if let Some(w) = b.to_warehouse {
        active_warehouse(&state, w).await?;
    }
    if let Some(s) = b.supplier_id {
        check_supplier(&state, s).await?;
    }
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE inv_documents SET
             from_warehouse = CASE WHEN $2 THEN $3 ELSE from_warehouse END,
             to_warehouse = CASE WHEN $4 THEN $5 ELSE to_warehouse END,
             supplier_id = CASE WHEN $6 THEN $7 ELSE supplier_id END,
             reference = CASE WHEN $8 THEN $9 ELSE reference END,
             currency = CASE WHEN $10 THEN $11 ELSE currency END,
             comment = CASE WHEN $12 THEN $13 ELSE comment END,
             doc_date = COALESCE($14, doc_date), updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(b.from_warehouse.is_some())
    .bind(b.from_warehouse.flatten())
    .bind(b.to_warehouse.is_some())
    .bind(b.to_warehouse.flatten())
    .bind(b.supplier_id.is_some())
    .bind(b.supplier_id.flatten())
    .bind(b.reference.is_some())
    .bind(clean(b.reference.clone().flatten()))
    .bind(b.currency.is_some())
    .bind(b.currency.clone().flatten().map(|c| c.trim().to_uppercase()))
    .bind(b.comment.is_some())
    .bind(clean(b.comment.clone().flatten()))
    .bind(b.doc_date)
    .execute(&mut *tx)
    .await?;
    if let Some(items) = &b.items {
        save_lines(&mut tx, id, &kind, items).await?;
    }
    tx.commit().await?;
    view(&state, id).await.map(Json)
}

/// Drafts can be deleted; posted documents are cancelled instead, so history stays.
async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let (_, status) = draft(&state, id).await?;
    if status != "draft" {
        return Err(AppError::bad("only drafts can be deleted; cancel a posted document instead"));
    }
    sqlx::query("DELETE FROM inv_documents WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "deleted": true })))
}

#[derive(FromRow)]
struct Head {
    kind: String,
    status: String,
    from_warehouse: Option<Uuid>,
    to_warehouse: Option<Uuid>,
}

#[derive(FromRow)]
struct Line {
    id: Uuid,
    product_id: Uuid,
    quantity: Decimal,
    applied: Option<Decimal>,
    /// Receipts: the price per unit in the base currency.
    base_price: Option<Decimal>,
}

async fn head_and_lines(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, id: Uuid) -> AppResult<(Head, Vec<Line>)> {
    let h: Head = sqlx::query_as("SELECT kind, status, from_warehouse, to_warehouse FROM inv_documents WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(AppError::NotFound("document"))?;
    let lines: Vec<Line> = sqlx::query_as(
        "SELECT l.id, l.product_id, l.quantity, l.applied,
                l.price * COALESCE((SELECT c.rate FROM crm_currencies c WHERE c.code = d.currency), 1) AS base_price
         FROM inv_document_lines l JOIN inv_documents d ON d.id = l.document_id WHERE l.document_id = $1 ORDER BY l.sort",
    )
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
    Ok((h, lines))
}

/// Applies the document to stock, all or nothing.
async fn post_document(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let mut tx = state.db.begin().await?;
    let (h, lines) = head_and_lines(&mut tx, id).await?;
    if h.status != "draft" {
        return Err(AppError::bad("this document is already posted or cancelled"));
    }
    if lines.is_empty() {
        return Err(AppError::bad("add at least one product"));
    }
    let why = || Reason { kind: "document", document: Some(id), item: None, user: Some(auth.id) };
    let need = |w: Option<Uuid>, what: &str| w.ok_or_else(|| AppError::bad(format!("choose the warehouse goods {what}")));
    match h.kind.as_str() {
        "receipt" => {
            let to = need(h.to_warehouse, "arrive in")?;
            for l in &lines {
                adjust_costed(&mut tx, l.product_id, to, l.quantity, false, why(), l.base_price).await?;
            }
        }
        "writeoff" => {
            let from = need(h.from_warehouse, "leave")?;
            for l in &lines {
                adjust(&mut tx, l.product_id, from, -l.quantity, false, why()).await?;
            }
        }
        "transfer" => {
            let from = need(h.from_warehouse, "leave")?;
            let to = need(h.to_warehouse, "go to")?;
            if from == to {
                return Err(AppError::bad("a transfer goes between two different warehouses"));
            }
            for l in &lines {
                // The goods keep the cost they left with.
                let (_, out) = adjust_costed(&mut tx, l.product_id, from, -l.quantity, false, why(), None).await?;
                adjust_costed(&mut tx, l.product_id, to, l.quantity, false, why(), Some(-out / l.quantity)).await?;
            }
        }
        _ => {
            let at = need(h.to_warehouse, "were counted in")?;
            for l in &lines {
                let have: Decimal = sqlx::query_scalar("SELECT COALESCE((SELECT quantity FROM inv_stock WHERE product_id = $1 AND warehouse_id = $2), 0)")
                    .bind(l.product_id)
                    .bind(at)
                    .fetch_one(&mut *tx)
                    .await?;
                let delta = l.quantity - have;
                if delta != Decimal::ZERO {
                    adjust(&mut tx, l.product_id, at, delta, true, why()).await?;
                } else {
                    // Counting turns stock counting on even when nothing changes.
                    sqlx::query("INSERT INTO inv_stock (product_id, warehouse_id, quantity) VALUES ($1, $2, 0) ON CONFLICT DO NOTHING").bind(l.product_id).bind(at).execute(&mut *tx).await?;
                    sqlx::query("UPDATE crm_products SET stock = COALESCE(stock, 0) WHERE id = $1").bind(l.product_id).execute(&mut *tx).await?;
                }
                sqlx::query("UPDATE inv_document_lines SET applied = $2 WHERE id = $1").bind(l.id).bind(delta).execute(&mut *tx).await?;
            }
        }
    }
    sqlx::query("UPDATE inv_documents SET status = 'posted', posted_by = $2, posted_at = now(), updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(auth.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    view(&state, id).await.map(Json)
}

/// Undoes a posted document. Refused when the goods it brought in are already gone.
async fn cancel(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let mut tx = state.db.begin().await?;
    let (h, lines) = head_and_lines(&mut tx, id).await?;
    if h.status != "posted" {
        return Err(AppError::bad("only a posted document can be cancelled"));
    }
    let why = || Reason { kind: "document_cancelled", document: Some(id), item: None, user: Some(auth.id) };
    for l in &lines {
        match h.kind.as_str() {
            "receipt" => {
                adjust(&mut tx, l.product_id, h.to_warehouse.unwrap(), -l.quantity, false, why()).await?;
            }
            "writeoff" => {
                // Back at the cost it was written off at.
                let unit: Option<Decimal> = sqlx::query_scalar("SELECT unit_cost FROM inv_movements WHERE document_id = $1 AND product_id = $2 AND delta < 0 ORDER BY created_at DESC LIMIT 1")
                    .bind(id)
                    .bind(l.product_id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .flatten();
                adjust_costed(&mut tx, l.product_id, h.from_warehouse.unwrap(), l.quantity, false, why(), unit).await?;
            }
            "transfer" => {
                let (_, out) = adjust_costed(&mut tx, l.product_id, h.to_warehouse.unwrap(), -l.quantity, false, why(), None).await?;
                adjust_costed(&mut tx, l.product_id, h.from_warehouse.unwrap(), l.quantity, false, why(), Some(-out / l.quantity)).await?;
            }
            _ => {
                if let Some(a) = l.applied.filter(|a| *a != Decimal::ZERO) {
                    adjust(&mut tx, l.product_id, h.to_warehouse.unwrap(), -a, true, why()).await?;
                }
            }
        }
    }
    sqlx::query("UPDATE inv_documents SET status = 'cancelled', updated_at = now() WHERE id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    view(&state, id).await.map(Json)
}
