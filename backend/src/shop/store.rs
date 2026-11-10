//! The public shop: catalog, product pages, photos, checkout and the customer's order page.
//! No sign-in. Nothing here is shown while the shop is closed.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{settings, Settings, ORDER};
use crate::{
    auth::{tokens::random_token, ClientIp},
    crm::{
        automation::system_auth,
        broadcast, entity_type_by_id,
        items::{create_item, normalize},
        money::{line, sum, totals, ProductRow},
        CONTACT,
    },
    error::{AppError, AppResult},
    inventory::{adjust, Reason},
    notify::{notify, NewNotification},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/public/shop", get(shop))
        .route("/public/shop/products", get(products))
        .route("/public/shop/products/{slug}", get(product))
        .route("/public/shop/images/{slug}", get(image))
        .route("/public/shop/checkout", post(checkout))
        .route("/public/shop/orders/{token}", get(order))
}

async fn open_shop(state: &AppState) -> AppResult<Settings> {
    let s = settings(state).await?;
    if !s.enabled {
        return Err(AppError::NotFound("shop"));
    }
    Ok(s)
}

async fn company(state: &AppState) -> Option<String> {
    let v: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'company_name'").fetch_optional(&state.db).await.ok().flatten();
    v.and_then(|v| v.as_str().map(str::to_string)).filter(|s| !s.is_empty())
}

async fn shop(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let s = open_shop(&state).await?;
    let categories: Vec<(String, String)> = sqlx::query_as(
        "SELECT c.name, c.slug FROM shop_categories c
         WHERE EXISTS (SELECT 1 FROM crm_products p WHERE p.shop_category_id = c.id AND p.in_shop AND p.is_active AND p.currency = $1)
         ORDER BY c.sort, c.name",
    )
    .bind(&s.currency)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "name": s.name,
        "intro": s.intro,
        "currency": s.currency,
        "company": company(&state).await,
        "terms": s.terms,
        "payment_instructions": s.payment_instructions,
        "categories": categories.into_iter().map(|(name, slug)| json!({ "name": name, "slug": slug })).collect::<Vec<_>>(),
    })))
}

#[derive(FromRow)]
struct Listed {
    id: Uuid,
    slug: String,
    name: String,
    description: Option<String>,
    price: Decimal,
    tax_rate: Option<Decimal>,
    tax_included: bool,
    unit: String,
    stock: Option<Decimal>,
    category: Option<String>,
    has_image: bool,
    parent_id: Option<Uuid>,
    props: Value,
}

/// The warehouse the shop sells from.
pub(crate) const SHOP_WAREHOUSE: &str = "COALESCE((SELECT warehouse_id FROM shop_settings), (SELECT id FROM inv_warehouses WHERE is_default))";

/// Stock shown and sold is what the shop's warehouse holds.
const LISTED_SELECT: &str = "SELECT p.id, p.slug, p.name, COALESCE(p.description, pp.description) AS description, p.price, p.tax_rate, p.tax_included, p.unit,
    CASE WHEN p.stock IS NULL THEN NULL ELSE COALESCE((SELECT quantity FROM inv_stock s WHERE s.product_id = p.id
        AND s.warehouse_id = COALESCE((SELECT warehouse_id FROM shop_settings), (SELECT id FROM inv_warehouses WHERE is_default))), 0) END AS stock,
    c.slug AS category, COALESCE(p.image_file_id, pp.image_file_id) IS NOT NULL AS has_image, p.parent_id, p.props
    FROM crm_products p LEFT JOIN shop_categories c ON c.id = p.shop_category_id
    LEFT JOIN crm_products pp ON pp.id = p.parent_id
    WHERE p.in_shop AND p.is_active AND p.slug IS NOT NULL AND p.currency = $1 AND (p.parent_id IS NULL OR (pp.in_shop AND pp.is_active))";

