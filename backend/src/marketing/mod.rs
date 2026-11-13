//! Marketing: segments (saved audiences of CRM records), email campaigns sent to them with
//! open and click tracking, and the opt-out list every campaign honours.
//!
//! Everything here needs the "Run marketing" permission. Segments use the same filters as
//! the CRM lists and see every record (a campaign speaks for the company, not for one
//! employee's share of the CRM).

mod design;
mod generator;
mod send;
mod track;

use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    crm::{entity_type_by_id, items::{push_filter, Filter}, EntityType},
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub use generator::{run_job as run_generator, GENERATOR_JOB};
pub use send::{run_job, DUE_JOB, JOB};

/// Scheduled campaigns and sales generators that are due.
pub async fn run_due(state: &AppState) -> anyhow::Result<()> {
    send::run_due(state).await?;
    generator::run_due(state).await
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/marketing/segments", get(list_segments).post(create_segment))
        .route("/marketing/segments/preview", post(preview_segment))
        .route("/marketing/segments/import", post(import_segment))
        .route("/marketing/segments/{id}", get(get_segment).patch(update_segment).delete(delete_segment))
        .route("/marketing/campaigns", get(list_campaigns).post(create_campaign))
        .route("/marketing/campaigns/{id}", get(get_campaign).patch(update_campaign).delete(delete_campaign))
        .route("/marketing/campaigns/{id}/test", post(send::test_send))
        .route("/marketing/campaigns/{id}/send", post(send::schedule))
        .route("/marketing/campaigns/{id}/cancel", post(send::cancel))
        .route("/marketing/campaigns/{id}/stats", get(stats))
        .route("/marketing/campaigns/{id}/recipients", get(recipients))
        .route("/marketing/optouts", get(list_optouts).post(add_optout))
        .route("/marketing/optouts/{email}", delete(remove_optout))
        .route("/marketing/design/preview", post(preview_design))
        .route("/public/m/i/{file}/{sig}", get(design::image))
        .merge(generator::router())
        .merge(track::router())
}

fn require(auth: &AuthUser) -> AppResult<()> {
    auth.require(perm::MARKETING)
}

/// Types a segment can be made of: those with an email address.
const SEGMENT_TYPES: &[i16] = &[crate::crm::LEAD, crate::crm::CONTACT, crate::crm::COMPANY];

// ---------------------------------------------------------------------------
// Segments
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
struct Segment {
    id: Uuid,
    name: String,
    entity_type: i16,
    filter: Value,
    /// Imported segments: exactly these records; the filter is not used.
    item_ids: Option<Vec<Uuid>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const SEGMENT_COLUMNS: &str = "id, name, entity_type, filter, item_ids, created_at, updated_at";

async fn load_segment(state: &AppState, id: Uuid) -> AppResult<Segment> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {SEGMENT_COLUMNS} FROM mkt_segments WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("segment"))
}

fn parse_filter(v: &Value) -> AppResult<Filter> {
    let f: Filter = serde_json::from_value(v.clone()).map_err(|e| AppError::bad(format!("invalid filter: {e}")))?;
    // "Mine" means nothing for a company-wide audience.
    if f.assigned_to.as_deref().is_some_and(|a| a.split(',').any(|x| x.trim() == "me")) {
        return Err(AppError::bad("choose people by name, not \"me\", for a segment"));
    }
    Ok(f)
}

async fn segment_type(state: &AppState, et: i16) -> AppResult<EntityType> {
    if !SEGMENT_TYPES.contains(&et) {
        return Err(AppError::bad("segments are made of leads, contacts or companies"));
    }
    entity_type_by_id(state, et).await
}

