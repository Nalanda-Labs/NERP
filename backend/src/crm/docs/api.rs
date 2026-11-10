//! Document templates and generated documents: HTTP API, numbering, rendering and the
//! background job for the external engines.

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use std::sync::OnceLock;
use uuid::Uuid;

use super::{data, latex, office, typst_engine};
use crate::{
    auth::{permissions as perm, AuthUser},
    crm::{access::{is_crm_admin, require}, entity_type, items::load_row, timeline},
    error::{AppError, AppResult},
    jobs, realtime,
    routes::{files, required},
    state::AppState,
};

pub const JOB: &str = "crm.document";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/doc-templates", get(list_templates).post(create_template))
        .route("/crm/doc-templates/sample", get(sample))
        .route("/crm/doc-templates/preview", post(preview))
        .route("/crm/doc-templates/{id}", get(get_template).patch(update_template).delete(delete_template))
        .route("/crm/items/{id}/documents", get(list_documents).post(create_document))
        .route("/crm/documents/{id}", get(get_document).delete(delete_document))
        .route("/crm/documents/{id}/regenerate", post(regenerate))
        .merge(super::browse::router())
}

// ---------------------------------------------------------------------------
// Engines
// ---------------------------------------------------------------------------

/// Which renderers work on this server, found once at start-up.
#[derive(Serialize, Clone, Default)]
pub struct Engines {
    pub typst: Option<String>,
    pub latex: Option<String>,
    pub office: Option<String>,
}

static ENGINES: OnceLock<Engines> = OnceLock::new();

pub fn engines() -> Engines {
    ENGINES.get().cloned().unwrap_or_else(|| Engines { typst: Some(typst_version()), ..Default::default() })
}

fn typst_version() -> String {
    "Typst 0.15 (built in)".into()
}

pub async fn detect(state: &AppState) {
    let c = &state.config.docs;
    super::process::init(c.concurrency);
    let e = Engines {
        typst: Some(typst_version()),
        latex: super::process::version(&c.lualatex_bin).await,
        office: super::process::version(&c.soffice_bin).await,
    };
    tracing::info!(latex = ?e.latex, office = ?e.office, "document engines");
    let _ = ENGINES.set(e);
}

fn available(engine: &str) -> bool {
    let e = engines();
    match engine {
        "typst" => true,
        "latex" => e.latex.is_some(),
        "office" => e.office.is_some(),
        _ => false,
    }
}

fn engine_label(engine: &str) -> &'static str {
    match engine {
        "latex" => "LuaLaTeX",
        "office" => "LibreOffice",
        _ => "Typst",
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

pub struct Rendered {
    pub pdf: Vec<u8>,
    /// The filled DOCX, for LibreOffice templates.
    pub docx: Option<Vec<u8>>,
}

/// Errors from rendering: the template's fault (shown to the user, never retried) or the
/// server's (retried).
pub enum RenderError {
    Template(String),
    Server(String),
}

async fn logo(state: &AppState, id: Option<Uuid>) -> Option<(String, Vec<u8>)> {
    let (_, mime, _, bytes) = files::read_bytes(state, id?).await.ok()?;
    let ext = match mime.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/svg+xml" => "svg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => return None,
    };
    Some((ext.to_string(), bytes))
}

pub async fn render(state: &AppState, engine: &str, source: Option<&str>, file_id: Option<Uuid>, data: &Value, logo_file: Option<Uuid>) -> Result<Rendered, RenderError> {
    let c = &state.config.docs;
    let logo = logo(state, logo_file).await;
    match engine {
        "typst" => {
            let src = source.unwrap_or_default().to_string();
            let data = data.clone();
            // Typst finds the image format from the bytes; the file is always "logo".
            let logo = logo.map(|(_, b)| ("logo".to_string(), b));
            let pdf = typst_engine::render_isolated(&src, &data, logo, c.timeout_secs).await.map_err(|e| match e {
                typst_engine::IsolatedError::Template(m) => RenderError::Template(m),
                typst_engine::IsolatedError::Server(m) => RenderError::Server(m),
            })?;
            Ok(Rendered { pdf, docx: None })
        }
        "latex" => {
            if !available("latex") {
                return Err(RenderError::Template("LuaLaTeX is not installed on this server".into()));
            }
            let logo = logo.map(|(ext, b)| (format!("logo.{ext}"), b));
            let pdf = latex::render(&c.lualatex_bin, source.unwrap_or_default(), data, logo, c.timeout_secs)
                .await
                .map_err(RenderError::Template)?;
            Ok(Rendered { pdf, docx: None })
        }
        "office" => {
            let Some(fid) = file_id else {
                return Err(RenderError::Template("upload a .docx or .odt template first".into()));
            };
            let (_, _, _, template) = files::read_bytes(state, fid)
                .await
                .map_err(|_| RenderError::Template("the template file is missing".into()))?;
            let docx = office::fill_docx(&template, data).map_err(RenderError::Template)?;
            if !available("office") {
                return Err(RenderError::Template("LibreOffice is not installed on this server".into()));
            }
            let pdf = office::convert(&c.soffice_bin, &docx, "docx", "pdf", c.timeout_secs)
                .await
                .map_err(RenderError::Template)?;
            Ok(Rendered { pdf, docx: Some(docx) })
        }
        _ => Err(RenderError::Template("unknown engine".into())),
    }
}

