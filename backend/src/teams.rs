//! Cross-functional teams: people from any department working together under a lead, beside
//! the line structure of departments. A person can be in several teams, each with a role in it
//! ("Design", "QA"…). Teams show on the company structure and on profiles, and each has a chat
//! whose members follow the team.
//!
//! Those who manage the structure create, change and remove teams; a team's lead manages its
//! members.
use axum::{
    extract::{Path, State},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    audit,
    auth::{permissions as perm, AuthUser, ClientIp},
    error::{AppError, AppResult},
    im::Link,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/teams", get(list).post(create))
        .route("/teams/{id}", get(one).patch(update).delete(remove))
        .route("/teams/{id}/members", put(set_members))
        .route("/teams/{id}/chat", post(chat))
        .route("/users/{id}/teams", get(of_user))
}

#[derive(Deserialize, Clone)]
struct Member {
    user_id: Uuid,
    #[serde(default)]
    role: Option<String>,
}

#[derive(Deserialize)]
struct TeamInput {
    name: String,
    description: Option<String>,
    color: Option<String>,
    lead_id: Option<Uuid>,
    #[serde(default)]
    members: Option<Vec<Member>>,
}

fn check(b: &TeamInput) -> AppResult<String> {
    let name = b.name.trim().to_string();
    if name.is_empty() || name.len() > 200 || b.description.as_deref().is_some_and(|d| d.len() > 5000) {
        return Err(AppError::bad("give the team a name (and a description of reasonable length)"));
    }
    if b.color.as_deref().is_some_and(|c| !(c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|x| x.is_ascii_hexdigit()))) {
        return Err(AppError::bad("the colour is #rrggbb"));
    }
    Ok(name)
}

async fn employees_exist(state: &AppState, ids: &[Uuid]) -> AppResult<()> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ANY($1) AND NOT is_guest AND NOT is_system").bind(ids).fetch_one(&state.db).await?;
    if n as usize != ids.len() {
        return Err(AppError::bad("team members are employees"));
    }
    Ok(())
}

async fn team_lead(state: &AppState, id: Uuid) -> AppResult<Option<Uuid>> {
    let row: Option<Option<Uuid>> = sqlx::query_scalar("SELECT lead_id FROM teams WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
    row.ok_or(AppError::NotFound("team"))
}

async fn list(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let rows: Vec<(Uuid, String, Option<String>, Option<String>, Option<Uuid>, Option<String>, Option<Uuid>, i64, bool)> = sqlx::query_as(
        "SELECT t.id, t.name, t.description, t.color, t.lead_id, trim(l.first_name || ' ' || l.last_name), l.avatar_file_id,
                (SELECT COUNT(*) FROM team_members m JOIN users u ON u.id = m.user_id WHERE m.team_id = t.id AND u.is_active),
                EXISTS (SELECT 1 FROM team_members m WHERE m.team_id = t.id AND m.user_id = $1) OR t.lead_id = $1
         FROM teams t LEFT JOIN users l ON l.id = t.lead_id ORDER BY lower(t.name)",
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "items": rows.into_iter().map(|(id, n, d, c, lid, lname, lav, count, mine)| json!({
            "id": id, "name": n, "description": d, "color": c, "lead": lid.map(|l| json!({ "id": l, "name": lname, "avatar_file_id": lav })), "members": count, "mine": mine,
        })).collect::<Vec<_>>(),
        "can_manage": auth.can(perm::DEPARTMENTS_MANAGE),
    })))
}

