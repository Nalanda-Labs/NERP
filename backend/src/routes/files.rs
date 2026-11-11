//! File storage on local disk with metadata in the database.
//! Files are written under UPLOAD_DIR/YYYY/MM/<uuid> and never by their original name.

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use std::path::PathBuf;
use uuid::Uuid;

use super::{Page, PageParams};
use crate::{
    audit,
    auth::{permissions as perm, AuthUser, ClientIp, MaybeAuthUser},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router(state: &AppState) -> Router<AppState> {
    Router::new()
        .route("/files", get(list).post(upload))
        .route("/files/{id}", get(meta).delete(remove))
        .route("/files/{id}/content", get(content))
        .route("/download-links", axum::routing::post(download_link))
        // Leave headroom for multipart framing on top of the file itself.
        .layer(DefaultBodyLimit::max(state.config.max_upload_bytes + 64 * 1024))
}

#[derive(Serialize, FromRow)]
pub struct FileMeta {
    pub(crate) id: Uuid,
    pub(crate) owner_id: Option<Uuid>,
    pub(crate) original_name: String,
    pub(crate) mime_type: String,
    pub(crate) size_bytes: i64,
    sha256: String,
    pub(crate) is_public: bool,
    created_at: DateTime<Utc>,
    #[serde(skip)]
    pub(crate) storage_path: String,
    #[serde(skip)]
    pub(crate) enc_key: Option<String>,
}

const META_COLUMNS: &str =
    "id, owner_id, original_name, mime_type, size_bytes, sha256, is_public, created_at, storage_path, enc_key";

pub(crate) async fn fetch(state: &AppState, id: Uuid) -> AppResult<FileMeta> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {META_COLUMNS} FROM files WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("file"))
}

fn can_read_direct(file: &FileMeta, auth: &AuthUser) -> bool {
    file.is_public || file.owner_id == Some(auth.id) || auth.can(perm::FILES_MANAGE)
}

/// Owner, public, files.manage, or attached to something the user can see (a task, a CRM
/// document or email, a chat message).
pub(crate) async fn can_read(state: &AppState, file: &FileMeta, auth: &AuthUser) -> AppResult<bool> {
    if can_read_direct(file, auth) {
        return Ok(true);
    }
    if crate::tasks::access::can_view_task_file(state, auth, file.id).await? {
        return Ok(true);
    }
    if crate::crm::docs::can_view_file(state, auth, file.id).await? {
        return Ok(true);
    }
    if crate::crm::email::can_view_file(state, auth, file.id).await? {
        return Ok(true);
    }
    if crate::im::can_view_file(state, auth, file.id).await? {
        return Ok(true);
    }
    if crate::feed::can_view_file(state, auth, file.id).await? {
        return Ok(true);
    }
    if crate::lists::can_view_file(state, auth, file.id).await? {
        return Ok(true);
    }
    if crate::accounting::can_view_file(state, auth, file.id).await? {
        return Ok(true);
    }
    if crate::training::can_view_file(state, auth, file.id).await? {
        return Ok(true);
    }
    if crate::support::can_view_file(state, auth, file.id).await? {
        return Ok(true);
    }
    if crate::meetings::can_view_file(state, auth, file.id).await? {
        return Ok(true);
    }
    if crate::kb::can_view_file(state, auth, file.id).await? {
        return Ok(true);
    }
    crate::drive::can_view_file(state, auth, file.id).await
}

/// Stores bytes produced on the server (generated PDFs, built-in templates) as a file.
pub async fn store_bytes(state: &AppState, owner: Option<Uuid>, name: &str, mime: &str, bytes: &[u8]) -> AppResult<Uuid> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    let rel = PathBuf::from(format!("{:04}/{:02}/{id}", now.year(), now.month()));
    let abs = state.config.upload_dir.join(&rel);
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = abs.with_extension("part");
    let mut w = crate::blob::Writer::create(&state.config.file_keys, &tmp).await?;
    w.write(bytes).await?;
    let done = w.finish().await?;
    tokio::fs::rename(&tmp, &abs).await?;
    let insert = sqlx::query(
        "INSERT INTO files (id, owner_id, original_name, mime_type, size_bytes, sha256, storage_path, is_public, enc_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, FALSE, $8)",
    )
    .bind(id)
    .bind(owner)
    .bind(sanitize_name(name))
    .bind(sanitize_mime(Some(mime)))
    .bind(bytes.len() as i64)
    .bind(done.sha256)
    .bind(rel.to_string_lossy().as_ref())
    .bind(done.key)
    .execute(&state.db)
    .await;
    if let Err(e) = insert {
        let _ = tokio::fs::remove_file(&abs).await;
        return Err(e.into());
    }
    Ok(id)
}

