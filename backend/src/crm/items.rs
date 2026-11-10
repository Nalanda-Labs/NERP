//! CRM records: list/filter, Kanban, read, create, update, delete, stage moves,
//! bulk actions and lead conversion. One implementation serves every record type.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::{collections::HashMap, str::FromStr};
use uuid::Uuid;

use super::{
    access::{self, require, visible, Visible},
    broadcast,
    config::{categories, default_placement, stage},
    entity_type, entity_type_by_id,
    fields::{clean_address, custom_fields, system_options, validate_uf},
    money::{base_currency, rate_of, totals},
    search_text, timeline, EntityType, COMPANY, CONTACT, DEAL, LEAD,
};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    routes::{clean, Page},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/{type}/items", get(list).post(create))
        .route("/crm/{type}/kanban", get(kanban))
        .route("/crm/{type}/bulk", post(bulk))
        .route("/crm/{type}/items/{id}", get(detail).patch(update).delete(remove))
        .route("/crm/{type}/items/{id}/move", post(move_item))
        .route("/crm/lead/items/{id}/convert", post(convert))
        .route("/crm/{type}/items/from/{source}", post(create_from))
        .route("/crm/lookup", get(lookup))
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow, Clone)]
pub struct Row {
    pub id: Uuid,
    pub entity_type: i16,
    pub number: i64,
    pub title: String,
    pub category_id: Option<Uuid>,
    pub stage_id: Option<Uuid>,
    pub stage_name: Option<String>,
    pub stage_color: Option<String>,
    pub stage_semantics: String,
    pub assigned_to: Option<Uuid>,
    pub assigned_name: Option<String>,
    pub assigned_avatar_file_id: Option<Uuid>,
    pub honorific: Option<String>,
    pub first_name: Option<String>,
    pub middle_name: Option<String>,
    pub last_name: Option<String>,
    pub job_title: Option<String>,
    pub birthdate: Option<NaiveDate>,
    pub company_name: Option<String>,
    pub company_kind: Option<String>,
    pub industry: Option<String>,
    pub employees: Option<String>,
    pub annual_revenue: Option<Decimal>,
    pub opportunity: Decimal,
    pub currency: Option<String>,
    pub opportunity_base: Decimal,
    pub is_manual_amount: bool,
    pub probability: Option<i16>,
    pub close_date: Option<NaiveDate>,
    pub source: Option<String>,
    pub source_details: Option<String>,
    pub address: Option<Value>,
    pub comments: Option<String>,
    pub company_id: Option<Uuid>,
    pub company_title: Option<String>,
    pub source_item_id: Option<Uuid>,
    pub uf: Value,
    /// Campaign tags: { source, medium, campaign, content, term }.
    pub utm: Option<Value>,
    pub is_closed: bool,
    pub closed_at: Option<DateTime<Utc>>,
    pub stage_changed_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub position: f64,
    pub open_activities: i64,
    pub next_activity_at: Option<DateTime<Utc>>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    #[sqlx(skip)]
    pub phone: Option<String>,
    #[sqlx(skip)]
    pub email: Option<String>,
}

pub const ROW_SELECT: &str = "SELECT i.id, i.entity_type, i.number, i.title, i.category_id, i.stage_id,
    s.name AS stage_name, s.color AS stage_color, i.stage_semantics, i.assigned_to,
    CASE WHEN au.id IS NULL THEN NULL ELSE au.first_name || ' ' || au.last_name END AS assigned_name,
    au.avatar_file_id AS assigned_avatar_file_id,
    i.honorific, i.first_name, i.middle_name, i.last_name, i.job_title, i.birthdate,
    i.company_name, i.company_kind, i.industry, i.employees, i.annual_revenue,
    i.opportunity, i.currency, i.opportunity_base, i.is_manual_amount, i.probability, i.close_date,
    i.source, i.source_details, i.address, i.comments, i.company_id, co.title AS company_title, i.source_item_id,
    i.uf, i.utm, i.is_closed, i.closed_at, i.stage_changed_at, i.last_activity_at, i.position,
    (SELECT COUNT(*) FROM crm_activities a WHERE a.item_id = i.id AND NOT a.is_completed) AS open_activities,
    (SELECT MIN(a.deadline) FROM crm_activities a WHERE a.item_id = i.id AND NOT a.is_completed) AS next_activity_at,
    i.created_by, i.created_at, i.updated_at, i.deleted_at
    FROM crm_items i
    LEFT JOIN crm_stages s ON s.id = i.stage_id
    LEFT JOIN users au ON au.id = i.assigned_to
    LEFT JOIN crm_items co ON co.id = i.company_id
    WHERE TRUE";

#[derive(Serialize, FromRow, Clone)]
pub struct Multifield {
    pub id: Uuid,
    pub kind: String,
    pub value_type: String,
    pub value: String,
}

#[derive(Serialize, FromRow)]
pub struct Linked {
    pub id: Uuid,
    pub entity_type: i16,
    pub number: i64,
    pub title: String,
    pub stage_name: Option<String>,
    pub stage_color: Option<String>,
    pub opportunity: Decimal,
    pub currency: Option<String>,
    pub stage_semantics: Option<String>,
    pub close_date: Option<NaiveDate>,
    pub is_primary: bool,
}

#[derive(Serialize)]
pub struct Detail {
    #[serde(flatten)]
    pub row: Row,
    pub multifields: Vec<Multifield>,
    pub contacts: Vec<Linked>,
    /// Deals / leads that this contact or company takes part in.
    pub related: Vec<Linked>,
    pub source_item: Option<Linked>,
    /// Records made from this one (quotes and invoices of a deal, and the invoices of
    /// those quotes; records created by robots).
    pub children: Vec<Linked>,
    pub observers: Vec<super::collab::Person>,
    pub can_write: bool,
    pub can_delete: bool,
}

async fn attach_contact_details(state: &AppState, rows: &mut [Row]) -> AppResult<()> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let mf: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT item_id, kind, value FROM crm_multifields WHERE item_id = ANY($1) AND kind IN ('phone', 'email') ORDER BY sort",
    )
    .bind(&ids)
    .fetch_all(&state.db)
    .await?;
    let mut first: HashMap<(Uuid, &str), String> = HashMap::new();
    for (id, kind, value) in &mf {
        first.entry((*id, if kind == "phone" { "phone" } else { "email" })).or_insert_with(|| value.clone());
    }
    for r in rows.iter_mut() {
        r.phone = first.get(&(r.id, "phone")).cloned();
        r.email = first.get(&(r.id, "email")).cloned();
    }
    Ok(())
}

/// Loads one record (including deleted ones when `include_deleted`).
pub async fn load_row(state: &AppState, id: Uuid, include_deleted: bool) -> AppResult<Row> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(ROW_SELECT);
    qb.push(" AND i.id = ").push_bind(id);
    if !include_deleted {
        qb.push(" AND i.deleted_at IS NULL");
    }
    let mut row: Row = qb.build_query_as().fetch_optional(&state.db).await?.ok_or(AppError::NotFound("record"))?;
    attach_contact_details(state, std::slice::from_mut(&mut row)).await?;
    Ok(row)
}

const LINKED_SELECT: &str = "SELECT i.id, i.entity_type, i.number, i.title, s.name AS stage_name, s.color AS stage_color,
    i.opportunity, i.currency, s.semantics AS stage_semantics, i.close_date";

