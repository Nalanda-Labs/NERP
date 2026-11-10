//! The automation report (CRM admins): every robot and schedule run of a pipeline with
//! totals per status and per robot, a retry for failed runs, and how long records spend in
//! each stage.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use uuid::Uuid;

use super::{enqueue_run, require_admin};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/automation/report", get(report))
        .route("/crm/automation/stage-times", get(stage_times))
        .route("/crm/robot-runs/{id}/retry", post(retry))
}

const STATUSES: &[&str] = &["pending", "scheduled", "waiting", "running", "done", "failed", "skipped"];

#[derive(Deserialize)]
struct ReportQuery {
    category_id: Uuid,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    status: Option<String>,
    /// A robot's or a schedule's id.
    source: Option<Uuid>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Serialize, FromRow)]
struct RunRow {
    id: Uuid,
    robot_id: Option<Uuid>,
    schedule_id: Option<Uuid>,
    robot_name: String,
    action: Option<String>,
    status: String,
    run_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    wait_until: Option<DateTime<Utc>>,
    result: Option<String>,
    attempts: i32,
    created_at: DateTime<Utc>,
    item_id: Uuid,
    item_title: String,
    item_number: i64,
    type_code: String,
}

#[derive(Serialize, FromRow)]
struct BySource {
    robot_id: Option<Uuid>,
    schedule_id: Option<Uuid>,
    name: String,
    total: i64,
    done: i64,
    failed: i64,
    skipped: i64,
    open: i64,
}

fn period(q: &ReportQuery) -> AppResult<(DateTime<Utc>, DateTime<Utc>)> {
    let to = q.to.unwrap_or_else(|| Utc::now() + Duration::days(1));
    let from = q.from.unwrap_or(to - Duration::days(30));
    if to <= from {
        return Err(AppError::bad("the end of the period must be after its start"));
    }
    if to - from > Duration::days(366) {
        return Err(AppError::bad("choose a period of at most a year"));
    }
    Ok((from, to))
}

fn push_filters(qb: &mut QueryBuilder<Postgres>, q: &ReportQuery, from: DateTime<Utc>, to: DateTime<Utc>, with_status: bool) {
    qb.push(" WHERE r.category_id = ").push_bind(q.category_id);
    qb.push(" AND r.created_at >= ").push_bind(from).push(" AND r.created_at < ").push_bind(to);
    if let Some(s) = q.source {
        qb.push(" AND (r.robot_id = ").push_bind(s).push(" OR r.schedule_id = ").push_bind(s).push(")");
    }
    if with_status && let Some(st) = q.status.as_deref().filter(|s| !s.is_empty()) {
        if st == "open" {
            qb.push(" AND r.status IN ('pending', 'scheduled', 'waiting', 'running')");
        } else {
            qb.push(" AND r.status = ").push_bind(st.to_string());
        }
    }
    if let Some(text) = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let like = format!("%{}%", text.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
        qb.push(" AND (i.title ILIKE ").push_bind(like.clone()).push(" OR r.robot_name ILIKE ").push_bind(like.clone()).push(" OR r.result ILIKE ").push_bind(like).push(")");
    }
}

