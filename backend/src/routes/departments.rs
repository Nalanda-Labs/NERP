use axum::{
    extract::{Path, State},
    routing::{delete, get, patch},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::collections::HashMap;
use uuid::Uuid;

use super::{clean, double_option, required};
use crate::{
    audit,
    notify::{notify, NewNotification},
    auth::{permissions as perm, AuthUser, ClientIp},
    error::{is_fk_violation, AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/departments", get(list).post(create))
        .route("/departments/{id}", patch(update).delete(remove))
        .route("/departments/{id}/members", axum::routing::post(add_member))
        .route("/departments/{id}/members/{user_id}", delete(remove_member))
}

#[derive(Serialize, FromRow)]
pub struct Department {
    id: Uuid,
    parent_id: Option<Uuid>,
    name: String,
    description: Option<String>,
    head_user_id: Option<Uuid>,
    head_name: Option<String>,
    head_avatar_file_id: Option<Uuid>,
    sort: i32,
    member_count: i64,
    created_at: DateTime<Utc>,
}

/// Returns the whole structure as a flat list; clients assemble the tree from `parent_id`.
async fn list(State(state): State<AppState>, _auth: AuthUser) -> AppResult<Json<Vec<Department>>> {
    Ok(Json(load_all(&state).await?))
}

async fn load_all(state: &AppState) -> AppResult<Vec<Department>> {
    Ok(sqlx::query_as::<_, Department>(
        "SELECT d.id, d.parent_id, d.name, d.description, d.head_user_id,
                CASE WHEN h.id IS NULL THEN NULL ELSE h.first_name || ' ' || h.last_name END AS head_name,
                h.avatar_file_id AS head_avatar_file_id,
                d.sort,
                (SELECT COUNT(*) FROM user_departments ud JOIN users u ON u.id = ud.user_id
                  WHERE ud.department_id = d.id AND u.is_active) AS member_count,
                d.created_at
         FROM departments d LEFT JOIN users h ON h.id = d.head_user_id
         ORDER BY d.sort, d.name",
    )
    .fetch_all(&state.db)
    .await?)
}

async fn fetch(state: &AppState, id: Uuid) -> AppResult<Department> {
    load_all(state).await?.into_iter().find(|d| d.id == id).ok_or(AppError::NotFound("department"))
}

/// Fails if making `parent` the parent of `id` would create a loop.
async fn check_no_cycle(state: &AppState, id: Uuid, parent: Uuid) -> AppResult<()> {
    let edges: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as("SELECT id, parent_id FROM departments")
        .fetch_all(&state.db)
        .await?;
    let parents: HashMap<Uuid, Option<Uuid>> = edges.into_iter().collect();
    if !parents.contains_key(&parent) {
        return Err(AppError::bad("parent department does not exist"));
    }
    let mut cur = Some(parent);
    while let Some(c) = cur {
        if c == id {
            return Err(AppError::bad("a department cannot be placed inside itself or its sub-departments"));
        }
        cur = parents.get(&c).copied().flatten();
    }
    Ok(())
}

async fn ensure_user_exists(state: &AppState, id: Uuid) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if exists { Ok(()) } else { Err(AppError::bad("selected head does not exist")) }
}

#[derive(Deserialize)]
pub struct CreateDepartment {
    name: String,
    parent_id: Option<Uuid>,
    description: Option<String>,
    head_user_id: Option<Uuid>,
    sort: Option<i32>,
}

async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Json(body): Json<CreateDepartment>,
) -> AppResult<Json<Department>> {
    auth.require(perm::DEPARTMENTS_MANAGE)?;
    let name = required("name", &body.name, 200)?;
    if let Some(p) = body.parent_id {
        check_no_cycle(&state, Uuid::new_v4(), p).await?;
    }
    if let Some(h) = body.head_user_id {
        ensure_user_exists(&state, h).await?;
    }

    let mut tx = state.db.begin().await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO departments (name, parent_id, description, head_user_id, sort)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(&name)
    .bind(body.parent_id)
    .bind(clean(body.description))
    .bind(body.head_user_id)
    .bind(body.sort.unwrap_or(100))
    .fetch_one(&mut *tx)
    .await?;
    // The head of a department is also a member of it.
    if let Some(h) = body.head_user_id {
        sqlx::query("INSERT INTO user_departments (user_id, department_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(h)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    audit::record(&mut *tx, Some(auth.id), "department.create", "department", Some(id.to_string()), json!({ "name": name }), ip.as_deref()).await;
    tx.commit().await?;
    if let Some(h) = body.head_user_id {
        notify_head(&state, h, id, &name, auth.id).await;
    }
    Ok(Json(fetch(&state, id).await?))
}

#[derive(Deserialize)]
pub struct UpdateDepartment {
    name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    parent_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "double_option")]
    description: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    head_user_id: Option<Option<Uuid>>,
    sort: Option<i32>,
}

