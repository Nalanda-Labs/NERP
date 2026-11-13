//! Task reports: workload and efficiency per person, and how long tasks take.
//!
//! For each person over a period: tasks finished, finished on time (by the deadline, or with
//! none), finished late, open now, overdue now, the hours of estimates still open, the time
//! logged, ratings, and the average time from a task's creation to its completion.
//! **Efficiency** is the share of tasks handled on time: finished on time over everything
//! finished in the period plus what is overdue now.
//!
//! Everyone sees their own; department heads see their people (and sub-departments');
//! "manage all tasks" sees everyone.

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/tasks/reports/efficiency", get(efficiency))
}

#[derive(Deserialize)]
struct ReportQuery {
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
    /// me | team (my departments' people) | all; team by default.
    scope: Option<String>,
    project_id: Option<Uuid>,
}

async fn efficiency(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ReportQuery>) -> AppResult<Json<Value>> {
    let to = q.to.unwrap_or_else(Utc::now);
    let from = q.from.unwrap_or(to - Duration::days(30));
    if from >= to {
        return Err(AppError::bad("the period ends before it starts"));
    }
    if let Some(p) = q.project_id {
        super::access::project_access(&state, &auth, p).await?;
    }
    let scope = q.scope.unwrap_or_else(|| "team".into());
    let people: Vec<Uuid> = match scope.as_str() {
        "me" => vec![auth.id],
        "all" => {
            if !auth.can(perm::TASKS_MANAGE_ALL) {
                return Err(AppError::Forbidden);
            }
            sqlx::query_scalar!("SELECT id FROM users WHERE is_active").fetch_all(&state.db).await?
        }
        _ => {
            let mut v: Vec<Uuid> = sqlx::query_scalar!("WITH RECURSIVE tree AS (SELECT id FROM departments WHERE head_user_id = $1 UNION ALL SELECT d.id FROM departments d JOIN tree ON d.parent_id = tree.id)
                 SELECT DISTINCT ud.user_id FROM user_departments ud JOIN tree ON tree.id = ud.department_id JOIN users u ON u.id = ud.user_id WHERE u.is_active", auth.id)
            .fetch_all(&state.db)
            .await?;
            v.push(auth.id);
            v.sort();
            v.dedup();
            v
        }
    };
    #[allow(clippy::type_complexity)]
    let rows: Vec<(Uuid, String, Option<Uuid>, i64, i64, i64, i64, i64, f64, f64, Option<f64>, i64, i64)> = sqlx::query!(r#"SELECT u.id AS "id!", trim(u.first_name || ' ' || u.last_name) AS "c1!", u.avatar_file_id AS "avatar_file_id?",
            COUNT(t.id) FILTER (WHERE t.status = 'done' AND t.completed_at >= $2 AND t.completed_at < $3) AS "done!",
            COUNT(t.id) FILTER (WHERE t.status = 'done' AND t.completed_at >= $2 AND t.completed_at < $3 AND (t.deadline IS NULL OR t.completed_at <= t.deadline)) AS "on_time!",
            COUNT(t.id) FILTER (WHERE t.status IN ('new','in_progress','review')) AS "open!",
            COUNT(t.id) FILTER (WHERE t.status IN ('new','in_progress','review') AND t.deadline < now()) AS "overdue!",
            COUNT(t.id) FILTER (WHERE t.status = 'in_progress') AS "in_progress!",
            COALESCE(SUM(t.time_estimate_secs) FILTER (WHERE t.status IN ('new','in_progress','review')), 0)::float8 / 3600 AS "open_estimate_hours!",
            COALESCE((SELECT SUM(te.seconds) FROM task_time_entries te WHERE te.user_id = u.id AND te.started_at >= $2 AND te.started_at < $3 AND te.stopped_at IS NOT NULL), 0)::float8 / 3600 AS "logged_hours!",
            (AVG(EXTRACT(EPOCH FROM (t.completed_at - t.created_at)) / 3600) FILTER (WHERE t.status = 'done' AND t.completed_at >= $2 AND t.completed_at < $3))::float8 AS "avg_hours?",
            COUNT(t.id) FILTER (WHERE t.rating = 'good' AND t.rated_at >= $2 AND t.rated_at < $3) AS "rated_good!",
            COUNT(t.id) FILTER (WHERE t.rating = 'bad' AND t.rated_at >= $2 AND t.rated_at < $3) AS "rated_bad!"
         FROM users u LEFT JOIN tasks t ON t.responsible_id = u.id AND ($4::uuid IS NULL OR t.project_id = $4)
         WHERE u.id = ANY($1)
         GROUP BY u.id ORDER BY u.last_name, u.first_name"#, &people, from, to, q.project_id).map(|r| (r.id, r.c1, r.avatar_file_id, r.done, r.on_time, r.open, r.overdue, r.in_progress, r.open_estimate_hours, r.logged_hours, r.avg_hours, r.rated_good, r.rated_bad))
    .fetch_all(&state.db)
    .await?;
    let round = |x: f64| (x * 10.0).round() / 10.0;
    let mut totals = (0i64, 0i64, 0i64, 0i64);
    let items: Vec<Value> = rows
        .into_iter()
        .map(|(id, name, avatar, done, on_time, open, overdue, in_progress, est, logged, avg, good, bad)| {
            totals.0 += done;
            totals.1 += on_time;
            totals.2 += open;
            totals.3 += overdue;
            let base = done + overdue;
            json!({
                "user_id": id, "full_name": name, "avatar_file_id": avatar,
                "done": done, "on_time": on_time, "late": done - on_time, "open": open, "overdue": overdue, "in_progress": in_progress,
                "open_estimate_hours": round(est), "logged_hours": round(logged), "avg_completion_hours": avg.map(round),
                "rated_good": good, "rated_bad": bad,
                "efficiency": if base == 0 { None } else { Some(((on_time as f64 / base as f64) * 100.0).round() as i64) },
            })
        })
        .collect();
    let base = totals.0 + totals.3;
    Ok(Json(json!({
        "from": from, "to": to, "scope": scope,
        "items": items,
        "totals": { "done": totals.0, "on_time": totals.1, "open": totals.2, "overdue": totals.3, "efficiency": if base == 0 { None } else { Some(((totals.1 as f64 / base as f64) * 100.0).round() as i64) } },
    })))
}
