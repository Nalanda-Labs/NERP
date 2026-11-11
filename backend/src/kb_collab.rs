//! Editing a knowledge base page together, live.
//!
//! Editors share a Yjs document (a CRDT: everyone's changes merge, in any order). The server
//! keeps the document's updates for the page in Redis, merges them now and then (with `yrs`,
//! the Rust port of Yjs) and passes each update and each editor's cursor on to the page's
//! other editors through the real-time connection. The page itself is still saved as
//! Markdown, with a version each time, as before.
//!
//! The first editor of a page with no live document seeds it from the saved page (a lock
//! makes sure only one does); the document is forgotten a day after the last change, or
//! when the page is changed outside the live document (a Markdown save, a restore).

use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::Utc;
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/kb/pages/{id}/collab", post(join))
        .route("/kb/pages/{id}/collab/update", post(update))
        .route("/kb/pages/{id}/collab/awareness", post(awareness))
        .route("/kb/pages/{id}/collab/leave", post(leave))
}

const TTL: u64 = 24 * 3600;
/// An editor counts as present for this long after their last sign of life.
const PRESENT_SECS: i64 = 60;
const MAX_UPDATE: usize = 1 << 20;
/// Updates kept before they are merged into one.
const COMPACT_AT: isize = 64;

fn updates_key(page: Uuid) -> String {
    format!("kb:collab:{page}:updates")
}
fn people_key(page: Uuid) -> String {
    format!("kb:collab:{page}:people")
}
fn seed_key(page: Uuid) -> String {
    format!("kb:collab:{page}:seed")
}

/// Forgets a page's live document (it was changed some other way); editors start again.
pub async fn reset(state: &AppState, page: Uuid) {
    let mut r = state.redis.clone();
    let people: Vec<Uuid> = present(state, page).await.into_iter().map(|p| p.0).collect();
    let _: Result<(), _> = r.del(&[updates_key(page), seed_key(page)]).await;
    if !people.is_empty() {
        crate::realtime::publish(state, Some(people), "kb.collab.reset", json!({ "page_id": page })).await;
    }
}

/// Editors present now: (user, client, name).
async fn present(state: &AppState, page: Uuid) -> Vec<(Uuid, String, String)> {
    let mut r = state.redis.clone();
    let all: Vec<(String, String)> = r.hgetall(people_key(page)).await.unwrap_or_default();
    let now = Utc::now().timestamp();
    let mut out = Vec::new();
    for (client, v) in all {
        let Ok(p) = serde_json::from_str::<Value>(&v) else { continue };
        if now - p["at"].as_i64().unwrap_or(0) > PRESENT_SECS {
            let _: Result<(), _> = r.hdel(people_key(page), &client).await;
            continue;
        }
        if let Some(u) = p["user"].as_str().and_then(|u| Uuid::parse_str(u).ok()) {
            out.push((u, client, p["name"].as_str().unwrap_or("").to_string()));
        }
    }
    out
}

async fn may_edit(state: &AppState, auth: &AuthUser, page: Uuid) -> AppResult<()> {
    let space: Uuid = sqlx::query_scalar("SELECT space_id FROM kb_pages WHERE id = $1").bind(page).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("page"))?;
    crate::kb::editable(state, auth, space).await
}

fn client_id(c: &str) -> AppResult<String> {
    let c = c.trim();
    if c.is_empty() || c.len() > 40 || !c.chars().all(|x| x.is_ascii_alphanumeric() || x == '-') {
        return Err(AppError::bad("bad client id"));
    }
    Ok(c.to_string())
}

#[derive(Deserialize)]
struct JoinBody {
    /// The editor's own id for this document (its Yjs client id).
    client: String,
}

async fn mark_present(state: &AppState, auth: &AuthUser, page: Uuid, client: &str) -> AppResult<String> {
    let name: String = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1").bind(auth.id).fetch_one(&state.db).await?;
    let mut r = state.redis.clone();
    let _: () = r.hset(people_key(page), client, json!({ "user": auth.id, "name": name, "at": Utc::now().timestamp() }).to_string()).await?;
    let _: () = r.expire(people_key(page), TTL as i64).await?;
    Ok(name)
}

