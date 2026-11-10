//! Website builder: simple company sites, served at `/s/<site>/<page>`.
//!
//! A site has a look (colour, font, logo, footer) and pages; a page is a list of blocks (a
//! hero, text in Markdown, features, an image, a gallery, a call to action, contact details,
//! or one of the CRM's web forms). Pages are edited as drafts and published; visitors see
//! only what was published. Images are the editors' own uploads and are served publicly only
//! while a published page (or the logo) uses them. Text is rendered from Markdown and
//! cleaned; links may only point to web addresses, other pages, e-mail or phone numbers.
//! Page views are counted per day.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sites", get(list).post(create))
        .route("/sites/{id}", get(get_site).patch(update_site).delete(delete_site))
        .route("/sites/{id}/pages", post(create_page))
        .route("/sites/{id}/publish", post(publish_site))
        .route("/sites/{id}/stats", get(stats))
        .route("/sites/pages/{id}", get(get_page).patch(update_page).delete(delete_page))
        .route("/sites/pages/{id}/publish", post(publish_page))
        .route("/sites/pages/{id}/unpublish", post(unpublish_page))
        .route("/public/sites/{slug}", get(public_site))
        .route("/public/sites/{slug}/page", get(public_page))
        .route("/public/sites/{slug}/files/{file}", get(public_file))
}

fn require(auth: &AuthUser) -> AppResult<()> {
    auth.require(perm::SITES)
}

// ---------------------------------------------------------------------------
// Blocks
// ---------------------------------------------------------------------------

const KINDS: &[&str] = &["hero", "text", "features", "image", "gallery", "cta", "contact", "form"];

fn text_field(b: &Map<String, Value>, k: &str, max: usize) -> AppResult<Option<String>> {
    match b.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.chars().count() > max {
                return Err(AppError::bad(format!("“{k}” can be at most {max} characters")));
            }
            Ok((!s.is_empty()).then(|| s.to_string()))
        }
        _ => Err(AppError::bad(format!("“{k}” must be text"))),
    }
}

/// Only web addresses, pages of the site, e-mail and phone links.
fn link(s: Option<String>) -> AppResult<Option<String>> {
    match s {
        None => Ok(None),
        Some(l) if l.starts_with("https://") || l.starts_with("http://") || l.starts_with("mailto:") || l.starts_with("tel:") || l.starts_with('#') || (l.starts_with('/') && !l.starts_with("//")) => Ok(Some(l)),
        Some(_) => Err(AppError::bad("links must start with https://, /, #, mailto: or tel:")),
    }
}

fn uuid_field(b: &Map<String, Value>, k: &str) -> AppResult<Option<Uuid>> {
    match b.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        Some(Value::String(s)) => Uuid::parse_str(s).map(Some).map_err(|_| AppError::bad(format!("“{k}” is not valid"))),
        _ => Err(AppError::bad(format!("“{k}” is not valid"))),
    }
}

fn put(o: &mut Map<String, Value>, k: &str, v: Option<String>) {
    if let Some(v) = v {
        o.insert(k.into(), json!(v));
    }
}

