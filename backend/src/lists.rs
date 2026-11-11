//! Lists: the company's own small databases (purchase requests, assets, contracts…), each
//! with its own fields, and business processes that run on their items (see workflows.rs).
//!
//! Access per list: everyone, or chosen people and departments (with sub-departments), at
//! three levels: read (see items), add (also add items and change your own), manage
//! (change any item, the fields, the processes and who has access). People with "Manage
//! lists" make lists and manage every list; whoever made a list manages it.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/lists", get(list_lists).post(create_list))
        .route("/lists/{id}", get(get_list).patch(update_list).delete(delete_list))
        .route("/lists/{id}/fields", put(put_fields))
        .route("/lists/{id}/items", get(items).post(create_item))
        .route("/lists/{id}/export", get(export))
        .route("/lists/items/{id}", get(get_item).patch(update_item).delete(delete_item))
}

// ---------------------------------------------------------------------------
// Access
// ---------------------------------------------------------------------------

const LEVELS: &[&str] = &["none", "read", "add", "manage"];

fn rank(l: &str) -> usize {
    LEVELS.iter().position(|x| *x == l).unwrap_or(0)
}

/// The user's level on a list.
pub(crate) async fn level(state: &AppState, auth: &AuthUser, list: Uuid) -> AppResult<&'static str> {
    let (to_all, created_by): (bool, Option<Uuid>) = sqlx::query_as("SELECT to_all, created_by FROM lists WHERE id = $1")
        .bind(list)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("list"))?;
    if auth.can(perm::LISTS) || created_by == Some(auth.id) {
        return Ok("manage");
    }
    let rows: Vec<String> = sqlx::query_scalar(
        "WITH RECURSIVE mine AS (SELECT department_id AS id FROM user_departments WHERE user_id = $2
                                 UNION SELECT d.parent_id FROM departments d JOIN mine m ON d.id = m.id WHERE d.parent_id IS NOT NULL)
         SELECT level FROM list_access WHERE list_id = $1 AND (user_id = $2 OR department_id IN (SELECT id FROM mine))",
    )
    .bind(list)
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    let mut best = if to_all { "add" } else { "none" };
    for r in rows {
        if rank(&r) > rank(best) {
            best = LEVELS[rank(&r)];
        }
    }
    Ok(best)
}

pub(crate) async fn need(state: &AppState, auth: &AuthUser, list: Uuid, want: &str) -> AppResult<&'static str> {
    let l = level(state, auth, list).await?;
    if rank(l) == 0 {
        return Err(AppError::NotFound("list"));
    }
    if rank(l) < rank(want) {
        return Err(AppError::Forbidden);
    }
    Ok(l)
}

