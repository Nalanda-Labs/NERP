//! Storage: how much the uploaded files take, where (drive, its old versions and trash, chats,
//! the rest), of what kind and whose; the largest files; free disk space. Cleaning up: the
//! drive's trash at once (it empties itself after 30 days anyway), and old versions of drive
//! files beyond the last few.

use axum::{extract::State, routing::{get, post}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    audit,
    auth::{permissions as perm, AuthUser, ClientIp},
    error::{AppError, AppResult},
    routes::files::delete_stored,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/admin/storage", get(overview)).route("/admin/storage/cleanup", post(cleanup))
}

/// Total and free bytes of the disk holding the uploads.
fn disk(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a valid NUL-terminated path and a zeroed struct for the call to fill.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let block = s.f_frsize as u64;
    Some((s.f_blocks as u64 * block, s.f_bavail as u64 * block))
}

const AREAS: &str = "WITH cur AS (SELECT file_id FROM drive_nodes WHERE file_id IS NOT NULL AND deleted_at IS NULL),
    trash AS (SELECT v.file_id FROM drive_versions v JOIN drive_nodes n ON n.id = v.node_id WHERE n.deleted_at IS NOT NULL AND v.file_id IS NOT NULL),
    old AS (SELECT v.file_id FROM drive_versions v JOIN drive_nodes n ON n.id = v.node_id WHERE n.deleted_at IS NULL AND v.file_id IS NOT NULL AND v.file_id IS DISTINCT FROM n.file_id),
    chat AS (SELECT DISTINCT unnest(file_ids) AS file_id FROM im_messages WHERE cardinality(file_ids) > 0)
  SELECT CASE WHEN f.id IN (SELECT file_id FROM cur) THEN 'drive'
              WHEN f.id IN (SELECT file_id FROM old) THEN 'drive_versions'
              WHEN f.id IN (SELECT file_id FROM trash) THEN 'drive_trash'
              WHEN f.id IN (SELECT file_id FROM chat) THEN 'chats'
              ELSE 'other' END AS area, COUNT(*), COALESCE(SUM(f.size_bytes), 0)::bigint
  FROM files f GROUP BY 1 ORDER BY 3 DESC";

const KINDS: &str = "SELECT CASE WHEN mime_type LIKE 'image/%' THEN 'images' WHEN mime_type LIKE 'video/%' THEN 'video' WHEN mime_type LIKE 'audio/%' THEN 'audio'
      WHEN mime_type = 'application/pdf' THEN 'pdf'
      WHEN mime_type LIKE 'application/vnd.openxmlformats%' OR mime_type LIKE 'application/vnd.oasis%' OR mime_type LIKE 'application/msword%' OR mime_type LIKE 'application/vnd.ms-%' THEN 'documents'
      WHEN mime_type IN ('application/zip', 'application/x-7z-compressed', 'application/x-tar', 'application/gzip', 'application/x-rar-compressed') THEN 'archives'
      ELSE 'other' END, COUNT(*), COALESCE(SUM(size_bytes), 0)::bigint FROM files GROUP BY 1 ORDER BY 3 DESC";

async fn overview(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::FILES_MANAGE)?;
    let (count, bytes): (i64, i64) = sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(size_bytes), 0)::bigint FROM files").fetch_one(&state.db).await?;
    let areas: Vec<(String, i64, i64)> = sqlx::query_as(AREAS).fetch_all(&state.db).await?;
    let kinds: Vec<(String, i64, i64)> = sqlx::query_as(KINDS).fetch_all(&state.db).await?;
    let people: Vec<(Option<Uuid>, Option<String>, i64, i64)> = sqlx::query_as(
        "SELECT f.owner_id, trim(u.first_name || ' ' || u.last_name), COUNT(*), COALESCE(SUM(f.size_bytes), 0)::bigint
         FROM files f LEFT JOIN users u ON u.id = f.owner_id GROUP BY 1, 2 ORDER BY 4 DESC LIMIT 15",
    )
    .fetch_all(&state.db)
    .await?;
    let largest: Vec<(Uuid, String, i64, Option<String>, chrono::DateTime<chrono::Utc>, String)> = sqlx::query_as(
        "SELECT f.id, f.original_name, f.size_bytes, trim(u.first_name || ' ' || u.last_name), f.created_at, f.mime_type
         FROM files f LEFT JOIN users u ON u.id = f.owner_id ORDER BY f.size_bytes DESC LIMIT 20",
    )
    .fetch_all(&state.db)
    .await?;
    let old_versions: (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(v.size_bytes), 0)::bigint FROM drive_versions v JOIN drive_nodes n ON n.id = v.node_id
         WHERE n.deleted_at IS NULL AND v.file_id IS NOT NULL AND v.file_id IS DISTINCT FROM n.file_id",
    )
    .fetch_one(&state.db)
    .await?;
    let trash: (i64, i64) = sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(size_bytes), 0)::bigint FROM drive_nodes WHERE deleted_at IS NOT NULL AND kind = 'file'").fetch_one(&state.db).await?;
    let pair = |r: Vec<(String, i64, i64)>| r.into_iter().map(|(k, n, b)| json!({ "key": k, "files": n, "bytes": b })).collect::<Vec<_>>();
    Ok(Json(json!({
        "files": count, "bytes": bytes,
        "disk": disk(&state.config.upload_dir).map(|(t, f)| json!({ "total": t, "free": f })),
        "areas": pair(areas), "kinds": pair(kinds),
        "people": people.into_iter().map(|(id, n, c, b)| json!({ "id": id, "name": n, "files": c, "bytes": b })).collect::<Vec<_>>(),
        "largest": largest.into_iter().map(|(id, n, b, o, at, m)| json!({ "id": id, "name": n, "bytes": b, "owner": o, "created_at": at, "mime_type": m })).collect::<Vec<_>>(),
        "cleanup": { "old_versions": { "count": old_versions.0, "bytes": old_versions.1 }, "drive_trash": { "count": trash.0, "bytes": trash.1 } },
    })))
}