fn row_of(p: &Listed, quantity: Decimal) -> ProductRow {
    ProductRow {
        id: None,
        product_id: Some(p.id),
        name: p.name.clone(),
        price: p.price,
        quantity,
        unit: p.unit.clone(),
        discount_kind: "percent".into(),
        discount_value: Decimal::ZERO,
        tax_rate: p.tax_rate,
        tax_included: p.tax_included,
    }
}

#[derive(Serialize)]
struct PublicProduct {
    slug: String,
    name: String,
    description: Option<String>,
    /// What one unit costs, tax included.
    price: Decimal,
    tax_rate: Option<Decimal>,
    unit: String,
    category: Option<String>,
    image: Option<String>,
    /// Units left when stock is counted.
    stock: Option<Decimal>,
    /// A product with variants: how many, and `price` is the lowest of theirs.
    variants: usize,
}

fn public(p: Listed) -> PublicProduct {
    let price = line(&row_of(&p, Decimal::ONE)).total;
    PublicProduct {
        variants: 0,
        image: p.has_image.then(|| format!("/api/public/shop/images/{}", p.slug)),
        price,
        tax_rate: p.tax_rate,
        unit: p.unit,
        category: p.category,
        stock: p.stock.map(|s| s.normalize()),
        description: p.description,
        name: p.name,
        slug: p.slug,
    }
}

#[derive(Deserialize)]
struct ListQuery {
    category: Option<String>,
    q: Option<String>,
}

async fn products(State(state): State<AppState>, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let s = open_shop(&state).await?;
    let term = q.q.map(|t| t.trim().to_lowercase()).filter(|t| !t.is_empty()).map(|t| format!("%{}%", t.replace('%', "\\%").replace('_', "\\_")));
    let items: Vec<Listed> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{LISTED_SELECT} AND ($2::text IS NULL OR c.slug = $2) AND ($3::text IS NULL OR lower(p.name) LIKE $3 OR lower(p.description) LIKE $3)
         ORDER BY p.name LIMIT 500"
    )))
    .bind(&s.currency)
    .bind(q.category.filter(|c| !c.is_empty()))
    .bind(term)
    .fetch_all(&state.db)
    .await?;
    // Products with variants show once: from the lowest price, with all their stock.
    let ids: Vec<Uuid> = items.iter().map(|p| p.id).collect();
    let variants: Vec<Listed> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{LISTED_SELECT} AND p.parent_id = ANY($2)")))
        .bind(&s.currency)
        .bind(&ids)
        .fetch_all(&state.db)
        .await?;
    let out: Vec<PublicProduct> = items
        .into_iter()
        .filter(|p| p.parent_id.is_none())
        .map(|p| {
            let id = p.id;
            let mut out = public(p);
            let mine: Vec<PublicProduct> = variants.iter().filter(|v| v.parent_id == Some(id)).map(|v| public(clone_listed(v))).collect();
            if !mine.is_empty() {
                out.variants = mine.len();
                out.price = mine.iter().map(|v| v.price).min().unwrap_or(out.price);
                let counted: Vec<Decimal> = mine.iter().filter_map(|v| v.stock).collect();
                out.stock = if counted.len() == mine.len() { Some(counted.iter().sum::<Decimal>().normalize()) } else { None };
            }
            out
        })
        .collect();
    Ok(Json(json!({ "items": out, "currency": s.currency })))
}

fn clone_listed(v: &Listed) -> Listed {
    Listed {
        id: v.id,
        slug: v.slug.clone(),
        name: v.name.clone(),
        description: v.description.clone(),
        price: v.price,
        tax_rate: v.tax_rate,
        tax_included: v.tax_included,
        unit: v.unit.clone(),
        stock: v.stock,
        category: v.category.clone(),
        has_image: v.has_image,
        parent_id: v.parent_id,
        props: v.props.clone(),
    }
}

async fn listed(state: &AppState, s: &Settings, slug: &str) -> AppResult<Listed> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{LISTED_SELECT} AND p.slug = $2")))
        .bind(&s.currency)
        .bind(slug)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("product"))
}

