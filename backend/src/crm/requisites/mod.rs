//! Requisites: the legal details of companies and contacts (registered name, tax and
//! registration numbers per country, legal address, bank accounts), and the choice of
//! which of them a deal or lead is billed to.

pub mod schema;

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;
use std::collections::HashMap;
use uuid::Uuid;

use super::{
    access::{require, visible},
    broadcast,
    fields::clean_address,
    items::{load_row, refresh_derived},
    entity_type_by_id, timeline, COMPANY, CONTACT,
};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/requisites/schema", get(get_schema))
        .route("/crm/requisites/check", get(check_value))
        .route("/crm/items/{id}/requisites", get(list).post(create))
        .route("/crm/requisites/{id}", axum::routing::patch(update).delete(remove))
        .route("/crm/items/{id}/buyer", get(get_buyer).put(put_buyer))
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow, Clone)]
pub struct Requisite {
    pub id: Uuid,
    pub item_id: Uuid,
    pub kind: String,
    pub country: String,
    pub name: String,
    pub fields: Value,
    pub address: Option<Value>,
    pub is_default: bool,
    pub sort: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[sqlx(skip)]
    pub banks: Vec<Bank>,
}

#[derive(Serialize, Deserialize, FromRow, Clone)]
pub struct Bank {
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub fields: Value,
    #[serde(default)]
    pub is_default: bool,
}

const COLUMNS: &str = "id, item_id, kind, country, name, fields, address, is_default, sort, created_at, updated_at";

/// Requisites of some items, with their bank accounts, default first.
pub async fn of_items(state: &AppState, items: &[Uuid]) -> AppResult<Vec<Requisite>> {
    let mut reqs: Vec<Requisite> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM crm_requisites WHERE item_id = ANY($1) ORDER BY is_default DESC, sort, created_at"
    )))
    .bind(items)
    .fetch_all(&state.db)
    .await?;
    let ids: Vec<Uuid> = reqs.iter().map(|r| r.id).collect();
    let banks: Vec<(Uuid, Uuid, String, Value, bool)> = sqlx::query_as(
        "SELECT requisite_id, id, name, fields, is_default FROM crm_requisite_banks WHERE requisite_id = ANY($1) ORDER BY is_default DESC, sort",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await?;
    for r in &mut reqs {
        r.banks = banks
            .iter()
            .filter(|b| b.0 == r.id)
            .map(|b| Bank { id: Some(b.1), name: b.2.clone(), fields: b.3.clone(), is_default: b.4 })
            .collect();
    }
    Ok(reqs)
}

async fn load(state: &AppState, id: Uuid) -> AppResult<Requisite> {
    let r: Requisite = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLUMNS} FROM crm_requisites WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("requisite"))?;
    Ok(of_items(state, &[r.item_id]).await?.into_iter().find(|x| x.id == id).unwrap_or(r))
}

/// The record a requisite belongs to must be a company or contact the caller may access.
async fn owner(state: &AppState, auth: &AuthUser, item: Uuid, action: &str) -> AppResult<i16> {
    let row = load_row(state, item, false).await?;
    if row.entity_type != COMPANY && row.entity_type != CONTACT {
        return Err(AppError::bad("requisites belong to companies and contacts"));
    }
    require(state, auth, row.entity_type, action, row.assigned_to).await?;
    Ok(row.entity_type)
}

// ---------------------------------------------------------------------------
// Schema and checks
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SchemaQuery {
    country: Option<String>,
}

fn country_code(c: &str) -> AppResult<String> {
    let c = c.trim().to_uppercase();
    if c.len() == 2 && c.chars().all(|x| x.is_ascii_uppercase()) { Ok(c) } else { Err(AppError::bad("country must be a two-letter ISO code")) }
}

async fn get_schema(_auth: AuthUser, Query(q): Query<SchemaQuery>) -> AppResult<Json<Value>> {
    match q.country {
        Some(c) => Ok(Json(schema::describe(&country_code(&c)?))),
        None => Ok(Json(json!({ "specific": schema::specific_countries() }))),
    }
}

#[derive(Serialize)]
struct Duplicate {
    item_id: Uuid,
    entity_type: i16,
    title: String,
    field: String,
    value: String,
}