async fn report(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ReportQuery>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    if let Some(s) = q.status.as_deref().filter(|s| !s.is_empty() && *s != "open")
        && !STATUSES.contains(&s) {
            return Err(AppError::bad("unknown status"));
        }
    let (from, to) = period(&q)?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).clamp(0, 100_000);
    const FROM: &str = " FROM crm_robot_runs r JOIN crm_items i ON i.id = r.item_id JOIN crm_entity_types t ON t.id = i.entity_type";

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT r.id, r.robot_id, r.schedule_id, r.robot_name, r.action, r.status, r.run_at, r.finished_at, r.wait_until, r.result, r.attempts, r.created_at,
                r.item_id, i.title AS item_title, i.number AS item_number, t.code AS type_code",
    );
    qb.push(FROM);
    push_filters(&mut qb, &q, from, to, true);
    qb.push(" ORDER BY r.created_at DESC, r.id DESC LIMIT ").push_bind(limit).push(" OFFSET ").push_bind(offset);
    let items: Vec<RunRow> = qb.build_query_as().fetch_all(&state.db).await?;

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT COUNT(*)");
    qb.push(FROM);
    push_filters(&mut qb, &q, from, to, true);
    let total: i64 = qb.build_query_scalar().fetch_one(&state.db).await?;

    // Totals ignore the status filter, so the status chips always show every count.
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT r.status, COUNT(*)::int8");
    qb.push(FROM);
    push_filters(&mut qb, &q, from, to, false);
    qb.push(" GROUP BY r.status");
    let counts: Vec<(String, i64)> = qb.build_query_as().fetch_all(&state.db).await?;
    let counts: serde_json::Map<String, Value> = STATUSES.iter().map(|s| (s.to_string(), json!(counts.iter().find(|c| c.0 == *s).map(|c| c.1).unwrap_or(0)))).collect();

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT r.robot_id, r.schedule_id, MAX(r.robot_name) AS name, COUNT(*)::int8 AS total,
                COUNT(*) FILTER (WHERE r.status = 'done')::int8 AS done,
                COUNT(*) FILTER (WHERE r.status = 'failed')::int8 AS failed,
                COUNT(*) FILTER (WHERE r.status = 'skipped')::int8 AS skipped,
                COUNT(*) FILTER (WHERE r.status IN ('pending', 'scheduled', 'waiting', 'running'))::int8 AS open",
    );
    qb.push(FROM);
    push_filters(&mut qb, &q, from, to, false);
    qb.push(" GROUP BY r.robot_id, r.schedule_id ORDER BY failed DESC, total DESC LIMIT 100");
    let by_source: Vec<BySource> = qb.build_query_as().fetch_all(&state.db).await?;

    Ok(Json(json!({
        "period": { "from": from, "to": to },
        "counts": counts,
        "by_source": by_source,
        "items": items,
        "total": total,
    })))
}

