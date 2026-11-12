//! System → Health: one page that says whether the installation is in good shape, and a
//! self-test that proves the moving parts work (database, Redis, file storage with its
//! encryption, outgoing mail).
//!
//! The `health.check` job runs every 15 minutes, keeps the result and, when a check turns
//! red or amber (or everything turns green again), emails those who manage the system and
//! notifies them in NERP.
use std::path::Path;

use axum::{extract::State, routing::{get, post, put}, Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    audit,
    auth::{permissions as perm, AuthUser, ClientIp},
    error::{AppError, AppResult},
    state::AppState,
};

pub const JOB: &str = "health.check";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/health", get(show))
        .route("/admin/health/run", post(run_now))
        .route("/admin/health/settings", put(save_settings))
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Off,
    Ok,
    Warn,
    Fail,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Check {
    key: &'static str,
    title: &'static str,
    level: Level,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    link: Option<&'static str>,
}

fn check(key: &'static str, title: &'static str, level: Level, text: impl Into<String>, link: Option<&'static str>) -> Check {
    Check { key, title, level, text: text.into(), link }
}

fn ago(t: DateTime<Utc>) -> String {
    let m = (Utc::now() - t).num_minutes().max(0);
    match m {
        0..=1 => "just now".into(),
        2..=89 => format!("{m} minutes ago"),
        90..=2879 => format!("{} hours ago", (m + 30) / 60),
        _ => format!("{} days ago", (m + 720) / 1440),
    }
}

fn bytes(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", units[i]) }
}

// ---- the checks ----

async fn database(state: &AppState) -> Check {
    let t = std::time::Instant::now();
    let r: Result<(i64, i32, i32), _> = sqlx::query_as(
        "SELECT pg_database_size(current_database()), (SELECT count(*)::int FROM pg_stat_activity WHERE datname = current_database()),
                current_setting('max_connections')::int",
    )
    .fetch_one(&state.db)
    .await;
    match r {
        Err(e) => check("database", "Database", Level::Fail, format!("Not answering: {e}"), None),
        Ok((size, used, max)) => {
            let ms = t.elapsed().as_millis();
            let level = if used * 10 > max * 9 { Level::Fail } else if used * 10 > max * 8 || ms > 1000 { Level::Warn } else { Level::Ok };
            check("database", "Database", level, format!("Answers in {ms} ms; {}; {used} of {max} connections in use", bytes(size as u64)), None)
        }
    }
}

async fn redis_check(state: &AppState) -> Check {
    let mut r = state.redis.clone();
    if let Err(e) = redis::cmd("PING").query_async::<String>(&mut r).await {
        return check("redis", "Redis", Level::Fail, format!("Not answering: {e}"), Some("/system"));
    }
    let p = crate::routes::admin::redis_persistence(state).await;
    if p["persistent"] == json!(true) {
        check("redis", "Redis", if p["last_save_ok"] == json!(false) { Level::Warn } else { Level::Ok }, if p["last_save_ok"] == json!(false) { "Answers, but its last save to disk failed" } else { "Answers and keeps its data on disk" }, Some("/system"))
    } else {
        check("redis", "Redis", Level::Warn, "Answers, but doesn't save to disk: a restart drops caches and rate limits (sign-ins are kept in the database)", Some("/system"))
    }
}