/// Other records (that the caller can see) with the same identifier values.
async fn duplicates(state: &AppState, auth: &AuthUser, country: &str, values: &[(String, String)], exclude_item: Option<Uuid>) -> AppResult<Vec<Duplicate>> {
    let compacts: Vec<String> = values.iter().map(|(_, v)| v.clone()).collect();
    if compacts.is_empty() {
        return Ok(vec![]);
    }
    let rows: Vec<(Uuid, i16, String, Option<Uuid>, String, String, String)> = sqlx::query_as(
        "SELECT i.id, i.entity_type, i.title, i.assigned_to, x.code, x.value, r.country
         FROM crm_requisite_ids x JOIN crm_requisites r ON r.id = x.requisite_id JOIN crm_items i ON i.id = x.item_id
         WHERE x.value = ANY($1) AND i.deleted_at IS NULL AND ($2::uuid IS NULL OR x.item_id <> $2)
         LIMIT 50",
    )
    .bind(&compacts)
    .bind(exclude_item)
    .fetch_all(&state.db)
    .await?;
    let mut vis: HashMap<i16, super::access::Visible> = HashMap::new();
    let mut out = Vec::new();
    for (id, et, title, assigned, code, value, their_country) in rows {
        // The same number means the same entity only under the same scheme.
        if !values.iter().any(|(c, v)| *c == code && *v == value) || their_country != country {
            continue;
        }
        if let std::collections::hash_map::Entry::Vacant(e) = vis.entry(et) {
            e.insert(visible(state, auth, et, "read").await?);
        }
        if !vis[&et].allows(assigned) || out.iter().any(|d: &Duplicate| d.item_id == id) {
            continue;
        }
        let label = schema::identifiers(country).iter().find(|f| f.code == code).map(|f| f.label).unwrap_or("ID");
        out.push(Duplicate { item_id: id, entity_type: et, title, field: label.to_string(), value });
    }
    Ok(out)
}

#[derive(Deserialize)]
struct CheckQuery {
    country: String,
    code: String,
    value: String,
    item_id: Option<Uuid>,
}

/// Live check while typing: format error and records with the same number.
async fn check_value(State(state): State<AppState>, auth: AuthUser, Query(q): Query<CheckQuery>) -> AppResult<Json<Value>> {
    let country = country_code(&q.country)?;
    let Some(field) = schema::identifiers(&country).iter().chain(schema::bank(&country)).find(|f| f.code == q.code) else {
        return Err(AppError::bad("unknown field"));
    };
    if q.value.trim().is_empty() {
        return Ok(Json(json!({ "error": null, "duplicates": [] })));
    }
    let error = schema::check(field, &q.value).err();
    let dups = if error.is_none() && schema::identifiers(&country).iter().any(|f| f.code == q.code) {
        duplicates(&state, &auth, &country, &[(q.code.clone(), schema::compact(&q.value))], q.item_id).await?
    } else {
        vec![]
    };
    Ok(Json(json!({ "error": error, "duplicates": dups })))
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Body {
    kind: Option<String>,
    country: String,
    name: String,
    #[serde(default)]
    fields: Map<String, Value>,
    address: Option<Map<String, Value>>,
    #[serde(default)]
    is_default: bool,
    #[serde(default)]
    banks: Vec<Bank>,
}

struct Clean {
    kind: String,
    country: String,
    name: String,
    fields: Map<String, Value>,
    /// (code, compact value) of identifiers, for the index.
    ids: Vec<(String, String)>,
    address: Option<Value>,
    banks: Vec<(String, Map<String, Value>, bool)>,
}

/// Validates against the country's schema. Unknown fields are rejected; empty ones dropped.
fn clean(b: Body) -> AppResult<Clean> {
    let country = country_code(&b.country)?;
    let kind = b.kind.unwrap_or_else(|| "company".into());
    if kind != "company" && kind != "person" {
        return Err(AppError::bad("kind must be company or person"));
    }
    let name = b.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 500 {
        return Err(AppError::bad("enter the legal name (up to 500 characters)"));
    }
    let take = |defs: &'static [schema::Field], input: &Map<String, Value>, for_person: bool| -> AppResult<Map<String, Value>> {
        let mut out = Map::new();
        for (k, v) in input {
            let Some(def) = defs.iter().find(|f| f.code == k) else {
                return Err(AppError::bad(format!("{k} is not a field for this country")));
            };
            let s = match v {
                Value::Null => continue,
                Value::String(s) => s.trim().to_string(),
                _ => return Err(AppError::bad(format!("{} must be text", def.label))),
            };
            if s.is_empty() {
                continue;
            }
            if s.chars().count() > 200 {
                return Err(AppError::bad(format!("{} is too long", def.label)));
            }
            if for_person && !def.person {
                return Err(AppError::bad(format!("{} applies to organisations only", def.label)));
            }
            schema::check(def, &s).map_err(AppError::bad)?;
            // Structured identifiers are case-insensitive: keep them upper case, spacing as typed.
            let s = if matches!(def.check, schema::Check::Any | schema::Check::Email) { s } else { s.to_uppercase() };
            out.insert(k.clone(), Value::String(s));
        }
        Ok(out)
    };
    let fields = take(schema::identifiers(&country), &b.fields, kind == "person")?;
    let ids = fields.iter().map(|(k, v)| (k.clone(), schema::compact(v.as_str().unwrap_or_default()))).filter(|(_, v)| v.len() >= 3).collect();
    let address = match b.address {
        Some(a) => {
            let a = clean_address(&a)?;
            if a.is_empty() { None } else { Some(Value::Object(a)) }
        }
        None => None,
    };
    if b.banks.len() > 20 {
        return Err(AppError::bad("at most 20 bank accounts"));
    }
    let mut banks = Vec::new();
    for bank in b.banks {
        let input = match bank.fields {
            Value::Object(m) => m,
            Value::Null => Map::new(),
            _ => return Err(AppError::bad("bank fields must be an object")),
        };
        let f = take(schema::bank(&country), &input, false)?;
        if f.is_empty() {
            continue;
        }
        banks.push((bank.name.trim().chars().take(200).collect(), f, bank.is_default));
    }
    // Exactly one default account when there are any.
    if !banks.is_empty() && !banks.iter().any(|b| b.2) {
        banks[0].2 = true;
    }
    let mut seen = false;
    for b in &mut banks {
        if b.2 {
            b.2 = !seen;
            seen = true;
        }
    }
    Ok(Clean { kind, country, name, fields, ids, address, banks })
}