/// The live document so far (merged), whether this editor seeds it, and who else is here.
async fn join(State(state): State<AppState>, auth: AuthUser, Path(page): Path<Uuid>, Json(b): Json<JoinBody>) -> AppResult<Json<Value>> {
    may_edit(&state, &auth, page).await?;
    let client = client_id(&b.client)?;
    mark_present(&state, &auth, page, &client).await?;
    let mut r = state.redis.clone();
    let updates: Vec<Vec<u8>> = r.lrange(updates_key(page), 0, -1).await?;
    let merged = merge(&updates)?;
    // Nothing yet: one editor (the first to ask) puts the saved page in.
    let seed = merged.is_none() && redis::cmd("SET").arg(seed_key(page)).arg(&client).arg("NX").arg("EX").arg(30).query_async::<Option<String>>(&mut r).await?.is_some();
    let others: Vec<Value> = present(&state, page).await.into_iter().filter(|p| p.1 != client).map(|(u, c, n)| json!({ "user_id": u, "client": c, "name": n })).collect();
    Ok(Json(json!({ "state": merged.map(|m| B64.encode(m)), "seed": seed, "people": others })))
}

fn merge(updates: &[Vec<u8>]) -> AppResult<Option<Vec<u8>>> {
    if updates.is_empty() {
        return Ok(None);
    }
    if updates.len() == 1 {
        return Ok(Some(updates[0].clone()));
    }
    let refs: Vec<&[u8]> = updates.iter().map(Vec::as_slice).collect();
    yrs::merge_updates_v1(&refs).map(Some).map_err(|_| AppError::bad("the live document is damaged"))
}

#[derive(Deserialize)]
struct UpdateBody {
    client: String,
    /// A Yjs update (v1), base64.
    update: String,
}

/// An editor's change: kept, and passed on to the page's other editors.
async fn update(State(state): State<AppState>, auth: AuthUser, Path(page): Path<Uuid>, Json(b): Json<UpdateBody>) -> AppResult<Json<Value>> {
    may_edit(&state, &auth, page).await?;
    let client = client_id(&b.client)?;
    let bytes = B64.decode(b.update.trim()).map_err(|_| AppError::bad("bad update"))?;
    if bytes.is_empty() || bytes.len() > MAX_UPDATE {
        return Err(AppError::bad("bad update"));
    }
    // A real Yjs update (merging it alone checks it decodes).
    yrs::merge_updates_v1(&[bytes.as_slice()]).map_err(|_| AppError::bad("bad update"))?;
    let mut r = state.redis.clone();
    let n: isize = r.rpush(updates_key(page), &bytes).await?;
    let _: () = r.expire(updates_key(page), TTL as i64).await?;
    mark_present(&state, &auth, page, &client).await?;
    let to: Vec<Uuid> = present(&state, page).await.into_iter().map(|p| p.0).collect();
    crate::realtime::publish(&state, Some(to), "kb.collab", json!({ "page_id": page, "client": client, "update": b.update.trim() })).await;
    if n >= COMPACT_AT {
        compact(&state, page, n).await?;
    }
    Ok(Json(json!({ "ok": true })))
}

/// Replaces the first `n` updates with their merge (later ones stay after it).
async fn compact(state: &AppState, page: Uuid, n: isize) -> AppResult<()> {
    let mut r = state.redis.clone();
    let first: Vec<Vec<u8>> = r.lrange(updates_key(page), 0, n - 1).await?;
    let Some(merged) = merge(&first)? else { return Ok(()) };
    let script = redis::Script::new("redis.call('LTRIM', KEYS[1], tonumber(ARGV[1]), -1) redis.call('LPUSH', KEYS[1], ARGV[2]) return 1");
    let _: i64 = script.key(updates_key(page)).arg(first.len()).arg(merged).invoke_async(&mut r).await?;
    Ok(())
}

#[derive(Deserialize)]
struct AwarenessBody {
    client: String,
    /// A y-protocols awareness update, base64 (cursors, names).
    update: String,
}

/// Where an editor is: passed on to the others, and keeps them counted as present.
async fn awareness(State(state): State<AppState>, auth: AuthUser, Path(page): Path<Uuid>, Json(b): Json<AwarenessBody>) -> AppResult<Json<Value>> {
    may_edit(&state, &auth, page).await?;
    let client = client_id(&b.client)?;
    if b.update.len() > 16_384 || B64.decode(b.update.trim()).is_err() {
        return Err(AppError::bad("bad awareness update"));
    }
    mark_present(&state, &auth, page, &client).await?;
    let to: Vec<Uuid> = present(&state, page).await.into_iter().map(|p| p.0).collect();
    crate::realtime::publish(&state, Some(to), "kb.collab.awareness", json!({ "page_id": page, "client": client, "update": b.update.trim() })).await;
    Ok(Json(json!({ "ok": true })))
}

async fn leave(State(state): State<AppState>, auth: AuthUser, Path(page): Path<Uuid>, Json(b): Json<JoinBody>) -> AppResult<Json<Value>> {
    may_edit(&state, &auth, page).await?;
    let client = client_id(&b.client)?;
    let mut r = state.redis.clone();
    let _: () = r.hdel(people_key(page), &client).await?;
    Ok(Json(json!({ "ok": true })))
}
