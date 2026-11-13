//! Sales intelligence: where clients come from.
//!
//! * A tracking script (`/track.js`) on the company's websites keeps a visitor id in the
//!   browser and reports each page. Pages within 30 minutes of each other, from one source,
//!   make a visit; each visit is put down to a channel (paid, search, social, email, referral,
//!   campaign, offline, direct) and a source (a CRM traffic source when its UTM tag matches).
//! * Web forms and live chat on those sites pass the visitor id along, so the record they
//!   create carries the visitor's journey: every visit before it, and what came after.
//! * Traffic analysis: visits, visitors, new records, deals, deals won and revenue by channel
//!   or source (first touch), against what was spent on them.
//! * Short links (and QR codes) for offline ads redirect to a page with UTM tags and count
//!   who came; visits through them count for the ad's source.
//!
//! Setting this up and the reports need "Run marketing" (or CRM administration).

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use redis::AsyncCommands;
use reqwest::Url;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, tokens::random_token, AuthUser, ClientIp},
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/public/track/{key}", post(hit))
        .route("/tracking/sites", get(list_sites).post(create_site))
        .route("/tracking/sites/{id}", patch(update_site).delete(delete_site))
        .route("/tracking/links", get(list_links).post(create_link))
        .route("/tracking/links/{id}", patch(update_link).delete(delete_link))
        .route("/tracking/links/{id}/qr.svg", get(link_qr))
        .route("/tracking/spend", get(list_spend).put(put_spend))
        .route("/tracking/report", get(report))
        .route("/crm/items/{id}/journey", get(journey))
}

pub const CLEANUP_JOB: &str = "tracking.cleanup";

/// Keeps tracking data bounded: page lists after 400 days, and visitors who never became a
/// CRM record and weren't seen for 400 days (with their visits). Journeys of records stay.
pub async fn cleanup(state: &AppState) -> anyhow::Result<()> {
    sqlx::query!("DELETE FROM track_pages WHERE at < now() - interval '400 days'").execute(&state.db).await?;
    sqlx::query!("DELETE FROM track_visitors WHERE item_id IS NULL AND last_seen < now() - interval '400 days'").execute(&state.db).await?;
    Ok(())
}

fn require_manage(auth: &AuthUser) -> AppResult<()> {
    if auth.can(perm::MARKETING) || crate::crm::access::is_crm_admin(auth) { Ok(()) } else { Err(AppError::Forbidden) }
}

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

const SEARCH: &[&str] = &["google.", "bing.com", "duckduckgo.com", "yahoo.", "yandex.", "baidu.com", "ecosia.org", "search.brave.com", "startpage.com", "qwant.com", "naver.com", "seznam.cz"];
const SOCIAL: &[&str] = &[
    "facebook.com", "fb.com", "instagram.com", "linkedin.com", "lnkd.in", "t.co", "twitter.com", "x.com", "youtube.com", "youtu.be", "tiktok.com",
    "reddit.com", "pinterest.", "vk.com", "threads.net", "bsky.app", "mastodon.", "whatsapp.com", "telegram.org", "t.me", "xing.com",
];
const PAID_MEDIUMS: &[&str] = &["cpc", "ppc", "paid", "paidsearch", "paid_search", "paid-social", "paidsocial", "paid_social", "display", "banner", "cpm", "retargeting"];
const CLICK_IDS: &[&str] = &["gclid", "gbraid", "wbraid", "fbclid", "msclkid", "yclid", "ttclid", "li_fat_id", "twclid"];

fn bare_host(u: &Url) -> String {
    u.host_str().unwrap_or("").trim_start_matches("www.").to_ascii_lowercase()
}

fn matches_host(host: &str, list: &[&str]) -> bool {
    list.iter().any(|d| if d.ends_with('.') { host.starts_with(d) || host.contains(&format!(".{d}")) } else { host == *d || host.ends_with(&format!(".{d}")) })
}

#[derive(Debug, PartialEq)]
pub struct Touch {
    pub channel: &'static str,
    pub source: String,
    pub utm: Map<String, Value>,
    /// The ad link the visit came through (its code), if any.
    pub link: Option<String>,
}

