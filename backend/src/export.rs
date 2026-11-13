//! One-click full company export in open formats.
//!
//! A ZIP holding every table as CSV (one consistent snapshot), a description of the columns,
//! the stored files decrypted under their own names, and a manifest with each part's SHA-256
//! and the audit chain's head. Passwords, password and token hashes, API secrets, mailbox
//! credentials and encryption keys are left out; session tables and the job queue too.
//!
//! Exports are written to EXPORT_DIR (default <UPLOAD_DIR>/.exports) and removed after
//! EXPORT_KEEP_DAYS (default 7). Only one runs at a time.
use std::{io::Write, path::PathBuf};

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::{delete, get},
    Json, Router,
};
use axum::body::Bytes;
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    audit,
    auth::{permissions as perm, AuthUser, ClientIp},
    error::{AppError, AppResult},
    state::AppState,
};

pub const PURGE_JOB: &str = "export.purge";

/// Tables left out entirely: sign-in secrets, the job queue, migration bookkeeping.
const SKIP_TABLES: &[&str] = &["_sqlx_migrations", "auth_sessions", "auth_used_refresh", "auth_link_tokens", "jobs"];

/// Columns left out (table.column).
const SKIP_COLUMNS: &[&str] = &[
    "api_keys.secret_hash",
    "app_passwords.token_hash",
    "bi_links.password_hash",
    "bi_links.token_hash",
    "bi_sources.password",
    "board_links.password_hash",
    "board_links.token_hash",
    "bookings.token_hash",
    "cal_ext_accounts.access_token",
    "cal_ext_accounts.secret",
    "cal_feeds.token_hash",
    "chat_sessions.token_hash",
    "crm_mailboxes.access_token",
    "crm_mailboxes.secret",
    "crm_triggers.token",
    "directory_config.ldap_secret",
    "directory_config.saml_key",
    "directory_config.sso_secret",
    "drive_links.password_hash",
    "drive_links.token_hash",
    "files.enc_key",
    "files.storage_path",
    "join_links.token_hash",
    "mkt_recipients.token",
    "shop_orders.token",
    "sign_in_providers.secret",
    "sign_signers.token_hash",
    "support_tickets.access_hash",
    "support_tickets.access_secret",
    "user_recovery_codes.code_hash",
    "users.password_hash",
    "users.totp_secret",
    "webhook_subscriptions.secret",
];

/// Any column whose name looks like a secret and isn't listed above is left out as well, so
/// a new table's secret doesn't slip into exports.
fn looks_secret(column: &str) -> bool {
    let c = column.to_ascii_lowercase();
    ["password", "secret", "token_hash", "refresh_hash", "code_hash", "access_token", "refresh_token", "enc_key", "private_key"]
        .iter()
        .any(|w| c.contains(w))
        && !c.ends_with("_at")
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/exports", get(list).post(start))
        .route("/admin/exports/{id}", delete(remove))
        .route("/admin/exports/{id}/download", get(download))
}

fn dir(state: &AppState) -> PathBuf {
    std::env::var("EXPORT_DIR").ok().filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| state.config.upload_dir.join(".exports"))
}

fn keep_days() -> i64 {
    std::env::var("EXPORT_KEEP_DAYS").ok().and_then(|v| v.parse().ok()).unwrap_or(7)
}