fn pdf_response(pdf: Vec<u8>, name: &str) -> Response {
    let mut res = pdf.into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox; default-src 'none'"));
    if let Ok(v) = HeaderValue::from_str(&format!("inline; filename=\"{name}\"")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    res
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
pub struct Template {
    id: Uuid,
    name: String,
    engine: String,
    entity_types: Vec<i16>,
    doc_title: String,
    locale: String,
    source: Option<String>,
    file_id: Option<Uuid>,
    number_prefix: String,
    builtin_key: Option<String>,
    is_active: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[sqlx(skip)]
    available: bool,
}

const TEMPLATE_COLUMNS: &str =
    "id, name, engine, entity_types, doc_title, locale, source, file_id, number_prefix, builtin_key, is_active, created_at, updated_at";

async fn load_template(state: &AppState, id: Uuid) -> AppResult<Template> {
    let mut t: Template = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {TEMPLATE_COLUMNS} FROM crm_doc_templates WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("template"))?;
    t.available = available(&t.engine);
    Ok(t)
}

#[derive(Deserialize)]
struct TemplateQuery {
    /// Record type code: only active templates offered for it.
    r#type: Option<String>,
}

async fn list_templates(State(state): State<AppState>, auth: AuthUser, Query(q): Query<TemplateQuery>) -> AppResult<Json<Value>> {
    let admin = is_crm_admin(&auth);
    let t = match &q.r#type {
        Some(code) => Some(entity_type(&state, code).await?.id),
        None if admin => None,
        None => return Err(AppError::bad("type is required")),
    };
    let mut items: Vec<Template> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TEMPLATE_COLUMNS} FROM crm_doc_templates
         WHERE ($1::smallint IS NULL OR ($1 = ANY(entity_types) AND is_active)) ORDER BY name"
    )))
    .bind(t)
    .fetch_all(&state.db)
    .await?;
    for i in &mut items {
        i.available = available(&i.engine);
        if !admin {
            i.source = None;
        }
    }
    if t.is_some() {
        items.retain(|i| i.available);
    }
    Ok(Json(json!({ "items": items, "engines": engines() })))
}

