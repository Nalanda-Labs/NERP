//! Currencies (with exchange rates to a base currency), the product catalog, and product
//! rows on deals and leads with discount and tax arithmetic in exact decimals.
//!
//! Line arithmetic (per unit, then × quantity):
//!   discounted = price − discount            (discount: percent of price, or an amount)
//!   tax included:  net = discounted / (1 + rate), tax = discounted − net
//!   tax excluded:  net = discounted,             tax = discounted × rate
//!   line total = (net + tax) × quantity

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch},
    Json, Router,
};
use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use std::str::FromStr;
use uuid::Uuid;

use super::{access::is_crm_admin, access::require, broadcast, items::load_row, timeline};
use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    routes::{clean, required, Page},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/currencies", get(list_currencies).post(add_currency))
        .route("/crm/currencies/{code}", patch(update_currency).delete(remove_currency))
        .route("/crm/products", get(list_products).post(create_product))
        .route("/crm/products/{id}", get(get_product).patch(update_product).delete(delete_product))
        .route("/crm/items/{id}/products", get(get_rows).put(put_rows))
}

// ---------------------------------------------------------------------------
// Currencies
// ---------------------------------------------------------------------------

pub async fn base_currency(state: &AppState) -> AppResult<String> {
    Ok(sqlx::query_scalar("SELECT code FROM crm_currencies WHERE is_base LIMIT 1")
        .fetch_optional(&state.db)
        .await?
        .unwrap_or_else(|| "USD".into()))
}

/// Value of one unit of `code` in the base currency, or None if not enabled.
pub async fn rate_of(state: &AppState, code: &str) -> AppResult<Option<Decimal>> {
    Ok(sqlx::query_scalar("SELECT rate FROM crm_currencies WHERE code = $1").bind(code).fetch_optional(&state.db).await?)
}

#[derive(Serialize, FromRow)]
struct Currency {
    code: String,
    rate: Decimal,
    is_base: bool,
    sort: i32,
    updated_at: DateTime<Utc>,
}

async fn currencies(state: &AppState) -> AppResult<Vec<Currency>> {
    Ok(sqlx::query_as("SELECT code, rate, is_base, sort, updated_at FROM crm_currencies ORDER BY is_base DESC, sort, code")
        .fetch_all(&state.db)
        .await?)
}