/// Stores a file from disk (e.g. a call recording) without reading it into memory at once.
/// The source is left in place.
pub async fn store_path(state: &AppState, owner: Option<Uuid>, name: &str, mime: &str, src: &std::path::Path) -> AppResult<Uuid> {
    use tokio::io::AsyncReadExt;
    let id = Uuid::new_v4();
    let now = Utc::now();
    let rel = PathBuf::from(format!("{:04}/{:02}/{id}", now.year(), now.month()));
    let abs = state.config.upload_dir.join(&rel);
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = abs.with_extension("part");
    let mut input = tokio::fs::File::open(src).await?;
    let mut out = crate::blob::Writer::create(&state.config.file_keys, &tmp).await?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = input.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        out.write(&buf[..n]).await?;
    }
    let done = out.finish().await?;
    tokio::fs::rename(&tmp, &abs).await?;
    let insert = sqlx::query(
        "INSERT INTO files (id, owner_id, original_name, mime_type, size_bytes, sha256, storage_path, is_public, enc_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, FALSE, $8)",
    )
    .bind(id)
    .bind(owner)
    .bind(sanitize_name(name))
    .bind(sanitize_mime(Some(mime)))
    .bind(done.size as i64)
    .bind(done.sha256)
    .bind(rel.to_string_lossy().as_ref())
    .bind(done.key)
    .execute(&state.db)
    .await;
    if let Err(e) = insert {
        let _ = tokio::fs::remove_file(&abs).await;
        return Err(e.into());
    }
    Ok(id)
}

/// Reads a stored file: (original name, mime type, owner, bytes).
pub async fn read_bytes(state: &AppState, id: Uuid) -> AppResult<(String, String, Option<Uuid>, Vec<u8>)> {
    let f = fetch(state, id).await?;
    let bytes = read_content(state, &f).await?;
    Ok((f.original_name, f.mime_type, f.owner_id, bytes))
}

/// A stored file's content (decrypted).
pub async fn read_content(state: &AppState, f: &FileMeta) -> AppResult<Vec<u8>> {
    crate::blob::read(state, &f.storage_path, f.enc_key.as_deref()).await.map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => AppError::NotFound("file"),
        _ => AppError::Internal(e.into()),
    })
}

/// A plain copy of a stored file at `dest`, for tools that need a path (ffmpeg).
pub async fn copy_out(state: &AppState, f: &FileMeta, dest: &std::path::Path) -> std::io::Result<()> {
    crate::blob::open(&state.config.file_keys, &state.config.upload_dir.join(&f.storage_path), f.enc_key.as_deref()).await?.copy_to(dest).await
}

/// Deletes a server-managed file (row and disk). Missing files are ignored.
pub async fn delete_stored(state: &AppState, id: Uuid) {
    let path: Option<String> = sqlx::query_scalar("DELETE FROM files WHERE id = $1 RETURNING storage_path")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    if let Some(p) = path {
        crate::blob::discard(state, &p).await;
    }
}

/// Strips path components and control characters from a client-supplied filename.
fn sanitize_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base.chars().filter(|c| !c.is_control()).take(255).collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." { "file".into() } else { cleaned }
}

fn sanitize_mime(mime: Option<&str>) -> String {
    let m = mime.unwrap_or("application/octet-stream").trim().to_lowercase();
    let valid = m.len() <= 127
        && m.split_once('/').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
        && m.chars().all(|c| c.is_ascii_alphanumeric() || "/.+-_".contains(c));
    if valid { m } else { "application/octet-stream".into() }
}

