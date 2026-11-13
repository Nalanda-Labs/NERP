//! Catalog depth: product properties, variants and price types.
//!
//! - Properties (Material, Weight, Size…) are text, number or list; a product stores its
//!   values by property id. Variant properties are lists whose values make variants.
//! - A variant is a product of its own (name, SKU, price, stock, photo) whose `parent_id` is
//!   the product it varies; it carries one value of each variant property. Variants are made
//!   from the parent ("every combination of these sizes and colours") and copy its price,
//!   tax, unit, description and shop settings, to be changed one by one afterwards.
//! - Price types (Wholesale, Partner…) are extra prices beside each product's base price;
//!   product rows on deals can be filled with any of them.

use axum::{
    extract::{Path, State},
    routing::{get, patch, post},
    Json, Router,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;
use std::{collections::HashMap, str::FromStr};
use uuid::Uuid;

use super::money::{can_manage_products, unique_product_slug};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/product-props", get(list_props).post(create_prop))
        .route("/crm/product-props/{id}", patch(update_prop).delete(delete_prop))
        .route("/crm/price-types", get(list_price_types).post(create_price_type))
        .route("/crm/price-types/{id}", patch(update_price_type).delete(delete_price_type))
        .route("/crm/products/{id}/details", get(details))
        .route("/crm/products/{id}/props", axum::routing::put(put_props))
        .route("/crm/products/{id}/prices", axum::routing::put(put_prices))
        .route("/crm/products/{id}/variants", post(make_variants))
        .route("/crm/prices", post(prices_for))
        .route("/crm/catalog/sections/{id}/editors", get(section_editors).put(set_section_editors))
        .route("/crm/catalog/my-sections", get(my_sections))
}

// ---------------------------------------------------------------------------
// Section editors
// ---------------------------------------------------------------------------