/// A product page. A variant's address shows its parent's page with that variant chosen.
async fn product(State(state): State<AppState>, Path(slug): Path<String>) -> AppResult<Json<Value>> {
    let s = open_shop(&state).await?;
    let mut p = listed(&state, &s, &slug).await?;
    let mut selected = None;
    if let Some(parent) = p.parent_id {
        selected = Some(p.slug.clone());
        let parent_slug: Option<String> = sqlx::query_scalar("SELECT slug FROM crm_products WHERE id = $1").bind(parent).fetch_one(&state.db).await?;
        p = listed(&state, &s, &parent_slug.ok_or(AppError::NotFound("product"))?).await?;
    }
    let defs = crate::crm::catalog::props(&state).await?;
    let variants: Vec<Listed> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{LISTED_SELECT} AND p.parent_id = $2 ORDER BY p.name")))
        .bind(&s.currency)
        .bind(p.id)
        .fetch_all(&state.db)
        .await?;
    // What the customer picks: each variant property with the values variants have.
    let axes: Vec<Value> = defs
        .iter()
        .filter(|d| d.is_variant)
        .filter_map(|d| {
            let key = d.id.to_string();
            let vals: Vec<&String> = d.options.iter().filter(|o| variants.iter().any(|v| v.props.get(&key).and_then(Value::as_str) == Some(o.as_str()))).collect();
            (!vals.is_empty()).then(|| json!({ "id": key, "name": d.name, "values": vals }))
        })
        .collect();
    // Properties shown on the page (the product's own, not the variant ones).
    let shown: Vec<Value> = defs
        .iter()
        .filter(|d| d.in_shop && !d.is_variant)
        .filter_map(|d| p.props.get(d.id.to_string()).map(|v| json!({ "name": d.name, "value": v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()) })))
        .collect();
    let variant_values = |v: &Listed| -> Value { json!(defs.iter().filter(|d| d.is_variant).filter_map(|d| v.props.get(d.id.to_string()).map(|x| (d.id.to_string(), x.clone()))).collect::<Map<String, Value>>()) };
    let vs: Vec<Value> = variants
        .iter()
        .map(|v| {
            let mut j = serde_json::to_value(public(clone_listed(v))).unwrap();
            j["values"] = variant_values(v);
            j
        })
        .collect();
    let mut product = public(p);
    if !vs.is_empty() {
        product.variants = vs.len();
        product.price = vs.iter().filter_map(|v| v["price"].as_str().and_then(|x| Decimal::from_str_exact(x).ok())).min().unwrap_or(product.price);
    }
    Ok(Json(json!({ "product": product, "variants": vs, "axes": axes, "props": shown, "selected": selected, "currency": s.currency })))
}

async fn image(State(state): State<AppState>, Path(slug): Path<String>) -> AppResult<Response> {
    let s = open_shop(&state).await?;
    // A variant without a photo of its own shows its parent's.
    let file: Option<Uuid> = sqlx::query_scalar(
        "SELECT COALESCE(p.image_file_id, pp.image_file_id) FROM crm_products p LEFT JOIN crm_products pp ON pp.id = p.parent_id
         WHERE p.slug = $1 AND p.in_shop AND p.is_active AND p.currency = $2",
    )
        .bind(&slug)
        .bind(&s.currency)
        .fetch_optional(&state.db)
        .await?
        .flatten();
    let file = file.ok_or(AppError::NotFound("photo"))?;
    let (_, mime, _, bytes) = crate::routes::files::read_bytes(&state, file).await?;
    if !["image/png", "image/jpeg", "image/webp", "image/gif"].contains(&mime.as_str()) {
        return Err(AppError::NotFound("photo"));
    }
    let mut r = bytes.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=300"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'"));
    Ok(r)
}

// ---------------------------------------------------------------------------
// Checkout
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CartLine {
    slug: String,
    quantity: f64,
}

#[derive(Deserialize)]
struct Checkout {
    items: Vec<CartLine>,
    name: String,
    email: String,
    #[serde(default)]
    phone: String,
    address: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    accept_terms: bool,
    /// Hidden decoy field: people leave it empty.
    #[serde(default)]
    website: String,
}

