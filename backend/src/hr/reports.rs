//! Work reports: people report daily, weekly or monthly (the company's rule, or their own) to
//! whoever reads their reports (by default the nearest department heads and deputies). A
//! report is prepared with what the period holds (tasks finished, time on tasks, hours worked,
//! meetings), written, sent, and marked (good, bad or just read) with a comment. People are
//! reminded when a period ended without a report.

use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{Datelike, Duration, NaiveDate, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{approvers, hr_manager, manages, time::tz_of};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    notify::{notify, NewNotification},
    state::AppState,
};

pub const REMIND_JOB: &str = "hr.reports.remind";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/hr/reports", get(list))
        .route("/hr/reports/current", get(current))
        .route("/hr/reports/{id}", get(one).put(write))
        .route("/hr/reports/{id}/send", post(send))
        .route("/hr/reports/{id}/review", post(review))
        .route("/hr/report-rules", get(rules).put(set_rule))
        .route("/hr/report-rules/company", put(set_company))
}

/// The period a day falls in: (start, end).
pub(crate) fn period_of(kind: &str, day: NaiveDate) -> (NaiveDate, NaiveDate) {
    match kind {
        "weekly" => {
            let s = day - Duration::days(day.weekday().num_days_from_monday() as i64);
            (s, s + Duration::days(6))
        }
        "monthly" => {
            let s = day.with_day(1).unwrap();
            let next = if s.month() == 12 { NaiveDate::from_ymd_opt(s.year() + 1, 1, 1) } else { NaiveDate::from_ymd_opt(s.year(), s.month() + 1, 1) }.unwrap();
            (s, next - Duration::days(1))
        }
        _ => (day, day),
    }
}

