//! Drive: folders and files in each employee's own space and in the company's space.
//!
//! - My drive is private; folders and files in it can be shared with people and departments
//!   (with sub-departments), to view or to edit (add files and new versions). A shared folder
//!   shares what is inside it.
//! - The company drive is open to every employee to view and add to; renaming, moving and
//!   deleting is for whoever added something and people with "Manage the company drive".
//! - Uploading a file with the name of one already in the folder adds a new version; older
//!   versions can be downloaded and restored.
//! - A file can get public download links, optionally expiring.
//! - Deleted things go to the trash for 30 days (folders with their contents) and can be put
//!   back; after that, or when emptied, they and all their versions are gone.

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    response::Response,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::{ClientIp, permissions as perm, tokens::random_token, AuthUser},
    error::{AppError, AppResult},
    routes::{
        files::delete_stored,
        required,
    },
    state::AppState,
};

pub const PURGE_JOB: &str = "drive.purge";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/drive", get(browse))
        .route("/drive/folders", post(create_folder))
        .route("/drive/files", post(add_file))
        .route("/drive/search", get(search))
        .route("/drive/trash", get(trash))
        .route("/drive/trash/{id}", delete(destroy))
        .route("/drive/nodes/{id}", get(get_node).patch(update_node).delete(remove_node))
        .route("/drive/nodes/{id}/restore", post(restore_node))
        .route("/drive/nodes/{id}/download", get(download))
        .route("/drive/nodes/{id}/versions/{v}/restore", post(restore_version))
        .route("/drive/nodes/{id}/shares", put(set_shares))
        .route("/drive/nodes/{id}/links", post(create_link))
        .route("/drive/links/{id}", delete(delete_link))
        .route("/public/drive/{token}", get(public_info))
        .route("/public/drive/{token}/download", get(public_download))
}

#[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
enum Level {
    None,
    View,
    Edit,
    Own,
}

#[derive(FromRow, Clone)]
struct Node {
    id: Uuid,
    kind: String,
    space: String,
    owner_id: Option<Uuid>,
    project_id: Option<Uuid>,
    parent_id: Option<Uuid>,
    name: String,
    file_id: Option<Uuid>,
    created_by: Option<Uuid>,
    deleted_at: Option<DateTime<Utc>>,
}

const NODE_COLS: &str = "id, kind, space, owner_id, project_id, parent_id, name, file_id, created_by, deleted_at";

async fn node(state: &AppState, id: Uuid) -> AppResult<Node> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {NODE_COLS} FROM drive_nodes WHERE id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("item"))
}

fn manager(auth: &AuthUser) -> bool {
    auth.can(perm::DRIVE)
}

/// A workgroup's files: its owners manage everything, members add files and manage their
/// own, viewers (and everyone, for an open group) look.
async fn group_level(state: &AppState, auth: &AuthUser, project: Uuid, created_by: Option<Uuid>) -> AppResult<Level> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT p.visibility, (SELECT role FROM project_members WHERE project_id = p.id AND user_id = $2) FROM projects p WHERE p.id = $1")
            .bind(project)
            .bind(auth.id)
            .fetch_optional(&state.db)
            .await?;
    let Some((visibility, role)) = row else { return Ok(Level::None) };
    Ok(match role.as_deref() {
        _ if auth.can(perm::TASKS_MANAGE_ALL) => Level::Own,
        Some("owner") => Level::Own,
        Some("member") if created_by == Some(auth.id) => Level::Own,
        Some("member") => Level::Edit,
        Some(_) => Level::View,
        None if visibility == "open" => Level::View,
        None => Level::None,
    })
}

/// What the user may do with a node.
async fn level_of(state: &AppState, auth: &AuthUser, n: &Node) -> AppResult<Level> {
    if n.space == "group" {
        return group_level(state, auth, n.project_id.unwrap_or_default(), n.created_by).await;
    }
    if n.space == "company" {
        return Ok(if manager(auth) || n.created_by == Some(auth.id) { Level::Own } else { Level::Edit });
    }
    if n.owner_id == Some(auth.id) {
        return Ok(Level::Own);
    }
    let levels: Vec<String> = sqlx::query_scalar(
        "WITH RECURSIVE up AS (SELECT id, parent_id FROM drive_nodes WHERE id = $1 UNION ALL SELECT d.id, d.parent_id FROM drive_nodes d JOIN up ON d.id = up.parent_id),
              mine AS (SELECT department_id AS id FROM user_departments WHERE user_id = $2
                       UNION SELECT d.parent_id FROM departments d JOIN mine m ON d.id = m.id WHERE d.parent_id IS NOT NULL)
         SELECT s.level FROM drive_shares s WHERE s.node_id IN (SELECT id FROM up) AND (s.user_id = $2 OR s.department_id IN (SELECT id FROM mine))",
    )
    .bind(n.id)
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    Ok(if levels.iter().any(|l| l == "edit") { Level::Edit } else if levels.is_empty() { Level::None } else { Level::View })
}

async fn need(state: &AppState, auth: &AuthUser, id: Uuid, want: Level) -> AppResult<(Node, Level)> {
    let n = node(state, id).await?;
    let l = level_of(state, auth, &n).await?;
    if l == Level::None {
        return Err(AppError::NotFound("item"));
    }
    if l < want {
        return Err(AppError::Forbidden);
    }
    Ok((n, l))
}