#[derive(Serialize, sqlx::FromRow)]
struct Export {
    id: Uuid,
    status: String,
    include_files: bool,
    requested_by: Option<Uuid>,
    requested_by_name: Option<String>,
    file_name: Option<String>,
    size_bytes: Option<i64>,
    sha256: Option<String>,
    tables: Option<i32>,
    files: Option<i32>,
    error: Option<String>,
    created_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    let items: Vec<Export> = sqlx::query_as!(Export, r#"SELECT e.*, CASE WHEN u.id IS NULL THEN NULL ELSE u.first_name || ' ' || u.last_name END AS "requested_by_name?"
         FROM company_exports e LEFT JOIN users u ON u.id = e.requested_by ORDER BY e.created_at DESC LIMIT 50"#)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items, "keep_days": keep_days() })))
}

#[derive(Deserialize)]
struct StartInput {
    #[serde(default = "yes")]
    include_files: bool,
}
fn yes() -> bool {
    true
}

async fn start(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Json(b): Json<StartInput>) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    let id: Option<Uuid> = sqlx::query_scalar!("INSERT INTO company_exports (include_files, requested_by)
         SELECT $1, $2 WHERE NOT EXISTS (SELECT 1 FROM company_exports WHERE status = 'running') RETURNING id", b.include_files, auth.id)
    .fetch_optional(&state.db)
    .await?;
    let Some(id) = id else { return Err(AppError::Conflict("An export is already running".into())) };
    audit::record(&state.db, Some(auth.id), "system.export", "export", Some(id.to_string()), json!({ "include_files": b.include_files }), ip.as_deref()).await;
    let st = state.clone();
    tokio::spawn(async move {
        let r = run(&st, id, b.include_files).await;
        let q = match r {
            Ok(done) => sqlx::query("UPDATE company_exports SET status = 'done', file_name = $2, size_bytes = $3, sha256 = $4, tables = $5, files = $6, finished_at = now() WHERE id = $1")
                .bind(id)
                .bind(done.file_name)
                .bind(done.size as i64)
                .bind(done.sha256)
                .bind(done.tables as i32)
                .bind(done.files as i32),
            Err(e) => {
                tracing::warn!(error = ?e, "company export failed");
                let _ = tokio::fs::remove_file(dir(&st).join(format!("{id}.zip.part"))).await;
                sqlx::query("UPDATE company_exports SET status = 'failed', error = $2, finished_at = now() WHERE id = $1").bind(id).bind(format!("{e:#}"))
            }
        };
        if let Err(e) = q.execute(&st.db).await {
            tracing::warn!(error = ?e, "recording the export's result failed");
        }
    });
    Ok(Json(json!({ "id": id })))
}

async fn find(state: &AppState, id: Uuid) -> AppResult<Export> {
    sqlx::query_as!(Export, r#"SELECT e.*, NULL::text AS "requested_by_name?" FROM company_exports e WHERE id = $1"#, id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("export"))
}

async fn download(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, headers: HeaderMap, Path(id): Path<Uuid>) -> AppResult<Response> {
    auth.require(perm::SYSTEM_MANAGE)?;
    let e = find(&state, id).await?;
    let Some(name) = e.file_name.filter(|_| e.status == "done") else { return Err(AppError::NotFound("export")) };
    let path = dir(&state).join(format!("{id}.zip"));
    if !path.is_file() {
        return Err(AppError::NotFound("export"));
    }
    audit::record(&state.db, Some(auth.id), "system.export_download", "export", Some(id.to_string()), json!({}), ip.as_deref()).await;
    crate::routes::files::stream_disk(&path, &name, "application/zip", &headers).await
}

async fn remove(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    let e = find(&state, id).await?;
    if e.status == "running" {
        return Err(AppError::Conflict("The export is still running".into()));
    }
    let _ = tokio::fs::remove_file(dir(&state).join(format!("{id}.zip"))).await;
    sqlx::query!("DELETE FROM company_exports WHERE id = $1", id).execute(&state.db).await?;
    audit::record(&state.db, Some(auth.id), "system.export_delete", "export", Some(id.to_string()), json!({}), ip.as_deref()).await;
    Ok(Json(json!({ "ok": true })))
}

/// At start: exports interrupted by a restart never finish.
pub async fn at_start(state: &AppState) -> anyhow::Result<()> {
    sqlx::query!("UPDATE company_exports SET status = 'failed', error = 'The server restarted during the export', finished_at = now() WHERE status = 'running'")
        .execute(&state.db)
        .await?;
    for e in std::fs::read_dir(dir(state)).into_iter().flatten().flatten() {
        if e.file_name().to_string_lossy().ends_with(".zip.part") {
            let _ = std::fs::remove_file(e.path());
        }
    }
    Ok(())
}

/// Daily: removes exports older than EXPORT_KEEP_DAYS.
pub async fn run_purge(state: &AppState) -> anyhow::Result<()> {
    let old: Vec<Uuid> = sqlx::query_scalar!("DELETE FROM company_exports WHERE status <> 'running' AND created_at < now() - make_interval(days => $1::int) RETURNING id", keep_days() as i32)
        .fetch_all(&state.db)
        .await?;
    for id in old {
        let _ = tokio::fs::remove_file(dir(state).join(format!("{id}.zip"))).await;
    }
    Ok(())
}

// ---- writing the ZIP ----

enum Msg {
    Start(String),
    Data(Bytes),
}

struct Entry {
    name: String,
    size: u64,
    sha256: String,
}

struct Done {
    file_name: String,
    size: u64,
    sha256: String,
    tables: usize,
    files: usize,
}

/// The writer thread: entries arrive as a name followed by their bytes.
fn write_zip(path: PathBuf, mut rx: mpsc::Receiver<Msg>) -> anyhow::Result<Vec<Entry>> {
    let file = std::fs::File::create(&path)?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::with_capacity(1 << 20, file));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
    let mut entries: Vec<Entry> = vec![];
    let mut hasher: Option<Sha256> = None;
    let close = |entries: &mut Vec<Entry>, hasher: &mut Option<Sha256>| {
        if let (Some(h), Some(e)) = (hasher.take(), entries.last_mut()) {
            e.sha256 = hex::encode(h.finalize());
        }
    };
    while let Some(m) = rx.blocking_recv() {
        match m {
            Msg::Start(name) => {
                close(&mut entries, &mut hasher);
                z.start_file(name.as_str(), opts)?;
                entries.push(Entry { name, size: 0, sha256: String::new() });
                hasher = Some(Sha256::new());
            }
            Msg::Data(b) => {
                z.write_all(&b)?;
                if let (Some(h), Some(e)) = (hasher.as_mut(), entries.last_mut()) {
                    h.update(&b);
                    e.size += b.len() as u64;
                }
            }
        }
    }
    close(&mut entries, &mut hasher);
    z.finish()?.flush()?;
    Ok(entries)
}

/// A name safe as a ZIP path component.
fn safe_name(n: &str) -> String {
    let s: String = n.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    let s = s.trim().trim_start_matches('.').to_string();
    if s.is_empty() { "file".into() } else { s.chars().take(150).collect() }
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

async fn run(state: &AppState, id: Uuid, include_files: bool) -> anyhow::Result<Done> {
    let d = dir(state);
    tokio::fs::create_dir_all(&d).await?;
    let part = d.join(format!("{id}.zip.part"));
    let (tx, rx) = mpsc::channel::<Msg>(32);
    let p = part.clone();
    let writer = tokio::task::spawn_blocking(move || write_zip(p, rx));
    let send = |m: Msg| {
        let tx = tx.clone();
        async move { tx.send(m).await.map_err(|_| anyhow::anyhow!("the ZIP writer stopped")) }
    };

    // One consistent snapshot for every table.
    let mut db = state.db.begin().await?;
    sqlx::query!("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY").execute(&mut *db).await?;
    let cols: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT c.table_name::text, c.column_name::text, c.data_type::text, c.is_nullable::text
         FROM information_schema.columns c JOIN information_schema.tables t USING (table_schema, table_name)
         WHERE c.table_schema = 'public' AND t.table_type = 'BASE TABLE' ORDER BY c.table_name, c.ordinal_position",
    )
    .fetch_all(&mut *db)
    .await?;
    let mut tables: Vec<(String, Vec<(String, String, bool)>, Vec<String>)> = vec![];
    for (t, c, ty, nullable) in cols {
        if SKIP_TABLES.contains(&t.as_str()) {
            continue;
        }
        if tables.last().map(|x| &x.0) != Some(&t) {
            tables.push((t.clone(), vec![], vec![]));
        }
        let last = tables.last_mut().expect("pushed");
        if SKIP_COLUMNS.contains(&format!("{t}.{c}").as_str()) || looks_secret(&c) {
            last.2.push(c);
        } else {
            last.1.push((c, ty, nullable == "YES"));
        }
    }
    tables.retain(|t| !t.1.is_empty());

    let mut table_info = vec![];
    for (t, columns, left_out) in &tables {
        let list = columns.iter().map(|c| quote_ident(&c.0)).collect::<Vec<_>>().join(", ");
        let rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM {}", quote_ident(t)))).fetch_one(&mut *db).await?;
        send(Msg::Start(format!("data/{t}.csv"))).await?;
        {
            let mut s = db.copy_out_raw(&format!("COPY (SELECT {list} FROM {}) TO STDOUT WITH (FORMAT csv, HEADER)", quote_ident(t))).await?;
            while let Some(chunk) = s.try_next().await? {
                send(Msg::Data(chunk)).await?;
            }
        }
        table_info.push(json!({
            "name": t,
            "file": format!("data/{t}.csv"),
            "rows": rows,
            "columns": columns.iter().map(|(c, ty, n)| json!({ "name": c, "type": ty, "nullable": n })).collect::<Vec<_>>(),
            "left_out": left_out,
        }));
    }

    let mut file_info = vec![];
    if include_files {
        let files: Vec<(Uuid, String, String, String, Option<String>, i64)> =
            sqlx::query!(r#"SELECT id, original_name, mime_type, storage_path, enc_key, size_bytes FROM files ORDER BY created_at, id"#).map(|r| (r.id, r.original_name, r.mime_type, r.storage_path, r.enc_key, r.size_bytes)).fetch_all(&mut *db).await?;
        for (fid, name, mime, rel, key, size) in files {
            let path = format!("files/{fid}/{}", safe_name(&name));
            let blob = match crate::blob::open(&state.config.file_keys, &state.config.upload_dir.join(&rel), key.as_deref()).await {
                Ok(b) => b,
                Err(e) => {
                    file_info.push(json!({ "id": fid, "name": name, "missing": e.to_string() }));
                    continue;
                }
            };
            send(Msg::Start(path.clone())).await?;
            let total = blob.size;
            let mut s = std::pin::pin!(blob.stream(0, total));
            while let Some(chunk) = s.try_next().await? {
                send(Msg::Data(chunk)).await?;
            }
            file_info.push(json!({ "id": fid, "name": name, "mime_type": mime, "size": size, "path": path }));
        }
    }
    let head: Option<(i64, Vec<u8>)> = sqlx::query!(r#"SELECT chain_pos AS "chain_pos!", hash AS "hash!" FROM audit_log WHERE chain_pos IS NOT NULL ORDER BY chain_pos DESC LIMIT 1"#).map(|r| (r.chain_pos, r.hash)).fetch_optional(&mut *db).await?;
    let schema: Option<i64> = sqlx::query_scalar!(r#"SELECT max(version) AS "v?" FROM _sqlx_migrations"#).fetch_one(&mut *db).await?;
    let company: Option<Value> = sqlx::query_scalar!("SELECT value FROM settings WHERE key = 'company_name'").fetch_optional(&mut *db).await?;
    db.commit().await?;

    send(Msg::Start("README.txt".into())).await?;
    send(Msg::Data(Bytes::from_static(README.as_bytes()))).await?;
    // The manifest is written last so it can list every part's checksum: the writer reports them.
    drop(tx);
    let entries = writer.await??;
    let checks: Vec<Value> = entries.iter().map(|e| json!({ "path": e.name, "size": e.size, "sha256": e.sha256 })).collect();
    let manifest = json!({
        "format": "nerp-export/1",
        "created_at": Utc::now(),
        "company": company,
        "schema_version": schema,
        "audit_head": head.map(|(p, h)| json!({ "position": p, "hash": hex::encode(h) })),
        "tables": table_info,
        "files": file_info,
        "checksums": checks,
    });
    // Append the manifest to the finished archive.
    let p = part.clone();
    let tables_n = tables.len();
    let files_n = file_info.iter().filter(|f| f.get("path").is_some()).count();
    let body = serde_json::to_vec_pretty(&manifest)?;
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let f = std::fs::OpenOptions::new().read(true).write(true).open(&p)?;
        let mut z = zip::ZipWriter::new_append(f)?;
        z.start_file("manifest.json", zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated))?;
        z.write_all(&body)?;
        z.finish()?;
        Ok(())
    })
    .await??;

    // The archive's own checksum, for the record.
    let p = part.clone();
    let (size, sha256) = tokio::task::spawn_blocking(move || -> anyhow::Result<(u64, String)> {
        let mut f = std::fs::File::open(&p)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut n = 0u64;
        loop {
            let k = std::io::Read::read(&mut f, &mut buf)?;
            if k == 0 {
                break;
            }
            h.update(&buf[..k]);
            n += k as u64;
        }
        Ok((n, hex::encode(h.finalize())))
    })
    .await??;
    tokio::fs::rename(&part, d.join(format!("{id}.zip"))).await?;
    let file_name = format!("nerp-export-{}.zip", Utc::now().format("%Y-%m-%d-%H%M"));
    Ok(Done { file_name, size, sha256, tables: tables_n, files: files_n })
}

const README: &str = "NERP company export (format nerp-export/1)

data/<table>.csv   every table as CSV with a header row (one consistent snapshot).
                   Dates are ISO 8601 in UTC; JSON columns are JSON text; arrays use
                   PostgreSQL's {a,b} form; empty unquoted fields are NULL.
files/<id>/<name>  the stored files, decrypted, under their original names. The id matches
                   files.id in data/files.csv, which other tables point to.
manifest.json      tables with their columns and types (and columns left out), files,
                   the SHA-256 of every part, the schema version and the audit trail's head
                   (position and hash) at the time of the export.

Left out on purpose: passwords and password/token hashes, two-factor secrets, API and
webhook secrets, mailbox and calendar credentials, sign-in provider secrets, file
encryption keys, sign-in sessions and the job queue.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_columns_are_spotted() {
        assert!(looks_secret("smtp_password"));
        assert!(looks_secret("refresh_token"));
        assert!(!looks_secret("password_changed_at"));
        assert!(!looks_secret("title"));
    }

    #[test]
    fn zip_names() {
        assert_eq!(safe_name("../a/b.txt"), "_a_b.txt");
        assert_eq!(safe_name(""), "file");
    }
}
