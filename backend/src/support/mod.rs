//! Help desk: requests (tickets) from employees and customers, answered by the support team.
//!
//! - **Tickets** come from employees in the app, from customers on the public page
//!   (`/help`, each gets a private link to follow the ticket), and by email to the help-desk
//!   inbox (a CRM mailbox; replies go out through it, with `[#N]` in the subject).
//! - Each ticket has a category, criticality, status, difficulty and a responsible person;
//!   staff write replies the customer sees and notes only support sees, and log the time
//!   spent. Every change is kept in the ticket's history.
//! - **SLAs** set how soon support must reply, in working hours (per weekday, in the help
//!   desk's time zone, skipping company holidays). A warning goes out before the deadline,
//!   overdue tickets are flagged and their people told. Tickets waiting too long for the
//!   customer close by themselves; customers rate the answer when it is closed.
//! - **Frequently used answers** can be put into a reply; **statistics** show volumes,
//!   response and resolution times, overdue replies and ratings.
//!
//! Everyone can ask for help and sees their own requests. "Work on the help desk" sees and
//! answers every ticket; "Manage the help desk" also sets it up.

pub mod mail;
mod public;
pub mod sla;
pub mod tickets;

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

pub use sla::JOB as SLA_JOB;
pub use mail::SEND_JOB as MAIL_SEND_JOB;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/support/settings", get(get_settings).put(put_settings))
        .route("/support/dictionary", get(list_dictionary).post(create_entry))
        .route("/support/dictionary/{id}", patch(update_entry).delete(delete_entry))
        .route("/support/slas", get(list_slas).post(create_sla))
        .route("/support/slas/{id}", patch(update_sla).delete(delete_sla))
        .route("/support/answers", get(list_answers).post(create_answer))
        .route("/support/answers/{id}", patch(update_answer).delete(delete_answer))
        .route("/support/team", get(team))
        .route("/support/stats", get(stats))
        .merge(tickets::router())
        .merge(public::router())
}

/// The stored form of a customer's link token.
pub(crate) fn hash(token: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(token.as_bytes()))
}

/// Works on tickets: sees and answers all of them.
pub(crate) fn worker(auth: &AuthUser) -> bool {
    auth.can(perm::SUPPORT_WORK) || auth.can(perm::SUPPORT_MANAGE)
}

fn require_worker(auth: &AuthUser) -> AppResult<()> {
    if worker(auth) { Ok(()) } else { Err(AppError::Forbidden) }
}

fn require_manager(auth: &AuthUser) -> AppResult<()> {
    auth.require(perm::SUPPORT_MANAGE)
}

/// Whether another person may be given tickets (they work on the help desk).
pub(crate) async fn is_worker(state: &AppState, user: Uuid) -> AppResult<bool> {
    let active: bool = sqlx::query_scalar!("SELECT is_active FROM users WHERE id = $1", user).fetch_optional(&state.db).await?.unwrap_or(false);
    if !active {
        return Ok(false);
    }
    let p = crate::auth::load_permissions(state, user).await?;
    Ok(p.iter().any(|p| p == perm::ALL || p == perm::SUPPORT_WORK || p == perm::SUPPORT_MANAGE))
}