async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateDepartment>,
) -> AppResult<Json<Department>> {
    auth.require(perm::DEPARTMENTS_MANAGE)?;
    fetch(&state, id).await?;

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("UPDATE departments SET updated_at = now()");
    if let Some(v) = &body.name {
        qb.push(", name = ").push_bind(required("name", v, 200)?);
    }
    if let Some(v) = body.parent_id {
        if let Some(p) = v {
            check_no_cycle(&state, id, p).await?;
        }
        qb.push(", parent_id = ").push_bind(v);
    }
    if let Some(v) = body.description {
        qb.push(", description = ").push_bind(clean(v));
    }
    if let Some(v) = body.head_user_id {
        if let Some(h) = v {
            ensure_user_exists(&state, h).await?;
        }
        qb.push(", head_user_id = ").push_bind(v);
    }
    if let Some(v) = body.sort {
        qb.push(", sort = ").push_bind(v);
    }
    qb.push(" WHERE id = ").push_bind(id);

    let before = fetch(&state, id).await?;
    let mut tx = state.db.begin().await?;
    qb.build().execute(&mut *tx).await?;
    if let Some(Some(h)) = body.head_user_id {
        sqlx::query("INSERT INTO user_departments (user_id, department_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(h)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    audit::record(&mut *tx, Some(auth.id), "department.update", "department", Some(id.to_string()), json!({}), ip.as_deref()).await;
    tx.commit().await?;
    let after = fetch(&state, id).await?;
    if let Some(h) = after.head_user_id.filter(|h| before.head_user_id != Some(*h)) {
        notify_head(&state, h, id, &after.name, auth.id).await;
    }
    Ok(Json(after))
}

async fn remove(
    State(state): State<AppState>,
    auth: AuthUser,
    ClientIp(ip): ClientIp,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    auth.require(perm::DEPARTMENTS_MANAGE)?;
    let dept = fetch(&state, id).await?;
    let has_children: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM departments WHERE parent_id = $1)")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    if has_children {
        return Err(AppError::bad("move or delete the sub-departments first"));
    }
    sqlx::query("DELETE FROM departments WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| {
            if is_fk_violation(&e) {
                AppError::bad("move or delete the sub-departments first")
            } else {
                e.into()
            }
        })?;
    audit::record(&state.db, Some(auth.id), "department.delete", "department", Some(id.to_string()), json!({ "name": dept.name }), ip.as_deref()).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct AddMember {
    user_id: Uuid,
}

async fn add_member(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<AddMember>,
) -> AppResult<Json<serde_json::Value>> {
    auth.require(perm::DEPARTMENTS_MANAGE)?;
    let dept = fetch(&state, id).await?;
    let added = sqlx::query("INSERT INTO user_departments (user_id, department_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(body.user_id)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| if is_fk_violation(&e) { AppError::NotFound("user") } else { e.into() })?
        .rows_affected();
    let _ = crate::im::sync_audiences(&state, Some(body.user_id)).await;
    if added > 0 {
        notify(
            &state,
            &[body.user_id],
            NewNotification {
                kind: "department.added",
                title: format!("You were added to {}", dept.name),
                body: None,
                link: Some(format!("/structure?d={id}")),
                actor: Some(auth.id),
            },
        )
        .await;
    }
    Ok(Json(json!({ "ok": true })))
}

async fn remove_member(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<serde_json::Value>> {
    auth.require(perm::DEPARTMENTS_MANAGE)?;
    sqlx::query("DELETE FROM user_departments WHERE user_id = $1 AND department_id = $2")
        .bind(user_id)
        .bind(id)
        .execute(&state.db)
        .await?;
    let _ = crate::im::sync_audiences(&state, Some(user_id)).await;
    Ok(Json(json!({ "ok": true })))
}

async fn notify_head(state: &AppState, user: Uuid, dept: Uuid, name: &str, actor: Uuid) {
    notify(
        state,
        &[user],
        NewNotification {
            kind: "department.head",
            title: format!("You are now head of {name}"),
            body: None,
            link: Some(format!("/structure?d={dept}")),
            actor: Some(actor),
        },
    )
    .await;
}
