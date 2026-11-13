//! Two-way sync with outside calendars.
//!
//! A person connects a CalDAV account (iCloud with an app-specific password, Nextcloud,
//! Fastmail, Yahoo…), Google Calendar (through Google's CalDAV, signed in with Google) or
//! Microsoft 365 (through Microsoft Graph). Every few minutes, and on "Sync now":
//!
//! 1. what changed there comes into NERP (new events are added, changed ones updated);
//! 2. what was deleted there is deleted in NERP;
//! 3. what changed in NERP goes there, NERP events not yet there are added (from 30 days
//!    ago on), and what was deleted in NERP is deleted there.
//!
//! When an event changed on both sides since the last sync, the outside version wins.
//! NERP's own events keep their guests in NERP (they are sent without guests, so the
//! outside calendar invites no one); events that came from outside keep the guests they
//! have there. Events from one account never go to another.

mod graph;
mod remote;

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use self::{graph::Graph, remote::Auth, remote::Dav, remote::Item};
use super::{dav, ical, load_events};
use crate::{
    auth::{tokens::random_token, AuthUser},
    crm::email::oauth,
    crypto,
    error::{AppError, AppResult},
    state::AppState,
};

pub const SYNC_JOB: &str = "calendar.ext_sync";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/calendar/accounts", get(list))
        .route("/calendar/accounts/providers", get(providers))
        .route("/calendar/accounts/caldav/discover", post(discover))
        .route("/calendar/accounts/caldav", post(connect_caldav))
        .route("/calendar/accounts/oauth/{kind}", post(oauth_start))
        .route("/calendar/accounts/{id}", axum::routing::patch(update).delete(disconnect))
        .route("/calendar/accounts/{id}/sync", post(sync_now))
}

#[derive(sqlx::FromRow, Clone)]
struct Account {
    id: Uuid,
    user_id: Uuid,
    kind: String,
    name: String,
    username: Option<String>,
    secret: Vec<u8>,
    access_token: Option<Vec<u8>>,
    access_expires_at: Option<DateTime<Utc>>,
    calendar_url: String,
    calendar_name: Option<String>,
    push: bool,
    last_sync_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
}

const COLS: &str = "id, user_id, kind, name, username, secret, access_token, access_expires_at, calendar_url, calendar_name, push, last_sync_at, last_error";

async fn load(state: &AppState, id: Uuid) -> AppResult<Account> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM cal_ext_accounts WHERE id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("calendar account"))
}

async fn mine(state: &AppState, auth: &AuthUser, id: Uuid) -> AppResult<Account> {
    let a = load(state, id).await?;
    if a.user_id != auth.id {
        return Err(AppError::NotFound("calendar account"));
    }
    Ok(a)
}

async fn account_json(state: &AppState, a: &Account) -> AppResult<Value> {
    let (linked, from_there): (i64, i64) = sqlx::query!(r#"SELECT (SELECT COUNT(*) FROM cal_ext_links WHERE account_id = $1) AS "c0!", (SELECT COUNT(*) FROM cal_events WHERE ext_account_id = $1) AS "c1!""#, a.id).map(|r| (r.c0, r.c1))
    .fetch_one(&state.db)
    .await?;
    Ok(json!({
        "id": a.id, "kind": a.kind, "name": a.name, "calendar_name": a.calendar_name,
        "calendar_url": if a.kind == "caldav" { Some(a.calendar_url.clone()) } else { None },
        "push": a.push, "last_sync_at": a.last_sync_at, "last_error": a.last_error, "events": linked, "from_there": from_there,
    }))
}

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let rows: Vec<Account> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM cal_ext_accounts WHERE user_id = $1 ORDER BY created_at"))).bind(auth.id).fetch_all(&state.db).await?;
    let mut out = Vec::new();
    for a in &rows {
        out.push(account_json(&state, a).await?);
    }
    Ok(Json(json!({ "items": out })))
}

async fn providers(State(state): State<AppState>, _auth: AuthUser) -> Json<Value> {
    Json(json!({
        "google": state.config.oauth.google.is_some(),
        "microsoft": state.config.oauth.microsoft.is_some(),
        "redirect_uri": oauth::redirect_uri(&state),
    }))
}

// ---------------------------------------------------------------------------
// Connecting
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CaldavInput {
    url: String,
    username: String,
    password: String,
    #[serde(default = "yes")]
    push: bool,
    name: Option<String>,
}
fn yes() -> bool {
    true
}