async fn one(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let t: Option<(String, Option<String>, Option<String>, Option<Uuid>, DateTime<Utc>)> =
        sqlx::query_as("SELECT name, description, color, lead_id, created_at FROM teams WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
    let (name, desc, color, lead, created) = t.ok_or(AppError::NotFound("team"))?;
    let members: Vec<(Uuid, String, Option<Uuid>, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT u.id, trim(u.first_name || ' ' || u.last_name), u.avatar_file_id, u.position, m.role,
                (SELECT string_agg(d.name, ', ' ORDER BY d.name) FROM user_departments ud JOIN departments d ON d.id = ud.department_id WHERE ud.user_id = u.id)
         FROM team_members m JOIN users u ON u.id = m.user_id WHERE m.team_id = $1 AND u.is_active
         ORDER BY u.id = $2 DESC, lower(u.first_name), lower(u.last_name)",
    )
    .bind(id)
    .bind(lead)
    .fetch_all(&state.db)
    .await?;
    let chat: Option<Uuid> = sqlx::query_scalar("SELECT id FROM im_chats WHERE team_id = $1").bind(id).fetch_optional(&state.db).await?;
    let in_team = lead == Some(auth.id) || members.iter().any(|m| m.0 == auth.id);
    Ok(Json(json!({
        "id": id, "name": name, "description": desc, "color": color, "lead_id": lead, "created_at": created,
        "members": members.into_iter().map(|(uid, n, av, pos, role, depts)| json!({ "id": uid, "name": n, "avatar_file_id": av, "position": pos, "role": role, "departments": depts, "lead": Some(uid) == lead })).collect::<Vec<_>>(),
        "chat_id": if in_team { chat } else { None }, "in_team": in_team,
        "can_edit": auth.can(perm::DEPARTMENTS_MANAGE), "can_edit_members": auth.can(perm::DEPARTMENTS_MANAGE) || lead == Some(auth.id),
    })))
}

async fn write_members(tx: &mut sqlx::PgConnection, id: Uuid, lead: Option<Uuid>, members: &[Member]) -> AppResult<()> {
    sqlx::query("DELETE FROM team_members WHERE team_id = $1").bind(id).execute(&mut *tx).await?;
    let mut ids: Vec<Uuid> = vec![];
    let mut roles: Vec<Option<String>> = vec![];
    for m in members {
        if ids.contains(&m.user_id) {
            continue;
        }
        let role = m.role.as_deref().map(str::trim).filter(|r| !r.is_empty()).map(|r| r.chars().take(100).collect::<String>());
        ids.push(m.user_id);
        roles.push(role);
    }
    // The lead is a member too.
    if let Some(l) = lead.filter(|l| !ids.contains(l)) {
        ids.push(l);
        roles.push(Some("Lead".into()));
    }
    sqlx::query("INSERT INTO team_members (team_id, user_id, role) SELECT $1, u, r FROM unnest($2::uuid[], $3::text[]) AS x(u, r)")
        .bind(id)
        .bind(&ids)
        .bind(&roles)
        .execute(&mut *tx)
        .await?;
    Ok(())
}

async fn create(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Json(b): Json<TeamInput>) -> AppResult<Json<Value>> {
    auth.require(perm::DEPARTMENTS_MANAGE)?;
    let name = check(&b)?;
    let members = b.members.clone().unwrap_or_default();
    if members.len() > 500 {
        return Err(AppError::bad("at most 500 members"));
    }
    employees_exist(&state, &members.iter().map(|m| m.user_id).chain(b.lead_id).collect::<Vec<_>>()).await?;
    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO teams (name, description, color, lead_id, created_by) VALUES ($1, NULLIF(trim($2), ''), $3, $4, $5) RETURNING id")
        .bind(&name)
        .bind(&b.description)
        .bind(&b.color)
        .bind(b.lead_id)
        .bind(auth.id)
        .fetch_one(&mut *tx)
        .await?;
    write_members(&mut tx, id, b.lead_id, &members).await?;
    tx.commit().await?;
    audit::record(&state.db, Some(auth.id), "team.create", "team", Some(id.to_string()), json!({ "name": name }), ip.as_deref()).await;
    one(State(state), auth, Path(id)).await
}

async fn update(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(id): Path<Uuid>, Json(b): Json<TeamInput>) -> AppResult<Json<Value>> {
    auth.require(perm::DEPARTMENTS_MANAGE)?;
    team_lead(&state, id).await?;
    let name = check(&b)?;
    if let Some(l) = b.lead_id {
        employees_exist(&state, &[l]).await?;
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE teams SET name = $2, description = NULLIF(trim($3), ''), color = $4, lead_id = $5, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(&name)
        .bind(&b.description)
        .bind(&b.color)
        .bind(b.lead_id)
        .execute(&mut *tx)
        .await?;
    if let Some(m) = &b.members {
        employees_exist(&state, &m.iter().map(|m| m.user_id).collect::<Vec<_>>()).await?;
        write_members(&mut tx, id, b.lead_id, m).await?;
    } else if let Some(l) = b.lead_id {
        sqlx::query("INSERT INTO team_members (team_id, user_id, role) VALUES ($1, $2, 'Lead') ON CONFLICT DO NOTHING").bind(id).bind(l).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    crate::im::sync_linked_chat(&state, Link::Team(id)).await?;
    audit::record(&state.db, Some(auth.id), "team.update", "team", Some(id.to_string()), json!({ "name": name }), ip.as_deref()).await;
    one(State(state), auth, Path(id)).await
}

#[derive(Deserialize)]
struct MembersInput {
    members: Vec<Member>,
}

/// The team's people (structure managers, or the team's lead).
async fn set_members(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(id): Path<Uuid>, Json(b): Json<MembersInput>) -> AppResult<Json<Value>> {
    let lead = team_lead(&state, id).await?;
    if !auth.can(perm::DEPARTMENTS_MANAGE) && lead != Some(auth.id) {
        return Err(AppError::Forbidden);
    }
    if b.members.len() > 500 {
        return Err(AppError::bad("at most 500 members"));
    }
    employees_exist(&state, &b.members.iter().map(|m| m.user_id).collect::<Vec<_>>()).await?;
    let mut tx = state.db.begin().await?;
    write_members(&mut tx, id, lead, &b.members).await?;
    tx.commit().await?;
    crate::im::sync_linked_chat(&state, Link::Team(id)).await?;
    audit::record(&state.db, Some(auth.id), "team.members", "team", Some(id.to_string()), json!({ "count": b.members.len() }), ip.as_deref()).await;
    one(State(state), auth, Path(id)).await
}

async fn remove(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    auth.require(perm::DEPARTMENTS_MANAGE)?;
    team_lead(&state, id).await?;
    let gone = crate::im::linked_chat_people(&state, Link::Team(id)).await?;
    sqlx::query("DELETE FROM teams WHERE id = $1").bind(id).execute(&state.db).await?;
    crate::im::linked_chat_gone(&state, gone).await;
    audit::record(&state.db, Some(auth.id), "team.delete", "team", Some(id.to_string()), json!({}), ip.as_deref()).await;
    Ok(Json(json!({ "ok": true })))
}

/// The team's chat (made on first use), for its members.
async fn chat(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    team_lead(&state, id).await?;
    if !Link::Team(id).has(&state, auth.id).await? {
        return Err(AppError::Forbidden);
    }
    let chat = crate::im::linked_chat(&state, Link::Team(id)).await?;
    Ok(Json(json!({ "chat_id": chat })))
}

async fn of_user(State(state): State<AppState>, _auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let employee: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1 AND NOT is_guest AND NOT is_system)").bind(id).fetch_one(&state.db).await?;
    if !employee {
        return Err(AppError::NotFound("employee"));
    }
    let rows: Vec<(Uuid, String, Option<String>, Option<String>, bool)> = sqlx::query_as(
        "SELECT t.id, t.name, t.color, m.role, t.lead_id = $1 FROM team_members m JOIN teams t ON t.id = m.team_id WHERE m.user_id = $1 ORDER BY lower(t.name)",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(tid, n, c, r, lead)| json!({ "id": tid, "name": n, "color": c, "role": r, "lead": lead })).collect::<Vec<_>>() })))
}