/// Files kept by the drive (any version) are readable by whoever can see the item.
pub async fn can_view_file(state: &AppState, auth: &AuthUser, file: Uuid) -> AppResult<bool> {
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM drive_nodes WHERE file_id = $1 UNION SELECT node_id FROM drive_versions WHERE file_id = $1 LIMIT 5").bind(file).fetch_all(&state.db).await?;
    for id in ids {
        let n = node(state, id).await?;
        if n.deleted_at.is_none() && level_of(state, auth, &n).await? > Level::None {
            return Ok(true);
        }
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// Browsing
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct Entry {
    id: Uuid,
    kind: String,
    space: String,
    parent_id: Option<Uuid>,
    name: String,
    size_bytes: i64,
    mime_type: Option<String>,
    version: i32,
    owner_name: Option<String>,
    updated_by_name: Option<String>,
    updated_at: DateTime<Utc>,
    shared: bool,
}

const ENTRY_SELECT: &str = "SELECT n.id, n.kind, n.space, n.parent_id, n.name, n.size_bytes, n.mime_type, n.version,
    trim(o.first_name || ' ' || o.last_name) AS owner_name, trim(u.first_name || ' ' || u.last_name) AS updated_by_name, n.updated_at,
    EXISTS (SELECT 1 FROM drive_shares s WHERE s.node_id = n.id) AS shared
    FROM drive_nodes n LEFT JOIN users o ON o.id = n.owner_id LEFT JOIN users u ON u.id = n.updated_by";

#[derive(Deserialize)]
struct BrowseQuery {
    /// personal | company | shared | group (with project_id)
    space: Option<String>,
    folder: Option<Uuid>,
    project_id: Option<Uuid>,
    /// Shared with me: only what this person shares (their drive, as I see it).
    owner: Option<Uuid>,
}

async fn browse(State(state): State<AppState>, auth: AuthUser, Query(q): Query<BrowseQuery>) -> AppResult<Json<Value>> {
    let mut group = q.project_id.filter(|_| q.folder.is_none() && q.space.as_deref() == Some("group"));
    let (items, path, level): (Vec<Entry>, Vec<Value>, Level) = match q.folder {
        Some(f) => {
            let (n, l) = need(&state, &auth, f, Level::View).await?;
            group = n.project_id;
            if n.kind != "folder" || n.deleted_at.is_some() {
                return Err(AppError::NotFound("folder"));
            }
            let items = sqlx::query_as(sqlx::AssertSqlSafe(format!("{ENTRY_SELECT} WHERE n.parent_id = $1 AND n.deleted_at IS NULL ORDER BY n.kind DESC, lower(n.name)")))
                .bind(f)
                .fetch_all(&state.db)
                .await?;
            // The way up, as far as the user can see.
            let up: Vec<(Uuid, String)> = sqlx::query_as(
                "WITH RECURSIVE up AS (SELECT id, parent_id, name, 0 AS n FROM drive_nodes WHERE id = $1
                                       UNION ALL SELECT d.id, d.parent_id, d.name, up.n + 1 FROM drive_nodes d JOIN up ON d.id = up.parent_id)
                 SELECT id, name FROM up ORDER BY n DESC",
            )
            .bind(f)
            .fetch_all(&state.db)
            .await?;
            let mut path = Vec::new();
            for (id, name) in up {
                let pn = node(&state, id).await?;
                if level_of(&state, &auth, &pn).await? > Level::None {
                    path.push(json!({ "id": id, "name": name }));
                }
            }
            (items, path, l)
        }
        None => match q.space.as_deref().unwrap_or("personal") {
            "company" => (
                sqlx::query_as(sqlx::AssertSqlSafe(format!("{ENTRY_SELECT} WHERE n.space = 'company' AND n.parent_id IS NULL AND n.deleted_at IS NULL ORDER BY n.kind DESC, lower(n.name)")))
                    .fetch_all(&state.db)
                    .await?,
                vec![],
                if manager(&auth) { Level::Own } else { Level::Edit },
            ),
            "group" => {
                let project = q.project_id.ok_or_else(|| AppError::bad("which group?"))?;
                let l = group_level(&state, &auth, project, None).await?;
                if l == Level::None {
                    return Err(AppError::NotFound("project"));
                }
                (
                    sqlx::query_as(sqlx::AssertSqlSafe(format!("{ENTRY_SELECT} WHERE n.space = 'group' AND n.project_id = $1 AND n.parent_id IS NULL AND n.deleted_at IS NULL ORDER BY n.kind DESC, lower(n.name)")))
                        .bind(project)
                        .fetch_all(&state.db)
                        .await?,
                    vec![],
                    l,
                )
            }
            "shared" => (
                sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "{ENTRY_SELECT} WHERE n.deleted_at IS NULL AND n.owner_id <> $1 AND ($2::uuid IS NULL OR n.owner_id = $2) AND n.id IN (
                        SELECT s.node_id FROM drive_shares s WHERE s.user_id = $1 OR s.department_id IN (
                            WITH RECURSIVE mine AS (SELECT department_id AS id FROM user_departments WHERE user_id = $1
                                                    UNION SELECT d.parent_id FROM departments d JOIN mine m ON d.id = m.id WHERE d.parent_id IS NOT NULL)
                            SELECT id FROM mine))
                     ORDER BY n.kind DESC, lower(n.name)"
                )))
                .bind(auth.id)
                .bind(q.owner)
                .fetch_all(&state.db)
                .await?,
                vec![],
                Level::View,
            ),
            _ => (
                sqlx::query_as(sqlx::AssertSqlSafe(format!("{ENTRY_SELECT} WHERE n.space = 'personal' AND n.owner_id = $1 AND n.parent_id IS NULL AND n.deleted_at IS NULL ORDER BY n.kind DESC, lower(n.name)")))
                    .bind(auth.id)
                    .fetch_all(&state.db)
                    .await?,
                vec![],
                Level::Own,
            ),
        },
    };
    // A workgroup's files: which group.
    let project = match group {
        Some(g) => sqlx::query_scalar::<_, String>("SELECT name FROM projects WHERE id = $1").bind(g).fetch_optional(&state.db).await?.map(|n| json!({ "id": g, "name": n })),
        None => None,
    };
    Ok(Json(json!({ "items": items, "path": path, "can_add": level >= Level::Edit, "can_manage": level == Level::Own, "project": project })))
}