/// Where a visit comes from, from its landing address and referrer. `sources` are the CRM
/// traffic sources (code, utm_source values); `same_site` hosts aren't referrers.
pub fn classify(page: &Url, referrer: Option<&Url>, sources: &[(String, Vec<String>)]) -> Touch {
    let q: HashMap<String, String> = page.query_pairs().map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string())).collect();
    let mut utm = Map::new();
    for k in ["source", "medium", "campaign", "content", "term"] {
        if let Some(v) = q.get(&format!("utm_{k}")).filter(|v| !v.is_empty()) {
            utm.insert(k.into(), json!(v.chars().take(200).collect::<String>()));
        }
    }
    let utm_source = utm.get("source").and_then(Value::as_str).map(str::to_ascii_lowercase);
    let medium = utm.get("medium").and_then(Value::as_str).map(str::to_ascii_lowercase).unwrap_or_default();
    let own = utm_source.as_deref().and_then(|s| sources.iter().find(|(_, u)| u.iter().any(|x| x == s)).map(|(c, _)| c.clone()));
    let link = q.get("nerp_ad").filter(|v| !v.is_empty()).cloned();
    let page_host = bare_host(page);
    let ref_host = referrer.map(bare_host).filter(|h| !h.is_empty() && *h != page_host);

    let source = own.clone().or(utm_source.clone()).or(ref_host.clone()).unwrap_or_else(|| "(direct)".into());
    let channel = if link.is_some() && medium == "offline" {
        "offline"
    } else if PAID_MEDIUMS.contains(&medium.as_str()) || CLICK_IDS.iter().any(|c| q.contains_key(*c)) {
        "paid"
    } else if medium == "email" || medium == "newsletter" || utm_source.as_deref() == Some("email") || utm_source.as_deref() == Some("newsletter") {
        "email"
    } else if medium == "social" || medium == "social-media" || medium == "sm" {
        "social"
    } else if medium == "offline" || medium == "print" || medium == "qr" {
        "offline"
    } else if utm_source.is_some() {
        "campaign"
    } else if let Some(h) = &ref_host {
        if matches_host(h, SEARCH) {
            "search"
        } else if matches_host(h, SOCIAL) {
            "social"
        } else if h.starts_with("mail.") || h.contains("webmail") || h.contains("outlook.") {
            "email"
        } else {
            "referral"
        }
    } else {
        "direct"
    };
    // A click id without UTM tags: name the ad network.
    let source = if source == "(direct)" || ref_host.as_deref() == Some(source.as_str()) {
        match () {
            _ if q.contains_key("gclid") || q.contains_key("gbraid") || q.contains_key("wbraid") => "google ads".to_string(),
            _ if q.contains_key("fbclid") && channel == "paid" => "meta ads".to_string(),
            _ if q.contains_key("msclkid") => "microsoft ads".to_string(),
            _ => source,
        }
    } else {
        source
    };
    Touch { channel, source, utm, link }
}

// ---------------------------------------------------------------------------
// The script's reports
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Hit {
    /// Visitor id.
    v: String,
    /// Page address, title, referrer.
    u: String,
    #[serde(default)]
    t: Option<String>,
    #[serde(default)]
    r: Option<String>,
}

async fn limited(state: &AppState, key: String, max: i64, secs: i64) -> AppResult<bool> {
    let mut redis = state.redis.clone();
    let n: i64 = redis.incr(&key, 1).await?;
    if n == 1 {
        let _: () = redis.expire(&key, secs).await?;
    }
    Ok(n > max)
}

/// A page view from the script. Sent with `navigator.sendBeacon` (text/plain, no preflight),
/// so it answers 204 whatever happens; bad reports are dropped.
async fn hit(State(state): State<AppState>, ClientIp(ip): ClientIp, Path(key): Path<String>, body: Bytes) -> StatusCode {
    if let Err(e) = record_hit(&state, ip, &key, &body).await {
        tracing::debug!(error = ?e, "tracking hit dropped");
    }
    StatusCode::NO_CONTENT
}