async fn save(state: &AppState, auth: &AuthUser, item: Uuid, et: i16, existing: Option<Uuid>, c: Clean, is_default: bool) -> AppResult<Uuid> {
    let mut tx = state.db.begin().await?;
    let has_default: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_requisites WHERE item_id = $1 AND is_default AND id IS DISTINCT FROM $2)")
        .bind(item)
        .bind(existing)
        .fetch_one(&mut *tx)
        .await?;
    // The first requisite becomes the default; choosing another default moves it.
    let default = is_default || !has_default;
    if default && has_default {
        sqlx::query("UPDATE crm_requisites SET is_default = FALSE WHERE item_id = $1 AND is_default").bind(item).execute(&mut *tx).await?;
    }
    let id = match existing {
        Some(id) => {
            sqlx::query(
                "UPDATE crm_requisites SET kind = $2, country = $3, name = $4, fields = $5, address = $6, is_default = $7, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(&c.kind)
            .bind(&c.country)
            .bind(&c.name)
            .bind(Value::Object(c.fields.clone()))
            .bind(&c.address)
            .bind(default)
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM crm_requisite_banks WHERE requisite_id = $1").bind(id).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM crm_requisite_ids WHERE requisite_id = $1").bind(id).execute(&mut *tx).await?;
            id
        }
        None => {
            let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM crm_requisites WHERE item_id = $1").bind(item).fetch_one(&mut *tx).await?;
            if n >= 50 {
                return Err(AppError::bad("a record can have at most 50 requisites"));
            }
            sqlx::query_scalar(
                "INSERT INTO crm_requisites (item_id, kind, country, name, fields, address, is_default, sort, created_by)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
            )
            .bind(item)
            .bind(&c.kind)
            .bind(&c.country)
            .bind(&c.name)
            .bind(Value::Object(c.fields.clone()))
            .bind(&c.address)
            .bind(default)
            .bind(100 + n as i32)
            .bind(auth.id)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    for (i, (name, fields, is_default)) in c.banks.iter().enumerate() {
        sqlx::query("INSERT INTO crm_requisite_banks (requisite_id, name, fields, is_default, sort) VALUES ($1, $2, $3, $4, $5)")
            .bind(id)
            .bind(name)
            .bind(Value::Object(fields.clone()))
            .bind(is_default)
            .bind(i as i32)
            .execute(&mut *tx)
            .await?;
    }
    for (code, value) in &c.ids {
        sqlx::query("INSERT INTO crm_requisite_ids (requisite_id, item_id, code, value) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(item)
            .bind(code)
            .bind(value)
            .execute(&mut *tx)
            .await?;
    }
    let t = entity_type_by_id(state, et).await?;
    // Legal names and numbers become searchable on the record.
    refresh_derived(state, &mut tx, &t, item).await?;
    timeline(&mut *tx, item, "requisite", Some(auth.id), None, json!({ "event": if existing.is_some() { "updated" } else { "added" }, "name": c.name, "country": c.country })).await?;
    tx.commit().await?;
    broadcast(state, et, item, false).await;
    Ok(id)
}

async fn list(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>) -> AppResult<Json<Value>> {
    owner(&state, &auth, item, "read").await?;
    Ok(Json(json!({ "items": of_items(&state, &[item]).await? })))
}

async fn respond(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Json<Value>> {
    let r = load(state, id).await?;
    let ids: Vec<(String, String)> = sqlx::query_as("SELECT code, value FROM crm_requisite_ids WHERE requisite_id = $1").bind(id).fetch_all(&state.db).await?;
    let dups = duplicates(state, auth, &r.country, &ids, Some(r.item_id)).await?;
    let mut v = serde_json::to_value(&r).unwrap();
    v["duplicates"] = json!(dups);
    Ok(Json(v))
}

async fn create(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>, Json(b): Json<Body>) -> AppResult<Json<Value>> {
    let et = owner(&state, &auth, item, "write").await?;
    let is_default = b.is_default;
    let id = save(&state, &auth, item, et, None, clean(b)?, is_default).await?;
    respond(&state, &auth, id).await
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Body>) -> AppResult<Json<Value>> {
    let cur = load(&state, id).await?;
    let et = owner(&state, &auth, cur.item_id, "write").await?;
    let is_default = b.is_default || cur.is_default;
    save(&state, &auth, cur.item_id, et, Some(id), clean(b)?, is_default).await?;
    respond(&state, &auth, id).await
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let cur = load(&state, id).await?;
    let et = owner(&state, &auth, cur.item_id, "write").await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM crm_requisites WHERE id = $1").bind(id).execute(&mut *tx).await?;
    if cur.is_default {
        // Promote the next one.
        sqlx::query(
            "UPDATE crm_requisites SET is_default = TRUE WHERE id = (SELECT id FROM crm_requisites WHERE item_id = $1 ORDER BY sort, created_at LIMIT 1)",
        )
        .bind(cur.item_id)
        .execute(&mut *tx)
        .await?;
    }
    let t = entity_type_by_id(&state, et).await?;
    refresh_derived(&state, &mut tx, &t, cur.item_id).await?;
    timeline(&mut *tx, cur.item_id, "requisite", Some(auth.id), None, json!({ "event": "deleted", "name": cur.name, "country": cur.country })).await?;
    tx.commit().await?;
    broadcast(&state, et, cur.item_id, false).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Buyer of a deal or lead
// ---------------------------------------------------------------------------

/// The requisites a deal or lead may be billed to: those of its company and contacts.
async fn buyer_options(state: &AppState, item: Uuid, company: Option<Uuid>) -> AppResult<Vec<(Requisite, String)>> {
    let contacts: Vec<Uuid> = sqlx::query_scalar("SELECT contact_id FROM crm_item_contacts WHERE item_id = $1 ORDER BY is_primary DESC, sort")
        .bind(item)
        .fetch_all(&state.db)
        .await?;
    let owners: Vec<Uuid> = company.into_iter().chain(contacts.iter().copied()).collect();
    let titles: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, title FROM crm_items WHERE id = ANY($1) AND deleted_at IS NULL").bind(&owners).fetch_all(&state.db).await?;
    let mut reqs = of_items(state, &owners).await?;
    // Company first, then contacts in their order; each owner's default first.
    reqs.sort_by_key(|r| (owners.iter().position(|o| *o == r.item_id).unwrap_or(usize::MAX), !r.is_default, r.sort));
    Ok(reqs
        .into_iter()
        .filter_map(|r| {
            let title = titles.iter().find(|t| t.0 == r.item_id)?.1.clone();
            Some((r, title))
        })
        .collect())
}

/// The requisite a document for this record is addressed to: the one chosen on the deal
/// or lead if still valid, else the first default among its company and contacts. For a
/// company or contact, its own default.
pub async fn buyer_of(state: &AppState, item: Uuid) -> AppResult<Option<Requisite>> {
    let (et, company, chosen): (i16, Option<Uuid>, Option<Uuid>) = sqlx::query_as("SELECT entity_type, company_id, requisite_id FROM crm_items WHERE id = $1")
        .bind(item)
        .fetch_one(&state.db)
        .await?;
    if et == COMPANY || et == CONTACT {
        return Ok(of_items(state, &[item]).await?.into_iter().next());
    }
    let opts = buyer_options(state, item, company).await?;
    if let Some(c) = chosen
        && let Some((r, _)) = opts.iter().find(|(r, _)| r.id == c) {
            return Ok(Some(r.clone()));
        }
    Ok(opts.into_iter().map(|(r, _)| r).next())
}

/// "VAT number DE123456789 · Handelsregister HRB 12345": the printable identifiers.
pub fn tax_ids(r: &Requisite) -> Vec<(String, String)> {
    schema::identifiers(&r.country)
        .iter()
        .filter(|f| f.print)
        .filter_map(|f| r.fields.get(f.code).and_then(Value::as_str).map(|v| (f.label.to_string(), v.to_string())))
        .collect()
}

/// A bank account as label/value lines, for documents.
pub fn bank_lines(country: &str, b: &Bank) -> Vec<(String, String)> {
    schema::bank(country)
        .iter()
        .filter_map(|f| b.fields.get(f.code).and_then(Value::as_str).map(|v| (f.label.to_string(), v.to_string())))
        .collect()
}

async fn deal_or_lead(state: &AppState, auth: &AuthUser, item: Uuid, action: &str) -> AppResult<(Option<Uuid>, Option<Uuid>, i16)> {
    let row = load_row(state, item, false).await?;
    if !entity_type_by_id(state, row.entity_type).await?.has_client {
        return Err(AppError::bad("only records linked to a client have a buyer"));
    }
    require(state, auth, row.entity_type, action, row.assigned_to).await?;
    let chosen: Option<Uuid> = sqlx::query_scalar("SELECT requisite_id FROM crm_items WHERE id = $1").bind(item).fetch_one(&state.db).await?;
    Ok((row.company_id, chosen, row.entity_type))
}

async fn get_buyer(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>) -> AppResult<Json<Value>> {
    let (company, chosen, _) = deal_or_lead(&state, &auth, item, "read").await?;
    let opts = buyer_options(&state, item, company).await?;
    let current = buyer_of(&state, item).await?.map(|r| r.id);
    Ok(Json(json!({
        "chosen": chosen.filter(|c| opts.iter().any(|(r, _)| r.id == *c)),
        "current": current,
        "options": opts.iter().map(|(r, title)| json!({
            "id": r.id, "item_id": r.item_id, "item_title": title, "name": r.name, "country": r.country,
            "is_default": r.is_default,
            "tax_line": tax_ids(r).iter().map(|(l, v)| format!("{l} {v}")).collect::<Vec<_>>().join(" · "),
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct BuyerBody {
    requisite_id: Option<Uuid>,
}

async fn put_buyer(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>, Json(b): Json<BuyerBody>) -> AppResult<Json<Value>> {
    let (company, _, et) = deal_or_lead(&state, &auth, item, "write").await?;
    if let Some(r) = b.requisite_id
        && !buyer_options(&state, item, company).await?.iter().any(|(x, _)| x.id == r) {
            return Err(AppError::bad("choose requisites of this record's company or contacts"));
        }
    sqlx::query("UPDATE crm_items SET requisite_id = $2, updated_at = now() WHERE id = $1").bind(item).bind(b.requisite_id).execute(&state.db).await?;
    broadcast(&state, et, item, false).await;
    get_buyer(State(state), auth, Path(item)).await
}