/// How often someone reports, and who reads it.
async fn rule(state: &AppState, user: Uuid) -> AppResult<(String, Vec<Uuid>)> {
    let company: String = sqlx::query_scalar!("SELECT report_period FROM hr_settings WHERE id").fetch_one(&state.db).await?;
    let own: Option<(Option<String>, Option<Uuid>)> = sqlx::query!(r#"SELECT period, reviewer_id FROM hr_report_rules WHERE user_id = $1"#, user).map(|r| (r.period, r.reviewer_id)).fetch_optional(&state.db).await?;
    let period = own.as_ref().and_then(|o| o.0.clone()).unwrap_or(company);
    let reviewers = match own.and_then(|o| o.1) {
        Some(r) => vec![r],
        None => approvers(state, user).await?,
    };
    Ok((period, reviewers))
}

/// What the period holds, for the report.
async fn figures(state: &AppState, user: Uuid, from: NaiveDate, to: NaiveDate) -> AppResult<Value> {
    let tz = tz_of(state, user).await?;
    let at = |d: NaiveDate| tz.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).earliest().map(|t| t.with_timezone(&Utc)).unwrap_or_else(Utc::now);
    let (a, b) = (at(from), at(to + Duration::days(1)));
    let done: Vec<(Uuid, i64, String)> = sqlx::query!(r#"SELECT id, number, title FROM tasks WHERE responsible_id = $1 AND status = 'done' AND completed_at >= $2 AND completed_at < $3 ORDER BY completed_at LIMIT 200"#, user, a, b).map(|r| (r.id, r.number, r.title))
    .fetch_all(&state.db)
    .await?;
    let task_secs: i64 = sqlx::query_scalar!(r#"SELECT COALESCE(SUM(seconds), 0)::bigint AS "v!" FROM task_time_entries WHERE user_id = $1 AND started_at >= $2 AND started_at < $3"#, user, a, b)
        .fetch_one(&state.db)
        .await?;
    let meetings: i64 = sqlx::query_scalar!(r#"SELECT COUNT(DISTINCT e.id) AS "v!" FROM cal_events e LEFT JOIN cal_attendees x ON x.event_id = e.id AND x.user_id = $1
         WHERE (e.owner_id = $1 OR (x.user_id IS NOT NULL AND x.status <> 'declined')) AND e.starts_at >= $2 AND e.starts_at < $3 AND NOT e.all_day"#, user, a, b)
    .fetch_one(&state.db)
    .await?;
    let worked = super::time::worked_minutes(state, user, from, to).await?;
    Ok(json!({
        "tasks_done": done.iter().map(|(id, n, t)| json!({ "id": id, "number": n, "title": t })).collect::<Vec<_>>(),
        "task_minutes": task_secs / 60, "worked_minutes": worked, "meetings": meetings,
    }))
}

#[derive(sqlx::FromRow, serde::Serialize)]
struct Report {
    id: Uuid,
    user_id: Uuid,
    user_name: String,
    period: String,
    period_start: NaiveDate,
    period_end: NaiveDate,
    body: Option<String>,
    plans: Option<String>,
    figures: Value,
    status: String,
    reviewer_ids: Vec<Uuid>,
    sent_at: Option<chrono::DateTime<Utc>>,
    mark: Option<String>,
    review_comment: Option<String>,
    reviewed_by_name: Option<String>,
    reviewed_at: Option<chrono::DateTime<Utc>>,
}

const SELECT: &str = "SELECT r.id, r.user_id, trim(u.first_name || ' ' || u.last_name) AS user_name, r.period, r.period_start, r.period_end, r.body, r.plans,
    r.figures, r.status, r.reviewer_ids, r.sent_at, r.mark, r.review_comment, trim(v.first_name || ' ' || v.last_name) AS reviewed_by_name, r.reviewed_at
    FROM hr_reports r JOIN users u ON u.id = r.user_id LEFT JOIN users v ON v.id = r.reviewed_by";

async fn load(state: &AppState, id: Uuid) -> AppResult<Report> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE r.id = $1"))).bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("report"))
}

/// Its author, its readers, their managers and HR may see a report.
async fn visible(state: &AppState, auth: &AuthUser, r: &Report) -> AppResult<bool> {
    Ok(r.user_id == auth.id || r.reviewer_ids.contains(&auth.id) || hr_manager(auth) || manages(state, auth, r.user_id).await?)
}

/// The draft for a period (made when first asked for, with the period's figures).
pub(crate) async fn draft(state: &AppState, user: Uuid, period: &str, start: NaiveDate, end: NaiveDate) -> AppResult<Uuid> {
    if let Some(id) = sqlx::query_scalar!("SELECT id FROM hr_reports WHERE user_id = $1 AND period = $2 AND period_start = $3", user, period, start).fetch_optional(&state.db).await? {
        return Ok(id);
    }
    let f = figures(state, user, start, end).await?;
    sqlx::query!("INSERT INTO hr_reports (user_id, period, period_start, period_end, figures) VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING", user, period, start, end, f)
        .execute(&state.db)
        .await?;
    Ok(sqlx::query_scalar!("SELECT id FROM hr_reports WHERE user_id = $1 AND period = $2 AND period_start = $3", user, period, start).fetch_one(&state.db).await?)
}

#[derive(Deserialize)]
struct CurrentQuery {
    /// Another day's period (default today); and a period other than one's rule (e.g. the day, from My day).
    day: Option<NaiveDate>,
    period: Option<String>,
}

/// My report for the current period (or a given day's), prepared if needed.
async fn current(State(state): State<AppState>, auth: AuthUser, Query(q): Query<CurrentQuery>) -> AppResult<Json<Value>> {
    let (rule_period, reviewers) = rule(&state, auth.id).await?;
    let period = q.period.unwrap_or(rule_period.clone());
    if !["daily", "weekly", "monthly"].contains(&period.as_str()) {
        return Ok(Json(json!({ "period": "none", "report": null })));
    }
    let tz = tz_of(&state, auth.id).await?;
    let day = q.day.unwrap_or_else(|| Utc::now().with_timezone(&tz).date_naive());
    let (s, e) = period_of(&period, day);
    let id = draft(&state, auth.id, &period, s, e).await?;
    // A draft's figures stay up to date.
    let r = load(&state, id).await?;
    if r.status == "draft" {
        let f = figures(&state, auth.id, s, e).await?;
        sqlx::query!("UPDATE hr_reports SET figures = $2 WHERE id = $1", id, f).execute(&state.db).await?;
    }
    let names: Vec<String> = sqlx::query_scalar!(r#"SELECT trim(first_name || ' ' || last_name) AS "v!" FROM users WHERE id = ANY($1) ORDER BY 1"#, &reviewers).fetch_all(&state.db).await?;
    Ok(Json(json!({ "period": period, "rule": rule_period, "readers": names, "report": load(&state, id).await? })))
}

async fn one(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let r = load(&state, id).await?;
    if !visible(&state, &auth, &r).await? {
        return Err(AppError::NotFound("report"));
    }
    Ok(Json(json!(r)))
}

#[derive(Deserialize)]
struct WriteInput {
    body: Option<String>,
    plans: Option<String>,
}

async fn write(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<WriteInput>) -> AppResult<Json<Value>> {
    let r = load(&state, id).await?;
    if r.user_id != auth.id {
        return Err(AppError::NotFound("report"));
    }
    if r.status != "draft" {
        return Err(AppError::bad("a sent report can't be changed"));
    }
    let cut = |s: Option<String>| s.map(|x| x.chars().take(20000).collect::<String>());
    sqlx::query!("UPDATE hr_reports SET body = COALESCE($2, body), plans = COALESCE($3, plans) WHERE id = $1", id, cut(b.body), cut(b.plans)).execute(&state.db).await?;
    Ok(Json(json!(load(&state, id).await?)))
}

async fn send(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    let r = load(&state, id).await?;
    if r.user_id != auth.id {
        return Err(AppError::NotFound("report"));
    }
    if r.status != "draft" {
        return Err(AppError::bad("this report was sent already"));
    }
    if r.body.as_deref().is_none_or(|b| b.trim().is_empty()) {
        return Err(AppError::bad("write what you did first"));
    }
    let (_, reviewers) = rule(&state, auth.id).await?;
    if reviewers.is_empty() {
        return Err(AppError::bad("nobody reads your reports yet: ask HR to set who does"));
    }
    let f = figures(&state, auth.id, r.period_start, r.period_end).await?;
    sqlx::query!("UPDATE hr_reports SET status = 'sent', sent_at = now(), reviewer_ids = $2, figures = $3 WHERE id = $1", id, &reviewers, f).execute(&state.db).await?;
    notify(&state, &reviewers, NewNotification {
        kind: "hr.report",
        title: format!("{} sent a work report for {}", r.user_name, describe(&r.period, r.period_start, r.period_end)),
        body: r.body.as_ref().map(|b| b.chars().take(200).collect()),
        link: Some(format!("/hr/reports/{id}")),
        actor: Some(auth.id),
    })
    .await;
    Ok(Json(json!(load(&state, id).await?)))
}

fn describe(period: &str, s: NaiveDate, _e: NaiveDate) -> String {
    match period {
        "daily" => s.format("%a %-d %b").to_string(),
        "weekly" => format!("the week of {}", s.format("%-d %b")),
        _ => s.format("%B %Y").to_string(),
    }
}

#[derive(Deserialize)]
struct ReviewInput {
    mark: String,
    comment: Option<String>,
}

async fn review(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<ReviewInput>) -> AppResult<Json<Value>> {
    let r = load(&state, id).await?;
    if !(r.reviewer_ids.contains(&auth.id) || hr_manager(&auth) || manages(&state, &auth, r.user_id).await?) || r.user_id == auth.id {
        return Err(AppError::Forbidden);
    }
    if r.status == "draft" {
        return Err(AppError::bad("the report hasn't been sent"));
    }
    if !["good", "bad", "none"].contains(&b.mark.as_str()) {
        return Err(AppError::bad("mark it good, bad or none"));
    }
    sqlx::query!("UPDATE hr_reports SET status = 'reviewed', mark = $2, review_comment = $3, reviewed_by = $4, reviewed_at = now() WHERE id = $1", id, &b.mark, crate::routes::clean(b.comment).map(|c| c.chars().take(4000).collect::<String>()), auth.id)
        .execute(&state.db)
        .await?;
    let me: String = sqlx::query_scalar!("SELECT first_name FROM users WHERE id = $1", auth.id).fetch_one(&state.db).await?;
    let verb = match b.mark.as_str() { "good" => "liked", "bad" => "has concerns about", _ => "read" };
    notify(&state, &[r.user_id], NewNotification { kind: "hr.report", title: format!("{me} {verb} your report for {}", describe(&r.period, r.period_start, r.period_end)), body: None, link: Some(format!("/hr/reports/{id}")), actor: Some(auth.id) }).await;
    Ok(Json(json!(load(&state, id).await?)))
}

#[derive(Deserialize)]
struct ListQuery {
    /// mine (default) | review: sent to me | person: one person's (their managers and HR)
    scope: Option<String>,
    user_id: Option<Uuid>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    let rows: Vec<Report> = match q.scope.as_deref() {
        Some("review") => sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE $1 = ANY(r.reviewer_ids) AND r.status <> 'draft' ORDER BY (r.status = 'sent') DESC, r.sent_at DESC LIMIT 300")))
            .bind(auth.id)
            .fetch_all(&state.db)
            .await?,
        Some("person") => {
            let u = q.user_id.ok_or_else(|| AppError::bad("whose reports?"))?;
            if u != auth.id && !manages(&state, &auth, u).await? {
                return Err(AppError::Forbidden);
            }
            sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE r.user_id = $1 AND (r.status <> 'draft' OR r.user_id = $2) ORDER BY r.period_start DESC LIMIT 300")))
                .bind(u)
                .bind(auth.id)
                .fetch_all(&state.db)
                .await?
        }
        _ => sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} WHERE r.user_id = $1 ORDER BY r.period_start DESC LIMIT 300"))).bind(auth.id).fetch_all(&state.db).await?,
    };
    Ok(Json(json!({ "items": rows })))
}

async fn rules(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    let company: String = sqlx::query_scalar!("SELECT report_period FROM hr_settings WHERE id").fetch_one(&state.db).await?;
    let (mine, readers) = rule(&state, auth.id).await?;
    let own: Vec<(Uuid, String, Option<String>, Option<Uuid>, Option<String>)> = if hr_manager(&auth) {
        sqlx::query_as(
            "SELECT r.user_id, trim(u.first_name || ' ' || u.last_name), r.period, r.reviewer_id, trim(v.first_name || ' ' || v.last_name)
             FROM hr_report_rules r JOIN users u ON u.id = r.user_id LEFT JOIN users v ON v.id = r.reviewer_id ORDER BY 2",
        )
        .fetch_all(&state.db)
        .await?
    } else {
        vec![]
    };
    Ok(Json(json!({
        "company": company, "mine": mine, "readers": readers,
        "people": own.into_iter().map(|(u, n, p, r, rn)| json!({ "user_id": u, "name": n, "period": p, "reviewer_id": r, "reviewer_name": rn })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct RuleInput {
    user_id: Uuid,
    /// None: the company's.
    period: Option<String>,
    reviewer_id: Option<Uuid>,
}

/// A person's own rule: set by HR or their manager.
async fn set_rule(State(state): State<AppState>, auth: AuthUser, Json(b): Json<RuleInput>) -> AppResult<Json<Value>> {
    if !hr_manager(&auth) && !manages(&state, &auth, b.user_id).await? {
        return Err(AppError::Forbidden);
    }
    if b.period.as_deref().is_some_and(|p| !["none", "daily", "weekly", "monthly"].contains(&p)) {
        return Err(AppError::bad("reports are none, daily, weekly or monthly"));
    }
    if b.reviewer_id == Some(b.user_id) {
        return Err(AppError::bad("someone else reads one's reports"));
    }
    if b.period.is_none() && b.reviewer_id.is_none() {
        sqlx::query!("DELETE FROM hr_report_rules WHERE user_id = $1", b.user_id).execute(&state.db).await?;
    } else {
        sqlx::query!("INSERT INTO hr_report_rules (user_id, period, reviewer_id) VALUES ($1, $2, $3) ON CONFLICT (user_id) DO UPDATE SET period = $2, reviewer_id = $3", b.user_id, b.period.as_deref(), b.reviewer_id)
            .execute(&state.db)
            .await?;
    }
    rules(State(state), auth).await
}

#[derive(Deserialize)]
struct CompanyInput {
    period: String,
}

async fn set_company(State(state): State<AppState>, auth: AuthUser, Json(b): Json<CompanyInput>) -> AppResult<Json<Value>> {
    if !hr_manager(&auth) {
        return Err(AppError::Forbidden);
    }
    if !["none", "daily", "weekly", "monthly"].contains(&b.period.as_str()) {
        return Err(AppError::bad("reports are none, daily, weekly or monthly"));
    }
    sqlx::query!("UPDATE hr_settings SET report_period = $1, updated_at = now() WHERE id", &b.period).execute(&state.db).await?;
    rules(State(state), auth).await
}

/// Hourly: someone whose last period ended without a report sent is reminded once.
pub async fn remind(state: &AppState) -> Result<()> {
    let people: Vec<Uuid> = sqlx::query_scalar!("SELECT id FROM users WHERE is_active").fetch_all(&state.db).await?;
    for u in people {
        let (period, _) = rule(state, u).await?;
        if !["daily", "weekly", "monthly"].contains(&period.as_str()) {
            continue;
        }
        let tz = tz_of(state, u).await?;
        let today = Utc::now().with_timezone(&tz).date_naive();
        let (s, _) = period_of(&period, today);
        let (ps, pe) = period_of(&period, s - Duration::days(1));
        // Not for periods before someone joined, and only on working days for daily ones.
        let joined: chrono::DateTime<Utc> = sqlx::query_scalar!("SELECT created_at FROM users WHERE id = $1", u).fetch_one(&state.db).await?;
        if joined.with_timezone(&tz).date_naive() > ps {
            continue;
        }
        let worked: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM hr_work_days WHERE user_id = $1 AND day BETWEEN $2 AND $3) AS "v!""#, u, ps, pe).fetch_one(&state.db).await?;
        if period == "daily" && !worked {
            continue;
        }
        let id = draft(state, u, &period, ps, pe).await?;
        let n = sqlx::query!("UPDATE hr_reports SET reminded_at = now() WHERE id = $1 AND status = 'draft' AND reminded_at IS NULL", id).execute(&state.db).await?.rows_affected();
        if n == 1 {
            notify(state, &[u], NewNotification { kind: "hr.report", title: format!("Your work report for {} is due", describe(&period, ps, pe)), body: None, link: Some(format!("/hr/reports/{id}")), actor: None }).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 12).unwrap(); // a Thursday
        assert_eq!(period_of("daily", d), (d, d));
        assert_eq!(period_of("weekly", d), (NaiveDate::from_ymd_opt(2026, 11, 9).unwrap(), NaiveDate::from_ymd_opt(2026, 11, 15).unwrap()));
        assert_eq!(period_of("monthly", NaiveDate::from_ymd_opt(2026, 12, 31).unwrap()), (NaiveDate::from_ymd_opt(2026, 12, 1).unwrap(), NaiveDate::from_ymd_opt(2026, 12, 31).unwrap()));
    }
}