async fn list_currencies(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Vec<Currency>>> {
    Ok(Json(currencies(&state).await?))
}

fn iso_code(c: &str) -> AppResult<String> {
    let c = c.trim().to_uppercase();
    if c.len() == 3 && c.chars().all(|ch| ch.is_ascii_uppercase()) {
        Ok(c)
    } else {
        Err(AppError::bad("currency must be a three-letter ISO 4217 code"))
    }
}

fn positive(s: &str) -> AppResult<Decimal> {
    let d = Decimal::from_str(s.trim()).map_err(|_| AppError::bad("rate must be a number"))?;
    if d <= Decimal::ZERO {
        return Err(AppError::bad("rate must be greater than zero"));
    }
    Ok(d)
}

#[derive(Deserialize)]
struct NewCurrency {
    code: String,
    /// Value of 1 unit in the base currency.
    rate: String,
}

async fn add_currency(State(state): State<AppState>, auth: AuthUser, Json(body): Json<NewCurrency>) -> AppResult<Json<Vec<Currency>>> {
    if !is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    let code = iso_code(&body.code)?;
    sqlx::query("INSERT INTO crm_currencies (code, rate, sort) VALUES ($1, $2, 100) ON CONFLICT (code) DO NOTHING")
        .bind(&code)
        .bind(positive(&body.rate)?)
        .execute(&state.db)
        .await?;
    Ok(Json(currencies(&state).await?))
}

#[derive(Deserialize)]
struct UpdateCurrency {
    rate: Option<String>,
    /// Make this the base currency: every rate is rescaled so amounts keep their value.
    is_base: Option<bool>,
}

/// Recomputes base-currency amounts after a rate change. Runs in slices so a currency
/// used by millions of records never holds one huge transaction.
async fn recompute_base(state: &AppState, code: Option<&str>) -> AppResult<()> {
    loop {
        let n = sqlx::query(
            "UPDATE crm_items i SET opportunity_base = ROUND(i.opportunity * c.rate, 4)
             FROM crm_currencies c
             WHERE i.id IN (SELECT i2.id FROM crm_items i2 JOIN crm_currencies c2 ON c2.code = i2.currency
                            WHERE ($1::text IS NULL OR i2.currency = $1)
                              AND i2.opportunity_base <> ROUND(i2.opportunity * c2.rate, 4) LIMIT 5000)
               AND c.code = i.currency",
        )
        .bind(code)
        .execute(&state.db)
        .await?
        .rows_affected();
        if n == 0 {
            return Ok(());
        }
    }
}

async fn update_currency(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(body): Json<UpdateCurrency>) -> AppResult<Json<Vec<Currency>>> {
    if !is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    let code = iso_code(&code)?;
    let current = rate_of(&state, &code).await?.ok_or(AppError::NotFound("currency"))?;
    if body.is_base == Some(true) && base_currency(&state).await? != code {
        // The books are kept in the base currency: it can't change under them.
        let posted: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM acc_entries WHERE status <> 'draft')").fetch_one(&state.db).await?;
        if posted {
            return Err(AppError::bad("the accounts are kept in the base currency and have entries, so it can't change"));
        }
    }
    if body.is_base == Some(true) {
        // New base B with old rate r_B: every rate r becomes r / r_B (and B becomes 1).
        let mut tx = state.db.begin().await?;
        sqlx::query("UPDATE crm_currencies SET rate = rate / $1, is_base = (code = $2), updated_at = now()")
            .bind(current)
            .bind(&code)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        recompute_base(&state, None).await?;
    } else if let Some(r) = &body.rate {
        let is_base: bool = sqlx::query_scalar("SELECT is_base FROM crm_currencies WHERE code = $1").bind(&code).fetch_one(&state.db).await?;
        if is_base {
            return Err(AppError::bad("the base currency's rate is always 1"));
        }
        sqlx::query("UPDATE crm_currencies SET rate = $1, updated_at = now() WHERE code = $2")
            .bind(positive(r)?)
            .bind(&code)
            .execute(&state.db)
            .await?;
        recompute_base(&state, Some(&code)).await?;
    }
    Ok(Json(currencies(&state).await?))
}

async fn remove_currency(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>) -> AppResult<Json<Vec<Currency>>> {
    if !is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    let code = iso_code(&code)?;
    let used: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM crm_items WHERE currency = $1) OR EXISTS (SELECT 1 FROM crm_products WHERE currency = $1)",
    )
    .bind(&code)
    .fetch_one(&state.db)
    .await?;
    if used {
        return Err(AppError::bad("records or products still use this currency"));
    }
    let n = sqlx::query("DELETE FROM crm_currencies WHERE code = $1 AND NOT is_base").bind(&code).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::bad("the base currency cannot be removed"));
    }
    Ok(Json(currencies(&state).await?))
}

// ---------------------------------------------------------------------------
// Product catalog
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
pub struct Product {
    id: Uuid,
    name: String,
    sku: Option<String>,
    description: Option<String>,
    price: Decimal,
    currency: String,
    tax_rate: Option<Decimal>,
    tax_included: bool,
    unit: String,
    is_active: bool,
    updated_at: DateTime<Utc>,
    /// Shop: listed in the online shop, under this address and category, with this photo.
    in_shop: bool,
    slug: Option<String>,
    shop_category_id: Option<Uuid>,
    image_file_id: Option<Uuid>,
    /// Units on hand; None = not counted.
    stock: Option<Decimal>,
    /// Variants: the product they are a variant of, and their property values.
    parent_id: Option<Uuid>,
    props: Value,
    /// A service: sold like a product, never kept in stock.
    is_service: bool,
    /// What a unit costs the company (in `currency`), for gross profit; None = unknown.
    cost: Option<Decimal>,
}

const PRODUCT_COLUMNS: &str = "id, name, sku, description, price, currency, tax_rate, tax_included, unit, is_active, updated_at, in_shop, slug, shop_category_id, image_file_id, stock, parent_id, props, is_service, cost";