/// Records of a segment with their first email address (none: no address). Visibility is
/// company-wide; the filter's own conditions apply.
pub(crate) async fn members(state: &AppState, t: &EntityType, filter: &Filter, ids: Option<&[Uuid]>, limit: i64) -> AppResult<Vec<(Uuid, String, Option<String>)>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT i.id, i.title, (SELECT m.value FROM crm_multifields m WHERE m.item_id = i.id AND m.kind = 'email' ORDER BY m.sort LIMIT 1) AS email FROM crm_items i WHERE TRUE",
    );
    restrict(&mut qb, t, filter, ids)?;
    qb.push(" ORDER BY i.created_at LIMIT ").push_bind(limit);
    Ok(qb.build_query_as().fetch_all(&state.db).await?)
}

/// A filter segment's conditions, or an imported segment's records.
fn restrict(qb: &mut QueryBuilder<Postgres>, t: &EntityType, filter: &Filter, ids: Option<&[Uuid]>) -> AppResult<()> {
    match ids {
        Some(ids) => {
            qb.push(" AND i.entity_type = ").push_bind(t.id).push(" AND i.id = ANY(").push_bind(ids.to_vec()).push(")");
            Ok(())
        }
        None => push_filter(qb, t, filter, Uuid::nil()),
    }
}

async fn count_members(state: &AppState, t: &EntityType, filter: &Filter, ids: Option<&[Uuid]>) -> AppResult<(i64, i64, i64)> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT COUNT(*)::int8,
                COUNT(*) FILTER (WHERE e.email IS NOT NULL)::int8,
                COUNT(*) FILTER (WHERE e.email IS NOT NULL AND EXISTS (SELECT 1 FROM mkt_optouts o WHERE o.email = lower(e.email)))::int8
         FROM crm_items i
         LEFT JOIN LATERAL (SELECT m.value AS email FROM crm_multifields m WHERE m.item_id = i.id AND m.kind = 'email' ORDER BY m.sort LIMIT 1) e ON TRUE
         WHERE TRUE",
    );
    restrict(&mut qb, t, filter, ids)?;
    Ok(qb.build_query_as().fetch_one(&state.db).await?)
}

async fn describe_segment(state: &AppState, s: &Segment) -> AppResult<Value> {
    let t = entity_type_by_id(state, s.entity_type).await?;
    let (total, with_email, opted_out) = count_members(state, &t, &parse_filter(&s.filter)?, s.item_ids.as_deref()).await?;
    Ok(json!({
        "id": s.id, "name": s.name, "entity_type": s.entity_type, "type_code": t.code, "filter": s.filter, "imported": s.item_ids.is_some(),
        "created_at": s.created_at, "updated_at": s.updated_at,
        "count": total, "with_email": with_email, "opted_out": opted_out, "reachable": with_email - opted_out,
    }))
}

async fn list_segments(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require(&auth)?;
    let rows: Vec<Segment> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {SEGMENT_COLUMNS} FROM mkt_segments ORDER BY lower(name)"))).fetch_all(&state.db).await?;
    let mut items = Vec::new();
    for s in &rows {
        items.push(describe_segment(&state, s).await?);
    }
    Ok(Json(json!({ "items": items })))
}

async fn get_segment(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    Ok(Json(describe_segment(&state, &load_segment(&state, id).await?).await?))
}

#[derive(Deserialize)]
struct SegmentBody {
    name: Option<String>,
    entity_type: Option<i16>,
    filter: Option<Value>,
}

/// Counts and the first records of a segment that is not saved yet.
async fn preview_segment(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SegmentBody>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let t = segment_type(&state, b.entity_type.unwrap_or(crate::crm::CONTACT)).await?;
    let f = parse_filter(&b.filter.unwrap_or_else(|| json!({})))?;
    let (total, with_email, opted_out) = count_members(&state, &t, &f, None).await?;
    let sample: Vec<Value> = members(&state, &t, &f, None, 20).await?.into_iter().map(|(id, title, email)| json!({ "id": id, "title": title, "email": email })).collect();
    Ok(Json(json!({ "count": total, "with_email": with_email, "opted_out": opted_out, "reachable": with_email - opted_out, "sample": sample, "type_code": t.code })))
}

