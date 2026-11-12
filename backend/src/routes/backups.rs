//! Backups, from the app: the backups `deploy/backup/nerp-backup.sh` made (database dump and
//! uploaded files, one dated folder each), backing up now, proving the newest one restores
//! (`nerp-verify-backup.sh`, into a temporary database), and downloading a database dump.
//!
//! Restoring replaces the live data. From the command line it runs with the server stopped
//! (`nerp-restore.sh`; the page shows the exact command). From the page it runs only when the
//! server allows it (`BACKUP_RESTORE=true`), for someone with "Manage system" who gives their
//! password and types the backup's name: the app pauses (every other request answers 503),
//! other database connections are closed, the backup is restored (database and files, in one
//! transaction for the database), the database is brought up to this version's schema, and
//! the app carries on. Only this server instance pauses; run one instance while restoring.
//!
//! Point-in-time restore: while `nerp-wal-receive.sh` streams the database's WAL into the
//! backup folder (through a replication slot, WAL_SLOT), backups carry a base backup and the
//! database can be put back as it was at any moment since the oldest of them
//! (`nerp-pitr-restore.sh`); the page shows whether WAL is being received and the command,
//! and, when restoring from the app is allowed, does it the same way as a restore.
//!
//! BACKUP_DIR (default /var/backups/nerp) is where backups live; BACKUP_SCRIPTS (default
//! ../deploy/backup, from where the server runs) holds the scripts.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use redis::AsyncCommands;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

use crate::{
    audit,
    auth::{permissions as perm, AuthUser, ClientIp},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/backups", get(list).post(back_up))
        .route("/admin/backups/verify", post(verify))
        .route("/admin/backups/{name}/database.dump", get(download))
        .route("/admin/backups/{name}/restore", post(restore))
        .route("/admin/backups/restore-to-moment", post(restore_to_moment))
}

/// While a restore runs, the API answers 503 (see `maintenance`).
static RESTORING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn restoring() -> bool {
    RESTORING.load(std::sync::atomic::Ordering::SeqCst)
}

/// Middleware: during a restore, everything but the health check waits outside.
pub async fn maintenance(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    use axum::response::IntoResponse;
    // Inside the API router the path has no "/api" in front.
    if restoring() && !matches!(req.uri().path(), "/health" | "/api/health") {
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": { "code": "maintenance", "message": "NERP is restoring a backup; try again in a few minutes" } }))).into_response();
    }
    next.run(req).await
}

fn restore_allowed() -> bool {
    std::env::var("BACKUP_RESTORE").is_ok_and(|v| v == "true")
}