fn backups() -> Vec<Check> {
    let d = crate::routes::backups::dir();
    let mut names: Vec<String> = std::fs::read_dir(&d)
        .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().to_str().map(String::from)).filter(|n| crate::routes::backups::valid_name(n)).collect())
        .unwrap_or_default();
    names.sort();
    let Some(newest) = names.last() else {
        return vec![check("backups", "Backups", Level::Fail, if d.is_dir() { "No backup yet".to_string() } else { "No backups: the backup folder doesn't exist (see SETUP.md, Backups)".to_string() }, Some("/system/backups"))];
    };
    let at = crate::routes::backups::backup_time(newest);
    let hours = at.map(|t| (Utc::now() - t).num_hours()).unwrap_or(i64::MAX);
    let level = if hours > 48 { Level::Fail } else if hours > 26 { Level::Warn } else { Level::Ok };
    let mut out = vec![check("backups", "Backups", level, format!("Newest backup {}; {} kept", at.map(ago).unwrap_or_else(|| newest.clone()), names.len()), Some("/system/backups"))];
    // The newest backup that a restore test looked at.
    let verified = names.iter().rev().find_map(|n| {
        let m = crate::routes::backups::manifest(&d.join(n));
        let ok = m.get("verified_at").and_then(|v| v.as_str()).and_then(|v| v.parse::<DateTime<Utc>>().ok());
        let bad = m.get("verify_failed_at").and_then(|v| v.as_str()).and_then(|v| v.parse::<DateTime<Utc>>().ok());
        (ok.is_some() || bad.is_some()).then_some((n.clone(), ok, bad))
    });
    out.push(match verified {
        None => check("backup_restore_test", "Restore test", Level::Warn, "No backup has been test-restored yet (nerp-verify-backup.sh runs after each nightly backup)", Some("/system/backups")),
        Some((_, ok, bad)) if bad > ok => check("backup_restore_test", "Restore test", Level::Fail, format!("The last restore test failed ({})", bad.map(ago).unwrap_or_default()), Some("/system/backups")),
        Some((n, Some(ok), _)) => {
            let days = (Utc::now() - ok).num_days();
            let level = if &n != newest && days > 2 { Level::Warn } else if days > 8 { Level::Warn } else { Level::Ok };
            check("backup_restore_test", "Restore test", level, format!("A backup was restored and checked {}", ago(ok)), Some("/system/backups"))
        }
        Some(_) => check("backup_restore_test", "Restore test", Level::Warn, "No passing restore test", Some("/system/backups")),
    });
    out
}

async fn wal(state: &AppState) -> Check {
    let slot = std::env::var("WAL_SLOT").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "nerp_wal".into());
    let r: Result<Option<(bool, Option<i64>)>, _> = sqlx::query_as("SELECT active, pg_wal_lsn_diff(pg_current_wal_lsn(), restart_lsn)::bigint FROM pg_replication_slots WHERE slot_name = $1")
        .bind(&slot)
        .fetch_optional(&state.db)
        .await;
    match r {
        Ok(None) => check("pitr", "Point-in-time restore", Level::Warn, "Not set up: the database can only go back to the last nightly backup", Some("/system/backups")),
        Ok(Some((false, kept))) => check("pitr", "Point-in-time restore", Level::Fail, format!("The change log isn't being received; the database keeps {} for it meanwhile", bytes(kept.unwrap_or(0).max(0) as u64)), Some("/system/backups")),
        Ok(Some((true, kept))) => {
            let kept = kept.unwrap_or(0).max(0) as u64;
            let level = if kept > 8 << 30 { Level::Warn } else { Level::Ok };
            check("pitr", "Point-in-time restore", level, format!("The change log is being received ({} behind)", bytes(kept)), Some("/system/backups"))
        }
        Err(e) => check("pitr", "Point-in-time restore", Level::Warn, format!("Couldn't read the replication slot: {e}"), Some("/system/backups")),
    }
}

async fn encryption(state: &AppState) -> Check {
    match crate::blob::status(state).await {
        Err(e) => check("encryption", "File encryption", Level::Warn, e.to_string(), Some("/system/storage")),
        Ok(s) if s["enabled"] != json!(true) => check("encryption", "File encryption", Level::Warn, "Stored files aren't encrypted (FILE_ENCRYPTION_KEY isn't set)", Some("/system/storage")),
        Ok(s) if s["converting"] == json!(true) => check("encryption", "File encryption", Level::Ok, format!("On; {} files still being moved to the current key", s["not_encrypted"].as_i64().unwrap_or(0) + s["other_key"].as_i64().unwrap_or(0)), Some("/system/storage")),
        Ok(s) => check("encryption", "File encryption", Level::Ok, format!("On; {} files encrypted", s["encrypted"]), Some("/system/storage")),
    }
}