async fn create_segment(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SegmentBody>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let et = b.entity_type.unwrap_or(crate::crm::CONTACT);
    segment_type(&state, et).await?;
    let filter = b.filter.unwrap_or_else(|| json!({}));
    parse_filter(&filter)?;
    let id: Uuid = sqlx::query_scalar!("INSERT INTO mkt_segments (name, entity_type, filter, created_by) VALUES ($1, $2, $3, $4) RETURNING id", required("name", b.name.as_deref().unwrap_or(""), 120)?, et, &filter, auth.id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(describe_segment(&state, &load_segment(&state, id).await?).await?))
}

#[derive(Deserialize)]
struct ImportRow {
    email: String,
    first_name: Option<String>,
    last_name: Option<String>,
    company_name: Option<String>,
}

#[derive(Deserialize)]
struct ImportBody {
    name: String,
    /// Contacts (default) or leads.
    entity_type: Option<i16>,
    rows: Vec<ImportRow>,
    /// Add addresses not yet in the CRM as new records; otherwise they are left out.
    #[serde(default)]
    create_missing: bool,
}

/// An imported segment: a list of addresses matched to CRM records of the type (the
/// earliest one with that address), and new records for the rest when asked.
async fn import_segment(State(state): State<AppState>, auth: AuthUser, Json(b): Json<ImportBody>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let et = b.entity_type.unwrap_or(crate::crm::CONTACT);
    if et != crate::crm::CONTACT && et != crate::crm::LEAD {
        return Err(AppError::bad("import contacts or leads"));
    }
    let t = segment_type(&state, et).await?;
    let name = required("name", &b.name, 120)?;
    if b.rows.is_empty() {
        return Err(AppError::bad("there are no addresses to import"));
    }
    if b.rows.len() > 20_000 {
        return Err(AppError::bad("import at most 20000 addresses at a time"));
    }
    let clip = |v: &Option<String>| v.as_deref().map(str::trim).filter(|x| !x.is_empty()).map(|x| x.chars().take(200).collect::<String>());
    let (mut ids, mut created, mut missing, mut invalid) = (Vec::<Uuid>::new(), 0, 0, 0);
    let mut seen = std::collections::HashSet::new();
    for r in &b.rows {
        let email = r.email.trim().to_lowercase();
        if email.parse::<lettre::Address>().is_err() {
            invalid += 1;
            continue;
        }
        if !seen.insert(email.clone()) {
            continue;
        }
        let found: Option<Uuid> = sqlx::query_scalar!("SELECT i.id FROM crm_items i JOIN crm_multifields m ON m.item_id = i.id
             WHERE i.entity_type = $1 AND m.kind = 'email' AND lower(m.value) = $2 ORDER BY i.created_at LIMIT 1", et, &email)
        .fetch_optional(&state.db)
        .await?;
        let id = match found {
            Some(id) => id,
            None if b.create_missing => {
                let (first, last, company) = (clip(&r.first_name), clip(&r.last_name), clip(&r.company_name));
                let mut body = serde_json::Map::new();
                if let Some(v) = &first { body.insert("first_name".into(), json!(v)); }
                if let Some(v) = &last { body.insert("last_name".into(), json!(v)); }
                if et == crate::crm::LEAD {
                    if let Some(v) = &company { body.insert("company_name".into(), json!(v)); }
                    let who = [first.clone(), last.clone()].into_iter().flatten().collect::<Vec<_>>().join(" ");
                    body.insert("title".into(), json!(if who.is_empty() { email.clone() } else { who }));
                } else if first.is_none() && last.is_none() {
                    body.insert("first_name".into(), json!(email.split('@').next().unwrap_or("")));
                }
                body.insert("multifields".into(), json!([{ "kind": "email", "value_type": "work", "value": email }]));
                created += 1;
                crate::crm::items::create_item(&state, &auth, &t, body, None).await?
            }
            None => {
                missing += 1;
                continue;
            }
        };
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let id: Uuid = sqlx::query_scalar!("INSERT INTO mkt_segments (name, entity_type, filter, item_ids, created_by) VALUES ($1, $2, '{}', $3, $4) RETURNING id", name, et, &ids, auth.id)
        .fetch_one(&state.db)
        .await?;
    let mut out = describe_segment(&state, &load_segment(&state, id).await?).await?;
    out["import"] = json!({ "matched": ids.len() - created, "created": created, "not_found": missing, "invalid": invalid });
    Ok(Json(out))
}

async fn update_segment(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<SegmentBody>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let cur = load_segment(&state, id).await?;
    let et = b.entity_type.unwrap_or(cur.entity_type);
    segment_type(&state, et).await?;
    if cur.item_ids.is_some() && (et != cur.entity_type || b.filter.is_some()) {
        return Err(AppError::bad("an imported segment keeps its records; only its name can change"));
    }
    let filter = b.filter.unwrap_or(cur.filter);
    parse_filter(&filter)?;
    sqlx::query!("UPDATE mkt_segments SET name = $2, entity_type = $3, filter = $4, updated_at = now() WHERE id = $1", id, match &b.name { Some(n) => required("name", n, 120)?, None => cur.name }, et, &filter)
        .execute(&state.db)
        .await?;
    Ok(Json(describe_segment(&state, &load_segment(&state, id).await?).await?))
}

async fn delete_segment(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let used: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM mkt_campaigns WHERE $1 = ANY(segment_ids) AND status IN ('draft', 'scheduled', 'sending')) AS "v!""#, id)
        .fetch_one(&state.db)
        .await?;
    if used {
        return Err(AppError::bad("a campaign that has not been sent yet uses this segment"));
    }
    let n = sqlx::query!("DELETE FROM mkt_segments WHERE id = $1", id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("segment"));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Campaigns
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow, Clone)]
pub(crate) struct Campaign {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub status: String,
    pub segment_ids: Vec<Uuid>,
    pub subject: String,
    pub body: String,
    pub from_name: Option<String>,
    pub reply_to: Option<String>,
    pub send_at: Option<DateTime<Utc>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub skipped: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Designed in blocks (then `body` is its plain text).
    pub design: Option<Value>,
    /// email | sms
    pub channel: String,
}