#[derive(Deserialize)]
struct ProductQuery {
    q: Option<String>,
    include_inactive: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

async fn list_products(State(state): State<AppState>, _auth: AuthUser, Query(q): Query<ProductQuery>) -> AppResult<Json<Page<Product>>> {
    let limit: i64 = q.limit.as_deref().and_then(|v| v.parse().ok()).unwrap_or(50).clamp(1, 200);
    let offset: i64 = q.offset.as_deref().and_then(|v| v.parse().ok()).unwrap_or(0).max(0);
    let term = clean(q.q).map(|t| format!("%{}%", t.to_lowercase().replace('%', "\\%").replace('_', "\\_")));
    let all = q.include_inactive.as_deref() == Some("true");
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM crm_products WHERE ($1::text IS NULL OR lower(name) LIKE $1 OR lower(sku) LIKE $1) AND ($2 OR is_active)",
    )
    .bind(&term)
    .bind(all)
    .fetch_one(&state.db)
    .await?;
    let items: Vec<Product> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {PRODUCT_COLUMNS} FROM crm_products
         WHERE ($1::text IS NULL OR lower(name) LIKE $1 OR lower(sku) LIKE $1) AND ($2 OR is_active)
         -- Variants right after their parent.
         ORDER BY COALESCE((SELECT x.name FROM crm_products x WHERE x.id = crm_products.parent_id), name), parent_id NULLS FIRST, name
         LIMIT $3 OFFSET $4"
    )))
    .bind(&term)
    .bind(all)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(Page { items, total, limit, offset }))
}

#[derive(Deserialize)]
struct ProductInput {
    name: Option<String>,
    sku: Option<String>,
    description: Option<String>,
    price: Option<String>,
    currency: Option<String>,
    tax_rate: Option<String>,
    tax_included: Option<bool>,
    unit: Option<String>,
    is_active: Option<bool>,
    // Shop fields: left out = unchanged, null = cleared.
    in_shop: Option<bool>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    shop_category_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    image_file_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    stock: Option<Option<String>>,
    is_service: Option<bool>,
    /// Left out = unchanged, null or "" = unknown.
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    cost: Option<Option<String>>,
}

pub(super) fn can_manage_products(auth: &AuthUser) -> AppResult<()> {
    if auth.can(perm::CRM_PRODUCTS) || is_crm_admin(auth) { Ok(()) } else { Err(AppError::Forbidden) }
}

/// Catalog managers edit every product; a section's editors (people, or members of its
/// departments and their sub-departments) edit the products in that section.
pub(super) async fn may_edit_section(state: &AppState, auth: &AuthUser, section: Option<Uuid>) -> AppResult<()> {
    if can_manage_products(auth).is_ok() {
        return Ok(());
    }
    let Some(section) = section else { return Err(AppError::Forbidden) };
    let ok: bool = sqlx::query_scalar(
        "WITH RECURSIVE mine AS (
             SELECT department_id AS id FROM user_departments WHERE user_id = $2
             UNION SELECT d.parent_id FROM departments d JOIN mine ON d.id = mine.id WHERE d.parent_id IS NOT NULL)
         SELECT EXISTS (SELECT 1 FROM catalog_section_editors e WHERE e.category_id = $1 AND (e.user_id = $2 OR e.department_id IN (SELECT id FROM mine)))",
    )
    .bind(section)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    if ok { Ok(()) } else { Err(AppError::Forbidden) }
}

/// The section a product (or a variant, through its product) belongs to.
pub(super) async fn section_of(state: &AppState, product: Uuid) -> AppResult<Option<Uuid>> {
    let r: Option<Option<Uuid>> = sqlx::query_scalar("SELECT COALESCE(p.shop_category_id, parent.shop_category_id) FROM crm_products p LEFT JOIN crm_products parent ON parent.id = p.parent_id WHERE p.id = $1")
        .bind(product)
        .fetch_optional(&state.db)
        .await?;
    r.ok_or(AppError::NotFound("product"))
}

pub(super) async fn may_edit_product(state: &AppState, auth: &AuthUser, product: Uuid) -> AppResult<()> {
    let section = section_of(state, product).await?;
    may_edit_section(state, auth, section).await
}

