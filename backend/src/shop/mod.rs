//! Commerce: an online shop on top of the product catalog.
//!
//! Products marked "in the shop" are listed on the public shop pages (`/shop`) by category,
//! with a photo and, when stock is counted, how many are left. Checkout finds or creates the
//! customer's contact, creates an order (a CRM record of the built-in "order" type, with the
//! products as its rows), takes the units off the stock, emails the customer a confirmation
//! with a link to their order page and notifies the employee who handles it. Payment is by
//! bank transfer or on invoice: the confirmation carries the shop's payment instructions,
//! and employees move the order to "Paid" when the money arrives.
//!
//! Moving an order to a failure stage (Cancelled) puts its units back on the shelf once;
//! reopening it takes them again. The customer gets an email when their order changes stage.

mod extras;
mod store;

use axum::{
    extract::{Path, State},
    routing::{get, patch},
    Json, Router,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

/// The built-in order record type (see migration 0023).
pub const ORDER: i16 = 7;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/shop/settings", get(get_settings).patch(update_settings))
        .route("/shop/categories", get(list_categories).post(create_category))
        .route("/shop/categories/{id}", patch(update_category).delete(delete_category))
        .route("/shop/orders/{item}", get(order_details))
        .merge(store::router())
        .merge(extras::router())
}

fn require(auth: &AuthUser) -> AppResult<()> {
    auth.require(perm::SHOP)
}

/// Product editors pick categories too.
fn require_catalog(auth: &AuthUser) -> AppResult<()> {
    if auth.can(perm::SHOP) || auth.can(perm::CRM_PRODUCTS) || auth.can(perm::CRM_ADMIN) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow, Clone)]
pub(crate) struct Settings {
    pub enabled: bool,
    pub name: String,
    pub intro: Option<String>,
    pub currency: Option<String>,
    pub handlers: Vec<Uuid>,
    pub payment_instructions: Option<String>,
    pub terms: Option<String>,
    /// Where the shop sells from; None = the default warehouse.
    pub warehouse_id: Option<Uuid>,
}

pub(crate) async fn settings(state: &AppState) -> AppResult<Settings> {
    let mut s: Settings = sqlx::query_as("SELECT enabled, name, intro, currency, handlers, payment_instructions, terms, warehouse_id FROM shop_settings")
        .fetch_one(&state.db)
        .await?;
    if s.currency.is_none() {
        s.currency = Some(crate::crm::money::base_currency(state).await?);
    }
    Ok(s)
}

async fn get_settings(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require(&auth)?;
    settings_view(&state).await.map(Json)
}

async fn settings_view(state: &AppState) -> AppResult<Value> {
    let s = settings(state).await?;
    let (listed, other_currency): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE currency = $1), COUNT(*) FILTER (WHERE currency <> $1) FROM crm_products WHERE in_shop AND is_active",
    )
    .bind(&s.currency)
    .fetch_one(&state.db)
    .await?;
    let handlers: Vec<(Uuid, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, trim(first_name || ' ' || last_name), avatar_file_id FROM users WHERE id = ANY($1) ORDER BY array_position($1, id)",
    )
    .bind(&s.handlers)
    .fetch_all(&state.db)
    .await?;
    Ok(json!({
        "handlers": handlers.into_iter().map(|(id, name, avatar)| json!({ "id": id, "name": name, "avatar_file_id": avatar })).collect::<Vec<_>>(),
        "settings": s,
        "shop_url": format!("{}/shop", state.config.public_url),
        "listed": listed,
        "other_currency": other_currency,
    }))
}

#[derive(Deserialize)]
struct SettingsInput {
    enabled: Option<bool>,
    name: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    intro: Option<Option<String>>,
    currency: Option<String>,
    handlers: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    payment_instructions: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    terms: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    warehouse_id: Option<Option<Uuid>>,
}

fn long_text(v: Option<Option<String>>, max: usize, what: &str) -> AppResult<Option<Option<String>>> {
    match v {
        None => Ok(None),
        Some(t) => {
            let t = clean(t);
            if t.as_ref().is_some_and(|t| t.chars().count() > max) {
                return Err(AppError::bad(format!("{what} can be at most {max} characters")));
            }
            Ok(Some(t))
        }
    }
}

