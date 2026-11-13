//! Absences: requesting time off, approving it, the absence chart and vacation balances.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use super::{approvers, holidays, manages, managed, settings, working_days};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/hr/absences", get(chart).post(request))
        .route("/hr/absences/mine", get(mine))
        .route("/hr/absences/pending", get(pending))
        .route("/hr/absences/{id}/decide", post(decide))
        .route("/hr/absences/{id}/cancel", post(cancel))
        .route("/hr/balance", get(balance))
}

const KINDS: &[(&str, &str)] = &[("vacation", "Vacation"), ("sick", "Sick leave"), ("personal", "Personal day"), ("business_trip", "Business trip"), ("other", "Time off")];

fn label(kind: &str) -> &'static str {
    KINDS.iter().find(|(k, _)| *k == kind).map(|(_, l)| *l).unwrap_or("Time off")
}

#[derive(Serialize, FromRow)]
struct Absence {
    id: Uuid,
    user_id: Uuid,
    user_name: String,
    kind: String,
    starts_on: NaiveDate,
    ends_on: NaiveDate,
    note: Option<String>,
    status: String,
    decided_by_name: Option<String>,
    decided_at: Option<DateTime<Utc>>,
    decision_note: Option<String>,
    created_at: DateTime<Utc>,
}

const SELECT: &str = "SELECT a.id, a.user_id, trim(u.first_name || ' ' || u.last_name) AS user_name, a.kind, a.starts_on, a.ends_on, a.note, a.status,
    trim(d.first_name || ' ' || d.last_name) AS decided_by_name, a.decided_at, a.decision_note, a.created_at
    FROM hr_absences a JOIN users u ON u.id = a.user_id LEFT JOIN users d ON d.id = a.decided_by";

/// Working days of an absence.
async fn length(state: &AppState, from: NaiveDate, to: NaiveDate) -> AppResult<i64> {
    let s = settings(state).await?;
    Ok(working_days(&s, &holidays(state, from, to).await?, from, to))
}

#[derive(Deserialize)]
struct ChartQuery {
    from: NaiveDate,
    to: NaiveDate,
    department: Option<Uuid>,
}

/// Who is away when: approved and requested absences of everyone (or one department and
/// its sub-departments). Notes are shown only to the person and their managers.
async fn chart(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ChartQuery>) -> AppResult<Json<Value>> {
    if q.to < q.from || (q.to - q.from).num_days() > 400 {
        return Err(AppError::bad("show at most 400 days"));
    }
    let people: Vec<(Uuid, String, Option<Uuid>)> = sqlx::query!(r#"WITH RECURSIVE depts AS (SELECT id FROM departments WHERE id = $1 UNION SELECT d.id FROM departments d JOIN depts x ON d.parent_id = x.id)
         SELECT u.id AS "id!", trim(u.first_name || ' ' || u.last_name) AS "c1!", u.avatar_file_id AS "avatar_file_id?" FROM users u
         WHERE u.is_active AND ($1::uuid IS NULL OR u.id IN (SELECT user_id FROM user_departments WHERE department_id IN (SELECT id FROM depts)))
         ORDER BY u.first_name, u.last_name LIMIT 1000"#, q.department).map(|r| (r.id, r.c1, r.avatar_file_id))
    .fetch_all(&state.db)
    .await?;
    let ids: Vec<Uuid> = people.iter().map(|p| p.0).collect();
    let rows: Vec<Absence> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.user_id = ANY($1) AND a.status IN ('approved', 'pending') AND a.starts_on <= $3 AND a.ends_on >= $2 ORDER BY a.starts_on"
    )))
    .bind(&ids)
    .bind(q.from)
    .bind(q.to)
    .fetch_all(&state.db)
    .await?;
    let mine = managed(&state, &auth).await?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|mut a| {
            let sees = a.user_id == auth.id || mine.as_ref().is_none_or(|m| m.contains(&a.user_id));
            if !sees {
                a.note = None;
                a.decision_note = None;
            }
            serde_json::to_value(a).unwrap()
        })
        .collect();
    let hol = holidays(&state, q.from, q.to).await?;
    let s = settings(&state).await?;
    Ok(Json(json!({
        "people": people.into_iter().map(|(id, name, a)| json!({ "id": id, "name": name, "avatar_file_id": a })).collect::<Vec<_>>(),
        "items": items,
        "holidays": hol,
        "work_weekdays": s.work_weekdays,
    })))
}

async fn mine(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let items: Vec<Absence> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE a.user_id = $1 ORDER BY a.starts_on DESC LIMIT 200"))).bind(auth.id).fetch_all(&state.db).await?;
    let year = Utc::now().year();
    Ok(Json(json!({ "items": items, "balance": balance_of(&state, auth.id, year).await? })))
}