#[derive(Deserialize)]
pub struct ListQuery {
    /// With files.manage, list everyone's files instead of only your own.
    all: Option<String>,
    #[serde(flatten)]
    page: PageParams,
}

async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Page<FileMeta>>> {
    let all = q.all.as_deref() == Some("true");
    if all {
        auth.require(perm::FILES_MANAGE)?;
    }
    let owner = if all { None } else { Some(auth.id) };
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE ($1::uuid IS NULL OR owner_id = $1)")
        .bind(owner)
        .fetch_one(&state.db)
        .await?;
    let items: Vec<FileMeta> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {META_COLUMNS} FROM files WHERE ($1::uuid IS NULL OR owner_id = $1)
         ORDER BY created_at DESC LIMIT $2 OFFSET $3"
    )))
    .bind(owner)
    .bind(q.page.limit())
    .bind(q.page.offset())
    .fetch_all(&state.db)
    .await?;
    Ok(Json(Page { items, total, limit: q.page.limit(), offset: q.page.offset() }))
}

/// Multipart upload: a `file` part, plus an optional `public` part ("true" makes the
/// file readable by anyone who knows its id — used for avatars and logos).
async fn upload(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    mut multipart: Multipart,
) -> AppResult<Json<FileMeta>> {
    let mut is_public = false;
    let mut stored: Option<(Uuid, String, String, i64, String, PathBuf, Option<String>)> = None;

    while let Some(mut field) = multipart.next_field().await.map_err(|e| multipart_error(&state, e))? {
        match field.name() {
            Some("public") => {
                let v = field.text().await.map_err(|e| AppError::bad(e.to_string()))?;
                is_public = v.trim() == "true";
            }
            Some("file") if stored.is_none() => {
                let id = Uuid::new_v4();
                let name = sanitize_name(field.file_name().unwrap_or("file"));
                let mime = sanitize_mime(field.content_type());
                let now = Utc::now();
                let rel = PathBuf::from(format!("{:04}/{:02}/{id}", now.year(), now.month()));
                let abs = state.config.upload_dir.join(&rel);
                if let Some(parent) = abs.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }

                let tmp = abs.with_extension("part");
                let mut out = crate::blob::Writer::create(&state.config.file_keys, &tmp).await?;
                let mut size: usize = 0;
                let result: AppResult<crate::blob::Written> = async {
                    while let Some(chunk) = field.chunk().await.map_err(|e| multipart_error(&state, e))? {
                        size += chunk.len();
                        if size > state.config.max_upload_bytes {
                            return Err(AppError::PayloadTooLarge(format!(
                                "files may be at most {} MB",
                                state.config.max_upload_bytes / (1024 * 1024)
                            )));
                        }
                        out.write(&chunk).await?;
                    }
                    Ok(out.finish().await?)
                }
                .await;
                let done = match result {
                    Ok(d) => d,
                    Err(e) => {
                        let _ = tokio::fs::remove_file(&tmp).await;
                        return Err(e);
                    }
                };
                tokio::fs::rename(&tmp, &abs).await?;
                stored = Some((id, name, mime, size as i64, done.sha256, rel, done.key));
            }
            _ => {}
        }
    }

    let (id, name, mime, size, digest, rel, enc_key) =
        stored.ok_or_else(|| AppError::bad("no file was uploaded (expected a 'file' field)"))?;
    let insert = sqlx::query(
        "INSERT INTO files (id, owner_id, original_name, mime_type, size_bytes, sha256, storage_path, is_public, enc_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(auth.id)
    .bind(&name)
    .bind(&mime)
    .bind(size)
    .bind(&digest)
    .bind(rel.to_string_lossy().as_ref())
    .bind(is_public)
    .bind(&enc_key)
    .execute(&state.db)
    .await;
    if let Err(e) = insert {
        let _ = tokio::fs::remove_file(state.config.upload_dir.join(&rel)).await;
        return Err(e.into());
    }
    audit::record(&state.db, Some(auth.id), "file.upload", "file", Some(id.to_string()), json!({ "name": name, "size": size }), ip.as_deref()).await;
    // Videos get a copy every browser plays (in the background).
    if crate::video::is_video(&mime, &name) {
        crate::video::queue(&state, id).await;
    }
    Ok(Json(fetch(&state, id).await?))
}

