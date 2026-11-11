//! BI: dashboards of charts over datasets.
//!
//! - **Datasets**: NERP's own data (deals, leads, tasks, working time, tickets…, see
//!   `builtin`), CSV files uploaded or CSV/JSON fetched from a URL (refreshed every few
//!   hours), and tables or queries in outside PostgreSQL databases.
//! - **Charts** group a dataset by one or two columns (dates by day, week, month, quarter or
//!   year) and show figures (count, total, average, smallest, largest, distinct values), or
//!   list rows as a table; each has its own filters, and a date column the dashboard's
//!   period applies to. A single figure can be compared with the period before.
//! - **Dashboards** have a period and filters (applied to every chart whose dataset has the
//!   column), saved as presets for oneself or everyone; they are shared with people,
//!   departments or everyone, or through public links (with an expiry date or a password).
//! - People with `bi.manage` build dashboards and datasets and see every dashboard; others
//!   see what is shared with them. Every dataset is company-wide: whoever may open a
//!   dashboard sees its figures.
//! - Datasets can be downloaded as CSV (with an API key too), so spreadsheets and other BI
//!   tools can read them.

pub mod builtin;
pub mod query;
mod sources;

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    auth::{permissions::BI, tokens::random_token, AuthUser, ClientIp},
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};
use query::{Column, Ctx, Filter, Spec, P};
use sources::{bind_all, Source, SOURCE_COLS};

pub const REFRESH_JOB: &str = "bi.refresh";
const CHARTS: &[&str] = &["number", "bar", "hbar", "line", "area", "pie", "funnel", "table"];
const MAX_WIDGETS: i64 = 40;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/bi/dashboards", get(list).post(create))
        .route("/bi/dashboards/{id}", get(open).patch(update).delete(remove))
        .route("/bi/dashboards/{id}/copy", post(copy))
        .route("/bi/dashboards/{id}/data", post(data))
        .route("/bi/dashboards/{id}/widgets", post(add_widget))
        .route("/bi/dashboards/{id}/layout", put(layout))
        .route("/bi/dashboards/{id}/access", put(set_access))
        .route("/bi/dashboards/{id}/links", get(links).post(new_link))
        .route("/bi/dashboards/{id}/links/{link}", delete(drop_link))
        .route("/bi/dashboards/{id}/presets", post(add_preset))
        .route("/bi/presets/{id}", delete(drop_preset))
        .route("/bi/widgets/{id}", patch(change_widget).delete(drop_widget))
        .route("/bi/query", post(preview))
        .route("/bi/datasets", get(datasets).post(new_dataset))
        .route("/bi/datasets/{key}", get(dataset_info).patch(change_dataset).delete(drop_dataset))
        .route("/bi/datasets/{key}/refresh", post(refresh))
        .route("/bi/datasets/{key}/export.csv", get(export))
        .route("/bi/sources", get(list_sources).post(new_source))
        .route("/bi/sources/{id}", patch(change_source).delete(drop_source))
        .route("/bi/sources/{id}/tables", get(tables))
        .route("/public/bi/{token}", get(pub_open))
        .route("/public/bi/{token}/data", post(pub_data))
}

fn bad(e: impl Into<String>) -> AppError {
    AppError::BadRequest(e.into())
}

// ---------------------------------------------------------------------------
// Datasets
// ---------------------------------------------------------------------------

enum Target {
    Nerp,
    Outside(Source),
}

struct Ds {
    key: String,
    name: String,
    cols: Vec<Column>,
    from_sql: String,
    target: Target,
}

#[derive(sqlx::FromRow)]
struct DsRow {
    id: Uuid,
    name: String,
    kind: String,
    source_id: Option<Uuid>,
    query: Option<String>,
    url: Option<String>,
    refresh_hours: Option<i32>,
    columns: Value,
    row_count: i32,
    refreshed_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
}
const DS_COLS: &str = "id, name, kind, source_id, query, url, refresh_hours, columns, row_count, refreshed_at, last_error";

async fn ds_row(state: &AppState, id: Uuid) -> AppResult<DsRow> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {DS_COLS} FROM bi_datasets WHERE id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("dataset"))
}

async fn source(state: &AppState, id: Uuid) -> AppResult<Source> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {SOURCE_COLS} FROM bi_sources WHERE id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("source"))
}

async fn dataset(state: &AppState, key: &str) -> AppResult<Ds> {
    if let Some(k) = key.strip_prefix("nerp:") {
        let b = builtin::find(k).ok_or(AppError::NotFound("dataset"))?;
        return Ok(Ds {
            key: key.into(),
            name: b.name.into(),
            cols: b.cols.iter().map(|c| Column { key: c.key.into(), label: c.label.into(), ty: c.ty }).collect(),
            from_sql: b.sql.into(),
            target: Target::Nerp,
        });
    }
    let id: Uuid = key.parse().map_err(|_| AppError::NotFound("dataset"))?;
    let r = ds_row(state, id).await?;
    let cols: Vec<Column> = serde_json::from_value(r.columns).unwrap_or_default();
    Ok(match r.source_id {
        Some(sid) => {
            let s = source(state, sid).await?;
            Ds { key: key.into(), name: r.name, from_sql: sources::outside_sql(r.query.as_deref().unwrap_or(""), &cols), cols, target: Target::Outside(s) }
        }
        None => Ds { key: key.into(), name: r.name, from_sql: sources::rows_sql(r.id, &cols), cols, target: Target::Nerp },
    })
}

/// Runs statements on the dataset's database: rows of JSON arrays.
async fn execute(state: &AppState, ds: &Ds, built: &[query::Built]) -> Result<Vec<Vec<Value>>, String> {
    let mut texts = Vec::new();
    match &ds.target {
        Target::Nerp => {
            let run = async {
                let mut tx = state.db.begin().await?;
                sqlx::query("SET TRANSACTION READ ONLY").execute(&mut *tx).await?;
                sqlx::query("SET LOCAL statement_timeout = '20s'").execute(&mut *tx).await?;
                let mut out = Vec::new();
                for b in built {
                    out.push(bind_all!(sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(b.sql.clone())), &b.params).fetch_all(&mut *tx).await?);
                }
                tx.rollback().await?;
                Ok::<_, sqlx::Error>(out)
            };
            texts = run.await.map_err(|e| {
                let s = e.to_string();
                if s.contains("statement timeout") {
                    "the query took too long; narrow it down with filters or a shorter period".to_string()
                } else {
                    tracing::warn!(error = %s, dataset = %ds.key, "BI query failed");
                    format!("the query failed: {}", s.chars().take(300).collect::<String>())
                }
            })?;
        }
        Target::Outside(s) => {
            let mut c = sources::connect(state, s).await?;
            for b in built {
                match c.rows(b).await {
                    Ok(r) => texts.push(r),
                    Err(e) => {
                        c.close().await;
                        return Err(e);
                    }
                }
            }
            c.close().await;
        }
    }
    Ok(texts
        .into_iter()
        .map(|rows| rows.into_iter().map(|t| serde_json::from_str(&t).unwrap_or(Value::Null)).collect())
        .collect())
}