fn tax(s: &Option<String>) -> AppResult<Option<Decimal>> {
    match clean(s.clone()) {
        None => Ok(None),
        Some(v) => {
            let d = Decimal::from_str(&v).map_err(|_| AppError::bad("tax rate must be a number"))?;
            if d.is_sign_negative() || d > Decimal::from(100) {
                return Err(AppError::bad("tax rate must be between 0 and 100"));
            }
            Ok(Some(d))
        }
    }
}

async fn save_product(state: &AppState, auth: &AuthUser, id: Option<Uuid>, b: ProductInput) -> AppResult<Uuid> {
    let price = match clean(b.price.clone()) {
        Some(p) => Decimal::from_str(&p).map_err(|_| AppError::bad("price must be a number"))?,
        None => Decimal::ZERO,
    };
    if price.is_sign_negative() {
        return Err(AppError::bad("price cannot be negative"));
    }
    let currency = match &b.currency {
        Some(c) => iso_code(c)?,
        None => base_currency(state).await?,
    };
    rate_of(state, &currency).await?.ok_or_else(|| AppError::bad(format!("currency {currency} is not enabled")))?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 300)?;
    let tax_rate = tax(&b.tax_rate)?;
    let unit = clean(b.unit).unwrap_or_else(|| "pcs".into());
    // Services are never counted in stock.
    let stock = if b.is_service == Some(true) { None } else { b.stock };
    let shop = ShopFields { in_shop: b.in_shop, shop_category_id: b.shop_category_id, image_file_id: b.image_file_id, stock };
    let is_service = b.is_service;
    let cost = match &b.cost {
        None => None,
        Some(c) => Some(match c.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            None => None,
            Some(c) => {
                let d = Decimal::from_str(c).map_err(|_| AppError::bad("cost must be a number"))?;
                if d.is_sign_negative() {
                    return Err(AppError::bad("cost cannot be negative"));
                }
                Some(d)
            }
        }),
    };
    let id = match id {
        Some(id) => {
            sqlx::query(
                "UPDATE crm_products SET name = $1, sku = $2, description = $3, price = $4, currency = $5, tax_rate = $6,
                     tax_included = $7, unit = $8, is_active = $9, updated_at = now() WHERE id = $10",
            )
            .bind(name)
            .bind(clean(b.sku))
            .bind(clean(b.description))
            .bind(price)
            .bind(currency)
            .bind(tax_rate)
            .bind(b.tax_included.unwrap_or(false))
            .bind(unit)
            .bind(b.is_active.unwrap_or(true))
            .bind(id)
            .execute(&state.db)
            .await?;
            id
        }
        None => sqlx::query_scalar(
            "INSERT INTO crm_products (name, sku, description, price, currency, tax_rate, tax_included, unit, is_active)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
        )
        .bind(name)
        .bind(clean(b.sku))
        .bind(clean(b.description))
        .bind(price)
        .bind(currency)
        .bind(tax_rate)
        .bind(b.tax_included.unwrap_or(false))
        .bind(unit)
        .bind(b.is_active.unwrap_or(true))
        .fetch_one(&state.db)
        .await?,
    };
    if let Some(sv) = is_service {
        // A service holds no stock: a product still in a warehouse can't become one.
        if sv {
            let held: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM inv_stock WHERE product_id = $1 AND quantity <> 0)").bind(id).fetch_one(&state.db).await?;
            if held {
                return Err(AppError::bad("this product is in stock; write it off before making it a service"));
            }
            sqlx::query("UPDATE crm_products SET stock = NULL WHERE id = $1").bind(id).execute(&state.db).await?;
        }
        sqlx::query("UPDATE crm_products SET is_service = $2 WHERE id = $1").bind(id).bind(sv).execute(&state.db).await?;
    }
    if let Some(c) = cost {
        sqlx::query("UPDATE crm_products SET cost = $2 WHERE id = $1").bind(id).bind(c).execute(&state.db).await?;
    }
    save_shop_fields(state, auth, id, shop).await?;
    Ok(id)
}