pub(crate) const CAMPAIGN_COLUMNS: &str =
    "id, name, slug, status, segment_ids, subject, body, from_name, reply_to, send_at, started_at, finished_at, skipped, created_at, updated_at, design, channel";

pub(crate) async fn load_campaign(state: &AppState, id: Uuid) -> AppResult<Campaign> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {CAMPAIGN_COLUMNS} FROM mkt_campaigns WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("campaign"))
}

/// Recipient counts by state, for lists and results.
async fn counts(state: &AppState, id: Uuid) -> AppResult<Value> {
    let (total, sent, failed, pending, opened, clicked, unsubscribed): (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*)::int8,
                COUNT(*) FILTER (WHERE status = 'sent')::int8,
                COUNT(*) FILTER (WHERE status = 'failed')::int8,
                COUNT(*) FILTER (WHERE status = 'pending')::int8,
                COUNT(*) FILTER (WHERE opened_at IS NOT NULL)::int8,
                COUNT(*) FILTER (WHERE clicked_at IS NOT NULL)::int8,
                COUNT(*) FILTER (WHERE unsubscribed_at IS NOT NULL)::int8
         FROM mkt_recipients WHERE campaign_id = $1",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(json!({ "recipients": total, "sent": sent, "failed": failed, "pending": pending, "opened": opened, "clicked": clicked, "unsubscribed": unsubscribed }))
}