/// One chart's figures: {columns, rows} and, when compared, the period before.
async fn run(state: &AppState, cache: &tokio::sync::Mutex<HashMap<String, std::sync::Arc<Ds>>>, spec: &Spec, ctx: &Ctx<'_>, compare: bool) -> Result<Value, String> {
    let ds = {
        let mut c = cache.lock().await;
        match c.get(&spec.dataset) {
            Some(d) => d.clone(),
            None => {
                let d = std::sync::Arc::new(dataset(state, &spec.dataset).await.map_err(|e| e.to_string())?);
                c.insert(spec.dataset.clone(), d.clone());
                d
            }
        }
    };
    let tz = if matches!(ds.target, Target::Nerp) { ctx.tz } else { None };
    let here = Ctx { period: ctx.period, extra: ctx.extra, tz };
    let mut built = vec![query::build(spec, &ds.cols, &ds.from_sql, &here)?];
    let before = if compare && spec.date_field.is_some() { ctx.period.map(query::previous) } else { None };
    if let Some(p) = before {
        built.push(query::build(spec, &ds.cols, &ds.from_sql, &Ctx { period: Some(p), extra: ctx.extra, tz })?);
    }
    let mut res = execute(state, &ds, &built).await?;
    let mut v = json!({ "columns": built[0].columns, "rows": res.remove(0) });
    if let Some((f, t)) = before {
        v["previous"] = json!({ "rows": res.remove(0), "from": f, "to": t - chrono::Duration::days(1) });
    }
    Ok(v)
}

fn need_bi(auth: &AuthUser) -> AppResult<()> {
    auth.require(BI)
}

async fn datasets(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    let own: Vec<DsRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {DS_COLS} FROM bi_datasets ORDER BY lower(name)"))).fetch_all(&state.db).await?;
    let names: HashMap<Uuid, String> = sqlx::query_as::<_, (Uuid, String)>("SELECT id, name FROM bi_sources").fetch_all(&state.db).await?.into_iter().collect();
    let mut items: Vec<Value> = builtin::ALL
        .iter()
        .map(|b| json!({ "key": format!("nerp:{}", b.key), "name": b.name, "group": b.group, "kind": "nerp",
            "columns": b.cols.iter().map(|c| json!({ "key": c.key, "label": c.label, "type": c.ty })).collect::<Vec<_>>() }))
        .collect();
    items.extend(own.into_iter().map(|r| ds_json(&r, &names)));
    Ok(Json(json!({ "items": items })))
}

fn ds_json(r: &DsRow, names: &HashMap<Uuid, String>) -> Value {
    json!({ "key": r.id.to_string(), "id": r.id, "name": r.name, "group": "Own data", "kind": r.kind, "source_id": r.source_id,
        "source_name": r.source_id.and_then(|s| names.get(&s)), "query": r.query, "url": r.url, "refresh_hours": r.refresh_hours, "columns": r.columns,
        "row_count": r.row_count, "refreshed_at": r.refreshed_at, "last_error": r.last_error })
}

async fn dataset_info(State(state): State<AppState>, auth: AuthUser, Path(key): Path<String>) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    let ds = dataset(&state, &key).await?;
    let spec = Spec { dataset: key.clone(), columns: ds.cols.iter().take(30).map(|c| c.key.clone()).collect(), limit: Some(20), ..Default::default() };
    let preview = match query::build(&spec, &ds.cols, &ds.from_sql, &Ctx::default()) {
        Ok(b) => execute(&state, &ds, &[b]).await.map(|mut r| json!(r.remove(0))).unwrap_or_else(|e| json!({ "error": e })),
        Err(e) => json!({ "error": e }),
    };
    let mut v = json!({ "key": ds.key, "name": ds.name, "columns": ds.cols, "preview": preview });
    if let Ok(id) = key.parse::<Uuid>() {
        let names: HashMap<Uuid, String> = sqlx::query_as::<_, (Uuid, String)>("SELECT id, name FROM bi_sources").fetch_all(&state.db).await?.into_iter().collect();
        let r = ds_row(&state, id).await?;
        let mut j = ds_json(&r, &names);
        j["preview"] = v["preview"].take();
        j["used_by"] = json!(sqlx::query_scalar::<_, String>("SELECT DISTINCT d.title FROM bi_widgets w JOIN bi_dashboards d ON d.id = w.dashboard_id WHERE w.spec->>'dataset' = $1").bind(&key).fetch_all(&state.db).await?);
        v = j;
    }
    Ok(Json(v))
}

#[derive(Deserialize)]
struct NewDataset {
    name: String,
    kind: String,
    file_id: Option<Uuid>,
    url: Option<String>,
    refresh_hours: Option<i32>,
    source_id: Option<Uuid>,
    table: Option<String>,
    query: Option<String>,
}

fn check_refresh(h: Option<i32>) -> AppResult<()> {
    if h.is_some_and(|h| !(1..=24 * 31).contains(&h)) {
        return Err(bad("refresh every 1 hour to 31 days"));
    }
    Ok(())
}

async fn read_upload(state: &AppState, auth: &AuthUser, file_id: Uuid) -> AppResult<sources::Table> {
    if !crate::routes::files::may_read(state, auth, file_id).await? {
        return Err(AppError::NotFound("file"));
    }
    let (name, mime, _, bytes) = crate::routes::files::read_bytes(state, file_id).await?;
    let json = name.to_lowercase().ends_with(".json") || mime.contains("json");
    if json { sources::parse_json(&bytes) } else { sources::parse_csv(&bytes) }.map_err(bad)
}