async fn record_hit(state: &AppState, ip: Option<String>, key: &str, body: &[u8]) -> AppResult<()> {
    if body.len() > 8192 {
        return Err(AppError::bad("too big"));
    }
    let h: Hit = serde_json::from_slice(body).map_err(|_| AppError::bad("bad report"))?;
    let vid = Uuid::parse_str(&h.v).map_err(|_| AppError::bad("bad visitor"))?;
    let page = Url::parse(&h.u.chars().take(2000).collect::<String>()).map_err(|_| AppError::bad("bad page"))?;
    if !["http", "https"].contains(&page.scheme()) {
        return Err(AppError::bad("bad page"));
    }
    let site: Option<(Uuid, Vec<String>)> = sqlx::query!(r#"SELECT id, domains FROM track_sites WHERE key = $1 AND is_active"#, key).map(|r| (r.id, r.domains)).fetch_optional(&state.db).await?;
    let Some((site, domains)) = site else { return Err(AppError::NotFound("site")) };
    let host = bare_host(&page);
    if !domains.is_empty() && !domains.iter().any(|d| host == *d || host.ends_with(&format!(".{d}"))) {
        return Err(AppError::bad("not this site's page"));
    }
    let who = ip.unwrap_or_else(|| "unknown".into());
    if limited(state, format!("track:ip:{who}"), 1200, 600).await? || limited(state, format!("track:v:{vid}"), 120, 60).await? {
        return Err(AppError::TooManyRequests("slow down".into()));
    }
    let referrer = h.r.as_deref().filter(|r| !r.is_empty()).and_then(|r| Url::parse(&r.chars().take(2000).collect::<String>()).ok()).filter(|r| ["http", "https"].contains(&r.scheme()));
    let title = h.t.map(|t| t.trim().chars().take(300).collect::<String>()).filter(|t| !t.is_empty());

    sqlx::query!("INSERT INTO track_visitors (id) VALUES ($1) ON CONFLICT (id) DO UPDATE SET last_seen = now()", vid).execute(&state.db).await?;
    let sources: Vec<(String, Vec<String>)> = sqlx::query!(r#"SELECT code, utm FROM crm_traffic_sources"#).map(|r| (r.code, r.utm)).fetch_all(&state.db).await?;
    let touch = classify(&page, referrer.as_ref(), &sources);
    let last: Option<(Uuid, DateTime<Utc>, String, String, i32)> =
        sqlx::query!(r#"SELECT id, last_at, channel, source, pages FROM track_visits WHERE visitor_id = $1 ORDER BY started_at DESC LIMIT 1"#, vid).map(|r| (r.id, r.last_at, r.channel, r.source, r.pages)).fetch_optional(&state.db).await?;
    // A new visit after half an hour, or when the page was reached from somewhere else.
    let tagged = !touch.utm.is_empty() || touch.link.is_some() || page.query_pairs().any(|(k, _)| CLICK_IDS.contains(&k.as_ref()));
    let from_elsewhere = referrer.as_ref().is_some_and(|r| bare_host(r) != host);
    let visit = match last {
        Some((id, at, ch, src, pages)) if at > Utc::now() - Duration::minutes(30) && !(tagged || from_elsewhere) || last_same(&touch, &ch, &src, at) => {
            if pages >= 200 {
                sqlx::query!("UPDATE track_visits SET last_at = now() WHERE id = $1", id).execute(&state.db).await?;
                return Ok(());
            }
            sqlx::query!("UPDATE track_visits SET last_at = now(), pages = pages + 1 WHERE id = $1", id).execute(&state.db).await?;
            id
        }
        _ => {
            let link: Option<(Uuid, String)> = match &touch.link {
                Some(code) => sqlx::query_as("SELECT id, source FROM track_links WHERE code = $1").bind(code).fetch_optional(&state.db).await?,
                None => None,
            };
            let (channel, source) = match &link {
                Some((_, src)) => (if touch.channel == "direct" || touch.channel == "campaign" { "offline" } else { touch.channel }, src.clone()),
                None => (touch.channel, touch.source.clone()),
            };
            sqlx::query_scalar!("INSERT INTO track_visits (visitor_id, site_id, channel, source, utm, referrer, landing, pages, link_id)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, 1, $8) RETURNING id", vid, site, channel, source.chars().take(200).collect::<String>(), Value::Object(touch.utm.clone()), referrer.as_ref().map(|r| r.as_str().to_string()), page.as_str(), link.map(|l| l.0))
            .fetch_one(&state.db)
            .await?
        }
    };
    sqlx::query!("INSERT INTO track_pages (visit_id, url, title) VALUES ($1, $2, $3)", visit, page.as_str(), title).execute(&state.db).await?;
    Ok(())
}

/// The same tagged landing reloaded within the half hour (a refresh): still the same visit.
fn last_same(t: &Touch, channel: &str, source: &str, at: DateTime<Utc>) -> bool {
    at > Utc::now() - Duration::minutes(30) && t.channel == channel && t.source == source && t.link.is_none()
}

/// A CRM record made from this visitor (a web form, a chat): the visitor is theirs. A visitor
/// already linked keeps its first record.
pub async fn link_visitor(state: &AppState, visitor: Option<&str>, item: Uuid) {
    let Some(v) = visitor.and_then(|v| Uuid::parse_str(v.trim()).ok()) else { return };
    if let Err(e) = sqlx::query!("UPDATE track_visitors SET item_id = COALESCE(item_id, $2) WHERE id = $1", v, item).execute(&state.db).await {
        tracing::warn!(error = ?e, "could not link the visitor");
    }
}

// ---------------------------------------------------------------------------
// Sites
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct Site {
    id: Uuid,
    name: String,
    domains: Vec<String>,
    key: String,
    is_active: bool,
    created_at: DateTime<Utc>,
}

fn site_json(state: &AppState, s: &Site) -> Value {
    let origin = &state.config.public_url;
    json!({
        "id": s.id, "name": s.name, "domains": s.domains, "key": s.key, "is_active": s.is_active, "created_at": s.created_at,
        "snippet": format!("<script src=\"{origin}/track.js\" data-site=\"{}\" async></script>", s.key),
    })
}

async fn list_sites(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require_manage(&auth)?;
    let sites: Vec<Site> = sqlx::query_as!(Site, "SELECT id, name, domains, key, is_active, created_at FROM track_sites ORDER BY created_at").fetch_all(&state.db).await?;
    let stats: Vec<(Uuid, i64)> = sqlx::query!(r#"SELECT site_id AS "site_id!", COUNT(*) AS "c1!" FROM track_visits WHERE started_at > now() - interval '30 days' GROUP BY site_id"#).map(|r| (r.site_id, r.c1)).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": sites.iter().map(|s| {
        let mut v = site_json(&state, s);
        v["visits_30d"] = json!(stats.iter().find(|x| x.0 == s.id).map(|x| x.1).unwrap_or(0));
        v
    }).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct SiteInput {
    name: Option<String>,
    domains: Option<Vec<String>>,
    is_active: Option<bool>,
}

fn clean_domains(d: &[String]) -> AppResult<Vec<String>> {
    let mut out = Vec::new();
    for x in d {
        let x = x.trim().to_ascii_lowercase();
        let x = x.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').trim_start_matches("www.").to_string();
        if x.is_empty() {
            continue;
        }
        if x.len() > 253 || !x.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') || !x.contains('.') {
            return Err(AppError::bad(format!("“{x}” isn't a host name like example.com")));
        }
        if !out.contains(&x) {
            out.push(x);
        }
    }
    if out.len() > 50 {
        return Err(AppError::bad("at most 50 host names"));
    }
    Ok(out)
}

async fn create_site(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SiteInput>) -> AppResult<Json<Value>> {
    require_manage(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 200)?;
    let domains = clean_domains(&b.domains.unwrap_or_default())?;
    let s: Site = sqlx::query_as!(Site, "INSERT INTO track_sites (name, domains, key, created_by) VALUES ($1, $2, $3, $4) RETURNING id, name, domains, key, is_active, created_at", name, &domains, random_token(12), auth.id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(site_json(&state, &s)))
}

async fn update_site(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<SiteInput>) -> AppResult<Json<Value>> {
    require_manage(&auth)?;
    let name = match &b.name {
        Some(n) => Some(required("name", n, 200)?),
        None => None,
    };
    let domains = match &b.domains {
        Some(d) => Some(clean_domains(d)?),
        None => None,
    };
    let s: Site = sqlx::query_as!(Site, "UPDATE track_sites SET name = COALESCE($2, name), domains = COALESCE($3, domains), is_active = COALESCE($4, is_active) WHERE id = $1
         RETURNING id, name, domains, key, is_active, created_at", id, name, domains.as_deref(), b.is_active)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("site"))?;
    Ok(Json(site_json(&state, &s)))
}

async fn delete_site(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manage(&auth)?;
    let n = sqlx::query!("DELETE FROM track_sites WHERE id = $1", id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("site"));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Ad links
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct Link {
    id: Uuid,
    code: String,
    name: String,
    target_url: String,
    source: String,
    kind: String,
    utm: Value,
    clicks: i64,
    is_active: bool,
    created_at: DateTime<Utc>,
}

const LINK_COLS: &str = "id, code, name, target_url, source, kind, utm, clicks, is_active, created_at";

async fn link_json(state: &AppState, l: &Link) -> AppResult<Value> {
    let (visits, visitors, records): (i64, i64, i64) = sqlx::query!(r#"SELECT COUNT(*) AS "c0!", COUNT(DISTINCT v.visitor_id) AS "c1!", COUNT(DISTINCT tv.item_id) AS "c2!"
         FROM track_visits v JOIN track_visitors tv ON tv.id = v.visitor_id WHERE v.link_id = $1"#, l.id).map(|r| (r.c0, r.c1, r.c2))
    .fetch_one(&state.db)
    .await?;
    let mut v = serde_json::to_value(l).unwrap_or_default();
    v["short_url"] = json!(format!("{}/go/{}", state.config.public_url, l.code));
    v["visits"] = json!(visits);
    v["visitors"] = json!(visitors);
    v["records"] = json!(records);
    Ok(v)
}

async fn list_links(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require_manage(&auth)?;
    let links: Vec<Link> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {LINK_COLS} FROM track_links ORDER BY created_at DESC"))).fetch_all(&state.db).await?;
    let mut out = Vec::new();
    for l in &links {
        out.push(link_json(&state, l).await?);
    }
    Ok(Json(json!({ "items": out })))
}

#[derive(Deserialize)]
struct LinkInput {
    name: Option<String>,
    target_url: Option<String>,
    source: Option<String>,
    kind: Option<String>,
    /// utm_campaign (and content/term) to add; source and medium come from the link.
    campaign: Option<String>,
    is_active: Option<bool>,
}

fn check_target(u: &str) -> AppResult<String> {
    let url = crate::outbound::check_url(u.trim()).map_err(AppError::bad)?;
    Ok(url.to_string())
}

async fn create_link(State(state): State<AppState>, auth: AuthUser, Json(b): Json<LinkInput>) -> AppResult<Json<Value>> {
    require_manage(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 200)?;
    let target = check_target(b.target_url.as_deref().unwrap_or(""))?;
    let source = required("source", b.source.as_deref().unwrap_or(""), 100)?.to_lowercase();
    let kind = b.kind.unwrap_or_else(|| "offline".into());
    if !["offline", "online"].contains(&kind.as_str()) {
        return Err(AppError::bad("kind must be offline or online"));
    }
    let mut utm = Map::new();
    if let Some(c) = clean(b.campaign).map(|c| c.chars().take(200).collect::<String>()) {
        utm.insert("campaign".into(), json!(c));
    }
    let code = random_token(5).to_lowercase();
    let l: Link = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO track_links (code, name, target_url, source, kind, utm, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING {LINK_COLS}"
    )))
    .bind(code)
    .bind(name)
    .bind(target)
    .bind(source)
    .bind(kind)
    .bind(Value::Object(utm))
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(link_json(&state, &l).await?))
}

async fn update_link(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<LinkInput>) -> AppResult<Json<Value>> {
    require_manage(&auth)?;
    let name = match &b.name {
        Some(n) => Some(required("name", n, 200)?),
        None => None,
    };
    let target = match &b.target_url {
        Some(t) => Some(check_target(t)?),
        None => None,
    };
    let l: Link = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE track_links SET name = COALESCE($2, name), target_url = COALESCE($3, target_url), is_active = COALESCE($4, is_active) WHERE id = $1 RETURNING {LINK_COLS}"
    )))
    .bind(id)
    .bind(name)
    .bind(target)
    .bind(b.is_active)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("link"))?;
    Ok(Json(link_json(&state, &l).await?))
}