/// Checks blocks and keeps only the fields each kind has. Returns them with the images used.
fn clean_blocks(v: &Value) -> AppResult<(Value, Vec<Uuid>)> {
    let list = v.as_array().ok_or_else(|| AppError::bad("blocks must be a list"))?;
    if list.len() > 100 {
        return Err(AppError::bad("at most 100 blocks on a page"));
    }
    let mut out = Vec::new();
    let mut images = Vec::new();
    for raw in list {
        let b = raw.as_object().ok_or_else(|| AppError::bad("a block must be an object"))?;
        let kind = b.get("kind").and_then(Value::as_str).filter(|k| KINDS.contains(k)).ok_or_else(|| AppError::bad("unknown block"))?;
        let mut o = Map::new();
        o.insert("kind".into(), json!(kind));
        match kind {
            "hero" | "cta" => {
                put(&mut o, "heading", text_field(b, "heading", 200)?);
                put(&mut o, "text", text_field(b, "text", 1000)?);
                put(&mut o, "button_label", text_field(b, "button_label", 60)?);
                put(&mut o, "button_link", link(text_field(b, "button_link", 500)?)?);
                if kind == "hero"
                    && let Some(img) = uuid_field(b, "image")? {
                        images.push(img);
                        o.insert("image".into(), json!(img));
                    }
            }
            "text" => put(&mut o, "markdown", text_field(b, "markdown", 50_000)?),
            "features" => {
                put(&mut o, "heading", text_field(b, "heading", 200)?);
                let items = b.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                if items.len() > 12 {
                    return Err(AppError::bad("at most 12 features"));
                }
                let mut clean = Vec::new();
                for i in items {
                    let m = i.as_object().cloned().unwrap_or_default();
                    clean.push(json!({ "title": text_field(&m, "title", 120)?, "text": text_field(&m, "text", 500)? }));
                }
                o.insert("items".into(), json!(clean));
            }
            "image" => {
                let img = uuid_field(b, "image")?.ok_or_else(|| AppError::bad("choose an image"))?;
                images.push(img);
                o.insert("image".into(), json!(img));
                put(&mut o, "caption", text_field(b, "caption", 300)?);
            }
            "gallery" => {
                let ids = b.get("images").and_then(Value::as_array).cloned().unwrap_or_default();
                if ids.len() > 30 {
                    return Err(AppError::bad("at most 30 images in a gallery"));
                }
                let mut clean = Vec::new();
                for i in ids {
                    let id = i.as_str().and_then(|s| Uuid::parse_str(s).ok()).ok_or_else(|| AppError::bad("a gallery image is not valid"))?;
                    images.push(id);
                    clean.push(id);
                }
                o.insert("images".into(), json!(clean));
            }
            "contact" => {
                put(&mut o, "heading", text_field(b, "heading", 200)?);
                put(&mut o, "address", text_field(b, "address", 500)?);
                put(&mut o, "phone", text_field(b, "phone", 50)?);
                put(&mut o, "email", text_field(b, "email", 254)?);
                put(&mut o, "hours", text_field(b, "hours", 300)?);
            }
            _ => {
                put(&mut o, "heading", text_field(b, "heading", 200)?);
                put(&mut o, "form_key", text_field(b, "form_key", 64)?);
            }
        }
        out.push(Value::Object(o));
    }
    Ok((Value::Array(out), images))
}

/// Images must be pictures the editor uploaded, or ones the site already uses.
async fn check_images(state: &AppState, auth: &AuthUser, site: Uuid, ids: &[Uuid]) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let used = site_files(state, site, false).await?;
    let ok: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM files WHERE id = ANY($1) AND mime_type IN ('image/png', 'image/jpeg', 'image/webp', 'image/gif') AND (owner_id = $2 OR id = ANY($3))")
        .bind(ids)
        .bind(auth.id)
        .bind(&used)
        .fetch_all(&state.db)
        .await?;
    if ids.iter().any(|i| !ok.contains(i)) {
        return Err(AppError::bad("images must be PNG, JPEG, WebP or GIF pictures you uploaded"));
    }
    Ok(())
}

/// Files a site uses: in drafts and published pages, or only published (and the logo).
async fn site_files(state: &AppState, site: Uuid, published_only: bool) -> AppResult<Vec<Uuid>> {
    let rows: Vec<(Value, Option<Value>)> = sqlx::query_as("SELECT blocks, published_blocks FROM site_pages WHERE site_id = $1").bind(site).fetch_all(&state.db).await?;
    let settings: Value = sqlx::query_scalar("SELECT settings FROM site_sites WHERE id = $1").bind(site).fetch_optional(&state.db).await?.unwrap_or(json!({}));
    let mut out: Vec<Uuid> = settings["logo_file_id"].as_str().and_then(|s| Uuid::parse_str(s).ok()).into_iter().collect();
    for (draft, live) in rows {
        let lists = if published_only { vec![live] } else { vec![Some(draft), live] };
        for list in lists.into_iter().flatten() {
            if let Ok((_, imgs)) = clean_blocks(&list) {
                out.extend(imgs);
            }
        }
    }
    Ok(out)
}