async fn get_template(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Template>> {
    require_admin(&auth)?;
    Ok(Json(load_template(&state, id).await?))
}

fn require_admin(auth: &AuthUser) -> AppResult<()> {
    if is_crm_admin(auth) { Ok(()) } else { Err(AppError::Forbidden) }
}

/// A LaTeX template is code the server runs: LuaTeX's Lua can read and write any file the
/// server can (shell commands stay off). So writing one takes "Manage system", the trust of
/// running the server; Typst and Word templates cannot reach files and stay with CRM admins.
fn require_code(auth: &AuthUser, engine: &str) -> AppResult<()> {
    if engine == "latex" && !auth.can(crate::auth::permissions::SYSTEM_MANAGE) {
        return Err(AppError::bad("LaTeX templates run code on the server: adding or changing one needs the \"Manage system\" permission"));
    }
    Ok(())
}

#[derive(Deserialize)]
struct TemplateBody {
    name: Option<String>,
    engine: Option<String>,
    /// Record type codes.
    entity_types: Option<Vec<String>>,
    doc_title: Option<String>,
    locale: Option<String>,
    source: Option<String>,
    file_id: Option<Uuid>,
    number_prefix: Option<String>,
    is_active: Option<bool>,
}

async fn type_ids(state: &AppState, codes: &[String]) -> AppResult<Vec<i16>> {
    let mut out = Vec::new();
    for c in codes {
        let id = entity_type(state, c).await?.id;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    if out.is_empty() {
        return Err(AppError::bad("choose at least one record type"));
    }
    Ok(out)
}

fn check_prefix(p: &str) -> AppResult<String> {
    let p = p.trim();
    if p.chars().count() > 40 || p.chars().any(|c| c.is_control()) {
        return Err(AppError::bad("the number prefix may have up to 40 characters"));
    }
    Ok(p.to_string())
}

fn check_locale(l: &str) -> AppResult<String> {
    let l = l.trim();
    if l.len() > 35 || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(AppError::bad("locale must be a language tag like en-US or de-DE"));
    }
    Ok(l.to_string())
}

fn check_source(s: &str) -> AppResult<()> {
    if s.len() > 512 * 1024 { Err(AppError::bad("the template is too large (512 KB max)")) } else { Ok(()) }
}

/// Accepts an uploaded .docx as is, converts an .odt to .docx, rejects anything else.
/// Returns the file id to keep.
async fn office_template_file(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Uuid> {
    let (name, _, owner, bytes) = files::read_bytes(state, id).await?;
    if owner != Some(auth.id) && !auth.can(perm::FILES_MANAGE) {
        return Err(AppError::NotFound("file"));
    }
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).map_err(|_| AppError::bad("upload a .docx or .odt file"))?;
    if zip.by_name("word/document.xml").is_ok() {
        return Ok(id);
    }
    let is_odt = {
        let mut m = String::new();
        zip.by_name("mimetype").ok().and_then(|f| std::io::Read::read_to_string(&mut std::io::Read::take(f, 200), &mut m).ok()).is_some()
            && m.trim() == "application/vnd.oasis.opendocument.text"
    };
    if !is_odt {
        return Err(AppError::bad("upload a Word (.docx) or Writer (.odt) document"));
    }
    if !available("office") {
        return Err(AppError::bad(".odt templates need LibreOffice on the server; upload a .docx instead"));
    }
    let c = &state.config.docs;
    let docx = office::convert(&c.soffice_bin, &bytes, "odt", "docx", c.timeout_secs).await.map_err(AppError::bad)?;
    let base = name.trim_end_matches(".odt");
    let new = files::store_bytes(state, Some(auth.id), &format!("{base}.docx"), "application/vnd.openxmlformats-officedocument.wordprocessingml.document", &docx).await?;
    Ok(new)
}

async fn create_template(State(state): State<AppState>, auth: AuthUser, Json(b): Json<TemplateBody>) -> AppResult<Json<Template>> {
    require_admin(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 200)?;
    let engine = b.engine.unwrap_or_else(|| "typst".into());
    if !["typst", "latex", "office"].contains(&engine.as_str()) {
        return Err(AppError::bad("engine must be typst, latex or office"));
    }
    require_code(&auth, &engine)?;
    let types = type_ids(&state, &b.entity_types.unwrap_or_else(|| vec!["deal".into()])).await?;
    let source = if engine == "office" { None } else { Some(b.source.unwrap_or_default()) };
    if let Some(s) = &source {
        check_source(s)?;
    }
    let file_id = match (engine.as_str(), b.file_id) {
        ("office", Some(f)) => Some(office_template_file(&state, &auth, f).await?),
        _ => None,
    };
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO crm_doc_templates (name, engine, entity_types, doc_title, locale, source, file_id, number_prefix, is_active, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
    )
    .bind(&name)
    .bind(&engine)
    .bind(&types)
    .bind(required("document title", b.doc_title.as_deref().unwrap_or("Invoice"), 100)?)
    .bind(check_locale(b.locale.as_deref().unwrap_or(""))?)
    .bind(source)
    .bind(file_id)
    .bind(check_prefix(b.number_prefix.as_deref().unwrap_or("INV-{YYYY}-"))?)
    .bind(b.is_active.unwrap_or(true))
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(load_template(&state, id).await?))
}