async fn limited(state: &AppState, key: String, max: i64, secs: i64) -> AppResult<bool> {
    let mut redis = state.redis.clone();
    let n: i64 = redis.incr(&key, 1).await?;
    if n == 1 {
        let _: () = redis.expire(&key, secs).await?;
    }
    Ok(n > max)
}

fn text(v: &str, what: &str, max: usize, needed: bool) -> AppResult<Option<String>> {
    let t = v.trim();
    if t.is_empty() {
        return if needed { Err(AppError::bad(format!("please enter your {what}"))) } else { Ok(None) };
    }
    if t.chars().count() > max {
        return Err(AppError::bad(format!("the {what} can be at most {max} characters")));
    }
    Ok(Some(t.to_string()))
}

fn valid_email(e: &str) -> bool {
    let Some((local, domain)) = e.split_once('@') else { return false };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && e.len() <= 254 && !e.contains(char::is_whitespace)
}

/// "Ann Marie Lee" → ("Ann Marie", "Lee").
fn split_name(n: &str) -> (String, Option<String>) {
    match n.trim().rsplit_once(char::is_whitespace) {
        Some((first, last)) => (first.trim().to_string(), Some(last.to_string())),
        None => (n.trim().to_string(), None),
    }
}

async fn checkout(State(state): State<AppState>, ClientIp(ip): ClientIp, Json(b): Json<Checkout>) -> AppResult<Json<Value>> {
    let s = open_shop(&state).await?;
    // Bots fill every field: answer as if it worked, store nothing.
    if !b.website.is_empty() {
        return Ok(Json(json!({ "token": random_token(24), "number": 0 })));
    }
    let name = text(&b.name, "name", 200, true)?.unwrap();
    let email = text(&b.email, "email", 254, true)?.unwrap().to_lowercase();
    if !valid_email(&email) {
        return Err(AppError::bad("please enter a valid email address"));
    }
    let phone = text(&b.phone, "phone", 40, false)?;
    let address = text(&b.address, "delivery address", 1000, true)?.unwrap();
    let note = text(&b.note, "note", 2000, false)?;
    if s.terms.is_some() && !b.accept_terms {
        return Err(AppError::bad("please accept the terms to place your order"));
    }
    if b.items.is_empty() {
        return Err(AppError::bad("your cart is empty"));
    }
    if b.items.len() > 50 {
        return Err(AppError::bad("at most 50 different products in one order"));
    }
    // The same product twice in the cart counts once, with the quantities added up.
    let mut cart: Vec<(String, Decimal)> = Vec::new();
    for l in &b.items {
        if !(1.0..=999.0).contains(&l.quantity) || l.quantity.fract() != 0.0 {
            return Err(AppError::bad("quantities must be whole numbers from 1 to 999"));
        }
        let quantity = Decimal::from(l.quantity as i64);
        match cart.iter_mut().find(|(s, _)| *s == l.slug) {
            Some((_, q)) => *q += quantity,
            None => cart.push((l.slug.clone(), quantity)),
        }
    }

    // Counted once the form is filled in properly, so a customer correcting typos is not
    // locked out; checking costs nothing, ordering writes.
    let who = ip.unwrap_or_else(|| "unknown".into());
    if limited(&state, format!("shop:checkout:ip:{who}"), 30, 600).await?
        || limited(&state, format!("shop:checkout:email:{email}"), 5, 600).await?
        || limited(&state, "shop:checkout:all".into(), 1000, 3600).await?
    {
        return Err(AppError::TooManyRequests("too many orders in a short time; please try again later".into()));
    }

    // Take the units off the shelf, all or nothing.
    let warehouse: Uuid = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT {SHOP_WAREHOUSE}"))).fetch_one(&state.db).await?;
    let mut rows: Vec<ProductRow> = Vec::new();
    let mut moved: Vec<Uuid> = Vec::new();
    let mut tx = state.db.begin().await?;
    for (slug, q) in &cart {
        let p: Listed = sqlx::query_as(sqlx::AssertSqlSafe(format!("{LISTED_SELECT} AND p.slug = $2 FOR UPDATE OF p")))
            .bind(&s.currency)
            .bind(slug)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| AppError::bad("a product in your cart is no longer sold; remove it and try again"))?;
        let has_variants: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_products WHERE parent_id = $1 AND in_shop AND is_active)")
            .bind(p.id)
            .fetch_one(&mut *tx)
            .await?;
        if has_variants {
            return Err(AppError::bad(format!("choose which “{}” you want on its page", p.name)));
        }
        if let Some(left) = p.stock {
            if left < *q {
                return Err(AppError::bad(if left <= Decimal::ZERO {
                    format!("“{}” is sold out", p.name)
                } else {
                    format!("only {} of “{}” left", left.normalize(), p.name)
                }));
            }
            moved.push(adjust(&mut tx, p.id, warehouse, -*q, false, Reason { kind: "order", document: None, item: None, user: None }).await?);
        }
        rows.push(row_of(&p, *q));
    }
    tx.commit().await?;

    let customer = Customer { name: &name, email: &email, phone: phone.as_deref(), address: &address, note: note.as_deref() };
    match place(&state, &s, &rows, &customer, warehouse, &moved).await {
        Ok(v) => Ok(Json(v)),
        Err(e) => {
            // The order did not go through: put the units back.
            if !moved.is_empty() {
                let counted: Vec<Uuid> = sqlx::query_scalar("SELECT product_id FROM inv_movements WHERE id = ANY($1)").bind(&moved).fetch_all(&state.db).await.unwrap_or_default();
                if let Ok(mut tx) = state.db.begin().await {
                    for r in rows.iter().filter(|r| r.product_id.is_some_and(|p| counted.contains(&p))) {
                        let _ = adjust(&mut tx, r.product_id.unwrap(), warehouse, r.quantity, true, Reason { kind: "order_failed", document: None, item: None, user: None }).await;
                    }
                    let _ = tx.commit().await;
                }
            }
            Err(e)
        }
    }
}