/// Where new things go. A folder's space is inherited.
struct Place {
    space: String,
    owner: Option<Uuid>,
    project: Option<Uuid>,
    parent: Option<Uuid>,
}

async fn place(state: &AppState, auth: &AuthUser, space: Option<&str>, project: Option<Uuid>, parent: Option<Uuid>) -> AppResult<Place> {
    match parent {
        Some(p) => {
            let (n, _) = need(state, auth, p, Level::Edit).await?;
            if n.kind != "folder" || n.deleted_at.is_some() {
                return Err(AppError::bad("that is not a folder"));
            }
            Ok(Place { space: n.space, owner: n.owner_id, project: n.project_id, parent: Some(p) })
        }
        None => match space.unwrap_or("personal") {
            "company" => Ok(Place { space: "company".into(), owner: None, project: None, parent: None }),
            "personal" => Ok(Place { space: "personal".into(), owner: Some(auth.id), project: None, parent: None }),
            "group" => {
                let project = project.ok_or_else(|| AppError::bad("which group?"))?;
                match group_level(state, auth, project, None).await? {
                    Level::None => Err(AppError::NotFound("project")),
                    Level::View => Err(AppError::Forbidden),
                    _ => Ok(Place { space: "group".into(), owner: None, project: Some(project), parent: None }),
                }
            }
            _ => Err(AppError::bad("space must be personal, company or group")),
        },
    }
}

fn clean_name(n: &str) -> AppResult<String> {
    let n = required("name", n, 200)?;
    if n.contains('/') || n.contains('\\') || n == "." || n == ".." {
        return Err(AppError::bad("names can't contain slashes"));
    }
    Ok(n)
}

/// A live item with this name in the same folder.
async fn sibling(state: &AppState, at: &Place, name: &str, except: Option<Uuid>) -> AppResult<Option<(Uuid, String)>> {
    Ok(sqlx::query_as(
        "SELECT id, kind FROM drive_nodes WHERE space = $1 AND owner_id IS NOT DISTINCT FROM $2 AND parent_id IS NOT DISTINCT FROM $3
           AND lower(name) = lower($4) AND deleted_at IS NULL AND id IS DISTINCT FROM $5 AND project_id IS NOT DISTINCT FROM $6 LIMIT 1",
    )
    .bind(&at.space)
    .bind(at.owner)
    .bind(at.parent)
    .bind(name)
    .bind(except)
    .bind(at.project)
    .fetch_optional(&state.db)
    .await?)
}

#[derive(Deserialize)]
struct FolderInput {
    space: Option<String>,
    project_id: Option<Uuid>,
    parent_id: Option<Uuid>,
    name: String,
}