/// Requests waiting for the user's decision.
async fn pending(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let scope = managed(&state, &auth).await?;
    let items: Vec<Absence> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} WHERE a.status = 'pending' AND a.user_id <> $1 AND ($2::uuid[] IS NULL OR a.user_id = ANY($2)) ORDER BY a.starts_on"
    )))
    .bind(auth.id)
    .bind(scope)
    .fetch_all(&state.db)
    .await?;
    let mut out = Vec::new();
    for a in items {
        let days = length(&state, a.starts_on, a.ends_on).await?;
        let mut v = serde_json::to_value(&a).unwrap();
        v["days"] = json!(days);
        if a.kind == "vacation" {
            v["balance"] = balance_of(&state, a.user_id, a.starts_on.year()).await?;
        }
        out.push(v);
    }
    Ok(Json(json!({ "items": out })))
}

/// A year's vacation: allowance, used (approved), requested (pending), left.
pub(crate) async fn balance_of(state: &AppState, user: Uuid, year: i32) -> AppResult<Value> {
    let s = settings(state).await?;
    let own: Option<Decimal> = sqlx::query_scalar!("SELECT days FROM hr_allowances WHERE user_id = $1 AND year = $2", user, year).fetch_optional(&state.db).await?;
    let allowance = own.unwrap_or(s.annual_vacation_days);
    let (first, last) = (NaiveDate::from_ymd_opt(year, 1, 1).unwrap(), NaiveDate::from_ymd_opt(year, 12, 31).unwrap());
    let rows: Vec<(NaiveDate, NaiveDate, String)> = sqlx::query!(r#"SELECT starts_on, ends_on, status FROM hr_absences WHERE user_id = $1 AND kind = 'vacation' AND status IN ('approved', 'pending') AND starts_on <= $3 AND ends_on >= $2"#, user, first, last).map(|r| (r.starts_on, r.ends_on, r.status))
    .fetch_all(&state.db)
    .await?;
    let hol = holidays(state, first, last).await?;
    let (mut used, mut requested) = (0, 0);
    for (a, b, st) in rows {
        let n = working_days(&s, &hol, a.max(first), b.min(last));
        if st == "approved" { used += n } else { requested += n }
    }
    let left = allowance - Decimal::from(used);
    Ok(json!({ "year": year, "allowance": allowance, "used": used, "requested": requested, "left": left, "custom": own.is_some() }))
}

#[derive(Deserialize)]
struct BalanceQuery {
    user: Option<Uuid>,
    year: Option<i32>,
}

async fn balance(State(state): State<AppState>, auth: AuthUser, Query(q): Query<BalanceQuery>) -> AppResult<Json<Value>> {
    let user = q.user.unwrap_or(auth.id);
    if user != auth.id && !manages(&state, &auth, user).await? {
        return Err(AppError::Forbidden);
    }
    balance_of(&state, user, q.year.unwrap_or_else(|| Utc::now().year())).await.map(Json)
}

#[derive(Deserialize)]
struct RequestInput {
    kind: String,
    starts_on: NaiveDate,
    ends_on: NaiveDate,
    note: Option<String>,
}

async fn request(State(state): State<AppState>, auth: AuthUser, Json(b): Json<RequestInput>) -> AppResult<Json<Value>> {
    if !KINDS.iter().any(|(k, _)| *k == b.kind) {
        return Err(AppError::bad("kind must be vacation, sick, personal, business_trip or other"));
    }
    if b.ends_on < b.starts_on {
        return Err(AppError::bad("the time off ends before it starts"));
    }
    if (b.ends_on - b.starts_on).num_days() > 366 {
        return Err(AppError::bad("at most a year at once"));
    }
    if b.starts_on.year() != b.ends_on.year() && b.kind == "vacation" {
        return Err(AppError::bad("split a vacation at the turn of the year (each year has its own allowance)"));
    }
    let overlap: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM hr_absences WHERE user_id = $1 AND status IN ('approved', 'pending') AND starts_on <= $3 AND ends_on >= $2) AS "v!""#, auth.id, b.starts_on, b.ends_on)
        .fetch_one(&state.db)
        .await?;
    if overlap {
        return Err(AppError::bad("you already have time off on some of those days"));
    }
    let days = length(&state, b.starts_on, b.ends_on).await?;
    if b.kind == "vacation" {
        if days == 0 {
            return Err(AppError::bad("those are not working days"));
        }
        let bal = balance_of(&state, auth.id, b.starts_on.year()).await?;
        let left: f64 = bal["left"].as_str().and_then(|x| x.parse().ok()).or_else(|| bal["left"].as_f64()).unwrap_or(0.0);
        let requested = bal["requested"].as_i64().unwrap_or(0) as f64;
        if days as f64 > left - requested {
            return Err(AppError::bad(format!("that is {days} working days; you have {} left (with requests waiting)", left - requested)));
        }
    }
    let note = b.note.map(|n| n.trim().chars().take(1000).collect::<String>()).filter(|n| !n.is_empty());
    let id: Uuid = sqlx::query_scalar!("INSERT INTO hr_absences (user_id, kind, starts_on, ends_on, note) VALUES ($1, $2, $3, $4, $5) RETURNING id", auth.id, &b.kind, b.starts_on, b.ends_on, note.as_deref())
        .fetch_one(&state.db)
        .await?;
    let me: String = sqlx::query_scalar!(r#"SELECT trim(first_name || ' ' || last_name) AS "v!" FROM users WHERE id = $1"#, auth.id).fetch_one(&state.db).await?;
    notify(&state, &approvers(&state, auth.id).await?, NewNotification {
        kind: "hr.absence.request",
        title: format!("{me} asks for {}: {} – {} ({days} working day{})", label(&b.kind).to_lowercase(), b.starts_on.format("%-d %b"), b.ends_on.format("%-d %b %Y"), if days == 1 { "" } else { "s" }),
        body: note,
        link: Some("/hr/absences?tab=approve".into()),
        actor: Some(auth.id),
    })
    .await;
    let a: Absence = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE a.id = $1"))).bind(id).fetch_one(&state.db).await?;
    let mut v = serde_json::to_value(a).unwrap();
    v["days"] = json!(days);
    Ok(Json(v))
}

#[derive(Deserialize)]
struct DecideBody {
    approve: bool,
    note: Option<String>,
}

async fn decide(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<DecideBody>) -> AppResult<Json<Value>> {
    let (user, status, kind, s, e): (Uuid, String, String, NaiveDate, NaiveDate) = sqlx::query!(r#"SELECT user_id, status, kind, starts_on, ends_on FROM hr_absences WHERE id = $1"#, id).map(|r| (r.user_id, r.status, r.kind, r.starts_on, r.ends_on))
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("request"))?;
    if user == auth.id || !manages(&state, &auth, user).await? {
        return Err(AppError::Forbidden);
    }
    if status != "pending" {
        return Err(AppError::bad("this request has already been decided"));
    }
    let note = b.note.map(|n| n.trim().chars().take(1000).collect::<String>()).filter(|n| !n.is_empty());
    let new = if b.approve { "approved" } else { "rejected" };
    sqlx::query!("UPDATE hr_absences SET status = $2, decided_by = $3, decided_at = now(), decision_note = $4 WHERE id = $1 AND status = 'pending'", id, new, auth.id, note.as_deref())
        .execute(&state.db)
        .await?;
    let me: String = sqlx::query_scalar!(r#"SELECT trim(first_name || ' ' || last_name) AS "v!" FROM users WHERE id = $1"#, auth.id).fetch_one(&state.db).await?;
    notify(&state, &[user], NewNotification {
        kind: "hr.absence.decided",
        title: format!("{me} {new} your {} {} – {}", label(&kind).to_lowercase(), s.format("%-d %b"), e.format("%-d %b")),
        body: note,
        link: Some("/hr/absences".into()),
        actor: Some(auth.id),
    })
    .await;
    pending(State(state), auth).await
}

/// The person withdraws a request, or cancels approved time off that hasn't started;
/// managers can cancel any.
async fn cancel(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let (user, status, starts): (Uuid, String, NaiveDate) = sqlx::query!(r#"SELECT user_id, status, starts_on FROM hr_absences WHERE id = $1"#, id).map(|r| (r.user_id, r.status, r.starts_on))
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("request"))?;
    let manager = user != auth.id && manages(&state, &auth, user).await?;
    if user != auth.id && !manager {
        return Err(AppError::Forbidden);
    }
    let started = starts <= Utc::now().date_naive();
    match status.as_str() {
        "pending" => {}
        "approved" if manager || !started => {}
        "approved" => return Err(AppError::bad("time off that has started can only be cancelled by your manager")),
        _ => return Err(AppError::bad("this request is already closed")),
    }
    sqlx::query!("UPDATE hr_absences SET status = 'cancelled' WHERE id = $1", id).execute(&state.db).await?;
    if status == "approved" && !manager {
        let me: String = sqlx::query_scalar!(r#"SELECT trim(first_name || ' ' || last_name) AS "v!" FROM users WHERE id = $1"#, auth.id).fetch_one(&state.db).await?;
        notify(&state, &approvers(&state, user).await?, NewNotification { kind: "hr.absence.cancelled", title: format!("{me} cancelled approved time off from {}", starts.format("%-d %b")), body: None, link: Some("/hr/absences".into()), actor: Some(auth.id) }).await;
    }
    mine(State(state), auth).await
}