async fn delete_link(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manage(&auth)?;
    let n = sqlx::query!("DELETE FROM track_links WHERE id = $1", id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("link"));
    }
    Ok(Json(json!({ "ok": true })))
}

/// The short link as a QR code, for print.
async fn link_qr(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Response> {
    require_manage(&auth)?;
    let code: String = sqlx::query_scalar!("SELECT code FROM track_links WHERE id = $1", id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("link"))?;
    let url = format!("{}/go/{code}", state.config.public_url);
    let qr = qrcodegen::QrCode::encode_text(&url, qrcodegen::QrCodeEcc::Medium).map_err(|_| AppError::bad("the link is too long for a QR code"))?;
    let border = 4;
    let size = qr.size() + border * 2;
    let mut path = String::new();
    for y in 0..qr.size() {
        for x in 0..qr.size() {
            if qr.get_module(x, y) {
                path.push_str(&format!("M{},{}h1v1h-1z", x + border, y + border));
            }
        }
    }
    let svg = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {size} {size}\" shape-rendering=\"crispEdges\"><rect width=\"100%\" height=\"100%\" fill=\"#fff\"/><path d=\"{path}\" fill=\"#000\"/></svg>\n"
    );
    let mut res = svg.into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("image/svg+xml"));
    res.headers_mut().insert(header::CONTENT_DISPOSITION, HeaderValue::from_str(&format!("inline; filename=\"qr-{code}.svg\"")).unwrap());
    Ok(res)
}