async fn update_settings(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SettingsInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let mut s = settings(&state).await?;
    if let Some(n) = &b.name {
        s.name = required("shop name", n, 120)?;
    }
    if let Some(c) = &b.currency {
        let c = c.trim().to_uppercase();
        crate::crm::money::rate_of(&state, &c).await?.ok_or_else(|| AppError::bad(format!("currency {c} is not enabled in the CRM")))?;
        s.currency = Some(c);
    }
    if let Some(h) = b.handlers {
        let mut h = h;
        h.dedup();
        let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND is_active").bind(&h).fetch_one(&state.db).await?;
        if active as usize != h.len() {
            return Err(AppError::bad("orders can only be handled by active employees"));
        }
        s.handlers = h;
    }
    if let Some(v) = long_text(b.intro, 2000, "the introduction")? {
        s.intro = v;
    }
    if let Some(v) = long_text(b.payment_instructions, 4000, "payment instructions")? {
        s.payment_instructions = v;
    }
    if let Some(v) = long_text(b.terms, 20000, "terms")? {
        s.terms = v;
    }
    if let Some(w) = b.warehouse_id {
        if let Some(id) = w {
            let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM inv_warehouses WHERE id = $1 AND is_active)").bind(id).fetch_one(&state.db).await?;
            if !ok {
                return Err(AppError::bad("that warehouse does not exist or is inactive"));
            }
        }
        s.warehouse_id = w;
    }
    if let Some(e) = b.enabled {
        s.enabled = e;
    }
    if s.enabled && s.payment_instructions.is_none() {
        return Err(AppError::bad("add payment instructions before opening the shop: customers need to know how to pay"));
    }
    sqlx::query(
        "UPDATE shop_settings SET enabled = $1, name = $2, intro = $3, currency = $4, handlers = $5, payment_instructions = $6, terms = $7, warehouse_id = $8, updated_at = now()",
    )
    .bind(s.enabled)
    .bind(&s.name)
    .bind(&s.intro)
    .bind(&s.currency)
    .bind(&s.handlers)
    .bind(&s.payment_instructions)
    .bind(&s.terms)
    .bind(s.warehouse_id)
    .execute(&state.db)
    .await?;
    crate::audit::record(&state.db, Some(auth.id), "shop.settings", "shop", None, json!({ "enabled": s.enabled }), None).await;
    settings_view(&state).await.map(Json)
}

// ---------------------------------------------------------------------------
// Categories
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct Category {
    id: Uuid,
    name: String,
    slug: String,
    sort: i32,
    products: i64,
}

const CATEGORY_SELECT: &str = "SELECT c.id, c.name, c.slug, c.sort,
    (SELECT COUNT(*) FROM crm_products p WHERE p.shop_category_id = c.id AND p.in_shop AND p.is_active) AS products
    FROM shop_categories c";

async fn categories(state: &AppState) -> AppResult<Vec<Category>> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("{CATEGORY_SELECT} ORDER BY c.sort, c.name"))).fetch_all(&state.db).await?)
}

async fn list_categories(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "items": categories(&state).await? })))
}

#[derive(Deserialize)]
struct CategoryInput {
    name: Option<String>,
    sort: Option<i32>,
}

async fn unique_category_slug(state: &AppState, name: &str, except: Option<Uuid>) -> AppResult<String> {
    let base = crate::routes::slugify(name, "category");
    for i in 1..1000 {
        let s = if i == 1 { base.clone() } else { format!("{base}-{i}") };
        let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM shop_categories WHERE slug = $1 AND id IS DISTINCT FROM $2)")
            .bind(&s)
            .bind(except)
            .fetch_one(&state.db)
            .await?;
        if !taken {
            return Ok(s);
        }
    }
    Ok(format!("{base}-{}", Uuid::new_v4().simple()))
}

async fn create_category(State(state): State<AppState>, auth: AuthUser, Json(b): Json<CategoryInput>) -> AppResult<Json<Value>> {
    require_catalog(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 120)?;
    let slug = unique_category_slug(&state, &name, None).await?;
    let sort = match b.sort {
        Some(s) => s,
        None => sqlx::query_scalar("SELECT COALESCE(MAX(sort), 0) + 10 FROM shop_categories").fetch_one(&state.db).await?,
    };
    sqlx::query("INSERT INTO shop_categories (name, slug, sort) VALUES ($1, $2, $3)").bind(&name).bind(&slug).bind(sort).execute(&state.db).await?;
    Ok(Json(json!({ "items": categories(&state).await? })))
}

/// Renaming keeps the address (links to the category keep working).
async fn update_category(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<CategoryInput>) -> AppResult<Json<Value>> {
    require_catalog(&auth)?;
    let name = b.name.as_deref().map(|n| required("name", n, 120)).transpose()?;
    let n = sqlx::query("UPDATE shop_categories SET name = COALESCE($2, name), sort = COALESCE($3, sort) WHERE id = $1")
        .bind(id)
        .bind(name)
        .bind(b.sort)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("category"));
    }
    Ok(Json(json!({ "items": categories(&state).await? })))
}