/// Files in list items are readable by whoever can read the list.
pub async fn can_view_file(state: &AppState, auth: &AuthUser, file: Uuid) -> AppResult<bool> {
    let lists: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT i.list_id FROM list_items i, LATERAL jsonb_each(i.\"values\") v WHERE v.value = to_jsonb($1::text) LIMIT 20",
    )
    .bind(file.to_string())
    .fetch_all(&state.db)
    .await?;
    for l in lists {
        if rank(level(state, auth, l).await?) >= 1 {
            return Ok(true);
        }
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// Lists
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct ListRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    to_all: bool,
    items: i64,
    created_at: DateTime<Utc>,
}

async fn list_lists(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let rows: Vec<ListRow> = sqlx::query_as("SELECT l.id, l.name, l.description, l.to_all, (SELECT COUNT(*) FROM list_items i WHERE i.list_id = l.id) AS items, l.created_at FROM lists l ORDER BY l.name")
        .fetch_all(&state.db)
        .await?;
    let mut out = Vec::new();
    for r in rows {
        let l = level(&state, &auth, r.id).await?;
        if rank(l) > 0 {
            let mut v = serde_json::to_value(&r).unwrap();
            v["level"] = json!(l);
            out.push(v);
        }
    }
    Ok(Json(json!({ "items": out, "can_create": auth.can(perm::LISTS) })))
}

#[derive(Serialize, FromRow, Deserialize, Clone)]
pub(crate) struct Field {
    #[serde(default)]
    pub id: Option<Uuid>,
    #[serde(default)]
    pub code: String,
    pub label: String,
    pub kind: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub required: bool,
}

pub(crate) async fn fields(state: &AppState, list: Uuid) -> AppResult<Vec<Field>> {
    Ok(sqlx::query_as("SELECT id, code, label, kind, options, required FROM list_fields WHERE list_id = $1 ORDER BY sort, label").bind(list).fetch_all(&state.db).await?)
}

#[derive(Deserialize, Default)]
struct Who {
    #[serde(default)]
    users: Vec<Uuid>,
    #[serde(default)]
    departments: Vec<Uuid>,
}

#[derive(Deserialize)]
struct Access {
    #[serde(default)]
    read: Who,
    #[serde(default)]
    add: Who,
    #[serde(default)]
    manage: Who,
}

#[derive(Deserialize)]
struct ListInput {
    name: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    to_all: Option<bool>,
    access: Option<Access>,
    fields: Option<Vec<Field>>,
}

async fn set_access(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, list: Uuid, a: &Access) -> AppResult<()> {
    sqlx::query("DELETE FROM list_access WHERE list_id = $1").bind(list).execute(&mut **tx).await?;
    for (level, w) in [("read", &a.read), ("add", &a.add), ("manage", &a.manage)] {
        for u in &w.users {
            sqlx::query("INSERT INTO list_access (list_id, user_id, level) VALUES ($1, $2, $3)").bind(list).bind(u).bind(level).execute(&mut **tx).await?;
        }
        for d in &w.departments {
            sqlx::query("INSERT INTO list_access (list_id, department_id, level) VALUES ($1, $2, $3)").bind(list).bind(d).bind(level).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

const KINDS: &[&str] = &["text", "textarea", "number", "money", "date", "select", "user", "checkbox", "file", "crm"];

/// Field codes come from labels: "Cost centre" → cost_centre.
fn code_of(label: &str) -> String {
    let s: String = label.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let s = s.split('_').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("_");
    let s: String = s.chars().take(40).collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) { format!("f_{s}") } else { s }
}

pub(crate) async fn save_fields(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, list: Uuid, list_fields: &[Field]) -> AppResult<()> {
    if list_fields.len() > 60 {
        return Err(AppError::bad("at most 60 fields"));
    }
    let mut codes: Vec<String> = Vec::new();
    let mut keep: Vec<Uuid> = Vec::new();
    for (i, f) in list_fields.iter().enumerate() {
        let label = required("field name", &f.label, 100)?;
        if !KINDS.contains(&f.kind.as_str()) {
            return Err(AppError::bad(format!("“{label}”: unknown kind of field")));
        }
        let options: Vec<String> = f.options.iter().map(|o| o.trim().to_string()).filter(|o| !o.is_empty()).collect();
        if f.kind == "select" && options.is_empty() {
            return Err(AppError::bad(format!("“{label}” needs choices")));
        }
        let code = if f.code.trim().is_empty() { code_of(&label) } else { code_of(&f.code) };
        if codes.contains(&code) || ["title", "number", "author", "created", "id"].contains(&code.as_str()) {
            return Err(AppError::bad(format!("two fields can't both be called “{label}”")));
        }
        codes.push(code.clone());
        let id: Uuid = match f.id {
            // The kind of an existing field stays (its values were checked against it).
            Some(id) => sqlx::query_scalar("UPDATE list_fields SET label = $3, options = $4, required = $5, sort = $6 WHERE id = $1 AND list_id = $2 RETURNING id")
                .bind(id)
                .bind(list)
                .bind(&label)
                .bind(&options)
                .bind(f.required)
                .bind(i as i32 * 10)
                .fetch_optional(&mut **tx)
                .await?
                .ok_or(AppError::NotFound("field"))?,
            None => sqlx::query_scalar("INSERT INTO list_fields (list_id, code, label, kind, options, required, sort) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id")
                .bind(list)
                .bind(&code)
                .bind(&label)
                .bind(&f.kind)
                .bind(&options)
                .bind(f.required)
                .bind(i as i32 * 10)
                .fetch_one(&mut **tx)
                .await
                .map_err(|e| match e {
                    sqlx::Error::Database(d) if d.is_unique_violation() => AppError::bad(format!("a field called “{label}” exists")),
                    e => e.into(),
                })?,
        };
        keep.push(id);
    }
    sqlx::query("DELETE FROM list_fields WHERE list_id = $1 AND NOT id = ANY($2)").bind(list).bind(&keep).execute(&mut **tx).await?;
    Ok(())
}

async fn create_list(State(state): State<AppState>, auth: AuthUser, Json(b): Json<ListInput>) -> AppResult<Json<Value>> {
    auth.require(perm::LISTS)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 120)?;
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO lists (name, description, to_all, created_by) VALUES ($1, $2, $3, $4) RETURNING id")
        .bind(&name)
        .bind(clean(b.description.flatten()))
        .bind(b.to_all.unwrap_or(true))
        .bind(auth.id)
        .fetch_one(&mut *tx)
        .await?;
    if let Some(a) = &b.access {
        set_access(&mut tx, id, a).await?;
    }
    save_fields(&mut tx, id, &b.fields.unwrap_or_default()).await?;
    tx.commit().await?;
    get_list(State(state), auth, Path(id)).await
}

async fn get_list(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let l = need(&state, &auth, id, "read").await?;
    let (name, description, to_all): (String, Option<String>, bool) = sqlx::query_as("SELECT name, description, to_all FROM lists WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let mut v = json!({ "id": id, "name": name, "description": description, "to_all": to_all, "level": l, "fields": fields(&state, id).await? });
    let wfs: Vec<(Uuid, String, String, bool)> = sqlx::query_as("SELECT id, name, trigger, is_active FROM wf_templates WHERE list_id = $1 ORDER BY name").bind(id).fetch_all(&state.db).await?;
    v["workflows"] = json!(wfs.into_iter().map(|(id, n, t, a)| json!({ "id": id, "name": n, "trigger": t, "is_active": a })).collect::<Vec<_>>());
    if l == "manage" {
        let rows: Vec<(String, Option<Uuid>, Option<Uuid>, Option<String>)> = sqlx::query_as(
            "SELECT a.level, a.user_id, a.department_id, COALESCE(trim(u.first_name || ' ' || u.last_name), d.name) FROM list_access a
             LEFT JOIN users u ON u.id = a.user_id LEFT JOIN departments d ON d.id = a.department_id WHERE a.list_id = $1",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await?;
        let pick = |level: &str| json!({
            "users": rows.iter().filter(|r| r.0 == level && r.1.is_some()).map(|r| json!({ "id": r.1, "name": r.3, "avatar_file_id": null })).collect::<Vec<_>>(),
            "departments": rows.iter().filter(|r| r.0 == level && r.2.is_some()).map(|r| r.2).collect::<Vec<_>>(),
        });
        v["access"] = json!({ "read": pick("read"), "add": pick("add"), "manage": pick("manage") });
    }
    Ok(Json(v))
}

async fn update_list(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ListInput>) -> AppResult<Json<Value>> {
    need(&state, &auth, id, "manage").await?;
    let name = b.name.as_deref().map(|n| required("name", n, 120)).transpose()?;
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE lists SET name = COALESCE($2, name), description = CASE WHEN $3 THEN $4 ELSE description END, to_all = COALESCE($5, to_all) WHERE id = $1")
        .bind(id)
        .bind(name)
        .bind(b.description.is_some())
        .bind(clean(b.description.flatten()))
        .bind(b.to_all)
        .execute(&mut *tx)
        .await?;
    if let Some(a) = &b.access {
        set_access(&mut tx, id, a).await?;
    }
    if let Some(f) = &b.fields {
        save_fields(&mut tx, id, f).await?;
    }
    tx.commit().await?;
    get_list(State(state), auth, Path(id)).await
}

async fn put_fields(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(f): Json<Vec<Field>>) -> AppResult<Json<Value>> {
    need(&state, &auth, id, "manage").await?;
    let mut tx = state.db.begin().await?;
    save_fields(&mut tx, id, &f).await?;
    tx.commit().await?;
    get_list(State(state), auth, Path(id)).await
}

async fn delete_list(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&state, &auth, id, "manage").await?;
    sqlx::query("DELETE FROM lists WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

/// Checks values against the fields; unknown fields are refused. Returns the cleaned values.
async fn clean_values(state: &AppState, auth: &AuthUser, fs: &[Field], input: &Map<String, Value>, current: Option<&Value>, creating: bool) -> AppResult<Map<String, Value>> {
    let mut out = current.and_then(|c| c.as_object().cloned()).unwrap_or_default();
    for k in input.keys() {
        if !fs.iter().any(|f| f.code == *k) {
            return Err(AppError::bad(format!("unknown field “{k}”")));
        }
    }
    for f in fs {
        let Some(v) = input.get(&f.code) else {
            if creating && f.required {
                return Err(AppError::bad(format!("“{}” is required", f.label)));
            }
            continue;
        };
        let empty = v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty());
        if empty {
            if f.required {
                return Err(AppError::bad(format!("“{}” is required", f.label)));
            }
            out.remove(&f.code);
            continue;
        }
        let bad = || AppError::bad(format!("“{}” is not valid", f.label));
        let s = v.as_str().map(str::trim);
        let clean = match f.kind.as_str() {
            "text" | "textarea" => {
                let t = s.ok_or_else(bad)?;
                if t.chars().count() > if f.kind == "text" { 1000 } else { 20_000 } {
                    return Err(AppError::bad(format!("“{}” is too long", f.label)));
                }
                json!(t)
            }
            "number" => json!(v.as_f64().or_else(|| s.and_then(|x| x.replace(',', ".").parse().ok())).ok_or_else(bad)?),
            "money" => {
                let d: rust_decimal::Decimal = s.map(|x| x.replace(',', ".")).or_else(|| v.as_f64().map(|n| n.to_string())).and_then(|x| x.parse().ok()).ok_or_else(bad)?;
                json!(format!("{:.2}", d.round_dp(2)))
            }
            "date" => json!(NaiveDate::parse_from_str(s.ok_or_else(bad)?, "%Y-%m-%d").map_err(|_| bad())?.to_string()),
            "select" => json!(f.options.iter().find(|o| Some(o.as_str()) == s).ok_or_else(bad)?),
            "checkbox" => json!(v.as_bool().ok_or_else(bad)?),
            "user" => {
                let id = s.and_then(|x| Uuid::parse_str(x).ok()).ok_or_else(bad)?;
                let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND is_active)").bind(id).fetch_one(&state.db).await?;
                if !ok {
                    return Err(bad());
                }
                json!(id)
            }
            "file" => {
                let id = s.and_then(|x| Uuid::parse_str(x).ok()).ok_or_else(bad)?;
                let already = current.and_then(|c| c.get(&f.code)).and_then(Value::as_str) == Some(&id.to_string());
                let own: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM files WHERE id = $1 AND owner_id = $2)").bind(id).bind(auth.id).fetch_one(&state.db).await?;
                if !already && !own {
                    return Err(AppError::bad(format!("“{}”: attach a file you uploaded", f.label)));
                }
                json!(id)
            }
            _ => {
                let id = s.and_then(|x| Uuid::parse_str(x).ok()).ok_or_else(bad)?;
                let row = crate::crm::items::load_row(state, id, false).await.map_err(|_| bad())?;
                crate::crm::access::require_read(state, auth, row.entity_type, id, row.assigned_to).await.map_err(|_| bad())?;
                json!(id)
            }
        };
        out.insert(f.code.clone(), clean);
    }
    Ok(out)
}

#[derive(Serialize, FromRow)]
struct ItemRow {
    id: Uuid,
    list_id: Uuid,
    number: i64,
    title: String,
    values: Value,
    created_by: Option<Uuid>,
    author_name: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    /// The latest business process on it.
    process: Option<String>,
    process_status: Option<String>,
}

const ITEM_SELECT: &str = "SELECT i.id, i.list_id, i.number, i.title, i.\"values\", i.created_by, trim(u.first_name || ' ' || u.last_name) AS author_name, i.created_at, i.updated_at,
    (SELECT w.name FROM wf_instances w WHERE w.item_id = i.id ORDER BY w.started_at DESC LIMIT 1) AS process,
    (SELECT w.status FROM wf_instances w WHERE w.item_id = i.id ORDER BY w.started_at DESC LIMIT 1) AS process_status
    FROM list_items i LEFT JOIN users u ON u.id = i.created_by";

#[derive(Deserialize)]
struct ItemsQuery {
    q: Option<String>,
    mine: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn items(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<ItemsQuery>) -> AppResult<Json<Value>> {
    need(&state, &auth, id, "read").await?;
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let term = clean(q.q).map(|t| format!("%{}%", t.to_lowercase().replace('%', "\\%").replace('_', "\\_")));
    let filter = "i.list_id = $1 AND ($2::text IS NULL OR lower(i.title) LIKE $2 OR lower(i.\"values\"::text) LIKE $2) AND (NOT $3 OR i.created_by = $4)";
    let total: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM list_items i WHERE {filter}")))
        .bind(id)
        .bind(&term)
        .bind(q.mine.unwrap_or(false))
        .bind(auth.id)
        .fetch_one(&state.db)
        .await?;
    let rows: Vec<ItemRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{ITEM_SELECT} WHERE {filter} ORDER BY i.number DESC LIMIT $5 OFFSET $6")))
        .bind(id)
        .bind(&term)
        .bind(q.mine.unwrap_or(false))
        .bind(auth.id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!({ "items": rows, "total": total, "limit": limit, "offset": offset })))
}

pub(crate) async fn item_row(state: &AppState, id: Uuid) -> AppResult<(Uuid, Option<Uuid>)> {
    sqlx::query_as("SELECT list_id, created_by FROM list_items WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("item"))
}

async fn item_view(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Value> {
    let row: ItemRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("{ITEM_SELECT} WHERE i.id = $1"))).bind(id).fetch_one(&state.db).await?;
    let l = level(state, auth, row.list_id).await?;
    let mut v = serde_json::to_value(&row).unwrap();
    v["can_edit"] = json!(l == "manage" || (l == "add" && row.created_by == Some(auth.id)));
    v["processes"] = crate::workflows::item_processes(state, id).await?;
    // Names for people, files and CRM records in the values.
    let fs = fields(state, row.list_id).await?;
    let mut labels = Map::new();
    for f in &fs {
        let Some(val) = row.values.get(&f.code).and_then(Value::as_str) else { continue };
        let Ok(uid) = Uuid::parse_str(val) else { continue };
        let label: Option<String> = match f.kind.as_str() {
            "user" => sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(uid).fetch_optional(&state.db).await?,
            "file" => sqlx::query_scalar("SELECT original_name FROM files WHERE id = $1").bind(uid).fetch_optional(&state.db).await?,
            "crm" => sqlx::query_scalar("SELECT t.code || ':' || i.title FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type WHERE i.id = $1").bind(uid).fetch_optional(&state.db).await?,
            _ => None,
        };
        if let Some(lb) = label {
            labels.insert(f.code.clone(), json!(lb));
        }
    }
    v["labels"] = Value::Object(labels);
    Ok(v)
}

async fn get_item(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (list, _) = item_row(&state, id).await?;
    need(&state, &auth, list, "read").await.map_err(|_| AppError::NotFound("item"))?;
    item_view(&state, &auth, id).await.map(Json)
}

#[derive(Deserialize)]
struct ItemInput {
    title: Option<String>,
    #[serde(default)]
    values: Map<String, Value>,
}

/// Adds an item for someone (automation acting for them): their rights apply, and the list's
/// processes start as for any new item.
pub(crate) async fn add_item(state: &AppState, auth: AuthUser, list: Uuid, title: String, values: Map<String, Value>) -> AppResult<Value> {
    create_item(State(state.clone()), auth, Path(list), Json(ItemInput { title: Some(title), values })).await.map(|j| j.0)
}

/// Whether this person may add items to the list.
pub(crate) async fn may_add(state: &AppState, auth: &AuthUser, list: Uuid) -> bool {
    need(state, auth, list, "add").await.is_ok()
}

/// The list's fields as (code, kind), in order.
pub(crate) async fn field_kinds(state: &AppState, list: Uuid) -> AppResult<Vec<(String, String)>> {
    Ok(fields(state, list).await?.into_iter().map(|f| (f.code, f.kind)).collect())
}

async fn create_item(State(state): State<AppState>, auth: AuthUser, Path(list): Path<Uuid>, Json(b): Json<ItemInput>) -> AppResult<Json<Value>> {
    need(&state, &auth, list, "add").await?;
    let title = required("title", b.title.as_deref().unwrap_or(""), 300)?;
    let fs = fields(&state, list).await?;
    let values = clean_values(&state, &auth, &fs, &b.values, None, true).await?;
    let mut tx = state.db.begin().await?;
    let n: i64 = sqlx::query_scalar("UPDATE lists SET last_number = last_number + 1 WHERE id = $1 RETURNING last_number").bind(list).fetch_one(&mut *tx).await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO list_items (list_id, number, title, \"values\", created_by, updated_by) VALUES ($1, $2, $3, $4, $5, $5) RETURNING id")
        .bind(list)
        .bind(n)
        .bind(&title)
        .bind(Value::Object(values))
        .bind(auth.id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    crate::workflows::on_item(&state, &auth, list, id, "create").await;
    item_view(&state, &auth, id).await.map(Json)
}

async fn update_item(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ItemInput>) -> AppResult<Json<Value>> {
    let (list, author) = item_row(&state, id).await?;
    let l = need(&state, &auth, list, "add").await?;
    if l != "manage" && author != Some(auth.id) {
        return Err(AppError::Forbidden);
    }
    let title = b.title.as_deref().map(|t| required("title", t, 300)).transpose()?;
    let current: Value = sqlx::query_scalar("SELECT \"values\" FROM list_items WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let fs = fields(&state, list).await?;
    let values = clean_values(&state, &auth, &fs, &b.values, Some(&current), false).await?;
    sqlx::query("UPDATE list_items SET title = COALESCE($2, title), \"values\" = $3, updated_by = $4, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(title)
        .bind(Value::Object(values))
        .bind(auth.id)
        .execute(&state.db)
        .await?;
    crate::workflows::on_item(&state, &auth, list, id, "update").await;
    item_view(&state, &auth, id).await.map(Json)
}

async fn delete_item(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (list, author) = item_row(&state, id).await?;
    let l = need(&state, &auth, list, "add").await?;
    if l != "manage" && author != Some(auth.id) {
        return Err(AppError::Forbidden);
    }
    sqlx::query("DELETE FROM list_items WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

/// The list as CSV: number, title, author, created, then one column per field.
async fn export(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Response> {
    need(&state, &auth, id, "read").await?;
    let fs = fields(&state, id).await?;
    let rows: Vec<ItemRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{ITEM_SELECT} WHERE i.list_id = $1 ORDER BY i.number LIMIT 50000"))).bind(id).fetch_all(&state.db).await?;
    let cell = |s: &str| {
        // Spreadsheet formulas are neutralised; quotes doubled.
        let s = if s.starts_with(['=', '+', '-', '@']) { format!("'{s}") } else { s.to_string() };
        format!("\"{}\"", s.replace('"', "\"\""))
    };
    let mut out = String::from("\u{feff}");
    let head: Vec<String> = ["Number", "Title", "Author", "Created"].iter().map(|h| cell(h)).chain(fs.iter().map(|f| cell(&f.label))).collect();
    out.push_str(&head.join(","));
    out.push_str("\r\n");
    for r in rows {
        let mut line = vec![r.number.to_string(), cell(&r.title), cell(r.author_name.as_deref().unwrap_or("")), r.created_at.format("%Y-%m-%d %H:%M").to_string()];
        for f in &fs {
            let v = r.values.get(&f.code).map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default();
            line.push(cell(&v));
        }
        out.push_str(&line.join(","));
        out.push_str("\r\n");
    }
    let mut res = out.into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/csv; charset=utf-8"));
    res.headers_mut().insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"list.csv\""));
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_from_labels() {
        assert_eq!(code_of("Cost centre"), "cost_centre");
        assert_eq!(code_of("  Amount (€) "), "amount");
        assert_eq!(code_of("2nd approver"), "f_2nd_approver");
    }
}