/// Free space on the file system holding `p` (or its nearest existing parent).
fn free_space(p: &Path) -> Option<(u64, u64)> {
    let mut at = p.to_path_buf();
    while !at.exists() {
        at = at.parent()?.to_path_buf();
    }
    let c = std::ffi::CString::new(at.as_os_str().as_encoded_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a valid C string and a zeroed statvfs to fill.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let f = s.f_frsize as u64;
    Some((s.f_bavail as u64 * f, s.f_blocks as u64 * f))
}

fn disks(state: &AppState) -> Vec<Check> {
    let mut seen: Vec<(u64, u64)> = vec![];
    let mut out = vec![];
    for (key, title, p) in [
        ("disk_files", "Disk: files", state.config.upload_dir.clone()),
        ("disk_backups", "Disk: backups", crate::routes::backups::dir()),
    ] {
        let Some((free, total)) = free_space(&p) else { continue };
        if seen.contains(&(free / (1 << 20), total)) {
            continue; // the same file system
        }
        seen.push((free / (1 << 20), total));
        let pct = if total > 0 { free * 100 / total } else { 0 };
        let level = if pct < 5 || free < 1 << 30 { Level::Fail } else if pct < 15 || free < 5 << 30 { Level::Warn } else { Level::Ok };
        out.push(check(key, title, level, format!("{} free of {} ({pct}%)", bytes(free), bytes(total)), Some("/system/storage")));
    }
    out
}

/// The certificate PUBLIC_URL serves: trusted, for the name, and how long it is valid.
async fn certificate(state: &AppState) -> Check {
    let url = std::env::var("HEALTH_TLS_URL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| state.config.public_url.clone());
    let Ok(u) = reqwest::Url::parse(&url) else { return check("certificate", "HTTPS certificate", Level::Warn, "PUBLIC_URL isn't a valid address", None) };
    let host = u.host_str().unwrap_or_default().to_string();
    if u.scheme() != "https" {
        let local = host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
        return check("certificate", "HTTPS certificate", if local { Level::Off } else { Level::Warn }, if local { "Not checked: PUBLIC_URL is a local address" } else { "PUBLIC_URL isn't HTTPS" }, None);
    }
    let port = u.port_or_known_default().unwrap_or(443);
    match peer_certificate(&host, port, false).await {
        Ok(der) => match not_after(&der) {
            Some(end) => {
                let days = (end - Utc::now()).num_days();
                let level = if days < 7 { Level::Fail } else if days < 21 { Level::Warn } else { Level::Ok };
                check("certificate", "HTTPS certificate", level, format!("Trusted; valid until {} ({days} days)", end.format("%Y-%m-%d")), None)
            }
            None => check("certificate", "HTTPS certificate", Level::Warn, "Trusted, but its expiry couldn't be read", None),
        },
        Err(e) => {
            let end = peer_certificate(&host, port, true).await.ok().and_then(|d| not_after(&d));
            check("certificate", "HTTPS certificate", Level::Fail, format!("{host}: {e}{}", end.map(|t| format!(" (it runs until {})", t.format("%Y-%m-%d"))).unwrap_or_default()), None)
        }
    }
}

async fn peer_certificate(host: &str, port: u16, allow_invalid: bool) -> Result<Vec<u8>, String> {
    use tokio_rustls::rustls::pki_types::ServerName;
    let cfg = crate::crm::email::imap::client_config(allow_invalid)?;
    let name = ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
    let tcp = tokio::time::timeout(std::time::Duration::from_secs(10), tokio::net::TcpStream::connect((host, port)))
        .await
        .map_err(|_| "no answer within 10 seconds".to_string())?
        .map_err(|e| e.to_string())?;
    let tls = tokio::time::timeout(std::time::Duration::from_secs(10), tokio_rustls::TlsConnector::from(std::sync::Arc::new(cfg)).connect(name, tcp))
        .await
        .map_err(|_| "no TLS answer within 10 seconds".to_string())?
        .map_err(|e| e.to_string())?;
    let (_, conn) = tls.get_ref();
    conn.peer_certificates().and_then(|c| c.first()).map(|c| c.to_vec()).ok_or_else(|| "no certificate".into())
}

fn not_after(der: &[u8]) -> Option<DateTime<Utc>> {
    let (_, c) = x509_parser::parse_x509_certificate(der).ok()?;
    DateTime::from_timestamp(c.validity().not_after.timestamp(), 0)
}

async fn jobs(state: &AppState) -> Vec<Check> {
    let failed: Vec<(String, i64)> = sqlx::query_as("SELECT kind, COUNT(*) FROM jobs WHERE status = 'failed' AND updated_at > now() - interval '24 hours' GROUP BY kind ORDER BY 2 DESC")
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let late: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE status = 'pending' AND run_at < now() - interval '10 minutes'").fetch_one(&state.db).await.unwrap_or(0);
    let mut out = vec![];
    let other: Vec<&(String, i64)> = failed.iter().filter(|(k, _)| k != crate::jobs::EMAIL_SEND).collect();
    out.push(if other.is_empty() {
        check("jobs", "Background jobs", if late > 0 { Level::Warn } else { Level::Ok }, if late > 0 { format!("{late} jobs waiting over 10 minutes: is a worker running?") } else { "Nothing failed in the last 24 hours".into() }, Some("/system"))
    } else {
        let n: i64 = other.iter().map(|x| x.1).sum();
        check("jobs", "Background jobs", Level::Warn, format!("{n} failed in the last 24 hours ({}){}", other.iter().take(4).map(|(k, c)| format!("{k} ×{c}")).collect::<Vec<_>>().join(", "), if late > 0 { format!("; {late} waiting over 10 minutes") } else { String::new() }), Some("/system"))
    });
    let (sent, failed_mail): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*) FILTER (WHERE status = 'done'), COUNT(*) FILTER (WHERE status = 'failed') FROM jobs WHERE kind = $1 AND updated_at > now() - interval '24 hours'",
    )
    .bind(crate::jobs::EMAIL_SEND)
    .fetch_one(&state.db)
    .await
    .unwrap_or((0, 0));
    out.push(if failed_mail == 0 {
        check("mail", "Outgoing email", Level::Ok, if sent > 0 { format!("{sent} sent in the last 24 hours") } else { "Nothing failed in the last 24 hours".into() }, Some("/system"))
    } else {
        check("mail", "Outgoing email", if sent == 0 { Level::Fail } else { Level::Warn }, format!("{failed_mail} failed and {sent} sent in the last 24 hours"), Some("/system"))
    });
    out
}