async fn describe(state: &AppState, c: &Campaign) -> AppResult<Value> {
    let mut v = serde_json::to_value(c).unwrap_or_default();
    v["counts"] = counts(state, c.id).await?;
    Ok(v)
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

async fn list_campaigns(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let rows: Vec<Campaign> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {CAMPAIGN_COLUMNS} FROM mkt_campaigns WHERE ($1::text IS NULL OR status = $1) ORDER BY created_at DESC LIMIT 500"
    )))
    .bind(q.status.filter(|s| !s.is_empty()))
    .fetch_all(&state.db)
    .await?;
    let mut items = Vec::new();
    for c in &rows {
        items.push(describe(&state, c).await?);
    }
    Ok(Json(json!({ "items": items })))
}

async fn get_campaign(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    Ok(Json(describe(&state, &load_campaign(&state, id).await?).await?))
}

#[derive(Deserialize)]
struct CampaignBody {
    name: Option<String>,
    /// email (default) | sms
    channel: Option<String>,
    segment_ids: Option<Vec<Uuid>>,
    subject: Option<String>,
    body: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    from_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    reply_to: Option<Option<String>>,
    /// The message in blocks; null goes back to plain text.
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    design: Option<Option<Value>>,
}

/// A design checked, and its plain text (placeholders kept) for the body.
async fn checked_design(state: &AppState, auth: &AuthUser, d: &Value) -> AppResult<(Value, String)> {
    let me = auth.id;
    let db = state.db.clone();
    let clean = design::clean(d, async move |f: Uuid| {
        sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM files WHERE id = $1 AND owner_id = $2) AS "v!""#, f, me).fetch_one(&db).await.unwrap_or(false)
    })
    .await?;
    let (_, text) = design::render(state, &clean, &|t| t.to_string(), &|u| u.to_string(), ("", ""));
    Ok((clean, text.trim_end().trim_end_matches("--").trim_end().to_string()))
}

#[derive(Deserialize)]
struct PreviewDesign {
    design: Value,
}

/// The designed email as it will look (for a sample client; links not tracked).
async fn preview_design(State(state): State<AppState>, auth: AuthUser, Json(b): Json<PreviewDesign>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let (d, _) = checked_design(&state, &auth, &b.design).await?;
    let sample = json!({ "first_name": "Alex", "last_name": "Sample", "title": "Alex Sample", "name": "Alex Sample", "assigned": "Your name", "assigned_email": "you@example.com", "company": "Sample Ltd" });
    let company = send::company_name(&state).await;
    let footer = format!("You get this email from {}. <a href=\"#\" style=\"color:#64748b\">Unsubscribe</a>", send::esc(&company));
    let (html, text) = design::render(&state, &d, &|t| crate::crm::automation::fill(t, &sample), &|u| u.to_string(), (&footer, "Unsubscribe: …"));
    Ok(Json(json!({ "html": html, "text": text, "design": d })))
}

fn slugify(name: &str) -> String {
    crate::routes::slugify(name, "campaign")
}

async fn unique_slug(state: &AppState, name: &str) -> AppResult<String> {
    let base = slugify(name);
    for i in 1..1000 {
        let s = if i == 1 { base.clone() } else { format!("{base}-{i}") };
        let taken: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM mkt_campaigns WHERE slug = $1) AS "v!""#, &s).fetch_one(&state.db).await?;
        if !taken {
            return Ok(s);
        }
    }
    Ok(format!("{base}-{}", Uuid::new_v4().simple()))
}

async fn check_segments(state: &AppState, ids: &[Uuid]) -> AppResult<Vec<Uuid>> {
    let mut v = ids.to_vec();
    v.sort();
    v.dedup();
    if v.len() > 20 {
        return Err(AppError::bad("at most 20 segments"));
    }
    let n: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM mkt_segments WHERE id = ANY($1)"#, &v).fetch_one(&state.db).await?;
    if n != v.len() as i64 {
        return Err(AppError::bad("unknown segment"));
    }
    Ok(v)
}