/// Blocks as visitors get them: Markdown rendered, images as addresses.
fn render_blocks(slug: &str, blocks: &Value) -> Value {
    let img = |id: &Value| id.as_str().map(|i| format!("/api/public/sites/{slug}/files/{i}"));
    json!(blocks
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut b| {
            if let Some(md) = b.get("markdown").and_then(Value::as_str) {
                b["html"] = json!(crate::kb::render(md));
            }
            if let Some(i) = b.get("image").cloned() {
                b["image_url"] = json!(img(&i));
            }
            if let Some(list) = b.get("images").and_then(Value::as_array).cloned() {
                b["image_urls"] = json!(list.iter().map(img).collect::<Vec<_>>());
            }
            b
        })
        .collect::<Vec<_>>())
}

// ---------------------------------------------------------------------------
// Sites
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct SiteRow {
    id: Uuid,
    name: String,
    slug: String,
    settings: Value,
    pages: i64,
    published: i64,
    updated_at: DateTime<Utc>,
}

const SITE_SELECT: &str = "SELECT s.id, s.name, s.slug, s.settings,
    (SELECT COUNT(*) FROM site_pages p WHERE p.site_id = s.id) AS pages,
    (SELECT COUNT(*) FROM site_pages p WHERE p.site_id = s.id AND p.published_blocks IS NOT NULL) AS published, s.updated_at
    FROM site_sites s";

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require(&auth)?;
    let items: Vec<SiteRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SITE_SELECT} ORDER BY s.name"))).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": items, "base": format!("{}/s/", state.config.public_url) })))
}

fn slug_ok(s: &str) -> AppResult<String> {
    let s = s.trim().to_lowercase();
    if s.len() < 2 || s.len() > 40 || !s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') || s.starts_with('-') || s.ends_with('-') {
        return Err(AppError::bad("the address is 2–40 lowercase letters, digits and dashes"));
    }
    Ok(s)
}

#[derive(Deserialize)]
struct SiteInput {
    name: Option<String>,
    slug: Option<String>,
    settings: Option<Value>,
}

fn clean_settings(v: &Value) -> AppResult<(Value, Option<Uuid>)> {
    let m = v.as_object().cloned().unwrap_or_default();
    let color = text_field(&m, "color", 7)?.unwrap_or_else(|| "#1a73e8".into());
    if !(color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit())) {
        return Err(AppError::bad("colour must look like #1a73e8"));
    }
    let font = text_field(&m, "font", 10)?.filter(|f| f == "serif").unwrap_or_else(|| "sans".into());
    let logo = uuid_field(&m, "logo_file_id")?;
    Ok((json!({ "color": color.to_lowercase(), "font": font, "logo_file_id": logo, "footer": text_field(&m, "footer", 500)? }), logo))
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SiteInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 120)?;
    let slug = slug_ok(b.slug.as_deref().unwrap_or(&crate::routes::slugify(&name, "site")))?;
    let (settings, _) = clean_settings(&b.settings.unwrap_or(json!({})))?;
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO site_sites (name, slug, settings, created_by) VALUES ($1, $2, $3, $4) RETURNING id")
        .bind(&name)
        .bind(&slug)
        .bind(&settings)
        .bind(auth.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(d) if d.is_unique_violation() => AppError::bad("that address is taken"),
            e => e.into(),
        })?;
    // A first page to start from.
    let blocks = json!([{ "kind": "hero", "heading": name, "text": "What we do, in a sentence." }, { "kind": "text", "markdown": "Tell visitors about the company." }]);
    sqlx::query("INSERT INTO site_pages (site_id, path, title, blocks) VALUES ($1, '', 'Home', $2)").bind(id).bind(blocks).execute(&mut *tx).await?;
    tx.commit().await?;
    get_site(State(state), auth, Path(id)).await
}