async fn meta(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<FileMeta>> {
    let file = fetch(&state, id).await?;
    if !can_read(&state, &file, &auth).await? {
        return Err(AppError::NotFound("file"));
    }
    Ok(Json(file))
}

/// Streams the file. Public files need no token so they can be used in `<img src>`.
async fn content(
    State(state): State<AppState>,
    MaybeAuthUser(auth): MaybeAuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<NameQuery>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let file = fetch(&state, id).await?;
    let allowed = file.is_public
        || match &auth {
            Some(a) => can_read(&state, &file, a).await?,
            None => false,
        };
    if !allowed {
        return Err(if auth.is_none() { AppError::Unauthorized } else { AppError::NotFound("file") });
    }

    let name = q.name(&file.original_name);
    stream(&state, &file, &name, &headers).await
}

/// `?name=` saves a download under another name (a link's own `download` name loses to the
/// server's, so the page passes it here). Only the suggested file name changes.
#[derive(Deserialize, Default)]
pub struct NameQuery {
    pub name: Option<String>,
}

impl NameQuery {
    pub fn name(&self, default: &str) -> String {
        let clean: String = self.name.as_deref().unwrap_or("").chars().filter(|c| !c.is_control() && !matches!(c, '/' | '\\')).take(255).collect();
        let clean = clean.trim();
        if clean.is_empty() || clean.chars().all(|c| c == '.') { default.to_string() } else { clean.to_string() }
    }
}

#[derive(Deserialize)]
struct LinkBody {
    /// What to download, e.g. /api/files/{id}/content (with any query string).
    path: String,
}

/// A link the browser can download (or play) a private file from directly, without holding it
/// in memory first: the file or drive download path with a short-lived `dl` token added.
async fn download_link(State(state): State<AppState>, auth: AuthUser, Json(b): Json<LinkBody>) -> AppResult<Json<serde_json::Value>> {
    let (path, query) = b.path.split_once('?').unwrap_or((b.path.as_str(), ""));
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let id_ok = |s: &str| Uuid::parse_str(s).is_ok();
    let allowed = matches!(parts.as_slice(), ["api", "files", id, "content" | "video" | "poster"] if id_ok(id))
        || matches!(parts.as_slice(), ["api", "drive", "nodes", id, "download"] if id_ok(id));
    if !allowed || query.split('&').any(|kv| kv.starts_with("dl=")) {
        return Err(AppError::bad("download links are for file downloads"));
    }
    let token = crate::auth::make_download_link(&state, &auth, path).await?;
    let sep = if query.is_empty() { "?" } else { "&" };
    let full = if query.is_empty() { path.to_string() } else { format!("{path}?{query}") };
    Ok(Json(json!({ "url": format!("{full}{sep}dl={token}"), "expires_in": crate::auth::DOWNLOAD_LINK_SECS })))
}

/// A byte range asked for: `bytes=a-b`, `bytes=a-` or `bytes=-n` (one range). Ok(None): the
/// whole file; Err: a range outside it.
fn range(headers: &HeaderMap, size: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(v) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) else { return Ok(None) };
    let Some(spec) = v.trim().strip_prefix("bytes=") else { return Ok(None) };
    if spec.contains(',') {
        // Several ranges: answer with the whole file (allowed by the HTTP rules).
        return Ok(None);
    }
    let (a, b) = spec.split_once('-').ok_or(())?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: u64 = n.parse().map_err(|_| ())?;
            if n == 0 {
                return Err(());
            }
            (size.saturating_sub(n), size.saturating_sub(1))
        }
        (a, "") => (a.parse().map_err(|_| ())?, size.saturating_sub(1)),
        (a, b) => (a.parse().map_err(|_| ())?, b.parse::<u64>().map_err(|_| ())?.min(size.saturating_sub(1))),
    };
    if size == 0 || start > end || start >= size {
        return Err(());
    }
    Ok(Some((start, end)))
}