/// Runs a failed robot again, as a new run. It still checks the robot's stage and
/// conditions, but does not wait again.
async fn retry(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let old: Option<(String, Option<Uuid>, Option<Uuid>)> = sqlx::query_as("SELECT status, robot_id, schedule_id FROM crm_robot_runs WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
    let Some((status, robot, schedule)) = old else { return Err(AppError::NotFound("run")) };
    if status != "failed" {
        return Err(AppError::bad("only failed runs can be retried"));
    }
    if robot.is_none() && schedule.is_none() {
        return Err(AppError::bad("the robot of this run was deleted"));
    }
    let run: Uuid = sqlx::query_scalar(
        "INSERT INTO crm_robot_runs (robot_id, schedule_id, robot_name, item_id, stage_id, category_id, action, run_at, depth, data)
         SELECT robot_id, schedule_id, robot_name, item_id, stage_id, category_id, action, now(), 0, '{\"waited\": true}' FROM crm_robot_runs WHERE id = $1 RETURNING id",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    enqueue_run(&state, run, Utc::now(), false).await?;
    Ok(Json(json!({ "id": run })))
}

#[derive(Deserialize)]
struct TimesQuery {
    category_id: Uuid,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
}

#[derive(Serialize, FromRow)]
struct StageTime {
    stage_id: Uuid,
    entries: i64,
    avg_hours: Option<f64>,
    median_hours: Option<f64>,
    max_hours: Option<f64>,
}

/// Per stage, for visits that started in the period: how many, and how long they lasted
/// (visits still going count until now). Plus what is in each stage now, and how often
/// the stage's time limit was exceeded.
async fn stage_times(State(state): State<AppState>, auth: AuthUser, Query(q): Query<TimesQuery>) -> AppResult<Json<Value>> {
    require_admin(&auth)?;
    let to = q.to.unwrap_or_else(|| Utc::now() + Duration::days(1));
    let from = q.from.unwrap_or(to - Duration::days(90));
    if to <= from || to - from > Duration::days(3 * 366) {
        return Err(AppError::bad("choose a period of at most three years"));
    }
    let times: Vec<StageTime> = sqlx::query_as(
        "WITH visits AS (
             SELECT h.item_id, h.category_id, h.stage_id, h.entered_at,
                    LEAD(h.entered_at) OVER (PARTITION BY h.item_id ORDER BY h.entered_at, h.id) AS left_at
               FROM crm_stage_history h
              WHERE h.item_id IN (SELECT item_id FROM crm_stage_history WHERE category_id = $1 AND entered_at >= $2 AND entered_at < $3)
         )
         SELECT v.stage_id, COUNT(*)::int8 AS entries,
                (AVG(EXTRACT(EPOCH FROM (COALESCE(v.left_at, now()) - v.entered_at))) / 3600)::float8 AS avg_hours,
                (percentile_cont(0.5) WITHIN GROUP (ORDER BY EXTRACT(EPOCH FROM (COALESCE(v.left_at, now()) - v.entered_at))) / 3600)::float8 AS median_hours,
                (MAX(EXTRACT(EPOCH FROM (COALESCE(v.left_at, now()) - v.entered_at))) / 3600)::float8 AS max_hours
           FROM visits v JOIN crm_items i ON i.id = v.item_id AND i.deleted_at IS NULL
          WHERE v.category_id = $1 AND v.entered_at >= $2 AND v.entered_at < $3
          GROUP BY v.stage_id",
    )
    .bind(q.category_id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.db)
    .await?;
    let now: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT i.stage_id, COUNT(*)::int8,
                COUNT(*) FILTER (WHERE s.sla_minutes IS NOT NULL AND i.stage_changed_at + make_interval(mins => s.sla_minutes) < now())::int8
           FROM crm_items i JOIN crm_stages s ON s.id = i.stage_id
          WHERE i.category_id = $1 AND i.deleted_at IS NULL
          GROUP BY i.stage_id",
    )
    .bind(q.category_id)
    .fetch_all(&state.db)
    .await?;
    let breaches: Vec<(String, i64)> = sqlx::query_as(
        "SELECT t.data->'stage'->>'id', COUNT(*)::int8 FROM crm_timeline t JOIN crm_items i ON i.id = t.item_id
          WHERE t.kind = 'sla' AND i.category_id = $1 AND t.created_at >= $2 AND t.created_at < $3
          GROUP BY 1",
    )
    .bind(q.category_id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.db)
    .await?;
    let stages: Vec<(Uuid, String, String, String, Option<i32>)> = sqlx::query_as("SELECT id, name, color, semantics, sla_minutes FROM crm_stages WHERE category_id = $1 ORDER BY sort, name")
        .bind(q.category_id)
        .fetch_all(&state.db)
        .await?;
    let rows: Vec<Value> = stages
        .iter()
        .map(|(id, name, color, semantics, sla)| {
            let t = times.iter().find(|t| t.stage_id == *id);
            let n = now.iter().find(|n| n.0 == *id);
            json!({
                "stage_id": id, "name": name, "color": color, "semantics": semantics, "sla_minutes": sla,
                "entries": t.map(|t| t.entries).unwrap_or(0),
                "avg_hours": t.and_then(|t| t.avg_hours),
                "median_hours": t.and_then(|t| t.median_hours),
                "max_hours": t.and_then(|t| t.max_hours),
                "in_stage": n.map(|n| n.1).unwrap_or(0),
                "over_limit": n.map(|n| n.2).unwrap_or(0),
                "breaches": breaches.iter().find(|b| b.0 == id.to_string()).map(|b| b.1).unwrap_or(0),
            })
        })
        .collect();
    Ok(Json(json!({ "period": { "from": from, "to": to }, "stages": rows })))
}
