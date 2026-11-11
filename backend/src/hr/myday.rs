//! My day: one page for today — the work day (start, pause, end), today's shift and check-in,
//! tasks due today or overdue, today's events, one's own plan for the day (free items or
//! tasks, ticked off), and the day's report.

use axum::{
    extract::{Path, State},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{Duration, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::time::tz_of;
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::required,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/hr/my-day", get(view))
        .route("/hr/my-day/plan", post(add))
        .route("/hr/my-day/plan/{id}", patch(change).delete(remove))
}

async fn view(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let tz = tz_of(&state, auth.id).await?;
    let today = Utc::now().with_timezone(&tz).date_naive();
    let at = |d: chrono::NaiveDate| tz.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).earliest().map(|t| t.with_timezone(&Utc)).unwrap_or_else(Utc::now);
    let (from, to) = (at(today), at(today + Duration::days(1)));
    let workday = super::time::today_view(&state, auth.id).await?;
    let shift: Option<(String, i32, i32, String)> =
        sqlx::query_as("SELECT s.name, s.start_min, s.end_min, s.color FROM hr_rota r JOIN hr_shifts s ON s.id = r.shift_id WHERE r.user_id = $1 AND r.day = $2").bind(auth.id).bind(today).fetch_optional(&state.db).await?;
    let checkin: Option<(String, Option<String>)> = sqlx::query_as("SELECT status, location FROM checkins WHERE user_id = $1 AND day = $2").bind(auth.id).bind(today).fetch_optional(&state.db).await?;
    let tasks: Vec<(Uuid, i64, String, Option<chrono::DateTime<Utc>>, String)> = sqlx::query_as(
        "SELECT id, number, title, deadline, status FROM tasks
         WHERE responsible_id = $1 AND status NOT IN ('done', 'deferred') AND deadline < $2 ORDER BY deadline LIMIT 50",
    )
    .bind(auth.id)
    .bind(to)
    .fetch_all(&state.db)
    .await?;
    // Today's events (repeats included).
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT e.id FROM cal_events e LEFT JOIN cal_attendees a ON a.event_id = e.id AND a.user_id = $1
         WHERE (e.owner_id = $1 OR (a.user_id IS NOT NULL AND a.status <> 'declined')) AND e.starts_at < $3 AND (e.recurrence IS NOT NULL OR e.ends_at > $2)",
    )
    .bind(auth.id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.db)
    .await?;
    let events = crate::calendar::load_events(&state, &ids).await?;
    let ex = crate::calendar::exceptions(&state, &ids).await?;
    let mut today_events = Vec::new();
    for (e, _) in &events {
        let excs: Vec<chrono::DateTime<Utc>> = ex.iter().filter(|(id, _)| *id == e.id).map(|(_, t)| *t).collect();
        for s in crate::calendar::occurrences(e, &excs, from, to) {
            today_events.push(json!({ "id": e.id, "title": e.title, "start": s, "end": s + (e.ends_at - e.starts_at), "all_day": e.all_day, "location": e.location }));
        }
    }
    today_events.sort_by(|a, b| a["start"].as_str().cmp(&b["start"].as_str()));
    let plan: Vec<(Uuid, String, Option<Uuid>, bool, Option<i64>)> = sqlx::query_as(
        "SELECT p.id, p.title, p.task_id, p.done OR COALESCE(t.status = 'done', FALSE), t.number FROM hr_day_plan p LEFT JOIN tasks t ON t.id = p.task_id
         WHERE p.user_id = $1 AND p.day = $2 ORDER BY p.position, p.created_at",
    )
    .bind(auth.id)
    .bind(today)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "day": today, "workday": workday,
        "shift": shift.map(|(n, s, e, c)| json!({ "name": n, "start_min": s, "end_min": e, "color": c })),
        "checkin": checkin.map(|(s, l)| json!({ "status": s, "location": l })),
        "tasks": tasks.into_iter().map(|(id, n, t, d, s)| json!({ "id": id, "number": n, "title": t, "deadline": d, "status": s, "overdue": d.is_some_and(|d| d < from) })).collect::<Vec<_>>(),
        "events": today_events,
        "plan": plan.into_iter().map(|(id, t, task, done, n)| json!({ "id": id, "title": t, "task_id": task, "task_number": n, "done": done })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct NewItem {
    title: Option<String>,
    task_id: Option<Uuid>,
}

async fn add(State(state): State<AppState>, auth: AuthUser, Json(b): Json<NewItem>) -> AppResult<Json<Value>> {
    let tz = tz_of(&state, auth.id).await?;
    let today = Utc::now().with_timezone(&tz).date_naive();
    let title = match b.task_id {
        Some(t) => {
            let (core, _) = crate::tasks::access::load_task(&state, &auth, t).await?;
            core.title
        }
        None => required("title", b.title.as_deref().unwrap_or(""), 300)?,
    };
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hr_day_plan WHERE user_id = $1 AND day = $2").bind(auth.id).bind(today).fetch_one(&state.db).await?;
    if n >= 100 {
        return Err(AppError::bad("that's a full day already"));
    }
    if let Some(t) = b.task_id {
        let dup: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM hr_day_plan WHERE user_id = $1 AND day = $2 AND task_id = $3)").bind(auth.id).bind(today).bind(t).fetch_one(&state.db).await?;
        if dup {
            return view(State(state), auth).await;
        }
    }
    sqlx::query("INSERT INTO hr_day_plan (user_id, day, title, task_id, position) VALUES ($1, $2, $3, $4, $5)").bind(auth.id).bind(today).bind(title).bind(b.task_id).bind(n as i32).execute(&state.db).await?;
    view(State(state), auth).await
}

#[derive(Deserialize)]
struct Change {
    done: Option<bool>,
    title: Option<String>,
}

async fn change(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<Change>) -> AppResult<Json<Value>> {
    let title = b.title.as_deref().map(|t| required("title", t, 300)).transpose()?;
    let n = sqlx::query("UPDATE hr_day_plan SET done = COALESCE($3, done), title = COALESCE($4, title) WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(auth.id)
        .bind(b.done)
        .bind(title)
        .execute(&state.db)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("plan item"));
    }
    view(State(state), auth).await
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let n = sqlx::query("DELETE FROM hr_day_plan WHERE id = $1 AND user_id = $2").bind(id).bind(auth.id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("plan item"));
    }
    view(State(state), auth).await
}