/// A unit's cost of a catalogue product in `currency`: its cost price, or else the average
/// purchase price of its posted receipts. None when neither is known.
pub(crate) async fn unit_cost(state: &AppState, product: Uuid, currency: &str) -> AppResult<Option<Decimal>> {
    // Products whose stock is counted cost what the stock costs (FIFO or average).
    if let Some(c) = crate::inventory::costing::current_unit_cost(state, product).await? {
        let base = base_currency(state).await?;
        if base == currency {
            return Ok(Some(round4(c)));
        }
        let rt = rate_of(state, currency).await?;
        return Ok(rt.filter(|r| !r.is_zero()).map(|r| round4(c / r)));
    }
    let own: Option<(Option<Decimal>, String)> = sqlx::query_as("SELECT cost, currency FROM crm_products WHERE id = $1").bind(product).fetch_optional(&state.db).await?;
    let (amount, from) = match own {
        Some((Some(c), cur)) => (c, cur),
        Some((None, _)) => {
            let avg: Option<(Decimal, String)> = sqlx::query_as(
                "SELECT SUM(l.price * l.quantity) / NULLIF(SUM(l.quantity), 0), COALESCE(d.currency, (SELECT code FROM crm_currencies WHERE is_base))
                 FROM inv_document_lines l JOIN inv_documents d ON d.id = l.document_id
                 WHERE l.product_id = $1 AND d.kind = 'receipt' AND d.status = 'posted' AND l.price IS NOT NULL AND l.quantity > 0
                 GROUP BY 2 ORDER BY SUM(l.quantity) DESC LIMIT 1",
            )
            .bind(product)
            .fetch_optional(&state.db)
            .await?
            .and_then(|(a, c): (Option<Decimal>, String)| a.map(|a| (a, c)));
            match avg {
                Some(x) => x,
                None => return Ok(None),
            }
        }
        None => return Ok(None),
    };
    if from == currency {
        return Ok(Some(round4(amount)));
    }
    let (Some(rf), Some(rt)) = (rate_of(state, &from).await?, rate_of(state, currency).await?) else { return Ok(None) };
    Ok(if rt.is_zero() { None } else { Some(round4(amount * rf / rt)) })
}

fn round4(d: Decimal) -> Decimal {
    d.round_dp_with_strategy(4, RoundingStrategy::MidpointAwayFromZero)
}

struct ShopFields {
    in_shop: Option<bool>,
    shop_category_id: Option<Option<Uuid>>,
    image_file_id: Option<Option<Uuid>>,
    stock: Option<Option<String>>,
}

const IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];

/// The shop columns of a product. A photo becomes public in the shop, so it must be an image
/// the employee uploaded themselves (or the one already there).
async fn save_shop_fields(state: &AppState, auth: &AuthUser, id: Uuid, f: ShopFields) -> AppResult<()> {
    let (name, slug, image): (String, Option<String>, Option<Uuid>) =
        sqlx::query_as("SELECT name, slug, image_file_id FROM crm_products WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    if let Some(Some(c)) = f.shop_category_id {
        let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM shop_categories WHERE id = $1)").bind(c).fetch_one(&state.db).await?;
        if !ok {
            return Err(AppError::bad("that shop category does not exist"));
        }
    }
    if let Some(Some(file)) = f.image_file_id.filter(|v| *v != image) {
        let row: Option<(Option<Uuid>, String)> = sqlx::query_as("SELECT owner_id, mime_type FROM files WHERE id = $1").bind(file).fetch_optional(&state.db).await?;
        match row {
            Some((owner, mime)) if owner == Some(auth.id) => {
                if !IMAGE_TYPES.contains(&mime.as_str()) {
                    return Err(AppError::bad("the photo must be a PNG, JPEG, WebP or GIF image"));
                }
            }
            _ => return Err(AppError::bad("upload the photo first")),
        }
    }
    let stock = match f.stock {
        None => None,
        Some(v) => Some(match clean(v) {
            None => None,
            Some(v) => {
                let d = Decimal::from_str(&v).map_err(|_| AppError::bad("stock must be a number"))?;
                if d.is_sign_negative() {
                    return Err(AppError::bad("stock cannot be negative"));
                }
                Some(d)
            }
        }),
    };
    // A product gets its shop address the first time it is listed, and keeps it.
    let slug = match (slug, f.in_shop) {
        (None, Some(true)) => Some(unique_product_slug(state, &name).await?),
        (s, _) => s,
    };
    sqlx::query(
        "UPDATE crm_products SET
             in_shop = COALESCE($2, in_shop),
             shop_category_id = CASE WHEN $3 THEN $4 ELSE shop_category_id END,
             image_file_id = CASE WHEN $5 THEN $6 ELSE image_file_id END,
             slug = $7
         WHERE id = $1",
    )
    .bind(id)
    .bind(f.in_shop)
    .bind(f.shop_category_id.is_some())
    .bind(f.shop_category_id.flatten())
    .bind(f.image_file_id.is_some())
    .bind(f.image_file_id.flatten())
    .bind(slug)
    .execute(&state.db)
    .await?;
    // Stock lives in warehouses: the form's total changes the default warehouse.
    if let Some(total) = stock {
        crate::inventory::set_total(state, auth, id, total).await?;
    }
    Ok(())
}