/// Creates the contact (or finds it by email), the order record and its rows.
struct Customer<'a> {
    name: &'a str,
    email: &'a str,
    phone: Option<&'a str>,
    address: &'a str,
    note: Option<&'a str>,
}

async fn place(state: &AppState, s: &Settings, rows: &[ProductRow], c: &Customer<'_>, warehouse: Uuid, moved: &[Uuid]) -> AppResult<Value> {
    let Customer { name, email, phone, address, note } = *c;
    let auth = system_auth();
    // In turn among the handlers who still work here.
    let active: Vec<Uuid> = sqlx::query_scalar("SELECT h FROM unnest($1::uuid[]) WITH ORDINALITY u(h, n) JOIN users ON users.id = h AND users.is_active ORDER BY n")
        .bind(&s.handlers)
        .fetch_all(&state.db)
        .await?;
    let handler = match active.len() {
        0 => None,
        n => {
            let mut redis = state.redis.clone();
            let i: i64 = redis.incr("shop:handler:rr", 1).await?;
            Some(active[((i - 1) as usize) % n])
        }
    };
    let contact: Option<Uuid> = sqlx::query_scalar(
        "SELECT i.id FROM crm_multifields m JOIN crm_items i ON i.id = m.item_id
         WHERE m.kind = 'email' AND m.normalized = $1 AND i.entity_type = $2 AND i.deleted_at IS NULL ORDER BY i.updated_at DESC LIMIT 1",
    )
    .bind(normalize("email", email))
    .bind(CONTACT)
    .fetch_optional(&state.db)
    .await?;
    let contact = match contact {
        Some(c) => c,
        None => {
            let (first, last) = split_name(name);
            let mut c = Map::new();
            c.insert("first_name".into(), json!(first));
            if let Some(l) = last {
                c.insert("last_name".into(), json!(l));
            }
            let mut mf = vec![json!({ "kind": "email", "value_type": "work", "value": email })];
            if let Some(p) = phone {
                mf.push(json!({ "kind": "phone", "value_type": "mobile", "value": p }));
            }
            c.insert("multifields".into(), json!(mf));
            c.insert("source".into(), json!("website"));
            c.insert("source_details".into(), json!(s.name));
            // Nobody handles orders: no responsible person.
            c.insert("assigned_to".into(), json!(handler));
            let ct = entity_type_by_id(state, CONTACT).await?;
            let id = create_item(state, &auth, &ct, c, None).await?;
            broadcast(state, CONTACT, id, false).await;
            id
        }
    };

    let t = entity_type_by_id(state, ORDER).await?;
    let mut comments = format!("Deliver to:\n{address}");
    if let Some(p) = phone {
        comments.push_str(&format!("\nPhone: {p}"));
    }
    if let Some(n) = note {
        comments.push_str(&format!("\n\nFrom the customer:\n{n}"));
    }
    let mut body = Map::new();
    body.insert("title".into(), json!(format!("Shop order: {name}")));
    body.insert("contact_ids".into(), json!([contact]));
    body.insert("currency".into(), json!(s.currency));
    body.insert("comments".into(), json!(comments));
    body.insert("source".into(), json!("website"));
    body.insert("source_details".into(), json!(s.name));
    body.insert("assigned_to".into(), json!(handler));
    let id = create_item(state, &auth, &t, body, None).await?;
    for (i, r) in rows.iter().enumerate() {
        sqlx::query(
            "INSERT INTO crm_item_products (item_id, product_id, name, price, quantity, unit, discount_kind, discount_value, tax_rate, tax_included, sort)
             VALUES ($1, $2, $3, $4, $5, $6, 'percent', 0, $7, $8, $9)",
        )
        .bind(id)
        .bind(r.product_id)
        .bind(&r.name)
        .bind(r.price)
        .bind(r.quantity)
        .bind(&r.unit)
        .bind(r.tax_rate)
        .bind(r.tax_included)
        .bind(i as i32)
        .execute(&state.db)
        .await?;
    }
    sqlx::query("UPDATE crm_items SET is_manual_amount = FALSE WHERE id = $1").bind(id).execute(&state.db).await?;
    let tt = totals(state, id).await?;
    let number: i64 = sqlx::query_scalar("UPDATE crm_items SET title = 'Order ' || number || ': ' || $2 WHERE id = $1 RETURNING number")
        .bind(id)
        .bind(name)
        .fetch_one(&state.db)
        .await?;
    let token = random_token(24);
    sqlx::query("INSERT INTO shop_orders (item_id, token, name, email, phone, address, note, warehouse_id) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)")
        .bind(id)
        .bind(&token)
        .bind(name)
        .bind(email)
        .bind(phone)
        .bind(address)
        .bind(note)
        .bind(warehouse)
        .execute(&state.db)
        .await?;
    sqlx::query("UPDATE inv_movements SET item_id = $1 WHERE id = ANY($2)").bind(id).bind(moved).execute(&state.db).await?;
    crate::crm::timeline(&state.db, id, "shop_order", None, None, json!({ "shop": s.name, "total": tt.total.to_string(), "currency": s.currency })).await?;
    broadcast(state, ORDER, id, false).await;

    // The customer's confirmation.
    let currency = s.currency.clone().unwrap_or_default();
    let link = format!("{}/shop/order/{token}", state.config.public_url);
    let mut lines: Vec<String> = vec![format!("Hi {},", split_name(name).0), "Thank you for your order. Here is what you ordered:".to_string()];
    lines.push(rows.iter().map(|r| format!("{} × {} — {} {currency}", r.quantity.normalize(), r.name, line(r).total)).collect::<Vec<_>>().join("\n"));
    lines.push(format!("Total: {} {currency}{}", tt.total, if tt.tax > Decimal::ZERO { format!(" (including {} {currency} tax)", tt.tax) } else { String::new() }));
    lines.push(format!("We will deliver to:\n{address}"));
    if let Some(p) = &s.payment_instructions {
        lines.push(format!("How to pay:\n{}", p.replace("{{order}}", &number.to_string()).replace("{{total}}", &format!("{} {currency}", tt.total))));
    }
    let subject = format!("Your order {number} from {}", s.name);
    let mail = crate::mail::customer_notice(email, &s.name, &subject, &format!("Order {number} received"), &lines, Some(("See your order", &link)));
    if let Err(e) = crate::jobs::send_email(state, mail).await {
        tracing::warn!(order = %id, error = ?e, "could not queue the order confirmation");
    }
    if let Some(h) = handler {
        notify(state, &[h], NewNotification {
            kind: "shop.order",
            title: format!("New order {number} from {name}: {} {currency}", tt.total),
            body: None,
            link: Some(format!("/crm/order/{id}")),
            actor: None,
        })
        .await;
    }
    Ok(json!({ "token": token, "number": number }))
}

