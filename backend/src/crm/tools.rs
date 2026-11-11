//! Data tools: duplicate detection and merge, CSV import (background job) and export
//! (streamed), recycle bin with automatic purge, and saved filters.

use anyhow::{anyhow, Result};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use futures_util::{stream, StreamExt};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::{
    access::{is_crm_admin, require, require_any, visible, Visible},
    broadcast, entity_type, entity_type_by_id,
    items::{create_item, load_row, normalize, push_filter, refresh_derived, update_item, Filter},
    timeline, EntityType, COMPANY,
};
use crate::{
    auth::{acting_as, AuthUser},
    error::{AppError, AppResult},
    jobs,
    notify::{notify, NewNotification},
    routes::{clean, required, Page},
    state::AppState,
};

pub const IMPORT_JOB: &str = "crm.import";
pub const PURGE_JOB: &str = "crm.purge";
const RECYCLE_DAYS: i64 = 30;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/{type}/duplicates", get(duplicates))
        .route("/crm/{type}/merge", post(merge))
        .route("/crm/{type}/duplicate-groups", get(duplicate_groups))
        .route("/crm/{type}/duplicate-groups/ignore", post(ignore_group))
        .route("/crm/{type}/duplicate-groups/merge", post(merge_groups))
        .route("/crm/{type}/import", post(start_import))
        .route("/crm/imports/{job}", get(import_status))
        .route("/crm/{type}/export", get(export))
        .route("/crm/{type}/export.xlsx", get(export_xlsx))
        .route("/crm/{type}/export.vcf", get(export_vcf))
        .route("/crm/recycle-bin", get(recycle_list))
        .route("/crm/recycle-bin/{id}/restore", post(restore))
        .route("/crm/recycle-bin/{id}", delete(purge_one))
        .route("/crm/{type}/filters", get(list_filters).post(save_filter))
        .route("/crm/filters/{id}", delete(delete_filter))
}

// ---------------------------------------------------------------------------
// Duplicates
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DupQuery {
    /// Comma-separated phones and emails to check.
    phones: Option<String>,
    emails: Option<String>,
    /// Full name (contacts/leads) or company name.
    name: Option<String>,
    exclude: Option<Uuid>,
}

#[derive(Serialize, FromRow)]
struct Dup {
    id: Uuid,
    number: i64,
    title: String,
    assigned_to: Option<Uuid>,
    reason: String,
}

/// Records of the same type sharing a phone, email or exact name. Records the caller
/// can't see are counted but not shown.
async fn duplicates(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Query(q): Query<DupQuery>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    let split = |s: &Option<String>, kind: &str| -> Vec<String> {
        s.as_deref().unwrap_or("").split(',').map(|v| normalize(kind, v)).filter(|v| v.len() >= 3).collect()
    };
    let phones = split(&q.phones, "phone");
    let emails = split(&q.emails, "email");
    let name = clean(q.name).map(|n| n.to_lowercase());
    let found: Vec<Dup> = sqlx::query_as(
        "SELECT DISTINCT ON (i.id) i.id, i.number, i.title, i.assigned_to, d.reason FROM (
             SELECT m.item_id AS id, 'phone' AS reason FROM crm_multifields m WHERE m.kind = 'phone' AND m.normalized = ANY($2)
             UNION ALL SELECT m.item_id, 'email' FROM crm_multifields m WHERE m.kind = 'email' AND m.normalized = ANY($3)
             UNION ALL SELECT i2.id, 'name' FROM crm_items i2 WHERE $4::text IS NOT NULL AND i2.entity_type = $1 AND lower(i2.title) = $4
         ) d JOIN crm_items i ON i.id = d.id
         WHERE i.entity_type = $1 AND i.deleted_at IS NULL AND ($5::uuid IS NULL OR i.id <> $5)
         LIMIT 50",
    )
    .bind(t.id)
    .bind(&phones)
    .bind(&emails)
    .bind(&name)
    .bind(q.exclude)
    .fetch_all(&state.db)
    .await?;
    let scope = visible(&state, &auth, t.id, "read").await?;
    let (shown, hidden): (Vec<Dup>, Vec<Dup>) = found.into_iter().partition(|d| scope.allows(d.assigned_to));
    Ok(Json(json!({ "items": shown, "hidden_count": hidden.len() })))
}

#[derive(Deserialize)]
struct Merge {
    target_id: Uuid,
    source_ids: Vec<Uuid>,
}

/// Merges records into one: the target keeps its values, gaps are filled from the sources,
/// and contact details, links, timeline and activities move over. Sources are removed.
async fn merge(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(b): Json<Merge>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    merge_records(&state, &auth, &t, &b).await?;
    Ok(Json(json!({ "ok": true, "id": b.target_id })))
}