pub(crate) fn dir() -> PathBuf {
    PathBuf::from(std::env::var("BACKUP_DIR").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/var/backups/nerp".into()))
}

fn scripts() -> PathBuf {
    PathBuf::from(std::env::var("BACKUP_SCRIPTS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "../deploy/backup".into()))
}

/// Backup folders are named by their UTC time: 2026-11-10T020000Z.
pub(crate) fn valid_name(n: &str) -> bool {
    n.len() == 18 && n.starts_with("20") && n.ends_with('Z') && n.chars().all(|c| c.is_ascii_digit() || c == '-' || c == 'T' || c == 'Z')
}

pub(crate) fn manifest(path: &std::path::Path) -> Map<String, Value> {
    std::fs::read_to_string(path.join("manifest.txt"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::from(v))))
        .collect()
}

async fn last(state: &AppState, key: &str) -> AppResult<Value> {
    Ok(sqlx::query_scalar::<_, Value>("SELECT value FROM settings WHERE key = $1").bind(key).fetch_optional(&state.db).await?.unwrap_or(Value::Null))
}

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    let d = dir();
    let mut items: Vec<Value> = std::fs::read_dir(&d)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().to_str().map(String::from))
                .filter(|n| valid_name(n))
                .map(|n| {
                    let m = manifest(&d.join(&n));
                    json!({ "name": n, "manifest": m, "has_dump": d.join(&n).join("database.dump").is_file() })
                })
                .collect()
        })
        .unwrap_or_default();
    items.sort_by(|a, b| b["name"].as_str().cmp(&a["name"].as_str()));
    // As set, or relative to the NERP folder (no server paths reach the browser).
    let s = if std::env::var("BACKUP_SCRIPTS").ok().filter(|v| !v.is_empty()).is_some() { scripts() } else { PathBuf::from("deploy/backup") };
    let mut redis = state.redis.clone();
    let running: Option<String> = redis.get("backup:running").await?;
    let newest = items.first().and_then(|b| b["name"].as_str().map(String::from));
    let earliest = items.iter().rev().find(|b| b["manifest"]["base_backup"] == json!(1)).and_then(|b| b["name"].as_str().map(String::from));
    let slot_name = std::env::var("WAL_SLOT").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "nerp_wal".into());
    // The slot: whether something reads it now, when it last answered, and how much WAL the
    // server keeps for it (grows without bound while nothing reads it).
    let slot: Option<(bool, Option<chrono::DateTime<chrono::Utc>>, Option<i64>)> = sqlx::query_as(
        "SELECT s.active, r.reply_time, pg_wal_lsn_diff(pg_current_wal_lsn(), s.restart_lsn)::bigint
         FROM pg_replication_slots s LEFT JOIN pg_stat_replication r ON r.pid = s.active_pid WHERE s.slot_name = $1",
    )
    .bind(&slot_name)
    .fetch_optional(&state.db)
    .await?;
    Ok(Json(json!({
        "dir": d,
        "dir_exists": d.is_dir(),
        "scripts": scripts().join("nerp-backup.sh").is_file(),
        "running": running,
        "items": items,
        "last_backup": last(&state, "backup_last").await?,
        "last_verify": last(&state, "backup_verify_last").await?,
        "restore_command": newest.map(|n| format!("sudo systemctl stop nerp && {}/nerp-restore.sh {}/{n} --yes && sudo systemctl start nerp", s.display(), d.display())),
        "restore_from_app": restore_allowed(),
        "last_restore": last(&state, "backup_restore_last").await?,
        "pitr": {
            "slot": slot_name,
            "set_up": slot.is_some(),
            "receiving": slot.as_ref().is_some_and(|s| s.0),
            "last_reply_at": slot.as_ref().and_then(|s| s.1),
            "retained_bytes": slot.as_ref().and_then(|s| s.2),
            "earliest": earliest.as_deref().and_then(backup_time),
            "command": format!("sudo systemctl stop nerp && {}/nerp-pitr-restore.sh --dest {} --to \"YYYY-MM-DD HH:MM:SS+00\" --into-live --yes && sudo systemctl start nerp", s.display(), d.display()),
        },
    })))
}

/// A backup folder's name as a time: 2026-11-10T020000Z → 2026-11-10T02:00:00Z.
pub(crate) fn backup_time(name: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::NaiveDateTime::parse_from_str(name, "%Y-%m-%dT%H%M%SZ").ok().map(|t| t.and_utc())
}

/// Runs a script in the background; its outcome is kept under `key` in the settings.
async fn run(state: &AppState, auth: &AuthUser, ip: Option<&str>, script: &str, key: &'static str, what: &'static str) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    // The scripts work on the live database (backend/.env): never from a sandbox.
    if state.config.sandbox {
        return Err(AppError::bad("this is a sandbox: backups are made on the live system"));
    }
    let path = scripts().join(script);
    if !path.is_file() {
        return Err(AppError::bad(format!("{} was not found (set BACKUP_SCRIPTS)", path.display())));
    }
    let mut redis = state.redis.clone();
    let free: bool = redis::cmd("SET").arg("backup:running").arg(what).arg("NX").arg("EX").arg(4 * 3600).query_async::<Option<String>>(&mut redis).await?.is_some();
    if !free {
        return Err(AppError::Conflict("a backup or check is already running".into()));
    }
    audit::record(&state.db, Some(auth.id), &format!("system.{what}"), "backup", None, json!({}), ip).await;
    let state = state.clone();
    let started = chrono::Utc::now();
    tokio::spawn(async move {
        let out = tokio::process::Command::new("bash").arg(&path).arg("--dest").arg(dir()).kill_on_drop(false).output().await;
        let (ok, text) = match out {
            Ok(o) => (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))),
            Err(e) => (false, e.to_string()),
        };
        // The NERP folder's own path is shortened (no server paths reach the browser).
        let text = match std::env::current_dir().ok().and_then(|d| d.parent().map(|p| p.display().to_string())) {
            Some(root) if root.len() > 1 => text.replace(&root, "<NERP>"),
            _ => text,
        };
        let tail: String = text.chars().rev().take(4000).collect::<Vec<_>>().into_iter().rev().collect();
        let v = json!({ "started_at": started, "finished_at": chrono::Utc::now(), "ok": ok, "output": tail });
        let _ = sqlx::query("INSERT INTO settings (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()").bind(key).bind(v).execute(&state.db).await;
        let mut redis = state.redis.clone();
        let _: Result<(), _> = redis.del("backup:running").await;
        if !ok {
            tracing::warn!(what, "backup script failed");
        }
    });
    Ok(Json(json!({ "started": true })))
}