async fn create_folder(State(state): State<AppState>, auth: AuthUser, Json(b): Json<FolderInput>) -> AppResult<Json<Value>> {
    let name = clean_name(&b.name)?;
    let at = place(&state, &auth, b.space.as_deref(), b.project_id, b.parent_id).await?;
    if sibling(&state, &at, &name, None).await?.is_some() {
        return Err(AppError::bad(format!("“{name}” is already here")));
    }
    let id: Uuid = sqlx::query_scalar("INSERT INTO drive_nodes (kind, space, owner_id, parent_id, name, created_by, updated_by, project_id) VALUES ('folder', $1, $2, $3, $4, $5, $5, $6) RETURNING id")
        .bind(&at.space)
        .bind(at.owner)
        .bind(at.parent)
        .bind(&name)
        .bind(auth.id)
        .bind(at.project)
        .fetch_one(&state.db)
        .await?;
    get_node(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct FileInput {
    space: Option<String>,
    project_id: Option<Uuid>,
    parent_id: Option<Uuid>,
    /// An upload of the user's (POST /files).
    file_id: Uuid,
    name: Option<String>,
}

/// Adds an uploaded file; a file of the same name in the folder gets it as a new version.
async fn add_file(State(state): State<AppState>, auth: AuthUser, Json(b): Json<FileInput>) -> AppResult<Json<Value>> {
    let (fname, size, mime, owner): (String, i64, String, Option<Uuid>) = sqlx::query_as("SELECT original_name, size_bytes, mime_type, owner_id FROM files WHERE id = $1")
        .bind(b.file_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("file"))?;
    if owner != Some(auth.id) {
        return Err(AppError::bad("add a file you uploaded"));
    }
    let used: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM drive_nodes WHERE file_id = $1) OR EXISTS (SELECT 1 FROM drive_versions WHERE file_id = $1)").bind(b.file_id).fetch_one(&state.db).await?;
    if used {
        return Err(AppError::bad("that upload is already in the drive"));
    }
    let name = clean_name(b.name.as_deref().unwrap_or(&fname))?;
    let at = place(&state, &auth, b.space.as_deref(), b.project_id, b.parent_id).await?;
    let id = match sibling(&state, &at, &name, None).await? {
        Some((_, kind)) if kind == "folder" => return Err(AppError::bad(format!("a folder called “{name}” is already here"))),
        Some((existing, _)) => {
            need(&state, &auth, existing, Level::Edit).await?;
            let mut tx = state.db.begin().await?;
            let v: i32 = sqlx::query_scalar("UPDATE drive_nodes SET version = version + 1, file_id = $2, size_bytes = $3, mime_type = $4, updated_by = $5, updated_at = now() WHERE id = $1 RETURNING version")
                .bind(existing)
                .bind(b.file_id)
                .bind(size)
                .bind(&mime)
                .bind(auth.id)
                .fetch_one(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO drive_versions (node_id, version, file_id, size_bytes, uploaded_by) VALUES ($1, $2, $3, $4, $5)").bind(existing).bind(v).bind(b.file_id).bind(size).bind(auth.id).execute(&mut *tx).await?;
            tx.commit().await?;
            existing
        }
        None => {
            let mut tx = state.db.begin().await?;
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO drive_nodes (kind, space, owner_id, parent_id, name, file_id, size_bytes, mime_type, created_by, updated_by, project_id)
                 VALUES ('file', $1, $2, $3, $4, $5, $6, $7, $8, $8, $9) RETURNING id",
            )
            .bind(&at.space)
            .bind(at.owner)
            .bind(at.parent)
            .bind(&name)
            .bind(b.file_id)
            .bind(size)
            .bind(&mime)
            .bind(auth.id)
            .bind(at.project)
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query("INSERT INTO drive_versions (node_id, version, file_id, size_bytes, uploaded_by) VALUES ($1, 1, $2, $3, $4)").bind(id).bind(b.file_id).bind(size).bind(auth.id).execute(&mut *tx).await?;
            tx.commit().await?;
            id
        }
    };
    get_node(State(state), auth, Path(id)).await
}

async fn get_node(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (n, l) = need(&state, &auth, id, Level::View).await?;
    let e: Entry = sqlx::query_as(sqlx::AssertSqlSafe(format!("{ENTRY_SELECT} WHERE n.id = $1"))).bind(id).fetch_one(&state.db).await?;
    let mut v = serde_json::to_value(&e).unwrap();
    v["level"] = json!(match l { Level::Own => "own", Level::Edit => "edit", _ => "view" });
    v["deleted"] = json!(n.deleted_at.is_some());
    if n.kind == "file" {
        let versions: Vec<(i32, i64, Option<String>, DateTime<Utc>)> = sqlx::query_as(
            "SELECT v.version, v.size_bytes, trim(u.first_name || ' ' || u.last_name), v.uploaded_at FROM drive_versions v LEFT JOIN users u ON u.id = v.uploaded_by WHERE v.node_id = $1 ORDER BY v.version DESC",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await?;
        v["versions"] = json!(versions.into_iter().map(|(ver, sz, by, at)| json!({ "version": ver, "size_bytes": sz, "by": by, "at": at })).collect::<Vec<_>>());
    }
    if l == Level::Own {
        let shares: Vec<(Option<Uuid>, Option<Uuid>, String, Option<String>)> = sqlx::query_as(
            "SELECT s.user_id, s.department_id, s.level, COALESCE(trim(u.first_name || ' ' || u.last_name), d.name) FROM drive_shares s
             LEFT JOIN users u ON u.id = s.user_id LEFT JOIN departments d ON d.id = s.department_id WHERE s.node_id = $1",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await?;
        v["shares"] = json!(shares.into_iter().map(|(u, d, lv, name)| json!({ "user_id": u, "department_id": d, "level": lv, "name": name })).collect::<Vec<_>>());
        let links: Vec<(Uuid, Option<DateTime<Utc>>, i32, DateTime<Utc>, bool)> =
            sqlx::query_as("SELECT id, expires_at, downloads, created_at, password_hash IS NOT NULL FROM drive_links WHERE node_id = $1 ORDER BY created_at").bind(id).fetch_all(&state.db).await?;
        v["links"] = json!(links.into_iter().map(|(id, exp, d, at, pw)| json!({ "id": id, "expires_at": exp, "downloads": d, "created_at": at, "password": pw })).collect::<Vec<_>>());
    }
    Ok(Json(v))
}

#[derive(Deserialize)]
struct NodePatch {
    name: Option<String>,
    /// Move into this folder (same space); null: the space's top.
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    parent_id: Option<Option<Uuid>>,
}

async fn update_node(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<NodePatch>) -> AppResult<Json<Value>> {
    let (n, _) = need(&state, &auth, id, Level::Own).await?;
    let name = match &b.name {
        Some(x) => clean_name(x)?,
        None => n.name.clone(),
    };
    let parent = match b.parent_id {
        None => n.parent_id,
        Some(None) => None,
        Some(Some(p)) => {
            let (pn, _) = need(&state, &auth, p, Level::Edit).await?;
            if pn.kind != "folder" || pn.space != n.space || pn.owner_id != n.owner_id || pn.project_id != n.project_id {
                return Err(AppError::bad("move within the same drive, into a folder"));
            }
            let cycle: bool = sqlx::query_scalar(
                "WITH RECURSIVE up AS (SELECT id, parent_id FROM drive_nodes WHERE id = $1 UNION ALL SELECT d.id, d.parent_id FROM drive_nodes d JOIN up ON d.id = up.parent_id)
                 SELECT EXISTS (SELECT 1 FROM up WHERE id = $2)",
            )
            .bind(p)
            .bind(id)
            .fetch_one(&state.db)
            .await?;
            if cycle {
                return Err(AppError::bad("a folder can't go inside itself"));
            }
            Some(p)
        }
    };
    if sibling(&state, &Place { space: n.space.clone(), owner: n.owner_id, project: n.project_id, parent }, &name, Some(id)).await?.is_some() {
        return Err(AppError::bad(format!("“{name}” is already there")));
    }
    sqlx::query("UPDATE drive_nodes SET name = $2, parent_id = $3, updated_by = $4, updated_at = now() WHERE id = $1").bind(id).bind(&name).bind(parent).bind(auth.id).execute(&state.db).await?;
    get_node(State(state), auth, Path(id)).await
}

/// To the trash, with everything inside.
async fn remove_node(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&state, &auth, id, Level::Own).await?;
    sqlx::query(
        "WITH RECURSIVE down AS (SELECT id FROM drive_nodes WHERE id = $1 UNION ALL SELECT d.id FROM drive_nodes d JOIN down ON d.parent_id = down.id)
         UPDATE drive_nodes SET deleted_at = now(), deleted_by = $2 WHERE id IN (SELECT id FROM down) AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(auth.id)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "ok": true })))
}