async fn merge_records(state: &AppState, auth: &AuthUser, t: &EntityType, b: &Merge) -> AppResult<()> {
    let (state, auth) = (state.clone(), auth.clone());
    if b.source_ids.is_empty() || b.source_ids.len() > 20 || b.source_ids.contains(&b.target_id) {
        return Err(AppError::bad("choose between 1 and 20 other records to merge into the target"));
    }
    let target = load_row(&state, b.target_id, false).await?;
    if target.entity_type != t.id {
        return Err(AppError::NotFound("record"));
    }
    require(&state, &auth, t.id, "write", target.assigned_to).await?;
    for s in &b.source_ids {
        let r = load_row(&state, *s, false).await?;
        if r.entity_type != t.id {
            return Err(AppError::bad("all merged records must be of the same type"));
        }
        require(&state, &auth, t.id, "delete", r.assigned_to).await?;
    }
    let mut tx = state.db.begin().await?;
    for s in &b.source_ids {
        // Contact details the target doesn't have yet.
        sqlx::query(
            "UPDATE crm_multifields m SET item_id = $1, sort = sort + 100 WHERE m.item_id = $2
             AND NOT EXISTS (SELECT 1 FROM crm_multifields x WHERE x.item_id = $1 AND x.kind = m.kind AND x.normalized = m.normalized)",
        )
        .bind(b.target_id)
        .bind(s)
        .execute(&mut *tx)
        .await?;
        // Links: deals/leads/companies pointing at a merged contact or company.
        sqlx::query(
            "INSERT INTO crm_item_contacts (item_id, contact_id, is_primary, sort)
             SELECT item_id, $1, is_primary, sort FROM crm_item_contacts WHERE contact_id = $2 ON CONFLICT DO NOTHING",
        )
        .bind(b.target_id)
        .bind(s)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO crm_item_contacts (item_id, contact_id, is_primary, sort)
             SELECT $1, contact_id, FALSE, sort + 100 FROM crm_item_contacts WHERE item_id = $2 ON CONFLICT DO NOTHING",
        )
        .bind(b.target_id)
        .bind(s)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE crm_items SET company_id = $1 WHERE company_id = $2").bind(b.target_id).bind(s).execute(&mut *tx).await?;
        sqlx::query("UPDATE crm_timeline SET item_id = $1 WHERE item_id = $2").bind(b.target_id).bind(s).execute(&mut *tx).await?;
        // Requisites move over (never as the default if the target already has one), and
        // so do generated documents.
        sqlx::query("UPDATE crm_requisites SET item_id = $1, is_default = is_default AND NOT EXISTS (SELECT 1 FROM crm_requisites x WHERE x.item_id = $1) WHERE item_id = $2")
            .bind(b.target_id)
            .bind(s)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE crm_requisite_ids SET item_id = $1 WHERE item_id = $2").bind(b.target_id).bind(s).execute(&mut *tx).await?;
        sqlx::query("UPDATE crm_documents SET item_id = $1 WHERE item_id = $2").bind(b.target_id).bind(s).execute(&mut *tx).await?;
        sqlx::query("UPDATE sign_requests SET item_id = $1 WHERE item_id = $2").bind(b.target_id).bind(s).execute(&mut *tx).await?;
        sqlx::query("UPDATE chat_sessions SET item_id = $1 WHERE item_id = $2").bind(b.target_id).bind(s).execute(&mut *tx).await?;
        crate::accounting::merged(&mut tx, *s, b.target_id).await?;
        // A merged shop order keeps its customer's order page (one per record).
        sqlx::query("UPDATE shop_orders SET item_id = $1 WHERE item_id = $2 AND NOT EXISTS (SELECT 1 FROM shop_orders WHERE item_id = $1)")
            .bind(b.target_id)
            .bind(s)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE crm_activities SET item_id = $1 WHERE item_id = $2").bind(b.target_id).bind(s).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE crm_item_products SET item_id = $1 WHERE item_id = $2
             AND NOT EXISTS (SELECT 1 FROM crm_item_products x WHERE x.item_id = $1)",
        )
        .bind(b.target_id)
        .bind(s)
        .execute(&mut *tx)
        .await?;
        // Fill the target's empty fields; the target's custom field values win.
        sqlx::query(
            "UPDATE crm_items t SET
                honorific = COALESCE(t.honorific, s.honorific), first_name = COALESCE(t.first_name, s.first_name),
                middle_name = COALESCE(t.middle_name, s.middle_name), last_name = COALESCE(t.last_name, s.last_name),
                job_title = COALESCE(t.job_title, s.job_title), birthdate = COALESCE(t.birthdate, s.birthdate),
                company_name = COALESCE(t.company_name, s.company_name), company_kind = COALESCE(t.company_kind, s.company_kind),
                industry = COALESCE(t.industry, s.industry), employees = COALESCE(t.employees, s.employees),
                annual_revenue = COALESCE(t.annual_revenue, s.annual_revenue), source = COALESCE(t.source, s.source),
                source_details = COALESCE(t.source_details, s.source_details), address = COALESCE(t.address, s.address),
                comments = COALESCE(t.comments, s.comments), company_id = COALESCE(t.company_id, s.company_id),
                close_date = COALESCE(t.close_date, s.close_date), uf = s.uf || t.uf, updated_at = now()
             FROM crm_items s WHERE t.id = $1 AND s.id = $2",
        )
        .bind(b.target_id)
        .bind(s)
        .execute(&mut *tx)
        .await?;
        let title: String = sqlx::query_scalar("SELECT title FROM crm_items WHERE id = $1").bind(s).fetch_one(&mut *tx).await?;
        timeline(&mut *tx, b.target_id, "merged", Some(auth.id), None, json!({ "from": s, "title": title })).await?;
        sqlx::query("DELETE FROM crm_item_contacts WHERE item_id = $1 OR contact_id = $1").bind(s).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM crm_items WHERE id = $1").bind(s).execute(&mut *tx).await?;
    }
    refresh_derived(&state, &mut tx, t, b.target_id).await?;
    tx.commit().await?;
    for s in &b.source_ids {
        broadcast(&state, t.id, *s, true).await;
    }
    broadcast(&state, t.id, b.target_id, false).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Duplicate search over a whole record type
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct GroupQuery {
    /// Comma-separated: phone, email, name (all three when left out).
    by: Option<String>,
    /// Only groups whose shared value contains this.
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(FromRow)]
struct GroupRow {
    kind: String,
    value: String,
    shown: String,
    ids: Vec<Uuid>,
    total: i64,
}

#[derive(Serialize, FromRow)]
struct GroupItem {
    id: Uuid,
    number: i64,
    title: String,
    assigned_to: Option<Uuid>,
    assigned_name: Option<String>,
    created_at: DateTime<Utc>,
    phones: Option<String>,
    emails: Option<String>,
    deals: i64,
}

fn dup_kinds(by: &Option<String>) -> AppResult<Vec<String>> {
    let v: Vec<String> = by.as_deref().unwrap_or("phone,email,name").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    if v.is_empty() || v.iter().any(|k| !["phone", "email", "name"].contains(&k.as_str())) {
        return Err(AppError::bad("search by phone, email and/or name"));
    }
    Ok(v)
}

