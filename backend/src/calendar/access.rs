//! Who may see or change a person's calendar.
//!
//! Each person sets a level for everyone, for departments and for single colleagues; the
//! most specific match wins (a colleague's own row, then their departments' best, then the row
//! for everyone). Without any row a calendar is open to read, as it always was.
//!
//! - **none**: the calendar can't be opened (shared events still show to their guests);
//! - **busy**: times only, every event as "Busy";
//! - **read**: details of events, private ones as "Busy";
//! - **edit**: everything, and adding, changing and removing events on it.

use axum::{extract::State, routing::get, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    None,
    Busy,
    Read,
    Edit,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        Some(match s {
            "none" => Level::None,
            "busy" => Level::Busy,
            "read" => Level::Read,
            "edit" => Level::Edit,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Level::None => "none",
            Level::Busy => "busy",
            Level::Read => "read",
            Level::Edit => "edit",
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/calendar/access", get(get_access).put(put_access))
        .route("/calendar/shared-with-me", get(shared_with_me))
}

/// What `viewer` may do with `owner`'s calendar.
pub async fn level(state: &AppState, viewer: Uuid, owner: Uuid) -> AppResult<Level> {
    if viewer == owner {
        return Ok(Level::Edit);
    }
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT grantee_kind, level FROM cal_access
         WHERE owner_id = $1 AND (grantee_kind = 'everyone'
            OR (grantee_kind = 'user' AND grantee_id = $2)
            OR (grantee_kind = 'department' AND grantee_id IN (SELECT department_id FROM user_departments WHERE user_id = $2)))",
    )
    .bind(owner)
    .bind(viewer)
    .fetch_all(&state.db)
    .await?;
    let best = |kind: &str| rows.iter().filter(|(k, _)| k == kind).filter_map(|(_, l)| Level::parse(l)).max();
    Ok(best("user").or_else(|| best("department")).or_else(|| best("everyone")).unwrap_or(Level::Read))
}

async fn get_access(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    Ok(Json(access_json(&state, auth.id).await?))
}

async fn access_json(state: &AppState, owner: Uuid) -> AppResult<Value> {
    let rows: Vec<(String, Option<Uuid>, String, Option<String>)> = sqlx::query_as(
        "SELECT a.grantee_kind, a.grantee_id, a.level,
                CASE a.grantee_kind WHEN 'user' THEN (SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = a.grantee_id)
                                    WHEN 'department' THEN (SELECT name FROM departments WHERE id = a.grantee_id) END
         FROM cal_access a WHERE a.owner_id = $1 ORDER BY a.grantee_kind, 4",
    )
    .bind(owner)
    .fetch_all(&state.db)
    .await?;
    let everyone = rows.iter().find(|r| r.0 == "everyone").map(|r| r.2.clone()).unwrap_or_else(|| "read".into());
    Ok(json!({
        "everyone": everyone,
        "grants": rows.iter().filter(|r| r.0 != "everyone").map(|(k, id, l, name)| json!({ "kind": k, "id": id, "level": l, "name": name })).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
struct Grant {
    kind: String,
    id: Uuid,
    level: String,
}

#[derive(Deserialize)]
struct AccessInput {
    everyone: String,
    #[serde(default)]
    grants: Vec<Grant>,
}

/// Replaces one's own sharing settings.
async fn put_access(State(state): State<AppState>, auth: AuthUser, Json(b): Json<AccessInput>) -> AppResult<Json<Value>> {
    let everyone = Level::parse(&b.everyone).filter(|l| *l != Level::Edit).ok_or_else(|| AppError::bad("everyone may get none, busy or read"))?;
    if b.grants.len() > 200 {
        return Err(AppError::bad("at most 200 entries"));
    }
    for g in &b.grants {
        if Level::parse(&g.level).is_none() {
            return Err(AppError::bad("levels are none, busy, read or edit"));
        }
        let exists: bool = match g.kind.as_str() {
            "user" if g.id == auth.id => return Err(AppError::bad("you always have your own calendar")),
            "user" => sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)").bind(g.id).fetch_one(&state.db).await?,
            "department" => sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM departments WHERE id = $1)").bind(g.id).fetch_one(&state.db).await?,
            _ => return Err(AppError::bad("share with a user or a department")),
        };
        if !exists {
            return Err(AppError::bad(format!("no such {}", g.kind)));
        }
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM cal_access WHERE owner_id = $1").bind(auth.id).execute(&mut *tx).await?;
    if everyone != Level::Read {
        sqlx::query("INSERT INTO cal_access (owner_id, grantee_kind, level) VALUES ($1, 'everyone', $2)").bind(auth.id).bind(everyone.name()).execute(&mut *tx).await?;
    }
    for g in &b.grants {
        sqlx::query("INSERT INTO cal_access (owner_id, grantee_kind, grantee_id, level) VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING")
            .bind(auth.id)
            .bind(&g.kind)
            .bind(g.id)
            .bind(&g.level)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(Json(access_json(&state, auth.id).await?))
}

/// Colleagues who gave me (or my departments) access beyond the default, with my level.
async fn shared_with_me(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let owners: Vec<(Uuid, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT DISTINCT u.id, trim(u.first_name || ' ' || u.last_name), u.avatar_file_id FROM cal_access a JOIN users u ON u.id = a.owner_id
         WHERE u.is_active AND ((a.grantee_kind = 'user' AND a.grantee_id = $1)
            OR (a.grantee_kind = 'department' AND a.grantee_id IN (SELECT department_id FROM user_departments WHERE user_id = $1)))
         ORDER BY 2",
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::new();
    for (id, name, avatar) in owners {
        let l = level(&state, auth.id, id).await?;
        if l > Level::None {
            out.push(json!({ "id": id, "name": name, "avatar_file_id": avatar, "level": l.name() }));
        }
    }
    Ok(Json(json!({ "items": out })))
}
