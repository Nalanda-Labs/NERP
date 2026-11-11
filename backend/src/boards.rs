//! Whiteboards (drawn with Excalidraw in the browser).
//!
//! - **Co-editing**: each save sends the elements a person changed; the server keeps, for
//!   every element, the newer of the two versions (Excalidraw's own rule: higher `version`,
//!   then lower `versionNonce`) and raises the board's version. People with the board open
//!   hear about it at once (real-time); guests on a public link ask every few seconds.
//!   Who has it open shows as a list of names.
//! - **Who may open it**: its owner; everyone, departments or people it's shared with (view or
//!   edit); people of the task or workgroup it was made in; public links (view or edit, with
//!   an expiry date or a password if wanted). A locked board is read-only to all but its owner.
//! - **Images** are NERP files; **templates** are boards marked as templates; each board can
//!   have a chat.

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use base64::Engine;
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    auth::{tokens::random_token, AuthUser, ClientIp},
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

const MAX_SCENE: usize = 15 * 1024 * 1024;
const MAX_ELEMENTS: usize = 20_000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/boards", get(list).post(create))
        .route("/boards/{id}", get(open).patch(update).delete(remove))
        .route("/boards/{id}/scene", get(scene).put(save))
        .route("/boards/{id}/files", post(add_file))
        .route("/boards/{id}/files/{key}", get(file))
        .route("/boards/{id}/access", put(set_access))
        .route("/boards/{id}/links", get(links).post(new_link))
        .route("/boards/{id}/links/{link}", delete(drop_link))
        .route("/boards/{id}/chat", post(chat))
        .route("/public/boards/{token}", get(pub_open))
        .route("/public/boards/{token}/scene", get(pub_scene).put(pub_save))
        .route("/public/boards/{token}/files", post(pub_add_file))
        .route("/public/boards/{token}/files/{key}", get(pub_file))
        .layer(DefaultBodyLimit::max(MAX_SCENE))
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    None,
    View,
    Edit,
    Owner,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Level::None => "none",
            Level::View => "view",
            Level::Edit => "edit",
            Level::Owner => "owner",
        }
    }
    fn parse(s: &str) -> Level {
        if s == "edit" { Level::Edit } else { Level::View }
    }
}

#[derive(sqlx::FromRow, Clone)]
struct Board {
    id: Uuid,
    owner_id: Uuid,
    title: String,
    version: i64,
    is_template: bool,
    task_id: Option<Uuid>,
    project_id: Option<Uuid>,
    locked: bool,
    updated_at: DateTime<Utc>,
    /// Made for a feed post: its readers open it (to look or to draw, as the post says).
    post_id: Option<Uuid>,
    post_level: String,
    /// Kept in the drive: the folder's people open it as the drive lets them.
    drive_node_id: Option<Uuid>,
}

const COLS: &str = "id, owner_id, title, version, is_template, task_id, project_id, locked, updated_at, post_id, post_level, drive_node_id";

async fn load(state: &AppState, id: Uuid) -> AppResult<Board> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM boards WHERE id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("board"))
}

/// What a person may do with a board.
async fn level(state: &AppState, auth: &AuthUser, b: &Board) -> AppResult<Level> {
    if b.owner_id == auth.id {
        return Ok(Level::Owner);
    }
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT grantee_kind, level FROM board_access WHERE board_id = $1 AND (grantee_kind = 'everyone'
            OR (grantee_kind = 'user' AND grantee_id = $2)
            OR (grantee_kind = 'department' AND grantee_id IN (SELECT department_id FROM user_departments WHERE user_id = $2)))",
    )
    .bind(b.id)
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    // Shared with "everyone" means the company, not guests.
    let mut l = rows.iter().filter(|(k, _)| k != "everyone" || !auth.is_guest()).map(|(_, lv)| Level::parse(lv)).max().unwrap_or(Level::None);
    // Templates are for everyone in the company to start from.
    if b.is_template && !auth.is_guest() {
        l = l.max(Level::View);
    }
    if let Some(t) = b.task_id {
        if crate::tasks::access::load_task(state, auth, t).await.is_ok() {
            l = l.max(Level::Edit);
        }
    }
    if let Some(p) = b.project_id {
        if let Ok(a) = crate::tasks::access::project_access(state, auth, p).await {
            l = l.max(if a.contribute { Level::Edit } else { Level::View });
        }
    }
    if let Some(p) = b.post_id
        && l < Level::Edit
        && crate::feed::can_see_post(state, auth, p).await?
    {
        l = l.max(Level::parse(&b.post_level));
    }
    if let Some(n) = b.drive_node_id
        && l < Level::Edit
    {
        l = l.max(match crate::drive::board_item_level(state, auth, n).await? {
            2 => Level::Edit,
            1 => Level::View,
            _ => Level::None,
        });
    }
    if b.locked && l == Level::Edit {
        l = Level::View;
    }
    Ok(l)
}