#[derive(Deserialize)]
struct CleanupInput {
    /// drive_trash | old_versions
    what: String,
    /// Old versions: how many of each file's latest to keep (besides the current one).
    keep: Option<i32>,
    /// Only what is older than this.
    older_than_days: Option<i32>,
}

async fn cleanup(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Json(b): Json<CleanupInput>) -> AppResult<Json<Value>> {
    auth.require(perm::FILES_MANAGE)?;
    let days = b.older_than_days.unwrap_or(0);
    if !(0..=3650).contains(&days) {
        return Err(AppError::bad("older than 0 to 3650 days"));
    }
    let (removed, bytes) = match b.what.as_str() {
        "drive_trash" => {
            // The topmost items in the trash (their contents go with them).
            let ids: Vec<(Uuid, i64)> = sqlx::query_as(
                "SELECT n.id, (WITH RECURSIVE down AS (SELECT n.id AS id UNION ALL SELECT d.id FROM drive_nodes d JOIN down ON d.parent_id = down.id)
                    SELECT COALESCE(SUM(v.size_bytes), 0)::bigint FROM drive_versions v WHERE v.node_id IN (SELECT id FROM down))
                 FROM drive_nodes n WHERE n.deleted_at < now() - make_interval(days => $1)
                   AND NOT EXISTS (SELECT 1 FROM drive_nodes p WHERE p.id = n.parent_id AND p.deleted_at IS NOT NULL) LIMIT 5000",
            )
            .bind(days)
            .fetch_all(&state.db)
            .await?;
            let bytes = ids.iter().map(|(_, b)| b).sum::<i64>();
            for (id, _) in &ids {
                crate::drive::purge_node(&state, *id).await?;
            }
            (ids.len() as i64, bytes)
        }
        "old_versions" => {
            let keep = b.keep.unwrap_or(3);
            if !(0..=100).contains(&keep) {
                return Err(AppError::bad("keep 0 to 100 earlier versions"));
            }
            let gone: Vec<(Option<Uuid>, i64)> = sqlx::query_as(
                "WITH ranked AS (
                    SELECT v.node_id, v.version, v.file_id, v.size_bytes, v.uploaded_at, n.file_id AS current,
                           row_number() OVER (PARTITION BY v.node_id ORDER BY v.version DESC) AS r
                    FROM drive_versions v JOIN drive_nodes n ON n.id = v.node_id WHERE n.deleted_at IS NULL)
                 DELETE FROM drive_versions d USING ranked x
                 WHERE d.node_id = x.node_id AND d.version = x.version AND x.r > $1 + 1 AND x.file_id IS DISTINCT FROM x.current
                   AND x.uploaded_at < now() - make_interval(days => $2)
                 RETURNING d.file_id, d.size_bytes",
            )
            .bind(keep)
            .bind(days)
            .fetch_all(&state.db)
            .await?;
            for f in gone.iter().filter_map(|(f, _)| *f) {
                delete_stored(&state, f).await;
            }
            (gone.len() as i64, gone.iter().map(|(_, b)| b).sum::<i64>())
        }
        _ => return Err(AppError::bad("clean up the drive's trash or old versions")),
    };
    audit::record(&state.db, Some(auth.id), "storage.cleanup", "storage", None, json!({ "what": b.what, "removed": removed, "bytes": bytes }), ip.as_deref()).await;
    Ok(Json(json!({ "removed": removed, "bytes": bytes })))
}