async fn audit_chain(state: &AppState) -> Check {
    let last: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'audit_verify_last'").fetch_optional(&state.db).await.ok().flatten();
    match last {
        None => check("audit", "Audit trail", Level::Warn, "Not verified yet", Some("/audit")),
        Some(v) if v["ok"] != json!(true) => check("audit", "Audit trail", Level::Fail, format!("Doesn't verify: {}", v["problems"][0]["text"].as_str().unwrap_or("records were changed")), Some("/audit")),
        Some(v) => {
            let at = v["checked_at"].as_str().and_then(|s| s.parse::<DateTime<Utc>>().ok());
            let stale = at.is_none_or(|t| (Utc::now() - t).num_hours() > 50);
            check("audit", "Audit trail", if stale { Level::Warn } else { Level::Ok }, format!("{} records verified {}", v["records"], at.map(ago).unwrap_or_default()), Some("/audit"))
        }
    }
}

async fn office(state: &AppState) -> Option<Check> {
    let base = state.config.collabora_url.as_deref()?;
    let url = format!("{}/hosting/discovery", base.trim_end_matches('/'));
    let tls = crate::crm::email::imap::client_config(false).ok()?;
    let r = reqwest::Client::builder().tls_backend_preconfigured(tls).timeout(std::time::Duration::from_secs(10)).build().ok()?.get(&url).send().await;
    Some(match r {
        Ok(r) if r.status().is_success() => check("office", "Office editing", Level::Ok, "The document server answers", None),
        Ok(r) => check("office", "Office editing", Level::Fail, format!("The document server answers {}", r.status()), None),
        Err(e) => check("office", "Office editing", Level::Fail, format!("The document server doesn't answer: {e}"), None),
    })
}