/// `/go/{code}`: an ad's short link. Counts the click (once an hour per address) and sends
/// the person on to the page with UTM tags and the ad's code.
pub async fn go(State(state): State<AppState>, ClientIp(ip): ClientIp, Path(code): Path<String>) -> Response {
    let link: Option<(Uuid, String, String, String, Value)> = sqlx::query!(r#"SELECT id, target_url, source, kind, utm FROM track_links WHERE code = $1 AND is_active"#, code.to_lowercase()).map(|r| (r.id, r.target_url, r.source, r.kind, r.utm))
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    let Some((id, target, source, kind, utm)) = link else {
        return (StatusCode::NOT_FOUND, "This link doesn't exist or was switched off.").into_response();
    };
    let mut redis = state.redis.clone();
    let once: bool = redis::cmd("SET")
        .arg(format!("track:go:{id}:{}", ip.unwrap_or_default()))
        .arg(1)
        .arg("NX")
        .arg("EX")
        .arg(3600)
        .query_async(&mut redis)
        .await
        .map(|v: Option<String>| v.is_some())
        .unwrap_or(false);
    if once {
        let _ = sqlx::query!("UPDATE track_links SET clicks = clicks + 1 WHERE id = $1", id).execute(&state.db).await;
    }
    let mut url = match Url::parse(&target) {
        Ok(u) => u,
        Err(_) => return (StatusCode::NOT_FOUND, "This link is broken.").into_response(),
    };
    {
        let existing: Vec<String> = url.query_pairs().map(|(k, _)| k.to_string()).collect();
        let mut q = url.query_pairs_mut();
        if !existing.iter().any(|k| k == "utm_source") {
            q.append_pair("utm_source", &source);
        }
        if !existing.iter().any(|k| k == "utm_medium") {
            q.append_pair("utm_medium", if kind == "offline" { "offline" } else { "link" });
        }
        if let Some(c) = utm.get("campaign").and_then(Value::as_str)
            && !existing.iter().any(|k| k == "utm_campaign")
        {
            q.append_pair("utm_campaign", c);
        }
        q.append_pair("nerp_ad", &code.to_lowercase());
    }
    Redirect::to(url.as_str()).into_response()
}

// ---------------------------------------------------------------------------
// Spend
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SpendQuery {
    year: Option<i32>,
}

async fn list_spend(State(state): State<AppState>, auth: AuthUser, Query(q): Query<SpendQuery>) -> AppResult<Json<Value>> {
    require_manage(&auth)?;
    let year = q.year.unwrap_or_else(|| Utc::now().year());
    let rows: Vec<(String, NaiveDate, Decimal, String, Decimal, Option<String>)> = sqlx::query!(r#"SELECT source, month, amount, currency, amount_base, note FROM track_spend WHERE extract(year FROM month) = $1 ORDER BY source, month"#, year as f64).map(|r| (r.source, r.month, r.amount, r.currency, r.amount_base, r.note))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "year": year,
        "base_currency": crate::crm::money::base_currency(&state).await?,
        "items": rows.into_iter().map(|(s, m, a, c, b, n)| json!({ "source": s, "month": m.format("%Y-%m").to_string(), "amount": a.to_string(), "currency": c, "amount_base": b.to_string(), "note": n })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct SpendInput {
    /// A channel (paid, search…) or a source.
    source: String,
    /// YYYY-MM.
    month: String,
    /// Empty or zero removes the entry.
    amount: Option<String>,
    currency: Option<String>,
    note: Option<String>,
}

async fn put_spend(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SpendInput>) -> AppResult<Json<Value>> {
    require_manage(&auth)?;
    let source = required("source", &b.source, 100)?.to_lowercase();
    let month = NaiveDate::parse_from_str(&format!("{}-01", b.month.trim()), "%Y-%m-%d").map_err(|_| AppError::bad("month must look like 2026-11"))?;
    let amount = match clean(b.amount) {
        None => Decimal::ZERO,
        Some(a) => Decimal::from_str(&a).map_err(|_| AppError::bad("amount must be a number"))?,
    };
    if amount.is_sign_negative() {
        return Err(AppError::bad("amount cannot be negative"));
    }
    if amount.is_zero() {
        sqlx::query!("DELETE FROM track_spend WHERE source = $1 AND month = $2", &source, month).execute(&state.db).await?;
        return Ok(Json(json!({ "ok": true, "removed": true })));
    }
    let base = crate::crm::money::base_currency(&state).await?;
    let currency = clean(b.currency).map(|c| c.to_uppercase()).unwrap_or(base);
    let rate = crate::crm::money::rate_of(&state, &currency).await?.ok_or_else(|| AppError::bad(format!("currency {currency} is not enabled")))?;
    sqlx::query!("INSERT INTO track_spend (source, month, amount, currency, amount_base, note, updated_by) VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (source, month) DO UPDATE SET amount = EXCLUDED.amount, currency = EXCLUDED.currency, amount_base = EXCLUDED.amount_base,
             note = EXCLUDED.note, updated_by = EXCLUDED.updated_by, updated_at = now()", &source, month, amount, &currency, (amount * rate).round_dp(4), clean(b.note).map(|n| n.chars().take(500).collect::<String>()), auth.id)
    .execute(&state.db)
    .await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Traffic analysis
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ReportQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    /// channel (default) or source.
    by: Option<String>,
    site_id: Option<Uuid>,
}

#[derive(Default, Serialize, Clone)]
struct Row {
    key: String,
    visits: i64,
    visitors: i64,
    new_visitors: i64,
    records: i64,
    deals: i64,
    won: i64,
    revenue: Decimal,
    spend: Decimal,
}

/// Visits in the period by channel or source, and what the visitors who first came that way
/// became: CRM records made, deals opened and won (with revenue, in the base currency), against
/// the spend entered for that channel or source.
async fn report(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ReportQuery>) -> AppResult<Json<Value>> {
    require_manage(&auth)?;
    let today = Utc::now().date_naive();
    let from = q.from.unwrap_or_else(|| today.with_day(1).unwrap());
    let to = q.to.unwrap_or(today);
    if to < from || (to - from).num_days() > 3 * 366 {
        return Err(AppError::bad("choose a period of up to three years"));
    }
    // What the rows are: the channel, the source, the campaign (utm_campaign) or the landing page
    // (its path, whatever the host name and query).
    let by = q.by.as_deref().unwrap_or("channel");
    let col = match by {
        "channel" => "v.channel",
        "source" => "v.source",
        "campaign" => "COALESCE(NULLIF(v.utm->>'campaign', ''), '(none)')",
        "landing" => "COALESCE(NULLIF(regexp_replace(split_part(split_part(v.landing, '?', 1), '#', 1), '^[a-z]+://[^/]+', ''), ''), '/')",
        _ => return Err(AppError::bad("by must be channel, source, campaign or landing")),
    };
    let (start, end) = (from.and_hms_opt(0, 0, 0).unwrap().and_utc(), (to + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap().and_utc());

    let traffic: Vec<(String, i64, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {col}, COUNT(*), COUNT(DISTINCT v.visitor_id), COUNT(DISTINCT v.visitor_id) FILTER (WHERE tv.first_seen >= $1)
         FROM track_visits v JOIN track_visitors tv ON tv.id = v.visitor_id
         WHERE v.started_at >= $1 AND v.started_at < $2 AND ($3::uuid IS NULL OR v.site_id = $3) GROUP BY 1"
    )))
    .bind(start)
    .bind(end)
    .bind(q.site_id)
    .fetch_all(&state.db)
    .await?;

    // First touch of every visitor linked to a record, and the deals that record leads to (the
    // record itself, or deals of the contact or company it is).
    let outcomes: Vec<(String, i64, i64, i64, Decimal)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "WITH first AS (
             SELECT DISTINCT ON (v.visitor_id) v.visitor_id, {col} AS k FROM track_visits v
             WHERE ($3::uuid IS NULL OR v.site_id = $3) ORDER BY v.visitor_id, v.started_at),
         linked AS (
             SELECT f.k, tv.item_id FROM first f JOIN track_visitors tv ON tv.id = f.visitor_id WHERE tv.item_id IS NOT NULL),
         deals AS (
             SELECT DISTINCT l.k, d.id, d.created_at, d.stage_semantics, d.closed_at, d.opportunity_base
             FROM linked l JOIN crm_items d ON d.entity_type = 2 AND d.deleted_at IS NULL
                 AND (d.id = l.item_id OR d.company_id = l.item_id OR EXISTS (SELECT 1 FROM crm_item_contacts c WHERE c.item_id = d.id AND c.contact_id = l.item_id)))
         SELECT k,
             (SELECT COUNT(DISTINCT l.item_id) FROM linked l JOIN crm_items i ON i.id = l.item_id WHERE l.k = x.k AND i.created_at >= $1 AND i.created_at < $2 AND i.deleted_at IS NULL),
             COUNT(*) FILTER (WHERE created_at >= $1 AND created_at < $2),
             COUNT(*) FILTER (WHERE stage_semantics = 'success' AND closed_at >= $1 AND closed_at < $2),
             COALESCE(SUM(opportunity_base) FILTER (WHERE stage_semantics = 'success' AND closed_at >= $1 AND closed_at < $2), 0)
         FROM (SELECT DISTINCT k FROM linked) x LEFT JOIN deals USING (k) GROUP BY k"
    )))
    .bind(start)
    .bind(end)
    .bind(q.site_id)
    .fetch_all(&state.db)
    .await?;

    let spend: Vec<(String, Decimal)> = sqlx::query!(r#"SELECT source AS "source!", SUM(amount_base) AS "c1!" FROM track_spend WHERE month >= date_trunc('month', $1::date) AND month <= $2 GROUP BY source"#, from, to).map(|r| (r.source, r.c1))
        .fetch_all(&state.db)
        .await?;

    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    for (k, visits, visitors, new_visitors) in traffic {
        let r = rows.entry(k.clone()).or_insert_with(|| Row { key: k, ..Default::default() });
        r.visits = visits;
        r.visitors = visitors;
        r.new_visitors = new_visitors;
    }
    for (k, records, deals, won, revenue) in outcomes {
        if records == 0 && deals == 0 && won == 0 {
            continue;
        }
        let r = rows.entry(k.clone()).or_insert_with(|| Row { key: k, ..Default::default() });
        r.records = records;
        r.deals = deals;
        r.won = won;
        r.revenue = revenue;
    }
    // Spend is entered per source: it has no place among campaigns or pages.
    for (k, amount) in spend.into_iter().filter(|_| by == "channel" || by == "source") {
        let r = rows.entry(k.clone()).or_insert_with(|| Row { key: k, ..Default::default() });
        r.spend = amount;
    }
    let mut list: Vec<Row> = rows.into_values().collect();
    list.sort_by(|a, b| b.visits.cmp(&a.visits).then(b.revenue.cmp(&a.revenue)));
    // A visitor can come several ways: the totals count each once.
    let (all_visitors, all_new): (i64, i64) = sqlx::query!(r#"SELECT COUNT(DISTINCT v.visitor_id) AS "c0!", COUNT(DISTINCT v.visitor_id) FILTER (WHERE tv.first_seen >= $1) AS "c1!"
         FROM track_visits v JOIN track_visitors tv ON tv.id = v.visitor_id
         WHERE v.started_at >= $1 AND v.started_at < $2 AND ($3::uuid IS NULL OR v.site_id = $3)"#, start, end, q.site_id).map(|r| (r.c0, r.c1))
    .fetch_one(&state.db)
    .await?;
    let mut total = Row { key: "total".into(), visitors: all_visitors, new_visitors: all_new, ..Default::default() };
    for r in &list {
        total.visits += r.visits;
        total.records += r.records;
        total.deals += r.deals;
        total.won += r.won;
        total.revenue += r.revenue;
        total.spend += r.spend;
    }
    let row_json = |r: &Row| {
        let ratio = |a: Decimal, b: i64| if b == 0 { None } else { Some((a / Decimal::from(b)).round_dp(2).to_string()) };
        json!({
            "key": r.key, "visits": r.visits, "visitors": r.visitors, "new_visitors": r.new_visitors,
            "records": r.records, "deals": r.deals, "won": r.won,
            "revenue": r.revenue.round_dp(2).to_string(), "spend": r.spend.round_dp(2).to_string(),
            "conversion": if r.visitors == 0 { None } else { Some(((r.records as f64) * 1000.0 / r.visitors as f64).round() / 10.0) },
            "cost_per_record": ratio(r.spend, r.records),
            "roi": if r.spend.is_zero() { None } else { Some(((r.revenue - r.spend) * Decimal::ONE_HUNDRED / r.spend).round_dp(1).to_string()) },
        })
    };
    // Visits per day, for the chart.
    let daily: Vec<(NaiveDate, i64)> = sqlx::query!(r#"SELECT started_at::date AS "c0!", COUNT(*) AS "c1!" FROM track_visits WHERE started_at >= $1 AND started_at < $2 AND ($3::uuid IS NULL OR site_id = $3) GROUP BY 1 ORDER BY 1"#, start, end, q.site_id).map(|r| (r.c0, r.c1))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "period": { "from": from, "to": to },
        "by": by,
        "base_currency": crate::crm::money::base_currency(&state).await?,
        "rows": list.iter().map(row_json).collect::<Vec<_>>(),
        "total": row_json(&total),
        "daily": daily.into_iter().map(|(d, n)| json!({ "date": d, "visits": n })).collect::<Vec<_>>(),
    })))
}