/// Its products stay in the shop, without a category.
async fn delete_category(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_catalog(&auth)?;
    sqlx::query("DELETE FROM shop_categories WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "items": categories(&state).await? })))
}

// ---------------------------------------------------------------------------
// An order's checkout details, for its record page
// ---------------------------------------------------------------------------

async fn order_details(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = crate::crm::items::load_row(&state, item, false).await?;
    crate::crm::access::require_read(&state, &auth, row.entity_type, item, row.assigned_to).await?;
    let o: Option<(String, String, String, Option<String>, String, Option<String>, bool)> =
        sqlx::query_as("SELECT token, name, email, phone, address, note, stock_returned FROM shop_orders WHERE item_id = $1")
            .bind(item)
            .fetch_optional(&state.db)
            .await?;
    let Some((token, name, email, phone, address, note, returned)) = o else { return Err(AppError::NotFound("shop order")) };
    Ok(Json(json!({
        "name": name, "email": email, "phone": phone, "address": address, "note": note,
        "stock_returned": returned,
        "order_url": format!("{}/shop/order/{token}", state.config.public_url),
    })))
}

// ---------------------------------------------------------------------------
// Orders changing stage
// ---------------------------------------------------------------------------

/// Called after an order record moves to another stage: returns or retakes stock, and tells
/// the customer. Errors are logged; the stage change itself has already happened.
pub async fn order_stage_changed(state: &AppState, item: Uuid) {
    if let Err(e) = stage_changed(state, item).await {
        tracing::warn!(item = %item, error = ?e, "shop order stage change follow-up failed");
    }
}

async fn stage_changed(state: &AppState, item: Uuid) -> AppResult<()> {
    let row: Option<(String, String, i64, String, String, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT i.stage_semantics, s.name, i.number, o.token, o.email, o.name, o.warehouse_id
         FROM crm_items i JOIN shop_orders o ON o.item_id = i.id JOIN crm_stages s ON s.id = i.stage_id WHERE i.id = $1",
    )
    .bind(item)
    .fetch_optional(&state.db)
    .await?;
    let Some((semantics, stage, number, token, email, name, warehouse)) = row else { return Ok(()) };

    // Stock: back on the shelf when cancelled, taken again when reopened. The flag flips
    // first, so two quick moves cannot return the units twice.
    let cancelled = semantics == "failure";
    let mut tx = state.db.begin().await?;
    let flipped = sqlx::query("UPDATE shop_orders SET stock_returned = $2 WHERE item_id = $1 AND stock_returned <> $2")
        .bind(item)
        .bind(cancelled)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    if flipped == 1 {
        let warehouse = match warehouse {
            Some(w) => w,
            None => crate::inventory::default_warehouse(&mut *tx).await?,
        };
        // Products whose stock is counted.
        let rows: Vec<(Uuid, Decimal)> = sqlx::query_as(
            "SELECT r.product_id, SUM(r.quantity) FROM crm_item_products r JOIN crm_products p ON p.id = r.product_id
             WHERE r.item_id = $1 AND p.stock IS NOT NULL GROUP BY r.product_id",
        )
        .bind(item)
        .fetch_all(&mut *tx)
        .await?;
        for (product, q) in rows {
            let (delta, kind) = if cancelled { (q, "order_cancelled") } else { (-q, "order_reopened") };
            // Reopening an order whose goods were sold meanwhile may leave the shelf short.
            crate::inventory::adjust(&mut tx, product, warehouse, delta, true, crate::inventory::Reason { kind, document: None, item: Some(item), user: None }).await?;
        }
    }
    tx.commit().await?;

    let s = settings(state).await?;
    let link = format!("{}/shop/order/{token}", state.config.public_url);
    let (heading, line) = match semantics.as_str() {
        "failure" => (format!("Order {number} was cancelled"), "Your order was cancelled. If you have already paid, we will refund you. Reply to this email if you have any questions.".to_string()),
        "success" => (format!("Order {number} is complete"), "Your order is complete. Thank you for shopping with us!".to_string()),
        _ => (format!("Order {number}: {stage}"), format!("Your order is now “{stage}”.")),
    };
    let first = name.split_whitespace().next().unwrap_or("there").to_string();
    let mail = crate::mail::customer_notice(&email, &s.name, &heading, &heading, &[format!("Hi {first},"), line], Some(("See your order", &link)));
    crate::jobs::send_email(state, mail).await?;
    Ok(())
}