async fn get_site(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let s: SiteRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SITE_SELECT} WHERE s.id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("site"))?;
    let pages: Vec<(Uuid, String, String, bool, i32, Option<DateTime<Utc>>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, path, title, in_menu, sort, published_at, updated_at FROM site_pages WHERE site_id = $1 ORDER BY (path = '') DESC, sort, title",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let mut v = serde_json::to_value(&s).unwrap();
    v["url"] = json!(format!("{}/s/{}", state.config.public_url, s.slug));
    v["page_list"] = json!(pages
        .into_iter()
        .map(|(id, path, title, menu, sort, published_at, updated_at)| json!({
            "id": id, "path": path, "title": title, "in_menu": menu, "sort": sort, "published_at": published_at, "updated_at": updated_at,
            "changed": published_at.is_none_or(|p| updated_at > p),
        }))
        .collect::<Vec<_>>());
    Ok(Json(v))
}

async fn update_site(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<SiteInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let name = b.name.as_deref().map(|n| required("name", n, 120)).transpose()?;
    let slug = b.slug.as_deref().map(slug_ok).transpose()?;
    let settings = match &b.settings {
        Some(s) => {
            let (clean, logo) = clean_settings(s)?;
            if let Some(l) = logo {
                check_images(&state, &auth, id, &[l]).await?;
            }
            Some(clean)
        }
        None => None,
    };
    sqlx::query("UPDATE site_sites SET name = COALESCE($2, name), slug = COALESCE($3, slug), settings = COALESCE($4, settings), updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(name)
        .bind(slug)
        .bind(settings)
        .execute(&state.db)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(d) if d.is_unique_violation() => AppError::bad("that address is taken"),
            e => e.into(),
        })?;
    get_site(State(state), auth, Path(id)).await
}

async fn delete_site(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    sqlx::query("DELETE FROM site_sites WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn publish_site(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    sqlx::query("UPDATE site_pages SET published_title = title, published_description = description, published_blocks = blocks, published_at = now() WHERE site_id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    get_site(State(state), auth, Path(id)).await
}

async fn stats(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let rows: Vec<(String, String, i64, i64)> = sqlx::query_as(
        "SELECT p.title, p.path, COALESCE(SUM(v.views) FILTER (WHERE v.day >= CURRENT_DATE - 6), 0), COALESCE(SUM(v.views) FILTER (WHERE v.day >= CURRENT_DATE - 29), 0)
         FROM site_pages p LEFT JOIN site_views v ON v.page_id = p.id WHERE p.site_id = $1 GROUP BY p.id, p.title, p.path ORDER BY 4 DESC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(t, p, w, m)| json!({ "title": t, "path": p, "week": w, "month": m })).collect::<Vec<_>>() })))
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

fn path_ok(p: &str) -> AppResult<String> {
    let p = p.trim().trim_matches('/').to_lowercase();
    if p.is_empty() {
        return Ok(p);
    }
    if p.len() > 60 || !p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(AppError::bad("a page address is lowercase letters, digits and dashes"));
    }
    Ok(p)
}

#[derive(Deserialize)]
struct PageInput {
    title: Option<String>,
    path: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    blocks: Option<Value>,
    in_menu: Option<bool>,
    sort: Option<i32>,
}

async fn create_page(State(state): State<AppState>, auth: AuthUser, Path(site): Path<Uuid>, Json(b): Json<PageInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let title = required("title", b.title.as_deref().unwrap_or(""), 120)?;
    let path = path_ok(b.path.as_deref().unwrap_or(&crate::routes::slugify(&title, "page")))?;
    let (blocks, images) = clean_blocks(&b.blocks.unwrap_or(json!([])))?;
    check_images(&state, &auth, site, &images).await?;
    let sort: i32 = sqlx::query_scalar("SELECT COALESCE(MAX(sort), 0) + 10 FROM site_pages WHERE site_id = $1").bind(site).fetch_one(&state.db).await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO site_pages (site_id, path, title, blocks, sort, in_menu) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id")
        .bind(site)
        .bind(&path)
        .bind(&title)
        .bind(blocks)
        .bind(sort)
        .bind(b.in_menu.unwrap_or(true))
        .fetch_one(&state.db)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(d) if d.is_unique_violation() => AppError::bad("another page has that address"),
            sqlx::Error::Database(d) if d.is_foreign_key_violation() => AppError::NotFound("site"),
            e => e.into(),
        })?;
    get_page(State(state), auth, Path(id)).await
}