pub async fn load_detail(state: &AppState, auth: &AuthUser, t: &EntityType, id: Uuid) -> AppResult<Detail> {
    let mut row = load_row(state, id, false).await?;
    if row.entity_type != t.id {
        return Err(AppError::NotFound("record"));
    }
    access::require_read(state, auth, t.id, id, row.assigned_to).await?;
    // Hide values of custom fields that were deleted.
    let codes: Vec<String> = custom_fields(state, t.id).await?.into_iter().map(|f| f.code).collect();
    if let Value::Object(m) = &mut row.uf {
        m.retain(|k, _| codes.contains(k));
    }
    let multifields: Vec<Multifield> = sqlx::query_as("SELECT id, kind, value_type, value FROM crm_multifields WHERE item_id = $1 ORDER BY kind, sort")
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    let contacts: Vec<Linked> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{LINKED_SELECT}, l.is_primary FROM crm_item_contacts l JOIN crm_items i ON i.id = l.contact_id
         LEFT JOIN crm_stages s ON s.id = i.stage_id
         WHERE l.item_id = $1 AND i.deleted_at IS NULL ORDER BY l.is_primary DESC, l.sort"
    )))
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let related: Vec<Linked> = if t.id == CONTACT || t.id == COMPANY {
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{LINKED_SELECT}, FALSE AS is_primary FROM crm_items i LEFT JOIN crm_stages s ON s.id = i.stage_id
             WHERE i.deleted_at IS NULL AND (i.entity_type IN (1, 2) OR i.entity_type IN (SELECT id FROM crm_entity_types WHERE NOT is_system AND has_client))
                AND (i.company_id = $1
                OR EXISTS (SELECT 1 FROM crm_item_contacts l WHERE l.item_id = i.id AND l.contact_id = $1))
             UNION ALL
             {LINKED_SELECT}, FALSE FROM crm_items i LEFT JOIN crm_stages s ON s.id = i.stage_id
             WHERE i.deleted_at IS NULL AND $2 AND i.entity_type = 3 AND i.company_id = $1
             ORDER BY 2, 3 DESC LIMIT 200"
        )))
        .bind(id)
        .bind(t.id == COMPANY)
        .fetch_all(&state.db)
        .await?
    } else {
        vec![]
    };
    let source_item: Option<Linked> = match row.source_item_id {
        Some(s) => sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{LINKED_SELECT}, FALSE AS is_primary FROM crm_items i LEFT JOIN crm_stages s ON s.id = i.stage_id WHERE i.id = $1"
        )))
        .bind(s)
        .fetch_optional(&state.db)
        .await?,
        None => None,
    };
    let children: Vec<Linked> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{LINKED_SELECT}, FALSE AS is_primary FROM crm_items i LEFT JOIN crm_stages s ON s.id = i.stage_id
         WHERE i.deleted_at IS NULL AND (i.source_item_id = $1
            OR i.source_item_id IN (SELECT c.id FROM crm_items c WHERE c.source_item_id = $1 AND c.deleted_at IS NULL))
         ORDER BY i.created_at LIMIT 200"
    )))
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let can_write = visible(state, auth, t.id, "write").await?.allows(row.assigned_to);
    let can_delete = visible(state, auth, t.id, "delete").await?.allows(row.assigned_to);
    let observers = super::collab::observers(state, id).await?;
    Ok(Detail { row, multifields, contacts, related, source_item, children, observers, can_write, can_delete })
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default, Clone)]
pub struct Filter {
    pub q: Option<String>,
    pub category_id: Option<Uuid>,
    /// Comma-separated stage ids.
    pub stage_id: Option<String>,
    /// Comma-separated: process, success, failure.
    pub semantics: Option<String>,
    /// Comma-separated user ids, or "me".
    pub assigned_to: Option<String>,
    pub source: Option<String>,
    pub company_id: Option<Uuid>,
    pub contact_id: Option<Uuid>,
    pub created_from: Option<NaiveDate>,
    pub created_to: Option<NaiveDate>,
    pub updated_from: Option<NaiveDate>,
    pub updated_to: Option<NaiveDate>,
    pub close_from: Option<NaiveDate>,
    pub close_to: Option<NaiveDate>,
    /// Amount in the base currency.
    pub amount_min: Option<String>,
    pub amount_max: Option<String>,
    /// overdue | today | planned | none
    pub activity: Option<String>,
    /// JSON object of custom field equality filters, e.g. {"uf_region":"north"}.
    pub uf: Option<String>,
}

fn csv_uuids(s: &Option<String>, me: Uuid) -> AppResult<Vec<Uuid>> {
    s.as_deref()
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(|x| if x == "me" { Ok(me) } else { Uuid::parse_str(x).map_err(|_| AppError::bad("invalid id in filter")) })
        .collect()
}

fn csv(s: &Option<String>) -> Vec<String> {
    s.as_deref().unwrap_or("").split(',').map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect()
}

/// Appends every filter condition for records `i` of type `t`.
pub fn push_filter(qb: &mut QueryBuilder<Postgres>, t: &EntityType, f: &Filter, me: Uuid) -> AppResult<()> {
    qb.push(" AND i.entity_type = ").push_bind(t.id).push(" AND i.deleted_at IS NULL");
    if let Some(q) = clean(f.q.clone()) {
        let digits = q.trim_start_matches('#');
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) && q.len() < 3 {
            qb.push(" AND i.number = ").push_bind(digits.parse::<i64>().unwrap_or(0));
        } else {
            if q.chars().count() < 3 {
                return Err(AppError::bad("type at least 3 characters to search"));
            }
            let needle = q.to_lowercase().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            qb.push(" AND (i.search_text LIKE ").push_bind(format!("%{needle}%"));
            if let Ok(n) = digits.parse::<i64>() {
                qb.push(" OR i.number = ").push_bind(n);
            }
            qb.push(")");
        }
    }
    if let Some(c) = f.category_id {
        qb.push(" AND i.category_id = ").push_bind(c);
    }
    let stages = csv_uuids(&f.stage_id, me)?;
    if !stages.is_empty() {
        qb.push(" AND i.stage_id = ANY(").push_bind(stages).push(")");
    }
    let sem = csv(&f.semantics);
    if !sem.is_empty() {
        qb.push(" AND i.stage_semantics = ANY(").push_bind(sem).push(")");
    }
    let assigned = csv_uuids(&f.assigned_to, me)?;
    if !assigned.is_empty() {
        qb.push(" AND i.assigned_to = ANY(").push_bind(assigned).push(")");
    }
    let sources = csv(&f.source);
    if !sources.is_empty() {
        qb.push(" AND i.source = ANY(").push_bind(sources).push(")");
    }
    if let Some(c) = f.company_id {
        qb.push(" AND i.company_id = ").push_bind(c);
    }
    if let Some(c) = f.contact_id {
        qb.push(" AND EXISTS (SELECT 1 FROM crm_item_contacts l WHERE l.item_id = i.id AND l.contact_id = ").push_bind(c).push(")");
    }
    let day = |d: NaiveDate| d.and_hms_opt(0, 0, 0).unwrap().and_utc();
    if let Some(d) = f.created_from {
        qb.push(" AND i.created_at >= ").push_bind(day(d));
    }
    if let Some(d) = f.created_to {
        qb.push(" AND i.created_at < ").push_bind(day(d.succ_opt().unwrap_or(d)));
    }
    if let Some(d) = f.updated_from {
        qb.push(" AND i.updated_at >= ").push_bind(day(d));
    }
    if let Some(d) = f.updated_to {
        qb.push(" AND i.updated_at < ").push_bind(day(d.succ_opt().unwrap_or(d)));
    }
    if let Some(d) = f.close_from {
        qb.push(" AND i.close_date >= ").push_bind(d);
    }
    if let Some(d) = f.close_to {
        qb.push(" AND i.close_date <= ").push_bind(d);
    }
    let dec = |s: &Option<String>| -> AppResult<Option<Decimal>> {
        match clean(s.clone()) {
            Some(v) => Decimal::from_str(&v).map(Some).map_err(|_| AppError::bad("amount filter must be a number")),
            None => Ok(None),
        }
    };
    if let Some(v) = dec(&f.amount_min)? {
        qb.push(" AND i.opportunity_base >= ").push_bind(v);
    }
    if let Some(v) = dec(&f.amount_max)? {
        qb.push(" AND i.opportunity_base <= ").push_bind(v);
    }
    match f.activity.as_deref() {
        Some("overdue") => {
            qb.push(" AND EXISTS (SELECT 1 FROM crm_activities a WHERE a.item_id = i.id AND NOT a.is_completed AND a.deadline < now())");
        }
        Some("today") => {
            qb.push(" AND EXISTS (SELECT 1 FROM crm_activities a WHERE a.item_id = i.id AND NOT a.is_completed AND a.deadline::date = CURRENT_DATE)");
        }
        Some("planned") => {
            qb.push(" AND EXISTS (SELECT 1 FROM crm_activities a WHERE a.item_id = i.id AND NOT a.is_completed)");
        }
        Some("none") => {
            qb.push(" AND NOT EXISTS (SELECT 1 FROM crm_activities a WHERE a.item_id = i.id AND NOT a.is_completed)");
        }
        _ => {}
    }
    if let Some(uf) = clean(f.uf.clone()) {
        let obj: Map<String, Value> = serde_json::from_str(&uf).map_err(|_| AppError::bad("uf filter must be a JSON object"))?;
        for (k, v) in obj {
            if !k.starts_with("uf_") || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(AppError::bad("invalid custom field in filter"));
            }
            qb.push(" AND i.uf @> ").push_bind(json!({ k: v }));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// List & Kanban
// ---------------------------------------------------------------------------

const COUNT_CAP: i64 = 10_000;

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(flatten)]
    filter: Filter,
    /// updated | created | title | amount | close_date | number | activity
    sort: Option<String>,
    /// asc | desc
    dir: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

