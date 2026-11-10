//! First-run setup: creates the company, its root department and the first administrator.

use axum::{extract::State, routing::get, Json, Router};
use redis::AsyncCommands;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{auth::start_session, normalize_email, required};
use crate::{
    audit,
    auth::{password, tokens::ClientMeta},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/setup", get(status).post(run_setup))
}

async fn needs_setup(state: &AppState) -> AppResult<bool> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE NOT is_system)")
        .fetch_one(&state.db)
        .await?;
    Ok(!exists)
}

/// Public: tells the login page whether to show setup, and which company this is.
async fn status(State(state): State<AppState>) -> AppResult<Json<Value>> {
    let company: Option<Value> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'company_name'")
            .fetch_optional(&state.db)
            .await?;
    Ok(Json(json!({
        "needs_setup": needs_setup(&state).await?,
        "company_name": company.unwrap_or(Value::Null),
    })))
}

#[derive(Deserialize)]
pub struct SetupRequest {
    company_name: String,
    email: String,
    password: String,
    first_name: String,
    last_name: String,
}

async fn run_setup(
    State(state): State<AppState>,
    meta: ClientMeta,
    Json(body): Json<SetupRequest>,
) -> AppResult<axum::response::Response> {
    let company = required("company name", &body.company_name, 200)?;
    let email = normalize_email(&body.email)?;
    password::validate_password(&body.password)?;
    let first_name = required("first name", &body.first_name, 100)?;
    let last_name = required("last name", &body.last_name, 100)?;

    // A short Redis lock stops two simultaneous setup requests from both succeeding.
    let mut redis = state.redis.clone();
    let lock_key = "nerp:setup_lock";
    let locked: bool = redis::cmd("SET")
        .arg(lock_key)
        .arg(1)
        .arg("NX")
        .arg("EX")
        .arg(60)
        .query_async::<Option<String>>(&mut redis)
        .await?
        .is_some();
    if !locked {
        return Err(AppError::Conflict("setup is already in progress".into()));
    }
    let result = do_setup(&state, company, email, body.password, first_name, last_name, meta.ip.clone()).await;
    let _: () = redis.del(lock_key).await?;
    let user_id = result?;
    start_session(&state, user_id, &meta).await
}

async fn do_setup(
    state: &AppState,
    company: String,
    email: String,
    password_plain: String,
    first_name: String,
    last_name: String,
    ip: Option<String>,
) -> AppResult<Uuid> {
    if !needs_setup(state).await? {
        return Err(AppError::Conflict("NERP has already been set up".into()));
    }
    let hash = password::hash_password(password_plain).await?;

    let mut tx = state.db.begin().await?;
    let user_id: Uuid = sqlx::query_scalar(
        "INSERT INTO users (email, password_hash, first_name, last_name, position, last_login_at)
         VALUES ($1, $2, $3, $4, 'Administrator', now()) RETURNING id",
    )
    .bind(&email)
    .bind(hash)
    .bind(&first_name)
    .bind(&last_name)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query("INSERT INTO user_roles (user_id, role_id) SELECT $1, id FROM roles WHERE code IN ('admin', 'employee')")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    let dept_id: Uuid = sqlx::query_scalar(
        "INSERT INTO departments (name, head_user_id, sort) VALUES ($1, $2, 0) RETURNING id",
    )
    .bind(&company)
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO user_departments (user_id, department_id) VALUES ($1, $2)")
        .bind(user_id)
        .bind(dept_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('company_name', $1)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(Value::String(company.clone()))
    .execute(&mut *tx)
    .await?;

    audit::record(&mut *tx, Some(user_id), "setup.complete", "company", None, json!({ "company": company }), ip.as_deref()).await;
    tx.commit().await?;
    Ok(user_id)
}