#[derive(FromRow)]
struct PageRow {
    id: Uuid,
    site_id: Uuid,
    path: String,
    title: String,
    description: Option<String>,
    blocks: Value,
    in_menu: bool,
    sort: i32,
    published_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    slug: String,
}

async fn page_row(state: &AppState, id: Uuid) -> AppResult<PageRow> {
    sqlx::query_as(
        "SELECT p.id, p.site_id, p.path, p.title, p.description, p.blocks, p.in_menu, p.sort, p.published_at, p.updated_at, s.slug
         FROM site_pages p JOIN site_sites s ON s.id = p.site_id WHERE p.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("page"))
}

/// The draft, with Markdown rendered for the editor's preview.
async fn get_page(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let p = page_row(&state, id).await?;
    Ok(Json(json!({
        "id": p.id, "site_id": p.site_id, "path": p.path, "title": p.title, "description": p.description, "blocks": p.blocks,
        "rendered": render_blocks(&p.slug, &p.blocks), "in_menu": p.in_menu, "sort": p.sort, "published_at": p.published_at, "updated_at": p.updated_at,
        "changed": p.published_at.is_none_or(|x| p.updated_at > x),
        "url": format!("{}/s/{}/{}", state.config.public_url, p.slug, p.path),
    })))
}

async fn update_page(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PageInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let p = page_row(&state, id).await?;
    let title = b.title.as_deref().map(|t| required("title", t, 120)).transpose()?;
    let path = match &b.path {
        Some(x) if p.path.is_empty() && !x.trim().is_empty() => return Err(AppError::bad("the home page keeps its address")),
        Some(x) => Some(path_ok(x)?),
        None => None,
    };
    if path.as_deref() == Some("") && !p.path.is_empty() {
        return Err(AppError::bad("only the home page has an empty address"));
    }
    let blocks = match &b.blocks {
        Some(v) => {
            let (clean, images) = clean_blocks(v)?;
            check_images(&state, &auth, p.site_id, &images).await?;
            Some(clean)
        }
        None => None,
    };
    let description = b.description.map(|d| d.map(|x| x.trim().chars().take(300).collect::<String>()).filter(|x| !x.is_empty()));
    sqlx::query(
        "UPDATE site_pages SET title = COALESCE($2, title), path = COALESCE($3, path), description = CASE WHEN $4 THEN $5 ELSE description END,
             blocks = COALESCE($6, blocks), in_menu = COALESCE($7, in_menu), sort = COALESCE($8, sort), updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(title)
    .bind(path)
    .bind(description.is_some())
    .bind(description.flatten())
    .bind(blocks)
    .bind(b.in_menu)
    .bind(b.sort)
    .execute(&state.db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(d) if d.is_unique_violation() => AppError::bad("another page has that address"),
        e => e.into(),
    })?;
    get_page(State(state), auth, Path(id)).await
}

async fn delete_page(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let p = page_row(&state, id).await?;
    if p.path.is_empty() {
        return Err(AppError::bad("a site keeps its home page"));
    }
    sqlx::query("DELETE FROM site_pages WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn publish_page(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    sqlx::query("UPDATE site_pages SET published_title = title, published_description = description, published_blocks = blocks, published_at = now() WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    get_page(State(state), auth, Path(id)).await
}

async fn unpublish_page(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let p = page_row(&state, id).await?;
    if p.path.is_empty() {
        return Err(AppError::bad("take the site down by deleting it; the home page stays published"));
    }
    sqlx::query("UPDATE site_pages SET published_title = NULL, published_description = NULL, published_blocks = NULL, published_at = NULL WHERE id = $1").bind(id).execute(&state.db).await?;
    get_page(State(state), auth, Path(id)).await
}

// ---------------------------------------------------------------------------
// Visitors
// ---------------------------------------------------------------------------

async fn site_by_slug(state: &AppState, slug: &str) -> AppResult<(Uuid, String, Value)> {
    sqlx::query_as("SELECT id, name, settings FROM site_sites WHERE slug = $1").bind(slug).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("site"))
}

