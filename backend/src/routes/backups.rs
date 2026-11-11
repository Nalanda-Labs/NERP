//! Backups, from the app: the backups `deploy/backup/nerp-backup.sh` made (database dump and
//! uploaded files, one dated folder each), backing up now, proving the newest one restores
//! (`nerp-verify-backup.sh`, into a temporary database), and downloading a database dump.
//!
//! Restoring replaces the live data, so it runs from the command line with the server
//! stopped (`nerp-restore.sh`); the page shows the exact command.
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
}

fn dir() -> PathBuf {
    PathBuf::from(std::env::var("BACKUP_DIR").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/var/backups/nerp".into()))
}

fn scripts() -> PathBuf {
    PathBuf::from(std::env::var("BACKUP_SCRIPTS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "../deploy/backup".into()))
}

/// Backup folders are named by their UTC time: 2026-11-10T020000Z.
fn valid_name(n: &str) -> bool {
    n.len() == 18 && n.starts_with("20") && n.ends_with('Z') && n.chars().all(|c| c.is_ascii_digit() || c == '-' || c == 'T' || c == 'Z')
}

fn manifest(path: &std::path::Path) -> Map<String, Value> {
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
    Ok(Json(json!({
        "dir": d,
        "dir_exists": d.is_dir(),
        "scripts": scripts().join("nerp-backup.sh").is_file(),
        "running": running,
        "items": items,
        "last_backup": last(&state, "backup_last").await?,
        "last_verify": last(&state, "backup_verify_last").await?,
        "restore_command": newest.map(|n| format!("sudo systemctl stop nerp && {}/nerp-restore.sh {}/{n} --yes && sudo systemctl start nerp", s.display(), d.display())),
    })))
}

/// Runs a script in the background; its outcome is kept under `key` in the settings.
async fn run(state: &AppState, auth: &AuthUser, ip: Option<&str>, script: &str, key: &'static str, what: &'static str) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
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