#[derive(Serialize)]
pub struct ListPage {
    #[serde(flatten)]
    page: Page<Row>,
    /// True when there are more than `total` records (counting stops at 10,000).
    total_is_capped: bool,
}

fn num(s: &Option<String>, default: i64) -> i64 {
    s.as_deref().and_then(|v| v.parse().ok()).unwrap_or(default)
}

async fn list(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Query(q): Query<ListQuery>) -> AppResult<Json<ListPage>> {
    let t = entity_type(&state, &code).await?;
    let scope = visible(&state, &auth, t.id, "read").await?;
    if matches!(scope, Visible::Nothing) {
        return Err(AppError::Forbidden);
    }
    let limit = num(&q.limit, 50).clamp(1, 200);
    let offset = num(&q.offset, 0).max(0);

    // Count, but stop at the cap: exact counts over millions of rows are expensive.
    let mut cq: QueryBuilder<Postgres> = QueryBuilder::new("SELECT COUNT(*) FROM (SELECT 1 FROM crm_items i WHERE TRUE");
    push_filter(&mut cq, &t, &q.filter, auth.id)?;
    scope.push(&mut cq);
    cq.push(" LIMIT ").push_bind(COUNT_CAP + 1).push(") x");
    let total: i64 = cq.build_query_scalar().fetch_one(&state.db).await?;

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(ROW_SELECT);
    push_filter(&mut qb, &t, &q.filter, auth.id)?;
    scope.push(&mut qb);
    let desc = q.dir.as_deref() != Some("asc");
    let dir = if desc { "DESC" } else { "ASC" };
    let col = match q.sort.as_deref().unwrap_or("updated") {
        "created" => "i.created_at",
        "title" => "i.title",
        "amount" => "i.opportunity_base",
        "close_date" => "i.close_date",
        "number" => "i.number",
        "activity" => "i.last_activity_at",
        "position" => "i.position",
        _ => "i.updated_at",
    };
    qb.push(format!(" ORDER BY {col} {dir} NULLS LAST, i.id {dir} LIMIT "))
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);
    let mut items: Vec<Row> = qb.build_query_as().fetch_all(&state.db).await?;
    attach_contact_details(&state, &mut items).await?;
    Ok(Json(ListPage {
        page: Page { items, total: total.min(COUNT_CAP), limit, offset },
        total_is_capped: total > COUNT_CAP,
    }))
}

#[derive(Deserialize)]
struct KanbanQuery {
    #[serde(flatten)]
    filter: Filter,
    per_column: Option<String>,
}

/// One request for a whole board: every stage of the pipeline with its count, total amount
/// (in the base currency) and the first cards. Further cards load through the list endpoint.
async fn kanban(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Query(q): Query<KanbanQuery>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    if !t.has_stages {
        return Err(AppError::bad("this record type has no stages"));
    }
    let scope = visible(&state, &auth, t.id, "read").await?;
    if matches!(scope, Visible::Nothing) {
        return Err(AppError::Forbidden);
    }
    let cats = categories(&state, t.id).await?;
    let cat = match q.filter.category_id {
        Some(c) => cats.into_iter().find(|x| x.id == c).ok_or_else(|| AppError::bad("unknown pipeline"))?,
        None => cats.into_iter().find(|x| x.is_default).ok_or_else(|| AppError::bad("no pipeline configured"))?,
    };
    let per = num(&q.per_column, 20).clamp(1, 100);
    let mut columns = Vec::new();
    for s in &cat.stages {
        let mut f = q.filter.clone();
        f.category_id = Some(cat.id);
        f.stage_id = Some(s.id.to_string());
        let mut agg: QueryBuilder<Postgres> =
            QueryBuilder::new("SELECT COUNT(*), COALESCE(SUM(x.opportunity_base), 0) FROM (SELECT i.opportunity_base FROM crm_items i WHERE TRUE");
        push_filter(&mut agg, &t, &f, auth.id)?;
        scope.push(&mut agg);
        agg.push(" LIMIT ").push_bind(COUNT_CAP + 1).push(") x");
        let (count, sum): (i64, Decimal) = agg.build_query_as().fetch_one(&state.db).await?;
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(ROW_SELECT);
        push_filter(&mut qb, &t, &f, auth.id)?;
        scope.push(&mut qb);
        qb.push(" ORDER BY i.position ASC, i.updated_at DESC LIMIT ").push_bind(per);
        let mut items: Vec<Row> = qb.build_query_as().fetch_all(&state.db).await?;
        attach_contact_details(&state, &mut items).await?;
        columns.push(json!({
            "stage": s,
            "count": count.min(COUNT_CAP),
            "count_is_capped": count > COUNT_CAP,
            "sum_base": sum.to_string(),
            "items": items,
        }));
    }
    Ok(Json(json!({ "category_id": cat.id, "columns": columns, "base_currency": base_currency(&state).await? })))
}

async fn detail(State(state): State<AppState>, auth: AuthUser, Path((code, id)): Path<(String, Uuid)>) -> AppResult<Json<Detail>> {
    let t = entity_type(&state, &code).await?;
    Ok(Json(load_detail(&state, &auth, &t, id).await?))
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

fn text(v: &Value, field: &str, max: usize) -> AppResult<Option<String>> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) => {
            let s = s.trim();
            if s.chars().count() > max {
                return Err(AppError::bad(format!("{field} is too long")));
            }
            Ok(if s.is_empty() { None } else { Some(s.to_string()) })
        }
        _ => Err(AppError::bad(format!("{field} must be text"))),
    }
}

fn decimal(v: &Value, field: &str) -> AppResult<Option<Decimal>> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) if s.trim().is_empty() => Ok(None),
        Value::String(s) => Decimal::from_str(s.trim()).map(Some).map_err(|_| AppError::bad(format!("{field} must be a number"))),
        Value::Number(n) => Decimal::from_str(&n.to_string()).map(Some).map_err(|_| AppError::bad(format!("{field} must be a number"))),
        _ => Err(AppError::bad(format!("{field} must be a number"))),
    }
}

fn date(v: &Value, field: &str) -> AppResult<Option<NaiveDate>> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) if s.is_empty() => Ok(None),
        Value::String(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d").map(Some).map_err(|_| AppError::bad(format!("{field} must be a date"))),
        _ => Err(AppError::bad(format!("{field} must be a date"))),
    }
}