/// Every check (passive: nothing is written).
async fn checks(state: &AppState) -> Vec<Check> {
    let (db, rd, w, enc, cert, jb, au, of) = tokio::join!(database(state), redis_check(state), wal(state), encryption(state), certificate(state), jobs(state), audit_chain(state), office(state));
    let mut out = vec![db, rd];
    out.extend(backups());
    out.push(w);
    out.push(enc);
    out.extend(disks(state));
    out.push(cert);
    out.extend(jb);
    out.push(au);
    out.extend(of);
    out
}

// ---- the self-test ----

async fn self_test(state: &AppState) -> Vec<Check> {
    let mut out = vec![];
    // Database: write, read back, remove.
    let probe = Uuid::new_v4().to_string();
    let db = async {
        sqlx::query("INSERT INTO settings (key, value) VALUES ('health_probe', to_jsonb($1::text)) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value").bind(&probe).execute(&state.db).await?;
        let back: Value = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'health_probe'").fetch_one(&state.db).await?;
        sqlx::query("DELETE FROM settings WHERE key = 'health_probe'").execute(&state.db).await?;
        anyhow::ensure!(back == json!(probe), "read back something else");
        anyhow::Ok(())
    };
    out.push(match db.await {
        Ok(()) => check("test_database", "Test: database write", Level::Ok, "Wrote, read back and removed a value", None),
        Err(e) => check("test_database", "Test: database write", Level::Fail, e.to_string(), None),
    });
    // Redis: the same.
    let mut r = state.redis.clone();
    let rd = async {
        let _: () = redis::cmd("SET").arg("health:probe").arg(&probe).arg("EX").arg(60).query_async(&mut r).await?;
        let back: Option<String> = redis::cmd("GET").arg("health:probe").query_async(&mut r).await?;
        let _: () = redis::cmd("DEL").arg("health:probe").query_async(&mut r).await?;
        anyhow::ensure!(back.as_deref() == Some(probe.as_str()), "read back something else");
        anyhow::Ok(())
    };
    out.push(match rd.await {
        Ok(()) => check("test_redis", "Test: Redis", Level::Ok, "Wrote, read back and removed a value", None),
        Err(e) => check("test_redis", "Test: Redis", Level::Fail, e.to_string(), None),
    });
    // File storage: a file written as uploads are (encrypted when there is a key) and read back.
    let rel = format!(".health/{probe}");
    let fs = async {
        let path = state.config.upload_dir.join(&rel);
        tokio::fs::create_dir_all(path.parent().expect("has a parent")).await?;
        let content = format!("NERP health probe {probe}").repeat(100);
        let mut w = crate::blob::Writer::create(&state.config.file_keys, &path).await?;
        w.write(content.as_bytes()).await?;
        let written = w.finish().await?;
        let back = crate::blob::read(state, &rel, written.key.as_deref()).await;
        let _ = tokio::fs::remove_file(&path).await;
        anyhow::ensure!(back? == content.as_bytes(), "read back something else");
        anyhow::Ok(written.key.is_some())
    };
    out.push(match fs.await {
        Ok(enc) => check("test_files", "Test: file storage", Level::Ok, if enc { "Wrote an encrypted file, read it back and removed it" } else { "Wrote a file, read it back and removed it" }, None),
        Err(e) => check("test_files", "Test: file storage", Level::Fail, e.to_string(), None),
    });
    // Outgoing mail: the server is reached and accepts the sign-in.
    out.push(match tokio::time::timeout(std::time::Duration::from_secs(20), state.mailer.test_connection()).await {
        Ok(Ok(true)) => check("test_mail", "Test: mail server", Level::Ok, format!("{}:{} accepts connections", state.config.smtp.host, state.config.smtp.port), None),
        Ok(Ok(false)) => check("test_mail", "Test: mail server", Level::Fail, format!("{}:{} doesn't accept connections", state.config.smtp.host, state.config.smtp.port), None),
        Ok(Err(e)) => check("test_mail", "Test: mail server", Level::Fail, format!("{}:{}: {e:#}", state.config.smtp.host, state.config.smtp.port), None),
        Err(_) => check("test_mail", "Test: mail server", Level::Fail, format!("{}:{}: no answer within 20 seconds", state.config.smtp.host, state.config.smtp.port), None),
    });
    out
}

