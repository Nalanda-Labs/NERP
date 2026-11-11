//! Knowledge base: spaces of pages (handbooks, how-tos, policies), written in Markdown.
//!
//! A space is read by everyone or by chosen people and departments (a department includes its
//! sub-departments), and edited by its editors (chosen the same way), whoever made it, and
//! people with "Manage the knowledge base". Pages form a tree inside a space. Every save
//! keeps a version; any version can be looked at and restored. Saving over someone else's
//! newer version is refused, so nobody's changes are lost silently. Search covers the pages
//! you can read.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/kb/spaces", get(list_spaces).post(create_space))
        .route("/kb/spaces/{id}", get(get_space).patch(update_space).delete(delete_space))
        .route("/kb/spaces/{id}/pages", post(create_page))
        .route("/kb/pages/{id}", get(get_page).patch(update_page).delete(delete_page))
        .route("/kb/pages/{id}/versions", get(versions))
        .route("/kb/pages/{id}/versions/{v}", get(version))
        .route("/kb/pages/{id}/versions/{v}/restore", post(restore))
        .route("/kb/search", get(search))
        .route("/kb/preview", post(preview))
        .route("/kb/pages/{id}/files", get(page_files).post(attach_file))
}

fn manager(auth: &AuthUser) -> bool {
    auth.can(perm::KB)
}

/// The user's departments and their parents (a space opened to a department reaches its
/// sub-departments).
const MY_DEPTS: &str = "WITH RECURSIVE mine AS (
    SELECT department_id AS id FROM user_departments WHERE user_id = $1
    UNION SELECT d.parent_id FROM departments d JOIN mine m ON d.id = m.id WHERE d.parent_id IS NOT NULL)
    SELECT id FROM mine";

/// SQL: user $1 (manager: $2) has `level` on space s.
fn access_sql(level: &str) -> String {
    let base = format!(
        "EXISTS (SELECT 1 FROM kb_access a WHERE a.space_id = s.id AND a.level {} AND (a.user_id = $1 OR a.department_id IN ({MY_DEPTS})))",
        if level == "edit" { "= 'edit'" } else { "IN ('read', 'edit')" }
    );
    if level == "edit" { format!("($2 OR s.created_by = $1 OR {base})") } else { format!("($2 OR s.to_all OR s.created_by = $1 OR {base})") }
}

/// A heading's or footnote's anchor: lower-case letters, digits and dashes.
fn slug(text: &str) -> String {
    let s: String = text.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|x| !x.is_empty()).collect::<Vec<_>>().join("-");
    s.chars().take(60).collect()
}