// ---------------------------------------------------------------------------
// A record's journey
// ---------------------------------------------------------------------------

/// Every visit of the visitors behind a record (itself, or for a deal its contacts and
/// company), each with its pages, and the record's own milestones, oldest first.
async fn journey(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let row = crate::crm::items::load_row(&state, id, false).await?;
    crate::crm::access::require_read(&state, &auth, row.entity_type, id, row.assigned_to).await?;
    let mut people = vec![id];
    if let Some(c) = row.company_id {
        people.push(c);
    }
    let contacts: Vec<Uuid> = sqlx::query_scalar!("SELECT contact_id FROM crm_item_contacts WHERE item_id = $1", id).fetch_all(&state.db).await?;
    people.extend(contacts);
    let visits: Vec<(Uuid, DateTime<Utc>, DateTime<Utc>, String, String, Value, Option<String>, String, i32, Option<String>)> = sqlx::query!(r#"SELECT v.id AS "id!", v.started_at AS "started_at!", v.last_at AS "last_at!", v.channel AS "channel!", v.source AS "source!", v.utm AS "utm!", v.referrer AS "referrer?", v.landing AS "landing!", v.pages AS "pages!", l.name AS "name?"
         FROM track_visits v JOIN track_visitors tv ON tv.id = v.visitor_id LEFT JOIN track_links l ON l.id = v.link_id
         WHERE tv.item_id = ANY($1) ORDER BY v.started_at LIMIT 500"#, &people).map(|r| (r.id, r.started_at, r.last_at, r.channel, r.source, r.utm, r.referrer, r.landing, r.pages, r.name))
    .fetch_all(&state.db)
    .await?;
    let ids: Vec<Uuid> = visits.iter().map(|v| v.0).collect();
    let pages: Vec<(Uuid, DateTime<Utc>, String, Option<String>)> =
        sqlx::query!(r#"SELECT visit_id, at, url, title FROM track_pages WHERE visit_id = ANY($1) ORDER BY at"#, &ids).map(|r| (r.visit_id, r.at, r.url, r.title)).fetch_all(&state.db).await?;
    let mut by_visit: HashMap<Uuid, Vec<Value>> = HashMap::new();
    for (v, at, url, title) in pages {
        by_visit.entry(v).or_default().push(json!({ "at": at, "url": url, "title": title }));
    }
    let mut events = vec![json!({ "kind": "created", "at": row.created_at })];
    let stages: Vec<(DateTime<Utc>, String, String)> = sqlx::query_as(
        "SELECT h.created_at, s.name, h.semantics FROM crm_stage_history h JOIN crm_stages s ON s.id = h.stage_id WHERE h.item_id = $1 ORDER BY h.created_at",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    for (at, name, sem) in stages {
        events.push(json!({ "kind": if sem == "success" { "won" } else if sem == "failure" { "lost" } else { "stage" }, "at": at, "stage": name }));
    }
    let first = visits.first().map(|v| json!({ "channel": v.3, "source": v.4 }));
    Ok(Json(json!({
        "first_touch": first,
        "visits": visits.into_iter().map(|(vid, start, last, ch, src, utm, refr, landing, pages, link)| json!({
            "id": vid, "started_at": start, "last_at": last, "channel": ch, "source": src, "utm": utm, "referrer": refr,
            "landing": landing, "pages": pages, "ad": link, "page_list": by_visit.remove(&vid).unwrap_or_default(),
        })).collect::<Vec<_>>(),
        "events": events,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(page: &str, referrer: Option<&str>) -> Touch {
        let sources = vec![("partner".to_string(), vec!["acme-partner".to_string()])];
        classify(&Url::parse(page).unwrap(), referrer.map(|r| Url::parse(r).unwrap()).as_ref(), &sources)
    }

    #[test]
    fn channels() {
        let t = c("https://shop.example/", None);
        assert_eq!((t.channel, t.source.as_str()), ("direct", "(direct)"));
        let t = c("https://shop.example/x", Some("https://www.google.com/search?q=x"));
        assert_eq!((t.channel, t.source.as_str()), ("search", "google.com"));
        assert_eq!(c("https://shop.example/", Some("https://l.facebook.com/")).channel, "social");
        assert_eq!(c("https://shop.example/", Some("https://blog.other.example/post")).channel, "referral");
        assert_eq!(c("https://shop.example/?utm_source=google&utm_medium=cpc", None).channel, "paid");
        let t = c("https://shop.example/?gclid=abc", Some("https://www.google.com/"));
        assert_eq!((t.channel, t.source.as_str()), ("paid", "google ads"));
        assert_eq!(c("https://shop.example/?utm_source=newsletter&utm_medium=email", None).channel, "email");
        let t = c("https://shop.example/?utm_source=Acme-Partner&utm_campaign=spring", None);
        assert_eq!((t.channel, t.source.as_str()), ("campaign", "partner"));
        assert_eq!(t.utm["campaign"], "spring");
        let t = c("https://shop.example/?utm_source=poster&utm_medium=offline&nerp_ad=ab12c", None);
        assert_eq!((t.channel, t.link.as_deref()), ("offline", Some("ab12c")));
        // The site's own pages aren't referrers.
        assert_eq!(c("https://shop.example/b", Some("https://www.shop.example/a")).channel, "direct");
    }
}
