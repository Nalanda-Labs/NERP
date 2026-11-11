//! More for websites: starting templates (business, sales, marketing, recruitment), copying
//! pages and whole sites, reusable groups of blocks, export to a file and import from one,
//! finding a site by its own domain, and the sandboxed frame an HTML block runs in.
//!
//! An HTML block is the one place a site runs its own code. It is served from its own
//! address with a CSP `sandbox` (an opaque origin even when opened directly), so its scripts
//! can reach neither NERP's pages nor anyone's session.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::required,
    sites::{check_images, clean_blocks, clean_settings, get_page, get_site, path_ok, require, site_by_slug, site_files, slug_ok},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sites/{id}/copy", post(copy_site))
        .route("/sites/{id}/export", get(export))
        .route("/sites/import", post(import))
        .route("/sites/pages/{id}/copy", post(copy_page))
        .route("/sites/saved-blocks", get(saved).post(save_blocks))
        .route("/sites/saved-blocks/{id}", delete(drop_saved))
        .route("/public/site-domain", get(by_domain))
        .route("/public/sites/{slug}/embed/{page}/{n}", get(embed))
        .layer(axum::extract::DefaultBodyLimit::max(80 * 1024 * 1024))
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// A template's pages: (address, title, blocks).
pub fn template(key: &str, name: &str) -> AppResult<Vec<(String, String, Value)>> {
    let p = |path: &str, title: &str, blocks: Value| (path.to_string(), title.to_string(), blocks);
    Ok(match key {
        "business" => vec![p("", "Home", json!([{ "kind": "hero", "heading": name, "text": "What we do, in a sentence." }, { "kind": "text", "markdown": "Tell visitors about the company." }]))],
        "sales" => vec![
            p("", "Home", json!([
                { "kind": "hero", "heading": name, "text": "The one thing your customers get from you, said plainly.", "button_label": "See prices", "button_link": "/prices" },
                { "kind": "features", "heading": "Why customers choose us", "items": [
                    { "title": "Fast", "text": "Up and running in a day." }, { "title": "Fair", "text": "One clear price, no surprises." }, { "title": "Friendly", "text": "A person answers when you call." }] },
                { "kind": "products", "heading": "Popular right now", "limit": 4 },
                { "kind": "cta", "heading": "Ready when you are", "text": "Tell us what you need and we'll send an offer today.", "button_label": "Ask for an offer", "button_link": "/contact" }])),
            p("prices", "Prices", json!([
                { "kind": "hero", "heading": "Prices", "text": "Simple plans; change any time." },
                { "kind": "features", "items": [
                    { "title": "Starter", "text": "For a first try. Everything you need to begin." }, { "title": "Team", "text": "For growing teams, with priority support." }, { "title": "Company", "text": "For the whole company, with an account manager." }] }])),
            p("contact", "Contact", json!([{ "kind": "contact", "heading": "Talk to sales" }, { "kind": "form", "heading": "Ask for an offer" }])),
        ],
        "marketing" => vec![
            p("", "Home", json!([
                { "kind": "hero", "heading": "Something new from " .to_owned() + name, "text": "One line on what it is and who it is for.", "button_label": "Get early access", "button_link": "#signup" },
                { "kind": "features", "heading": "What you get", "items": [
                    { "title": "First benefit", "text": "What it does for the reader." }, { "title": "Second benefit", "text": "Why it matters." }, { "title": "Third benefit", "text": "What makes it different." }] },
                { "kind": "text", "markdown": "> “A short quote from a happy customer.” — Name, Company" },
                { "kind": "form", "heading": "Get early access" }])),
            p("event", "Event", json!([{ "kind": "hero", "heading": "Join our next event", "text": "Date, place, and what you'll learn." }, { "kind": "text", "markdown": "## Programme\n\n- 10:00 Welcome\n- 10:30 Talk\n- 12:00 Lunch" }, { "kind": "form", "heading": "Save your seat" }])),
        ],
        "recruitment" => vec![
            p("", "Careers", json!([
                { "kind": "hero", "heading": format!("Work at {name}"), "text": "Who we are, and the kind of people we look for.", "button_label": "Open positions", "button_link": "/jobs" },
                { "kind": "features", "heading": "Why join us", "items": [
                    { "title": "Growth", "text": "Training, mentors and a budget for learning." }, { "title": "Balance", "text": "Flexible hours and remote days." }, { "title": "Team", "text": "Kind, curious colleagues." }] },
                { "kind": "text", "markdown": "## How we hire\n\n1. Send your CV\n2. A short call\n3. Meet the team\n4. An offer" }])),
            p("jobs", "Open positions", json!([{ "kind": "text", "markdown": "## Open positions\n\n### Sales manager\nWhat the role does, and what you bring.\n\n### Support specialist\nWhat the role does, and what you bring." }, { "kind": "form", "heading": "Apply" }])),
        ],
        _ => return Err(AppError::bad("templates are business, sales, marketing and recruitment")),
    })
}