/// What the user put in the trash (the top items deleted).
async fn trash(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let items: Vec<(Uuid, String, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT n.id, n.kind, n.name, n.space, n.deleted_at FROM drive_nodes n
         WHERE n.deleted_at IS NOT NULL AND n.deleted_by = $1
           AND NOT EXISTS (SELECT 1 FROM drive_nodes p WHERE p.id = n.parent_id AND p.deleted_at IS NOT NULL AND p.deleted_by = $1)
         ORDER BY n.deleted_at DESC LIMIT 500",
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items.into_iter().map(|(id, k, n, s, at)| json!({ "id": id, "kind": k, "name": n, "space": s, "deleted_at": at, "purged_on": at + Duration::days(30) })).collect::<Vec<_>>() })))
}

async fn restore_node(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (n, _) = need(&state, &auth, id, Level::Own).await?;
    if n.deleted_at.is_none() {
        return Err(AppError::bad("it is not in the trash"));
    }
    // Back where it was, or to the top when that folder is gone too; renamed if taken.
    let parent = match n.parent_id {
        Some(p) if node(&state, p).await?.deleted_at.is_none() => Some(p),
        _ => None,
    };
    let mut name = n.name.clone();
    if sibling(&state, &Place { space: n.space.clone(), owner: n.owner_id, project: n.project_id, parent }, &name, Some(id)).await?.is_some() {
        name = format!("{name} (restored)");
    }
    sqlx::query(
        "WITH RECURSIVE down AS (SELECT id FROM drive_nodes WHERE id = $1 UNION ALL SELECT d.id FROM drive_nodes d JOIN down ON d.parent_id = down.id)
         UPDATE drive_nodes SET deleted_at = NULL, deleted_by = NULL WHERE id IN (SELECT id FROM down) AND deleted_at >= $2 - interval '1 second'",
    )
    .bind(id)
    .bind(n.deleted_at)
    .execute(&state.db)
    .await?;
    sqlx::query("UPDATE drive_nodes SET parent_id = $2, name = $3 WHERE id = $1").bind(id).bind(parent).bind(&name).execute(&state.db).await?;
    get_node(State(state), auth, Path(id)).await
}

/// Gone for good: the item, everything inside, every version's file.
pub(crate) async fn purge_node(state: &AppState, id: Uuid) -> AppResult<()> {
    purge(state, id).await
}

