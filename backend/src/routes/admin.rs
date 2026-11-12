//! System administration: background job queue, email test, live connection count.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{auth::company_name, clean, Page, PageParams};
use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    jobs::{self, JobRow},
    mail,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/system", get(system))
        .route("/admin/jobs", get(list_jobs))
        .route("/admin/jobs/{id}/retry", post(retry_job))
        .route("/admin/email/test", post(test_email))
}

async fn system(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    let counts: Vec<(String, i64)> = sqlx::query_as("SELECT status, COUNT(*) FROM jobs GROUP BY status")
        .fetch_all(&state.db)
        .await?;
    let counts: serde_json::Map<String, Value> = counts.into_iter().map(|(s, c)| (s, json!(c))).collect();
    Ok(Json(json!({
        "jobs": counts,
        "instance": state.instance_id,
        "worker_enabled": state.config.worker_enabled,
        "live_connections_on_this_instance": state.hub.connection_count(),
        "smtp": {
            "host": state.config.smtp.host,
            "port": state.config.smtp.port,
            "security": state.config.smtp.security,
            "from": state.config.smtp.from,
        },
        "public_url": state.config.public_url,
        "redis": redis_persistence(&state).await,
        "doc_engines": crate::crm::docs::engines(),
        "mail_oauth": {
            "google": state.config.oauth.google.is_some(),
            "microsoft": state.config.oauth.microsoft.is_some(),
            "redirect_uri": format!("{}/api/crm/mailboxes/oauth/callback", state.config.public_url),
        },
    })))
}

/// Whether Redis keeps its data across a restart (append-only file or snapshots). Sessions
/// and sign-in links are in PostgreSQL either way; without persistence a Redis restart only
/// resets rate limits and lockouts and empties caches.
pub async fn redis_persistence(state: &AppState) -> Value {
    let mut redis = state.redis.clone();
    let info: String = redis::cmd("INFO").arg("persistence").query_async(&mut redis).await.unwrap_or_default();
    let field = |k: &str| info.lines().find_map(|l| l.strip_prefix(&format!("{k}:"))).map(|v| v.trim().to_string());
    let aof = field("aof_enabled").as_deref() == Some("1");
    // CONFIG may be switched off (managed Redis); then the snapshot schedule is unknown.
    let save: Option<String> = redis::cmd("CONFIG").arg("GET").arg("save").query_async::<Vec<String>>(&mut redis).await.ok().and_then(|v| v.get(1).cloned());
    let snapshots = save.as_deref().map(|s| !s.trim().is_empty());
    json!({
        "aof": aof,
        "snapshots": snapshots,
        "snapshot_schedule": save,
        "last_save": field("rdb_last_save_time").and_then(|t| t.parse::<i64>().ok()).and_then(|t| chrono::DateTime::from_timestamp(t, 0)),
        "last_save_ok": field("rdb_last_bgsave_status").map(|s| s == "ok"),
        "persistent": aof || snapshots == Some(true),
    })
}

#[derive(Deserialize)]
struct JobQuery {
    status: Option<String>,
    kind: Option<String>,
    #[serde(flatten)]
    page: PageParams,
}

/// Email jobs carry whole message bodies; show only who/what in the list.
fn summarize(mut job: JobRow) -> JobRow {
    if job.kind == jobs::EMAIL_SEND {
        job.payload = json!({ "to": job.payload["to"], "subject": job.payload["subject"] });
    }
    job
}

async fn list_jobs(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<JobQuery>,
) -> AppResult<Json<Page<JobRow>>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    let status = clean(q.status);
    let kind = clean(q.kind);
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR kind = $2)",
    )
    .bind(&status)
    .bind(&kind)
    .fetch_one(&state.db)
    .await?;
    let items: Vec<JobRow> = sqlx::query_as(
        "SELECT id, kind, payload, status, run_at, attempts, max_attempts, locked_by, last_error, created_at, updated_at
         FROM jobs WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR kind = $2)
         ORDER BY created_at DESC LIMIT $3 OFFSET $4",
    )
    .bind(&status)
    .bind(&kind)
    .bind(q.page.limit())
    .bind(q.page.offset())
    .fetch_all(&state.db)
    .await?;
    Ok(Json(Page {
        items: items.into_iter().map(summarize).collect(),
        total,
        limit: q.page.limit(),
        offset: q.page.offset(),
    }))
}

async fn retry_job(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    let n = sqlx::query(
        "UPDATE jobs SET status = 'pending', attempts = 0, run_at = now(), last_error = NULL, updated_at = now()
         WHERE id = $1 AND status = 'failed'",
    )
    .bind(id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::bad("only failed jobs can be retried"));
    }
    state.job_wakeup.notify_one();
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct TestEmail {
    to: Option<String>,
}

async fn test_email(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<TestEmail>,
) -> AppResult<Json<Value>> {
    auth.require(perm::SYSTEM_MANAGE)?;
    let to = match clean(body.to) {
        Some(t) => super::normalize_email(&t)?,
        None => sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
            .bind(auth.id)
            .fetch_one(&state.db)
            .await?,
    };
    let id = jobs::send_email(&state, mail::test_message(&to, &company_name(&state).await)).await?;
    Ok(Json(json!({ "job_id": id, "to": to })))
}