async fn new_dataset(State(state): State<AppState>, auth: AuthUser, Json(b): Json<NewDataset>) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    let name = required("name", &b.name, 200)?;
    let id = Uuid::new_v4();
    match b.kind.as_str() {
        "csv" => {
            let t = read_upload(&state, &auth, b.file_id.ok_or_else(|| bad("upload a CSV file"))?).await?;
            sqlx::query("INSERT INTO bi_datasets (id, name, kind, created_by) VALUES ($1, $2, 'csv', $3)").bind(id).bind(&name).bind(auth.id).execute(&state.db).await?;
            let (cols, rows) = sources::typed(t);
            sources::store(&state, id, &cols, rows).await?;
        }
        "url" => {
            check_refresh(b.refresh_hours)?;
            let url = b.url.as_deref().map(str::trim).filter(|u| !u.is_empty()).ok_or_else(|| bad("give the file's URL"))?;
            crate::outbound::check_url(url).map_err(bad)?;
            let t = sources::fetch(&state, url).await.map_err(bad)?;
            sqlx::query("INSERT INTO bi_datasets (id, name, kind, url, refresh_hours, created_by) VALUES ($1, $2, 'url', $3, $4, $5)")
                .bind(id)
                .bind(&name)
                .bind(url)
                .bind(b.refresh_hours)
                .bind(auth.id)
                .execute(&state.db)
                .await?;
            let (cols, rows) = sources::typed(t);
            sources::store(&state, id, &cols, rows).await?;
        }
        "sql" => {
            let s = source(&state, b.source_id.ok_or_else(|| bad("pick a database"))?).await?;
            let q = match (b.table.as_deref().filter(|t| !t.is_empty()), b.query.as_deref().map(sources::strip).filter(|q| !q.is_empty())) {
                (Some(t), _) => sources::table_query(t).map_err(bad)?,
                (None, Some(q)) if q.len() <= 20_000 => q.to_string(),
                _ => return Err(bad("pick a table or write a query")),
            };
            let cols = describe(&state, &s, &q).await?;
            sqlx::query("INSERT INTO bi_datasets (id, name, kind, source_id, query, columns, refreshed_at, created_by) VALUES ($1, $2, 'sql', $3, $4, $5, now(), $6)")
                .bind(id)
                .bind(&name)
                .bind(s.id)
                .bind(&q)
                .bind(serde_json::to_value(&cols).map_err(anyhow::Error::from)?)
                .bind(auth.id)
                .execute(&state.db)
                .await?;
        }
        _ => return Err(bad("a dataset is a CSV file, a URL or a database query")),
    }
    dataset_info(State(state), auth, Path(id.to_string())).await
}

async fn describe(state: &AppState, s: &Source, q: &str) -> AppResult<Vec<Column>> {
    let mut c = sources::connect(state, s).await.map_err(|e| bad(format!("cannot connect: {e}")))?;
    let cols = c.describe(q).await;
    c.close().await;
    cols.map_err(|e| bad(format!("the query does not work: {e}")))
}

#[derive(Deserialize)]
struct DatasetChange {
    name: Option<String>,
    url: Option<String>,
    refresh_hours: Option<Option<i32>>,
    query: Option<String>,
}

async fn change_dataset(State(state): State<AppState>, auth: AuthUser, Path(key): Path<String>, Json(b): Json<DatasetChange>) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    let id: Uuid = key.parse().map_err(|_| AppError::Forbidden)?;
    let r = ds_row(&state, id).await?;
    if let Some(n) = &b.name {
        sqlx::query("UPDATE bi_datasets SET name = $2 WHERE id = $1").bind(id).bind(required("name", n, 200)?).execute(&state.db).await?;
    }
    if let Some(h) = b.refresh_hours {
        check_refresh(h)?;
        sqlx::query("UPDATE bi_datasets SET refresh_hours = $2 WHERE id = $1").bind(id).bind(h).execute(&state.db).await?;
    }
    if let Some(u) = b.url.as_deref().filter(|_| r.kind == "url") {
        crate::outbound::check_url(u.trim()).map_err(bad)?;
        sqlx::query("UPDATE bi_datasets SET url = $2 WHERE id = $1").bind(id).bind(u.trim()).execute(&state.db).await?;
        refresh_one(&state, id).await.map_err(bad)?;
    }
    if let (Some(q), Some(sid)) = (b.query.as_deref().map(sources::strip).filter(|q| !q.is_empty()), r.source_id) {
        let s = source(&state, sid).await?;
        let cols = describe(&state, &s, q).await?;
        sqlx::query("UPDATE bi_datasets SET query = $2, columns = $3, refreshed_at = now(), last_error = NULL WHERE id = $1")
            .bind(id)
            .bind(q)
            .bind(serde_json::to_value(&cols).map_err(anyhow::Error::from)?)
            .execute(&state.db)
            .await?;
    }
    dataset_info(State(state), auth, Path(key)).await
}

#[derive(Deserialize, Default)]
struct RefreshInput {
    file_id: Option<Uuid>,
}

/// Fetches a URL dataset again, reads an outside query's columns again, or replaces a CSV
/// dataset's rows with a new file.
async fn refresh(State(state): State<AppState>, auth: AuthUser, Path(key): Path<String>, body: Option<Json<RefreshInput>>) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    let id: Uuid = key.parse().map_err(|_| bad("NERP's own data is always current"))?;
    let r = ds_row(&state, id).await?;
    match r.kind.as_str() {
        "csv" => {
            let f = body.and_then(|b| b.0.file_id).ok_or_else(|| bad("upload the new file"))?;
            let (cols, rows) = sources::typed(read_upload(&state, &auth, f).await?);
            sources::store(&state, id, &cols, rows).await?;
        }
        "url" => refresh_one(&state, id).await.map_err(bad)?,
        _ => {
            let s = source(&state, r.source_id.expect("sql datasets have a source")).await?;
            let cols = describe(&state, &s, r.query.as_deref().unwrap_or("")).await?;
            sqlx::query("UPDATE bi_datasets SET columns = $2, refreshed_at = now(), last_error = NULL WHERE id = $1").bind(id).bind(serde_json::to_value(&cols).map_err(anyhow::Error::from)?).execute(&state.db).await?;
        }
    }
    dataset_info(State(state), auth, Path(key)).await
}

async fn refresh_one(state: &AppState, id: Uuid) -> Result<(), String> {
    let url: Option<String> = sqlx::query_scalar("SELECT url FROM bi_datasets WHERE id = $1 AND kind = 'url'").bind(id).fetch_optional(&state.db).await.map_err(|e| e.to_string())?.flatten();
    let url = url.ok_or("not a URL dataset")?;
    let res = match sources::fetch(state, &url).await {
        Ok(t) => {
            let (cols, rows) = sources::typed(t);
            sources::store(state, id, &cols, rows).await.map_err(|e| e.to_string())
        }
        Err(e) => Err(e),
    };
    if let Err(e) = &res {
        let _ = sqlx::query("UPDATE bi_datasets SET last_error = $2, refreshed_at = now() WHERE id = $1").bind(id).bind(e).execute(&state.db).await;
    }
    res
}