async fn purge(state: &AppState, id: Uuid) -> AppResult<()> {
    let files: Vec<Uuid> = sqlx::query_scalar(
        "WITH RECURSIVE down AS (SELECT id FROM drive_nodes WHERE id = $1 UNION ALL SELECT d.id FROM drive_nodes d JOIN down ON d.parent_id = down.id)
         SELECT file_id FROM drive_versions WHERE node_id IN (SELECT id FROM down) AND file_id IS NOT NULL",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    sqlx::query("DELETE FROM drive_nodes WHERE id = $1").bind(id).execute(&state.db).await?;
    for f in files {
        delete_stored(state, f).await;
    }
    Ok(())
}

async fn destroy(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (n, _) = need(&state, &auth, id, Level::Own).await?;
    if n.deleted_at.is_none() {
        return Err(AppError::bad("put it in the trash first"));
    }
    purge(&state, id).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Daily: empties what has been in the trash for 30 days.
pub async fn run_purge(state: &AppState) -> Result<()> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT n.id FROM drive_nodes n WHERE n.deleted_at < now() - interval '30 days'
           AND NOT EXISTS (SELECT 1 FROM drive_nodes p WHERE p.id = n.parent_id AND p.deleted_at < now() - interval '30 days') LIMIT 1000",
    )
    .fetch_all(&state.db)
    .await?;
    for id in ids {
        purge(state, id).await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
    }
    Ok(())
}


#[derive(Deserialize)]
struct DownloadQuery {
    version: Option<i32>,
    name: Option<String>,
}

async fn download(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<DownloadQuery>, headers: axum::http::HeaderMap) -> AppResult<Response> {
    let (n, _) = need(&state, &auth, id, Level::View).await?;
    let file = match q.version {
        Some(v) => sqlx::query_scalar::<_, Option<Uuid>>("SELECT file_id FROM drive_versions WHERE node_id = $1 AND version = $2").bind(id).bind(v).fetch_optional(&state.db).await?.flatten(),
        None => n.file_id,
    }
    .ok_or(AppError::NotFound("file"))?;
    let name = crate::routes::files::NameQuery { name: q.name }.name(&n.name);
    crate::routes::files::stream_id(&state, file, &name, &headers).await
}