/// A campaign's channel: email, or sms when text messages are set up.
async fn check_channel(state: &AppState, c: Option<&str>) -> AppResult<Option<String>> {
    match c {
        None => Ok(None),
        Some("email") => Ok(Some("email".into())),
        Some("sms") if crate::sms::enabled(state).await => Ok(Some("sms".into())),
        Some("sms") => Err(AppError::bad("text messages are not set up (System → Text messages)")),
        Some(_) => Err(AppError::bad("a campaign goes out by email or sms")),
    }
}

fn check_reply_to(r: Option<String>) -> AppResult<Option<String>> {
    let r = r.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(a) = &r
        && (a.len() > 200 || a.parse::<lettre::Address>().is_err()) {
            return Err(AppError::bad("the reply-to address is not valid"));
        }
    Ok(r)
}

fn text(v: Option<&str>, label: &str, max: usize) -> AppResult<String> {
    let s = v.unwrap_or("").to_string();
    if s.chars().count() > max {
        return Err(AppError::bad(format!("{label} is too long")));
    }
    Ok(s)
}

async fn create_campaign(State(state): State<AppState>, auth: AuthUser, Json(b): Json<CampaignBody>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 120)?;
    let design = match b.design.clone().flatten() {
        Some(d) => Some(checked_design(&state, &auth, &d).await?),
        None => None,
    };
    let body = match &design {
        Some((_, text)) => Some(text.clone()),
        None => b.body.clone(),
    };
    let channel = check_channel(&state, b.channel.as_deref()).await?.unwrap_or_else(|| "email".into());
    let id: Uuid = sqlx::query_scalar!("INSERT INTO mkt_campaigns (name, slug, segment_ids, subject, body, from_name, reply_to, created_by, design, channel) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id", &name, unique_slug(&state, &name).await?, &(check_segments(&state, &b.segment_ids.unwrap_or_default()).await?), text(b.subject.as_deref(), "subject", 300)?, text(body.as_deref(), "message", 100_000)?, b.from_name.flatten().map(|s| s.trim().chars().take(100).collect::<String>()).filter(|s| !s.is_empty()), check_reply_to(b.reply_to.flatten())?, auth.id, design.map(|d| d.0), channel)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(describe(&state, &load_campaign(&state, id).await?).await?))
}

async fn update_campaign(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<CampaignBody>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let cur = load_campaign(&state, id).await?;
    if cur.status != "draft" {
        return Err(AppError::bad("only drafts can be changed; cancel a scheduled campaign first"));
    }
    let (design, designed_body): (Option<Value>, Option<String>) = match &b.design {
        None => (cur.design.clone(), None),
        Some(None) => (None, None),
        Some(Some(d)) => {
            let (d, t) = checked_design(&state, &auth, d).await?;
            (Some(d), Some(t))
        }
    };
    let body = designed_body.or_else(|| b.body.clone());
    if let Some(ch) = check_channel(&state, b.channel.as_deref()).await? {
        sqlx::query!("UPDATE mkt_campaigns SET channel = $2 WHERE id = $1", id, ch).execute(&state.db).await?;
    }
    sqlx::query!("UPDATE mkt_campaigns SET name = $2, segment_ids = $3, subject = $4, body = $5, from_name = $6, reply_to = $7, design = $8, updated_at = now() WHERE id = $1", id, match &b.name { Some(n) => required("name", n, 120)?, None => cur.name }, &(match &b.segment_ids { Some(s) => check_segments(&state, s).await?, None => cur.segment_ids }), match &b.subject { Some(s) => text(Some(s), "subject", 300)?, None => cur.subject }, match &body { Some(s) => text(Some(s), "message", 100_000)?, None => cur.body }, match b.from_name { Some(f) => f.map(|s| s.trim().chars().take(100).collect::<String>()).filter(|s| !s.is_empty()), None => cur.from_name }, match b.reply_to { Some(r) => check_reply_to(r)?, None => cur.reply_to }, design)
        .execute(&state.db)
        .await?;
    Ok(Json(describe(&state, &load_campaign(&state, id).await?).await?))
}