/// The job: URL datasets due for a refresh.
pub async fn refresh_due(state: &AppState) -> anyhow::Result<()> {
    let due: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM bi_datasets WHERE kind = 'url' AND refresh_hours IS NOT NULL AND (refreshed_at IS NULL OR refreshed_at < now() - refresh_hours * interval '1 hour') LIMIT 50",
    )
    .fetch_all(&state.db)
    .await?;
    for id in due {
        if let Err(e) = refresh_one(state, id).await {
            tracing::info!(dataset = %id, error = %e, "BI dataset refresh failed");
        }
    }
    Ok(())
}

async fn drop_dataset(State(state): State<AppState>, auth: AuthUser, Path(key): Path<String>) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    let id: Uuid = key.parse().map_err(|_| AppError::Forbidden)?;
    let used: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bi_widgets WHERE spec->>'dataset' = $1").bind(&key).fetch_one(&state.db).await?;
    if used > 0 {
        return Err(AppError::Conflict(format!("{used} chart(s) use this dataset; remove them first")));
    }
    sqlx::query("DELETE FROM bi_datasets WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Every row (up to 100,000), as CSV.
async fn export(State(state): State<AppState>, auth: AuthUser, Path(key): Path<String>) -> AppResult<Response> {
    need_bi(&auth)?;
    let ds = dataset(&state, &key).await?;
    let sql = query::export(&ds.cols, &ds.from_sql, 100_000);
    let rows = execute(&state, &ds, &[query::Built { sql, params: vec![], columns: vec![] }]).await.map_err(bad)?.remove(0);
    use crate::routes::users::csv_cell;
    let mut body = String::from("\u{feff}");
    body.push_str(&ds.cols.iter().map(|c| csv_cell(&c.label)).collect::<Vec<_>>().join(","));
    body.push_str("\r\n");
    for r in rows {
        let cells: Vec<String> = r
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| match v {
                        Value::Null => String::new(),
                        Value::String(s) => csv_cell(s),
                        // Numbers and true/false cannot be formulas.
                        o => o.to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        body.push_str(&cells.join(","));
        body.push_str("\r\n");
    }
    let file = sources::unique_key(&ds.name, &mut vec![]);
    Ok(([(header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{file}.csv\""))], body).into_response())
}

// ---------------------------------------------------------------------------
// Outside databases
// ---------------------------------------------------------------------------

async fn list_sources(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    let rows: Vec<Source> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {SOURCE_COLS} FROM bi_sources ORDER BY lower(name)"))).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.iter().map(source_json).collect::<Vec<_>>() })))
}

fn source_json(s: &Source) -> Value {
    json!({ "id": s.id, "name": s.name, "kind": s.kind, "host": s.host, "port": s.port, "database": s.database, "username": s.username, "has_password": s.password.is_some(), "tls": s.tls })
}

#[derive(Deserialize)]
struct SourceInput {
    name: Option<String>,
    kind: Option<String>,
    host: Option<String>,
    port: Option<i32>,
    database: Option<String>,
    username: Option<String>,
    password: Option<String>,
    tls: Option<bool>,
}

/// Saves a database after connecting to it once.
async fn save_source(state: &AppState, auth: &AuthUser, id: Option<Uuid>, b: SourceInput) -> AppResult<Json<Value>> {
    let old = match id {
        Some(i) => Some(source(state, i).await?),
        None => None,
    };
    let kind = b.kind.or(old.as_ref().map(|o| o.kind.clone())).unwrap_or_else(|| "postgres".into());
    if kind != "postgres" {
        return Err(bad("outside databases are PostgreSQL"));
    }
    let s = Source {
        id: id.unwrap_or_else(Uuid::new_v4),
        name: required("name", &b.name.or(old.as_ref().map(|o| o.name.clone())).unwrap_or_default(), 200)?,
        host: required("host", &b.host.or(old.as_ref().map(|o| o.host.clone())).unwrap_or_default(), 255)?,
        port: b.port.or(old.as_ref().map(|o| o.port)).unwrap_or(5432),
        database: required("database", &b.database.or(old.as_ref().map(|o| o.database.clone())).unwrap_or_default(), 200)?,
        username: required("user", &b.username.or(old.as_ref().map(|o| o.username.clone())).unwrap_or_default(), 200)?,
        password: match b.password {
            Some(p) if p.is_empty() => None,
            Some(p) => Some(crate::crypto::seal(&state.config.data_key, &p)),
            None => old.as_ref().and_then(|o| o.password.clone()),
        },
        tls: b.tls.or(old.as_ref().map(|o| o.tls)).unwrap_or(false),
        kind,
    };
    if !(1..=65535).contains(&s.port) {
        return Err(bad("the port is 1 to 65535"));
    }
    let c = sources::connect(state, &s).await.map_err(|e| bad(format!("cannot connect: {e}")))?;
    c.close().await;
    sqlx::query(
        "INSERT INTO bi_sources (id, name, kind, host, port, database, username, password, tls, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (id) DO UPDATE SET name = $2, kind = $3, host = $4, port = $5, database = $6, username = $7, password = $8, tls = $9",
    )
    .bind(s.id)
    .bind(&s.name)
    .bind(&s.kind)
    .bind(&s.host)
    .bind(s.port)
    .bind(&s.database)
    .bind(&s.username)
    .bind(&s.password)
    .bind(s.tls)
    .bind(auth.id)
    .execute(&state.db)
    .await?;
    Ok(Json(source_json(&s)))
}

async fn new_source(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SourceInput>) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    save_source(&state, &auth, None, b).await
}

async fn change_source(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<SourceInput>) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    save_source(&state, &auth, Some(id), b).await
}