/// Sends a stored file from disk as it is read (never all in memory), with byte ranges so
/// videos can be played and seeked, and the download headers.
pub async fn stream(state: &AppState, file: &FileMeta, name: &str, headers: &HeaderMap) -> AppResult<Response> {
    let blob = crate::blob::open(&state.config.file_keys, &state.config.upload_dir.join(&file.storage_path), file.enc_key.as_deref()).await.map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => AppError::NotFound("file"),
        _ => AppError::Internal(e.into()),
    })?;
    send_blob(blob, &file.mime_type, file.is_public, name, headers)
}

/// The same for a file elsewhere on disk (a backup), always as a private download.
pub async fn stream_disk(path: &std::path::Path, name: &str, mime: &str, headers: &HeaderMap) -> AppResult<Response> {
    let blob = crate::blob::open(&crate::blob::Keys::default(), path, None).await.map_err(|_| AppError::NotFound("file"))?;
    send_blob(blob, mime, false, name, headers)
}

fn send_blob(blob: crate::blob::Blob, mime: &str, public: bool, name: &str, headers: &HeaderMap) -> AppResult<Response> {
    let size = blob.size;

    // Raster images, PDFs, audio and video may show in the page; everything else downloads.
    // SVG/HTML never render inline because they can carry scripts.
    let inline = matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif" | "application/pdf"
            | "video/mp4" | "video/webm" | "audio/mpeg" | "audio/ogg" | "audio/webm" | "audio/mp4"
    );
    let ascii_name: String = name
        .chars()
        .map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' })
        .collect();
    let disposition = format!(
        "{}; filename=\"{}\"; filename*=UTF-8''{}",
        if inline { "inline" } else { "attachment" },
        ascii_name,
        percent_encode(name)
    );

    let (status, start, len, content_range) = match range(headers, size) {
        Ok(None) => (StatusCode::OK, 0, size, None),
        Ok(Some((start, end))) => (StatusCode::PARTIAL_CONTENT, start, end - start + 1, Some(format!("bytes {start}-{end}/{size}"))),
        Err(()) => {
            let mut res = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            res.headers_mut().insert(header::CONTENT_RANGE, HeaderValue::from_str(&format!("bytes */{size}")).unwrap());
            return Ok(res);
        }
    };
    let mut res = Body::from_stream(blob.stream(start, len)).into_response();
    *res.status_mut() = status;
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(mime).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Some(cr) = content_range {
        h.insert(header::CONTENT_RANGE, HeaderValue::from_str(&cr).unwrap());
    }
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&disposition).unwrap_or(HeaderValue::from_static("attachment")));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox; default-src 'none'"));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if public { "public, max-age=86400, immutable" } else { "private, max-age=0" }),
    );
    Ok(res)
}

/// Streams a stored file by id under a given name (for downloads through other modules).
pub async fn stream_id(state: &AppState, id: Uuid, name: &str, headers: &HeaderMap) -> AppResult<Response> {
    let file = fetch(state, id).await?;
    stream(state, &file, name, headers).await
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

async fn remove(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let file = fetch(&state, id).await?;
    if file.owner_id != Some(auth.id) {
        if !can_read_direct(&file, &auth) {
            return Err(AppError::NotFound("file"));
        }
        auth.require(perm::FILES_MANAGE)?;
    }
    sqlx::query("DELETE FROM files WHERE id = $1").bind(id).execute(&state.db).await?;
    crate::blob::discard(&state, &file.storage_path).await;
    audit::record(&state.db, Some(auth.id), "file.delete", "file", Some(id.to_string()), json!({ "name": file.original_name }), ip.as_deref()).await;
    Ok(Json(json!({ "ok": true })))
}

/// A body cut off by the size limit is "too large", whichever read noticed it first.
fn multipart_error(state: &AppState, e: axum::extract::multipart::MultipartError) -> AppError {
    if e.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
        AppError::PayloadTooLarge(format!("files may be at most {} MB", state.config.max_upload_bytes / (1024 * 1024)))
    } else {
        AppError::bad(e.to_string())
    }
}

/// Whether a person may read a stored file (for other modules).
pub async fn may_read(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<bool> {
    match fetch(state, id).await {
        Ok(f) => can_read(state, &f, auth).await,
        Err(AppError::NotFound(_)) => Ok(false),
        Err(e) => Err(e),
    }
}