async fn need(state: &AppState, auth: &AuthUser, id: Uuid, at_least: Level) -> AppResult<(Board, Level)> {
    let b = load(state, id).await?;
    let l = level(state, auth, &b).await?;
    if l == Level::None {
        return Err(AppError::NotFound("board"));
    }
    if l < at_least {
        return Err(AppError::Forbidden);
    }
    Ok((b, l))
}

async fn board_json(state: &AppState, b: &Board, l: Level) -> AppResult<Value> {
    let owner: (String, Option<Uuid>) = sqlx::query_as("SELECT trim(first_name || ' ' || last_name), avatar_file_id FROM users WHERE id = $1").bind(b.owner_id).fetch_one(&state.db).await?;
    let task: Option<(i64, String)> = match b.task_id {
        Some(t) => sqlx::query_as("SELECT number, title FROM tasks WHERE id = $1").bind(t).fetch_optional(&state.db).await?,
        None => None,
    };
    let project: Option<String> = match b.project_id {
        Some(p) => sqlx::query_scalar("SELECT name FROM projects WHERE id = $1").bind(p).fetch_optional(&state.db).await?,
        None => None,
    };
    Ok(json!({
        "id": b.id, "title": b.title, "version": b.version, "is_template": b.is_template, "locked": b.locked, "updated_at": b.updated_at,
        "owner": { "id": b.owner_id, "name": owner.0, "avatar_file_id": owner.1 },
        "task": b.task_id.zip(task).map(|(id, (n, t))| json!({ "id": id, "number": n, "title": t })),
        "project": b.project_id.zip(project).map(|(id, n)| json!({ "id": id, "name": n })),
        "post_id": b.post_id,
        "drive_node_id": b.drive_node_id,
        "level": l.name(),
    }))
}

#[derive(Deserialize)]
struct ListQuery {
    /// mine | shared | templates | task | project
    scope: Option<String>,
    q: Option<String>,
    task_id: Option<Uuid>,
    project_id: Option<Uuid>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let term = q.q.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(|t| format!("%{}%", t.replace('%', "\\%").replace('_', "\\_")));
    let ids: Vec<Uuid> = match q.scope.as_deref().unwrap_or("mine") {
        "templates" => sqlx::query_scalar("SELECT id FROM boards WHERE is_template ORDER BY title LIMIT 200").fetch_all(&state.db).await?,
        "task" => sqlx::query_scalar("SELECT id FROM boards WHERE task_id = $1 ORDER BY updated_at DESC").bind(q.task_id).fetch_all(&state.db).await?,
        "project" => sqlx::query_scalar("SELECT id FROM boards WHERE project_id = $1 ORDER BY updated_at DESC").bind(q.project_id).fetch_all(&state.db).await?,
        "shared" => sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
            "SELECT DISTINCT b.id, b.updated_at FROM boards b JOIN board_access a ON a.board_id = b.id
             WHERE b.owner_id <> $1 AND NOT b.is_template AND (a.grantee_kind = 'everyone' OR (a.grantee_kind = 'user' AND a.grantee_id = $1)
                OR (a.grantee_kind = 'department' AND a.grantee_id IN (SELECT department_id FROM user_departments WHERE user_id = $1)))
             ORDER BY b.updated_at DESC LIMIT 500",
        )
        .bind(auth.id)
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|(id, _): (Uuid, DateTime<Utc>)| id)
        .collect(),
        _ => sqlx::query_scalar("SELECT id FROM boards WHERE owner_id = $1 AND NOT is_template ORDER BY updated_at DESC LIMIT 500").bind(auth.id).fetch_all(&state.db).await?,
    };
    let boards: Vec<Board> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM boards WHERE id = ANY($1) AND ($2::text IS NULL OR title ILIKE $2) ORDER BY updated_at DESC")))
        .bind(&ids)
        .bind(term)
        .fetch_all(&state.db)
        .await?;
    let mut out = Vec::new();
    for b in &boards {
        let l = level(&state, &auth, b).await?;
        if l > Level::None {
            out.push(board_json(&state, b, l).await?);
        }
    }
    Ok(Json(json!({ "items": out })))
}