async fn drop_source(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    let used: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bi_widgets w JOIN bi_datasets d ON w.spec->>'dataset' = d.id::text WHERE d.source_id = $1").bind(id).fetch_one(&state.db).await?;
    if used > 0 {
        return Err(AppError::Conflict(format!("{used} chart(s) use data from this database; remove them first")));
    }
    sqlx::query("DELETE FROM bi_sources WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn tables(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    let s = source(&state, id).await?;
    let mut c = sources::connect(&state, &s).await.map_err(|e| bad(format!("cannot connect: {e}")))?;
    let t = c.tables().await;
    c.close().await;
    Ok(Json(json!({ "items": t.map_err(bad)? })))
}

// ---------------------------------------------------------------------------
// Dashboards
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
struct Dash {
    id: Uuid,
    title: String,
    description: Option<String>,
    owner_id: Uuid,
    filters: Value,
    updated_at: DateTime<Utc>,
}

async fn load(state: &AppState, id: Uuid) -> AppResult<Dash> {
    sqlx::query_as("SELECT id, title, description, owner_id, filters, updated_at FROM bi_dashboards WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("dashboard"))
}

const SHARED: &str = "EXISTS (SELECT 1 FROM bi_access a WHERE a.dashboard_id = d.id AND (a.grantee_kind = 'everyone' OR (a.grantee_kind = 'user' AND a.grantee_id = $1)
    OR (a.grantee_kind = 'department' AND a.grantee_id IN (SELECT department_id FROM user_departments WHERE user_id = $1))))";

/// The dashboard, if the person may open it; and whether they may change it.
async fn viewable(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<(Dash, bool)> {
    let d = load(state, id).await?;
    if auth.can(BI) {
        return Ok((d, true));
    }
    let ok = d.owner_id == auth.id || sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT {SHARED} FROM bi_dashboards d WHERE d.id = $2"))).bind(auth.id).bind(id).fetch_one(&state.db).await?;
    if !ok {
        return Err(AppError::NotFound("dashboard"));
    }
    Ok((d, false))
}

async fn editable(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Dash> {
    need_bi(auth)?;
    load(state, id).await
}

#[derive(Deserialize)]
struct ListQuery {
    q: Option<String>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let all = auth.can(BI);
    let term = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")));
    let rows: Vec<(Uuid, String, Option<String>, Option<String>, DateTime<Utc>, i64, bool)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT d.id, d.title, d.description, bi_person(d.owner_id), d.updated_at, (SELECT COUNT(*) FROM bi_widgets w WHERE w.dashboard_id = d.id),
            EXISTS (SELECT 1 FROM bi_links l WHERE l.dashboard_id = d.id AND (l.expires_at IS NULL OR l.expires_at > now()))
         FROM bi_dashboards d
         WHERE ($2 OR d.owner_id = $1 OR {SHARED})
           AND ($3::text IS NULL OR d.title ILIKE $3 OR d.description ILIKE $3 OR bi_person(d.owner_id) ILIKE $3)
         ORDER BY d.updated_at DESC LIMIT 500"
    )))
    .bind(auth.id)
    .bind(all)
    .bind(term)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "can_manage": all, "items": rows.into_iter().map(|(id, t, d, o, u, n, p)| json!({ "id": id, "title": t, "description": d, "owner": o, "updated_at": u, "charts": n, "public": p })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct DashInput {
    title: Option<String>,
    description: Option<String>,
    filters: Option<Value>,
}

fn check_filters(v: &Value) -> AppResult<()> {
    if !v.is_object() {
        return Err(bad("filters are an object"));
    }
    if let Some(p) = v.get("period") {
        query::period(p, Utc::now().date_naive()).map_err(bad)?;
    }
    if let Some(f) = v.get("filters") {
        let f: Vec<Filter> = serde_json::from_value(f.clone()).map_err(|_| bad("filters are a list of field, op, value"))?;
        if f.len() > 20 || f.iter().any(|f| !query::valid_key(&f.field)) {
            return Err(bad("at most 20 filters, on columns"));
        }
    }
    Ok(())
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<DashInput>) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    let title = required("title", b.title.as_deref().unwrap_or(""), 200)?;
    let filters = b.filters.unwrap_or_else(|| json!({ "period": { "preset": "this_month" } }));
    check_filters(&filters)?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO bi_dashboards (title, description, owner_id, filters) VALUES ($1, $2, $3, $4) RETURNING id")
        .bind(title)
        .bind(b.description.as_deref().map(str::trim).filter(|s| !s.is_empty()))
        .bind(auth.id)
        .bind(filters)
        .fetch_one(&state.db)
        .await?;
    open(State(state), auth, Path(id)).await
}

#[derive(sqlx::FromRow)]
struct Widget {
    id: Uuid,
    title: String,
    chart: String,
    w: i16,
    h: i16,
    spec: Value,
}

async fn widgets(state: &AppState, id: Uuid) -> AppResult<Vec<Widget>> {
    Ok(sqlx::query_as("SELECT id, title, chart, w, h, spec FROM bi_widgets WHERE dashboard_id = $1 ORDER BY position, created_at").bind(id).fetch_all(&state.db).await?)
}

fn widget_json(w: &Widget, with_spec: bool) -> Value {
    let mut v = json!({ "id": w.id, "title": w.title, "chart": w.chart, "w": w.w, "h": w.h, "compare": w.spec.get("compare").and_then(Value::as_bool).unwrap_or(false) });
    if with_spec {
        v["spec"] = w.spec.clone();
    }
    v
}

/// The columns dashboard filters can use: those of the datasets its charts read.
async fn fields(state: &AppState, ws: &[Widget]) -> Vec<Value> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let keys: std::collections::BTreeSet<String> = ws.iter().filter_map(|w| w.spec.get("dataset").and_then(Value::as_str).map(String::from)).collect();
    for k in keys {
        if let Ok(ds) = dataset(state, &k).await {
            for c in ds.cols {
                if !seen.contains(&c.key) {
                    seen.push(c.key.clone());
                    out.push(json!(c));
                }
            }
        }
    }
    out
}