fn overall(cs: &[Check]) -> Level {
    cs.iter().map(|c| c.level).max().unwrap_or(Level::Ok).max(Level::Ok)
}

#[derive(Deserialize, Serialize, Default)]
struct Settings {
    #[serde(default = "yes")]
    alerts: bool,
    /// Further addresses for the alerts (an outside mailbox, in case NERP's own is down).
    #[serde(default)]
    extra_emails: Vec<String>,
}
fn yes() -> bool {
    true
}

async fn settings(state: &AppState) -> Settings {
    sqlx::query_scalar::<_, Value>("SELECT value FROM settings WHERE key = 'health_settings'")
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or(Settings { alerts: true, extra_emails: vec![] })
}

async fn store(state: &AppState, key: &str, v: &Value) {
    let _ = sqlx::query("INSERT INTO settings (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()")
        .bind(key)
        .bind(v)
        .execute(&state.db)
        .await;
}

/// Runs the checks, keeps the result and alerts on changes.
pub async fn run(state: &AppState) -> anyhow::Result<Value> {
    let cs = checks(state).await;
    let prev: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'health_last'").fetch_optional(&state.db).await?;
    let result = json!({ "checked_at": Utc::now(), "overall": overall(&cs), "checks": cs });
    store(state, "health_last", &result).await;
    alert(state, prev.as_ref(), &cs).await;
    Ok(result)
}

pub async fn run_job(state: &AppState) -> anyhow::Result<()> {
    run(state).await.map(|_| ())
}

/// Who hears about problems: those who manage the system.
async fn managers(state: &AppState) -> Vec<(Uuid, String)> {
    sqlx::query_as(
        "SELECT DISTINCT u.id, u.email FROM users u JOIN user_roles ur ON ur.user_id = u.id JOIN role_permissions rp ON rp.role_id = ur.role_id
         WHERE rp.permission IN ('*', $1) AND u.is_active AND NOT u.is_guest",
    )
    .bind(perm::SYSTEM_MANAGE)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default()
}