/// The calendars at a CalDAV address, to pick one.
async fn discover(State(state): State<AppState>, _auth: AuthUser, Json(b): Json<CaldavInput>) -> AppResult<Json<Value>> {
    let d = Dav::new(&state, &b.url, Auth::Basic(b.username.trim().to_string(), b.password.clone())).map_err(AppError::bad)?;
    let found = d.discover().await.map_err(AppError::bad)?;
    Ok(Json(json!({ "items": found.into_iter().map(|(url, name)| json!({ "url": url, "name": name })).collect::<Vec<_>>() })))
}

async fn connect_caldav(State(state): State<AppState>, auth: AuthUser, Json(b): Json<CaldavInput>) -> AppResult<Json<Value>> {
    let user = b.username.trim().to_string();
    if user.is_empty() || b.password.is_empty() {
        return Err(AppError::bad("enter the user name and password"));
    }
    // The address must be a calendar for events (found, if it's the account's address).
    let d = Dav::new(&state, &b.url, Auth::Basic(user.clone(), b.password.clone())).map_err(AppError::bad)?;
    let found = d.discover().await.map_err(AppError::bad)?;
    let (url, cal_name) = found.into_iter().next().ok_or_else(|| AppError::bad("no calendar there"))?;
    let n: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM cal_ext_accounts WHERE user_id = $1"#, auth.id).fetch_one(&state.db).await?;
    if n >= 10 {
        return Err(AppError::bad("at most 10 connected calendars"));
    }
    let name = b.name.map(|n| n.trim().chars().take(200).collect::<String>()).filter(|n| !n.is_empty()).unwrap_or_else(|| user.clone());
    let id: Uuid = sqlx::query_scalar!("INSERT INTO cal_ext_accounts (user_id, kind, name, username, secret, calendar_url, calendar_name, push) VALUES ($1, 'caldav', $2, $3, $4, $5, $6, $7)
         ON CONFLICT (user_id, calendar_url) DO UPDATE SET username = EXCLUDED.username, secret = EXCLUDED.secret, last_error = NULL RETURNING id", auth.id, &name, &user, crypto::seal(&state.config.data_key, &b.password), &url, &cal_name, b.push)
    .fetch_one(&state.db)
    .await?;
    crate::audit::record(&state.db, Some(auth.id), "calendar.account.connect", "cal_ext_account", Some(id.to_string()), json!({ "kind": "caldav", "url": url }), None).await;
    run(&state, id).await;
    Ok(Json(account_json(&state, &load(&state, id).await?).await?))
}

/// Starts Google or Microsoft sign-in for calendar access.
async fn oauth_start(State(state): State<AppState>, auth: AuthUser, Path(kind): Path<String>) -> AppResult<Json<Value>> {
    let mut p = oauth::provider(&state, &kind).ok_or_else(|| AppError::bad(format!("{kind} sign-in is not configured on this server")))?;
    p.scope = scope(&kind);
    let token = random_token(24);
    let verifier = random_token(48);
    let mut redis = state.redis.clone();
    let _: () = redis.set_ex(format!("crm:oauth:{token}"), json!({ "user": auth.id, "kind": kind, "verifier": verifier, "purpose": "calendar" }).to_string(), 600).await?;
    Ok(Json(json!({ "url": oauth::authorize_url(&p, &oauth::redirect_uri(&state), &token, &oauth::pkce_challenge(&verifier)) })))
}

fn scope(kind: &str) -> &'static str {
    if kind == "google" {
        "https://www.googleapis.com/auth/calendar openid email profile"
    } else {
        "https://graph.microsoft.com/Calendars.ReadWrite offline_access openid email profile"
    }
}

