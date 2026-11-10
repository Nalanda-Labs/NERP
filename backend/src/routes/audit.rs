use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, Postgres, QueryBuilder};
use uuid::Uuid;

use super::{clean, Page, PageParams};
use crate::{
    auth::{permissions as perm, AuthUser},
    error::AppResult,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/audit", get(list))
}

#[derive(Serialize, FromRow)]
pub struct AuditEntry {
    id: Uuid,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    action: String,
    entity_type: String,
    entity_id: Option<String>,
    details: Value,
    ip: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct AuditQuery {
    actor_id: Option<Uuid>,
    entity_type: Option<String>,
    entity_id: Option<String>,
    /// Prefix match, e.g. "auth." or "user."
    action: Option<String>,
    #[serde(flatten)]
    page: PageParams,
}

async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(q): Query<AuditQuery>,
) -> AppResult<Json<Page<AuditEntry>>> {
    auth.require(perm::AUDIT_VIEW)?;

    let build = |select: &str| {
        let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(select);
        qb.push(" FROM audit_log a LEFT JOIN users u ON u.id = a.actor_id WHERE TRUE");
        if let Some(v) = q.actor_id {
            qb.push(" AND a.actor_id = ").push_bind(v);
        }
        if let Some(v) = clean(q.entity_type.clone()) {
            qb.push(" AND a.entity_type = ").push_bind(v);
        }
        if let Some(v) = clean(q.entity_id.clone()) {
            qb.push(" AND a.entity_id = ").push_bind(v);
        }
        if let Some(v) = clean(q.action.clone()) {
            qb.push(" AND a.action LIKE ").push_bind(format!("{}%", v.replace('%', "\\%").replace('_', "\\_")));
        }
        qb
    };

    let total: i64 = build("SELECT COUNT(*)").build_query_scalar().fetch_one(&state.db).await?;
    let mut qb = build(
        "SELECT a.id, a.actor_id,
                CASE WHEN u.id IS NULL THEN NULL ELSE u.first_name || ' ' || u.last_name END AS actor_name,
                a.action, a.entity_type, a.entity_id, a.details, a.ip, a.created_at",
    );
    qb.push(" ORDER BY a.created_at DESC LIMIT ")
        .push_bind(q.page.limit())
        .push(" OFFSET ")
        .push_bind(q.page.offset());
    let items: Vec<AuditEntry> = qb.build_query_as().fetch_all(&state.db).await?;
    Ok(Json(Page { items, total, limit: q.page.limit(), offset: q.page.offset() }))
}
