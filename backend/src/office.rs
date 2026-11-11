//! Office documents in the browser.
//!
//! - **Previews**: Word, Excel, PowerPoint and OpenDocument files are turned into a PDF once
//!   (with the LibreOffice NERP already uses for documents) and shown in the browser.
//! - **Editing** with Collabora Online (`COLLABORA_URL`): NERP is the "WOPI host" Collabora
//!   reads files from and saves them to. Opening a drive file for editing gives the browser a
//!   short-lived, signed token for that person and file; Collabora calls back with it
//!   (CheckFileInfo, GetFile, PutFile). Every save is a new version of the drive file. Several
//!   people may edit the same file at once (Collabora joins them in one session).

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use hmac::{KeyInit, Mac};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

/// What LibreOffice can turn into a PDF preview, by extension.
const PREVIEWABLE: &[&str] = &["doc", "docx", "odt", "rtf", "xls", "xlsx", "ods", "ppt", "pptx", "odp", "odg", "csv", "txt", "wpd", "pages", "key", "numbers"];
/// Signed tokens last ten hours (a working day in the editor).
const TOKEN_SECS: i64 = 10 * 3600;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/files/{id}/preview", get(preview))
        .route("/drive/nodes/{id}/preview", get(drive_preview))
        .route("/office/status", get(status))
        .route("/drive/nodes/{id}/office", post(open))
        .route("/wopi/files/{id}", get(check_file_info).post(lock_ops))
        .route("/wopi/files/{id}/contents", get(get_file).post(put_file))
        .layer(axum::extract::DefaultBodyLimit::max(200 * 1024 * 1024))
}

fn ext_of(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Previews
// ---------------------------------------------------------------------------

/// The PDF made from an office file (made the first time it's asked for).
pub async fn preview_pdf(state: &AppState, file: Uuid) -> AppResult<Uuid> {
    if let Some(pdf) = sqlx::query_scalar::<_, Uuid>("SELECT pdf_id FROM file_previews WHERE file_id = $1").bind(file).fetch_optional(&state.db).await? {
        return Ok(pdf);
    }
    let (name, _mime, _, bytes) = crate::routes::files::read_bytes(state, file).await?;
    let ext = ext_of(&name);
    if !PREVIEWABLE.contains(&ext.as_str()) {
        return Err(AppError::bad("this kind of file has no preview"));
    }
    let c = &state.config.docs;
    let pdf = crate::crm::docs::office::convert(&c.soffice_bin, &bytes, &ext, "pdf", c.timeout_secs).await.map_err(|e| AppError::bad(format!("the preview could not be made: {e}")))?;
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&name);
    let id = crate::routes::files::store_bytes(state, None, &format!("{stem}.pdf"), "application/pdf", &pdf).await?;
    sqlx::query("INSERT INTO file_previews (file_id, pdf_id) VALUES ($1, $2) ON CONFLICT (file_id) DO NOTHING").bind(file).bind(id).execute(&state.db).await?;
    // Two at once: keep the one stored first.
    Ok(sqlx::query_scalar("SELECT pdf_id FROM file_previews WHERE file_id = $1").bind(file).fetch_one(&state.db).await?)
}