/// The provider sent the person back (from the shared OAuth callback): connect the account.
pub async fn oauth_finish(state: &AppState, saved: Value, code: String) -> Result<String, String> {
    let user: Uuid = serde_json::from_value(saved["user"].clone()).map_err(|e| e.to_string())?;
    let kind = saved["kind"].as_str().unwrap_or_default().to_string();
    let mut p = oauth::provider(state, &kind).ok_or("this provider is no longer configured")?;
    p.scope = scope(&kind);
    let t = oauth::token_request(&p, &[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &oauth::redirect_uri(state)),
        ("code_verifier", saved["verifier"].as_str().unwrap_or_default()),
    ])
    .await?;
    let refresh = t.refresh_token.ok_or("the provider did not grant offline access; try again and accept all permissions")?;
    let claims = t.id_token.as_deref().and_then(oauth::id_claims).unwrap_or(json!({}));
    let email = ["email", "preferred_username", "upn"].iter().find_map(|k| claims[k].as_str().filter(|s| s.contains('@'))).ok_or("the provider did not share the email address")?.to_lowercase();
    // Which calendar: Google's primary one (named after the account), Microsoft's default.
    let (cal_url, cal_name) = if kind == "google" {
        (format!("{}/{}/events/", state.config.oauth.google_caldav_url, email), "Google Calendar".to_string())
    } else {
        Graph::new(state, t.access_token.clone())?.default_calendar().await?
    };
    let key = &state.config.data_key;
    let id: Uuid = sqlx::query_scalar!("INSERT INTO cal_ext_accounts (user_id, kind, name, username, secret, access_token, access_expires_at, calendar_url, calendar_name)
         VALUES ($1, $2, $3, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (user_id, calendar_url) DO UPDATE SET secret = EXCLUDED.secret, access_token = EXCLUDED.access_token,
             access_expires_at = EXCLUDED.access_expires_at, last_error = NULL RETURNING id", user, &kind, &email, crypto::seal(key, &refresh), crypto::seal(key, &t.access_token), Utc::now() + Duration::seconds(t.expires_in.unwrap_or(3600)), &cal_url, &cal_name)
    .fetch_one(&state.db)
    .await
    .map_err(|e| e.to_string())?;
    crate::audit::record(&state.db, Some(user), "calendar.account.connect", "cal_ext_account", Some(id.to_string()), json!({ "kind": kind, "email": email }), None).await;
    let s = state.clone();
    tokio::spawn(async move { run(&s, id).await });
    Ok(email)
}

/// A valid access token for a Google or Microsoft account, refreshed when needed.
async fn access_token(state: &AppState, a: &Account) -> Result<String, String> {
    let key = &state.config.data_key;
    if let (Some(tok), Some(exp)) = (&a.access_token, a.access_expires_at) {
        if exp > Utc::now() + Duration::seconds(90) {
            if let Some(t) = crypto::open(key, tok) {
                return Ok(t);
            }
        }
    }
    let mut p = oauth::provider(state, &a.kind).ok_or("this provider is no longer configured on the server")?;
    p.scope = scope(&a.kind);
    let refresh = crypto::open(key, &a.secret).ok_or("the stored sign-in can't be read; connect the account again")?;
    let t = oauth::token_request(&p, &[("grant_type", "refresh_token"), ("refresh_token", &refresh)]).await?;
    sqlx::query!("UPDATE cal_ext_accounts SET access_token = $2, access_expires_at = $3, secret = COALESCE($4, secret) WHERE id = $1", a.id, crypto::seal(key, &t.access_token), Utc::now() + Duration::seconds(t.expires_in.unwrap_or(3600)), t.refresh_token.as_deref().map(|r| crypto::seal(key, r)))
        .execute(&state.db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(t.access_token)
}

#[derive(Deserialize)]
struct UpdateInput {
    push: Option<bool>,
    name: Option<String>,
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<UpdateInput>) -> AppResult<Json<Value>> {
    let a = mine(&state, &auth, id).await?;
    let name = b.name.map(|n| n.trim().chars().take(200).collect::<String>()).filter(|n| !n.is_empty());
    sqlx::query!("UPDATE cal_ext_accounts SET push = COALESCE($2, push), name = COALESCE($3, name) WHERE id = $1", a.id, b.push, name).execute(&state.db).await?;
    Ok(Json(account_json(&state, &load(&state, id).await?).await?))
}

#[derive(Deserialize)]
struct DisconnectQuery {
    /// Keep the events that came from there (as ordinary NERP events).
    #[serde(default)]
    keep: bool,
}

/// Stops syncing. Events that came from there go too, unless kept; nothing is deleted there.
async fn disconnect(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<DisconnectQuery>) -> AppResult<Json<Value>> {
    let a = mine(&state, &auth, id).await?;
    if !q.keep {
        sqlx::query!("DELETE FROM cal_events WHERE ext_account_id = $1", a.id).execute(&state.db).await?;
    }
    sqlx::query!("DELETE FROM cal_ext_accounts WHERE id = $1", a.id).execute(&state.db).await?;
    crate::audit::record(&state.db, Some(auth.id), "calendar.account.disconnect", "cal_ext_account", Some(id.to_string()), json!({ "kind": a.kind, "name": a.name }), None).await;
    Ok(Json(json!({ "ok": true })))
}

async fn sync_now(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let a = mine(&state, &auth, id).await?;
    run(&state, a.id).await;
    Ok(Json(account_json(&state, &load(&state, id).await?).await?))
}

// ---------------------------------------------------------------------------
// Syncing
// ---------------------------------------------------------------------------

/// Every five minutes: accounts not synced for four.
pub async fn run_due(state: &AppState) -> Result<()> {
    let due: Vec<Uuid> = sqlx::query_scalar!("SELECT id FROM cal_ext_accounts WHERE last_sync_at IS NULL OR last_sync_at < now() - interval '4 minutes' ORDER BY last_sync_at NULLS FIRST LIMIT 50")
        .fetch_all(&state.db)
        .await?;
    for id in due {
        run(state, id).await;
    }
    Ok(())
}

/// Syncs one account (one sync at a time per account), noting the outcome.
async fn run(state: &AppState, id: Uuid) {
    let mut redis = state.redis.clone();
    let lock = format!("calsync:lock:{id}");
    let got: bool = redis::cmd("SET").arg(&lock).arg(1).arg("NX").arg("EX").arg(300).query_async(&mut redis).await.map(|v: Option<String>| v.is_some()).unwrap_or(false);
    if !got {
        return;
    }
    let outcome = match load(state, id).await {
        Ok(a) => sync(state, &a).await,
        Err(_) => Ok(()),
    };
    let err = outcome.err().map(|e| e.chars().take(500).collect::<String>());
    if let Some(e) = &err {
        tracing::warn!("calendar sync {id}: {e}");
    }
    let _ = sqlx::query!("UPDATE cal_ext_accounts SET last_sync_at = now(), last_error = $2 WHERE id = $1", id, err).execute(&state.db).await;
    let _: Result<(), _> = redis.del(&lock).await;
}

/// The outside calendar, whichever kind.
enum Remote {
    Dav(Dav),
    Graph(Graph, String),
}

enum UpdateError {
    Conflict,
    Other(String),
}

impl Remote {
    async fn open(state: &AppState, a: &Account) -> Result<Remote, String> {
        Ok(match a.kind.as_str() {
            "caldav" => {
                let pw = crypto::open(&state.config.data_key, &a.secret).ok_or("the stored password can't be read; connect the account again")?;
                Remote::Dav(Dav::new(state, &a.calendar_url, Auth::Basic(a.username.clone().unwrap_or_default(), pw))?)
            }
            "google" => Remote::Dav(Dav::new(state, &a.calendar_url, Auth::Bearer(access_token(state, a).await?))?),
            _ => Remote::Graph(Graph::new(state, access_token(state, a).await?)?, a.calendar_url.clone()),
        })
    }

    async fn list(&self) -> Result<Vec<Item>, String> {
        match self {
            Remote::Dav(d) => d.list().await,
            Remote::Graph(g, cal) => g.list(cal).await,
        }
    }

    /// The items' events, read as iCalendar (unreadable ones are skipped).
    async fn fetch(&self, items: &[Item], tz: chrono_tz::Tz) -> Result<Vec<(Item, Vec<ical::Parsed>)>, String> {
        let mut out = Vec::new();
        match self {
            Remote::Dav(d) => {
                let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
                for (item, data) in d.fetch(&ids).await? {
                    match ical::parse(&data).and_then(|c| ical::events(&c, tz)) {
                        Ok(p) if !p.is_empty() => out.push((item, p)),
                        Ok(_) => {}
                        Err(e) => tracing::info!("calendar sync: skipped {}: {e}", item.id),
                    }
                }
            }
            Remote::Graph(g, _) => {
                for item in items {
                    match g.fetch(&item.id).await {
                        Ok(Some(p)) => out.push((item.clone(), p)),
                        Ok(None) => {}
                        Err(e) => tracing::info!("calendar sync: skipped {}: {e}", item.id),
                    }
                }
            }
        }
        Ok(out)
    }

    async fn create(&self, state: &AppState, e: &super::Event) -> Result<Item, String> {
        match self {
            Remote::Dav(d) => {
                let ics = dav::ics_with(state, e.id, false).await.map_err(|x| x.to_string())?;
                d.put(None, &format!("{}.ics", e.id), ics).await.map_err(|x| match x {
                    remote::PutError::Conflict => "an event with that name is there already".into(),
                    remote::PutError::Other(s) => s,
                })
            }
            Remote::Graph(g, cal) => g.create(cal, e, &exdates(state, e.id).await?).await,
        }
    }

    async fn update(&self, state: &AppState, e: &super::Event, item: &Item) -> Result<Item, UpdateError> {
        match self {
            Remote::Dav(d) => {
                let ics = dav::ics_with(state, e.id, false).await.map_err(|x| UpdateError::Other(x.to_string()))?;
                d.put(Some(item), "", ics).await.map_err(|x| match x {
                    remote::PutError::Conflict => UpdateError::Conflict,
                    remote::PutError::Other(s) => UpdateError::Other(s),
                })
            }
            Remote::Graph(g, _) => {
                let ex = exdates(state, e.id).await.map_err(UpdateError::Other)?;
                g.update(item, e, &ex).await.map_err(|x| match x {
                    graph::GraphError::Conflict => UpdateError::Conflict,
                    graph::GraphError::Other(s) => UpdateError::Other(s),
                })
            }
        }
    }

    async fn delete(&self, item: &Item) -> Result<(), String> {
        match self {
            Remote::Dav(d) => d.delete(item).await,
            Remote::Graph(g, _) => g.delete(item).await,
        }
    }
}

async fn exdates(state: &AppState, id: Uuid) -> Result<Vec<DateTime<Utc>>, String> {
    sqlx::query_scalar!("SELECT occurrence_start FROM cal_exceptions WHERE event_id = $1", id).fetch_all(&state.db).await.map_err(|e| e.to_string())
}

#[derive(sqlx::FromRow)]
struct Link {
    event_id: Uuid,
    remote_id: String,
    remote_etag: Option<String>,
    local_seq: i64,
    /// The event's version now (None: deleted in NERP).
    seq: Option<i64>,
    /// It came from this account (rather than from NERP).
    imported: Option<bool>,
}

async fn seq_of(state: &AppState, id: Uuid) -> Result<i64, String> {
    sqlx::query_scalar!("SELECT dav_seq FROM cal_events WHERE id = $1", id).fetch_one(&state.db).await.map_err(|e| e.to_string())
}

async fn link(state: &AppState, account: Uuid, event: Uuid, item: &Item) -> Result<(), String> {
    let seq = seq_of(state, event).await?;
    sqlx::query!("INSERT INTO cal_ext_links (account_id, event_id, remote_id, remote_etag, local_seq) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (account_id, event_id) DO UPDATE SET remote_id = $3, remote_etag = $4, local_seq = $5", account, event, &item.id, &item.etag, seq)
    .execute(&state.db)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

async fn unlink(state: &AppState, account: Uuid, event: Uuid) -> Result<(), String> {
    sqlx::query!("DELETE FROM cal_ext_links WHERE account_id = $1 AND event_id = $2", account, event).execute(&state.db).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn sync(state: &AppState, a: &Account) -> Result<(), String> {
    let remote = Remote::open(state, a).await?;
    let (email, name, tz): (String, String, String) = sqlx::query!(r#"SELECT email AS "email!", trim(first_name || ' ' || last_name) AS "c1!", timezone AS "timezone!" FROM users WHERE id = $1 AND is_active"#, a.user_id).map(|r| (r.email, r.c1, r.timezone))
        .fetch_optional(&state.db)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("the person isn't active")?;
    let me = dav::Me { id: a.user_id, email: email.to_lowercase(), name, tz: tz.parse().unwrap_or(chrono_tz::UTC) };
    let items = remote.list().await?;
    let links: Vec<Link> = sqlx::query_as!(Link, r#"SELECT l.event_id AS "event_id!", l.remote_id AS "remote_id!", l.remote_etag AS "remote_etag?", l.local_seq AS "local_seq!", e.dav_seq AS "seq?", (e.ext_account_id = l.account_id) AS "imported?"
         FROM cal_ext_links l LEFT JOIN cal_events e ON e.id = l.event_id WHERE l.account_id = $1"#, a.id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| e.to_string())?;
    let mut handled: Vec<Uuid> = Vec::new();

    // 1. Changed and new there.
    let changed: Vec<Item> = items.iter().filter(|i| links.iter().find(|l| l.remote_id == i.id).is_none_or(|l| l.remote_etag.as_deref() != Some(i.etag.as_str()))).cloned().collect();
    for (item, parsed) in remote.fetch(&changed, me.tz).await? {
        match links.iter().find(|l| l.remote_id == item.id) {
            Some(l) => {
                handled.push(l.event_id);
                match l.seq {
                    // Deleted in NERP meanwhile: deleting it there wins.
                    None => {
                        if a.push {
                            remote.delete(&item).await?;
                        }
                        unlink(state, a.id, l.event_id).await?;
                    }
                    Some(_) => {
                        let keep_people = l.imported != Some(true);
                        dav::store(state, &me, Some(l.event_id), &parsed, None, None, keep_people).await.map_err(|e| e.to_string())?;
                        link(state, a.id, l.event_id, &item).await?;
                    }
                }
            }
            None => {
                // One of ours that lost its link (sent before, or the account connected again)?
                let uid = parsed[0].uid.clone();
                let ours: Option<Uuid> = sqlx::query_scalar!("SELECT id FROM cal_events WHERE owner_id = $1 AND parent_id IS NULL AND (uid = $2 OR id::text || '@nerp' = $2)
                       AND NOT EXISTS (SELECT 1 FROM cal_ext_links WHERE account_id = $3 AND event_id = cal_events.id)", a.user_id, &uid, a.id)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| e.to_string())?;
                let id = match ours {
                    Some(id) => {
                        let imported: bool = sqlx::query_scalar("SELECT ext_account_id IS NOT DISTINCT FROM $2 FROM cal_events WHERE id = $1").bind(id).bind(a.id).fetch_one(&state.db).await.map_err(|e| e.to_string())?;
                        dav::store(state, &me, Some(id), &parsed, None, None, !imported).await.map_err(|e| e.to_string())?
                    }
                    None => dav::store(state, &me, None, &parsed, None, Some(a.id), false).await.map_err(|e| e.to_string())?,
                };
                handled.push(id);
                link(state, a.id, id, &item).await?;
            }
        }
    }

    // 2. Deleted there.
    for l in links.iter().filter(|l| !items.iter().any(|i| i.id == l.remote_id)) {
        handled.push(l.event_id);
        if l.seq.is_some() {
            let gone = load_events(state, &[l.event_id]).await.map_err(|e| e.to_string())?;
            sqlx::query!("DELETE FROM cal_events WHERE id = $1", l.event_id).execute(&state.db).await.map_err(|e| e.to_string())?;
            if let Some((e, guests)) = gone.into_iter().next() {
                crate::notify::notify(state, &guests, crate::notify::NewNotification { kind: "calendar.cancelled", title: format!("“{}” was cancelled", e.title), body: None, link: Some("/calendar".into()), actor: Some(a.user_id) }).await;
            }
        }
        unlink(state, a.id, l.event_id).await?;
    }

    if !a.push {
        return Ok(());
    }
    // 3. Changed and deleted in NERP.
    for l in links.iter().filter(|l| !handled.contains(&l.event_id)) {
        let item = Item { id: l.remote_id.clone(), etag: l.remote_etag.clone().unwrap_or_default() };
        match l.seq {
            None => {
                remote.delete(&item).await?;
                unlink(state, a.id, l.event_id).await?;
            }
            Some(s) if s > l.local_seq => {
                let Some((e, _)) = load_events(state, &[l.event_id]).await.map_err(|e| e.to_string())?.into_iter().next() else { continue };
                match remote.update(state, &e, &item).await {
                    Ok(new) => link(state, a.id, l.event_id, &new).await?,
                    // Changed there too: the next sync brings their version in.
                    Err(UpdateError::Conflict) => {}
                    Err(UpdateError::Other(x)) => return Err(x),
                }
            }
            _ => {}
        }
    }
    // New in NERP: the person's own events (not from another account), from 30 days ago on.
    let fresh: Vec<Uuid> = sqlx::query_scalar!("SELECT e.id FROM cal_events e WHERE e.owner_id = $1 AND e.parent_id IS NULL AND e.ext_account_id IS NULL
           AND (e.recurrence IS NOT NULL OR e.ends_at > now() - interval '30 days')
           AND NOT EXISTS (SELECT 1 FROM cal_ext_links l WHERE l.account_id = $2 AND l.event_id = e.id)
         ORDER BY e.starts_at LIMIT 500", a.user_id, a.id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| e.to_string())?;
    for (e, _) in load_events(state, &fresh).await.map_err(|e| e.to_string())? {
        let item = remote.create(state, &e).await?;
        link(state, a.id, e.id, &item).await?;
    }
    Ok(())
}