#[derive(Deserialize)]
struct NewBoard {
    title: String,
    /// Start from a template (or any board one may open): a copy of its drawing.
    from: Option<Uuid>,
    /// Or from a drawing made in the browser (e.g. a built-in template).
    scene: Option<Value>,
    task_id: Option<Uuid>,
    project_id: Option<Uuid>,
    /// Keep it in the drive: personal, company or group (with `project_id`), or in a folder.
    drive: Option<DrivePlace>,
}

#[derive(Deserialize)]
struct DrivePlace {
    space: Option<String>,
    project_id: Option<Uuid>,
    folder_id: Option<Uuid>,
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<NewBoard>) -> AppResult<Json<Value>> {
    let title = required("title", &b.title, 200)?;
    if let Some(t) = b.task_id {
        crate::tasks::access::load_task(&state, &auth, t).await?;
    }
    if let Some(p) = b.project_id {
        if !crate::tasks::access::project_access(&state, &auth, p).await?.contribute {
            return Err(AppError::Forbidden);
        }
    }
    let (scene, files): (Value, Vec<(String, Uuid)>) = match (b.from, b.scene) {
        (Some(src), _) => {
            need(&state, &auth, src, Level::View).await?;
            let s: Value = sqlx::query_scalar("SELECT scene FROM boards WHERE id = $1").bind(src).fetch_one(&state.db).await?;
            let f: Vec<(String, Uuid)> = sqlx::query_as("SELECT file_key, file_id FROM board_files WHERE board_id = $1").bind(src).fetch_all(&state.db).await?;
            (s, f)
        }
        (None, Some(s)) => (clean_scene(s)?, vec![]),
        _ => (json!({ "elements": [], "appState": {} }), vec![]),
    };
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO boards (owner_id, title, scene, task_id, project_id, updated_by) VALUES ($1, $2, $3, $4, $5, $1) RETURNING id")
        .bind(auth.id)
        .bind(&title)
        .bind(&scene)
        .bind(b.task_id)
        .bind(b.project_id)
        .fetch_one(&mut *tx)
        .await?;
    for (k, f) in files {
        sqlx::query("INSERT INTO board_files (board_id, file_key, file_id) VALUES ($1, $2, $3)").bind(id).bind(k).bind(f).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    if let Some(d) = &b.drive {
        match crate::drive::add_board_item(&state, &auth, d.space.as_deref(), d.project_id, d.folder_id, &title, id).await {
            Ok(node) => {
                sqlx::query("UPDATE boards SET drive_node_id = $2 WHERE id = $1").bind(id).bind(node).execute(&state.db).await?;
            }
            Err(e) => {
                sqlx::query("DELETE FROM boards WHERE id = $1").bind(id).execute(&state.db).await?;
                return Err(e);
            }
        }
    }
    let bd = load(&state, id).await?;
    Ok(Json(board_json(&state, &bd, Level::Owner).await?))
}

/// The board with its drawing, image files and (for its owner) sharing.
async fn open(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (b, l) = need(&state, &auth, id, Level::View).await?;
    let mut v = board_json(&state, &b, l).await?;
    let scene: Value = sqlx::query_scalar("SELECT scene FROM boards WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    v["scene"] = scene;
    v["files"] = files_json(&state, id).await?;
    if l == Level::Owner {
        v["access"] = access_json(&state, id).await?;
    }
    let me: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await?;
    v["viewers"] = seen(&state, id, &format!("u:{}", auth.id), &me).await;
    Ok(Json(v))
}

async fn files_json(state: &AppState, id: Uuid) -> AppResult<Value> {
    let rows: Vec<(String, Uuid, String)> =
        sqlx::query_as("SELECT bf.file_key, bf.file_id, f.mime_type FROM board_files bf JOIN files f ON f.id = bf.file_id WHERE bf.board_id = $1").bind(id).fetch_all(&state.db).await?;
    Ok(json!(rows.into_iter().map(|(k, f, m)| json!({ "key": k, "file_id": f, "mime": m })).collect::<Vec<_>>()))
}

async fn access_json(state: &AppState, id: Uuid) -> AppResult<Value> {
    let rows: Vec<(String, Option<Uuid>, String, Option<String>)> = sqlx::query_as(
        "SELECT a.grantee_kind, a.grantee_id, a.level,
                CASE a.grantee_kind WHEN 'user' THEN (SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = a.grantee_id)
                                    WHEN 'department' THEN (SELECT name FROM departments WHERE id = a.grantee_id) END
         FROM board_access a WHERE a.board_id = $1 ORDER BY 1, 4",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(json!(rows.into_iter().map(|(k, gid, l, n)| json!({ "kind": k, "id": gid, "level": l, "name": n })).collect::<Vec<_>>()))
}

#[derive(Deserialize)]
struct Patch {
    title: Option<String>,
    is_template: Option<bool>,
    locked: Option<bool>,
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(p): Json<Patch>) -> AppResult<Json<Value>> {
    let (b, l) = need(&state, &auth, id, Level::Edit).await?;
    if (p.is_template.is_some() || p.locked.is_some()) && l != Level::Owner {
        return Err(AppError::Forbidden);
    }
    let title = p.title.as_deref().map(|t| required("title", t, 200)).transpose()?;
    sqlx::query("UPDATE boards SET title = COALESCE($2, title), is_template = COALESCE($3, is_template), locked = COALESCE($4, locked), updated_at = now() WHERE id = $1")
        .bind(b.id)
        .bind(title)
        .bind(p.is_template)
        .bind(p.locked)
        .execute(&state.db)
        .await?;
    crate::im::sync_linked_chat(&state, crate::im::Link::Board(id)).await?;
    let b = load(&state, id).await?;
    Ok(Json(board_json(&state, &b, level(&state, &auth, &b).await?).await?))
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (b, _) = need(&state, &auth, id, Level::Owner).await?;
    let chat = crate::im::linked_chat_people(&state, crate::im::Link::Board(b.id)).await?;
    sqlx::query("DELETE FROM boards WHERE id = $1").bind(b.id).execute(&state.db).await?;
    crate::im::linked_chat_gone(&state, chat).await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// The drawing
// ---------------------------------------------------------------------------

/// Keeps only what a scene should hold; checks its size.
fn clean_scene(s: Value) -> AppResult<Value> {
    let elements = s.get("elements").and_then(Value::as_array).cloned().unwrap_or_default();
    if elements.len() > MAX_ELEMENTS {
        return Err(AppError::bad("a board can hold at most 20,000 shapes"));
    }
    if elements.iter().any(|e| e.get("id").and_then(Value::as_str).is_none()) {
        return Err(AppError::bad("every shape needs an id"));
    }
    Ok(json!({ "elements": elements, "appState": app_state(s.get("appState")) }))
}

/// The board-wide settings worth keeping (not someone's zoom or selection).
fn app_state(a: Option<&Value>) -> Value {
    let mut out = Map::new();
    if let Some(a) = a.and_then(Value::as_object) {
        for k in ["viewBackgroundColor", "gridSize", "gridModeEnabled"] {
            if let Some(v) = a.get(k) {
                out.insert(k.into(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// Whether `new` replaces `old`: a higher version, or the same with a lower nonce.
fn newer(new: &Value, old: &Value) -> bool {
    let v = |e: &Value, k: &str| e.get(k).and_then(Value::as_i64).unwrap_or(0);
    v(new, "version") > v(old, "version") || (v(new, "version") == v(old, "version") && v(new, "versionNonce") < v(old, "versionNonce"))
}

/// Merges changed elements into the stored ones. Returns the merged list and whether anything changed.
fn merge(stored: &[Value], incoming: &[Value]) -> (Vec<Value>, bool) {
    let mut out: Vec<Value> = stored.to_vec();
    let mut index: std::collections::HashMap<String, usize> = out.iter().enumerate().filter_map(|(i, e)| e["id"].as_str().map(|id| (id.to_string(), i))).collect();
    let mut changed = false;
    for e in incoming {
        let Some(id) = e["id"].as_str() else { continue };
        match index.get(id) {
            Some(&i) => {
                if newer(e, &out[i]) {
                    out[i] = e.clone();
                    changed = true;
                }
            }
            None => {
                index.insert(id.to_string(), out.len());
                out.push(e.clone());
                changed = true;
            }
        }
    }
    (out, changed)
}

#[derive(Deserialize)]
struct SceneQuery {
    since: Option<i64>,
}

/// The drawing if it's newer than `since` (and who has it open, which also notes me).
async fn scene(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<SceneQuery>) -> AppResult<Json<Value>> {
    let (b, l) = need(&state, &auth, id, Level::View).await?;
    let me: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await?;
    scene_json(&state, &b, l, q.since, &format!("u:{}", auth.id), &me).await.map(Json)
}

async fn scene_json(state: &AppState, b: &Board, l: Level, since: Option<i64>, who: &str, name: &str) -> AppResult<Value> {
    let viewers = seen(state, b.id, who, name).await;
    if since == Some(b.version) {
        return Ok(json!({ "version": b.version, "unchanged": true, "viewers": viewers, "level": l.name() }));
    }
    let scene: Value = sqlx::query_scalar("SELECT scene FROM boards WHERE id = $1").bind(b.id).fetch_one(&state.db).await?;
    Ok(json!({ "version": b.version, "scene": scene, "files": files_json(state, b.id).await?, "viewers": viewers, "level": l.name() }))
}

/// Notes `who` as looking at the board (for a minute) and returns everyone looking.
async fn seen(state: &AppState, board: Uuid, who: &str, name: &str) -> Value {
    let mut redis = state.redis.clone();
    let key = format!("board:viewers:{board}");
    let now = Utc::now().timestamp();
    let _: Result<(), _> = redis.zadd(&key, format!("{who}|{name}"), now).await;
    let _: Result<(), _> = redis.zrembyscore(&key, 0, now - 60).await;
    let _: Result<(), _> = redis.expire(&key, 120).await;
    let list: Vec<String> = redis.zrange(&key, 0, 100).await.unwrap_or_default();
    json!(list.into_iter().filter_map(|x| x.split_once('|').map(|(w, n)| json!({ "id": w, "name": n }))).collect::<Vec<_>>())
}

#[derive(Deserialize)]
struct SaveInput {
    /// Elements that changed (or all).
    elements: Vec<Value>,
    #[serde(default, rename = "appState")]
    app_state: Option<Value>,
}

async fn save(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<SaveInput>) -> AppResult<Json<Value>> {
    let (bd, _) = need(&state, &auth, id, Level::Edit).await?;
    let v = store(&state, &bd, b, Some(auth.id)).await?;
    Ok(Json(v))
}

/// Merges a save under a row lock and tells the people looking at it.
async fn store(state: &AppState, b: &Board, input: SaveInput, by: Option<Uuid>) -> AppResult<Value> {
    if input.elements.len() > MAX_ELEMENTS {
        return Err(AppError::bad("a board can hold at most 20,000 shapes"));
    }
    let mut tx = state.db.begin().await?;
    let (scene, version): (Value, i64) = sqlx::query_as("SELECT scene, version FROM boards WHERE id = $1 FOR UPDATE").bind(b.id).fetch_one(&mut *tx).await?;
    let stored = scene["elements"].as_array().cloned().unwrap_or_default();
    let (merged, mut changed) = merge(&stored, &input.elements);
    if merged.len() > MAX_ELEMENTS {
        return Err(AppError::bad("a board can hold at most 20,000 shapes"));
    }
    let mut app = scene["appState"].clone();
    if let Some(a) = input.app_state.as_ref() {
        let fresh = app_state(Some(a));
        if fresh.as_object().is_some_and(|m| !m.is_empty()) && fresh != app {
            app = fresh;
            changed = true;
        }
    }
    let version = if changed {
        let new = json!({ "elements": merged, "appState": app });
        if new.to_string().len() > MAX_SCENE {
            return Err(AppError::bad("the board is too big"));
        }
        sqlx::query_scalar("UPDATE boards SET scene = $2, version = version + 1, updated_by = $3, updated_at = now() WHERE id = $1 RETURNING version")
            .bind(b.id)
            .bind(&new)
            .bind(by)
            .fetch_one(&mut *tx)
            .await?
    } else {
        version
    };
    tx.commit().await?;
    if changed {
        // Tell signed-in people looking at it.
        let mut redis = state.redis.clone();
        let list: Vec<String> = redis.zrange(format!("board:viewers:{}", b.id), 0, 100).await.unwrap_or_default();
        let users: Vec<Uuid> = list.iter().filter_map(|x| x.strip_prefix("u:").and_then(|r| r.split('|').next()).and_then(|u| Uuid::parse_str(u).ok())).collect();
        if !users.is_empty() {
            crate::realtime::publish(state, Some(users), "board.changed", json!({ "id": b.id, "version": version, "by": by })).await;
        }
    }
    Ok(json!({ "version": version, "elements": merged }))
}

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct FileInput {
    /// Excalidraw's file id.
    key: String,
    /// A NERP file (uploaded first), or the image itself as a data: URL.
    file_id: Option<Uuid>,
    data_url: Option<String>,
}

async fn attach(state: &AppState, b: &Board, f: FileInput, owner: Option<Uuid>, auth: Option<&AuthUser>) -> AppResult<Value> {
    let key = f.key.trim();
    if key.is_empty() || key.len() > 100 {
        return Err(AppError::bad("bad file key"));
    }
    let file_id = match (f.file_id, f.data_url) {
        (Some(id), _) => {
            // Only one's own files (or ones one may read) go on a board.
            let auth = auth.ok_or(AppError::Forbidden)?;
            crate::routes::files::read_bytes(state, id).await?;
            let up: Option<Uuid> = sqlx::query_scalar("SELECT owner_id FROM files WHERE id = $1").bind(id).fetch_one(&state.db).await?;
            if up != Some(auth.id) {
                return Err(AppError::Forbidden);
            }
            id
        }
        (None, Some(d)) => {
            let (head, data) = d.split_once(',').ok_or_else(|| AppError::bad("bad image"))?;
            let mime = head.strip_prefix("data:").and_then(|h| h.strip_suffix(";base64")).ok_or_else(|| AppError::bad("bad image"))?;
            if !["image/png", "image/jpeg", "image/gif", "image/webp", "image/svg+xml"].contains(&mime) {
                return Err(AppError::bad("only images go on a board"));
            }
            let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| AppError::bad("bad image"))?;
            if bytes.len() > 10 * 1024 * 1024 {
                return Err(AppError::PayloadTooLarge("images may be at most 10 MB".into()));
            }
            let ext = mime.split('/').nth(1).unwrap_or("png").replace("svg+xml", "svg").replace("jpeg", "jpg");
            crate::routes::files::store_bytes(state, owner, &format!("board-image.{ext}"), mime, &bytes).await?
        }
        _ => return Err(AppError::bad("send a file id or the image")),
    };
    sqlx::query("INSERT INTO board_files (board_id, file_key, file_id) VALUES ($1, $2, $3) ON CONFLICT (board_id, file_key) DO NOTHING").bind(b.id).bind(key).bind(file_id).execute(&state.db).await?;
    Ok(json!({ "key": key, "file_id": file_id }))
}

async fn add_file(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(f): Json<FileInput>) -> AppResult<Json<Value>> {
    let (b, _) = need(&state, &auth, id, Level::Edit).await?;
    attach(&state, &b, f, Some(auth.id), Some(&auth)).await.map(Json)
}

async fn serve_file(state: &AppState, board: Uuid, key: &str) -> AppResult<Response> {
    let fid: Uuid = sqlx::query_scalar("SELECT file_id FROM board_files WHERE board_id = $1 AND file_key = $2").bind(board).bind(key).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("image"))?;
    let (_, mime, _, bytes) = crate::routes::files::read_bytes(state, fid).await?;
    let mut r = bytes.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=86400"));
    h.insert("X-Content-Type-Options", HeaderValue::from_static("nosniff"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"));
    Ok(r)
}

async fn file(State(state): State<AppState>, auth: AuthUser, Path((id, key)): Path<(Uuid, String)>) -> AppResult<Response> {
    need(&state, &auth, id, Level::View).await?;
    serve_file(&state, id, &key).await
}

// ---------------------------------------------------------------------------
// Sharing
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Grant {
    kind: String,
    id: Option<Uuid>,
    level: String,
}

#[derive(Deserialize)]
struct AccessInput {
    grants: Vec<Grant>,
}

async fn set_access(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<AccessInput>) -> AppResult<Json<Value>> {
    let (bd, _) = need(&state, &auth, id, Level::Owner).await?;
    if b.grants.len() > 200 {
        return Err(AppError::bad("at most 200 entries"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM board_access WHERE board_id = $1").bind(bd.id).execute(&mut *tx).await?;
    for g in &b.grants {
        if !["view", "edit"].contains(&g.level.as_str()) {
            return Err(AppError::bad("levels are view or edit"));
        }
        let gid = match (g.kind.as_str(), g.id) {
            ("everyone", _) => None,
            ("user", Some(u)) if u != bd.owner_id => Some(u),
            ("department", Some(d)) => Some(d),
            _ => return Err(AppError::bad("share with everyone, a department or a person")),
        };
        sqlx::query("INSERT INTO board_access (board_id, grantee_kind, grantee_id, level) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
            .bind(bd.id)
            .bind(&g.kind)
            .bind(gid)
            .bind(&g.level)
            .execute(&mut *tx)
            .await
            .map_err(|_| AppError::bad("no such person or department"))?;
    }
    tx.commit().await?;
    crate::im::sync_linked_chat(&state, crate::im::Link::Board(id)).await?;
    Ok(Json(json!({ "access": access_json(&state, id).await? })))
}

fn hash(t: &str) -> String {
    hex::encode(Sha256::digest(t.as_bytes()))
}

async fn links(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&state, &auth, id, Level::Owner).await?;
    let rows: Vec<(Uuid, String, Option<DateTime<Utc>>, bool, DateTime<Utc>)> =
        sqlx::query_as("SELECT id, level, expires_at, password_hash IS NOT NULL, created_at FROM board_links WHERE board_id = $1 ORDER BY created_at").bind(id).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(lid, l, e, p, c)| json!({ "id": lid, "level": l, "expires_at": e, "password": p, "created_at": c })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct LinkInput {
    level: String,
    expires_at: Option<DateTime<Utc>>,
    password: Option<String>,
}

/// A public link; its address is shown only now.
async fn new_link(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<LinkInput>) -> AppResult<Json<Value>> {
    crate::routes::settings::require_public_links(&state).await?;
    need(&state, &auth, id, Level::Owner).await?;
    if !["view", "edit"].contains(&b.level.as_str()) {
        return Err(AppError::bad("a link lets people view or edit"));
    }
    if b.expires_at.is_some_and(|e| e <= Utc::now()) {
        return Err(AppError::bad("the expiry date has passed"));
    }
    let pw = match b.password.filter(|p| !p.is_empty()) {
        Some(p) if p.len() < 4 => return Err(AppError::bad("the password needs at least 4 characters")),
        Some(p) => Some(crate::auth::password::hash_password(p).await?),
        None => None,
    };
    let token = random_token(24);
    let lid: Uuid = sqlx::query_scalar("INSERT INTO board_links (board_id, token_hash, level, expires_at, password_hash, created_by) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id")
        .bind(id)
        .bind(hash(&token))
        .bind(&b.level)
        .bind(b.expires_at)
        .bind(pw)
        .bind(auth.id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(json!({ "id": lid, "url": format!("{}/b/{token}", state.config.public_url) })))
}

async fn drop_link(State(state): State<AppState>, auth: AuthUser, Path((id, link)): Path<(Uuid, Uuid)>) -> AppResult<Json<Value>> {
    need(&state, &auth, id, Level::Owner).await?;
    sqlx::query("DELETE FROM board_links WHERE id = $1 AND board_id = $2").bind(link).bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn chat(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&state, &auth, id, Level::View).await?;
    let link = crate::im::Link::Board(id);
    if !link.has(&state, auth.id).await? {
        return Err(AppError::bad("the board's chat is for its owner and the people it is shared with by name"));
    }
    Ok(Json(json!({ "chat_id": crate::im::linked_chat(&state, link).await? })))
}

// ---------------------------------------------------------------------------
// Public links
// ---------------------------------------------------------------------------

/// The board and level behind a link: not expired, the password right (in the
/// `X-Board-Password` header), and not too many wrong guesses.
async fn by_link(state: &AppState, token: &str, headers: &HeaderMap, ip: Option<&str>) -> AppResult<(Board, Level)> {
    if !crate::routes::settings::public_links_allowed(state).await? {
        return Err(AppError::NotFound("board"));
    }
    let row: Option<(Uuid, String, Option<DateTime<Utc>>, Option<String>)> =
        sqlx::query_as("SELECT board_id, level, expires_at, password_hash FROM board_links WHERE token_hash = $1").bind(hash(token)).fetch_optional(&state.db).await?;
    let (board, lv, expires, pw) = row.ok_or(AppError::NotFound("board"))?;
    if expires.is_some_and(|e| e <= Utc::now()) {
        return Err(AppError::NotFound("board"));
    }
    if let Some(h) = pw {
        let mut redis = state.redis.clone();
        let key = format!("board:pw:{}:{}", hash(token), ip.unwrap_or("?"));
        let fails: i64 = redis.get(&key).await.unwrap_or(0);
        if fails >= 10 {
            return Err(AppError::TooManyRequests("too many wrong passwords; try again later".into()));
        }
        let given = headers.get("X-Board-Password").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        if given.is_empty() {
            return Err(AppError::InvalidCredentials("this board needs a password".into()));
        }
        if !crate::auth::password::verify_password(given, h).await? {
            let _: Result<i64, _> = redis.incr(&key, 1).await;
            let _: Result<(), _> = redis.expire(&key, 900).await;
            return Err(AppError::InvalidCredentials("wrong password".into()));
        }
    }
    let b = load(state, board).await?;
    let l = if b.locked { Level::View } else { Level::parse(&lv) };
    Ok((b, l))
}

async fn pub_open(State(state): State<AppState>, ClientIp(ip): ClientIp, headers: HeaderMap, Path(token): Path<String>) -> AppResult<Json<Value>> {
    let (b, l) = by_link(&state, &token, &headers, ip.as_deref()).await?;
    let mut v = json!({ "id": b.id, "title": b.title, "level": l.name() });
    let s = scene_json(&state, &b, l, None, &format!("g:{}", &hash(&format!("{token}{}", ip.unwrap_or_default()))[..12]), "Guest").await?;
    v["version"] = s["version"].clone();
    v["scene"] = s["scene"].clone();
    v["files"] = s["files"].clone();
    v["viewers"] = s["viewers"].clone();
    Ok(Json(v))
}

#[derive(Deserialize)]
struct PubScene {
    since: Option<i64>,
}

async fn pub_scene(State(state): State<AppState>, ClientIp(ip): ClientIp, headers: HeaderMap, Path(token): Path<String>, Query(q): Query<PubScene>) -> AppResult<Json<Value>> {
    let (b, l) = by_link(&state, &token, &headers, ip.as_deref()).await?;
    scene_json(&state, &b, l, q.since, &format!("g:{}", &hash(&format!("{token}{}", ip.unwrap_or_default()))[..12]), "Guest").await.map(Json)
}

async fn pub_save(State(state): State<AppState>, ClientIp(ip): ClientIp, headers: HeaderMap, Path(token): Path<String>, Json(b): Json<SaveInput>) -> AppResult<Json<Value>> {
    let (bd, l) = by_link(&state, &token, &headers, ip.as_deref()).await?;
    if l < Level::Edit {
        return Err(AppError::Forbidden);
    }
    store(&state, &bd, b, None).await.map(Json)
}

async fn pub_add_file(State(state): State<AppState>, ClientIp(ip): ClientIp, headers: HeaderMap, Path(token): Path<String>, Json(f): Json<FileInput>) -> AppResult<Json<Value>> {
    let (bd, l) = by_link(&state, &token, &headers, ip.as_deref()).await?;
    if l < Level::Edit || f.file_id.is_some() {
        return Err(AppError::Forbidden);
    }
    attach(&state, &bd, f, Some(bd.owner_id), None).await.map(Json)
}

async fn pub_file(State(state): State<AppState>, ClientIp(ip): ClientIp, headers: HeaderMap, Path((token, key)): Path<(String, String)>) -> AppResult<Response> {
    let (b, _) = by_link(&state, &token, &headers, ip.as_deref()).await?;
    serve_file(&state, b.id, &key).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_keeps_the_newer_element() {
        let stored = vec![json!({ "id": "a", "version": 2, "versionNonce": 5, "x": 1 }), json!({ "id": "b", "version": 1, "versionNonce": 1 })];
        let incoming = vec![
            json!({ "id": "a", "version": 1, "versionNonce": 0, "x": 9 }), // older: ignored
            json!({ "id": "b", "version": 1, "versionNonce": 0, "y": 2 }), // same version, lower nonce: wins
            json!({ "id": "c", "version": 1, "versionNonce": 3 }),         // new
        ];
        let (m, changed) = merge(&stored, &incoming);
        assert!(changed);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0]["x"], 1);
        assert_eq!(m[1]["y"], 2);
        let (_, again) = merge(&m, &incoming);
        assert!(!again);
        assert_eq!(app_state(Some(&json!({ "viewBackgroundColor": "#fff", "zoom": 2 }))), json!({ "viewBackgroundColor": "#fff" }));
    }
}