async fn back_up(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp) -> AppResult<Json<Value>> {
    run(&state, &auth, ip.as_deref(), "nerp-backup.sh", "backup_last", "backup").await
}

async fn verify(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp) -> AppResult<Json<Value>> {
    run(&state, &auth, ip.as_deref(), "nerp-verify-backup.sh", "backup_verify_last", "backup_verify").await
}

#[derive(serde::Deserialize)]
struct RestoreInput {
    password: String,
    /// The backup's name typed again.
    confirm: String,
}

/// Restores a backup over the live data (see the module notes).
async fn restore(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(name): Path<String>, Json(b): Json<RestoreInput>) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    if !restore_allowed() {
        return Err(AppError::bad("restoring from the app is off on this server (BACKUP_RESTORE); use the command shown"));
    }
    if !valid_name(&name) || !dir().join(&name).join("database.dump").is_file() {
        return Err(AppError::NotFound("backup"));
    }
    if b.confirm.trim() != name {
        return Err(AppError::bad("type the backup's name to confirm"));
    }
    if !crate::routes::auth::verify_user_password(&state, auth.id, b.password).await? {
        return Err(AppError::bad("password is incorrect"));
    }
    let args = vec![dir().join(&name).display().to_string(), "--yes".into()];
    restore_with(&state, &auth, ip.as_deref(), "nerp-restore.sh", args, name, "system.backup_restore").await
}

#[derive(serde::Deserialize)]
struct MomentInput {
    /// The moment, e.g. 2026-11-12T10:30:00+01:00.
    to: chrono::DateTime<chrono::FixedOffset>,
    password: String,
    /// "restore", typed.
    confirm: String,
}

/// Puts the database back as it was at a moment (see the module notes).
async fn restore_to_moment(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Json(b): Json<MomentInput>) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    if !restore_allowed() {
        return Err(AppError::bad("restoring from the app is off on this server (BACKUP_RESTORE); use the command shown"));
    }
    let to = b.to.with_timezone(&chrono::Utc);
    if to >= chrono::Utc::now() {
        return Err(AppError::bad("choose a moment in the past"));
    }
    let earliest = std::fs::read_dir(dir())
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| e.file_name().to_str().map(String::from))
                .filter(|n| valid_name(n) && manifest(&dir().join(n)).get("base_backup") == Some(&json!(1)))
                .filter_map(|n| backup_time(&n))
                .min()
        })
        .ok()
        .flatten();
    match earliest {
        None => return Err(AppError::bad("there are no base backups yet: turn on WAL archiving and make a backup")),
        Some(e) if to < e => return Err(AppError::bad(format!("the earliest moment that can be restored is {}", e.to_rfc3339()))),
        _ => {}
    }
    if b.confirm.trim() != "restore" {
        return Err(AppError::bad("type \"restore\" to confirm"));
    }
    if !crate::routes::auth::verify_user_password(&state, auth.id, b.password).await? {
        return Err(AppError::bad("password is incorrect"));
    }
    let when = to.format("%Y-%m-%d %H:%M:%S+00").to_string();
    let args = vec!["--to".into(), when.clone(), "--dest".into(), dir().display().to_string(), "--into-live".into(), "--yes".into()];
    restore_with(&state, &auth, ip.as_deref(), "nerp-pitr-restore.sh", args, when, "system.backup_restore_to_moment").await
}