async fn update_template(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<TemplateBody>) -> AppResult<Json<Template>> {
    require_admin(&auth)?;
    let cur = load_template(&state, id).await?;
    if b.engine.as_deref().is_some_and(|e| e != cur.engine) {
        return Err(AppError::bad("the engine of a template cannot change; create a new template"));
    }
    if b.source.is_some() {
        require_code(&auth, &cur.engine)?;
    }
    let types = match &b.entity_types {
        Some(c) => type_ids(&state, c).await?,
        None => cur.entity_types.clone(),
    };
    if let Some(s) = &b.source {
        check_source(s)?;
    }
    let file_id = match b.file_id {
        Some(f) if cur.engine == "office" && Some(f) != cur.file_id => Some(office_template_file(&state, &auth, f).await?),
        _ => cur.file_id,
    };
    sqlx::query(
        "UPDATE crm_doc_templates SET name = $2, entity_types = $3, doc_title = $4, locale = $5, source = $6, file_id = $7,
             number_prefix = $8, is_active = $9, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(match &b.name { Some(n) => required("name", n, 200)?, None => cur.name })
    .bind(&types)
    .bind(match &b.doc_title { Some(t) => required("document title", t, 100)?, None => cur.doc_title })
    .bind(match &b.locale { Some(l) => check_locale(l)?, None => cur.locale })
    .bind(if cur.engine == "office" { None } else { b.source.or(cur.source) })
    .bind(file_id)
    .bind(match &b.number_prefix { Some(p) => check_prefix(p)?, None => cur.number_prefix })
    .bind(b.is_active.unwrap_or(cur.is_active))
    .execute(&state.db)
    .await?;
    Ok(Json(load_template(&state, id).await?))
}

async fn delete_template(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    // Documents already generated keep their PDFs and the template's name.
    let n = sqlx::query("DELETE FROM crm_doc_templates WHERE id = $1").bind(id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("template"));
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct SampleQuery {
    locale: Option<String>,
    title: Option<String>,
}

/// Sample data: the field reference shown in the template editor.
async fn sample(auth: AuthUser, Query(q): Query<SampleQuery>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    Ok(Json(data::sample(q.locale.as_deref().unwrap_or("en-US"), q.title.as_deref().unwrap_or("Invoice"))))
}

#[derive(Deserialize)]
struct PreviewBody {
    engine: String,
    source: Option<String>,
    file_id: Option<Uuid>,
    locale: Option<String>,
    doc_title: Option<String>,
    /// Render with this record's data instead of the sample.
    item_id: Option<Uuid>,
}

async fn default_locale(state: &AppState) -> String {
    crate::routes::settings::load(state)
        .await
        .ok()
        .and_then(|s| s.get("default_locale").and_then(Value::as_str).map(String::from))
        .unwrap_or_else(|| "en-US".into())
}

/// Renders unsaved template source for the editor's live preview.
async fn preview(State(state): State<AppState>, auth: AuthUser, Json(b): Json<PreviewBody>) -> AppResult<Response> {
    require_admin(&auth)?;
    if let Some(s) = &b.source {
        require_code(&auth, &b.engine)?;
        check_source(s)?;
    }
    let locale = match b.locale.as_deref().map(check_locale).transpose()? {
        Some(l) if !l.is_empty() => l,
        _ => default_locale(&state).await,
    };
    let title = b.doc_title.unwrap_or_else(|| "Invoice".into());
    let (data, logo) = match b.item_id {
        Some(item) => {
            let row = load_row(&state, item, false).await?;
            let built = data::build(&state, &auth, item, row.entity_type, "PREVIEW", &title, &locale).await?;
            (built.data, built.logo_file)
        }
        None => {
            let mut d = data::sample(&locale, &title);
            // Show the real seller block and logo so the preview looks like the real thing.
            if let Ok((seller, logo)) = data::seller(&state).await {
                if seller["name"].as_str().is_some_and(|n| !n.is_empty()) {
                    d["seller"] = seller;
                }
                (d, logo)
            } else {
                (d, None)
            }
        }
    };
    match render(&state, &b.engine, b.source.as_deref(), b.file_id, &data, logo).await {
        Ok(r) => Ok(pdf_response(r.pdf, "preview.pdf")),
        Err(RenderError::Template(e)) | Err(RenderError::Server(e)) => Err(AppError::bad(e)),
    }
}

// ---------------------------------------------------------------------------
// Built-in templates
// ---------------------------------------------------------------------------

macro_rules! tpl {
    ($f:literal) => {
        include_str!(concat!("../../../assets/doc-templates/", $f))
    };
}
macro_rules! docx {
    ($f:literal) => {
        include_bytes!(concat!("../../../assets/doc-templates/", $f))
    };
}

const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

enum Src {
    Text(&'static str),
    Docx(&'static [u8], &'static str),
}

struct Builtin {
    key: &'static str,
    name: &'static str,
    engine: &'static str,
    title: &'static str,
    prefix: &'static str,
    types: &'static [i16],
    src: Src,
}

const INV: &str = "INV-{YYYY}-";
const BUILTINS: &[Builtin] = &[
    Builtin { key: "typst.invoice", name: "Invoice", engine: "typst", title: "Invoice", prefix: INV, types: &[2], src: Src::Text(tpl!("invoice.typ")) },
    Builtin { key: "typst.quote", name: "Quote", engine: "typst", title: "Quote", prefix: "QUO-{YYYY}-", types: &[1, 2], src: Src::Text(tpl!("quote.typ")) },
    Builtin { key: "latex.invoice", name: "Invoice (LaTeX)", engine: "latex", title: "Invoice", prefix: INV, types: &[2], src: Src::Text(tpl!("invoice.tex")) },
    Builtin { key: "office.invoice", name: "Invoice (Word)", engine: "office", title: "Invoice", prefix: INV, types: &[2], src: Src::Docx(docx!("invoice.docx"), "invoice-template.docx") },
    // Example designs, three per engine.
    Builtin { key: "typst.invoice.classic", name: "Invoice · Classic (Typst)", engine: "typst", title: "Invoice", prefix: INV, types: &[2], src: Src::Text(tpl!("invoice-classic.typ")) },
    Builtin { key: "typst.invoice.modern", name: "Invoice · Modern (Typst)", engine: "typst", title: "Invoice", prefix: INV, types: &[2], src: Src::Text(tpl!("invoice-modern.typ")) },
    Builtin { key: "typst.invoice.minimal", name: "Invoice · Minimal (Typst)", engine: "typst", title: "Invoice", prefix: INV, types: &[2], src: Src::Text(tpl!("invoice-minimal.typ")) },
    Builtin { key: "latex.invoice.classic", name: "Invoice · Classic (LaTeX)", engine: "latex", title: "Invoice", prefix: INV, types: &[2], src: Src::Text(tpl!("invoice-classic.tex")) },
    Builtin { key: "latex.invoice.modern", name: "Invoice · Modern (LaTeX)", engine: "latex", title: "Invoice", prefix: INV, types: &[2], src: Src::Text(tpl!("invoice-modern.tex")) },
    Builtin { key: "latex.invoice.minimal", name: "Invoice · Minimal (LaTeX)", engine: "latex", title: "Invoice", prefix: INV, types: &[2], src: Src::Text(tpl!("invoice-minimal.tex")) },
    Builtin { key: "office.invoice.classic", name: "Invoice · Classic (Word)", engine: "office", title: "Invoice", prefix: INV, types: &[2], src: Src::Docx(docx!("invoice-classic.docx"), "invoice-classic.docx") },
    Builtin { key: "office.invoice.modern", name: "Invoice · Modern (Word)", engine: "office", title: "Invoice", prefix: INV, types: &[2], src: Src::Docx(docx!("invoice-modern.docx"), "invoice-modern.docx") },
    Builtin { key: "office.invoice.minimal", name: "Invoice · Minimal (Word)", engine: "office", title: "Invoice", prefix: INV, types: &[2], src: Src::Docx(docx!("invoice-minimal.docx"), "invoice-minimal.docx") },
];

async fn store_docx(state: &AppState, name: &str, bytes: &[u8]) -> anyhow::Result<Uuid> {
    files::store_bytes(state, None, name, DOCX_MIME, bytes).await.map_err(|e| anyhow::anyhow!("{e:?}"))
}

/// Adds the built-in templates, and upgrades those no admin has changed (updated_at is
/// only bumped by edits through the API). Edited built-ins are never overwritten.
pub async fn seed(state: &AppState) -> anyhow::Result<()> {
    for b in BUILTINS {
        // (id, source, file_id, edited by an admin since it was added or last upgraded)
        let existing: Option<(Uuid, Option<String>, Option<Uuid>, bool)> =
            sqlx::query_as("SELECT id, source, file_id, updated_at > created_at FROM crm_doc_templates WHERE builtin_key = $1")
                .bind(b.key)
                .fetch_optional(&state.db)
                .await?;
        if let Some((id, source, file_id, edited)) = existing {
            // Untouched built-ins follow upgrades; edited ones are the admin's.
            if edited {
                continue;
            }
            match b.src {
                Src::Docx(bytes, name) => {
                    let same = match file_id {
                        Some(f) => files::read_bytes(state, f).await.map(|x| x.3 == bytes).unwrap_or(false),
                        None => false,
                    };
                    if !same {
                        let f = store_docx(state, name, bytes).await?;
                        sqlx::query("UPDATE crm_doc_templates SET file_id = $2 WHERE id = $1").bind(id).bind(f).execute(&state.db).await?;
                        if let Some(old) = file_id {
                            files::delete_stored(state, old).await;
                        }
                        tracing::info!(template = b.key, "upgraded built-in document template");
                    }
                }
                Src::Text(new) => {
                    if source.as_deref() != Some(new) {
                        sqlx::query("UPDATE crm_doc_templates SET source = $2 WHERE id = $1").bind(id).bind(new).execute(&state.db).await?;
                        tracing::info!(template = b.key, "upgraded built-in document template");
                    }
                }
            }
            continue;
        }
        let (source, file_id) = match b.src {
            Src::Text(t) => (Some(t), None),
            Src::Docx(bytes, name) => (None, Some(store_docx(state, name, bytes).await?)),
        };
        sqlx::query(
            "INSERT INTO crm_doc_templates (name, engine, entity_types, doc_title, source, file_id, number_prefix, builtin_key, is_active)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) ON CONFLICT (builtin_key) DO NOTHING",
        )
        .bind(b.name)
        .bind(b.engine)
        .bind(b.types)
        .bind(b.title)
        .bind(source)
        .bind(file_id)
        .bind(b.prefix)
        .bind(b.key)
        .bind(available(b.engine))
        .execute(&state.db)
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
pub struct Document {
    id: Uuid,
    item_id: Uuid,
    template_id: Option<Uuid>,
    template_name: String,
    engine: String,
    number: String,
    status: String,
    pdf_file_id: Option<Uuid>,
    source_file_id: Option<Uuid>,
    error: Option<String>,
    created_by: Option<Uuid>,
    created_by_name: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const DOC_SELECT: &str = "SELECT d.id, d.item_id, d.template_id, d.template_name, d.engine, d.number, d.status, d.pdf_file_id,
    d.source_file_id, d.error, d.created_by, NULLIF(trim(u.first_name || ' ' || u.last_name), '') AS created_by_name, d.created_at, d.updated_at
    FROM crm_documents d LEFT JOIN users u ON u.id = d.created_by";

async fn load_document(state: &AppState, id: Uuid) -> AppResult<Document> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{DOC_SELECT} WHERE d.id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("document"))
}

/// Checks the caller may `action` the record and returns its type.
async fn item_access(state: &AppState, auth: &AuthUser, item: Uuid, action: &str) -> AppResult<i16> {
    let row = load_row(state, item, false).await?;
    require(state, auth, row.entity_type, action, row.assigned_to).await?;
    Ok(row.entity_type)
}

async fn list_documents(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>) -> AppResult<Json<Value>> {
    item_access(&state, &auth, item, "read").await?;
    let items: Vec<Document> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{DOC_SELECT} WHERE d.item_id = $1 ORDER BY d.created_at DESC LIMIT 200")))
        .bind(item)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!({ "items": items })))
}

async fn get_document(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Document>> {
    let d = load_document(&state, id).await?;
    item_access(&state, &auth, d.item_id, "read").await?;
    Ok(Json(d))
}

/// Expands {YYYY}, {YY} and {MM} in a number prefix.
pub fn expand_prefix(prefix: &str, now: DateTime<Utc>) -> String {
    prefix
        .replace("{YYYY}", &format!("{:04}", now.year()))
        .replace("{YY}", &format!("{:02}", now.year() % 100))
        .replace("{MM}", &format!("{:02}", now.month()))
}

/// Next gap-free number for a prefix, inside the caller's transaction.
async fn next_number(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, prefix: &str) -> sqlx::Result<String> {
    let resolved = expand_prefix(prefix, Utc::now());
    let n: i64 = sqlx::query_scalar(
        "INSERT INTO crm_doc_counters (prefix, value) VALUES ($1, 1)
         ON CONFLICT (prefix) DO UPDATE SET value = crm_doc_counters.value + 1 RETURNING value",
    )
    .bind(&resolved)
    .fetch_one(&mut **tx)
    .await?;
    Ok(format!("{resolved}{n:04}"))
}

#[derive(Deserialize)]
struct CreateDoc {
    template_id: Uuid,
}

async fn create_document(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>, Json(b): Json<CreateDoc>) -> AppResult<Json<Document>> {
    let id = generate(&state, &auth, item, b.template_id).await?;
    Ok(Json(load_document(&state, id).await?))
}

/// Creates a document for a record (API and automation robots). Returns its id.
pub async fn generate(state: &AppState, auth: &AuthUser, item: Uuid, template_id: Uuid) -> AppResult<Uuid> {
    let (state, auth) = (state.clone(), auth.clone());
    let et = item_access(&state, &auth, item, "read").await?;
    let t = load_template(&state, template_id).await?;
    if !t.is_active || !t.entity_types.contains(&et) {
        return Err(AppError::bad("this template is not available for this record"));
    }
    if !t.available {
        return Err(AppError::bad(format!("{} is not installed on this server", engine_label(&t.engine))));
    }
    // Build first: it checks access and fails before a number is used up.
    let locale = if t.locale.is_empty() { default_locale(&state).await } else { t.locale.clone() };
    let mut built = data::build(&state, &auth, item, et, "", &t.doc_title, &locale).await?;

    let mut tx = state.db.begin().await?;
    let number = next_number(&mut tx, &t.number_prefix).await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO crm_documents (item_id, template_id, template_name, engine, number, created_by)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(item)
    .bind(t.id)
    .bind(&t.name)
    .bind(&t.engine)
    .bind(&number)
    .bind(auth.id)
    .fetch_one(&mut *tx)
    .await?;
    timeline(&mut *tx, item, "document", Some(auth.id), None, json!({ "document_id": id, "number": number, "template": t.name })).await?;
    tx.commit().await?;

    built.data["doc"]["number"] = json!(number);
    start(&state, id, &t, built).await?;
    let template = t.id.to_string();
    crate::crm::automation::fire(&state, item, "document_created", move |p| {
        p.get("template_id").and_then(Value::as_str).is_none_or(|x| x.is_empty() || x == template)
    })
    .await;
    Ok(id)
}

/// Typst renders right away; the external engines run in the job queue.
async fn start(state: &AppState, id: Uuid, t: &Template, built: data::Built) -> AppResult<()> {
    if t.engine == "typst" {
        finish(state, id, render(state, &t.engine, t.source.as_deref(), t.file_id, &built.data, built.logo_file).await).await?;
    } else {
        jobs::enqueue(
            &state.db,
            JOB,
            json!({ "document_id": id, "engine": t.engine, "source": t.source, "file_id": t.file_id, "data": built.data, "logo_file": built.logo_file }),
            None,
        )
        .await?;
        state.job_wakeup.notify_one();
    }
    Ok(())
}

/// Stores the result of a render and tells open record pages.
async fn finish(state: &AppState, id: Uuid, result: Result<Rendered, RenderError>) -> AppResult<()> {
    let d = load_document(state, id).await?;
    let old = [d.pdf_file_id, d.source_file_id];
    let safe: String = d.number.chars().map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '-' }).collect();
    match result {
        Ok(r) => {
            let pdf = files::store_bytes(state, d.created_by, &format!("{safe}.pdf"), "application/pdf", &r.pdf).await?;
            let docx = match &r.docx {
                Some(b) => Some(files::store_bytes(state, d.created_by, &format!("{safe}.docx"), "application/vnd.openxmlformats-officedocument.wordprocessingml.document", b).await?),
                None => None,
            };
            sqlx::query("UPDATE crm_documents SET status = 'ready', pdf_file_id = $2, source_file_id = $3, error = NULL, updated_at = now() WHERE id = $1")
                .bind(id)
                .bind(pdf)
                .bind(docx)
                .execute(&state.db)
                .await?;
            for f in old.into_iter().flatten() {
                files::delete_stored(state, f).await;
            }
        }
        Err(RenderError::Template(e)) | Err(RenderError::Server(e)) => {
            sqlx::query("UPDATE crm_documents SET status = 'failed', error = $2, updated_at = now() WHERE id = $1")
                .bind(id)
                .bind(e.chars().take(8000).collect::<String>())
                .execute(&state.db)
                .await?;
        }
    }
    notify(state, d.item_id, id).await;
    crate::crm::automation::document_finished(state, d.item_id, id).await;
    Ok(())
}

async fn notify(state: &AppState, item: Uuid, id: Uuid) {
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM crm_documents WHERE id = $1").bind(id).fetch_optional(&state.db).await.ok().flatten();
    realtime::publish(state, None, "crm.document", json!({ "item_id": item, "id": id, "status": status })).await;
}

/// Background job for LuaLaTeX and LibreOffice documents.
pub async fn run_job(state: &AppState, payload: &Value) -> anyhow::Result<()> {
    let id: Uuid = serde_json::from_value(payload["document_id"].clone())?;
    let engine = payload["engine"].as_str().unwrap_or_default();
    let file_id: Option<Uuid> = serde_json::from_value(payload["file_id"].clone()).unwrap_or(None);
    let logo: Option<Uuid> = serde_json::from_value(payload["logo_file"].clone()).unwrap_or(None);
    let result = render(state, engine, payload["source"].as_str(), file_id, &payload["data"], logo).await;
    // Server-side failures are retried by the queue; template errors are final.
    if let Err(RenderError::Server(e)) = &result {
        anyhow::bail!("rendering document {id}: {e}");
    }
    match finish(state, id, result).await {
        Ok(()) => Ok(()),
        // The document was deleted while it waited.
        Err(AppError::NotFound(_)) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("{e:?}")),
    }
}