// ---------------------------------------------------------------------------
// The customer's order page
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct OrderRow {
    item_id: Uuid,
    number: i64,
    currency: String,
    stage: String,
    semantics: String,
    stage_sort: i32,
    name: String,
    email: String,
    phone: Option<String>,
    address: String,
    note: Option<String>,
    created_at: DateTime<Utc>,
}

/// Works while the shop is closed too: customers keep their order links.
async fn order(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let o: OrderRow = sqlx::query_as(
        "SELECT o.item_id, i.number, i.currency, s.name AS stage, s.semantics, s.sort AS stage_sort, o.name, o.email, o.phone, o.address, o.note, o.created_at
         FROM shop_orders o JOIN crm_items i ON i.id = o.item_id JOIN crm_stages s ON s.id = i.stage_id
         WHERE o.token = $1 AND i.deleted_at IS NULL",
    )
    .bind(&token)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("order"))?;
    let s = settings(&state).await?;
    let rows = crate::crm::money::rows_of(&state, o.item_id).await?;
    let t = sum(&rows);
    // The way through the order's stages, to draw progress.
    let steps: Vec<(String, i32)> = sqlx::query_as(
        "SELECT s.name, s.sort FROM crm_stages s JOIN crm_items i ON i.category_id = s.category_id
         WHERE i.id = $1 AND s.semantics <> 'failure' ORDER BY s.sort",
    )
    .bind(o.item_id)
    .fetch_all(&state.db)
    .await?;
    let first = steps.first().map(|(_, sort)| *sort);
    Ok(Json(json!({
        "number": o.number,
        "created_at": o.created_at,
        "currency": o.currency,
        "stage": { "name": o.stage, "semantics": o.semantics },
        "steps": steps.iter().map(|(n, sort)| json!({ "name": n, "done": o.semantics != "failure" && *sort <= o.stage_sort })).collect::<Vec<_>>(),
        "rows": rows.iter().map(|r| json!({ "name": r.name, "quantity": r.quantity.normalize(), "unit": r.unit, "total": line(r).total })).collect::<Vec<_>>(),
        "totals": t,
        "customer": { "name": o.name, "email": o.email, "phone": o.phone, "address": o.address, "note": o.note },
        "shop": s.name,
        // How to pay, until the order moves past its first stage.
        "payment_instructions": if Some(o.stage_sort) == first && o.semantics == "process" {
            s.payment_instructions.map(|p| p.replace("{{order}}", &o.number.to_string()).replace("{{total}}", &format!("{} {}", t.total, o.currency)))
        } else {
            None
        },
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_split_at_the_last_space() {
        assert_eq!(split_name("Ann Marie Lee"), ("Ann Marie".into(), Some("Lee".into())));
        assert_eq!(split_name(" Cher "), ("Cher".into(), None));
    }

    #[test]
    fn emails() {
        assert!(valid_email("a.b@shop.example"));
        assert!(!valid_email("a@b"));
        assert!(!valid_email("@b.com"));
        assert!(!valid_email("a b@c.com"));
        assert!(!valid_email("a@.com"));
    }
}