/// Brings an old version back as the newest one.
async fn restore_version(State(state): State<AppState>, auth: AuthUser, Path((id, v)): Path<(Uuid, i32)>) -> AppResult<Json<Value>> {
    need(&state, &auth, id, Level::Edit).await?;
    let (file, size): (Option<Uuid>, i64) = sqlx::query_as("SELECT file_id, size_bytes FROM drive_versions WHERE node_id = $1 AND version = $2")
        .bind(id)
        .bind(v)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("version"))?;
    let file = file.ok_or(AppError::NotFound("file"))?;
    let mime: String = sqlx::query_scalar("SELECT mime_type FROM files WHERE id = $1").bind(file).fetch_one(&state.db).await?;
    let mut tx = state.db.begin().await?;
    let nv: i32 = sqlx::query_scalar("UPDATE drive_nodes SET version = version + 1, file_id = $2, size_bytes = $3, mime_type = $4, updated_by = $5, updated_at = now() WHERE id = $1 RETURNING version")
        .bind(id)
        .bind(file)
        .bind(size)
        .bind(&mime)
        .bind(auth.id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO drive_versions (node_id, version, file_id, size_bytes, uploaded_by) VALUES ($1, $2, $3, $4, $5)").bind(id).bind(nv).bind(file).bind(size).bind(auth.id).execute(&mut *tx).await?;
    tx.commit().await?;
    get_node(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct ShareInput {
    user_id: Option<Uuid>,
    department_id: Option<Uuid>,
    level: String,
}

/// Replaces who an item in My drive is shared with.
async fn set_shares(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Vec<ShareInput>>) -> AppResult<Json<Value>> {
    let (n, _) = need(&state, &auth, id, Level::Own).await?;
    if n.space != "personal" {
        return Err(AppError::bad("the company drive is open to everyone already"));
    }
    if b.len() > 200 {
        return Err(AppError::bad("at most 200 shares"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM drive_shares WHERE node_id = $1").bind(id).execute(&mut *tx).await?;
    for s in &b {
        if !["view", "edit"].contains(&s.level.as_str()) || s.user_id.is_some() == s.department_id.is_some() || s.user_id == Some(auth.id) {
            return Err(AppError::bad("share with a person or a department, to view or to edit"));
        }
        sqlx::query("INSERT INTO drive_shares (node_id, user_id, department_id, level) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(s.user_id)
            .bind(s.department_id)
            .bind(&s.level)
            .execute(&mut *tx)
            .await
            .map_err(|e| match e {
                sqlx::Error::Database(d) if d.is_foreign_key_violation() => AppError::bad("that person or department doesn't exist"),
                e => e.into(),
            })?;
    }
    tx.commit().await?;
    // People it is newly shared with hear about it.
    let who: Vec<Uuid> = b.iter().filter_map(|s| s.user_id).collect();
    let me: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await?;
    crate::notify::notify(&state, &who, crate::notify::NewNotification { kind: "drive.shared", title: format!("{me} shared “{}” with you", n.name), body: None, link: Some(if n.kind == "folder" { format!("/drive?folder={id}") } else { format!("/drive?file={id}") }), actor: Some(auth.id) }).await;
    get_node(State(state), auth, Path(id)).await
}

fn hash(t: &str) -> String {
    hex::encode(Sha256::digest(t.as_bytes()))
}

#[derive(Deserialize)]
struct LinkInput {
    expires_days: Option<i64>,
    /// Asked for before anything is shown.
    password: Option<String>,
}

async fn create_link(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<LinkInput>) -> AppResult<Json<Value>> {
    crate::routes::settings::require_public_links(&state).await?;
    need(&state, &auth, id, Level::Own).await?;
    if b.expires_days.is_some_and(|d| !(1..=365).contains(&d)) {
        return Err(AppError::bad("a link lasts 1 to 365 days, or until removed"));
    }
    let pw = match b.password.filter(|p| !p.is_empty()) {
        Some(p) if p.len() < 4 => return Err(AppError::bad("the password needs at least 4 characters")),
        Some(p) => Some(crate::auth::password::hash_password(p).await?),
        None => None,
    };
    let token = random_token(24);
    sqlx::query("INSERT INTO drive_links (node_id, token_hash, expires_at, created_by, password_hash) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(hash(&token))
        .bind(b.expires_days.map(|d| Utc::now() + Duration::days(d)))
        .bind(auth.id)
        .bind(pw)
        .execute(&state.db)
        .await?;
    let mut v = get_node(State(state.clone()), auth, Path(id)).await?.0;
    v["url"] = json!(format!("{}/d/{token}", state.config.public_url));
    Ok(Json(v))
}

async fn delete_link(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let node_id: Uuid = sqlx::query_scalar("SELECT node_id FROM drive_links WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("link"))?;
    need(&state, &auth, node_id, Level::Own).await?;
    sqlx::query("DELETE FROM drive_links WHERE id = $1").bind(id).execute(&state.db).await?;
    get_node(State(state), auth, Path(node_id)).await
}

/// The item behind a link: not expired, the password right (`X-Link-Password`), and not too
/// many wrong guesses from one address.
async fn link_node(state: &AppState, token: &str, headers: &axum::http::HeaderMap, ip: Option<&str>) -> AppResult<(Uuid, Node)> {
    use redis::AsyncCommands;
    if !crate::routes::settings::public_links_allowed(state).await? {
        return Err(AppError::NotFound("link"));
    }
    let (lid, nid, pw): (Uuid, Uuid, Option<String>) = sqlx::query_as("SELECT id, node_id, password_hash FROM drive_links WHERE token_hash = $1 AND (expires_at IS NULL OR expires_at > now())")
        .bind(hash(token))
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("link"))?;
    if let Some(h) = pw {
        let mut redis = state.redis.clone();
        let key = format!("drive:pw:{}:{}", hash(token), ip.unwrap_or("?"));
        let fails: i64 = redis.get(&key).await.unwrap_or(0);
        if fails >= 10 {
            return Err(AppError::TooManyRequests("too many wrong passwords; try again later".into()));
        }
        let given = headers.get("X-Link-Password").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        if given.is_empty() {
            return Err(AppError::InvalidCredentials("this link needs a password".into()));
        }
        if !crate::auth::password::verify_password(given, h).await? {
            let _: Result<i64, _> = redis.incr(&key, 1).await;
            let _: Result<(), _> = redis.expire(&key, 900).await;
            return Err(AppError::InvalidCredentials("wrong password".into()));
        }
    }
    let n = node(state, nid).await?;
    if n.deleted_at.is_some() {
        return Err(AppError::NotFound("link"));
    }
    Ok((lid, n))
}

/// Something inside a linked folder (any depth), not in the trash.
async fn inside(state: &AppState, folder: Uuid, id: Uuid) -> AppResult<Node> {
    let ok: bool = sqlx::query_scalar(
        "WITH RECURSIVE down AS (SELECT id FROM drive_nodes WHERE id = $1 UNION ALL SELECT d.id FROM drive_nodes d JOIN down ON d.parent_id = down.id WHERE d.deleted_at IS NULL)
         SELECT EXISTS (SELECT 1 FROM down WHERE id = $2)",
    )
    .bind(folder)
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    let n = node(state, id).await?;
    if !ok || n.deleted_at.is_some() {
        return Err(AppError::NotFound("file"));
    }
    Ok(n)
}

#[derive(Deserialize, Default)]
struct PublicQuery {
    /// In a linked folder: a folder or file inside it.
    item: Option<Uuid>,
}

async fn public_info(State(state): State<AppState>, ClientIp(ip): ClientIp, headers: axum::http::HeaderMap, Path(token): Path<String>, Query(q): Query<PublicQuery>) -> AppResult<Json<Value>> {
    let (_, root) = link_node(&state, &token, &headers, ip.as_deref()).await?;
    let n = match q.item {
        Some(i) if root.kind == "folder" => inside(&state, root.id, i).await?,
        _ => root.clone(),
    };
    if n.kind == "folder" {
        let items: Vec<(Uuid, String, String, i64, Option<String>)> = sqlx::query_as(
            "SELECT id, name, kind, size_bytes, mime_type FROM drive_nodes WHERE parent_id = $1 AND deleted_at IS NULL ORDER BY kind DESC, lower(name) LIMIT 2000",
        )
        .bind(n.id)
        .fetch_all(&state.db)
        .await?;
        // The way back up to the linked folder.
        let path: Vec<(Uuid, String)> = sqlx::query_as(
            "WITH RECURSIVE up AS (SELECT id, parent_id, name, 0 AS depth FROM drive_nodes WHERE id = $1
                UNION ALL SELECT d.id, d.parent_id, d.name, up.depth + 1 FROM drive_nodes d JOIN up ON d.id = up.parent_id WHERE up.id <> $2)
             SELECT id, name FROM up ORDER BY depth DESC",
        )
        .bind(n.id)
        .bind(root.id)
        .fetch_all(&state.db)
        .await?;
        return Ok(Json(json!({
            "kind": "folder", "name": n.name, "root": root.name,
            "path": path.into_iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
            "items": items.into_iter().map(|(id, name, kind, size, mime)| json!({ "id": id, "name": name, "kind": kind, "size_bytes": size, "mime_type": mime })).collect::<Vec<_>>(),
        })));
    }
    let (size, mime): (i64, Option<String>) = sqlx::query_as("SELECT size_bytes, mime_type FROM drive_nodes WHERE id = $1").bind(n.id).fetch_one(&state.db).await?;
    Ok(Json(json!({ "kind": "file", "name": n.name, "size_bytes": size, "mime_type": mime })))
}

async fn public_download(State(state): State<AppState>, ClientIp(ip): ClientIp, Path(token): Path<String>, Query(q): Query<PublicQuery>, headers: axum::http::HeaderMap) -> AppResult<Response> {
    let (lid, root) = link_node(&state, &token, &headers, ip.as_deref()).await?;
    let n = match q.item {
        Some(i) if root.kind == "folder" => inside(&state, root.id, i).await?,
        _ => root,
    };
    if n.kind != "file" {
        return Err(AppError::bad("pick a file in the folder"));
    }
    // A download is counted once, not for every piece a player asks for.
    if headers.get(axum::http::header::RANGE).and_then(|v| v.to_str().ok()).is_none_or(|r| r.starts_with("bytes=0-")) {
        sqlx::query("UPDATE drive_links SET downloads = downloads + 1 WHERE id = $1").bind(lid).execute(&state.db).await?;
    }
    crate::routes::files::stream_id(&state, n.file_id.ok_or(AppError::NotFound("file"))?, &n.name, &headers).await
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

/// By name, over My drive, the company drive and what is shared with me.
async fn search(State(state): State<AppState>, auth: AuthUser, Query(q): Query<SearchQuery>) -> AppResult<Json<Value>> {
    let t = q.q.trim().to_lowercase();
    if t.chars().count() < 2 {
        return Ok(Json(json!({ "items": [] })));
    }
    let rows: Vec<Entry> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ENTRY_SELECT} WHERE n.deleted_at IS NULL AND lower(n.name) LIKE $1 ORDER BY (n.owner_id = $2) DESC NULLS LAST, lower(n.name) LIMIT 300"
    )))
    .bind(format!("%{}%", t.replace('%', "\\%").replace('_', "\\_")))
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::new();
    for e in rows {
        if out.len() >= 100 {
            break;
        }
        let n = node(&state, e.id).await?;
        if level_of(&state, &auth, &n).await? > Level::None {
            out.push(e);
        }
    }
    Ok(Json(json!({ "items": out })))
}

// ---------------------------------------------------------------------------
// For the office editor (Collabora)
// ---------------------------------------------------------------------------

/// A drive file as the office editor sees it.
pub(crate) struct OfficeFile {
    pub name: String,
    pub file_id: Uuid,
    pub size: i64,
    pub version: i32,
    pub updated_at: DateTime<Utc>,
    pub owner: Option<Uuid>,
    pub can_edit: bool,
}

/// The file behind a drive item, if the person may at least see it.
pub(crate) async fn office_file(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<OfficeFile> {
    let (n, l) = need(state, auth, id, Level::View).await?;
    if n.kind != "file" || n.deleted_at.is_some() {
        return Err(AppError::NotFound("file"));
    }
    let (size, version, updated_at): (i64, i32, DateTime<Utc>) = sqlx::query_as("SELECT size_bytes, version, updated_at FROM drive_nodes WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    Ok(OfficeFile { name: n.name, file_id: n.file_id.ok_or(AppError::NotFound("file"))?, size, version, updated_at, owner: n.owner_id.or(n.created_by), can_edit: l >= Level::Edit })
}

/// Saves edited content as the item's newest version.
pub(crate) async fn save_version(state: &AppState, auth: &AuthUser, id: Uuid, bytes: &[u8]) -> AppResult<(i32, DateTime<Utc>)> {
    let (n, _) = need(state, auth, id, Level::Edit).await?;
    let mime: Option<String> = sqlx::query_scalar("SELECT mime_type FROM drive_nodes WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let mime = mime.unwrap_or_else(|| "application/octet-stream".into());
    let file = crate::routes::files::store_bytes(state, Some(auth.id), &n.name, &mime, bytes).await?;
    let mut tx = state.db.begin().await?;
    let (v, at): (i32, DateTime<Utc>) = sqlx::query_as(
        "UPDATE drive_nodes SET version = version + 1, file_id = $2, size_bytes = $3, updated_by = $4, updated_at = now() WHERE id = $1 RETURNING version, updated_at",
    )
    .bind(id)
    .bind(file)
    .bind(bytes.len() as i64)
    .bind(auth.id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO drive_versions (node_id, version, file_id, size_bytes, uploaded_by) VALUES ($1, $2, $3, $4, $5)").bind(id).bind(v).bind(file).bind(bytes.len() as i64).bind(auth.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((v, at))
}
