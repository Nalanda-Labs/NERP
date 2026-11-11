//! Record timeline: system events (created, stage moves, conversions, …) and comments.

use axum::{
    extract::{Path, Query, State},
    routing::{get, patch},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{broadcast, items::load_row, timeline};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/items/{id}/timeline", get(list).post(add_comment))
        .route("/crm/timeline/{entry}", patch(edit).delete(remove))
}

#[derive(Serialize, FromRow)]
struct Entry {
    id: Uuid,
    kind: String,
    author_id: Option<Uuid>,
    author_name: Option<String>,
    author_avatar_file_id: Option<Uuid>,
    body: Option<String>,
    data: Value,
    is_pinned: bool,
    edited_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct ListQuery {
    /// Load entries older than this (paging).
    before: Option<DateTime<Utc>>,
}

async fn readable(state: &AppState, auth: &AuthUser, item: Uuid) -> AppResult<(i16, Option<Uuid>)> {
    let r = load_row(state, item, false).await?;
    super::access::require_read(state, auth, r.entity_type, item, r.assigned_to).await?;
    Ok((r.entity_type, r.assigned_to))
}

/// Takes changes to fields the person may not see out of edit entries (and drops entries
/// left with none).
fn hide_changes(entries: &mut Vec<Entry>, hidden: &[String]) {
    if hidden.is_empty() {
        return;
    }
    entries.retain_mut(|e| {
        if e.kind != "edit" {
            return true;
        }
        if let Some(ch) = e.data.get_mut("changes").and_then(Value::as_array_mut) {
            ch.retain(|c| !c["field"].as_str().and_then(|f| f.strip_prefix("uf.")).is_some_and(|f| hidden.iter().any(|h| h == f)));
            return !ch.is_empty();
        }
        true
    });
}

async fn list(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let (t, _) = readable(&state, &auth, id).await?;
    let hidden = super::fields::hidden_codes(&state, &auth, t).await?;
    const SELECT: &str = "SELECT t.id, t.kind, t.author_id,
        CASE WHEN u.id IS NULL THEN NULL ELSE u.first_name || ' ' || u.last_name END AS author_name,
        u.avatar_file_id AS author_avatar_file_id, t.body, t.data, t.is_pinned, t.edited_at, t.created_at
        FROM crm_timeline t LEFT JOIN users u ON u.id = t.author_id WHERE t.item_id = $1";
    let pinned: Vec<Entry> = if q.before.is_none() {
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} AND t.is_pinned ORDER BY t.created_at DESC")))
            .bind(id)
            .fetch_all(&state.db)
            .await?
    } else {
        vec![]
    };
    let items: Vec<Entry> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} AND ($2::timestamptz IS NULL OR t.created_at < $2) ORDER BY t.created_at DESC LIMIT 50"
    )))
    .bind(id)
    .bind(q.before)
    .fetch_all(&state.db)
    .await?;
    let more = items.len() == 50;
    let (mut pinned, mut items) = (pinned, items);
    hide_changes(&mut pinned, &hidden);
    hide_changes(&mut items, &hidden);
    Ok(Json(json!({ "pinned": pinned, "items": items, "has_more": more })))
}

#[derive(Deserialize)]
struct NewComment {
    body: String,
}

async fn add_comment(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<NewComment>) -> AppResult<Json<Value>> {
    let (t, _) = readable(&state, &auth, id).await?;
    let body = required("comment", &b.body, 20_000)?;
    timeline(&state.db, id, "comment", Some(auth.id), Some(body.clone()), json!({})).await?;
    super::collab::tell_followers(&state, id, auth.id, true, "New comment on {record}".into(), Some(body.chars().take(200).collect())).await;
    sqlx::query("UPDATE crm_items SET last_activity_at = now() WHERE id = $1").bind(id).execute(&state.db).await?;
    broadcast(&state, t, id, false).await;
    super::automation::fire(&state, id, "comment_added", |_| true).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct EditEntry {
    body: Option<String>,
    is_pinned: Option<bool>,
}

async fn entry(state: &AppState, auth: &AuthUser, e: Uuid) -> AppResult<(Uuid, String, Option<Uuid>, i16)> {
    let (item, kind, author): (Uuid, String, Option<Uuid>) = sqlx::query_as("SELECT item_id, kind, author_id FROM crm_timeline WHERE id = $1")
        .bind(e)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("timeline entry"))?;
    let (t, _) = readable(state, auth, item).await?;
    Ok((item, kind, author, t))
}

/// Authors edit their comments; anyone who can see the record can pin entries.
async fn edit(State(state): State<AppState>, auth: AuthUser, Path(e): Path<Uuid>, Json(b): Json<EditEntry>) -> AppResult<Json<Value>> {
    let (item, kind, author, t) = entry(&state, &auth, e).await?;
    if let Some(body) = &b.body {
        if kind != "comment" || author != Some(auth.id) {
            return Err(AppError::bad("you can only edit your own comments"));
        }
        sqlx::query("UPDATE crm_timeline SET body = $1, edited_at = now() WHERE id = $2")
            .bind(required("comment", body, 20_000)?)
            .bind(e)
            .execute(&state.db)
            .await?;
    }
    if let Some(p) = b.is_pinned {
        sqlx::query("UPDATE crm_timeline SET is_pinned = $1 WHERE id = $2").bind(p).bind(e).execute(&state.db).await?;
    }
    broadcast(&state, t, item, false).await;
    Ok(Json(json!({ "ok": true })))
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(e): Path<Uuid>) -> AppResult<Json<Value>> {
    let (item, kind, author, t) = entry(&state, &auth, e).await?;
    if kind != "comment" || (author != Some(auth.id) && !super::access::is_crm_admin(&auth)) {
        return Err(AppError::bad("only comments can be deleted, by their author"));
    }
    sqlx::query("DELETE FROM crm_timeline WHERE id = $1").bind(e).execute(&state.db).await?;
    broadcast(&state, t, item, false).await;
    Ok(Json(json!({ "ok": true })))
}
