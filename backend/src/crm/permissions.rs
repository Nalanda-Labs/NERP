//! Editing the CRM access-rights matrix: role × record type × action → scope.

use axum::{
    extract::{Path, State},
    routing::{get, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::access::{is_crm_admin, Scope, ACTIONS};
use crate::{
    audit,
    auth::{invalidate_authz, AuthUser, ClientIp},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/permissions", get(matrix))
        .route("/crm/permissions/{role_id}", put(save))
}

async fn matrix(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    if !is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    let roles: Vec<(Uuid, String, String, bool)> = sqlx::query_as(
        "SELECT r.id, r.code, r.name, EXISTS (SELECT 1 FROM role_permissions rp WHERE rp.role_id = r.id AND rp.permission IN ('*', 'crm.admin'))
         FROM roles r ORDER BY r.is_system DESC, r.name",
    )
    .fetch_all(&state.db)
    .await?;
    let entries: Vec<(Uuid, i16, String, String)> = sqlx::query_as("SELECT role_id, entity_type, action, scope FROM crm_role_permissions")
        .fetch_all(&state.db)
        .await?;
    let mut m: Map<String, Value> = Map::new();
    for (role, t, action, scope) in entries {
        let r = m.entry(role.to_string()).or_insert_with(|| json!({}));
        let ty = r.as_object_mut().unwrap().entry(t.to_string()).or_insert_with(|| json!({}));
        ty.as_object_mut().unwrap().insert(action, json!(scope));
    }
    Ok(Json(json!({
        "roles": roles.iter().map(|(id, code, name, full)| json!({ "id": id, "code": code, "name": name, "full_access": full })).collect::<Vec<_>>(),
        "actions": ACTIONS,
        "scopes": ["none", "own", "department", "subdepartments", "all"],
        "matrix": m,
    })))
}

#[derive(Deserialize)]
struct Entry {
    entity_type: i16,
    action: String,
    scope: String,
}

#[derive(Deserialize)]
struct Save {
    entries: Vec<Entry>,
}

/// Replaces a role's CRM rights.
async fn save(State(state): State<AppState>, auth: AuthUser, ClientIp(ip): ClientIp, Path(role): Path<Uuid>, Json(b): Json<Save>) -> AppResult<Json<Value>> {
    if !is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    for e in &b.entries {
        if !ACTIONS.contains(&e.action.as_str()) || Scope::parse(&e.scope).is_none() {
            return Err(AppError::bad(format!("invalid entry: {} {}", e.action, e.scope)));
        }
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM crm_role_permissions WHERE role_id = $1").bind(role).execute(&mut *tx).await?;
    for e in &b.entries {
        sqlx::query("INSERT INTO crm_role_permissions (role_id, entity_type, action, scope) VALUES ($1, $2, $3, $4)")
            .bind(role)
            .bind(e.entity_type)
            .bind(&e.action)
            .bind(&e.scope)
            .execute(&mut *tx)
            .await
            .map_err(|e| if crate::error::is_fk_violation(&e) { AppError::bad("unknown role or record type") } else { e.into() })?;
    }
    audit::record(&mut *tx, Some(auth.id), "crm.permissions", "role", Some(role.to_string()), json!({ "entries": b.entries.len() }), ip.as_deref()).await;
    tx.commit().await?;
    // CRM scopes are read per request, but drop cached auth anyway for consistency.
    let members: Vec<Uuid> = sqlx::query_scalar("SELECT user_id FROM user_roles WHERE role_id = $1").bind(role).fetch_all(&state.db).await?;
    invalidate_authz(&state, &members).await?;
    matrix(State(state), auth).await
}