async fn open(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (d, can_edit) = viewable(&state, &auth, id).await?;
    let ws = widgets(&state, id).await?;
    let presets: Vec<(Uuid, String, bool, Value, bool)> = sqlx::query_as(
        "SELECT id, name, shared, filters, user_id = $2 FROM bi_presets WHERE dashboard_id = $1 AND (user_id = $2 OR shared) ORDER BY shared, lower(name)",
    )
    .bind(id)
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    let mut v = json!({
        "id": d.id, "title": d.title, "description": d.description, "filters": d.filters, "updated_at": d.updated_at,
        "owner": sqlx::query_scalar::<_, Option<String>>("SELECT bi_person($1)").bind(d.owner_id).fetch_one(&state.db).await?,
        "can_edit": can_edit,
        "widgets": ws.iter().map(|w| widget_json(w, true)).collect::<Vec<_>>(),
        "fields": fields(&state, &ws).await,
        "presets": presets.into_iter().map(|(i, n, s, f, mine)| json!({ "id": i, "name": n, "shared": s, "filters": f, "mine": mine })).collect::<Vec<_>>(),
    });
    if can_edit {
        v["access"] = access_json(&state, id).await?;
    }
    Ok(Json(v))
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<DashInput>) -> AppResult<Json<Value>> {
    editable(&state, &auth, id).await?;
    let title = b.title.as_deref().map(|t| required("title", t, 200)).transpose()?;
    if let Some(f) = &b.filters {
        check_filters(f)?;
    }
    sqlx::query("UPDATE bi_dashboards SET title = COALESCE($2, title), description = CASE WHEN $3 THEN $4 ELSE description END, filters = COALESCE($5, filters), updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(title)
        .bind(b.description.is_some())
        .bind(b.description.as_deref().map(str::trim).filter(|s| !s.is_empty()))
        .bind(b.filters)
        .execute(&state.db)
        .await?;
    open(State(state), auth, Path(id)).await
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    editable(&state, &auth, id).await?;
    sqlx::query("DELETE FROM bi_dashboards WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn copy(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let d = editable(&state, &auth, id).await?;
    let mut tx = state.db.begin().await?;
    let new: Uuid = sqlx::query_scalar("INSERT INTO bi_dashboards (title, description, owner_id, filters) VALUES ($1, $2, $3, $4) RETURNING id")
        .bind(format!("{} (copy)", d.title).chars().take(200).collect::<String>())
        .bind(&d.description)
        .bind(auth.id)
        .bind(&d.filters)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO bi_widgets (dashboard_id, position, title, chart, w, h, spec) SELECT $2, position, title, chart, w, h, spec FROM bi_widgets WHERE dashboard_id = $1")
        .bind(id)
        .bind(new)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    open(State(state), auth, Path(new)).await
}

#[derive(Deserialize, Default)]
struct DataInput {
    #[serde(default)]
    period: Value,
    #[serde(default)]
    filters: Vec<Filter>,
}

async fn tz_of(state: &AppState, user: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT timezone FROM users WHERE id = $1").bind(user).fetch_optional(&state.db).await.ok().flatten().filter(|t| t.parse::<chrono_tz::Tz>().is_ok()).unwrap_or_else(|| "UTC".into())
}

/// Every chart's figures.
async fn figures(state: &AppState, ws: &[Widget], b: &DataInput, tz: &str) -> AppResult<Value> {
    if b.filters.len() > 20 {
        return Err(bad("at most 20 filters"));
    }
    let today = Utc::now().with_timezone(&tz.parse::<chrono_tz::Tz>().unwrap_or(chrono_tz::UTC)).date_naive();
    let period = query::period(&b.period, today).map_err(bad)?;
    let ctx = Ctx { period, extra: &b.filters, tz: Some(tz) };
    let cache = tokio::sync::Mutex::new(HashMap::new());
    let jobs = ws.iter().map(|w| {
        let (ctx, cache) = (&ctx, &cache);
        async move {
            let res = match serde_json::from_value::<Spec>(w.spec.clone()) {
                Ok(spec) => run(state, cache, &spec, ctx, w.chart == "number" && w.spec.get("compare").and_then(Value::as_bool).unwrap_or(false)).await,
                Err(e) => Err(format!("the chart is broken: {e}")),
            };
            match res {
                Ok(v) => json!({ "id": w.id, "result": v }),
                Err(e) => json!({ "id": w.id, "error": e }),
            }
        }
    });
    let out = futures_util::future::join_all(jobs).await;
    Ok(json!({ "period": period.map(|(f, t)| json!({ "from": f, "to": t - chrono::Duration::days(1) })), "items": out }))
}

async fn data(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, body: Option<Json<DataInput>>) -> AppResult<Json<Value>> {
    viewable(&state, &auth, id).await?;
    let ws = widgets(&state, id).await?;
    let tz = tz_of(&state, auth.id).await;
    figures(&state, &ws, &body.map(|b| b.0).unwrap_or_default(), &tz).await.map(Json)
}

#[derive(Deserialize)]
struct PreviewInput {
    spec: Spec,
    #[serde(default)]
    chart: Option<String>,
    #[serde(default)]
    period: Value,
    #[serde(default)]
    filters: Vec<Filter>,
}

/// A chart's figures while it is being made.
async fn preview(State(state): State<AppState>, auth: AuthUser, Json(b): Json<PreviewInput>) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    let tz = tz_of(&state, auth.id).await;
    let today = Utc::now().with_timezone(&tz.parse::<chrono_tz::Tz>().unwrap_or(chrono_tz::UTC)).date_naive();
    let period = query::period(&b.period, today).map_err(bad)?;
    let cache = tokio::sync::Mutex::new(HashMap::new());
    let compare = b.chart.as_deref() == Some("number") && serde_json::to_value(&b.spec).ok().and_then(|v| v.get("compare").cloned()).and_then(|v| v.as_bool()).unwrap_or(false);
    let v = run(&state, &cache, &b.spec, &Ctx { period, extra: &b.filters, tz: Some(&tz) }, compare).await.map_err(bad)?;
    Ok(Json(v))
}

#[derive(Deserialize)]
struct WidgetInput {
    title: Option<String>,
    chart: Option<String>,
    w: Option<i16>,
    h: Option<i16>,
    spec: Option<Value>,
}

/// Checks a chart against its dataset (without running it).
async fn check_widget(state: &AppState, chart: &str, w: i16, h: i16, spec: &Value) -> AppResult<()> {
    if !CHARTS.contains(&chart) {
        return Err(bad("unknown kind of chart"));
    }
    if !(2..=12).contains(&w) || !(1..=6).contains(&h) {
        return Err(bad("a chart is 2 to 12 columns wide and 1 to 6 rows high"));
    }
    let s: Spec = serde_json::from_value(spec.clone()).map_err(|e| bad(format!("the chart's settings are wrong: {e}")))?;
    let ds = dataset(state, &s.dataset).await?;
    query::build(&s, &ds.cols, &ds.from_sql, &Ctx::default()).map_err(bad)?;
    if chart == "table" && s.groups.is_empty() && s.measures.is_empty() && s.columns.is_empty() {
        return Err(bad("pick the table's columns"));
    }
    if chart != "table" && !s.columns.is_empty() && s.groups.is_empty() && s.measures.is_empty() {
        return Err(bad("charts show figures; pick what to count"));
    }
    Ok(())
}

async fn add_widget(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<WidgetInput>) -> AppResult<Json<Value>> {
    editable(&state, &auth, id).await?;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bi_widgets WHERE dashboard_id = $1").bind(id).fetch_one(&state.db).await?;
    if n >= MAX_WIDGETS {
        return Err(bad(format!("at most {MAX_WIDGETS} charts on a dashboard")));
    }
    let title = required("title", b.title.as_deref().unwrap_or(""), 200)?;
    let chart = b.chart.unwrap_or_default();
    let (w, h) = (b.w.unwrap_or(6), b.h.unwrap_or(2));
    let spec = b.spec.ok_or_else(|| bad("the chart needs its settings"))?;
    check_widget(&state, &chart, w, h, &spec).await?;
    sqlx::query("INSERT INTO bi_widgets (dashboard_id, position, title, chart, w, h, spec) VALUES ($1, $2, $3, $4, $5, $6, $7)")
        .bind(id)
        .bind(n as i32)
        .bind(title)
        .bind(&chart)
        .bind(w)
        .bind(h)
        .bind(spec)
        .execute(&state.db)
        .await?;
    sqlx::query("UPDATE bi_dashboards SET updated_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
    open(State(state), auth, Path(id)).await
}

async fn change_widget(State(state): State<AppState>, auth: AuthUser, Path(wid): Path<Uuid>, Json(b): Json<WidgetInput>) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    let (dash, old): (Uuid, Widget) = {
        let d: Uuid = sqlx::query_scalar("SELECT dashboard_id FROM bi_widgets WHERE id = $1").bind(wid).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("chart"))?;
        let w: Widget = sqlx::query_as("SELECT id, title, chart, w, h, spec FROM bi_widgets WHERE id = $1").bind(wid).fetch_one(&state.db).await?;
        (d, w)
    };
    let title = match b.title.as_deref() {
        Some(t) => required("title", t, 200)?,
        None => old.title,
    };
    let chart = b.chart.unwrap_or(old.chart);
    let (w, h) = (b.w.unwrap_or(old.w), b.h.unwrap_or(old.h));
    let spec = b.spec.unwrap_or(old.spec);
    check_widget(&state, &chart, w, h, &spec).await?;
    sqlx::query("UPDATE bi_widgets SET title = $2, chart = $3, w = $4, h = $5, spec = $6 WHERE id = $1").bind(wid).bind(title).bind(chart).bind(w).bind(h).bind(spec).execute(&state.db).await?;
    sqlx::query("UPDATE bi_dashboards SET updated_at = now() WHERE id = $1").bind(dash).execute(&state.db).await?;
    open(State(state), auth, Path(dash)).await
}

