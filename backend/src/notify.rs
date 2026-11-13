//! In-app notifications. Any module calls `notify()`; the notification is stored and
//! pushed to the recipient's open browser tabs in real time.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    realtime,
    routes::{Page, PageParams},
    state::AppState,
};

#[derive(Serialize, FromRow, Clone)]
pub struct Notification {
    pub id: Uuid,
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    pub link: Option<String>,
    pub actor_id: Option<Uuid>,
    pub actor_name: Option<String>,
    pub actor_avatar_file_id: Option<Uuid>,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub struct NewNotification<'a> {
    pub kind: &'a str,
    pub title: String,
    pub body: Option<String>,
    pub link: Option<String>,
    pub actor: Option<Uuid>,
}

/// Sends a notification to each recipient. The actor never notifies themselves.
/// Errors are logged and swallowed: a notification must never fail the action that caused it.
pub async fn notify(state: &AppState, recipients: &[Uuid], n: NewNotification<'_>) {
    for &user in recipients {
        if Some(user) == n.actor {
            continue;
        }
        let row: Result<Notification, sqlx::Error> = sqlx::query_as(
            "WITH ins AS (
                 INSERT INTO notifications (user_id, kind, title, body, link, actor_id)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 RETURNING id, kind, title, body, link, actor_id, read_at, created_at)
             SELECT ins.id, ins.kind, ins.title, ins.body, ins.link, ins.actor_id,
                    CASE WHEN a.id IS NULL THEN NULL ELSE a.first_name || ' ' || a.last_name END AS actor_name,
                    a.avatar_file_id AS actor_avatar_file_id, ins.read_at, ins.created_at
             FROM ins LEFT JOIN users a ON a.id = ins.actor_id",
        )
        .bind(user)
        .bind(n.kind)
        .bind(&n.title)
        .bind(&n.body)
        .bind(&n.link)
        .bind(n.actor)
        .fetch_one(&state.db)
        .await;
        match row {
            Ok(notification) => {
                realtime::publish(state, Some(vec![user]), "notification", serde_json::to_value(notification).unwrap()).await
            }
            Err(e) => tracing::warn!(error = ?e, kind = n.kind, "could not store notification"),
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/notifications", get(list))
        .route("/notifications/unread-count", get(unread_count))
        .route("/notifications/read-all", post(read_all))
        .route("/notifications/{id}/read", post(read_one))
}

#[derive(Deserialize)]
struct ListQuery {
    unread: Option<String>,
    #[serde(flatten)]
    page: PageParams,
}

async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Page<Notification>>> {
    let unread_only = q.unread.as_deref() == Some("true");
    let total: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM notifications WHERE user_id = $1 AND (NOT $2 OR read_at IS NULL)"#, auth.id, unread_only)
    .fetch_one(&state.db)
    .await?;
    let items: Vec<Notification> = sqlx::query_as!(Notification, r#"SELECT n.id, n.kind, n.title, n.body AS "body?", n.link AS "link?", n.actor_id AS "actor_id?",
                CASE WHEN a.id IS NULL THEN NULL ELSE a.first_name || ' ' || a.last_name END AS "actor_name?",
                a.avatar_file_id AS "actor_avatar_file_id?", n.read_at AS "read_at?", n.created_at
         FROM notifications n LEFT JOIN users a ON a.id = n.actor_id
         WHERE n.user_id = $1 AND (NOT $2 OR n.read_at IS NULL)
         ORDER BY n.created_at DESC LIMIT $3 OFFSET $4"#, auth.id, unread_only, q.page.limit(), q.page.offset())
    .fetch_all(&state.db)
    .await?;
    Ok(Json(Page { items, total, limit: q.page.limit(), offset: q.page.offset() }))
}

async fn count_unread(state: &AppState, user: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM notifications WHERE user_id = $1 AND read_at IS NULL"#, user)
        .fetch_one(&state.db)
        .await?)
}

async fn unread_count(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    Ok(Json(json!({ "count": count_unread(&state, auth.id).await? })))
}

/// Tells the user's other tabs to refresh their badge.
async fn broadcast_read(state: &AppState, user: Uuid, ids: Option<Vec<Uuid>>) -> AppResult<Value> {
    let count = count_unread(state, user).await?;
    let data = json!({ "ids": ids, "unread": count });
    realtime::publish(state, Some(vec![user]), "notifications.read", data.clone()).await;
    Ok(data)
}

async fn read_one(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let n = sqlx::query!("UPDATE notifications SET read_at = COALESCE(read_at, now()) WHERE id = $1 AND user_id = $2", id, auth.id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("notification"));
    }
    Ok(Json(broadcast_read(&state, auth.id, Some(vec![id])).await?))
}

async fn read_all(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    sqlx::query!("UPDATE notifications SET read_at = now() WHERE user_id = $1 AND read_at IS NULL", auth.id)
        .execute(&state.db)
        .await?;
    Ok(Json(broadcast_read(&state, auth.id, None).await?))
}