pub(super) async fn unique_product_slug(state: &AppState, name: &str) -> AppResult<String> {
    let base = crate::routes::slugify(name, "product");
    for i in 1..1000 {
        let s = if i == 1 { base.clone() } else { format!("{base}-{i}") };
        let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_products WHERE slug = $1)").bind(&s).fetch_one(&state.db).await?;
        if !taken {
            return Ok(s);
        }
    }
    Ok(format!("{base}-{}", Uuid::new_v4().simple()))
}

async fn load_product(state: &AppState, id: Uuid) -> AppResult<Product> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {PRODUCT_COLUMNS} FROM crm_products WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("product"))
}

async fn get_product(State(state): State<AppState>, _auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Product>> {
    Ok(Json(load_product(&state, id).await?))
}

async fn create_product(State(state): State<AppState>, auth: AuthUser, Json(b): Json<ProductInput>) -> AppResult<Json<Product>> {
    may_edit_section(&state, &auth, b.shop_category_id.flatten()).await?;
    let id = save_product(&state, &auth, None, b).await?;
    Ok(Json(load_product(&state, id).await?))
}

async fn update_product(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ProductInput>) -> AppResult<Json<Product>> {
    load_product(&state, id).await?;
    may_edit_product(&state, &auth, id).await?;
    // Moving it to another section needs the right to that section too.
    if let Some(to) = b.shop_category_id {
        may_edit_section(&state, &auth, to).await?;
    }
    save_product(&state, &auth, Some(id), b).await?;
    Ok(Json(load_product(&state, id).await?))
}

/// Products already used on deals or stock documents are deactivated rather than deleted, so
/// history stays intact.
async fn delete_product(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    may_edit_product(&state, &auth, id).await?;
    // The product and its variants.
    let used: bool = sqlx::query_scalar(
        "WITH ids AS (SELECT id FROM crm_products WHERE id = $1 OR parent_id = $1)
         SELECT EXISTS (SELECT 1 FROM crm_item_products WHERE product_id IN (SELECT id FROM ids))
             OR EXISTS (SELECT 1 FROM inv_document_lines WHERE product_id IN (SELECT id FROM ids))",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if used {
        sqlx::query("UPDATE crm_products SET is_active = FALSE, updated_at = now() WHERE id = $1 OR parent_id = $1").bind(id).execute(&state.db).await?;
        return Ok(Json(json!({ "deactivated": true })));
    }
    sqlx::query("DELETE FROM crm_products WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "deleted": true })))
}

// ---------------------------------------------------------------------------
// Product rows on a record
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, FromRow, Clone)]
pub struct ProductRow {
    #[serde(default)]
    pub id: Option<Uuid>,
    pub product_id: Option<Uuid>,
    pub name: String,
    pub price: Decimal,
    pub quantity: Decimal,
    #[serde(default = "pcs")]
    pub unit: String,
    #[serde(default = "percent")]
    pub discount_kind: String,
    #[serde(default)]
    pub discount_value: Decimal,
    pub tax_rate: Option<Decimal>,
    #[serde(default)]
    pub tax_included: bool,
    /// Unit cost in the record's currency (gross profit); filled from the catalogue when left out.
    #[serde(default)]
    pub cost: Option<Decimal>,
}