/// Alerts on checks that got worse (to amber or red) since the last run, and once when all
/// is well again.
async fn alert(state: &AppState, prev: Option<&Value>, cs: &[Check]) {
    let before = |key: &str| -> Level {
        prev.and_then(|p| p["checks"].as_array())
            .and_then(|a| a.iter().find(|c| c["key"] == json!(key)))
            .and_then(|c| serde_json::from_value(c["level"].clone()).ok())
            .unwrap_or(Level::Ok)
    };
    let worse: Vec<&Check> = cs.iter().filter(|c| c.level >= Level::Warn && c.level > before(c.key)).collect();
    let was_bad = prev.and_then(|p| serde_json::from_value::<Level>(p["overall"].clone()).ok()).is_some_and(|l| l >= Level::Warn);
    let recovered = was_bad && overall(cs) <= Level::Ok;
    if worse.is_empty() && !recovered {
        return;
    }
    let s = settings(state).await;
    if !s.alerts {
        return;
    }
    let company = crate::routes::auth::company_name(state).await;
    let (subject, heading, lines): (String, String, Vec<String>) = if recovered && worse.is_empty() {
        (format!("{company}: NERP is healthy again"), "All checks pass again".into(), vec!["Every check on System → Health is green.".into()])
    } else {
        let red = worse.iter().any(|c| c.level == Level::Fail);
        (
            format!("{company}: NERP {} — {}", if red { "needs attention" } else { "warning" }, worse.iter().map(|c| c.title).collect::<Vec<_>>().join(", ")),
            if red { "Something needs attention".into() } else { "A check turned amber".into() },
            worse.iter().map(|c| format!("{} {}: {}", if c.level == Level::Fail { "✖" } else { "▲" }, c.title, c.text)).collect(),
        )
    };
    let people = managers(state).await;
    let link = format!("{}/system/health", state.config.public_url);
    let mut to: Vec<String> = people.iter().map(|p| p.1.clone()).collect();
    to.extend(s.extra_emails.iter().cloned());
    to.sort();
    to.dedup();
    for addr in to {
        let email = crate::mail::notice(&addr, &company, &subject, &heading, &lines, Some(("Open System → Health", &link)), "You get this because you manage NERP's system settings.");
        if let Err(e) = crate::jobs::send_email(state, email).await {
            tracing::warn!(error = ?e, "health alert email");
        }
    }
    let ids: Vec<Uuid> = people.iter().map(|p| p.0).collect();
    crate::notify::notify(
        state,
        &ids,
        crate::notify::NewNotification { kind: "system.health", title: subject, body: Some(lines.join("\n")), link: Some("/system/health".into()), actor: None },
    )
    .await;
}

// ---- routes ----

async fn show(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    let last: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'health_last'").fetch_optional(&state.db).await?;
    let last = match last {
        Some(v) => v,
        None => run(&state).await.map_err(AppError::Internal)?,
    };
    let test: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'health_test_last'").fetch_optional(&state.db).await?;
    Ok(Json(json!({ "last": last, "self_test": test, "settings": settings(&state).await, "every_minutes": 15 })))
}

/// Runs every check and the self-test now.
async fn run_now(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    let (last, tests) = tokio::join!(run(&state), self_test(&state));
    let last = last.map_err(AppError::Internal)?;
    let test = json!({ "checked_at": Utc::now(), "overall": overall(&tests), "checks": tests });
    store(&state, "health_test_last", &test).await;
    audit::record(&state.db, Some(auth.id), "system.health_test", "system", None, json!({ "overall": last["overall"], "self_test": test["overall"] }), ip.as_deref()).await;
    Ok(Json(json!({ "last": last, "self_test": test, "settings": settings(&state).await, "every_minutes": 15 })))
}

async fn save_settings(State(state): State<AppState>, auth: AuthUser, Json(mut s): Json<Settings>) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    let mut emails = vec![];
    for e in s.extra_emails.iter().map(|e| e.trim()).filter(|e| !e.is_empty()) {
        emails.push(crate::routes::normalize_email(e)?);
    }
    emails.dedup();
    if emails.len() > 10 {
        return Err(AppError::bad("at most 10 further addresses"));
    }
    s.extra_emails = emails;
    store(&state, "health_settings", &serde_json::to_value(&s).unwrap_or_default()).await;
    Ok(Json(serde_json::to_value(&s).unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_order() {
        assert!(Level::Fail > Level::Warn && Level::Warn > Level::Ok && Level::Ok > Level::Off);
        assert_eq!(overall(&[check("a", "A", Level::Off, "", None)]), Level::Ok);
        assert_eq!(overall(&[check("a", "A", Level::Ok, "", None), check("b", "B", Level::Warn, "", None)]), Level::Warn);
    }

    #[test]
    fn sizes() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(5 << 30), "5.0 GB");
    }
}