async fn drop_widget(State(state): State<AppState>, auth: AuthUser, Path(wid): Path<Uuid>) -> AppResult<Json<Value>> {
    need_bi(&auth)?;
    let dash: Uuid = sqlx::query_scalar("DELETE FROM bi_widgets WHERE id = $1 RETURNING dashboard_id").bind(wid).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("chart"))?;
    open(State(state), auth, Path(dash)).await
}

#[derive(Deserialize)]
struct LayoutItem {
    id: Uuid,
    w: i16,
    h: i16,
}

#[derive(Deserialize)]
struct LayoutInput {
    items: Vec<LayoutItem>,
}

/// The charts' order and sizes.
async fn layout(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<LayoutInput>) -> AppResult<Json<Value>> {
    editable(&state, &auth, id).await?;
    if b.items.iter().any(|i| !(2..=12).contains(&i.w) || !(1..=6).contains(&i.h)) {
        return Err(bad("a chart is 2 to 12 columns wide and 1 to 6 rows high"));
    }
    let mut tx = state.db.begin().await?;
    for (pos, i) in b.items.iter().enumerate() {
        sqlx::query("UPDATE bi_widgets SET position = $3, w = $4, h = $5 WHERE id = $1 AND dashboard_id = $2").bind(i.id).bind(id).bind(pos as i32).bind(i.w).bind(i.h).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE bi_dashboards SET updated_at = now() WHERE id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    open(State(state), auth, Path(id)).await
}

// ---------------------------------------------------------------------------
// Sharing and presets
// ---------------------------------------------------------------------------

async fn access_json(state: &AppState, id: Uuid) -> AppResult<Value> {
    let rows: Vec<(String, Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT a.grantee_kind, a.grantee_id, CASE a.grantee_kind WHEN 'user' THEN bi_person(a.grantee_id) WHEN 'department' THEN (SELECT name FROM departments WHERE id = a.grantee_id) END
         FROM bi_access a WHERE a.dashboard_id = $1 ORDER BY a.grantee_kind",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(json!(rows.into_iter().map(|(k, i, n)| json!({ "kind": k, "id": i, "name": n })).collect::<Vec<_>>()))
}

#[derive(Deserialize)]
struct Grant {
    kind: String,
    id: Option<Uuid>,
}

#[derive(Deserialize)]
struct AccessInput {
    grants: Vec<Grant>,
}

async fn set_access(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<AccessInput>) -> AppResult<Json<Value>> {
    editable(&state, &auth, id).await?;
    if b.grants.len() > 200 {
        return Err(bad("at most 200 entries"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM bi_access WHERE dashboard_id = $1").bind(id).execute(&mut *tx).await?;
    for g in &b.grants {
        let gid = match (g.kind.as_str(), g.id) {
            ("everyone", _) => None,
            ("user" | "department", Some(u)) => Some(u),
            _ => return Err(bad("share with everyone, a department or a person")),
        };
        sqlx::query("INSERT INTO bi_access (dashboard_id, grantee_kind, grantee_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
            .bind(id)
            .bind(&g.kind)
            .bind(gid)
            .execute(&mut *tx)
            .await
            .map_err(|_| bad("no such person or department"))?;
    }
    tx.commit().await?;
    Ok(Json(json!({ "access": access_json(&state, id).await? })))
}

#[derive(Deserialize)]
struct PresetInput {
    name: String,
    #[serde(default)]
    shared: bool,
    filters: Value,
}

async fn add_preset(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PresetInput>) -> AppResult<Json<Value>> {
    let (_, can_edit) = viewable(&state, &auth, id).await?;
    if b.shared && !can_edit {
        return Err(AppError::Forbidden);
    }
    check_filters(&b.filters)?;
    let name = required("name", &b.name, 100)?;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bi_presets WHERE dashboard_id = $1 AND user_id = $2").bind(id).bind(auth.id).fetch_one(&state.db).await?;
    if n >= 50 {
        return Err(bad("at most 50 presets"));
    }
    sqlx::query("INSERT INTO bi_presets (dashboard_id, user_id, name, shared, filters) VALUES ($1, $2, $3, $4, $5)").bind(id).bind(auth.id).bind(name).bind(b.shared).bind(b.filters).execute(&state.db).await?;
    open(State(state), auth, Path(id)).await
}

async fn drop_preset(State(state): State<AppState>, auth: AuthUser, Path(pid): Path<Uuid>) -> AppResult<Json<Value>> {
    let row: Option<(Uuid, Uuid)> = sqlx::query_as("SELECT dashboard_id, user_id FROM bi_presets WHERE id = $1").bind(pid).fetch_optional(&state.db).await?;
    let (dash, owner) = row.ok_or(AppError::NotFound("preset"))?;
    if owner != auth.id && !auth.can(BI) {
        return Err(AppError::NotFound("preset"));
    }
    sqlx::query("DELETE FROM bi_presets WHERE id = $1").bind(pid).execute(&state.db).await?;
    open(State(state), auth, Path(dash)).await
}

fn hash(t: &str) -> String {
    hex::encode(Sha256::digest(t.as_bytes()))
}

async fn links(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    editable(&state, &auth, id).await?;
    let rows: Vec<(Uuid, Option<DateTime<Utc>>, bool, DateTime<Utc>, Option<String>)> = sqlx::query_as(
        "SELECT id, expires_at, password_hash IS NOT NULL, created_at, bi_person(created_by) FROM bi_links WHERE dashboard_id = $1 ORDER BY created_at",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(l, e, p, c, by)| json!({ "id": l, "expires_at": e, "password": p, "created_at": c, "created_by": by })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct LinkInput {
    expires_at: Option<DateTime<Utc>>,
    password: Option<String>,
}

/// A public, view-only link; its address is shown only now.
async fn new_link(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<LinkInput>) -> AppResult<Json<Value>> {
    crate::routes::settings::require_public_links(&state).await?;
    editable(&state, &auth, id).await?;
    if b.expires_at.is_some_and(|e| e <= Utc::now()) {
        return Err(bad("the expiry date has passed"));
    }
    let pw = match b.password.filter(|p| !p.is_empty()) {
        Some(p) if p.len() < 4 => return Err(bad("the password needs at least 4 characters")),
        Some(p) => Some(crate::auth::password::hash_password(p).await?),
        None => None,
    };
    let token = random_token(24);
    let lid: Uuid = sqlx::query_scalar("INSERT INTO bi_links (dashboard_id, token_hash, expires_at, password_hash, created_by) VALUES ($1, $2, $3, $4, $5) RETURNING id")
        .bind(id)
        .bind(hash(&token))
        .bind(b.expires_at)
        .bind(pw)
        .bind(auth.id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(json!({ "id": lid, "url": format!("{}/dash/{token}", state.config.public_url) })))
}

async fn drop_link(State(state): State<AppState>, auth: AuthUser, Path((id, link)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    editable(&state, &auth, id).await?;
    sqlx::query("DELETE FROM bi_links WHERE id = $1 AND dashboard_id = $2").bind(link).bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

/// The dashboard behind a link: not expired, the password right (`X-Dashboard-Password`),
/// and not too many wrong guesses.
async fn by_link(state: &AppState, token: &str, headers: &HeaderMap, ip: Option<&str>) -> AppResult<Dash> {
    if !crate::routes::settings::public_links_allowed(state).await? {
        return Err(AppError::NotFound("dashboard"));
    }
    let row: Option<(Uuid, Option<DateTime<Utc>>, Option<String>)> =
        sqlx::query_as("SELECT dashboard_id, expires_at, password_hash FROM bi_links WHERE token_hash = $1").bind(hash(token)).fetch_optional(&state.db).await?;
    let (dash, expires, pw) = row.ok_or(AppError::NotFound("dashboard"))?;
    if expires.is_some_and(|e| e <= Utc::now()) {
        return Err(AppError::NotFound("dashboard"));
    }
    if let Some(h) = pw {
        let mut redis = state.redis.clone();
        let key = format!("bi:pw:{}:{}", hash(token), ip.unwrap_or("?"));
        let fails: i64 = redis.get(&key).await.unwrap_or(0);
        if fails >= 10 {
            return Err(AppError::TooManyRequests("too many wrong passwords; try again later".into()));
        }
        let given = headers.get("X-Dashboard-Password").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        if given.is_empty() {
            return Err(AppError::InvalidCredentials("this dashboard needs a password".into()));
        }
        if !crate::auth::password::verify_password(given, h).await? {
            let _: Result<i64, _> = redis.incr(&key, 1).await;
            let _: Result<(), _> = redis.expire(&key, 900).await;
            return Err(AppError::InvalidCredentials("wrong password".into()));
        }
    }
    load(state, dash).await
}

async fn pub_open(State(state): State<AppState>, ClientIp(ip): ClientIp, headers: HeaderMap, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let d = by_link(&state, &token, &headers, ip.as_deref()).await?;
    let ws = widgets(&state, d.id).await?;
    Ok(Json(json!({ "title": d.title, "description": d.description, "filters": d.filters, "widgets": ws.iter().map(|w| widget_json(w, false)).collect::<Vec<_>>() })))
}

async fn pub_data(State(state): State<AppState>, ClientIp(ip): ClientIp, headers: HeaderMap, Path(token): Path<String>, body: Option<Json<DataInput>>) -> AppResult<Json<Value>> {
    let d = by_link(&state, &token, &headers, ip.as_deref()).await?;
    let ws = widgets(&state, d.id).await?;
    // Visitors pick the period; the dashboard's own filters always apply.
    let mut input = body.map(|b| b.0).unwrap_or_default();
    input.filters = d.filters.get("filters").cloned().and_then(|f| serde_json::from_value(f).ok()).unwrap_or_default();
    let tz = tz_of(&state, d.owner_id).await;
    figures(&state, &ws, &input, &tz).await.map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_dataset_builds() {
        for b in builtin::ALL {
            let cols: Vec<Column> = b.cols.iter().map(|c| Column { key: c.key.into(), label: c.label.into(), ty: c.ty }).collect();
            assert!(cols.iter().all(|c| query::valid_key(&c.key)), "{}", b.key);
            let spec = Spec { dataset: format!("nerp:{}", b.key), columns: cols.iter().map(|c| c.key.clone()).collect(), ..Default::default() };
            query::build(&spec, &cols, b.sql, &Ctx::default()).unwrap();
        }
    }
}
