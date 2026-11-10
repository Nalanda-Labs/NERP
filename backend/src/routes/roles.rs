use axum::{
    extract::{Path, State},
    routing::{get, patch},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use std::collections::HashMap;
use uuid::Uuid;

use super::{clean, double_option, required};
use crate::{
    audit,
    auth::{invalidate_authz, permissions as perm, AuthUser, ClientIp},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/permissions", get(catalog))
        .route("/roles", get(list).post(create))
        .route("/roles/{id}", patch(update).delete(remove))
}

async fn catalog(_auth: AuthUser) -> Json<&'static [perm::PermissionInfo]> {
    Json(perm::CATALOG)
}

#[derive(FromRow)]
struct RoleRow {
    id: Uuid,
    code: String,
    name: String,
    description: Option<String>,
    is_system: bool,
    user_count: i64,
}

#[derive(Serialize)]
pub struct Role {
    id: Uuid,
    code: String,
    name: String,
    description: Option<String>,
    is_system: bool,
    user_count: i64,
    permissions: Vec<String>,
}

async fn load_roles(state: &AppState, only: Option<Uuid>) -> AppResult<Vec<Role>> {
    let rows: Vec<RoleRow> = sqlx::query_as(
        "SELECT r.id, r.code, r.name, r.description, r.is_system,
                (SELECT COUNT(*) FROM user_roles ur JOIN users u ON u.id = ur.user_id
                  WHERE ur.role_id = r.id AND u.is_active) AS user_count
         FROM roles r WHERE ($1::uuid IS NULL OR r.id = $1)
         ORDER BY r.is_system DESC, r.name",
    )
    .bind(only)
    .fetch_all(&state.db)
    .await?;
    let perms: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT role_id, permission FROM role_permissions ORDER BY permission")
            .fetch_all(&state.db)
            .await?;
    let mut by_role: HashMap<Uuid, Vec<String>> = HashMap::new();
    for (r, p) in perms {
        by_role.entry(r).or_default().push(p);
    }
    Ok(rows
        .into_iter()
        .map(|r| Role {
            permissions: by_role.remove(&r.id).unwrap_or_default(),
            id: r.id,
            code: r.code,
            name: r.name,
            description: r.description,
            is_system: r.is_system,
            user_count: r.user_count,
        })
        .collect())
}

async fn fetch(state: &AppState, id: Uuid) -> AppResult<Role> {
    load_roles(state, Some(id)).await?.pop().ok_or(AppError::NotFound("role"))
}

/// Any signed-in user may list roles (needed to display them on profiles).
async fn list(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Vec<Role>>> {
    Ok(Json(load_roles(&state, None).await?))
}

fn validate_permissions(perms: &[String]) -> AppResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for p in perms {
        if !perm::is_known(p) {
            return Err(AppError::bad(format!("unknown permission: {p}")));
        }
        if !out.contains(p) {
            out.push(p.clone());
        }
    }
    Ok(out)
}

fn validate_code(code: &str) -> AppResult<String> {
    let code = code.trim().to_lowercase();
    let ok = (2..=50).contains(&code.len())
        && code.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && code.chars().next().is_some_and(|c| c.is_ascii_lowercase());
    if !ok {
        return Err(AppError::bad(
            "code must be 2-50 characters: lowercase letters, digits, '-' or '_', starting with a letter",
        ));
    }
    Ok(code)
}

#[derive(Deserialize)]
pub struct CreateRole {
    code: String,
    name: String,
    description: Option<String>,
    #[serde(default)]
    permissions: Vec<String>,
}

async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Json(body): Json<CreateRole>,
) -> AppResult<Json<Role>> {
    auth.require(perm::ROLES_MANAGE)?;
    let code = validate_code(&body.code)?;
    let name = required("name", &body.name, 100)?;
    let perms = validate_permissions(&body.permissions)?;

    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO roles (code, name, description) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(&code)
    .bind(&name)
    .bind(clean(body.description))
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("a role with this code already exists".into())
        } else {
            e.into()
        }
    })?;
    sqlx::query("INSERT INTO role_permissions (role_id, permission) SELECT $1, UNNEST($2::text[])")
        .bind(id)
        .bind(&perms)
        .execute(&mut *tx)
        .await?;
    audit::record(&mut *tx, Some(auth.id), "role.create", "role", Some(id.to_string()), json!({ "code": code, "permissions": perms }), ip.as_deref()).await;
    tx.commit().await?;
    Ok(Json(fetch(&state, id).await?))
}

#[derive(Deserialize)]
pub struct UpdateRole {
    name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    description: Option<Option<String>>,
    permissions: Option<Vec<String>>,
}

async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateRole>,
) -> AppResult<Json<Role>> {
    auth.require(perm::ROLES_MANAGE)?;
    let role = fetch(&state, id).await?;
    if role.code == "admin" && body.permissions.is_some() {
        return Err(AppError::bad("the Administrator role always has every permission"));
    }

    let mut tx = state.db.begin().await?;
    if let Some(n) = &body.name {
        sqlx::query("UPDATE roles SET name = $1, updated_at = now() WHERE id = $2")
            .bind(required("name", n, 100)?)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(d) = body.description {
        sqlx::query("UPDATE roles SET description = $1, updated_at = now() WHERE id = $2")
            .bind(clean(d))
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(p) = &body.permissions {
        let perms = validate_permissions(p)?;
        sqlx::query("DELETE FROM role_permissions WHERE role_id = $1").bind(id).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO role_permissions (role_id, permission) SELECT $1, UNNEST($2::text[])")
            .bind(id)
            .bind(&perms)
            .execute(&mut *tx)
            .await?;
        audit::record(&mut *tx, Some(auth.id), "role.permissions", "role", Some(id.to_string()), json!({ "before": role.permissions, "after": perms }), ip.as_deref()).await;
    }
    tx.commit().await?;

    if body.permissions.is_some() {
        invalidate_role_members(&state, id).await?;
    }
    Ok(Json(fetch(&state, id).await?))
}

async fn invalidate_role_members(state: &AppState, role_id: Uuid) -> AppResult<()> {
    let members: Vec<Uuid> = sqlx::query_scalar("SELECT user_id FROM user_roles WHERE role_id = $1")
        .bind(role_id)
        .fetch_all(&state.db)
        .await?;
    invalidate_authz(state, &members).await
}

async fn remove(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    auth.require(perm::ROLES_MANAGE)?;
    let role = fetch(&state, id).await?;
    if role.is_system {
        return Err(AppError::bad("built-in roles cannot be deleted"));
    }
    let members: Vec<Uuid> = sqlx::query_scalar("SELECT user_id FROM user_roles WHERE role_id = $1")
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    sqlx::query("DELETE FROM roles WHERE id = $1").bind(id).execute(&state.db).await?;
    invalidate_authz(&state, &members).await?;
    audit::record(&state.db, Some(auth.id), "role.delete", "role", Some(id.to_string()), json!({ "code": role.code }), ip.as_deref()).await;
    Ok(Json(json!({ "ok": true })))
}