/// Markdown to safe HTML: GitHub-style tables, task lists, strikethrough, footnotes and
/// callouts (`> [!NOTE]`), headings with anchors (`#h-…`) and fenced code marked with its
/// language for highlighting. The result is cleaned: only those ids and classes survive.
pub fn render(md: &str) -> String {
    use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag, TagEnd};
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES | Options::ENABLE_GFM;
    let mut events: Vec<Event> = Parser::new_ext(md, opts).collect();
    // Heading anchors from their text (unique on the page); footnote labels made safe.
    let mut used = std::collections::HashMap::<String, usize>::new();
    let mut i = 0;
    while i < events.len() {
        if let Event::Start(Tag::Heading { .. }) = &events[i] {
            let mut text = String::new();
            let mut j = i + 1;
            while j < events.len() && !matches!(events[j], Event::End(TagEnd::Heading(_))) {
                if let Event::Text(t) | Event::Code(t) = &events[j] {
                    text.push_str(t);
                }
                j += 1;
            }
            let base = { let s = slug(&text); if s.is_empty() { "section".to_string() } else { s } };
            let n = used.entry(base.clone()).or_insert(0);
            *n += 1;
            let anchor = if *n == 1 { format!("h-{base}") } else { format!("h-{base}-{n}") };
            if let Event::Start(Tag::Heading { id, classes, attrs, .. }) = &mut events[i] {
                *id = Some(CowStr::from(anchor));
                classes.clear();
                attrs.clear();
            }
        }
        match &mut events[i] {
            Event::FootnoteReference(l) => *l = CowStr::from(format!("fn-{}", slug(l))),
            Event::Start(Tag::FootnoteDefinition(l)) => *l = CowStr::from(format!("fn-{}", slug(l))),
            _ => {}
        }
        i += 1;
    }
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    // Table columns are aligned with a style attribute, which cleaning removes: use `align`.
    for side in ["left", "center", "right"] {
        out = out.replace(&format!(r#" style="text-align: {side}""#), &format!(r#" align="{side}""#));
    }
    let safe_id = |v: &str| (v.starts_with("h-") || v.starts_with("fn-")) && v.len() <= 80 && v.chars().all(|c| c.is_alphanumeric() || c == '-');
    let clean = ammonia::Builder::default()
        .add_generic_attributes(["align"])
        .add_tags(["input"])
        .add_tag_attributes("input", ["type", "checked", "disabled"])
        .add_tag_attributes("code", ["class"])
        .add_tag_attributes("blockquote", ["class"])
        .add_tag_attributes("div", ["class", "id"])
        .add_tag_attributes("sup", ["class", "id"])
        .add_tag_attributes("li", ["id"])
        .add_tag_attributes("h1", ["id"]).add_tag_attributes("h2", ["id"]).add_tag_attributes("h3", ["id"])
        .add_tag_attributes("h4", ["id"]).add_tag_attributes("h5", ["id"]).add_tag_attributes("h6", ["id"])
        .attribute_filter(move |el, attr, value| match (el, attr) {
            (_, "id") => safe_id(value).then(|| value.into()),
            ("code", "class") => value
                .strip_prefix("language-")
                .filter(|l| !l.is_empty() && l.len() <= 30 && l.chars().all(|c| c.is_ascii_alphanumeric() || "+#-_".contains(c)))
                .map(|_| value.into()),
            ("blockquote", "class") => ["markdown-alert-note", "markdown-alert-tip", "markdown-alert-important", "markdown-alert-warning", "markdown-alert-caution"]
                .contains(&value)
                .then(|| value.into()),
            ("div" | "sup", "class") => ["footnote-definition", "footnote-reference", "footnote-definition-label"].contains(&value).then(|| value.into()),
            ("input", "type") => (value == "checkbox").then(|| value.into()),
            _ => Some(value.into()),
        })
        .link_rel(Some("noopener noreferrer"))
        .clean(&out)
        .to_string();
    drop_inputs(&clean)
}

/// Only checklist boxes are kept; any other `<input>` (from raw HTML) is removed.
fn drop_inputs(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("<input") {
        out.push_str(&rest[..i]);
        let end = rest[i..].find('>').map(|e| i + e + 1).unwrap_or(rest.len());
        let tag = &rest[i..end];
        if tag.contains(r#"type="checkbox""#) {
            out.push_str(tag);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Spaces
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct SpaceRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    to_all: bool,
    pages: i64,
    can_edit: bool,
    updated_at: Option<DateTime<Utc>>,
}

async fn list_spaces(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let items: Vec<SpaceRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT s.id, s.name, s.description, s.to_all, (SELECT COUNT(*) FROM kb_pages p WHERE p.space_id = s.id) AS pages,
                {} AS can_edit, (SELECT MAX(updated_at) FROM kb_pages p WHERE p.space_id = s.id) AS updated_at
         FROM kb_spaces s WHERE {} ORDER BY s.name",
        access_sql("edit"),
        access_sql("read")
    )))
    .bind(auth.id)
    .bind(manager(&auth))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items, "can_create": manager(&auth) })))
}

async fn can(state: &AppState, auth: &AuthUser, space: Uuid, level: &str) -> AppResult<bool> {
    let exists: Option<bool> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT {} FROM kb_spaces s WHERE s.id = $3", access_sql(level))))
        .bind(auth.id)
        .bind(manager(auth))
        .bind(space)
        .fetch_optional(&state.db)
        .await?;
    exists.ok_or(AppError::NotFound("space"))
}

async fn readable(state: &AppState, auth: &AuthUser, space: Uuid) -> AppResult<()> {
    if can(state, auth, space, "read").await? { Ok(()) } else { Err(AppError::NotFound("space")) }
}

async fn editable(state: &AppState, auth: &AuthUser, space: Uuid) -> AppResult<()> {
    readable(state, auth, space).await?;
    if can(state, auth, space, "edit").await? { Ok(()) } else { Err(AppError::Forbidden) }
}