async fn delete_campaign(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let c = load_campaign(&state, id).await?;
    if c.status == "sending" {
        return Err(AppError::bad("the campaign is being sent; cancel it first"));
    }
    sqlx::query!("DELETE FROM mkt_campaigns WHERE id = $1", id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Results: counts and rates, opens and clicks over time, clicks per link, and what the
/// campaign brought in (leads and deals carrying its utm_campaign).
async fn stats(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let c = load_campaign(&state, id).await?;
    let counts = counts(&state, id).await?;
    let links: Vec<(String, i64, i64)> = sqlx::query!(r#"SELECT k.url AS "url!", COUNT(*)::int8 AS "c1!", COUNT(DISTINCT k.recipient_id)::int8 AS "c2!" FROM mkt_clicks k JOIN mkt_recipients r ON r.id = k.recipient_id
         WHERE r.campaign_id = $1 GROUP BY k.url ORDER BY 2 DESC LIMIT 50"#, id).map(|r| (r.url, r.c1, r.c2))
    .fetch_all(&state.db)
    .await?;
    // First opens and clicks per hour since sending started (up to a week).
    let activity: Vec<(DateTime<Utc>, i64, i64)> = sqlx::query!(r#"SELECT h AS "h!", SUM(o)::int8 AS "c1!", SUM(c)::int8 AS "c2!" FROM (
             SELECT date_trunc('hour', opened_at) AS h, 1 AS o, 0 AS c FROM mkt_recipients WHERE campaign_id = $1 AND opened_at IS NOT NULL
             UNION ALL
             SELECT date_trunc('hour', clicked_at), 0, 1 FROM mkt_recipients WHERE campaign_id = $1 AND clicked_at IS NOT NULL
         ) x GROUP BY h ORDER BY h LIMIT 168"#, id).map(|r| (r.h, r.c1, r.c2))
    .fetch_all(&state.db)
    .await?;
    let brought: Vec<(String, i64, Option<rust_decimal::Decimal>)> = sqlx::query!(r#"SELECT t.code AS "code!", COUNT(*)::int8 AS "c1!", SUM(i.opportunity_base) FILTER (WHERE i.stage_semantics = 'success') AS "c2?"
         FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type
         WHERE i.deleted_at IS NULL AND i.utm->>'campaign' = $1 GROUP BY t.code"#, &c.slug).map(|r| (r.code, r.c1, r.c2))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "counts": counts,
        "skipped": c.skipped,
        "links": links.into_iter().map(|(url, clicks, people)| json!({ "url": url, "clicks": clicks, "people": people })).collect::<Vec<_>>(),
        "activity": activity.into_iter().map(|(h, o, k)| json!({ "hour": h, "opens": o, "clicks": k })).collect::<Vec<_>>(),
        "brought": brought.into_iter().map(|(code, n, won)| json!({ "type": code, "count": n, "won_amount": won })).collect::<Vec<_>>(),
        "utm_campaign": c.slug,
    })))
}