fn uuid(v: &Value, field: &str) -> AppResult<Option<Uuid>> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) if s.is_empty() => Ok(None),
        Value::String(s) => Uuid::parse_str(s).map(Some).map_err(|_| AppError::bad(format!("{field} is not a valid reference"))),
        _ => Err(AppError::bad(format!("{field} is not a valid reference"))),
    }
}

fn option(v: &Value, field: &str, key: &str) -> AppResult<Option<String>> {
    let s = text(v, field, 60)?;
    if let Some(s) = &s {
        let ok = system_options()[key].as_array().is_some_and(|a| a.iter().any(|x| x == s));
        if !ok {
            return Err(AppError::bad(format!("{field}: unknown value {s}")));
        }
    }
    Ok(s)
}

/// Normalises a phone/email/web/messenger value for duplicate matching.
///
/// Phones match on their last 10 digits, so the same number written with or without a
/// country code or trunk prefix matches in any country: "+1 (555) 010-7788" and
/// "555 010 7788", or "+44 20 7946 0000" and "020 7946 0000".
pub fn normalize(kind: &str, value: &str) -> String {
    match kind {
        "phone" => {
            let digits: Vec<char> = value.chars().filter(|c| c.is_ascii_digit()).collect();
            digits[digits.len().saturating_sub(10)..].iter().collect()
        }
        "web" => value.trim().to_lowercase().trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.").trim_end_matches('/').to_string(),
        _ => value.trim().to_lowercase(),
    }
}

/// Parses and validates multifields: [{kind, value_type, value}].
fn parse_multifields(v: &Value) -> AppResult<Vec<(String, String, String, String)>> {
    let arr = v.as_array().ok_or_else(|| AppError::bad("multifields must be a list"))?;
    let opts = system_options();
    let mut out = Vec::new();
    for m in arr {
        let kind = m["kind"].as_str().unwrap_or("");
        if !["phone", "email", "web", "im"].contains(&kind) {
            return Err(AppError::bad("unknown contact detail type"));
        }
        let value = m["value"].as_str().unwrap_or("").trim();
        if value.is_empty() {
            continue;
        }
        if value.chars().count() > 250 {
            return Err(AppError::bad("contact detail is too long"));
        }
        let vt = m["value_type"].as_str().unwrap_or(if kind == "im" { "other" } else { "work" });
        if !opts[kind].as_array().is_some_and(|a| a.iter().any(|x| x == vt)) {
            return Err(AppError::bad(format!("unknown {kind} type: {vt}")));
        }
        let normalized = normalize(kind, value);
        match kind {
            "phone" if normalized.len() < 3 || normalized.len() > 20 => return Err(AppError::bad(format!("invalid phone number: {value}"))),
            "email" if !(value.contains('@') && value.rsplit('@').next().is_some_and(|d| d.contains('.'))) => {
                return Err(AppError::bad(format!("invalid email: {value}")))
            }
            _ => {}
        }
        out.push((kind.to_string(), vt.to_string(), value.to_string(), normalized));
    }
    Ok(out)
}

async fn check_link(state: &AppState, id: Uuid, want: i16, what: &str) -> AppResult<()> {
    let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_items WHERE id = $1 AND entity_type = $2 AND deleted_at IS NULL)")
        .bind(id)
        .bind(want)
        .fetch_one(&state.db)
        .await?;
    if ok { Ok(()) } else { Err(AppError::bad(format!("{what} not found"))) }
}

/// What a write changed, for the timeline and notifications.
#[derive(Default)]
pub struct Changes {
    pub stage: Option<(Option<Uuid>, Uuid)>,
    pub assigned: Option<(Option<Uuid>, Option<Uuid>)>,
    pub fields: Vec<String>,
}