/// Groups of records of a type that share a phone, an email or a name, among the records the
/// person may read (largest groups first). Groups marked "not duplicates" stay out until
/// another record joins them.
async fn duplicate_groups(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Query(q): Query<GroupQuery>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    let scope = visible(&state, &auth, t.id, "read").await?;
    if matches!(scope, Visible::Nothing) {
        return Err(AppError::Forbidden);
    }
    let kinds = dup_kinds(&q.by)?;
    let limit = q.limit.unwrap_or(25).clamp(1, 100);
    let offset = q.offset.unwrap_or(0).max(0);
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("WITH v AS (SELECT i.id, i.title, i.created_at FROM crm_items i WHERE i.entity_type = ");
    qb.push_bind(t.id).push(" AND i.deleted_at IS NULL");
    scope.push(&mut qb);
    qb.push("), g AS (");
    let mut first = true;
    for k in ["phone", "email"] {
        if kinds.iter().any(|x| x == k) {
            if !first {
                qb.push(" UNION ALL ");
            }
            first = false;
            qb.push("SELECT m.kind, m.normalized AS value, MIN(m.value) AS shown, array_agg(DISTINCT v.id) AS ids
                     FROM crm_multifields m JOIN v ON v.id = m.item_id WHERE m.kind = ")
                .push_bind(k)
                .push(" AND length(m.normalized) >= 3 GROUP BY m.kind, m.normalized HAVING COUNT(DISTINCT v.id) > 1");
        }
    }
    if kinds.iter().any(|x| x == "name") {
        if !first {
            qb.push(" UNION ALL ");
        }
        qb.push("SELECT 'name' AS kind, lower(btrim(v.title)) AS value, MIN(v.title) AS shown, array_agg(v.id ORDER BY v.created_at) AS ids
                 FROM v WHERE length(btrim(v.title)) >= 2 GROUP BY lower(btrim(v.title)) HAVING COUNT(*) > 1");
    }
    qb.push(") SELECT g.kind, g.value, g.shown, g.ids, COUNT(*) OVER () AS total FROM g
              WHERE NOT EXISTS (SELECT 1 FROM crm_duplicate_ignores x WHERE x.entity_type = ")
        .push_bind(t.id)
        .push(" AND x.kind = g.kind AND x.value = g.value AND g.ids <@ x.item_ids)");
    if let Some(term) = clean(q.q.clone()) {
        let like = format!("%{}%", term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        qb.push(" AND (g.shown ILIKE ").push_bind(like.clone()).push(" OR g.value ILIKE ").push_bind(like).push(")");
    }
    qb.push(" ORDER BY cardinality(g.ids) DESC, g.kind, g.value LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);
    let groups: Vec<GroupRow> = qb.build_query_as().fetch_all(&state.db).await?;
    let total = groups.first().map(|g| g.total).unwrap_or(0);
    let all: Vec<Uuid> = groups.iter().flat_map(|g| g.ids.iter().copied()).collect();
    let items: Vec<GroupItem> = sqlx::query_as(
        "SELECT i.id, i.number, i.title, i.assigned_to,
                CASE WHEN u.id IS NULL THEN NULL ELSE u.first_name || ' ' || u.last_name END AS assigned_name, i.created_at,
                (SELECT string_agg(value, ', ') FROM crm_multifields m WHERE m.item_id = i.id AND m.kind = 'phone') AS phones,
                (SELECT string_agg(value, ', ') FROM crm_multifields m WHERE m.item_id = i.id AND m.kind = 'email') AS emails,
                (SELECT COUNT(*) FROM crm_items d WHERE d.deleted_at IS NULL AND (d.company_id = i.id OR EXISTS (SELECT 1 FROM crm_item_contacts c WHERE c.item_id = d.id AND c.contact_id = i.id)))::int8 AS deals
         FROM crm_items i LEFT JOIN users u ON u.id = i.assigned_to WHERE i.id = ANY($1)",
    )
    .bind(&all)
    .fetch_all(&state.db)
    .await?;
    let by_id: HashMap<Uuid, &GroupItem> = items.iter().map(|i| (i.id, i)).collect();
    let out: Vec<Value> = groups
        .iter()
        .map(|g| {
            let mut members: Vec<&&GroupItem> = g.ids.iter().filter_map(|id| by_id.get(id)).collect();
            // The oldest first: it's the one others merge into.
            members.sort_by_key(|m| m.created_at);
            json!({ "kind": g.kind, "value": g.value, "shown": g.shown, "count": g.ids.len(), "items": members.into_iter().take(50).collect::<Vec<_>>() })
        })
        .collect();
    Ok(Json(json!({ "groups": out, "total": total, "limit": limit, "offset": offset })))
}

#[derive(Deserialize)]
struct IgnoreGroup {
    kind: String,
    value: String,
    item_ids: Vec<Uuid>,
}

/// "Not duplicates": hides this group until another record joins it.
async fn ignore_group(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(b): Json<IgnoreGroup>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    dup_kinds(&Some(b.kind.clone()))?;
    if b.item_ids.len() < 2 || b.item_ids.len() > 1000 || b.value.len() > 1000 {
        return Err(AppError::bad("a group has between 2 and 1000 records"));
    }
    // Only records the person may change, of this type.
    let rows: Vec<(Uuid, i16, Option<Uuid>)> = sqlx::query_as("SELECT id, entity_type, assigned_to FROM crm_items WHERE id = ANY($1) AND deleted_at IS NULL").bind(&b.item_ids).fetch_all(&state.db).await?;
    let scope = visible(&state, &auth, t.id, "write").await?;
    if rows.len() != b.item_ids.len() || rows.iter().any(|r| r.1 != t.id || !scope.allows(r.2)) {
        return Err(AppError::Forbidden);
    }
    sqlx::query(
        "INSERT INTO crm_duplicate_ignores (entity_type, kind, value, item_ids, created_by) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (entity_type, kind, value) DO UPDATE SET item_ids = (SELECT array_agg(DISTINCT x) FROM unnest(crm_duplicate_ignores.item_ids || EXCLUDED.item_ids) x),
             created_by = EXCLUDED.created_by, created_at = now()",
    )
    .bind(t.id)
    .bind(&b.kind)
    .bind(&b.value)
    .bind(&b.item_ids)
    .bind(auth.id)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct MergeGroups {
    /// Each group's records; each merges into its oldest record (or `target_id` when given).
    groups: Vec<MergeGroup>,
}

#[derive(Deserialize)]
struct MergeGroup {
    item_ids: Vec<Uuid>,
    target_id: Option<Uuid>,
}

/// Merges several groups at once. Each group is merged on its own; one that can't be (no
/// right to delete a record, a record gone meanwhile) is reported and the rest go on.
async fn merge_groups(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(b): Json<MergeGroups>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    if b.groups.is_empty() || b.groups.len() > 100 {
        return Err(AppError::bad("choose between 1 and 100 groups"));
    }
    let mut merged = Vec::new();
    let mut failed = Vec::new();
    for g in &b.groups {
        let mut ids = g.item_ids.clone();
        ids.sort();
        ids.dedup();
        let present: Vec<(Uuid, DateTime<Utc>)> =
            sqlx::query_as("SELECT id, created_at FROM crm_items WHERE id = ANY($1) AND entity_type = $2 AND deleted_at IS NULL ORDER BY created_at, number")
                .bind(&ids)
                .bind(t.id)
                .fetch_all(&state.db)
                .await?;
        if present.len() < 2 {
            failed.push(json!({ "item_ids": g.item_ids, "error": "fewer than two of these records are left" }));
            continue;
        }
        let target = match g.target_id {
            Some(x) if present.iter().any(|p| p.0 == x) => x,
            Some(_) => {
                failed.push(json!({ "item_ids": g.item_ids, "error": "the record to keep isn't in the group" }));
                continue;
            }
            None => present[0].0,
        };
        let sources: Vec<Uuid> = present.iter().map(|p| p.0).filter(|x| *x != target).collect();
        match merge_records(&state, &auth, &t, &Merge { target_id: target, source_ids: sources }).await {
            Ok(()) => merged.push(target),
            Err(e) => failed.push(json!({ "item_ids": g.item_ids, "error": e.to_string() })),
        }
    }
    Ok(Json(json!({ "merged": merged, "failed": failed })))
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[derive(Deserialize, Serialize, Clone)]
pub struct ImportRequest {
    file_id: Uuid,
    /// Column index (as a string) → field key: a system field, "phone"/"email"/"web"/"im",
    /// "address.city" etc., a custom field code, "assigned_email", "stage_name" or "company".
    mapping: Map<String, Value>,
    #[serde(default = "yes")]
    has_header: bool,
    /// skip | update | create — what to do when a row matches an existing record by phone/email.
    #[serde(default = "skip")]
    duplicates: String,
    #[serde(default)]
    delimiter: Option<String>,
    /// csv (default) or vcard (a .vcf file: no mapping needed).
    #[serde(default)]
    format: Option<String>,
}
fn yes() -> bool {
    true
}
fn skip() -> String {
    "skip".into()
}

fn status_key(job: Uuid) -> String {
    format!("crm:import:{job}")
}

async fn start_import(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(b): Json<ImportRequest>) -> AppResult<Json<Value>> {
    let t = entity_type(&state, &code).await?;
    require_any(&state, &auth, t.id, "import").await?;
    require_any(&state, &auth, t.id, "add").await?;
    if !["skip", "update", "create"].contains(&b.duplicates.as_str()) {
        return Err(AppError::bad("duplicates must be skip, update or create"));
    }
    let owner: Option<Option<Uuid>> = sqlx::query_scalar("SELECT owner_id FROM files WHERE id = $1").bind(b.file_id).fetch_optional(&state.db).await?;
    if owner != Some(Some(auth.id)) {
        return Err(AppError::bad("upload the file first"));
    }
    let vcard = match b.format.as_deref().unwrap_or("csv") {
        "csv" => false,
        "vcard" => true,
        _ => return Err(AppError::bad("format must be csv or vcard")),
    };
    if vcard && t.id != super::CONTACT {
        return Err(AppError::bad("vCards hold people: import them as contacts"));
    }
    if !vcard && b.mapping.is_empty() {
        return Err(AppError::bad("map at least one column to a field"));
    }
    let job = Uuid::new_v4();
    let payload = json!({ "job_id": job, "user_id": auth.id, "type": t.code, "request": b });
    jobs::enqueue(&state.db, IMPORT_JOB, payload, None).await?;
    state.job_wakeup.notify_one();
    let mut redis = state.redis.clone();
    let _: () = redis.set_ex(status_key(job), json!({ "state": "queued" }).to_string(), 7 * 86400).await?;
    Ok(Json(json!({ "job_id": job })))
}

async fn import_status(State(state): State<AppState>, _auth: AuthUser, Path(job): Path<Uuid>) -> AppResult<Json<Value>> {
    let mut redis = state.redis.clone();
    let raw: Option<String> = redis.get(status_key(job)).await?;
    Ok(Json(raw.and_then(|r| serde_json::from_str(&r).ok()).ok_or(AppError::NotFound("import"))?))
}

/// A date as exports write it: 2026-01-31, 20260131, 1/31/2026 (US), 31.01.2026; others
/// (and Outlook's 0/0/00 for "none") are dropped.
fn import_date(raw: &str) -> Option<String> {
    use chrono::NaiveDate;
    let raw = raw.trim();
    for f in ["%Y-%m-%d", "%Y%m%d", "%m/%d/%Y", "%d.%m.%Y", "%Y/%m/%d"] {
        if let Ok(d) = NaiveDate::parse_from_str(raw, f) {
            return Some(d.format("%Y-%m-%d").to_string());
        }
    }
    None
}

/// Builds a record body from one CSV row according to the mapping.
async fn row_body(state: &AppState, t: &EntityType, map: &Map<String, Value>, rec: &csv::StringRecord, uf_codes: &HashSet<String>) -> Result<(Map<String, Value>, Vec<String>)> {
    let mut body = Map::new();
    let mut multi: Vec<Value> = Vec::new();
    let mut address = Map::new();
    let mut uf = Map::new();
    let mut match_keys = Vec::new();
    for (col, field) in map {
        let Ok(i) = col.parse::<usize>() else { continue };
        let Some(raw) = rec.get(i).map(str::trim).filter(|v| !v.is_empty()) else { continue };
        let Some(field) = field.as_str() else { continue };
        match field {
            "phone" | "email" | "web" | "im" => {
                // Google's exports put several values in one cell, separated by " ::: ".
                let raw = raw.replace(":::", ";");
                for part in raw.split([';', ',']).map(str::trim).filter(|p| !p.is_empty()) {
                    multi.push(json!({ "kind": field, "value_type": if field == "im" { "other" } else { "work" }, "value": part }));
                    if field == "phone" || field == "email" {
                        match_keys.push(format!("{field}:{}", normalize(field, part)));
                    }
                }
            }
            f if f.starts_with("address.") => {
                let k = &f[8..];
                // Only ISO codes are kept: exports often hold country names.
                if k == "country" {
                    if raw.len() == 2 && raw.chars().all(|c| c.is_ascii_alphabetic()) {
                        address.insert(k.to_string(), json!(raw.to_uppercase()));
                    }
                } else {
                    address.insert(k.to_string(), json!(raw));
                }
            }
            "birthdate" => {
                if let Some(d) = import_date(raw) {
                    body.insert("birthdate".into(), json!(d));
                }
            }
            f if uf_codes.contains(f) => {
                uf.insert(f.to_string(), json!(raw));
            }
            "assigned_email" => {
                let u: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE email = $1 AND is_active").bind(raw.to_lowercase()).fetch_optional(&state.db).await?;
                if let Some(u) = u {
                    body.insert("assigned_to".into(), json!(u));
                }
            }
            "stage_name" if t.has_stages => {
                let s: Option<Uuid> = sqlx::query_scalar(
                    "SELECT s.id FROM crm_stages s JOIN crm_categories c ON c.id = s.category_id
                     WHERE c.entity_type = $1 AND lower(s.name) = lower($2) ORDER BY c.is_default DESC, s.sort LIMIT 1",
                )
                .bind(t.id)
                .bind(raw)
                .fetch_optional(&state.db)
                .await?;
                if let Some(s) = s {
                    body.insert("stage_id".into(), json!(s));
                }
            }
            "company" if t.id != COMPANY => {
                // Link to an existing company by exact name, or create one.
                let existing: Option<Uuid> = sqlx::query_scalar(
                    "SELECT id FROM crm_items WHERE entity_type = 4 AND deleted_at IS NULL AND lower(title) = lower($1) LIMIT 1",
                )
                .bind(raw)
                .fetch_optional(&state.db)
                .await?;
                body.insert("company_id".into(), json!(existing.map(|e| e.to_string()).unwrap_or_else(|| format!("new:{raw}"))));
            }
            "opportunity" | "annual_revenue" => {
                let cleaned: String = raw.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
                body.insert(field.into(), json!(cleaned));
            }
            "probability" => {
                if let Ok(p) = raw.trim_end_matches('%').trim().parse::<i64>() {
                    body.insert(field.into(), json!(p));
                }
            }
            other => {
                body.insert(other.into(), json!(raw));
            }
        }
    }
    if !multi.is_empty() {
        body.insert("multifields".into(), Value::Array(multi));
    }
    if !address.is_empty() {
        body.insert("address".into(), Value::Object(address));
    }
    if !uf.is_empty() {
        body.insert("uf".into(), Value::Object(uf));
    }
    Ok((body, match_keys))
}

/// One record of an import: create it, or (by its phones and emails) update or skip the
/// record it matches.
async fn import_one(state: &AppState, auth: &AuthUser, t: &EntityType, company_type: &EntityType, duplicates: &str, mut body: Map<String, Value>, keys: Vec<String>) -> Result<&'static str> {
    if body.is_empty() {
        return Ok("skipped");
    }
    if let Some(Value::String(c)) = body.get("company_id").cloned()
        && let Some(name) = c.strip_prefix("new:") {
            let mut m = Map::new();
            m.insert("company_name".into(), json!(name));
            let id = create_item(state, auth, company_type, m, None).await.map_err(|e| anyhow!(e.to_string()))?;
            body.insert("company_id".into(), json!(id));
        }
    // Existing record with the same phone/email?
    let existing: Option<Uuid> = if keys.is_empty() || duplicates == "create" {
        None
    } else {
        let (phones, emails): (Vec<String>, Vec<String>) = keys.iter().cloned().partition(|k| k.starts_with("phone:"));
        let strip = |v: Vec<String>| v.into_iter().map(|k| k.split_once(':').unwrap().1.to_string()).collect::<Vec<_>>();
        sqlx::query_scalar(
            "SELECT i.id FROM crm_multifields m JOIN crm_items i ON i.id = m.item_id
             WHERE i.entity_type = $1 AND i.deleted_at IS NULL
               AND ((m.kind = 'phone' AND m.normalized = ANY($2)) OR (m.kind = 'email' AND m.normalized = ANY($3)))
             LIMIT 1",
        )
        .bind(t.id)
        .bind(strip(phones))
        .bind(strip(emails))
        .fetch_optional(&state.db)
        .await?
    };
    match (existing, duplicates) {
        (Some(_), "skip") => Ok("skipped"),
        (Some(id), _) => {
            body.remove("multifields"); // keep the existing record's contact details
            update_item(state, auth, t, id, body).await.map_err(|e| anyhow!(e.to_string()))?;
            Ok("updated")
        }
        (None, _) => {
            create_item(state, auth, t, body, None).await.map_err(|e| anyhow!(e.to_string()))?;
            Ok("created")
        }
    }
}

/// A vCard's body for import: the company from ORG linked (or made), and its phones and
/// emails as match keys.
async fn card_body(state: &AppState, c: super::vcard::Card) -> Result<(Map<String, Value>, Vec<String>)> {
    let mut body = c.body;
    let keys: Vec<String> = body
        .get("multifields")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    let kind = m["kind"].as_str()?;
                    (kind == "phone" || kind == "email").then(|| format!("{kind}:{}", normalize(kind, m["value"].as_str().unwrap_or(""))))
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(org) = c.company.filter(|o| !o.is_empty()) {
        let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM crm_items WHERE entity_type = 4 AND deleted_at IS NULL AND lower(title) = lower($1) LIMIT 1")
            .bind(&org)
            .fetch_optional(&state.db)
            .await?;
        body.insert("company_id".into(), json!(existing.map(|e| e.to_string()).unwrap_or_else(|| format!("new:{org}"))));
    }
    Ok((body, keys))
}

pub async fn run_import(state: &AppState, payload: &Value) -> Result<()> {
    let job: Uuid = serde_json::from_value(payload["job_id"].clone()).unwrap_or_else(|_| Uuid::nil());
    let user: Uuid = serde_json::from_value(payload["user_id"].clone())?;
    let code = payload["type"].as_str().ok_or_else(|| anyhow!("missing type"))?;
    let req: ImportRequest = serde_json::from_value(payload["request"].clone())?;
    let auth = acting_as(state, user).await?.ok_or_else(|| anyhow!("importing user is no longer active"))?;
    let t = entity_type(state, code).await?;
    let path: String = sqlx::query_scalar("SELECT storage_path FROM files WHERE id = $1").bind(req.file_id).fetch_one(&state.db).await?;
    let full = state.config.upload_dir.join(path);
    // Exports from mail programs come as UTF-8, UTF-16 or Windows-1252.
    let text = super::vcard::decode(&tokio::fs::read(&full).await?);
    let delimiter = req.delimiter.as_deref().and_then(|d| d.bytes().next()).unwrap_or(b',');
    let uf_codes: HashSet<String> = super::fields::visible_fields(state, &auth, t.id).await?.into_iter().map(|f| f.code).collect();
    let company_type = entity_type_by_id(state, COMPANY).await?;
    let vcard = req.format.as_deref() == Some("vcard");

    let (mut processed, mut created, mut updated, mut skipped) = (0u64, 0u64, 0u64, 0u64);
    let mut errors: Vec<Value> = Vec::new();
    let mut redis = state.redis.clone();
    let key = status_key(job);
    // Each record: its line in the file, and its body and match keys (or why it can't be read).
    let mut records: Vec<(usize, Result<(Map<String, Value>, Vec<String>)>)> = Vec::new();
    let mut rows = None;
    if vcard {
        for c in super::vcard::parse(&text) {
            let line = c.line;
            records.push((line, card_body(state, c).await));
        }
    } else {
        rows = Some(csv::ReaderBuilder::new().has_headers(req.has_header).delimiter(delimiter).flexible(true).from_reader(text.as_bytes()).into_records().enumerate());
    }
    let mut cards = records.into_iter();
    loop {
        let (line, rec) = match rows.as_mut() {
            Some(r) => match r.next() {
                Some((n, rec)) => {
                    let line = n + if req.has_header { 2 } else { 1 };
                    let built = match rec {
                        Ok(rec) => row_body(state, &t, &req.mapping, &rec, &uf_codes).await,
                        Err(e) => Err(e.into()),
                    };
                    (line, built)
                }
                None => break,
            },
            None => match cards.next() {
                Some(x) => x,
                None => break,
            },
        };
        let outcome = match rec {
            Ok((body, keys)) => import_one(state, &auth, &t, &company_type, &req.duplicates, body, keys).await,
            Err(e) => Err(e),
        };
        processed += 1;
        match outcome {
            Ok("created") => created += 1,
            Ok("updated") => updated += 1,
            Ok(_) => skipped += 1,
            Err(e) => {
                if errors.len() < 100 {
                    errors.push(json!({ "line": line, "error": e.to_string() }));
                }
            }
        }
        if processed % 200 == 0 {
            let _: () = redis
                .set_ex(&key, json!({ "state": "running", "processed": processed, "created": created, "updated": updated, "skipped": skipped, "errors": errors }).to_string(), 7 * 86400)
                .await?;
        }
    }
    let failed = errors.len();
    let _: () = redis
        .set_ex(&key, json!({ "state": "done", "processed": processed, "created": created, "updated": updated, "skipped": skipped, "errors": errors }).to_string(), 7 * 86400)
        .await?;
    notify(
        state,
        &[user],
        NewNotification {
            kind: "crm.import",
            title: format!("Import of {} finished", t.name_plural.to_lowercase()),
            body: Some(format!("{created} created, {updated} updated, {skipped} skipped, {failed} failed")),
            link: Some(format!("/crm/{}", t.code)),
            actor: None,
        },
    )
    .await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

fn csv_line(fields: &[String]) -> String {
    let mut out = fields
        .iter()
        .map(|f| {
            // Guard against spreadsheet formula injection.
            let f = if f.starts_with(['=', '+', '-', '@']) { format!("'{f}") } else { f.clone() };
            if f.contains([',', '"', '\n', '\r']) { format!("\"{}\"", f.replace('"', "\"\"")) } else { f }
        })
        .collect::<Vec<_>>()
        .join(",");
    out.push_str("\r\n");
    out
}

/// Streams matching records as CSV, in keyset-paged batches so memory stays flat.
async fn export(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Query(f): Query<Filter>) -> AppResult<Response> {
    let t = entity_type(&state, &code).await?;
    let scope = visible(&state, &auth, t.id, "export").await?;
    if matches!(scope, Visible::Nothing) {
        return Err(AppError::Forbidden);
    }
    let read = visible(&state, &auth, t.id, "read").await?;
    super::items::guard_filter(&state, &auth, &t, &f).await?;
    let defs = super::fields::visible_fields(&state, &auth, t.id).await?;
    let mut header = vec!["Number", "Title", "First name", "Last name", "Position", "Company", "Phones", "Emails", "Stage", "Amount", "Currency", "Source", "Responsible", "Created", "Updated"]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
    header.extend(defs.iter().map(|d| d.label.clone()));
    // Validate the filter once before streaming.
    push_filter(&mut QueryBuilder::new(""), &t, &f, auth.id)?;

    struct Cursor {
        last: Option<Uuid>,
        done: bool,
        header: Option<String>,
    }
    let st = state.clone();
    let me = auth.id;
    let body = stream::unfold(Cursor { last: None, done: false, header: Some(csv_line(&header)) }, move |mut c| {
        let (state, t, f, scope, read, defs) = (st.clone(), t.clone(), f.clone(), clone_vis(&scope), clone_vis(&read), defs.clone());
        async move {
            if let Some(h) = c.header.take() {
                return Some((Ok::<_, std::io::Error>(h), c));
            }
            if c.done {
                return None;
            }
            let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
                "SELECT i.id, i.number, i.title, i.first_name, i.last_name, i.job_title, co.title AS company, s.name AS stage,
                        i.opportunity::text AS amount, i.currency, i.source, u.first_name || ' ' || u.last_name AS responsible,
                        i.created_at, i.updated_at, i.uf,
                        (SELECT string_agg(value, '; ') FROM crm_multifields m WHERE m.item_id = i.id AND m.kind = 'phone') AS phones,
                        (SELECT string_agg(value, '; ') FROM crm_multifields m WHERE m.item_id = i.id AND m.kind = 'email') AS emails
                 FROM crm_items i LEFT JOIN crm_items co ON co.id = i.company_id LEFT JOIN crm_stages s ON s.id = i.stage_id
                 LEFT JOIN users u ON u.id = i.assigned_to WHERE TRUE",
            );
            let _ = push_filter(&mut qb, &t, &f, me);
            scope.push(&mut qb);
            read.push(&mut qb);
            if let Some(l) = c.last {
                qb.push(" AND i.id > ").push_bind(l);
            }
            qb.push(" ORDER BY i.id LIMIT 2000");
            let rows: Vec<ExportRow> = match qb.build_query_as().fetch_all(&state.db).await {
                Ok(r) => r,
                Err(e) => return Some((Err(std::io::Error::other(e.to_string())), Cursor { done: true, ..c })),
            };
            if rows.len() < 2000 {
                c.done = true;
            }
            c.last = rows.last().map(|r| r.id);
            let mut out = String::new();
            for r in rows {
                let mut fields = vec![
                    r.number.to_string(), r.title, r.first_name.unwrap_or_default(), r.last_name.unwrap_or_default(),
                    r.job_title.unwrap_or_default(), r.company.unwrap_or_default(), r.phones.unwrap_or_default(),
                    r.emails.unwrap_or_default(), r.stage.unwrap_or_default(), r.amount, r.currency.unwrap_or_default(),
                    r.source.unwrap_or_default(), r.responsible.unwrap_or_default(), r.created_at.to_rfc3339(), r.updated_at.to_rfc3339(),
                ];
                for d in &defs {
                    fields.push(match &r.uf[&d.code] {
                        Value::Null => String::new(),
                        Value::String(s) => s.clone(),
                        v => v.to_string(),
                    });
                }
                out.push_str(&csv_line(&fields));
            }
            if out.is_empty() && c.done {
                return None;
            }
            Some((Ok(out), c))
        }
    });
    // Fused: response layers (compression) may poll once more after the end.
    let mut res = Body::from_stream(body.fuse()).into_response();
    let name = format!("{}-{}.csv", code, Utc::now().format("%Y%m%d-%H%M"));
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/csv; charset=utf-8"));
    res.headers_mut().insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")).unwrap());
    Ok(res)
}

/// The same records as the CSV export (at most 100,000), in one list.
async fn export_rows(state: &AppState, auth: &AuthUser, code: &str, f: &Filter) -> AppResult<(super::EntityType, Vec<super::fields::FieldDef>, Vec<ExportRow>)> {
    let t = entity_type(state, code).await?;
    let scope = visible(state, auth, t.id, "export").await?;
    if matches!(scope, Visible::Nothing) {
        return Err(AppError::Forbidden);
    }
    let read = visible(state, auth, t.id, "read").await?;
    super::items::guard_filter(state, auth, &t, f).await?;
    let defs = super::fields::visible_fields(state, auth, t.id).await?;
    let mut out: Vec<ExportRow> = Vec::new();
    let mut last: Option<Uuid> = None;
    loop {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
            "SELECT i.id, i.number, i.title, i.first_name, i.last_name, i.job_title, co.title AS company, s.name AS stage,
                    i.opportunity::text AS amount, i.currency, i.source, u.first_name || ' ' || u.last_name AS responsible,
                    i.created_at, i.updated_at, i.uf,
                    (SELECT string_agg(value, '; ') FROM crm_multifields m WHERE m.item_id = i.id AND m.kind = 'phone') AS phones,
                    (SELECT string_agg(value, '; ') FROM crm_multifields m WHERE m.item_id = i.id AND m.kind = 'email') AS emails
             FROM crm_items i LEFT JOIN crm_items co ON co.id = i.company_id LEFT JOIN crm_stages s ON s.id = i.stage_id
             LEFT JOIN users u ON u.id = i.assigned_to WHERE TRUE",
        );
        push_filter(&mut qb, &t, f, auth.id)?;
        scope.push(&mut qb);
        read.push(&mut qb);
        if let Some(l) = last {
            qb.push(" AND i.id > ").push_bind(l);
        }
        qb.push(" ORDER BY i.id LIMIT 2000");
        let rows: Vec<ExportRow> = qb.build_query_as().fetch_all(&state.db).await?;
        let n = rows.len();
        last = rows.last().map(|r| r.id);
        out.extend(rows);
        if n < 2000 || out.len() >= 100_000 {
            break;
        }
    }
    out.sort_by_key(|r| r.number);
    Ok((t, defs, out))
}

fn attachment(name: String, kind: &'static str, body: Vec<u8>) -> Response {
    let mut res = body.into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    res.headers_mut().insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")).unwrap());
    res
}

/// The export as an Excel workbook: a header row, numbers and dates as such.
async fn export_xlsx(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Query(f): Query<Filter>) -> AppResult<Response> {
    use rust_xlsxwriter::{Format, Workbook};
    let (t, defs, rows) = export_rows(&state, &auth, &code, &f).await?;
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let _ = ws.set_name(&t.name_plural.chars().take(31).collect::<String>());
    let bold = Format::new().set_bold();
    let date = Format::new().set_num_format("yyyy-mm-dd hh:mm");
    let mut header: Vec<String> = ["Number", "Title", "First name", "Last name", "Position", "Company", "Phones", "Emails", "Stage", "Amount", "Currency", "Source", "Responsible", "Created", "Updated"].iter().map(|s| s.to_string()).collect();
    header.extend(defs.iter().map(|d| d.label.clone()));
    let x = |e: rust_xlsxwriter::XlsxError| AppError::Internal(anyhow!(e.to_string()));
    for (c, h) in header.iter().enumerate() {
        ws.write_string_with_format(0, c as u16, h, &bold).map_err(x)?;
    }
    for (i, r) in rows.iter().enumerate() {
        let row = (i + 1) as u32;
        ws.write_number(row, 0, r.number as f64).map_err(x)?;
        for (c, v) in [&r.title, r.first_name.as_deref().unwrap_or(""), r.last_name.as_deref().unwrap_or(""), r.job_title.as_deref().unwrap_or(""), r.company.as_deref().unwrap_or(""), r.phones.as_deref().unwrap_or(""), r.emails.as_deref().unwrap_or(""), r.stage.as_deref().unwrap_or("")].iter().enumerate() {
            // Strings are written as text, never as formulas.
            ws.write_string(row, (c + 1) as u16, *v).map_err(x)?;
        }
        if let Ok(a) = r.amount.parse::<f64>() {
            ws.write_number(row, 9, a).map_err(x)?;
        }
        ws.write_string(row, 10, r.currency.as_deref().unwrap_or("")).map_err(x)?;
        ws.write_string(row, 11, r.source.as_deref().unwrap_or("")).map_err(x)?;
        ws.write_string(row, 12, r.responsible.as_deref().unwrap_or("")).map_err(x)?;
        for (c, at) in [(13u16, r.created_at), (14, r.updated_at)] {
            let dt = rust_xlsxwriter::ExcelDateTime::parse_from_str(&at.format("%Y-%m-%dT%H:%M:%S").to_string()).map_err(x)?;
            ws.write_datetime_with_format(row, c, &dt, &date).map_err(x)?;
        }
        for (k, d) in defs.iter().enumerate() {
            let col = 15 + k as u16;
            match &r.uf[&d.code] {
                Value::Null => {}
                Value::Number(n) => { ws.write_number(row, col, n.as_f64().unwrap_or(0.0)).map_err(x)?; }
                Value::String(s) => { ws.write_string(row, col, s).map_err(x)?; }
                v => { ws.write_string(row, col, v.to_string()).map_err(x)?; }
            }
        }
    }
    let _ = ws.autofit();
    let bytes = wb.save_to_buffer().map_err(x)?;
    Ok(attachment(format!("{}-{}.xlsx", code, Utc::now().format("%Y%m%d-%H%M")), "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", bytes))
}

/// Contacts, leads and companies as vCards (3.0), which Outlook, Gmail and phones import.
async fn export_vcf(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Query(f): Query<Filter>) -> AppResult<Response> {
    let (t, _, rows) = export_rows(&state, &auth, &code, &f).await?;
    if ![super::CONTACT, super::LEAD, super::COMPANY].contains(&t.id) {
        return Err(AppError::bad("vCards are for contacts, leads and companies"));
    }
    let esc = |s: &str| s.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace(['\r', '\n'], " ");
    let mut out = String::new();
    for r in &rows {
        let (first, last) = (r.first_name.clone().unwrap_or_default(), r.last_name.clone().unwrap_or_default());
        let full = if t.id == super::COMPANY || (first.is_empty() && last.is_empty()) { r.title.clone() } else { format!("{first} {last}").trim().to_string() };
        out.push_str("BEGIN:VCARD\r\nVERSION:3.0\r\n");
        out.push_str(&format!("N:{};{};;;\r\nFN:{}\r\n", esc(&last), esc(&first), esc(&full)));
        let org = if t.id == super::COMPANY { Some(r.title.clone()) } else { r.company.clone() };
        if let Some(o) = org.filter(|o| !o.is_empty()) {
            out.push_str(&format!("ORG:{}\r\n", esc(&o)));
        }
        if let Some(j) = r.job_title.as_deref().filter(|j| !j.is_empty()) {
            out.push_str(&format!("TITLE:{}\r\n", esc(j)));
        }
        for e in r.emails.as_deref().unwrap_or("").split("; ").filter(|e| !e.is_empty()) {
            out.push_str(&format!("EMAIL;TYPE=INTERNET:{}\r\n", esc(e)));
        }
        for p in r.phones.as_deref().unwrap_or("").split("; ").filter(|p| !p.is_empty()) {
            out.push_str(&format!("TEL;TYPE=VOICE:{}\r\n", esc(p)));
        }
        out.push_str(&format!("UID:nerp-{}\r\nEND:VCARD\r\n", r.id));
    }
    Ok(attachment(format!("{}-{}.vcf", code, Utc::now().format("%Y%m%d-%H%M")), "text/vcard; charset=utf-8", out.into_bytes()))
}

#[derive(FromRow)]
struct ExportRow {
    id: Uuid,
    number: i64,
    title: String,
    first_name: Option<String>,
    last_name: Option<String>,
    job_title: Option<String>,
    company: Option<String>,
    stage: Option<String>,
    amount: String,
    currency: Option<String>,
    source: Option<String>,
    responsible: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    uf: Value,
    phones: Option<String>,
    emails: Option<String>,
}

fn clone_vis(v: &Visible) -> Visible {
    match v {
        Visible::Nothing => Visible::Nothing,
        Visible::Owners(u, o) => Visible::Owners(u.clone(), *o),
        Visible::Everything => Visible::Everything,
    }
}

// ---------------------------------------------------------------------------
// Recycle bin
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct Deleted {
    id: Uuid,
    entity_type: i16,
    number: i64,
    title: String,
    assigned_to: Option<Uuid>,
    deleted_at: DateTime<Utc>,
    deleted_by_name: Option<String>,
}

#[derive(Deserialize)]
struct RecycleQuery {
    #[serde(rename = "type")]
    code: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// What the caller deleted, or everything for CRM administrators.
async fn recycle_list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<RecycleQuery>) -> AppResult<Json<Value>> {
    let limit: i64 = q.limit.as_deref().and_then(|v| v.parse().ok()).unwrap_or(50).clamp(1, 200);
    let offset: i64 = q.offset.as_deref().and_then(|v| v.parse().ok()).unwrap_or(0).max(0);
    let t = match &q.code {
        Some(c) => Some(entity_type(&state, c).await?.id),
        None => None,
    };
    let admin = is_crm_admin(&auth);
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM crm_items WHERE deleted_at IS NOT NULL AND ($1::smallint IS NULL OR entity_type = $1) AND ($2 OR deleted_by = $3)",
    )
    .bind(t)
    .bind(admin)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    let items: Vec<Deleted> = sqlx::query_as(
        "SELECT i.id, i.entity_type, i.number, i.title, i.assigned_to, i.deleted_at,
                CASE WHEN u.id IS NULL THEN NULL ELSE u.first_name || ' ' || u.last_name END AS deleted_by_name
         FROM crm_items i LEFT JOIN users u ON u.id = i.deleted_by
         WHERE i.deleted_at IS NOT NULL AND ($1::smallint IS NULL OR i.entity_type = $1) AND ($2 OR i.deleted_by = $3)
         ORDER BY i.deleted_at DESC LIMIT $4 OFFSET $5",
    )
    .bind(t)
    .bind(admin)
    .bind(auth.id)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "page": Page { items, total, limit, offset }, "retention_days": RECYCLE_DAYS })))
}

async fn deleted_row(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<(i16, Option<Uuid>)> {
    let (t, owner, deleted_by, deleted): (i16, Option<Uuid>, Option<Uuid>, Option<DateTime<Utc>>) =
        sqlx::query_as("SELECT entity_type, assigned_to, deleted_by, deleted_at FROM crm_items WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(AppError::NotFound("record"))?;
    if deleted.is_none() {
        return Err(AppError::bad("this record is not in the recycle bin"));
    }
    if !is_crm_admin(auth) && deleted_by != Some(auth.id) {
        return Err(AppError::NotFound("record"));
    }
    Ok((t, owner))
}

async fn restore(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (t, _) = deleted_row(&state, &auth, id).await?;
    sqlx::query("UPDATE crm_items SET deleted_at = NULL, deleted_by = NULL, updated_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
    timeline(&state.db, id, "restored", Some(auth.id), None, json!({})).await?;
    broadcast(&state, t, id, false).await;
    let et = entity_type_by_id(&state, t).await?;
    Ok(Json(json!({ "ok": true, "type": et.code })))
}

async fn purge_one(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    deleted_row(&state, &auth, id).await?;
    if !crate::accounting::in_books(&state, &[id]).await?.is_empty() {
        return Err(AppError::bad("the accounts refer to this record, so it stays in the recycle bin (restore it if needed)"));
    }
    purge(&state, &[id]).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Deletes records for good, with the files of their generated documents and signatures (the
/// rows go with the record; the files would otherwise stay on disk).
async fn purge(state: &AppState, ids: &[Uuid]) -> AppResult<u64> {
    // Records the books refer to stay in the recycle bin.
    let keep = crate::accounting::in_books(state, ids).await?;
    let ids: Vec<Uuid> = ids.iter().copied().filter(|i| !keep.contains(i)).collect();
    let ids = &ids[..];
    let files: Vec<Uuid> = sqlx::query_scalar(
        "SELECT f FROM crm_documents d, LATERAL unnest(ARRAY[d.pdf_file_id, d.source_file_id]) f WHERE d.item_id = ANY($1) AND f IS NOT NULL
         UNION ALL SELECT f FROM sign_requests r, LATERAL unnest(ARRAY[r.original_file_id, r.signed_file_id]) f WHERE r.item_id = ANY($1) AND f IS NOT NULL
         UNION ALL SELECT s.signature_file_id FROM sign_signers s JOIN sign_requests r ON r.id = s.request_id WHERE r.item_id = ANY($1) AND s.signature_file_id IS NOT NULL",
    )
    .bind(ids)
    .fetch_all(&state.db)
    .await?;
    // Emails that only these records show go too, with their attachments.
    let emails: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT l.email_id FROM crm_email_links l WHERE l.item_id = ANY($1)
           AND NOT EXISTS (SELECT 1 FROM crm_email_links o WHERE o.email_id = l.email_id AND NOT o.item_id = ANY($1))",
    )
    .bind(ids)
    .fetch_all(&state.db)
    .await?;
    let mut files = files;
    files.extend(sqlx::query_scalar::<_, Uuid>("SELECT file_id FROM crm_email_files WHERE email_id = ANY($1)").bind(&emails).fetch_all(&state.db).await?);
    let n = sqlx::query("DELETE FROM crm_items WHERE id = ANY($1)").bind(ids).execute(&state.db).await?.rows_affected();
    sqlx::query("DELETE FROM crm_emails WHERE id = ANY($1)").bind(&emails).execute(&state.db).await?;
    for f in files {
        crate::routes::files::delete_stored(state, f).await;
    }
    Ok(n)
}

/// Daily job: permanently remove records that have been in the recycle bin too long.
pub async fn run_purge(state: &AppState) -> Result<()> {
    let mut total = 0u64;
    loop {
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM crm_items WHERE deleted_at < now() - make_interval(days => $1) LIMIT 1000")
            .bind(RECYCLE_DAYS as i32)
            .fetch_all(&state.db)
            .await?;
        let n = if ids.is_empty() { 0 } else { purge(state, &ids).await.map_err(|e| anyhow::anyhow!("{e:?}"))? };
        total += n;
        if n == 0 {
            break;
        }
    }
    if total > 0 {
        tracing::info!(purged = total, "emptied old records from the CRM recycle bin");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Saved filters
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct SavedFilter {
    id: Uuid,
    name: String,
    filter: Value,
}

async fn list_filters(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>) -> AppResult<Json<Vec<SavedFilter>>> {
    let t = entity_type(&state, &code).await?;
    Ok(Json(
        sqlx::query_as("SELECT id, name, filter FROM crm_saved_filters WHERE user_id = $1 AND entity_type = $2 ORDER BY name")
            .bind(auth.id)
            .bind(t.id)
            .fetch_all(&state.db)
            .await?,
    ))
}

#[derive(Deserialize)]
struct NewFilter {
    name: String,
    filter: Map<String, Value>,
}

async fn save_filter(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(b): Json<NewFilter>) -> AppResult<Json<Vec<SavedFilter>>> {
    let t = entity_type(&state, &code).await?;
    let name = required("name", &b.name, 100)?;
    // Check the filter parses before saving it.
    let f: Filter = serde_json::from_value(Value::Object(b.filter.clone())).map_err(|_| AppError::bad("invalid filter"))?;
    push_filter(&mut QueryBuilder::new(""), &t, &f, auth.id)?;
    super::items::guard_filter(&state, &auth, &t, &f).await?;
    sqlx::query("INSERT INTO crm_saved_filters (user_id, entity_type, name, filter) VALUES ($1, $2, $3, $4)")
        .bind(auth.id)
        .bind(t.id)
        .bind(name)
        .bind(Value::Object(b.filter))
        .execute(&state.db)
        .await?;
    list_filters(State(state), auth, Path(code)).await
}

async fn delete_filter(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    sqlx::query("DELETE FROM crm_saved_filters WHERE id = $1 AND user_id = $2").bind(id).bind(auth.id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}