#[derive(Deserialize)]
struct RecipientsQuery {
    /// sent | failed | pending | opened | clicked | unsubscribed
    status: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn recipients(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<RecipientsQuery>) -> AppResult<Json<Value>> {
    require(&auth)?;
    load_campaign(&state, id).await?;
    let cond = match q.status.as_deref().unwrap_or("") {
        "sent" | "failed" | "pending" => "r.status = $2",
        "opened" => "r.opened_at IS NOT NULL",
        "clicked" => "r.clicked_at IS NOT NULL",
        "unsubscribed" => "r.unsubscribed_at IS NOT NULL",
        "" => "TRUE",
        _ => return Err(AppError::bad("unknown status")),
    };
    let like = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| format!("%{}%", s.replace('%', "\\%").replace('_', "\\_")));
    let rows: Vec<(Uuid, Option<Uuid>, Option<String>, Option<String>, String, String, Option<String>, Option<DateTime<Utc>>, Option<DateTime<Utc>>, i32, Option<DateTime<Utc>>, i32, Option<DateTime<Utc>>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT r.id, r.item_id, i.title, t.code, r.email, r.status, r.error, r.sent_at, r.opened_at, r.open_count, r.clicked_at, r.click_count, r.unsubscribed_at
             FROM mkt_recipients r LEFT JOIN crm_items i ON i.id = r.item_id LEFT JOIN crm_entity_types t ON t.id = i.entity_type
             WHERE r.campaign_id = $1 AND {cond} AND ($3::text IS NULL OR r.email ILIKE $3 OR i.title ILIKE $3)
             ORDER BY r.email LIMIT $4 OFFSET $5"
        )))
        .bind(id)
        .bind(q.status.as_deref().unwrap_or(""))
        .bind(like)
        .bind(q.limit.unwrap_or(50).clamp(1, 200))
        .bind(q.offset.unwrap_or(0).max(0))
        .fetch_all(&state.db)
        .await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|(rid, item, title, code, email, status, error, sent, opened, opens, clicked, clicks, unsub)| {
            json!({ "id": rid, "item_id": item, "title": title, "type_code": code, "email": email, "status": status, "error": error, "sent_at": sent,
                    "opened_at": opened, "open_count": opens, "clicked_at": clicked, "click_count": clicks, "unsubscribed_at": unsub })
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

// ---------------------------------------------------------------------------
// Opt-outs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct OptoutQuery {
    q: Option<String>,
}

async fn list_optouts(State(state): State<AppState>, auth: AuthUser, Query(q): Query<OptoutQuery>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let like = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(|s| format!("%{}%", s.to_lowercase()));
    let rows: Vec<(String, String, Option<String>, DateTime<Utc>)> = sqlx::query!(r#"SELECT o.email AS "email!", o.source AS "source!", c.name AS "name?", o.created_at AS "created_at!" FROM mkt_optouts o LEFT JOIN mkt_campaigns c ON c.id = o.campaign_id
         WHERE ($1::text IS NULL OR o.email LIKE $1) ORDER BY o.created_at DESC LIMIT 500"#, like).map(|r| (r.email, r.source, r.name, r.created_at))
    .fetch_all(&state.db)
    .await?;
    let total: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM mkt_optouts"#).fetch_one(&state.db).await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(email, source, campaign, at)| json!({ "email": email, "source": source, "campaign": campaign, "created_at": at })).collect::<Vec<_>>(), "total": total })))
}

#[derive(Deserialize)]
struct OptoutBody {
    email: String,
}

async fn add_optout(State(state): State<AppState>, auth: AuthUser, Json(b): Json<OptoutBody>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let email = b.email.trim().to_lowercase();
    if email.parse::<lettre::Address>().is_err() {
        return Err(AppError::bad("enter a valid email address"));
    }
    sqlx::query!("INSERT INTO mkt_optouts (email, source, created_by) VALUES ($1, 'added', $2) ON CONFLICT DO NOTHING", &email, auth.id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Takes an address off the list (it was added by mistake, or the person asked again).
async fn remove_optout(State(state): State<AppState>, auth: AuthUser, Path(email): Path<String>) -> AppResult<Json<Value>> {
    require(&auth)?;
    let n = sqlx::query!("DELETE FROM mkt_optouts WHERE email = $1", email.trim().to_lowercase()).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("address"));
    }
    crate::audit::record(&state.db, Some(auth.id), "marketing.optout.remove", "optout", Some(email), json!({}), None).await;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn slugs() {
        assert_eq!(super::slugify("Spring Sale 2027!"), "spring-sale-2027");
        assert_eq!(super::slugify("  ---  "), "campaign");
    }
}