async fn section_editors(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    can_manage_products(&auth)?;
    let users: Vec<(Uuid, String)> = sqlx::query!(r#"SELECT u.id AS "id!", trim(u.first_name || ' ' || u.last_name) AS "c1!" FROM catalog_section_editors e JOIN users u ON u.id = e.user_id WHERE e.category_id = $1 ORDER BY 2"#, id).map(|r| (r.id, r.c1)).fetch_all(&state.db).await?;
    let deps: Vec<(Uuid, String)> = sqlx::query!(r#"SELECT d.id, d.name FROM catalog_section_editors e JOIN departments d ON d.id = e.department_id WHERE e.category_id = $1 ORDER BY 2"#, id).map(|r| (r.id, r.name)).fetch_all(&state.db).await?;
    Ok(Json(json!({
        "users": users.into_iter().map(|(i, n)| json!({ "id": i, "name": n })).collect::<Vec<_>>(),
        "departments": deps.into_iter().map(|(i, n)| json!({ "id": i, "name": n })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct Editors {
    #[serde(default)]
    users: Vec<Uuid>,
    #[serde(default)]
    departments: Vec<Uuid>,
}

async fn set_section_editors(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Editors>) -> AppResult<Json<Value>> {
    can_manage_products(&auth)?;
    let exists: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM shop_categories WHERE id = $1) AS "v!""#, id).fetch_one(&state.db).await?;
    if !exists {
        return Err(AppError::NotFound("section"));
    }
    let u: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM users WHERE id = ANY($1) AND is_active AND NOT is_guest"#, &b.users).fetch_one(&state.db).await?;
    let d: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM departments WHERE id = ANY($1)"#, &b.departments).fetch_one(&state.db).await?;
    if u as usize != b.users.len() || d as usize != b.departments.len() || b.users.len() + b.departments.len() > 200 {
        return Err(AppError::bad("choose active employees and existing departments"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query!("DELETE FROM catalog_section_editors WHERE category_id = $1", id).execute(&mut *tx).await?;
    for x in &b.users {
        sqlx::query!("INSERT INTO catalog_section_editors (category_id, user_id) VALUES ($1, $2)", id, x).execute(&mut *tx).await?;
    }
    for x in &b.departments {
        sqlx::query!("INSERT INTO catalog_section_editors (category_id, department_id) VALUES ($1, $2)", id, x).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    section_editors(State(state), auth, Path(id)).await
}

/// What the caller may edit in the catalog: everything, or these sections.
async fn my_sections(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    if can_manage_products(&auth).is_ok() {
        return Ok(Json(json!({ "all": true, "sections": [] })));
    }
    let ids: Vec<Uuid> = sqlx::query_scalar!("WITH RECURSIVE mine AS (
             SELECT department_id AS id FROM user_departments WHERE user_id = $1
             UNION SELECT d.parent_id FROM departments d JOIN mine ON d.id = mine.id WHERE d.parent_id IS NOT NULL)
         SELECT DISTINCT category_id FROM catalog_section_editors WHERE user_id = $1 OR department_id IN (SELECT id FROM mine)", auth.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "all": false, "sections": ids })))
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow, Clone)]
pub struct Prop {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub options: Vec<String>,
    pub is_variant: bool,
    pub in_shop: bool,
    pub sort: i32,
}

pub async fn props(state: &AppState) -> AppResult<Vec<Prop>> {
    Ok(sqlx::query_as!(Prop, "SELECT id, name, kind, options, is_variant, in_shop, sort FROM crm_product_props ORDER BY sort, name").fetch_all(&state.db).await?)
}

async fn list_props(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "items": props(&state).await? })))
}

#[derive(Deserialize)]
struct PropInput {
    name: Option<String>,
    kind: Option<String>,
    options: Option<Vec<String>>,
    is_variant: Option<bool>,
    in_shop: Option<bool>,
}

fn clean_options(o: Vec<String>) -> AppResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for v in o {
        let v = v.trim().to_string();
        if v.is_empty() || out.iter().any(|x| x.eq_ignore_ascii_case(&v)) {
            continue;
        }
        if v.chars().count() > 100 {
            return Err(AppError::bad("values can be at most 100 characters"));
        }
        out.push(v);
    }
    if out.len() > 100 {
        return Err(AppError::bad("at most 100 values"));
    }
    Ok(out)
}

async fn create_prop(State(state): State<AppState>, auth: AuthUser, Json(b): Json<PropInput>) -> AppResult<Json<Value>> {
    can_manage_products(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 100)?;
    let kind = b.kind.unwrap_or_else(|| "text".into());
    if !["text", "number", "list"].contains(&kind.as_str()) {
        return Err(AppError::bad("kind must be text, number or list"));
    }
    let options = clean_options(b.options.unwrap_or_default())?;
    let is_variant = b.is_variant.unwrap_or(false);
    if kind == "list" && options.is_empty() {
        return Err(AppError::bad("a list needs at least one value"));
    }
    if is_variant && kind != "list" {
        return Err(AppError::bad("only a list of values can make variants"));
    }
    let sort: i32 = sqlx::query_scalar!(r#"SELECT COALESCE(MAX(sort), 0) + 10 AS "v!" FROM crm_product_props"#).fetch_one(&state.db).await?;
    sqlx::query!("INSERT INTO crm_product_props (name, kind, options, is_variant, in_shop, sort) VALUES ($1, $2, $3, $4, $5, $6)", name, &kind, &(if kind == "list" { options } else { vec![] }), is_variant, b.in_shop.unwrap_or(true), sort)
        .execute(&state.db)
        .await?;
    list_props(State(state), auth).await
}

/// The kind cannot change (values already stored would not fit). Values removed from a list
/// stay on products that have them until edited.
async fn update_prop(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PropInput>) -> AppResult<Json<Value>> {
    can_manage_products(&auth)?;
    let (kind, is_variant): (String, bool) = sqlx::query!(r#"SELECT kind, is_variant FROM crm_product_props WHERE id = $1"#, id).map(|r| (r.kind, r.is_variant))
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("property"))?;
    if b.kind.as_ref().is_some_and(|k| *k != kind) {
        return Err(AppError::bad("a property's kind cannot change; make a new one"));
    }
    if b.is_variant.is_some_and(|v| v != is_variant) {
        let used: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM crm_products WHERE parent_id IS NOT NULL AND props ? $1) AS "v!""#, id.to_string()).fetch_one(&state.db).await?;
        if used {
            return Err(AppError::bad("variants already use this property"));
        }
        if kind != "list" {
            return Err(AppError::bad("only a list of values can make variants"));
        }
    }
    let name = b.name.as_deref().map(|n| required("name", n, 100)).transpose()?;
    let options = b.options.map(clean_options).transpose()?;
    if kind == "list" && options.as_ref().is_some_and(|o| o.is_empty()) {
        return Err(AppError::bad("a list needs at least one value"));
    }
    sqlx::query!("UPDATE crm_product_props SET name = COALESCE($2, name), options = COALESCE($3, options), is_variant = COALESCE($4, is_variant), in_shop = COALESCE($5, in_shop) WHERE id = $1", id, name, options.as_deref(), b.is_variant, b.in_shop)
    .execute(&state.db)
    .await?;
    list_props(State(state), auth).await
}

async fn delete_prop(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    can_manage_products(&auth)?;
    let used: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM crm_products WHERE parent_id IS NOT NULL AND props ? $1) AS "v!""#, id.to_string()).fetch_one(&state.db).await?;
    if used {
        return Err(AppError::bad("variants use this property; delete them first"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query!("UPDATE crm_products SET props = props - $1 WHERE props ? $1", id.to_string()).execute(&mut *tx).await?;
    sqlx::query!("DELETE FROM crm_product_props WHERE id = $1", id).execute(&mut *tx).await?;
    tx.commit().await?;
    list_props(State(state), auth).await
}

/// Checks property values against their definitions. Null or empty removes a value.
fn clean_props(defs: &[Prop], input: &Map<String, Value>, allow_variant: bool) -> AppResult<Map<String, Value>> {
    let mut out = Map::new();
    for (k, v) in input {
        let def = defs.iter().find(|d| d.id.to_string() == *k).ok_or_else(|| AppError::bad("unknown property"))?;
        if def.is_variant && !allow_variant {
            return Err(AppError::bad(format!("“{}” is set by making variants", def.name)));
        }
        let text = match v {
            Value::Null => continue,
            Value::String(s) => s.trim().to_string(),
            Value::Number(n) => n.to_string(),
            _ => return Err(AppError::bad(format!("“{}” is not valid", def.name))),
        };
        if text.is_empty() {
            continue;
        }
        let value = match def.kind.as_str() {
            "number" => json!(f64::from_str(&text.replace(',', ".")).map_err(|_| AppError::bad(format!("“{}” must be a number", def.name)))?),
            "list" => {
                let o = def.options.iter().find(|o| o.eq_ignore_ascii_case(&text)).ok_or_else(|| AppError::bad(format!("“{text}” is not a value of “{}”", def.name)))?;
                json!(o)
            }
            _ => {
                if text.chars().count() > 1000 {
                    return Err(AppError::bad(format!("“{}” can be at most 1000 characters", def.name)));
                }
                json!(text)
            }
        };
        out.insert(k.clone(), value);
    }
    Ok(out)
}

#[derive(Deserialize)]
struct PropsBody {
    props: Map<String, Value>,
}

/// Replaces a product's own property values (variant values stay as they are).
async fn put_props(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PropsBody>) -> AppResult<Json<Value>> {
    super::money::may_edit_product(&state, &auth, id).await?;
    let defs = props(&state).await?;
    let clean = clean_props(&defs, &b.props, false)?;
    let current: Value = sqlx::query_scalar!("SELECT props FROM crm_products WHERE id = $1", id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("product"))?;
    let mut merged = Map::new();
    // Keep variant values; replace the rest.
    for (k, v) in current.as_object().cloned().unwrap_or_default() {
        if defs.iter().any(|d| d.is_variant && d.id.to_string() == k) {
            merged.insert(k, v);
        }
    }
    merged.extend(clean);
    sqlx::query!("UPDATE crm_products SET props = $2, updated_at = now() WHERE id = $1", id, Value::Object(merged)).execute(&state.db).await?;
    details(State(state), auth, Path(id)).await
}

// ---------------------------------------------------------------------------
// Price types
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct PriceType {
    id: Uuid,
    name: String,
    sort: i32,
}

async fn price_types(state: &AppState) -> AppResult<Vec<PriceType>> {
    Ok(sqlx::query_as!(PriceType, "SELECT id, name, sort FROM crm_price_types ORDER BY sort, name").fetch_all(&state.db).await?)
}

async fn list_price_types(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "items": price_types(&state).await? })))
}

#[derive(Deserialize)]
struct NameInput {
    name: Option<String>,
}

async fn create_price_type(State(state): State<AppState>, auth: AuthUser, Json(b): Json<NameInput>) -> AppResult<Json<Value>> {
    can_manage_products(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 100)?;
    let sort: i32 = sqlx::query_scalar!(r#"SELECT COALESCE(MAX(sort), 0) + 10 AS "v!" FROM crm_price_types"#).fetch_one(&state.db).await?;
    sqlx::query!("INSERT INTO crm_price_types (name, sort) VALUES ($1, $2)", name, sort).execute(&state.db).await?;
    list_price_types(State(state), auth).await
}

async fn update_price_type(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<NameInput>) -> AppResult<Json<Value>> {
    can_manage_products(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 100)?;
    sqlx::query!("UPDATE crm_price_types SET name = $2 WHERE id = $1", id, name).execute(&state.db).await?;
    list_price_types(State(state), auth).await
}

/// Its prices go with it; product rows already on deals keep their prices.
async fn delete_price_type(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    can_manage_products(&auth)?;
    sqlx::query!("DELETE FROM crm_price_types WHERE id = $1", id).execute(&state.db).await?;
    list_price_types(State(state), auth).await
}

#[derive(Deserialize)]
struct PricesBody {
    /// price type id → price, or null to remove it.
    prices: HashMap<Uuid, Option<String>>,
}

async fn put_prices(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PricesBody>) -> AppResult<Json<Value>> {
    super::money::may_edit_product(&state, &auth, id).await?;
    let types: Vec<Uuid> = sqlx::query_scalar!("SELECT id FROM crm_price_types").fetch_all(&state.db).await?;
    let mut tx = state.db.begin().await?;
    for (t, p) in b.prices {
        if !types.contains(&t) {
            return Err(AppError::bad("unknown price type"));
        }
        match p.map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
            None => {
                sqlx::query!("DELETE FROM crm_product_prices WHERE product_id = $1 AND price_type_id = $2", id, t).execute(&mut *tx).await?;
            }
            Some(v) => {
                let d = Decimal::from_str(&v).map_err(|_| AppError::bad("prices must be numbers"))?;
                if d.is_sign_negative() {
                    return Err(AppError::bad("prices cannot be negative"));
                }
                sqlx::query!("INSERT INTO crm_product_prices (product_id, price_type_id, price) VALUES ($1, $2, $3)
                     ON CONFLICT (product_id, price_type_id) DO UPDATE SET price = $3", id, t, d)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    tx.commit().await?;
    details(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct PricesFor {
    price_type_id: Option<Uuid>,
    product_ids: Vec<Uuid>,
}

/// Prices of products in one price type (base prices where a product has none in it), for
/// filling product rows.
async fn prices_for(State(state): State<AppState>, _auth: AuthUser, Json(b): Json<PricesFor>) -> AppResult<Json<Value>> {
    let rows: Vec<(Uuid, Decimal)> = sqlx::query!(r#"SELECT p.id AS "id!", COALESCE((SELECT x.price FROM crm_product_prices x WHERE x.product_id = p.id AND x.price_type_id = $2), p.price) AS "c1!"
         FROM crm_products p WHERE p.id = ANY($1)"#, &b.product_ids, b.price_type_id).map(|r| (r.id, r.c1))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "prices": rows.into_iter().map(|(id, p)| (id.to_string(), json!(p))).collect::<Map<String, Value>>() })))
}

// ---------------------------------------------------------------------------
// One product's catalog details, and variants
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct VariantRow {
    id: Uuid,
    name: String,
    sku: Option<String>,
    price: Decimal,
    currency: String,
    stock: Option<Decimal>,
    is_active: bool,
    props: Value,
}

async fn details(State(state): State<AppState>, _auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (product_props, parent): (Value, Option<Uuid>) = sqlx::query!(r#"SELECT props, parent_id FROM crm_products WHERE id = $1"#, id).map(|r| (r.props, r.parent_id))
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("product"))?;
    let prices: Vec<(Uuid, Decimal)> = sqlx::query!(r#"SELECT price_type_id, price FROM crm_product_prices WHERE product_id = $1"#, id).map(|r| (r.price_type_id, r.price)).fetch_all(&state.db).await?;
    let variants: Vec<VariantRow> = sqlx::query_as!(VariantRow, r#"SELECT id, name, sku AS "sku?", price, currency, stock AS "stock?", is_active, props FROM crm_products WHERE parent_id = $1 ORDER BY name"#, id)
    .fetch_all(&state.db)
    .await?;
    let parent_name: Option<String> = match parent {
        Some(p) => sqlx::query_scalar!("SELECT name FROM crm_products WHERE id = $1", p).fetch_optional(&state.db).await?,
        None => None,
    };
    Ok(Json(json!({
        "props": product_props,
        "prices": prices.into_iter().map(|(t, p)| (t.to_string(), json!(p))).collect::<Map<String, Value>>(),
        "variants": variants,
        "parent": parent.map(|p| json!({ "id": p, "name": parent_name })),
        "definitions": props(&state).await?,
        "price_types": price_types(&state).await?,
    })))
}

/// A variant's values of the variant properties, in property order.
fn signature<'a>(defs: &[Prop], props: &'a Value) -> Vec<Option<&'a str>> {
    defs.iter().filter(|d| d.is_variant).map(|d| props.get(d.id.to_string()).and_then(Value::as_str)).collect()
}

#[derive(Deserialize)]
struct MakeVariants {
    /// Variant property id → the values to combine.
    values: HashMap<Uuid, Vec<String>>,
}

#[derive(FromRow)]
struct Parent {
    name: String,
    description: Option<String>,
    price: Decimal,
    currency: String,
    tax_rate: Option<Decimal>,
    tax_included: bool,
    unit: String,
    in_shop: bool,
    shop_category_id: Option<Uuid>,
    parent_id: Option<Uuid>,
    props: Value,
}

/// Makes a variant for every combination of the chosen values that doesn't exist yet.
async fn make_variants(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<MakeVariants>) -> AppResult<Json<Value>> {
    super::money::may_edit_product(&state, &auth, id).await?;
    let p: Parent = sqlx::query_as!(Parent, r#"SELECT name, description AS "description?", price, currency, tax_rate AS "tax_rate?", tax_included, unit, in_shop, shop_category_id AS "shop_category_id?", parent_id AS "parent_id?", props FROM crm_products WHERE id = $1"#, id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("product"))?;
    if p.parent_id.is_some() {
        return Err(AppError::bad("a variant cannot have variants of its own"));
    }
    let defs = props(&state).await?;
    // Axes in property order, each with its chosen values in the property's order.
    let mut axes: Vec<(&Prop, Vec<String>)> = Vec::new();
    for d in defs.iter().filter(|d| d.is_variant) {
        if let Some(chosen) = b.values.get(&d.id) {
            let vals: Vec<String> = d.options.iter().filter(|o| chosen.iter().any(|c| c.eq_ignore_ascii_case(o))).cloned().collect();
            if vals.len() != chosen.len() {
                return Err(AppError::bad(format!("some values are not values of “{}”", d.name)));
            }
            if !vals.is_empty() {
                axes.push((d, vals));
            }
        }
    }
    if b.values.keys().any(|k| !defs.iter().any(|d| d.is_variant && d.id == *k)) {
        return Err(AppError::bad("variants are made from variant properties"));
    }
    if axes.is_empty() {
        return Err(AppError::bad("choose at least one value"));
    }
    let mut combos: Vec<Vec<(&Prop, String)>> = vec![vec![]];
    for (d, vals) in &axes {
        combos = combos.into_iter().flat_map(|c| vals.iter().map(move |v| { let mut n = c.clone(); n.push((*d, v.clone())); n })).collect();
    }
    if combos.len() > 200 {
        return Err(AppError::bad(format!("that makes {} variants; at most 200 at once", combos.len())));
    }
    let existing: Vec<Value> = sqlx::query_scalar!("SELECT props FROM crm_products WHERE parent_id = $1", id).fetch_all(&state.db).await?;
    let own = p.props.as_object().cloned().unwrap_or_default();
    let mut made = 0;
    for combo in combos {
        // One variant per combination of variant values.
        let wanted: Vec<Option<&str>> = defs.iter().filter(|d| d.is_variant).map(|d| combo.iter().find(|(c, _)| c.id == d.id).map(|(_, v)| v.as_str())).collect();
        if existing.iter().any(|e| signature(&defs, e) == wanted) {
            continue;
        }
        let label = combo.iter().map(|(_, v)| v.as_str()).collect::<Vec<_>>().join(", ");
        let name = format!("{} ({label})", p.name);
        let mut values = own.clone();
        for (d, v) in &combo {
            values.insert(d.id.to_string(), json!(v));
        }
        let slug = if p.in_shop { Some(unique_product_slug(&state, &name).await?) } else { None };
        sqlx::query!("INSERT INTO crm_products (name, description, price, currency, tax_rate, tax_included, unit, in_shop, shop_category_id, parent_id, props, slug)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)", &name, p.description.as_deref(), p.price, &p.currency, p.tax_rate, p.tax_included, &p.unit, p.in_shop, p.shop_category_id, id, Value::Object(values), slug)
        .execute(&state.db)
        .await?;
        made += 1;
    }
    let mut v = details(State(state), auth, Path(id)).await?.0;
    v["made"] = json!(made);
    Ok(Json(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(kind: &str, options: &[&str], is_variant: bool) -> Prop {
        Prop { id: Uuid::new_v4(), name: "P".into(), kind: kind.into(), options: options.iter().map(|s| s.to_string()).collect(), is_variant, in_shop: true, sort: 0 }
    }

    #[test]
    fn values_are_checked_against_their_property() {
        let size = prop("list", &["S", "M"], true);
        let weight = prop("number", &[], false);
        let defs = vec![size.clone(), weight.clone()];
        let m = |k: &Prop, v: Value| { let mut m = Map::new(); m.insert(k.id.to_string(), v); m };
        assert_eq!(clean_props(&defs, &m(&weight, json!("2,5")), false).unwrap()[&weight.id.to_string()], json!(2.5));
        assert!(clean_props(&defs, &m(&weight, json!("heavy")), false).is_err());
        assert!(clean_props(&defs, &m(&size, json!("m")), false).is_err()); // variant values come from variants
        assert_eq!(clean_props(&defs, &m(&size, json!("m")), true).unwrap()[&size.id.to_string()], json!("M"));
        assert!(clean_props(&defs, &m(&size, json!("XL")), true).is_err());
        assert!(clean_props(&defs, &m(&weight, json!(null)), false).unwrap().is_empty());
    }
}