async fn public_site(State(state): State<AppState>, Path(slug): Path<String>) -> AppResult<Json<Value>> {
    let (id, name, settings) = site_by_slug(&state, &slug).await?;
    let menu: Vec<(String, String)> = sqlx::query_as(
        "SELECT published_title, path FROM site_pages WHERE site_id = $1 AND published_blocks IS NOT NULL AND in_menu ORDER BY (path = '') DESC, sort, published_title",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    if menu.is_empty() {
        return Err(AppError::NotFound("site"));
    }
    let logo = settings["logo_file_id"].as_str().map(|f| format!("/api/public/sites/{slug}/files/{f}"));
    Ok(Json(json!({
        "name": name, "color": settings["color"], "font": settings["font"], "footer": settings["footer"], "logo": logo,
        "menu": menu.into_iter().map(|(t, p)| json!({ "title": t, "path": p })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct PageQuery {
    #[serde(default)]
    path: String,
}

async fn public_page(State(state): State<AppState>, Path(slug): Path<String>, Query(q): Query<PageQuery>) -> AppResult<Json<Value>> {
    let (id, _, _) = site_by_slug(&state, &slug).await?;
    let row: Option<(Uuid, String, Option<String>, Value)> = sqlx::query_as(
        "SELECT id, published_title, published_description, published_blocks FROM site_pages WHERE site_id = $1 AND path = $2 AND published_blocks IS NOT NULL",
    )
    .bind(id)
    .bind(q.path.trim_matches('/').to_lowercase())
    .fetch_optional(&state.db)
    .await?;
    let (pid, title, description, blocks) = row.ok_or(AppError::NotFound("page"))?;
    let today: NaiveDate = Utc::now().date_naive();
    let _ = sqlx::query("INSERT INTO site_views (page_id, day, views) VALUES ($1, $2, 1) ON CONFLICT (page_id, day) DO UPDATE SET views = site_views.views + 1")
        .bind(pid)
        .bind(today)
        .execute(&state.db)
        .await;
    Ok(Json(json!({ "title": title, "description": description, "blocks": render_blocks(&slug, &blocks) })))
}

async fn public_file(State(state): State<AppState>, Path((slug, file)): Path<(String, Uuid)>) -> AppResult<Response> {
    let (id, _, _) = site_by_slug(&state, &slug).await?;
    if !site_files(&state, id, true).await?.contains(&file) {
        return Err(AppError::NotFound("file"));
    }
    let (_, mime, _, bytes) = crate::routes::files::read_bytes(&state, file).await?;
    if !["image/png", "image/jpeg", "image/webp", "image/gif"].contains(&mime.as_str()) {
        return Err(AppError::NotFound("file"));
    }
    let mut r = bytes.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'"));
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_checked() {
        let (b, imgs) = clean_blocks(&json!([{ "kind": "hero", "heading": " Hi ", "extra": "dropped", "button_link": "/contact" }, { "kind": "text", "markdown": "**x**" }])).unwrap();
        assert_eq!(b[0], json!({ "kind": "hero", "heading": "Hi", "button_link": "/contact" }));
        assert!(imgs.is_empty());
        assert!(clean_blocks(&json!([{ "kind": "script" }])).is_err());
        assert!(clean_blocks(&json!([{ "kind": "cta", "button_link": "javascript:alert(1)" }])).is_err());
        assert!(clean_blocks(&json!([{ "kind": "cta", "button_link": "//evil.example" }])).is_err());
        let id = Uuid::new_v4();
        assert_eq!(clean_blocks(&json!([{ "kind": "gallery", "images": [id.to_string()] }])).unwrap().1, vec![id]);
    }
}