fn pcs() -> String {
    "pcs".into()
}
fn percent() -> String {
    "percent".into()
}

#[derive(Serialize, Default)]
pub struct Line {
    pub net_unit: Decimal,
    pub discount_unit: Decimal,
    pub tax: Decimal,
    pub net: Decimal,
    pub total: Decimal,
}

#[derive(Serialize, Default)]
pub struct Totals {
    pub subtotal: Decimal,
    pub discount: Decimal,
    pub tax: Decimal,
    pub total: Decimal,
}

const HUNDRED: Decimal = Decimal::ONE_HUNDRED;

fn round(d: Decimal) -> Decimal {
    d.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

pub fn line(r: &ProductRow) -> Line {
    let discount_unit = match r.discount_kind.as_str() {
        "amount" => r.discount_value.min(r.price),
        _ => r.price * r.discount_value.min(HUNDRED) / HUNDRED,
    };
    let discounted = r.price - discount_unit;
    let rate = r.tax_rate.unwrap_or_default() / HUNDRED;
    let (net_unit, tax_unit) = if r.tax_included {
        let net = discounted / (Decimal::ONE + rate);
        (net, discounted - net)
    } else {
        (discounted, discounted * rate)
    };
    let q = r.quantity;
    Line {
        net_unit: round(net_unit),
        discount_unit: round(discount_unit),
        tax: round(tax_unit * q),
        net: round(net_unit * q),
        total: round((net_unit + tax_unit) * q),
    }
}

pub fn sum(rows: &[ProductRow]) -> Totals {
    let mut t = Totals::default();
    for r in rows {
        let l = line(r);
        t.subtotal += l.net;
        t.discount += round(l.discount_unit * r.quantity);
        t.tax += l.tax;
        t.total += l.total;
    }
    t
}

pub(crate) async fn rows_of(state: &AppState, item: Uuid) -> AppResult<Vec<ProductRow>> {
    Ok(sqlx::query_as(
        "SELECT id, product_id, name, price, quantity, unit, discount_kind, discount_value, tax_rate, tax_included, cost
         FROM crm_item_products WHERE item_id = $1 ORDER BY sort",
    )
    .bind(item)
    .fetch_all(&state.db)
    .await?)
}

/// Recomputes the record's amount from its rows when it is not entered manually.
pub async fn totals(state: &AppState, item: Uuid) -> AppResult<Totals> {
    let rows = rows_of(state, item).await?;
    let t = sum(&rows);
    sqlx::query(
        "UPDATE crm_items i SET opportunity = $1, opportunity_base = ROUND($1 * COALESCE(c.rate, 1), 4)
         FROM crm_currencies c WHERE i.id = $2 AND NOT i.is_manual_amount AND c.code = i.currency",
    )
    .bind(t.total)
    .bind(item)
    .execute(&state.db)
    .await?;
    Ok(t)
}

async fn get_rows(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = load_row(&state, id, false).await?;
    super::access::require_read(&state, &auth, row.entity_type, id, row.assigned_to).await?;
    let rows = rows_of(&state, id).await?;
    let lines: Vec<Value> = rows.iter().map(|r| json!({ "row": r, "line": line(r) })).collect();
    Ok(Json(json!({ "rows": lines, "totals": sum(&rows), "currency": row.currency, "is_manual_amount": row.is_manual_amount })))
}

#[derive(Deserialize)]
struct PutRows {
    rows: Vec<ProductRow>,
}

/// Replaces the product rows of a deal or lead.
async fn put_rows(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(body): Json<PutRows>) -> AppResult<Json<Value>> {
    let row = load_row(&state, id, false).await?;
    require(&state, &auth, row.entity_type, "write", row.assigned_to).await?;
    if crate::accounting::invoices::locked(&state, id).await? {
        return Err(AppError::bad("this invoice is in the books, so its products can't change; void it in Accounting first"));
    }
    if !super::entity_type_by_id(&state, row.entity_type).await?.has_amount {
        return Err(AppError::bad("this record type has no products"));
    }
    if body.rows.len() > 500 {
        return Err(AppError::bad("at most 500 product rows"));
    }
    for r in &body.rows {
        if r.name.trim().is_empty() || r.name.chars().count() > 300 {
            return Err(AppError::bad("every row needs a product name (max 300 characters)"));
        }
        if r.price.is_sign_negative() || r.quantity <= Decimal::ZERO || r.discount_value.is_sign_negative() {
            return Err(AppError::bad("price and discount cannot be negative, and quantity must be positive"));
        }
        if !["percent", "amount"].contains(&r.discount_kind.as_str()) {
            return Err(AppError::bad("discount must be percent or amount"));
        }
        if r.tax_rate.is_some_and(|t| t.is_sign_negative() || t > HUNDRED) {
            return Err(AppError::bad("tax rate must be between 0 and 100"));
        }
        if r.cost.is_some_and(|c| c.is_sign_negative()) {
            return Err(AppError::bad("cost cannot be negative"));
        }
    }
    // Rows from the catalogue without a cost take the product's.
    let currency = row.currency.clone().unwrap_or(base_currency(&state).await?);
    let mut costs = Vec::with_capacity(body.rows.len());
    for r in &body.rows {
        costs.push(match (r.cost, r.product_id) {
            (Some(c), _) => Some(c),
            (None, Some(p)) => unit_cost(&state, p, &currency).await?,
            _ => None,
        });
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM crm_item_products WHERE item_id = $1").bind(id).execute(&mut *tx).await?;
    for (i, r) in body.rows.iter().enumerate() {
        sqlx::query(
            "INSERT INTO crm_item_products (item_id, product_id, name, price, quantity, unit, discount_kind, discount_value, tax_rate, tax_included, sort, cost)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(id)
        .bind(r.product_id)
        .bind(r.name.trim())
        .bind(r.price)
        .bind(r.quantity)
        .bind(&r.unit)
        .bind(&r.discount_kind)
        .bind(r.discount_value)
        .bind(r.tax_rate)
        .bind(r.tax_included)
        .bind(i as i32)
        .bind(costs[i])
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE crm_items SET is_manual_amount = FALSE, updated_at = now() WHERE id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    let t = totals(&state, id).await?;
    timeline(&state.db, id, "products", Some(auth.id), None, json!({ "count": body.rows.len(), "total": t.total.to_string(), "currency": row.currency })).await?;
    broadcast(&state, row.entity_type, id, false).await;
    get_rows(State(state), auth, Path(id)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }
    fn row(price: &str, qty: &str, disc_kind: &str, disc: &str, tax: Option<&str>, incl: bool) -> ProductRow {
        ProductRow {
            id: None,
            product_id: None,
            name: "x".into(),
            price: d(price),
            quantity: d(qty),
            unit: "pcs".into(),
            discount_kind: disc_kind.into(),
            discount_value: d(disc),
            tax_rate: tax.map(d),
            tax_included: incl,
            cost: None,
        }
    }

    #[test]
    fn tax_excluded_with_percent_discount() {
        // 100 − 10% = 90 net; 18% tax = 16.20; × 3
        let l = line(&row("100", "3", "percent", "10", Some("18"), false));
        assert_eq!((l.net, l.tax, l.total), (d("270.00"), d("48.60"), d("318.60")));
    }

    #[test]
    fn tax_included_splits_gross() {
        // 120 gross incl. 20% → net 100, tax 20
        let l = line(&row("120", "1", "percent", "0", Some("20"), true));
        assert_eq!((l.net, l.tax, l.total), (d("100.00"), d("20.00"), d("120.00")));
    }

    #[test]
    fn amount_discount_never_goes_negative() {
        let l = line(&row("50", "2", "amount", "80", None, false));
        assert_eq!(l.total, d("0.00"));
    }

    #[test]
    fn totals_add_up() {
        let t = sum(&[row("10", "2", "percent", "0", Some("10"), false), row("5", "1", "amount", "1", None, false)]);
        assert_eq!((t.subtotal, t.tax, t.total, t.discount), (d("24.00"), d("2.00"), d("26.00"), d("1.00")));
    }
}