// ---------------------------------------------------------------------------
// Copies
// ---------------------------------------------------------------------------

async fn free_path(state: &AppState, site: Uuid, base: &str) -> AppResult<String> {
    let base = if base.is_empty() { "home".to_string() } else { base.to_string() };
    for n in 1..100 {
        let p = if n == 1 { format!("{base}-copy") } else { format!("{base}-copy-{n}") };
        let p: String = p.chars().take(60).collect();
        let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM site_pages WHERE site_id = $1 AND path = $2)").bind(site).bind(&p).fetch_one(&state.db).await?;
        if !taken {
            return Ok(p);
        }
    }
    Err(AppError::bad("too many copies"))
}

/// A copy of a page as a new, unpublished page of the same site.
async fn copy_page(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let (site, path, title, description, blocks, in_menu): (Uuid, String, String, Option<String>, Value, bool) =
        sqlx::query_as("SELECT site_id, path, title, description, blocks, in_menu FROM site_pages WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("page"))?;
    let new_path = free_path(&state, site, &path).await?;
    let sort: i32 = sqlx::query_scalar("SELECT COALESCE(MAX(sort), 0) + 10 FROM site_pages WHERE site_id = $1").bind(site).fetch_one(&state.db).await?;
    let nid: Uuid = sqlx::query_scalar("INSERT INTO site_pages (site_id, path, title, description, blocks, in_menu, sort) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id")
        .bind(site)
        .bind(&new_path)
        .bind(format!("{title} (copy)").chars().take(120).collect::<String>())
        .bind(description)
        .bind(blocks)
        .bind(in_menu)
        .bind(sort)
        .fetch_one(&state.db)
        .await?;
    get_page(State(state), auth, Path(nid)).await
}

#[derive(Deserialize, Default)]
struct CopyInput {
    name: Option<String>,
    slug: Option<String>,
}

/// A copy of a whole site (drafts only; nothing published, no domain).
async fn copy_site(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, body: Option<Json<CopyInput>>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let b = body.map(|b| b.0).unwrap_or_default();
    let (name, slug, settings): (String, String, Value) = sqlx::query_as("SELECT name, slug, settings FROM site_sites WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("site"))?;
    let name = required("name", b.name.as_deref().unwrap_or(&format!("{name} (copy)")), 120)?;
    let mut slug = slug_ok(b.slug.as_deref().unwrap_or(&format!("{}-copy", &slug[..slug.len().min(34)])))?;
    for n in 2..50 {
        let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM site_sites WHERE slug = $1)").bind(&slug).fetch_one(&state.db).await?;
        if !taken {
            break;
        }
        slug = slug_ok(&format!("{}-{n}", slug.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('-')))?;
    }
    let mut tx = state.db.begin().await?;
    let nid: Uuid = sqlx::query_scalar("INSERT INTO site_sites (name, slug, settings, created_by) VALUES ($1, $2, $3, $4) RETURNING id").bind(&name).bind(&slug).bind(settings).bind(auth.id).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO site_pages (site_id, path, title, description, blocks, in_menu, sort) SELECT $2, path, title, description, blocks, in_menu, sort FROM site_pages WHERE site_id = $1")
        .bind(id)
        .bind(nid)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    get_site(State(state), auth, Path(nid)).await
}

// ---------------------------------------------------------------------------
// Reusable groups of blocks
// ---------------------------------------------------------------------------

async fn saved(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require(&auth)?;
    let rows: Vec<(Uuid, String, Value, Option<String>)> = sqlx::query_as(
        "SELECT b.id, b.name, b.blocks, trim(u.first_name || ' ' || u.last_name) FROM site_saved_blocks b LEFT JOIN users u ON u.id = b.created_by ORDER BY lower(b.name)",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, n, b, by)| json!({ "id": id, "name": n, "blocks": b, "created_by": by })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct SaveInput {
    name: String,
    blocks: Value,
}

async fn save_blocks(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SaveInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let name = required("name", &b.name, 120)?;
    let (blocks, _) = clean_blocks(&b.blocks)?;
    if blocks.as_array().is_none_or(|a| a.is_empty()) {
        return Err(AppError::bad("pick at least one block"));
    }
    sqlx::query("INSERT INTO site_saved_blocks (name, blocks, created_by) VALUES ($1, $2, $3)").bind(name).bind(blocks).bind(auth.id).execute(&state.db).await?;
    saved(State(state), auth).await
}

async fn drop_saved(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    sqlx::query("DELETE FROM site_saved_blocks WHERE id = $1").bind(id).execute(&state.db).await?;
    saved(State(state), auth).await
}

// ---------------------------------------------------------------------------
// Export and import
// ---------------------------------------------------------------------------

const FORMAT: &str = "nerp-site/1";

/// The whole site as one JSON file: look, pages (drafts) and the pictures they use.
async fn export(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Response> {
    require(&auth)?;
    let (name, slug, settings): (String, String, Value) = sqlx::query_as("SELECT name, slug, settings FROM site_sites WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("site"))?;
    let pages: Vec<(String, String, Option<String>, Value, bool, i32)> =
        sqlx::query_as("SELECT path, title, description, blocks, in_menu, sort FROM site_pages WHERE site_id = $1 ORDER BY sort, path").bind(id).fetch_all(&state.db).await?;
    let mut files = serde_json::Map::new();
    for f in site_files(&state, id, false).await? {
        if let Ok((fname, mime, _, bytes)) = crate::routes::files::read_bytes(&state, f).await {
            files.insert(f.to_string(), json!({ "name": fname, "mime": mime, "data": base64::engine::general_purpose::STANDARD.encode(bytes) }));
        }
    }
    let doc = json!({
        "format": FORMAT, "name": name, "slug": slug, "settings": settings,
        "pages": pages.into_iter().map(|(p, t, d, b, m, s)| json!({ "path": p, "title": t, "description": d, "blocks": b, "in_menu": m, "sort": s })).collect::<Vec<_>>(),
        "files": files,
    });
    let body = serde_json::to_vec_pretty(&doc).map_err(anyhow::Error::from)?;
    Ok((
        [(header::CONTENT_TYPE, "application/json".to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{slug}.nerp-site.json\""))],
        body,
    )
        .into_response())
}

#[derive(Deserialize)]
struct ImportInput {
    data: Value,
    name: Option<String>,
    slug: Option<String>,
}

/// A new site from an exported file: its pictures become new uploads of the importer.
async fn import(State(state): State<AppState>, auth: AuthUser, Json(b): Json<ImportInput>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let d = &b.data;
    if d["format"] != FORMAT {
        return Err(AppError::bad("this is not a NERP site file"));
    }
    let name = required("name", b.name.as_deref().or(d["name"].as_str()).unwrap_or(""), 120)?;
    let slug = slug_ok(b.slug.as_deref().or(d["slug"].as_str()).unwrap_or(""))?;
    let pages = d["pages"].as_array().cloned().unwrap_or_default();
    if pages.is_empty() || pages.len() > 200 {
        return Err(AppError::bad("a site file has 1 to 200 pages"));
    }
    // Pictures first, so the blocks can point at their new ids.
    let mut text = serde_json::to_string(&json!({ "settings": d["settings"], "pages": pages })).map_err(anyhow::Error::from)?;
    if let Some(files) = d["files"].as_object() {
        if files.len() > 300 {
            return Err(AppError::bad("too many pictures"));
        }
        for (old, f) in files {
            let old: Uuid = old.parse().map_err(|_| AppError::bad("a picture in the file is not valid"))?;
            let mime = f["mime"].as_str().unwrap_or("");
            if !["image/png", "image/jpeg", "image/webp", "image/gif"].contains(&mime) {
                return Err(AppError::bad("pictures must be PNG, JPEG, WebP or GIF"));
            }
            let bytes = base64::engine::general_purpose::STANDARD.decode(f["data"].as_str().unwrap_or("")).map_err(|_| AppError::bad("a picture in the file is damaged"))?;
            let new = crate::routes::files::store_bytes(&state, Some(auth.id), f["name"].as_str().unwrap_or("picture"), mime, &bytes).await?;
            text = text.replace(&old.to_string(), &new.to_string());
        }
    }
    let v: Value = serde_json::from_str(&text).map_err(anyhow::Error::from)?;
    let (settings, logo) = clean_settings(&v["settings"])?;
    let mut clean = Vec::new();
    let mut images = Vec::new();
    for p in v["pages"].as_array().cloned().unwrap_or_default() {
        let (blocks, imgs) = clean_blocks(&p["blocks"])?;
        images.extend(imgs);
        clean.push((
            path_ok(p["path"].as_str().unwrap_or(""))?,
            required("title", p["title"].as_str().unwrap_or(""), 120)?,
            p["description"].as_str().map(|s| s.chars().take(300).collect::<String>()),
            blocks,
            p["in_menu"].as_bool().unwrap_or(true),
            p["sort"].as_i64().unwrap_or(0) as i32,
        ));
    }
    if !clean.iter().any(|p| p.0.is_empty()) {
        return Err(AppError::bad("the site file has no home page"));
    }
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO site_sites (name, slug, settings, created_by) VALUES ($1, $2, $3, $4) RETURNING id")
        .bind(&name)
        .bind(&slug)
        .bind(settings)
        .bind(auth.id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(d) if d.is_unique_violation() => AppError::bad("that address is taken"),
            e => e.into(),
        })?;
    for (path, title, desc, blocks, menu, sort) in clean {
        sqlx::query("INSERT INTO site_pages (site_id, path, title, description, blocks, in_menu, sort) VALUES ($1, $2, $3, $4, $5, $6, $7)")
            .bind(id)
            .bind(path)
            .bind(title)
            .bind(desc)
            .bind(blocks)
            .bind(menu)
            .bind(sort)
            .execute(&mut *tx)
            .await
            .map_err(|e| match e {
                sqlx::Error::Database(d) if d.is_unique_violation() => AppError::bad("two pages in the file have the same address"),
                e => e.into(),
            })?;
    }
    tx.commit().await?;
    images.extend(logo);
    check_images(&state, &auth, id, &images).await?;
    get_site(State(state), auth, Path(id)).await
}

// ---------------------------------------------------------------------------
// Visitors
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct HostQuery {
    host: String,
}

/// Which site a domain shows (the frontend asks when opened on a host that isn't NERP's).
async fn by_domain(State(state): State<AppState>, Query(q): Query<HostQuery>) -> AppResult<Json<Value>> {
    let host = q.host.trim().trim_end_matches('.').to_lowercase();
    let host = host.split(':').next().unwrap_or("").to_string();
    let slug: Option<String> = sqlx::query_scalar("SELECT slug FROM site_sites WHERE domain = $1").bind(&host).fetch_optional(&state.db).await?;
    Ok(Json(json!({ "slug": slug.ok_or(AppError::NotFound("site"))? })))
}

/// An HTML block of a published page, on its own, in a sandbox.
async fn embed(State(state): State<AppState>, Path((slug, page, n)): Path<(String, Uuid, usize)>) -> AppResult<Response> {
    let (site, _, _) = site_by_slug(&state, &slug).await?;
    let blocks: Option<Value> = sqlx::query_scalar("SELECT published_blocks FROM site_pages WHERE id = $1 AND site_id = $2").bind(page).bind(site).fetch_optional(&state.db).await?.flatten();
    let b = blocks.as_ref().and_then(|b| b.get(n)).filter(|b| b["kind"] == "html").ok_or(AppError::NotFound("block"))?;
    let html = format!("<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><style>body{{margin:0;font-family:system-ui,sans-serif}}</style></head><body>{}</body></html>", b["html"].as_str().unwrap_or(""));
    let mut r = html.into_response();
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    // Its own opaque origin, whatever opens it; anything else goes.
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox; default-src * data: blob: 'unsafe-inline' 'unsafe-eval'; frame-ancestors 'self'"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=60"));
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_valid_pages() {
        for t in ["business", "sales", "marketing", "recruitment"] {
            let pages = template(t, "Acme").unwrap();
            assert!(pages.iter().any(|p| p.0.is_empty()), "{t} has a home page");
            for (path, _, blocks) in pages {
                path_ok(&path).unwrap();
                clean_blocks(&blocks).unwrap();
            }
        }
        assert!(template("space", "x").is_err());
    }
}
