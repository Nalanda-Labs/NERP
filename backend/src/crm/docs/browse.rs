//! All generated documents across records (CRM → Documents), and ZIP download of
//! several PDFs. Only documents on records the caller may read are listed or zipped.

use axum::{
    extract::{Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::collections::HashSet;
use std::io::{Cursor, Write};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    crm::access::{visible, Visible},
    error::{AppError, AppResult},
    routes::files,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/crm/documents", get(list)).route("/crm/documents/zip", post(zip))
}

#[derive(Deserialize)]
struct ListQuery {
    q: Option<String>,
    /// Document kind: the template's document title ("Invoice", "Quote" …).
    kind: Option<String>,
    /// Record type code.
    #[serde(rename = "type")]
    type_code: Option<String>,
    status: Option<String>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Serialize, FromRow)]
struct Row {
    id: Uuid,
    item_id: Uuid,
    number: String,
    kind: String,
    template_name: String,
    engine: String,
    status: String,
    error: Option<String>,
    pdf_file_id: Option<Uuid>,
    source_file_id: Option<Uuid>,
    created_at: DateTime<Utc>,
    created_by_name: Option<String>,
    type_code: String,
    type_name: String,
    item_number: i64,
    item_title: String,
    client: Option<String>,
    amount: Option<String>,
    currency: Option<String>,
}

const SELECT: &str = "SELECT d.id, d.item_id, d.number, COALESCE(t.doc_title, d.template_name) AS kind, d.template_name, d.engine,
    d.status, d.error, d.pdf_file_id, d.source_file_id, d.created_at,
    NULLIF(trim(u.first_name || ' ' || u.last_name), '') AS created_by_name,
    et.code AS type_code, et.name AS type_name, i.number::int8 AS item_number, i.title AS item_title,
    COALESCE(co.title,
        (SELECT NULLIF(trim(concat_ws(' ', c.first_name, c.last_name)), '') FROM crm_item_contacts ic JOIN crm_items c ON c.id = ic.contact_id
          WHERE ic.item_id = i.id ORDER BY ic.is_primary DESC, ic.sort LIMIT 1),
        NULLIF(i.company_name, ''), NULLIF(trim(concat_ws(' ', i.first_name, i.last_name)), '')) AS client,
    i.opportunity::text AS amount, i.currency";

const FROM: &str = " FROM crm_documents d
    JOIN crm_items i ON i.id = d.item_id
    JOIN crm_entity_types et ON et.id = i.entity_type
    LEFT JOIN crm_items co ON co.id = i.company_id
    LEFT JOIN crm_doc_templates t ON t.id = d.template_id
    LEFT JOIN users u ON u.id = d.created_by
    WHERE i.deleted_at IS NULL";

/// The record types the caller can read, with their scope.
async fn scopes(state: &AppState, auth: &AuthUser) -> AppResult<Vec<(i16, Visible)>> {
    let types: Vec<i16> = sqlx::query_scalar("SELECT id FROM crm_entity_types ORDER BY id").fetch_all(&state.db).await?;
    let mut out = Vec::new();
    for t in types {
        let v = visible(state, auth, t, "read").await?;
        if !matches!(v, Visible::Nothing) {
            out.push((t, v));
        }
    }
    Ok(out)
}

/// Appends `AND (<record visible under one of the scopes>)`.
fn push_scopes(qb: &mut QueryBuilder<Postgres>, scopes: &[(i16, Visible)]) {
    qb.push(" AND (FALSE");
    for (t, v) in scopes {
        qb.push(" OR (i.entity_type = ").push_bind(*t);
        v.push(qb);
        qb.push(")");
    }
    qb.push(")");
}

fn push_filters(qb: &mut QueryBuilder<Postgres>, q: &ListQuery) {
    if let Some(s) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let like = format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        qb.push(" AND (d.number ILIKE ").push_bind(like.clone());
        qb.push(" OR i.title ILIKE ").push_bind(like.clone());
        qb.push(" OR co.title ILIKE ").push_bind(like).push(")");
    }
    if let Some(k) = q.kind.as_deref().filter(|s| !s.is_empty()) {
        qb.push(" AND COALESCE(t.doc_title, d.template_name) = ").push_bind(k.to_string());
    }
    if let Some(c) = q.type_code.as_deref().filter(|s| !s.is_empty()) {
        qb.push(" AND et.code = ").push_bind(c.to_string());
    }
    if let Some(s) = q.status.as_deref().filter(|s| !s.is_empty()) {
        qb.push(" AND d.status = ").push_bind(s.to_string());
    }
    if let Some(f) = q.from {
        qb.push(" AND d.created_at >= ").push_bind(f).push("::date");
    }
    if let Some(t) = q.to {
        qb.push(" AND d.created_at < (").push_bind(t).push("::date + 1)");
    }
}