#[derive(Serialize, FromRow)]
struct TreeNode {
    id: Uuid,
    parent_id: Option<Uuid>,
    title: String,
    sort: i32,
}

async fn tree(state: &AppState, space: Uuid) -> AppResult<Vec<TreeNode>> {
    Ok(sqlx::query_as("SELECT id, parent_id, title, sort FROM kb_pages WHERE space_id = $1 ORDER BY sort, title").bind(space).fetch_all(&state.db).await?)
}

async fn get_space(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    readable(&state, &auth, id).await?;
    let (name, description, to_all, created_by): (String, Option<String>, bool, Option<Uuid>) =
        sqlx::query_as("SELECT name, description, to_all, created_by FROM kb_spaces WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let can_edit = can(&state, &auth, id, "edit").await?;
    let mut v = json!({ "id": id, "name": name, "description": description, "to_all": to_all, "can_edit": can_edit, "pages": tree(&state, id).await? });
    // Who reads and edits it: for those who can change that.
    if manager(&auth) || created_by == Some(auth.id) {
        let rows: Vec<(String, Option<Uuid>, Option<Uuid>, Option<String>)> = sqlx::query_as(
            "SELECT a.level, a.user_id, a.department_id, COALESCE(trim(u.first_name || ' ' || u.last_name), d.name) FROM kb_access a
             LEFT JOIN users u ON u.id = a.user_id LEFT JOIN departments d ON d.id = a.department_id WHERE a.space_id = $1",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await?;
        let pick = |level: &str| -> Value {
            json!({
                "users": rows.iter().filter(|r| r.0 == level && r.1.is_some()).map(|r| json!({ "id": r.1, "name": r.3 })).collect::<Vec<_>>(),
                "departments": rows.iter().filter(|r| r.0 == level && r.2.is_some()).map(|r| r.2).collect::<Vec<_>>(),
            })
        };
        v["readers"] = pick("read");
        v["editors"] = pick("edit");
        v["can_manage"] = json!(true);
    }
    Ok(Json(v))
}

#[derive(Deserialize, Default)]
struct Who {
    #[serde(default)]
    users: Vec<Uuid>,
    #[serde(default)]
    departments: Vec<Uuid>,
}

#[derive(Deserialize)]
struct SpaceInput {
    name: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    to_all: Option<bool>,
    readers: Option<Who>,
    editors: Option<Who>,
}

async fn check_who(state: &AppState, w: &Who) -> AppResult<()> {
    let u: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND is_active").bind(&w.users).fetch_one(&state.db).await?;
    let d: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM departments WHERE id = ANY($1)").bind(&w.departments).fetch_one(&state.db).await?;
    if u as usize != w.users.len() || d as usize != w.departments.len() {
        return Err(AppError::bad("some of the people or departments don't exist"));
    }
    Ok(())
}

async fn set_access(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, space: Uuid, level: &str, w: &Who) -> AppResult<()> {
    sqlx::query("DELETE FROM kb_access WHERE space_id = $1 AND level = $2").bind(space).bind(level).execute(&mut **tx).await?;
    for u in &w.users {
        sqlx::query("INSERT INTO kb_access (space_id, user_id, level) VALUES ($1, $2, $3)").bind(space).bind(u).bind(level).execute(&mut **tx).await?;
    }
    for d in &w.departments {
        sqlx::query("INSERT INTO kb_access (space_id, department_id, level) VALUES ($1, $2, $3)").bind(space).bind(d).bind(level).execute(&mut **tx).await?;
    }
    Ok(())
}

async fn create_space(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SpaceInput>) -> AppResult<Json<Value>> {
    auth.require(perm::KB)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 120)?;
    let readers = b.readers.unwrap_or_default();
    let editors = b.editors.unwrap_or_default();
    check_who(&state, &readers).await?;
    check_who(&state, &editors).await?;
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO kb_spaces (name, description, to_all, created_by) VALUES ($1, $2, $3, $4) RETURNING id")
        .bind(&name)
        .bind(clean(b.description.flatten()))
        .bind(b.to_all.unwrap_or(true))
        .bind(auth.id)
        .fetch_one(&mut *tx)
        .await?;
    set_access(&mut tx, id, "read", &readers).await?;
    set_access(&mut tx, id, "edit", &editors).await?;
    tx.commit().await?;
    get_space(State(state), auth, Path(id)).await
}

async fn owns_space(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<()> {
    let created_by: Option<Uuid> = sqlx::query_scalar("SELECT created_by FROM kb_spaces WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("space"))?;
    readable(state, auth, id).await?;
    if manager(auth) || created_by == Some(auth.id) { Ok(()) } else { Err(AppError::Forbidden) }
}

async fn update_space(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<SpaceInput>) -> AppResult<Json<Value>> {
    owns_space(&state, &auth, id).await?;
    let name = b.name.as_deref().map(|n| required("name", n, 120)).transpose()?;
    if let Some(w) = &b.readers {
        check_who(&state, w).await?;
    }
    if let Some(w) = &b.editors {
        check_who(&state, w).await?;
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE kb_spaces SET name = COALESCE($2, name), description = CASE WHEN $3 THEN $4 ELSE description END, to_all = COALESCE($5, to_all) WHERE id = $1")
        .bind(id)
        .bind(name)
        .bind(b.description.is_some())
        .bind(clean(b.description.flatten()))
        .bind(b.to_all)
        .execute(&mut *tx)
        .await?;
    if let Some(w) = &b.readers {
        set_access(&mut tx, id, "read", w).await?;
    }
    if let Some(w) = &b.editors {
        set_access(&mut tx, id, "edit", w).await?;
    }
    tx.commit().await?;
    get_space(State(state), auth, Path(id)).await
}

async fn delete_space(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    owns_space(&state, &auth, id).await?;
    sqlx::query("DELETE FROM kb_spaces WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

#[derive(FromRow)]
struct PageRow {
    id: Uuid,
    space_id: Uuid,
    parent_id: Option<Uuid>,
    title: String,
    body: String,
    version: i32,
    updated_by_name: Option<String>,
    updated_at: DateTime<Utc>,
}

async fn page_row(state: &AppState, id: Uuid) -> AppResult<PageRow> {
    sqlx::query_as(
        "SELECT p.id, p.space_id, p.parent_id, p.title, p.body, p.version, trim(u.first_name || ' ' || u.last_name) AS updated_by_name, p.updated_at
         FROM kb_pages p LEFT JOIN users u ON u.id = p.updated_by WHERE p.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("page"))
}

async fn get_page(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let p = page_row(&state, id).await?;
    readable(&state, &auth, p.space_id).await.map_err(|_| AppError::NotFound("page"))?;
    let path: Vec<(Uuid, String)> = sqlx::query_as(
        "WITH RECURSIVE up AS (SELECT id, parent_id, title, 0 AS n FROM kb_pages WHERE id = $1
                               UNION ALL SELECT p.id, p.parent_id, p.title, up.n + 1 FROM kb_pages p JOIN up ON p.id = up.parent_id)
         SELECT id, title FROM up WHERE id <> $1 ORDER BY n DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let children: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, title FROM kb_pages WHERE parent_id = $1 ORDER BY sort, title").bind(id).fetch_all(&state.db).await?;
    let space: String = sqlx::query_scalar("SELECT name FROM kb_spaces WHERE id = $1").bind(p.space_id).fetch_one(&state.db).await?;
    Ok(Json(json!({
        "id": p.id, "space_id": p.space_id, "space_name": space, "parent_id": p.parent_id, "title": p.title, "body": p.body,
        "html": render(&p.body), "version": p.version, "updated_by_name": p.updated_by_name, "updated_at": p.updated_at,
        "can_edit": can(&state, &auth, p.space_id, "edit").await?,
        "path": path.into_iter().map(|(id, t)| json!({ "id": id, "title": t })).collect::<Vec<_>>(),
        "children": children.into_iter().map(|(id, t)| json!({ "id": id, "title": t })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct PageInput {
    title: Option<String>,
    body: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    parent_id: Option<Option<Uuid>>,
    /// The version the editor started from; saving over a newer one is refused.
    version: Option<i32>,
}

const MAX_BODY: usize = 500_000;

async fn check_parent(state: &AppState, space: Uuid, parent: Option<Uuid>, page: Option<Uuid>) -> AppResult<()> {
    let Some(parent) = parent else { return Ok(()) };
    let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM kb_pages WHERE id = $1 AND space_id = $2)").bind(parent).bind(space).fetch_one(&state.db).await?;
    if !ok {
        return Err(AppError::bad("the parent page must be in the same space"));
    }
    if let Some(page) = page {
        // Not under itself.
        let cycle: bool = sqlx::query_scalar(
            "WITH RECURSIVE up AS (SELECT id, parent_id FROM kb_pages WHERE id = $1 UNION ALL SELECT p.id, p.parent_id FROM kb_pages p JOIN up ON p.id = up.parent_id)
             SELECT EXISTS (SELECT 1 FROM up WHERE id = $2)",
        )
        .bind(parent)
        .bind(page)
        .fetch_one(&state.db)
        .await?;
        if cycle {
            return Err(AppError::bad("a page can't go under itself"));
        }
    }
    Ok(())
}

async fn create_page(State(state): State<AppState>, auth: AuthUser, Path(space): Path<Uuid>, Json(b): Json<PageInput>) -> AppResult<Json<Value>> {
    editable(&state, &auth, space).await?;
    let title = required("title", b.title.as_deref().unwrap_or(""), 200)?;
    let body = b.body.unwrap_or_default();
    if body.len() > MAX_BODY {
        return Err(AppError::bad("the page is too long"));
    }
    let parent = b.parent_id.flatten();
    check_parent(&state, space, parent, None).await?;
    let sort: i32 = sqlx::query_scalar("SELECT COALESCE(MAX(sort), 0) + 10 FROM kb_pages WHERE space_id = $1 AND parent_id IS NOT DISTINCT FROM $2").bind(space).bind(parent).fetch_one(&state.db).await?;
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO kb_pages (space_id, parent_id, title, body, sort, created_by, updated_by) VALUES ($1, $2, $3, $4, $5, $6, $6) RETURNING id")
        .bind(space)
        .bind(parent)
        .bind(&title)
        .bind(&body)
        .bind(sort)
        .bind(auth.id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO kb_versions (page_id, version, title, body, saved_by) VALUES ($1, 1, $2, $3, $4)").bind(id).bind(&title).bind(&body).bind(auth.id).execute(&mut *tx).await?;
    tx.commit().await?;
    get_page(State(state), auth, Path(id)).await
}

async fn save(state: &AppState, auth: &AuthUser, p: &PageRow, title: String, body: String, parent: Option<Uuid>) -> AppResult<()> {
    let mut tx = state.db.begin().await?;
    let v: i32 = sqlx::query_scalar("UPDATE kb_pages SET title = $2, body = $3, parent_id = $4, version = version + 1, updated_by = $5, updated_at = now() WHERE id = $1 AND version = $6 RETURNING version")
        .bind(p.id)
        .bind(&title)
        .bind(&body)
        .bind(parent)
        .bind(auth.id)
        .bind(p.version)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::Conflict("someone saved this page meanwhile; reload it and make your change again".into()))?;
    sqlx::query("INSERT INTO kb_versions (page_id, version, title, body, saved_by) VALUES ($1, $2, $3, $4, $5)").bind(p.id).bind(v).bind(&title).bind(&body).bind(auth.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn update_page(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PageInput>) -> AppResult<Json<Value>> {
    let p = page_row(&state, id).await?;
    editable(&state, &auth, p.space_id).await?;
    if b.version.is_some_and(|v| v != p.version) {
        return Err(AppError::Conflict(format!("{} saved a newer version meanwhile; reload the page and make your change again", p.updated_by_name.clone().unwrap_or_else(|| "Someone".into()))));
    }
    let title = match &b.title {
        Some(t) => required("title", t, 200)?,
        None => p.title.clone(),
    };
    let body = b.body.clone().unwrap_or(p.body.clone());
    if body.len() > MAX_BODY {
        return Err(AppError::bad("the page is too long"));
    }
    let parent = match b.parent_id {
        Some(np) => {
            check_parent(&state, p.space_id, np, Some(id)).await?;
            np
        }
        None => p.parent_id,
    };
    if title == p.title && body == p.body && parent == p.parent_id {
        return get_page(State(state), auth, Path(id)).await;
    }
    save(&state, &auth, &p, title, body, parent).await?;
    get_page(State(state), auth, Path(id)).await
}

/// With its sub-pages.
async fn delete_page(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let p = page_row(&state, id).await?;
    editable(&state, &auth, p.space_id).await?;
    sqlx::query("DELETE FROM kb_pages WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true, "space_id": p.space_id, "parent_id": p.parent_id })))
}

async fn versions(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let p = page_row(&state, id).await?;
    readable(&state, &auth, p.space_id).await.map_err(|_| AppError::NotFound("page"))?;
    let items: Vec<(i32, String, Option<String>, DateTime<Utc>, i32)> = sqlx::query_as(
        "SELECT v.version, v.title, trim(u.first_name || ' ' || u.last_name), v.saved_at, length(v.body) FROM kb_versions v LEFT JOIN users u ON u.id = v.saved_by
         WHERE v.page_id = $1 ORDER BY v.version DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": items.into_iter().map(|(v, t, by, at, len)| json!({ "version": v, "title": t, "saved_by": by, "saved_at": at, "length": len })).collect::<Vec<_>>() })))
}

async fn version(State(state): State<AppState>, auth: AuthUser, Path((id, v)): Path<(Uuid, i32)>) -> AppResult<Json<Value>> {
    let p = page_row(&state, id).await?;
    readable(&state, &auth, p.space_id).await.map_err(|_| AppError::NotFound("page"))?;
    let (title, body, at): (String, String, DateTime<Utc>) = sqlx::query_as("SELECT title, body, saved_at FROM kb_versions WHERE page_id = $1 AND version = $2")
        .bind(id)
        .bind(v)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("version"))?;
    Ok(Json(json!({ "version": v, "title": title, "body": body, "html": render(&body), "saved_at": at })))
}

/// Restoring saves the old version as a new one: nothing is lost.
async fn restore(State(state): State<AppState>, auth: AuthUser, Path((id, v)): Path<(Uuid, i32)>) -> AppResult<Json<Value>> {
    let p = page_row(&state, id).await?;
    editable(&state, &auth, p.space_id).await?;
    let (title, body): (String, String) = sqlx::query_as("SELECT title, body FROM kb_versions WHERE page_id = $1 AND version = $2")
        .bind(id)
        .bind(v)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("version"))?;
    let parent = p.parent_id;
    save(&state, &auth, &p, title, body, parent).await?;
    get_page(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct PreviewBody {
    body: String,
}

/// The editor's preview: the same rendering as a saved page.
async fn preview(_auth: AuthUser, Json(b): Json<PreviewBody>) -> AppResult<Json<Value>> {
    if b.body.len() > MAX_BODY {
        return Err(AppError::bad("the page is too long"));
    }
    Ok(Json(json!({ "html": render(&b.body) })))
}

// ---------------------------------------------------------------------------
// Pictures and files in pages
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AttachInput {
    file_id: Uuid,
}

/// Places an uploaded file in a page (its editors only; the file must be theirs), so the
/// page's readers may open it.
async fn attach_file(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<AttachInput>) -> AppResult<Json<Value>> {
    let p = page_row(&state, id).await?;
    editable(&state, &auth, p.space_id).await?;
    if !crate::routes::files::may_read(&state, &auth, b.file_id).await? {
        return Err(AppError::NotFound("file"));
    }
    sqlx::query("INSERT INTO kb_files (page_id, file_id, added_by) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING").bind(id).bind(b.file_id).bind(auth.id).execute(&state.db).await?;
    let (name, mime): (String, String) = sqlx::query_as("SELECT original_name, mime_type FROM files WHERE id = $1").bind(b.file_id).fetch_one(&state.db).await?;
    Ok(Json(json!({ "id": b.file_id, "name": name, "mime_type": mime, "url": format!("/api/files/{}/content", b.file_id) })))
}

async fn page_files(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let p = page_row(&state, id).await?;
    readable(&state, &auth, p.space_id).await.map_err(|_| AppError::NotFound("page"))?;
    let rows: Vec<(Uuid, String, String, i64)> = sqlx::query_as(
        "SELECT f.id, f.original_name, f.mime_type, f.size_bytes FROM kb_files k JOIN files f ON f.id = k.file_id WHERE k.page_id = $1 ORDER BY k.created_at",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(fid, n, m, s)| json!({ "id": fid, "name": n, "mime_type": m, "size_bytes": s, "url": format!("/api/files/{fid}/content") })).collect::<Vec<_>>() })))
}

/// A file placed in a page the person may read.
pub async fn can_view_file(state: &AppState, auth: &AuthUser, file: Uuid) -> AppResult<bool> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT EXISTS (SELECT 1 FROM kb_files k JOIN kb_pages p ON p.id = k.page_id JOIN kb_spaces s ON s.id = p.space_id WHERE k.file_id = $3 AND {})",
        access_sql("read")
    )))
    .bind(auth.id)
    .bind(manager(auth))
    .bind(file)
    .fetch_one(&state.db)
    .await?)
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

async fn search(State(state): State<AppState>, auth: AuthUser, Query(q): Query<SearchQuery>) -> AppResult<Json<Value>> {
    let t = q.q.trim().to_lowercase();
    if t.chars().count() < 2 {
        return Ok(Json(json!({ "items": [] })));
    }
    let like = format!("%{}%", t.replace('%', "\\%").replace('_', "\\_"));
    let rows: Vec<(Uuid, String, Uuid, String, String)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT p.id, p.title, s.id, s.name, p.body FROM kb_pages p JOIN kb_spaces s ON s.id = p.space_id
         WHERE {} AND (lower(p.title) LIKE $3 OR lower(p.body) LIKE $3)
         ORDER BY (lower(p.title) LIKE $3) DESC, p.updated_at DESC LIMIT 50",
        access_sql("read")
    )))
    .bind(auth.id)
    .bind(manager(&auth))
    .bind(&like)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|(id, title, sid, sname, body)| {
            // A snippet around the first match.
            let lower = body.to_lowercase();
            let snippet = match lower.find(&t) {
                Some(i) => {
                    let start = body.char_indices().map(|(b, _)| b).filter(|b| *b <= i).rev().nth(60).unwrap_or(0);
                    body[start..].chars().take(160).collect::<String>()
                }
                None => body.chars().take(160).collect(),
            };
            json!({ "id": id, "title": title, "space_id": sid, "space_name": sname, "snippet": snippet.replace('\n', " ") })
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_is_rendered_and_made_safe() {
        let html = render("# Title\n\n**bold** [link](https://example.com) <script>alert(1)</script>\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done");
        assert!(html.contains(r#"<h1 id="h-title">Title</h1>"#) && html.contains("<strong>bold</strong>") && html.contains("<table>"));
        assert!(html.contains("rel=\"noopener noreferrer\""));
        assert!(!html.contains("<script"));
        assert!(!render("<img src=x onerror=alert(1)>").contains("onerror"));
        assert!(!render("[x](javascript:alert(1))").contains("javascript:"));
    }

    #[test]
    fn renders_the_full_set_safely() {
        let html = render("# Intro\n\n## Setup\n\n## Setup\n\n- [x] done\n- [ ] open\n\nNote[^a].\n\n[^a]: The note.\n\n> [!WARNING]\n> Careful.\n\n```rust\nfn main() {}\n```\n\n~~old~~ | a | b |\n");
        assert!(html.contains(r#"<h1 id="h-intro">"#), "{html}");
        assert!(html.contains(r#"id="h-setup""#) && html.contains(r#"id="h-setup-2""#), "{html}");
        assert!(html.contains(r#"type="checkbox""#) && html.contains("checked"), "{html}");
        assert!(html.contains("fn-a") && html.contains("footnote-definition"), "{html}");
        assert!(html.contains("markdown-alert-warning"), "{html}");
        assert!(html.contains(r#"class="language-rust""#), "{html}");
        assert!(html.contains("<del>old</del>"), "{html}");
    }

    #[test]
    fn keeps_only_its_own_ids_and_classes() {
        let html = render("<h2 id=\"location\" class=\"x\">a</h2><div id=\"api\" class=\"evil\">b</div><input type=\"text\" value=\"x\"><code class=\"language-x onerror\">c</code>");
        assert!(!html.contains("location") && !html.contains(r#"id="api""#) && !html.contains("evil"), "{html}");
        assert!(!html.contains(r#"type="text""#) && !html.contains("value="), "{html}");
        assert!(!html.contains("language-x onerror"), "{html}");
        let h = render("```js\" onload=alert(1)\nx\n```");
        assert!(!h.contains("onload"), "{h}");
    }
}