/// A PDF preview of a file the person may read.
async fn preview(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Response> {
    if !crate::routes::files::may_read(&state, &auth, id).await? {
        return Err(AppError::NotFound("file"));
    }
    serve_preview(&state, id).await
}

/// The preview of a drive item's current version.
async fn drive_preview(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Response> {
    let f = crate::drive::office_file(&state, &auth, id).await?;
    serve_preview(&state, f.file_id).await
}

async fn serve_preview(state: &AppState, id: Uuid) -> AppResult<Response> {
    let state = state.clone();
    let pdf = preview_pdf(&state, id).await?;
    let (_, _, _, bytes) = crate::routes::files::read_bytes(&state, pdf).await?;
    let mut r = bytes.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("inline; filename=\"preview.pdf\""));
    h.insert("X-Content-Type-Options", HeaderValue::from_static("nosniff"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    Ok(r)
}

// ---------------------------------------------------------------------------
// Collabora: discovery and opening
// ---------------------------------------------------------------------------

/// Collabora's editors by extension: (action, URL) from /hosting/discovery (kept an hour).
async fn discovery(state: &AppState) -> AppResult<Vec<(String, String, String)>> {
    let base = state.config.collabora_url.as_deref().ok_or_else(|| AppError::bad("editing office files isn't set up on this server (COLLABORA_URL)"))?;
    let mut redis = state.redis.clone();
    let key = "office:discovery";
    if let Ok(Some(cached)) = redis.get::<_, Option<String>>(key).await {
        if let Ok(v) = serde_json::from_str(&cached) {
            return Ok(v);
        }
    }
    // NERP's own TLS set-up (the plain builder has none).
    let client = crate::crm::email::oauth::http().map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
    let xml = client
        .get(format!("{base}/hosting/discovery"))
        .send()
        .await
        .map_err(|e| AppError::bad(format!("Collabora could not be reached: {e}")))?
        .text()
        .await
        .map_err(|e| AppError::bad(format!("Collabora could not be read: {e}")))?;
    let list = parse_discovery(&xml);
    if list.is_empty() {
        return Err(AppError::bad("Collabora's discovery lists no editors"));
    }
    let _: Result<(), _> = redis.set_ex(key, serde_json::to_string(&list).unwrap(), 3600).await;
    Ok(list)
}

/// (extension, action, urlsrc) for every <action> in the discovery XML.
fn parse_discovery(xml: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut r = quick_xml::Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(quick_xml::events::Event::Empty(e)) | Ok(quick_xml::events::Event::Start(e)) if e.local_name().as_ref() == "action" => {
                let get = |k: &str| e.attributes().flatten().find(|a| a.key.local_name().as_ref() == k).map(|a| a.value.to_string());
                if let (Some(ext), Some(name), Some(url)) = (get("ext"), get("name"), get("urlsrc")) {
                    if !ext.is_empty() {
                        out.push((ext.to_lowercase(), name, url.replace("&amp;", "&")));
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

async fn status(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    let editing = match state.config.collabora_url {
        Some(_) => discovery(&state).await.map(|d| d.iter().map(|(e, ..)| e.clone()).collect::<std::collections::BTreeSet<_>>()).ok(),
        None => None,
    };
    Ok(Json(json!({ "preview": PREVIEWABLE, "editing": editing.is_some(), "editable": editing.unwrap_or_default() })))
}

fn sign(state: &AppState, payload: &str) -> String {
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(state.config.jwt_secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(format!("wopi:{payload}").as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// A token for one person's sign-in (session), one file, until a time:
/// "node.user.session.expiry.signature". It stops working when that sign-in ends (signing
/// out, a password change, sessions revoked), not only when it expires.
fn token(state: &AppState, node: Uuid, user: Uuid, session: Uuid, exp: i64) -> String {
    let p = format!("{node}.{user}.{session}.{exp}");
    format!("{p}.{}", sign(state, &p))
}

struct Token {
    node: Uuid,
    user: Uuid,
}

async fn verify(state: &AppState, t: &str, node: Uuid) -> AppResult<Token> {
    let parts: Vec<&str> = t.split('.').collect();
    let [n, u, sid, exp, sig] = parts.as_slice() else { return Err(AppError::Unauthorized) };
    let want = sign(state, &format!("{n}.{u}.{sid}.{exp}"));
    if want.len() != sig.len() || want.bytes().zip(sig.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) != 0 {
        return Err(AppError::Unauthorized);
    }
    let exp: i64 = exp.parse().map_err(|_| AppError::Unauthorized)?;
    let parse = |x: &str| Uuid::parse_str(x).map_err(|_| AppError::Unauthorized);
    let (n, u, sid) = (parse(n)?, parse(u)?, parse(sid)?);
    if exp < Utc::now().timestamp() || n != node {
        return Err(AppError::Unauthorized);
    }
    match crate::auth::tokens::get_session(state, sid).await? {
        Some(s) if s.user_id == u => Ok(Token { node: n, user: u }),
        _ => Err(AppError::Unauthorized),
    }
}

#[derive(Deserialize)]
struct OpenInput {
    /// Only look (default: edit when allowed).
    #[serde(default)]
    view: bool,
}

/// What the browser needs to open a drive file in Collabora: the editor's address (with
/// NERP's WOPI address for the file) and a token to post to it.
async fn open(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, body: Option<Json<OpenInput>>) -> AppResult<Json<Value>> {
    if auth.session_id.is_nil() {
        return Err(AppError::bad("open files for editing from a signed-in browser"));
    }
    let f = crate::drive::office_file(&state, &auth, id).await?;
    let ext = ext_of(&f.name);
    let editors = discovery(&state).await?;
    let wanted = if body.is_some_and(|b| b.view) || !f.can_edit { ["view", "edit"] } else { ["edit", "view"] };
    let (_, action, url) = wanted
        .iter()
        .find_map(|w| editors.iter().find(|(e, a, _)| *e == ext && a == w))
        .cloned()
        .ok_or_else(|| AppError::bad(format!("Collabora can't open .{ext} files")))?;
    let exp = Utc::now().timestamp() + TOKEN_SECS;
    let src = format!("{}/api/wopi/files/{id}", state.config.public_url);
    let enc: String = src.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    let sep = if url.contains('?') { if url.ends_with('?') || url.ends_with('&') { "" } else { "&" } } else { "?" };
    Ok(Json(json!({
        "url": format!("{url}{sep}WOPISrc={enc}&lang=en"),
        // Posted to the editor as access_token / access_token_ttl.
        "token": token(&state, id, auth.id, auth.session_id, exp),
        "token_ttl": exp * 1000,
        "action": action,
        "name": f.name,
        "can_edit": f.can_edit && action == "edit",
    })))
}

// ---------------------------------------------------------------------------
// WOPI (called by Collabora)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TokenQuery {
    access_token: Option<String>,
}

impl TokenQuery {
    fn token(&self) -> AppResult<&str> {
        self.access_token.as_deref().ok_or(AppError::Unauthorized)
    }
}

/// The person a token stands for, with their permissions now (so revoked access counts).
async fn person(state: &AppState, t: &Token) -> AppResult<(AuthUser, String)> {
    let row: Option<(String, bool)> = sqlx::query_as("SELECT trim(first_name || ' ' || last_name), is_active FROM users WHERE id = $1").bind(t.user).fetch_optional(&state.db).await?;
    let Some((name, true)) = row else { return Err(AppError::Unauthorized) };
    let perms = crate::auth::load_permissions(state, t.user).await?;
    Ok((AuthUser { id: t.user, session_id: Uuid::nil(), permissions: perms.into_iter().collect() }, name))
}

fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, true)
}

async fn check_file_info(State(state): State<AppState>, Path(id): Path<Uuid>, Query(q): Query<TokenQuery>) -> AppResult<Json<Value>> {
    let t = verify(&state, q.token()?, id).await?;
    let (auth, name) = person(&state, &t).await?;
    let f = crate::drive::office_file(&state, &auth, t.node).await.map_err(|_| AppError::Unauthorized)?;
    Ok(Json(json!({
        "BaseFileName": f.name,
        "Size": f.size,
        "Version": f.version.to_string(),
        "OwnerId": f.owner.map(|o| o.to_string()).unwrap_or_default(),
        "UserId": auth.id.to_string(),
        "UserFriendlyName": name,
        "UserCanWrite": f.can_edit,
        "UserCanNotWriteRelative": true,
        "SupportsUpdate": true,
        "SupportsLocks": false,
        "LastModifiedTime": stamp(f.updated_at),
        "PostMessageOrigin": state.config.public_url,
        "EnableOwnerTermination": false,
        "HideSaveOption": false,
    })))
}

async fn get_file(State(state): State<AppState>, Path(id): Path<Uuid>, Query(q): Query<TokenQuery>) -> AppResult<Response> {
    let t = verify(&state, q.token()?, id).await?;
    let (auth, _) = person(&state, &t).await?;
    let f = crate::drive::office_file(&state, &auth, t.node).await.map_err(|_| AppError::Unauthorized)?;
    let (_, _, _, bytes) = crate::routes::files::read_bytes(&state, f.file_id).await?;
    let mut r = bytes.into_response();
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    r.headers_mut().insert("X-WOPI-ItemVersion", HeaderValue::from_str(&f.version.to_string()).unwrap());
    Ok(r)
}

/// Saving: refused (409) when the file changed since the editor last loaded it, unless the
/// editor says to overwrite.
async fn put_file(State(state): State<AppState>, Path(id): Path<Uuid>, Query(q): Query<TokenQuery>, headers: HeaderMap, body: Bytes) -> AppResult<Response> {
    let t = verify(&state, q.token()?, id).await?;
    let (auth, _) = person(&state, &t).await?;
    let f = crate::drive::office_file(&state, &auth, t.node).await.map_err(|_| AppError::Unauthorized)?;
    if !f.can_edit {
        return Err(AppError::Forbidden);
    }
    if let Some(ts) = headers.get("X-COOL-WOPI-Timestamp").or(headers.get("X-LOOL-WOPI-Timestamp")).and_then(|v| v.to_str().ok()) {
        let theirs = DateTime::parse_from_rfc3339(ts).map(|t| t.with_timezone(&Utc)).ok();
        if theirs.is_some_and(|t| (t - f.updated_at).num_milliseconds().abs() > 1) {
            return Ok((StatusCode::CONFLICT, Json(json!({ "COOLStatusCode": 1010 }))).into_response());
        }
    }
    let (v, at) = crate::drive::save_version(&state, &auth, t.node, &body).await?;
    let mut r = Json(json!({ "LastModifiedTime": stamp(at) })).into_response();
    r.headers_mut().insert("X-WOPI-ItemVersion", HeaderValue::from_str(&v.to_string()).unwrap());
    Ok(r)
}

/// Locks (Collabora doesn't need them; NERP says it doesn't support them) and other
/// overrides: answered so the editor carries on.
async fn lock_ops(State(state): State<AppState>, Path(id): Path<Uuid>, Query(q): Query<TokenQuery>, headers: HeaderMap) -> AppResult<Response> {
    verify(&state, q.token()?, id).await?;
    let op = headers.get("X-WOPI-Override").and_then(|v| v.to_str().ok()).unwrap_or("");
    Ok(match op {
        "LOCK" | "UNLOCK" | "REFRESH_LOCK" | "GET_LOCK" => StatusCode::OK.into_response(),
        _ => StatusCode::NOT_IMPLEMENTED.into_response(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_discovery() {
        let xml = r#"<?xml version="1.0"?><wopi-discovery><net-zone name="external-http">
            <app name="writer"><action default="true" ext="docx" name="edit" urlsrc="http://127.0.0.1:9980/browser/abc/cool.html?"/><action ext="odt" name="edit" urlsrc="http://127.0.0.1:9980/browser/abc/cool.html?"/></app>
            <app name="Capabilities"><action ext="" name="getinfo" urlsrc="http://127.0.0.1:9980/hosting/capabilities"/></app>
            </net-zone></wopi-discovery>"#;
        let d = parse_discovery(xml);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0], ("docx".into(), "edit".into(), "http://127.0.0.1:9980/browser/abc/cool.html?".into()));
        assert_eq!(ext_of("Plan Q4.DOCX"), "docx");
    }
}