const COUNT_CAP: i64 = 10_000;

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let scopes = scopes(&state, &auth).await?;
    if scopes.is_empty() {
        return Err(AppError::Forbidden);
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);

    let mut cq: QueryBuilder<Postgres> = QueryBuilder::new("SELECT COUNT(*) FROM (SELECT 1");
    cq.push(FROM);
    push_scopes(&mut cq, &scopes);
    push_filters(&mut cq, &q);
    cq.push(" LIMIT ").push_bind(COUNT_CAP + 1).push(") x");
    let total: i64 = cq.build_query_scalar().fetch_one(&state.db).await?;

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(SELECT);
    qb.push(FROM);
    push_scopes(&mut qb, &scopes);
    push_filters(&mut qb, &q);
    qb.push(" ORDER BY d.created_at DESC, d.id DESC LIMIT ").push_bind(limit).push(" OFFSET ").push_bind(offset);
    let items: Vec<Row> = qb.build_query_as().fetch_all(&state.db).await?;

    // Kinds that occur among the documents the caller can see, for the filter.
    let mut kq: QueryBuilder<Postgres> = QueryBuilder::new("SELECT k FROM (SELECT DISTINCT COALESCE(t.doc_title, d.template_name) AS k");
    kq.push(FROM);
    push_scopes(&mut kq, &scopes);
    kq.push(") x ORDER BY k LIMIT 200");
    let kinds: Vec<String> = kq.build_query_scalar().fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": items, "total": total.min(COUNT_CAP), "total_capped": total > COUNT_CAP, "kinds": kinds })))
}

#[derive(Deserialize)]
struct ZipBody {
    ids: Vec<Uuid>,
}

const ZIP_MAX: usize = 200;

/// A file name from a document number: keeps letters, digits and `-_.`.
fn file_stem(number: &str) -> String {
    let s: String = number.chars().map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '-' }).collect();
    let s = s.trim_matches(['-', '.']).to_string();
    if s.is_empty() { "document".into() } else { s }
}

async fn zip(State(state): State<AppState>, auth: AuthUser, Json(b): Json<ZipBody>) -> AppResult<Response> {
    let ids: Vec<Uuid> = b.ids.into_iter().collect::<HashSet<_>>().into_iter().collect();
    if ids.is_empty() {
        return Err(AppError::bad("choose at least one document"));
    }
    if ids.len() > ZIP_MAX {
        return Err(AppError::bad(format!("at most {ZIP_MAX} documents at a time")));
    }
    let scopes = scopes(&state, &auth).await?;
    if scopes.is_empty() {
        return Err(AppError::Forbidden);
    }
    // Same visibility rules as the list: documents the caller can't see are skipped.
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT d.number, d.pdf_file_id");
    qb.push(FROM);
    push_scopes(&mut qb, &scopes);
    qb.push(" AND d.status = 'ready' AND d.pdf_file_id IS NOT NULL AND d.id = ANY(").push_bind(ids).push(") ORDER BY d.number");
    let docs: Vec<(String, Option<Uuid>)> = qb.build_query_as().fetch_all(&state.db).await?;
    if docs.is_empty() {
        return Err(AppError::bad("none of these documents has a PDF ready"));
    }

    let mut buf = Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        // PDFs are already compressed.
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let mut used = HashSet::new();
        for (number, file) in docs {
            let Some(file) = file else { continue };
            let Ok((_, _, _, bytes)) = files::read_bytes(&state, file).await else { continue };
            let stem = file_stem(&number);
            let mut name = format!("{stem}.pdf");
            let mut n = 2;
            while !used.insert(name.clone()) {
                name = format!("{stem} ({n}).pdf");
                n += 1;
            }
            z.start_file(name, opts).map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
            z.write_all(&bytes).map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        }
        z.finish().map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    }
    let mut res = buf.into_inner().into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    let name = format!("documents-{}.zip", Utc::now().format("%Y%m%d-%H%M"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::file_stem;

    #[test]
    fn zip_names_are_safe() {
        assert_eq!(file_stem("INV-2026-0001"), "INV-2026-0001");
        assert_eq!(file_stem("2603/../x"), "2603-..-x");
        assert_eq!(file_stem("../"), "document");
        assert_eq!(file_stem("Rg 1/26"), "Rg-1-26");
    }
}