/// Applies a JSON object of field values to a record inside a transaction.
/// Unknown keys are rejected so typos don't silently do nothing.
pub async fn apply(
    state: &AppState,
    tx: &mut sqlx::Transaction<'_, Postgres>,
    t: &EntityType,
    id: Uuid,
    current: Option<&Row>,
    body: &Map<String, Value>,
    actor: Uuid,
) -> AppResult<Changes> {
    let mut ch = Changes::default();
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("UPDATE crm_items SET updated_at = now(), updated_by = ");
    qb.push_bind(actor);
    let is_person = t.is_person();
    let has_money = t.has_amount;
    let allowed = |k: &str| -> bool {
        match k {
            "title" => t.id == LEAD || t.id == DEAL || t.is_custom(),
            "honorific" | "first_name" | "middle_name" | "last_name" | "job_title" | "birthdate" => is_person,
            "company_name" => t.id == LEAD || t.id == COMPANY,
            "company_kind" | "industry" | "employees" | "annual_revenue" => t.id == COMPANY,
            "opportunity" | "currency" | "is_manual_amount" => has_money,
            "probability" => t.id == DEAL,
            "close_date" => t.id == DEAL || t.is_custom(),
            "category_id" => t.has_pipelines,
            "stage_id" => t.has_stages,
            "company_id" => if t.is_custom() { t.has_client } else { t.id != COMPANY },
            "contact_ids" => if t.is_custom() { t.has_client } else { t.id != CONTACT },
            "source" | "source_details" | "address" | "comments" | "assigned_to" | "uf" | "multifields" | "utm" => true,
            _ => false,
        }
    };
    for k in body.keys() {
        if !allowed(k) {
            return Err(AppError::bad(format!("field {k} does not apply to {}", t.name_plural.to_lowercase())));
        }
    }

    for (k, v) in body {
        match k.as_str() {
            "title" | "honorific" | "first_name" | "middle_name" | "last_name" | "job_title" | "company_name" | "source_details" => {
                let max = if k == "source_details" { 2000 } else { 300 };
                qb.push(format!(", {k} = ")).push_bind(text(v, k, max)?);
                ch.fields.push(k.clone());
            }
            "comments" => {
                qb.push(", comments = ").push_bind(text(v, k, 20_000)?);
            }
            "birthdate" => {
                qb.push(", birthdate = ").push_bind(date(v, "date of birth")?);
            }
            "close_date" => {
                qb.push(", close_date = ").push_bind(date(v, "close date")?);
                ch.fields.push(k.clone());
            }
            "company_kind" | "industry" | "employees" | "source" => {
                qb.push(format!(", {k} = ")).push_bind(option(v, k, k)?);
            }
            "annual_revenue" => {
                qb.push(", annual_revenue = ").push_bind(decimal(v, "annual revenue")?);
            }
            "opportunity" => {
                let d = decimal(v, "amount")?.unwrap_or_default();
                if d.is_sign_negative() {
                    return Err(AppError::bad("amount cannot be negative"));
                }
                qb.push(", opportunity = ").push_bind(d);
                ch.fields.push(k.clone());
            }
            "currency" => {
                let c = text(v, "currency", 3)?.map(|c| c.to_uppercase());
                if let Some(c) = &c {
                    rate_of(state, c).await?.ok_or_else(|| AppError::bad(format!("currency {c} is not enabled in CRM settings")))?;
                }
                qb.push(", currency = ").push_bind(c);
            }
            "is_manual_amount" => {
                qb.push(", is_manual_amount = ").push_bind(v.as_bool().ok_or_else(|| AppError::bad("is_manual_amount must be true or false"))?);
            }
            "probability" => {
                let p = match v {
                    Value::Null => None,
                    _ => Some(v.as_i64().filter(|p| (0..=100).contains(p)).ok_or_else(|| AppError::bad("probability must be 0-100"))? as i16),
                };
                qb.push(", probability = ").push_bind(p);
            }
            "address" => {
                let a = match v {
                    Value::Null => None,
                    Value::Object(o) => Some(Value::Object(clean_address(o)?)),
                    _ => return Err(AppError::bad("address must be an object")),
                };
                qb.push(", address = ").push_bind(a);
            }
            "utm" => {
                let u = match v {
                    Value::Null => None,
                    Value::Object(o) => {
                        let mut m = Map::new();
                        for k in ["source", "medium", "campaign", "content", "term"] {
                            if let Some(s) = o.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
                                m.insert(k.into(), Value::String(s.chars().take(200).collect()));
                            }
                        }
                        (!m.is_empty()).then_some(Value::Object(m))
                    }
                    _ => return Err(AppError::bad("utm must be an object")),
                };
                qb.push(", utm = ").push_bind(u);
            }
            "assigned_to" => {
                let a = uuid(v, "responsible")?;
                if let Some(u) = a {
                    let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND is_active)").bind(u).fetch_one(&state.db).await?;
                    if !ok {
                        return Err(AppError::bad("responsible person not found"));
                    }
                }
                if current.map(|c| c.assigned_to) != Some(a) {
                    ch.assigned = Some((current.and_then(|c| c.assigned_to), a));
                }
                qb.push(", assigned_to = ").push_bind(a);
            }
            "company_id" => {
                let c = uuid(v, "company")?;
                if let Some(c) = c {
                    check_link(state, c, COMPANY, "company").await?;
                }
                qb.push(", company_id = ").push_bind(c);
            }
            "category_id" | "stage_id" | "contact_ids" | "uf" | "multifields" => {} // handled below
            _ => unreachable!(),
        }
    }

    // Pipeline / stage.
    if t.has_stages && (body.contains_key("stage_id") || body.contains_key("category_id")) {
        let cur_cat = current.and_then(|c| c.category_id);
        let explicit_cat = body.get("category_id").map(|v| uuid(v, "pipeline")).transpose()?.flatten();
        let new_stage = match body.get("stage_id").map(|v| uuid(v, "stage")).transpose()?.flatten() {
            Some(s) => stage(state, s).await?,
            None => {
                // Pipeline changed without a stage: first open stage of the new pipeline.
                let first: Uuid = sqlx::query_scalar("SELECT id FROM crm_stages WHERE category_id = $1 AND semantics = 'process' ORDER BY sort LIMIT 1")
                    .bind(explicit_cat.or(cur_cat))
                    .fetch_optional(&state.db)
                    .await?
                    .ok_or_else(|| AppError::bad("that pipeline has no open stages"))?;
                stage(state, first).await?
            }
        };
        if explicit_cat.is_some_and(|c| c != new_stage.category_id) {
            return Err(AppError::bad("that stage belongs to another pipeline"));
        }
        let cat_type: i16 = sqlx::query_scalar("SELECT entity_type FROM crm_categories WHERE id = $1").bind(new_stage.category_id).fetch_one(&state.db).await?;
        if cat_type != t.id {
            return Err(AppError::bad("that pipeline belongs to another record type"));
        }
        let old_stage = current.and_then(|c| c.stage_id);
        if old_stage != Some(new_stage.id) {
            ch.stage = Some((old_stage, new_stage.id));
            qb.push(", category_id = ").push_bind(new_stage.category_id)
                .push(", stage_id = ").push_bind(new_stage.id)
                .push(", stage_semantics = ").push_bind(new_stage.semantics.clone())
                .push(", is_closed = ").push_bind(new_stage.semantics != "process")
                .push(", stage_changed_at = now(), closed_at = CASE WHEN ")
                .push_bind(new_stage.semantics != "process")
                .push(" THEN now() ELSE NULL END");
        }
    }

    // Custom fields: merge with existing values.
    if let Some(uf) = body.get("uf") {
        let obj = uf.as_object().ok_or_else(|| AppError::bad("uf must be an object"))?;
        let clean = validate_uf(state, t.id, obj, current.is_some()).await?;
        let mut merged = current.and_then(|c| c.uf.as_object().cloned()).unwrap_or_default();
        for (k, v) in clean {
            if v.is_null() { merged.remove(&k); } else { merged.insert(k, v); }
        }
        qb.push(", uf = ").push_bind(Value::Object(merged));
    } else if current.is_none() {
        validate_uf(state, t.id, &Map::new(), false).await?; // required custom fields on create
    }

    qb.push(" WHERE id = ").push_bind(id);
    qb.build().execute(&mut **tx).await?;
    if let Some((_, to)) = ch.stage {
        // Stage history feeds the funnel report.
        sqlx::query(
            "INSERT INTO crm_stage_history (item_id, category_id, stage_id, semantics) SELECT $1, category_id, id, semantics FROM crm_stages WHERE id = $2",
        )
        .bind(id)
        .bind(to)
        .execute(&mut **tx)
        .await?;
    }

    if let Some(v) = body.get("multifields") {
        let mf = parse_multifields(v)?;
        sqlx::query("DELETE FROM crm_multifields WHERE item_id = $1").bind(id).execute(&mut **tx).await?;
        for (i, (kind, vt, value, norm)) in mf.iter().enumerate() {
            sqlx::query("INSERT INTO crm_multifields (item_id, kind, value_type, value, normalized, sort) VALUES ($1, $2, $3, $4, $5, $6)")
                .bind(id)
                .bind(kind)
                .bind(vt)
                .bind(value)
                .bind(norm)
                .bind(i as i32)
                .execute(&mut **tx)
                .await?;
        }
    }
    if let Some(v) = body.get("contact_ids") {
        let ids: Vec<Uuid> = v
            .as_array()
            .ok_or_else(|| AppError::bad("contact_ids must be a list"))?
            .iter()
            .map(|x| uuid(x, "contact").and_then(|o| o.ok_or_else(|| AppError::bad("contact is required"))))
            .collect::<AppResult<_>>()?;
        for c in &ids {
            check_link(state, *c, CONTACT, "contact").await?;
        }
        sqlx::query("DELETE FROM crm_item_contacts WHERE item_id = $1").bind(id).execute(&mut **tx).await?;
        for (i, c) in ids.iter().enumerate() {
            sqlx::query("INSERT INTO crm_item_contacts (item_id, contact_id, is_primary, sort) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
                .bind(id)
                .bind(c)
                .bind(i == 0)
                .bind(i as i32)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(ch)
}

/// (number, title, honorific, first, middle, last, company, job, opportunity, currency)
type DerivedSource = (i64, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Decimal, Option<String>);

/// Recomputes derived columns: display title, search text, amount in base currency.
pub async fn refresh_derived(state: &AppState, tx: &mut sqlx::Transaction<'_, Postgres>, t: &EntityType, id: Uuid) -> AppResult<()> {
    let r: DerivedSource =
        sqlx::query_as(
            "SELECT number, title, honorific, first_name, middle_name, last_name, company_name, job_title, opportunity, currency
             FROM crm_items WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    let (number, title, _hon, first, middle, last, company, job, opportunity, currency) = r;
    let person = [first.as_deref(), middle.as_deref(), last.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" ");
    let display = match t.id {
        CONTACT => if person.is_empty() { format!("Contact #{number}") } else { person.clone() },
        COMPANY => company.clone().unwrap_or_else(|| format!("Company #{number}")),
        _ if title.trim().is_empty() || title.starts_with('\u{0}') => match t.id {
            LEAD if !person.is_empty() => person.clone(),
            LEAD if company.is_some() => company.clone().unwrap(),
            _ => format!("{} #{number}", t.name),
        },
        _ => title.clone(),
    };
    let mut mf: Vec<(String, String)> = sqlx::query_as("SELECT value, normalized FROM crm_multifields WHERE item_id = $1")
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
    // Tax and registration numbers from requisites ("DE123456789"), and legal names.
    let ids: Vec<(String, String)> = sqlx::query_as("SELECT value, value FROM crm_requisite_ids WHERE item_id = $1").bind(id).fetch_all(&mut **tx).await?;
    mf.extend(ids);
    let legal: Vec<String> = sqlx::query_scalar("SELECT name FROM crm_requisites WHERE item_id = $1 LIMIT 5").bind(id).fetch_all(&mut **tx).await?;
    let legal = legal.join(" ");
    let num_tag = format!("#{number}");
    let search = search_text(
        &[Some(display.as_str()), Some(person.as_str()), company.as_deref(), job.as_deref(), Some(num_tag.as_str()), Some(legal.as_str())],
        &mf,
    );
    let rate = match &currency {
        Some(c) => rate_of(state, c).await?.unwrap_or(Decimal::ONE),
        None => Decimal::ONE,
    };
    sqlx::query("UPDATE crm_items SET title = $1, search_text = $2, opportunity_base = $3 WHERE id = $4")
        .bind(&display)
        .bind(search)
        .bind((opportunity * rate).round_dp(4))
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn record_changes(state: &AppState, id: Uuid, t: &EntityType, title: &str, ch: &Changes, actor: Uuid) -> AppResult<()> {
    if let Some((from, to)) = ch.stage {
        let names: Vec<(Uuid, String, String)> = sqlx::query_as("SELECT id, name, color FROM crm_stages WHERE id = ANY($1)")
            .bind(vec![from.unwrap_or(to), to])
            .fetch_all(&state.db)
            .await?;
        let name = |s: Uuid| names.iter().find(|n| n.0 == s).map(|n| json!({ "id": n.0, "name": n.1, "color": n.2 }));
        timeline(&state.db, id, "stage", Some(actor), None, json!({ "from": from.and_then(name), "to": name(to) })).await?;
        let to_name = names.iter().find(|n| n.0 == to).map(|n| n.1.clone()).unwrap_or_default();
        super::collab::tell_followers(state, id, actor, false, format!("{{record}} moved to {to_name}"), None).await;
    }
    if let Some((_, Some(to))) = ch.assigned {
        let who: String = sqlx::query_scalar("SELECT first_name || ' ' || last_name FROM users WHERE id = $1").bind(to).fetch_one(&state.db).await?;
        timeline(&state.db, id, "field", Some(actor), None, json!({ "field": "assigned_to", "to": who })).await?;
        if to != actor {
            notify(
                state,
                &[to],
                NewNotification {
                    kind: "crm.assigned",
                    title: format!("{} assigned to you: {title}", t.name),
                    body: None,
                    link: Some(format!("/crm/{}/{id}", t.code)),
                    actor: Some(actor),
                },
            )
            .await;
        }
    }
    Ok(())
}

async fn next_number(tx: &mut sqlx::Transaction<'_, Postgres>, t: i16) -> AppResult<i64> {
    Ok(sqlx::query_scalar("UPDATE crm_counters SET value = value + 1 WHERE entity_type = $1 RETURNING value")
        .bind(t)
        .fetch_one(&mut **tx)
        .await?)
}

/// Creates a record from a JSON body (used by the API, conversion and import).
pub async fn create_item(state: &AppState, auth: &AuthUser, t: &EntityType, mut body: Map<String, Value>, source_item: Option<Uuid>) -> AppResult<Uuid> {
    access::require_any(state, auth, t.id, "add").await?;
    if !body.contains_key("assigned_to") {
        body.insert("assigned_to".into(), json!(auth.id));
    }
    let (cat, stg) = default_placement(state, t).await?;
    if t.has_stages && !body.contains_key("stage_id") && !body.contains_key("category_id")
        && let Some(s) = stg {
            body.insert("stage_id".into(), json!(s));
        }
    if t.has_amount && !body.contains_key("currency") {
        body.insert("currency".into(), json!(base_currency(state).await?));
    }
    let mut tx = state.db.begin().await?;
    let number = next_number(&mut tx, t.id).await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO crm_items (entity_type, number, title, category_id, created_by, source_item_id, position)
         VALUES ($1, $2, '', $3, $4, $5, (-extract(epoch from now()))::float8) RETURNING id",
    )
    .bind(t.id)
    .bind(number)
    .bind(cat)
    .bind(auth.id)
    .bind(source_item)
    .fetch_one(&mut *tx)
    .await?;
    let ch = apply(state, &mut tx, t, id, None, &body, auth.id).await?;
    refresh_derived(state, &mut tx, t, id).await?;
    timeline(&mut *tx, id, "created", Some(auth.id), None, json!({})).await?;
    tx.commit().await?;

    let title: String = sqlx::query_scalar("SELECT title FROM crm_items WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let entered = ch.stage.map(|(_, to)| to);
    let ch = Changes { stage: None, ..ch }; // creation is not a stage change
    record_changes(state, id, t, &title, &ch, auth.id).await?;
    broadcast(state, t.id, id, false).await;
    crate::api::webhooks::emit_crm_item(state, "crm.item.created", id, json!({})).await;
    // Robots of the stage the record starts in.
    if let Some(stage) = entered {
        super::automation::stage_entered(state, id, stage).await;
    }
    Ok(id)
}

async fn create(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(body): Json<Map<String, Value>>) -> AppResult<Json<Detail>> {
    let t = entity_type(&state, &code).await?;
    let id = create_item(&state, &auth, &t, body, None).await?;
    Ok(Json(load_detail(&state, &auth, &t, id).await?))
}

/// Updates a record from a JSON body (API, Kanban moves, bulk actions, merge).
/// A posted invoice keeps its amount, currency and client (other fields and the stage can change).
async fn locked_invoice_check(state: &AppState, current: &Row, body: &Map<String, Value>) -> AppResult<()> {
    let refuse = |what: &str| Err(AppError::bad(format!("this invoice is in the books, so its {what} can't change; void it in Accounting first")));
    if let Some(v) = body.get("opportunity") {
        let new = v.as_str().map(String::from).unwrap_or_else(|| v.to_string()).parse::<Decimal>().ok();
        if new.map(|n| n.round_dp(2)) != Some(current.opportunity.round_dp(2)) {
            return refuse("amount");
        }
    }
    if let Some(v) = body.get("currency") {
        if v.as_str() != current.currency.as_deref() {
            return refuse("currency");
        }
    }
    if let Some(v) = body.get("is_manual_amount") {
        if v.as_bool() != Some(current.is_manual_amount) {
            return refuse("amount");
        }
    }
    if let Some(v) = body.get("company_id") {
        if v.as_str().and_then(|s| Uuid::parse_str(s).ok()) != current.company_id {
            return refuse("client");
        }
    }
    if let Some(v) = body.get("contact_ids") {
        let mut new: Vec<Uuid> = v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().and_then(|s| Uuid::parse_str(s).ok())).collect()).unwrap_or_default();
        let mut now: Vec<Uuid> = sqlx::query_scalar("SELECT contact_id FROM crm_item_contacts WHERE item_id = $1").bind(current.id).fetch_all(&state.db).await?;
        new.sort();
        now.sort();
        if new != now {
            return refuse("contacts");
        }
    }
    Ok(())
}

pub async fn update_item(state: &AppState, auth: &AuthUser, t: &EntityType, id: Uuid, body: Map<String, Value>) -> AppResult<()> {
    let current = load_row(state, id, false).await?;
    if current.entity_type != t.id {
        return Err(AppError::NotFound("record"));
    }
    require(state, auth, t.id, "read", current.assigned_to).await?;
    require(state, auth, t.id, "write", current.assigned_to).await?;
    if crate::accounting::invoices::locked(state, id).await? {
        locked_invoice_check(state, &current, &body).await?;
    }
    if let Some(Some(new_owner)) = body.get("assigned_to").map(|v| v.as_str().and_then(|s| Uuid::parse_str(s).ok())) {
        // Handing a record to someone outside your edit scope is allowed (that's how leads
        // are distributed), so no further check here.
        let _ = new_owner;
    }
    let before = super::collab::snapshot(state, &current).await?;
    let mut tx = state.db.begin().await?;
    let ch = apply(state, &mut tx, t, id, Some(&current), &body, auth.id).await?;
    refresh_derived(state, &mut tx, t, id).await?;
    tx.commit().await?;
    let changes = super::collab::record_edit(state, t, &before, id, auth.id).await?;
    if !changes.is_empty() {
        crate::api::webhooks::emit_crm_item(state, "crm.item.updated", id, json!({ "changes": changes })).await;
    }
    if let Some((from, _)) = ch.stage {
        crate::api::webhooks::emit_crm_item(state, "crm.item.stage_changed", id, json!({ "previous_stage_id": from })).await;
        if t.kind.as_deref() == Some("order") {
            crate::shop::order_stage_changed(state, id).await;
        }
    }
    let title: String = sqlx::query_scalar("SELECT title FROM crm_items WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    record_changes(state, id, t, &title, &ch, auth.id).await?;
    broadcast(state, t.id, id, false).await;
    match ch.stage {
        Some((_, stage)) => super::automation::stage_entered(state, id, stage).await,
        // Robots timed from a date, or waiting for conditions, look at the new values.
        None => super::automation::record_changed(state, id).await,
    }
    Ok(())
}

async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((code, id)): Path<(String, Uuid)>,
    Json(body): Json<Map<String, Value>>,
) -> AppResult<Json<Detail>> {
    let t = entity_type(&state, &code).await?;
    update_item(&state, &auth, &t, id, body).await?;
    Ok(Json(load_detail(&state, &auth, &t, id).await?))
}

/// Moves a record to the recycle bin.
pub async fn soft_delete(state: &AppState, auth: &AuthUser, t: &EntityType, id: Uuid) -> AppResult<()> {
    let current = load_row(state, id, false).await?;
    if current.entity_type != t.id {
        return Err(AppError::NotFound("record"));
    }
    if crate::accounting::invoices::locked(state, id).await? {
        return Err(AppError::bad("this invoice is in the books; void it in Accounting before deleting it"));
    }
    require(state, auth, t.id, "read", current.assigned_to).await?;
    require(state, auth, t.id, "delete", current.assigned_to).await?;
    sqlx::query("UPDATE crm_items SET deleted_at = now(), deleted_by = $1 WHERE id = $2")
        .bind(auth.id)
        .bind(id)
        .execute(&state.db)
        .await?;
    broadcast(state, t.id, id, true).await;
    crate::api::webhooks::emit_crm_item(state, "crm.item.deleted", id, json!({})).await;
    Ok(())
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path((code, id)): Path<(String, Uuid)>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    soft_delete(&state, &auth, &t, id).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct Move {
    stage_id: Uuid,
    after_id: Option<Uuid>,
    before_id: Option<Uuid>,
}

/// Kanban drag and drop.
async fn move_item(State(state): State<AppState>, auth: AuthUser, Path((code, id)): Path<(String, Uuid)>, Json(body): Json<Move>) -> AppResult<Json<Detail>> {
    let t = entity_type(&state, &code).await?;
    let pos = |n: Option<Uuid>| {
        let db = state.db.clone();
        async move {
            match n {
                Some(n) => sqlx::query_scalar::<_, f64>("SELECT position FROM crm_items WHERE id = $1").bind(n).fetch_optional(&db).await,
                None => Ok(None),
            }
        }
    };
    let position = match (pos(body.after_id).await?, pos(body.before_id).await?) {
        (Some(a), Some(b)) => (a + b) / 2.0,
        (Some(a), None) => a + 1.0,
        (None, Some(b)) => b - 1.0,
        (None, None) => 0.0,
    };
    let mut m = Map::new();
    m.insert("stage_id".into(), json!(body.stage_id));
    update_item(&state, &auth, &t, id, m).await?;
    sqlx::query("UPDATE crm_items SET position = $1 WHERE id = $2").bind(position).bind(id).execute(&state.db).await?;
    Ok(Json(load_detail(&state, &auth, &t, id).await?))
}

#[derive(Deserialize)]
struct Bulk {
    ids: Vec<Uuid>,
    /// assign | stage | delete
    action: String,
    value: Option<Value>,
}

/// Applies one action to many records; reports per-record failures instead of aborting.
async fn bulk(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(body): Json<Bulk>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    if body.ids.len() > 500 {
        return Err(AppError::bad("select at most 500 records at a time"));
    }
    let mut done = 0;
    let mut failed = Vec::new();
    for id in &body.ids {
        let r = match body.action.as_str() {
            "assign" => {
                let mut m = Map::new();
                m.insert("assigned_to".into(), body.value.clone().unwrap_or(Value::Null));
                update_item(&state, &auth, &t, *id, m).await
            }
            "stage" => {
                let mut m = Map::new();
                m.insert("stage_id".into(), body.value.clone().unwrap_or(Value::Null));
                update_item(&state, &auth, &t, *id, m).await
            }
            "delete" => soft_delete(&state, &auth, &t, *id).await,
            _ => return Err(AppError::bad("action must be assign, stage or delete")),
        };
        match r {
            Ok(()) => done += 1,
            Err(e) => failed.push(json!({ "id": id, "error": e.to_string() })),
        }
    }
    Ok(Json(json!({ "done": done, "failed": failed })))
}

// ---------------------------------------------------------------------------
// Lead conversion
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct Convert {
    /// Create a contact from the lead, or link this existing one.
    #[serde(default)]
    pub create_contact: bool,
    pub contact_id: Option<Uuid>,
    #[serde(default)]
    pub create_company: bool,
    pub company_id: Option<Uuid>,
    #[serde(default)]
    pub create_deal: bool,
    pub deal_category_id: Option<Uuid>,
}

/// Turns a lead into a contact, company and/or deal, carrying over its details, then
/// moves the lead to its successful stage.
async fn convert(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(body): Json<Convert>) -> AppResult<Json<Value>> {
    Ok(Json(convert_lead(&state, &auth, id, body).await?))
}

/// Lead conversion, for the API and for automation.
pub async fn convert_lead(state: &AppState, auth: &AuthUser, id: Uuid, body: Convert) -> AppResult<Value> {
    let (state, auth) = (state.clone(), auth.clone());
    let lt = entity_type_by_id(&state, LEAD).await?;
    let lead = load_detail(&state, &auth, &lt, id).await?;
    require(&state, &auth, LEAD, "write", lead.row.assigned_to).await?;
    if lead.row.stage_semantics == "success" {
        return Err(AppError::bad("this lead has already been converted"));
    }
    if !(body.create_contact || body.contact_id.is_some() || body.create_company || body.company_id.is_some() || body.create_deal) {
        return Err(AppError::bad("choose what to create from the lead"));
    }
    let r = &lead.row;
    let mf: Vec<Value> = lead.multifields.iter().map(|m| json!({ "kind": m.kind, "value_type": m.value_type, "value": m.value })).collect();

    let company = match (body.company_id, body.create_company) {
        (Some(c), _) => Some(c),
        (None, true) => {
            let name = r.company_name.clone().ok_or_else(|| AppError::bad("the lead has no company name to create a company from"))?;
            let mut m = Map::new();
            m.insert("company_name".into(), json!(name));
            m.insert("assigned_to".into(), json!(r.assigned_to));
            m.insert("source".into(), json!(r.source));
            m.insert("address".into(), r.address.clone().unwrap_or(Value::Null));
            m.insert("multifields".into(), json!(mf.iter().filter(|x| x["kind"] == "web").cloned().collect::<Vec<_>>()));
            let ct = entity_type_by_id(&state, COMPANY).await?;
            Some(create_item(&state, &auth, &ct, m, Some(id)).await?)
        }
        _ => None,
    };
    let contact = match (body.contact_id, body.create_contact) {
        (Some(c), _) => Some(c),
        (None, true) => {
            let mut m = Map::new();
            for k in ["honorific", "first_name", "middle_name", "last_name", "job_title"] {
                let v = match k {
                    "honorific" => &r.honorific,
                    "first_name" => &r.first_name,
                    "middle_name" => &r.middle_name,
                    "last_name" => &r.last_name,
                    _ => &r.job_title,
                };
                m.insert(k.into(), json!(v));
            }
            if r.first_name.is_none() && r.last_name.is_none() {
                m.insert("first_name".into(), json!(r.title));
            }
            m.insert("birthdate".into(), json!(r.birthdate));
            m.insert("assigned_to".into(), json!(r.assigned_to));
            m.insert("source".into(), json!(r.source));
            m.insert("address".into(), r.address.clone().unwrap_or(Value::Null));
            m.insert("multifields".into(), json!(mf.iter().filter(|x| x["kind"] != "web").cloned().collect::<Vec<_>>()));
            if let Some(c) = company {
                m.insert("company_id".into(), json!(c));
            }
            let ct = entity_type_by_id(&state, CONTACT).await?;
            Some(create_item(&state, &auth, &ct, m, Some(id)).await?)
        }
        _ => None,
    };
    let deal = if body.create_deal {
        let mut m = Map::new();
        m.insert("title".into(), json!(r.title));
        m.insert("assigned_to".into(), json!(r.assigned_to));
        m.insert("source".into(), json!(r.source));
        m.insert("source_details".into(), json!(r.source_details));
        m.insert("opportunity".into(), json!(r.opportunity.to_string()));
        m.insert("currency".into(), json!(r.currency));
        m.insert("is_manual_amount".into(), json!(r.is_manual_amount));
        m.insert("comments".into(), json!(r.comments));
        if let Some(c) = body.deal_category_id {
            m.insert("category_id".into(), json!(c));
        }
        if let Some(c) = company {
            m.insert("company_id".into(), json!(c));
        }
        if let Some(c) = contact {
            m.insert("contact_ids".into(), json!([c]));
        }
        let dt = entity_type_by_id(&state, DEAL).await?;
        let d = create_item(&state, &auth, &dt, m, Some(id)).await?;
        // Product rows travel with the deal.
        sqlx::query(
            "INSERT INTO crm_item_products (item_id, product_id, name, price, quantity, unit, discount_kind, discount_value, tax_rate, tax_included, sort)
             SELECT $1, product_id, name, price, quantity, unit, discount_kind, discount_value, tax_rate, tax_included, sort
             FROM crm_item_products WHERE item_id = $2",
        )
        .bind(d)
        .bind(id)
        .execute(&state.db)
        .await?;
        Some(d)
    } else {
        None
    };

    // Lead → its pipeline's successful stage.
    let success: Option<Uuid> = sqlx::query_scalar("SELECT id FROM crm_stages WHERE category_id = $1 AND semantics = 'success' ORDER BY sort LIMIT 1")
        .bind(r.category_id)
        .fetch_optional(&state.db)
        .await?;
    if let Some(s) = success {
        let mut m = Map::new();
        m.insert("stage_id".into(), json!(s));
        update_item(&state, &auth, &lt, id, m).await?;
    }
    timeline(&state.db, id, "converted", Some(auth.id), None, json!({ "contact_id": contact, "company_id": company, "deal_id": deal })).await?;
    for (target, kind) in [(contact, CONTACT), (company, COMPANY), (deal, DEAL)] {
        if let Some(tid) = target {
            timeline(&state.db, tid, "converted", Some(auth.id), None, json!({ "from_lead": id, "lead_title": r.title, "entity_type": kind })).await?;
        }
    }
    if let Some(d) = deal {
        let _ = totals(&state, d).await; // recompute deal amount from copied products
    }
    Ok(json!({ "contact_id": contact, "company_id": company, "deal_id": deal }))
}

// ---------------------------------------------------------------------------
// Quotes and invoices from deals
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct FromBody {
    category_id: Option<Uuid>,
}

/// Which records a quote or an invoice can be made from.
fn made_from(target: &EntityType, source: &EntityType) -> bool {
    match target.kind.as_deref() {
        Some("quote") => source.id == DEAL,
        Some("invoice") => source.id == DEAL || matches!(source.kind.as_deref(), Some("quote" | "order")),
        _ => false,
    }
}

/// Makes a quote from a deal, or an invoice from a deal or a quote: same title, client,
/// responsible person, currency, amount and products, linked back to its source. Quotes are
/// valid for 30 days and invoices due in 14 by default.
async fn create_from(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((code, source)): Path<(String, Uuid)>,
    body: Option<Json<FromBody>>,
) -> AppResult<Json<Detail>> {
    let t = entity_type(&state, &code).await?;
    let src = load_row(&state, source, false).await?;
    let st = entity_type_by_id(&state, src.entity_type).await?;
    require(&state, &auth, st.id, "read", src.assigned_to).await?;
    if !made_from(&t, &st) {
        return Err(AppError::bad("quotes are made from deals, and invoices from deals or quotes"));
    }
    let contacts: Vec<Uuid> = sqlx::query_scalar("SELECT contact_id FROM crm_item_contacts WHERE item_id = $1 ORDER BY is_primary DESC, sort")
        .bind(source)
        .fetch_all(&state.db)
        .await?;
    let days = if t.kind.as_deref() == Some("quote") { 30 } else { 14 };
    let mut m = Map::new();
    m.insert("title".into(), json!(src.title));
    m.insert("assigned_to".into(), json!(src.assigned_to.unwrap_or(auth.id)));
    m.insert("company_id".into(), json!(src.company_id));
    m.insert("contact_ids".into(), json!(contacts));
    m.insert("currency".into(), json!(src.currency));
    m.insert("opportunity".into(), json!(src.opportunity.to_string()));
    m.insert("is_manual_amount".into(), json!(src.is_manual_amount));
    m.insert("close_date".into(), json!((Utc::now().date_naive() + chrono::Duration::days(days)).to_string()));
    if let Some(c) = body.and_then(|b| b.0.category_id) {
        m.insert("category_id".into(), json!(c));
    }
    let id = create_item(&state, &auth, &t, m, Some(source)).await?;
    sqlx::query(
        "INSERT INTO crm_item_products (item_id, product_id, name, price, quantity, unit, discount_kind, discount_value, tax_rate, tax_included, sort)
         SELECT $1, product_id, name, price, quantity, unit, discount_kind, discount_value, tax_rate, tax_included, sort
         FROM crm_item_products WHERE item_id = $2",
    )
    .bind(id)
    .bind(source)
    .execute(&state.db)
    .await?;
    let _ = totals(&state, id).await;
    let number: i64 = sqlx::query_scalar("SELECT number FROM crm_items WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    timeline(&state.db, source, "linked", Some(auth.id), None, json!({ "item_id": id, "entity_type": t.id, "code": t.code, "number": number, "title": src.title })).await?;
    broadcast(&state, st.id, source, false).await;
    Ok(Json(load_detail(&state, &auth, &t, id).await?))
}

// ---------------------------------------------------------------------------
// Lookup (pickers)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct LookupQuery {
    /// Comma-separated type codes.
    types: String,
    q: String,
}

/// Quick search used by pickers (link a contact, choose a company, …).
async fn lookup(State(state): State<AppState>, auth: AuthUser, Query(q): Query<LookupQuery>) -> AppResult<Json<Vec<Value>>> {
    let mut out = Vec::new();
    for code in q.types.split(',').map(str::trim).filter(|s| !s.is_empty()).take(6) {
        let t = entity_type(&state, code).await?;
        let scope = visible(&state, &auth, t.id, "read").await?;
        if matches!(scope, Visible::Nothing) {
            continue;
        }
        let f = Filter { q: Some(q.q.clone()), ..Default::default() };
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT i.id, i.number, i.title, co.title FROM crm_items i LEFT JOIN crm_items co ON co.id = i.company_id WHERE TRUE");
        if push_filter(&mut qb, &t, &f, auth.id).is_err() {
            continue; // too short to search
        }
        scope.push(&mut qb);
        qb.push(" ORDER BY i.updated_at DESC LIMIT 8");
        let rows: Vec<(Uuid, i64, String, Option<String>)> = qb.build_query_as().fetch_all(&state.db).await?;
        for (id, number, title, company) in rows {
            out.push(json!({ "id": id, "type": t.code, "number": number, "title": title, "subtitle": company }));
        }
    }
    Ok(Json(out))
}