/// Pauses the app, runs a restore script over the live data, brings the schema up to date
/// and carries on; the outcome is kept as `backup_restore_last`.
async fn restore_with(state: &AppState, auth: &AuthUser, ip: Option<&str>, script: &str, args: Vec<String>, name: String, action: &str) -> AppResult<Json<Value>> {
    if state.config.sandbox {
        return Err(AppError::bad("this is a sandbox: restores are done on the live system"));
    }
    let script = scripts().join(script);
    if !script.is_file() {
        return Err(AppError::bad(format!("{} was not found (set BACKUP_SCRIPTS)", script.display())));
    }
    let mut redis = state.redis.clone();
    let free: bool = redis::cmd("SET").arg("backup:running").arg("restore").arg("NX").arg("EX").arg(4 * 3600).query_async::<Option<String>>(&mut redis).await?.is_some();
    if !free {
        return Err(AppError::Conflict("a backup or check is already running".into()));
    }
    audit::record(&state.db, Some(auth.id), action, "backup", Some(name.clone()), json!({}), ip).await;
    let state = state.clone();
    let started = chrono::Utc::now();
    tokio::spawn(async move {
        RESTORING.store(true, std::sync::atomic::Ordering::SeqCst);
        // Requests already inside finish; then every other connection to the database goes.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let _ = sqlx::query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid()").execute(&state.db).await;
        let out = tokio::process::Command::new("bash").arg(&script).args(&args).kill_on_drop(false).output().await;
        let (mut ok, mut text) = match out {
            Ok(o) => (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))),
            Err(e) => (false, e.to_string()),
        };
        // An older backup has an older schema: bring it up to this version.
        if ok && let Err(e) = crate::migrate::run(&state.db).await {
            ok = false;
            text.push_str(&format!("\nupdating the database failed: {e}"));
        }
        let text = match std::env::current_dir().ok().and_then(|d| d.parent().map(|p| p.display().to_string())) {
            Some(root) if root.len() > 1 => text.replace(&root, "<NERP>"),
            _ => text,
        };
        let tail: String = text.chars().rev().take(4000).collect::<Vec<_>>().into_iter().rev().collect();
        let v = json!({ "started_at": started, "finished_at": chrono::Utc::now(), "ok": ok, "backup": name, "output": tail });
        let _ = sqlx::query("INSERT INTO settings (key, value) VALUES ('backup_restore_last', $1) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()").bind(v).execute(&state.db).await;
        let mut redis = state.redis.clone();
        let _: Result<(), _> = redis.del("backup:running").await;
        RESTORING.store(false, std::sync::atomic::Ordering::SeqCst);
        if !ok {
            tracing::warn!("restoring the backup failed");
        }
    });
    Ok(Json(json!({ "started": true })))
}

/// The database dump of one backup (all the company's data: for administrators only).
async fn download(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, headers: HeaderMap, Path(name): Path<String>) -> AppResult<Response> {
    auth.require(perm::SYSTEM_MANAGE)?;
    if !valid_name(&name) {
        return Err(AppError::NotFound("backup"));
    }
    let path = dir().join(&name).join("database.dump");
    if !path.is_file() {
        return Err(AppError::NotFound("backup"));
    }
    audit::record(&state.db, Some(auth.id), "system.backup_download", "backup", Some(name.clone()), json!({}), ip.as_deref()).await;
    crate::routes::files::stream_disk(&path, &format!("nerp-{name}.dump"), "application/octet-stream", &headers).await
}

#[cfg(test)]
mod tests {
    #[test]
    fn names() {
        assert!(super::valid_name("2026-11-10T020000Z"));
        assert!(!super::valid_name("../etc/passwd"));
        assert!(!super::valid_name("2026-11-10T02000Z"));
        assert!(!super::valid_name("2026-11-10T0200/0Z"));
    }
}