/// Renders again with the record's current data, keeping the number.
async fn regenerate(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Document>> {
    let d = load_document(&state, id).await?;
    let et = item_access(&state, &auth, d.item_id, "read").await?;
    let t = load_template(&state, d.template_id.ok_or_else(|| AppError::bad("the template was deleted"))?).await?;
    if !t.available {
        return Err(AppError::bad(format!("{} is not installed on this server", engine_label(&t.engine))));
    }
    let locale = if t.locale.is_empty() { default_locale(&state).await } else { t.locale.clone() };
    let built = data::build(&state, &auth, d.item_id, et, &d.number, &t.doc_title, &locale).await?;
    sqlx::query("UPDATE crm_documents SET status = 'pending', error = NULL, template_name = $2, engine = $3, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(&t.name)
        .bind(&t.engine)
        .execute(&state.db)
        .await?;
    start(&state, id, &t, built).await?;
    Ok(Json(load_document(&state, id).await?))
}

async fn delete_document(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let d = load_document(&state, id).await?;
    item_access(&state, &auth, d.item_id, "read").await?;
    if d.created_by != Some(auth.id) {
        item_access(&state, &auth, d.item_id, "write").await?;
    }
    sqlx::query("DELETE FROM crm_documents WHERE id = $1").bind(id).execute(&state.db).await?;
    for f in [d.pdf_file_id, d.source_file_id].into_iter().flatten() {
        files::delete_stored(&state, f).await;
    }
    timeline(&state.db, d.item_id, "document_deleted", Some(auth.id), None, json!({ "number": d.number })).await?;
    notify(&state, d.item_id, id).await;
    Ok(Json(json!({ "ok": true })))
}

/// Files module hook: generated documents are readable by anyone who can read their
/// record; template files by CRM admins.
pub async fn can_view_file(state: &AppState, auth: &AuthUser, file: Uuid) -> AppResult<bool> {
    // Generated documents, and the copies sent for signature (as sent, and signed).
    let item: Option<Uuid> = sqlx::query_scalar(
        "SELECT item_id FROM crm_documents WHERE pdf_file_id = $1 OR source_file_id = $1
         UNION ALL SELECT item_id FROM sign_requests WHERE original_file_id = $1 OR signed_file_id = $1 LIMIT 1",
    )
        .bind(file)
        .fetch_optional(&state.db)
        .await?;
    if let Some(item) = item {
        return Ok(item_access(state, auth, item, "read").await.is_ok());
    }
    if is_crm_admin(auth) {
        let is_template: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_doc_templates WHERE file_id = $1)")
            .bind(file)
            .fetch_one(&state.db)
            .await?;
        return Ok(is_template);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn prefix_tokens() {
        let t = Utc.with_ymd_and_hms(2026, 3, 9, 0, 0, 0).unwrap();
        assert_eq!(expand_prefix("INV-{YYYY}-", t), "INV-2026-");
        assert_eq!(expand_prefix("{YY}{MM}/", t), "2603/");
        assert_eq!(expand_prefix("Q", t), "Q");
    }
}