async fn check_worker(state: &AppState, user: Option<Uuid>, what: &str) -> AppResult<()> {
    if let Some(u) = user
        && !is_worker(state, u).await? {
            return Err(AppError::bad(format!("{what} must be someone who works on the help desk")));
        }
    Ok(())
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

async fn settings_json(state: &AppState) -> AppResult<Value> {
    let (mailbox, portal, intro, days, reopen, tz): (Option<Uuid>, bool, Option<String>, i32, bool, String) =
        sqlx::query!(r#"SELECT mailbox_id, portal_enabled, portal_intro, auto_close_days, allow_reopen, timezone FROM support_settings WHERE id"#).map(|r| (r.mailbox_id, r.portal_enabled, r.portal_intro, r.auto_close_days, r.allow_reopen, r.timezone))
            .fetch_one(&state.db)
            .await?;
    let mailbox_email: Option<String> = match mailbox {
        Some(m) => sqlx::query_scalar!("SELECT email FROM crm_mailboxes WHERE id = $1", m).fetch_optional(&state.db).await?,
        None => None,
    };
    Ok(json!({ "mailbox_id": mailbox, "mailbox_email": mailbox_email, "portal_enabled": portal, "portal_intro": intro,
               "portal_url": format!("{}/help", state.config.public_url), "auto_close_days": days, "allow_reopen": reopen, "timezone": tz }))
}

async fn get_settings(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require_worker(&auth)?;
    Ok(Json(settings_json(&state).await?))
}

#[derive(Deserialize)]
struct SettingsInput {
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    mailbox_id: Option<Option<Uuid>>,
    portal_enabled: Option<bool>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    portal_intro: Option<Option<String>>,
    auto_close_days: Option<i32>,
    allow_reopen: Option<bool>,
    timezone: Option<String>,
}

async fn put_settings(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SettingsInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    if let Some(Some(m)) = b.mailbox_id {
        let mb = crate::crm::email::load_mailbox(&state, m).await?;
        if !mb.may_send(&state, auth.id).await? {
            return Err(AppError::bad("choose a mailbox you own or that is shared with you"));
        }
    }
    if let Some(d) = b.auto_close_days
        && !(0..=365).contains(&d) {
            return Err(AppError::bad("close after 0 (never) to 365 days"));
        }
    if let Some(tz) = &b.timezone
        && tz.parse::<chrono_tz::Tz>().is_err() {
            return Err(AppError::bad("unknown time zone"));
        }
    let intro = b.portal_intro.map(clean);
    if intro.as_ref().and_then(|i| i.as_ref()).is_some_and(|i| i.chars().count() > 5000) {
        return Err(AppError::bad("the introduction is too long"));
    }
    sqlx::query!("UPDATE support_settings SET
             mailbox_id = CASE WHEN $1 THEN $2 ELSE mailbox_id END,
             portal_enabled = COALESCE($3, portal_enabled),
             portal_intro = CASE WHEN $4 THEN $5 ELSE portal_intro END,
             auto_close_days = COALESCE($6, auto_close_days),
             allow_reopen = COALESCE($7, allow_reopen),
             timezone = COALESCE($8, timezone),
             updated_at = now()
         WHERE id", b.mailbox_id.is_some(), b.mailbox_id.flatten(), b.portal_enabled, intro.is_some(), intro.flatten(), b.auto_close_days, b.allow_reopen, b.timezone)
    .execute(&state.db)
    .await?;
    Ok(Json(settings_json(&state).await?))
}

/// People who work on the help desk (to assign tickets to).
async fn team(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require_worker(&auth)?;
    let users: Vec<(Uuid, String, Option<Uuid>)> =
        sqlx::query!(r#"SELECT id AS "id!", first_name || ' ' || last_name AS "c1!", avatar_file_id AS "avatar_file_id?" FROM users WHERE is_active AND NOT is_system ORDER BY first_name, last_name"#).map(|r| (r.id, r.c1, r.avatar_file_id))
            .fetch_all(&state.db)
            .await?;
    let mut items = Vec::new();
    for (id, name, avatar) in users {
        if is_worker(&state, id).await? {
            items.push(json!({ "id": id, "name": name, "avatar_file_id": avatar }));
        }
    }
    Ok(Json(json!({ "items": items })))
}

// ---------------------------------------------------------------------------
// Dictionary: categories, criticality, statuses, marks, difficulty
// ---------------------------------------------------------------------------

const KINDS: &[&str] = &["category", "criticality", "status", "mark", "difficulty"];

#[derive(sqlx::FromRow, serde::Serialize)]
struct Entry {
    id: Uuid,
    kind: String,
    name: String,
    description: Option<String>,
    sort: i32,
    is_default: bool,
    responsible_id: Option<Uuid>,
}

const ENTRY_SELECT: &str = "SELECT id, kind, name, description, sort, is_default, responsible_id FROM support_dictionary";

/// Everyone reads it: people asking for help choose a category and criticality.
async fn list_dictionary(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Value>> {
    let rows: Vec<Entry> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{ENTRY_SELECT} ORDER BY kind, sort, name"))).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows })))
}

/// Checks that `id` is an entry of `kind`.
pub(crate) async fn entry_of(state: &AppState, id: Option<Uuid>, kind: &str) -> AppResult<Option<Uuid>> {
    let Some(id) = id else { return Ok(None) };
    let ok: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM support_dictionary WHERE id = $1 AND kind = $2) AS "v!""#, id, kind)
        .fetch_one(&state.db)
        .await?;
    if ok { Ok(Some(id)) } else { Err(AppError::bad(format!("unknown {kind}"))) }
}

pub(crate) async fn default_of(state: &AppState, kind: &str) -> AppResult<Option<Uuid>> {
    Ok(sqlx::query_scalar!("SELECT id FROM support_dictionary WHERE kind = $1 AND is_default ORDER BY sort LIMIT 1", kind).fetch_optional(&state.db).await?)
}

pub(crate) async fn entry_name(state: &AppState, id: Option<Uuid>) -> AppResult<String> {
    Ok(match id {
        Some(id) => sqlx::query_scalar!("SELECT name FROM support_dictionary WHERE id = $1", id).fetch_optional(&state.db).await?.unwrap_or_default(),
        None => String::new(),
    })
}

#[derive(Deserialize)]
struct EntryInput {
    kind: Option<String>,
    name: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    sort: Option<i32>,
    is_default: Option<bool>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    responsible_id: Option<Option<Uuid>>,
}

async fn create_entry(State(state): State<AppState>, auth: AuthUser, Json(b): Json<EntryInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let kind = b.kind.unwrap_or_default();
    if !KINDS.contains(&kind.as_str()) {
        return Err(AppError::bad("kind is one of category, criticality, status, mark, difficulty"));
    }
    let name = required("name", &b.name.unwrap_or_default(), 200)?;
    let responsible = b.responsible_id.flatten();
    if responsible.is_some() && kind != "category" {
        return Err(AppError::bad("only categories have a responsible person"));
    }
    check_worker(&state, responsible, "the responsible person").await?;
    let sort = match b.sort {
        Some(s) => s,
        None => sqlx::query_scalar!(r#"SELECT max(sort) AS "v?" FROM support_dictionary WHERE kind = $1"#, &kind).fetch_one(&state.db).await?.unwrap_or(0) + 100,
    };
    let mut tx = state.db.begin().await?;
    if b.is_default == Some(true) {
        sqlx::query!("UPDATE support_dictionary SET is_default = FALSE WHERE kind = $1", &kind).execute(&mut *tx).await?;
    }
    let id: Uuid = sqlx::query_scalar!("INSERT INTO support_dictionary (kind, name, description, sort, is_default, responsible_id) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id", &kind, &name, clean(b.description.flatten()), sort, b.is_default.unwrap_or(false), responsible)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    entry_json(&state, id).await
}

async fn entry_json(state: &AppState, id: Uuid) -> AppResult<Json<Value>> {
    let e: Entry = sqlx::query_as(sqlx::AssertSqlSafe(format!("{ENTRY_SELECT} WHERE id = $1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("entry"))?;
    Ok(Json(serde_json::to_value(e).unwrap()))
}

async fn update_entry(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<EntryInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let kind: String = sqlx::query_scalar!("SELECT kind FROM support_dictionary WHERE id = $1", id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("entry"))?;
    let name = match b.name {
        Some(n) => Some(required("name", &n, 200)?),
        None => None,
    };
    if let Some(Some(r)) = b.responsible_id {
        if kind != "category" {
            return Err(AppError::bad("only categories have a responsible person"));
        }
        check_worker(&state, Some(r), "the responsible person").await?;
    }
    let mut tx = state.db.begin().await?;
    if b.is_default == Some(true) {
        sqlx::query!("UPDATE support_dictionary SET is_default = FALSE WHERE kind = $1 AND id <> $2", &kind, id).execute(&mut *tx).await?;
    }
    sqlx::query!("UPDATE support_dictionary SET name = COALESCE($2, name), description = CASE WHEN $3 THEN $4 ELSE description END, sort = COALESCE($5, sort),
             is_default = COALESCE($6, is_default), responsible_id = CASE WHEN $7 THEN $8 ELSE responsible_id END
         WHERE id = $1", id, name, b.description.is_some(), clean(b.description.flatten()), b.sort, b.is_default, b.responsible_id.is_some(), b.responsible_id.flatten())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    entry_json(&state, id).await
}

async fn delete_entry(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    // Tickets keep working without it (their field is cleared); SLAs stop matching it.
    sqlx::query!("UPDATE support_slas SET category_ids = array_remove(category_ids, $1), criticality_ids = array_remove(criticality_ids, $1)", id)
        .execute(&state.db)
        .await?;
    let n = sqlx::query!("DELETE FROM support_dictionary WHERE id = $1", id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("entry"));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// SLAs
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow, serde::Serialize)]
struct SlaRow {
    id: Uuid,
    name: String,
    description: Option<String>,
    priority: i32,
    is_default: bool,
    response_minutes: i32,
    notice_minutes: i32,
    responsible_id: Option<Uuid>,
    category_ids: Vec<Uuid>,
    criticality_ids: Vec<Uuid>,
    requesters: String,
    hours: Value,
    skip_holidays: bool,
}

const SLA_SELECT: &str = "SELECT id, name, description, priority, is_default, response_minutes, notice_minutes, responsible_id, category_ids, criticality_ids,
    requesters, hours, skip_holidays FROM support_slas";

async fn list_slas(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require_worker(&auth)?;
    let rows: Vec<SlaRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SLA_SELECT} ORDER BY is_default DESC, priority DESC, name"))).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows })))
}

#[derive(Deserialize)]
struct SlaInput {
    name: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    priority: Option<i32>,
    response_minutes: Option<i32>,
    notice_minutes: Option<i32>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    responsible_id: Option<Option<Uuid>>,
    category_ids: Option<Vec<Uuid>>,
    criticality_ids: Option<Vec<Uuid>>,
    requesters: Option<String>,
    hours: Option<Value>,
    skip_holidays: Option<bool>,
}

async fn check_entries(state: &AppState, ids: &[Uuid], kind: &str) -> AppResult<()> {
    for id in ids {
        entry_of(state, Some(*id), kind).await?;
    }
    Ok(())
}

async fn save_sla(state: &AppState, id: Option<Uuid>, b: SlaInput) -> AppResult<Uuid> {
    let cur: Option<SlaRow> = match id {
        Some(id) => Some(
            sqlx::query_as(sqlx::AssertSqlSafe(format!("{SLA_SELECT} WHERE id = $1")))
                .bind(id)
                .fetch_optional(&state.db)
                .await?
                .ok_or(AppError::NotFound("SLA"))?,
        ),
        None => None,
    };
    let c = cur.as_ref();
    let name = required("name", &b.name.or(c.map(|c| c.name.clone())).unwrap_or_default(), 200)?;
    let response = b.response_minutes.or(c.map(|c| c.response_minutes)).ok_or_else(|| AppError::bad("set the response time"))?;
    if !(1..=60 * 24 * 90).contains(&response) {
        return Err(AppError::bad("the response time is 1 minute to 90 days"));
    }
    let notice = b.notice_minutes.or(c.map(|c| c.notice_minutes)).unwrap_or(0);
    if notice < 0 || notice >= response {
        return Err(AppError::bad("the warning comes before the deadline: less than the response time"));
    }
    let requesters = b.requesters.or(c.map(|c| c.requesters.clone())).unwrap_or_else(|| "any".into());
    if !["any", "staff", "external"].contains(&requesters.as_str()) {
        return Err(AppError::bad("requesters is any, staff or external"));
    }
    let hours = match b.hours {
        Some(h) => sla::hours_json(&sla::parse_hours(&h).map_err(AppError::bad)?),
        None => c.map(|c| c.hours.clone()).unwrap_or_else(|| sla::hours_json(&(1..=7).map(|d| (d, vec![(0, 1440)])).collect())),
    };
    if sla::parse_hours(&hours).map(|h| h.is_empty()).unwrap_or(true) {
        return Err(AppError::bad("set at least one working span"));
    }
    let categories = b.category_ids.or(c.map(|c| c.category_ids.clone())).unwrap_or_default();
    let criticalities = b.criticality_ids.or(c.map(|c| c.criticality_ids.clone())).unwrap_or_default();
    check_entries(state, &categories, "category").await?;
    check_entries(state, &criticalities, "criticality").await?;
    let responsible = match b.responsible_id {
        Some(r) => r,
        None => c.and_then(|c| c.responsible_id),
    };
    check_worker(state, responsible, "the responsible person").await?;
    let description = match b.description {
        Some(d) => clean(d),
        None => c.and_then(|c| c.description.clone()),
    };
    let row = (
        name,
        description,
        b.priority.or(c.map(|c| c.priority)).unwrap_or(0),
        response,
        notice,
        responsible,
        categories,
        criticalities,
        requesters,
        hours,
        b.skip_holidays.or(c.map(|c| c.skip_holidays)).unwrap_or(true),
    );
    let id = match id {
        Some(id) => {
            sqlx::query!("UPDATE support_slas SET name = $2, description = $3, priority = $4, response_minutes = $5, notice_minutes = $6, responsible_id = $7,
                     category_ids = $8, criticality_ids = $9, requesters = $10, hours = $11, skip_holidays = $12 WHERE id = $1", id, row.0, row.1, row.2, row.3, row.4, row.5, &row.6, &row.7, row.8, row.9, row.10)
            .execute(&state.db)
            .await?;
            id
        }
        None => {
            sqlx::query_scalar!("INSERT INTO support_slas (name, description, priority, response_minutes, notice_minutes, responsible_id, category_ids, criticality_ids,
                     requesters, hours, skip_holidays) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id", row.0, row.1, row.2, row.3, row.4, row.5, &row.6, &row.7, row.8, row.9, row.10)
            .fetch_one(&state.db)
            .await?
        }
    };
    Ok(id)
}

async fn sla_json(state: &AppState, id: Uuid) -> AppResult<Json<Value>> {
    let r: SlaRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SLA_SELECT} WHERE id = $1"))).bind(id).fetch_one(&state.db).await?;
    Ok(Json(serde_json::to_value(r).unwrap()))
}

async fn create_sla(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SlaInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let id = save_sla(&state, None, b).await?;
    sla_json(&state, id).await
}

async fn update_sla(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<SlaInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    save_sla(&state, Some(id), b).await?;
    sla_json(&state, id).await
}

async fn delete_sla(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let default: bool = sqlx::query_scalar!("SELECT is_default FROM support_slas WHERE id = $1", id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("SLA"))?;
    if default {
        return Err(AppError::bad("the default SLA covers every other ticket and cannot be removed"));
    }
    // Its open tickets move to the default agreement (deadlines stay as they were).
    sqlx::query!("UPDATE support_tickets SET sla_id = (SELECT id FROM support_slas WHERE is_default LIMIT 1) WHERE sla_id = $1", id).execute(&state.db).await?;
    sqlx::query!("DELETE FROM support_slas WHERE id = $1", id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Frequently used answers
// ---------------------------------------------------------------------------

async fn list_answers(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    require_worker(&auth)?;
    let rows: Vec<(Uuid, String, String, i32)> = sqlx::query!(r#"SELECT id, title, body, sort FROM support_answers ORDER BY sort, title"#).map(|r| (r.id, r.title, r.body, r.sort)).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, title, body, sort)| json!({ "id": id, "title": title, "body": body, "sort": sort })).collect::<Vec<_>>() })))
}

#[derive(Deserialize)]
struct AnswerInput {
    title: Option<String>,
    body: Option<String>,
    sort: Option<i32>,
}

async fn create_answer(State(state): State<AppState>, auth: AuthUser, Json(b): Json<AnswerInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let title = required("title", &b.title.unwrap_or_default(), 200)?;
    let body = required("text", &b.body.unwrap_or_default(), 20_000)?;
    let id: Uuid = sqlx::query_scalar!("INSERT INTO support_answers (title, body, sort, created_by) VALUES ($1, $2, $3, $4) RETURNING id", &title, &body, b.sort.unwrap_or(100), auth.id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(json!({ "id": id, "title": title, "body": body, "sort": b.sort.unwrap_or(100) })))
}

async fn update_answer(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<AnswerInput>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    let title = b.title.map(|t| required("title", &t, 200)).transpose()?;
    let body = b.body.map(|t| required("text", &t, 20_000)).transpose()?;
    let row: Option<(String, String, i32)> = sqlx::query!(r#"UPDATE support_answers SET title = COALESCE($2, title), body = COALESCE($3, body), sort = COALESCE($4, sort) WHERE id = $1 RETURNING title, body, sort"#, id, title, body, b.sort).map(|r| (r.title, r.body, r.sort))
    .fetch_optional(&state.db)
    .await?;
    let (title, body, sort) = row.ok_or(AppError::NotFound("answer"))?;
    Ok(Json(json!({ "id": id, "title": title, "body": body, "sort": sort })))
}

async fn delete_answer(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_manager(&auth)?;
    sqlx::query!("DELETE FROM support_answers WHERE id = $1", id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Files and statistics
// ---------------------------------------------------------------------------

/// Files attached to ticket messages: support sees them all; the requester sees those on
/// messages (not notes) of their own tickets.
pub async fn can_view_file(state: &AppState, auth: &AuthUser, file: Uuid) -> AppResult<bool> {
    Ok(sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM support_messages m JOIN support_tickets t ON t.id = m.ticket_id
         WHERE $2 = ANY(m.file_ids) AND ($3 OR t.responsible_id = $1 OR (t.requester_id = $1 AND m.kind = 'message'))) AS "v!""#, auth.id, file, worker(auth))
    .fetch_one(&state.db)
    .await?)
}

#[derive(Deserialize)]
struct StatsQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

async fn stats(State(state): State<AppState>, auth: AuthUser, Query(q): Query<StatsQuery>) -> AppResult<Json<Value>> {
    require_worker(&auth)?;
    let to = q.to.unwrap_or_else(|| Utc::now().date_naive());
    let from = q.from.unwrap_or(to - chrono::Duration::days(29));
    if from > to || (to - from).num_days() > 366 {
        return Err(AppError::bad("choose up to a year, from before to"));
    }
    let (start, end): (DateTime<Utc>, DateTime<Utc>) = (from.and_hms_opt(0, 0, 0).unwrap().and_utc(), (to + chrono::Duration::days(1)).and_hms_opt(0, 0, 0).unwrap().and_utc());
    let db = &state.db;
    let (created, closed, auto_closed): (i64, i64, i64) = sqlx::query!(r#"SELECT count(*) FILTER (WHERE created_at >= $1 AND created_at < $2) AS "c0!",
                count(*) FILTER (WHERE closed_at >= $1 AND closed_at < $2) AS "c1!",
                count(*) FILTER (WHERE closed_at >= $1 AND closed_at < $2 AND auto_closed) AS "c2!"
         FROM support_tickets WHERE NOT is_spam"#, start, end).map(|r| (r.c0, r.c1, r.c2))
    .fetch_one(db)
    .await?;
    let (open, overdue, unassigned, on_hold): (i64, i64, i64, i64) = sqlx::query!(r#"SELECT count(*) AS "c0!", count(*) FILTER (WHERE is_overdue) AS "c1!", count(*) FILTER (WHERE responsible_id IS NULL) AS "c2!", count(*) FILTER (WHERE on_hold) AS "c3!"
         FROM support_tickets WHERE closed_at IS NULL AND NOT is_spam"#).map(|r| (r.c0, r.c1, r.c2, r.c3))
    .fetch_one(db)
    .await?;
    let (first_response, resolution): (Option<f64>, Option<f64>) = sqlx::query!(r#"SELECT (SELECT avg(extract(epoch FROM first_response_at - created_at) / 60)::float8 FROM support_tickets
                 WHERE NOT is_spam AND created_at >= $1 AND created_at < $2 AND first_response_at IS NOT NULL) AS "c0",
                (SELECT avg(extract(epoch FROM closed_at - created_at) / 60)::float8 FROM support_tickets
                 WHERE NOT is_spam AND closed_at >= $1 AND closed_at < $2) AS "c1""#, start, end).map(|r| (r.c0, r.c1))
    .fetch_one(db)
    .await?;
    let (replies, late_replies, minutes): (i64, i64, i64) = sqlx::query!(r#"SELECT count(*) FILTER (WHERE kind = 'message') AS "c0!", count(*) FILTER (WHERE kind = 'message' AND is_overdue) AS "c1!", COALESCE(sum(minutes_spent), 0)::int8 AS "c2!"
         FROM support_messages WHERE by_support AND created_at >= $1 AND created_at < $2"#, start, end).map(|r| (r.c0, r.c1, r.c2))
    .fetch_one(db)
    .await?;
    let st = &state;
    let group = |col: &'static str| async move {
        let rows: Vec<(Option<Uuid>, Option<String>, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT t.{col}, d.name, count(*) FILTER (WHERE t.closed_at IS NULL), count(*) FILTER (WHERE t.created_at >= $1 AND t.created_at < $2)
             FROM support_tickets t LEFT JOIN support_dictionary d ON d.id = t.{col}
             WHERE NOT t.is_spam AND (t.closed_at IS NULL OR (t.created_at >= $1 AND t.created_at < $2))
             GROUP BY t.{col}, d.name, d.sort ORDER BY d.sort NULLS LAST"
        )))
        .bind(start)
        .bind(end)
        .fetch_all(&st.db)
        .await?;
        AppResult::Ok(rows.into_iter().map(|(id, name, open, created)| json!({ "id": id, "name": name.unwrap_or_else(|| "None".into()), "open": open, "created": created })).collect::<Vec<_>>())
    };
    let by_category = group("category_id").await?;
    let by_status = group("status_id").await?;
    let by_criticality = group("criticality_id").await?;
    let people: Vec<(Option<Uuid>, Option<String>, i64, i64, i64, i64, i64)> = sqlx::query!(r#"SELECT t.responsible_id AS "responsible_id?", u.first_name || ' ' || u.last_name AS "c1?",
                count(*) FILTER (WHERE t.closed_at IS NULL) AS "c2!", count(*) FILTER (WHERE t.closed_at IS NULL AND t.is_overdue) AS "c3!",
                count(*) FILTER (WHERE t.closed_at >= $1 AND t.closed_at < $2) AS "c4!",
                COALESCE((SELECT sum(m.minutes_spent) FROM support_messages m WHERE m.author_id = t.responsible_id AND m.created_at >= $1 AND m.created_at < $2), 0)::int8 AS "c5!",
                COALESCE((SELECT count(*) FROM support_messages m WHERE m.author_id = t.responsible_id AND m.by_support AND m.kind = 'message' AND m.is_overdue AND m.created_at >= $1 AND m.created_at < $2), 0)::int8 AS "c6!"
         FROM support_tickets t LEFT JOIN users u ON u.id = t.responsible_id
         WHERE NOT t.is_spam AND (t.closed_at IS NULL OR (t.closed_at >= $1 AND t.closed_at < $2))
         GROUP BY t.responsible_id, u.first_name, u.last_name ORDER BY u.first_name NULLS LAST"#, start, end).map(|r| (r.responsible_id, r.c1, r.c2, r.c3, r.c4, r.c5, r.c6))
    .fetch_all(db)
    .await?;
    let marks: Vec<(Uuid, String, i64)> = sqlx::query!(r#"SELECT d.id AS "id!", d.name AS "name!", count(t.id) AS "c2!" FROM support_dictionary d
         LEFT JOIN support_tickets t ON t.mark_id = d.id AND t.closed_at >= $1 AND t.closed_at < $2
         WHERE d.kind = 'mark' GROUP BY d.id, d.name, d.sort ORDER BY d.sort"#, start, end).map(|r| (r.id, r.name, r.c2))
    .fetch_all(db)
    .await?;
    let daily: Vec<(NaiveDate, i64, i64)> = sqlx::query!(r#"SELECT d::date AS "c0!", (SELECT count(*) FROM support_tickets WHERE NOT is_spam AND created_at >= d AND created_at < d + interval '1 day') AS "c1!",
                (SELECT count(*) FROM support_tickets WHERE NOT is_spam AND closed_at >= d AND closed_at < d + interval '1 day') AS "c2!"
         FROM generate_series($1::timestamptz, $2::timestamptz - interval '1 day', interval '1 day') d ORDER BY d"#, start, end).map(|r| (r.c0, r.c1, r.c2))
    .fetch_all(db)
    .await?;
    Ok(Json(json!({
        "from": from, "to": to,
        "created": created, "closed": closed, "auto_closed": auto_closed,
        "open": open, "overdue": overdue, "unassigned": unassigned, "on_hold": on_hold,
        "avg_first_response_minutes": first_response.map(|m| m.round() as i64),
        "avg_resolution_minutes": resolution.map(|m| m.round() as i64),
        "replies": replies, "late_replies": late_replies, "minutes_spent": minutes,
        "by_category": by_category, "by_status": by_status, "by_criticality": by_criticality,
        "by_responsible": people.into_iter().map(|(id, name, open, overdue, closed, minutes, late)| json!({
            "id": id, "name": name.unwrap_or_else(|| "Unassigned".into()), "open": open, "overdue": overdue, "closed": closed, "minutes_spent": minutes, "late_replies": late })).collect::<Vec<_>>(),
        "marks": marks.into_iter().map(|(id, name, n)| json!({ "id": id, "name": name, "count": n })).collect::<Vec<_>>(),
        "daily": daily.into_iter().map(|(d, c, x)| json!({ "day": d, "created": c, "closed": x })).collect::<Vec<_>>(),
    })))
}
