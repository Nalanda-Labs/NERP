//! Sales commissions from paid invoices, taken back when the money is.
//!
//! A plan says who earns (people, departments or everyone), on what (the whole invoice or
//! some products) and how much: a percent of the net sale (without tax, in the base currency),
//! with optional monthly tiers. An invoice's responsible person earns as its money comes in:
//! what it has earned is the sale × the share actually paid and not credited. Whenever that
//! changes — a payment, a voided payment, a credit note, a voided invoice — the difference is
//! entered in the current open month: earned, or taken back. Statements per person and month
//! add the entries up; a month is locked once paid out (later changes land in the next open
//! month) and exported as CSV for payroll.
use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use chrono::{Datelike, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    audit,
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/commissions/plans", get(list_plans).post(create_plan))
        .route("/commissions/plans/{id}", patch(update_plan).delete(delete_plan))
        .route("/commissions/statement", get(statement))
        .route("/commissions/summary", get(summary))
        .route("/commissions/months/{month}/lock", post(lock).delete(unlock))
        .route("/commissions/recalculate", post(recalculate))
        .route("/commissions/export.csv", get(export))
}

fn first_of(d: NaiveDate) -> NaiveDate {
    d.with_day(1).expect("day 1")
}

fn round2(d: Decimal) -> Decimal {
    d.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero)
}

// ---- plans ----

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Tier {
    from: Decimal,
    rate: Decimal,
}

#[derive(Serialize, sqlx::FromRow)]
struct Plan {
    id: Uuid,
    name: String,
    is_active: bool,
    rate: Decimal,
    tiers: Value,
    product_ids: Vec<Uuid>,
    user_ids: Vec<Uuid>,
    department_ids: Vec<Uuid>,
}

const PLAN_COLS: &str = "id, name, is_active, rate, tiers, product_ids, user_ids, department_ids";

async fn list_plans(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::COMMISSIONS_MANAGE)?;
    let plans: Vec<Plan> = sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {PLAN_COLS} FROM comm_plans ORDER BY is_active DESC, name"))).fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": plans })))
}

#[derive(Deserialize)]
struct PlanInput {
    name: String,
    #[serde(default = "yes")]
    is_active: bool,
    rate: Decimal,
    #[serde(default)]
    tiers: Vec<Tier>,
    #[serde(default)]
    product_ids: Vec<Uuid>,
    #[serde(default)]
    user_ids: Vec<Uuid>,
    #[serde(default)]
    department_ids: Vec<Uuid>,
}
fn yes() -> bool {
    true
}

fn check_plan(b: &mut PlanInput) -> AppResult<()> {
    b.name = b.name.trim().to_string();
    if b.name.is_empty() || b.name.len() > 200 {
        return Err(AppError::bad("give the plan a name"));
    }
    let pct = |r: Decimal| r >= Decimal::ZERO && r <= Decimal::from(100);
    if !pct(b.rate) || b.tiers.iter().any(|t| !pct(t.rate) || t.from <= Decimal::ZERO) {
        return Err(AppError::bad("rates are percents from 0 to 100, and tiers start above zero"));
    }
    b.tiers.sort_by(|a, c| a.from.cmp(&c.from));
    if b.tiers.len() > 20 || b.product_ids.len() > 500 || b.user_ids.len() > 500 || b.department_ids.len() > 200 {
        return Err(AppError::bad("too many tiers, products or people"));
    }
    Ok(())
}

async fn create_plan(State(state): State<AppState>, auth: AuthUser, Json(mut b): Json<PlanInput>) -> AppResult<Json<Plan>> {
    auth.require(perm::COMMISSIONS_MANAGE)?;
    check_plan(&mut b)?;
    let p: Plan = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO comm_plans (name, is_active, rate, tiers, product_ids, user_ids, department_ids, created_by) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {PLAN_COLS}"
    )))
    .bind(&b.name)
    .bind(b.is_active)
    .bind(b.rate)
    .bind(json!(b.tiers))
    .bind(&b.product_ids)
    .bind(&b.user_ids)
    .bind(&b.department_ids)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    audit::record(&state.db, Some(auth.id), "commissions.plan", "comm_plan", Some(p.id.to_string()), json!({ "name": p.name, "rate": p.rate }), None).await;
    Ok(Json(p))
}

async fn update_plan(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(mut b): Json<PlanInput>) -> AppResult<Json<Plan>> {
    auth.require(perm::COMMISSIONS_MANAGE)?;
    check_plan(&mut b)?;
    let p: Plan = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE comm_plans SET name = $2, is_active = $3, rate = $4, tiers = $5, product_ids = $6, user_ids = $7, department_ids = $8, updated_at = now()
         WHERE id = $1 RETURNING {PLAN_COLS}"
    )))
    .bind(id)
    .bind(&b.name)
    .bind(b.is_active)
    .bind(b.rate)
    .bind(json!(b.tiers))
    .bind(&b.product_ids)
    .bind(&b.user_ids)
    .bind(&b.department_ids)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("plan"))?;
    audit::record(&state.db, Some(auth.id), "commissions.plan", "comm_plan", Some(id.to_string()), json!({ "name": p.name, "rate": p.rate, "is_active": p.is_active }), None).await;
    Ok(Json(p))
}

/// A plan with earnings is kept (turned off) so its entries keep their meaning.
async fn delete_plan(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    auth.require(perm::COMMISSIONS_MANAGE)?;
    let used: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM comm_entries WHERE plan_id = $1) AS "v!""#, id).fetch_one(&state.db).await?;
    if used {
        sqlx::query!("UPDATE comm_plans SET is_active = FALSE, updated_at = now() WHERE id = $1", id).execute(&state.db).await?;
        return Ok(Json(json!({ "deleted": false, "deactivated": true })));
    }
    sqlx::query!("DELETE FROM comm_plans WHERE id = $1", id).execute(&state.db).await?;
    Ok(Json(json!({ "deleted": true })))
}

// ---- earning ----

/// The first month that isn't locked, from this one on.
async fn open_month(db: &mut sqlx::PgConnection) -> sqlx::Result<NaiveDate> {
    let mut m = first_of(Utc::now().date_naive());
    loop {
        let locked: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM comm_months WHERE month = $1) AS "v!""#, m).fetch_one(&mut *db).await?;
        if !locked {
            return Ok(m);
        }
        m = first_of(m + chrono::Duration::days(32));
    }
}

/// Brings an invoice's commissions in line with what has been paid and credited. Called after
/// every payment, credit note and void; safe to call any time.
pub async fn sync_invoice(state: &AppState, item: Uuid) {
    if let Err(e) = sync(state, item).await {
        tracing::warn!(error = ?e, %item, "commission sync failed");
    }
}

async fn sync(state: &AppState, item: Uuid) -> anyhow::Result<()> {
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('comm:' || $1::text))").bind(item).execute(&mut *tx).await?;
    let inv: Option<(Decimal, Decimal, String, Decimal, i64, Option<Uuid>)> = sqlx::query!(r#"SELECT a.total, a.credited, a.status, a.fx_rate, i.number, i.assigned_to FROM acc_invoices a JOIN crm_items i ON i.id = a.item_id WHERE a.item_id = $1"#, item).map(|r| (r.total, r.credited, r.status, r.fx_rate, r.number, r.assigned_to))
    .fetch_optional(&mut *tx)
    .await?;
    let existing: Vec<(Uuid, Uuid, Decimal, Decimal)> = sqlx::query!(r#"SELECT plan_id AS "plan_id!", user_id AS "user_id!", SUM(base) AS "c2!", SUM(amount) AS "c3!" FROM comm_entries WHERE invoice_id = $1 GROUP BY plan_id, user_id"#, item).map(|r| (r.plan_id, r.user_id, r.c2, r.c3))
    .fetch_all(&mut *tx)
    .await?;
    let Some((total, credited, status, fx, number, responsible)) = inv else { return Ok(()) };
    let paid = crate::accounting::invoices::paid(&mut *tx, "invoice", item).await?;
    // The share of the sale that is really paid for.
    let share = if status == "void" || total <= Decimal::ZERO { Decimal::ZERO } else { (paid.min(total - credited).max(Decimal::ZERO) / total).min(Decimal::ONE) };
    // Who earns: whoever earned on it first, else the responsible person now.
    let earner = existing.first().map(|e| e.1).or(responsible);
    let mut targets: HashMap<(Uuid, Uuid), Decimal> = HashMap::new();
    if let Some(user) = earner {
        let rows = crate::crm::money::rows_of(state, item).await?;
        let net_of = |products: &[Uuid]| -> Decimal {
            if rows.is_empty() {
                return if products.is_empty() { total } else { Decimal::ZERO };
            }
            rows.iter().filter(|r| products.is_empty() || r.product_id.is_some_and(|p| products.contains(&p))).map(|r| crate::crm::money::line(r).net).sum()
        };
        let plans: Vec<(Uuid, Vec<Uuid>)> = sqlx::query!(r#"SELECT p.id AS "id!", p.product_ids AS "product_ids!" FROM comm_plans p WHERE p.is_active AND (
                 (cardinality(p.user_ids) = 0 AND cardinality(p.department_ids) = 0) OR $1 = ANY(p.user_ids)
                 OR EXISTS (SELECT 1 FROM user_departments ud WHERE ud.user_id = $1 AND ud.department_id = ANY(p.department_ids)))"#, user).map(|r| (r.id, r.product_ids))
        .fetch_all(&mut *tx)
        .await?;
        for (plan, products) in plans {
            targets.insert((plan, user), round2(net_of(&products) * fx * share));
        }
    }
    // Plans that earned before but no longer apply go back to zero.
    for (plan, user, _, _) in &existing {
        targets.entry((*plan, *user)).or_insert(Decimal::ZERO);
    }
    let month = open_month(&mut tx).await?;
    let pct = (share * Decimal::from(100)).round_dp(0);
    for ((plan, user), target_base) in targets {
        let (had_base, had_amount) = existing.iter().find(|e| e.0 == plan && e.1 == user).map(|e| (e.2, e.3)).unwrap_or_default();
        let delta = target_base - had_base;
        if delta.abs() < Decimal::new(1, 2) {
            continue;
        }
        let (rate, amount, reason) = if delta > Decimal::ZERO {
            let rate = rate_for(&mut tx, plan, user, month, delta).await?;
            (rate, round2(delta * rate / Decimal::from(100)), format!("Invoice #{number}: {pct}% paid"))
        } else {
            // Taken back at the rate it was earned at.
            let rate = if had_base.is_zero() { Decimal::ZERO } else { had_amount / had_base * Decimal::from(100) };
            let amount = if had_base.is_zero() { Decimal::ZERO } else { round2(had_amount * delta / had_base) };
            let why = if status == "void" { "voided".to_string() } else if credited > Decimal::ZERO && paid >= total - credited { "credited".to_string() } else { format!("now {pct}% paid") };
            (rate.round_dp(4), amount, format!("Invoice #{number}: taken back ({why})"))
        };
        sqlx::query!("INSERT INTO comm_entries (plan_id, user_id, invoice_id, month, base, rate, amount, reason) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)", plan, user, item, month, delta, rate, amount, &reason)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The plan's rate for this much more sale in the month: the highest tier the person's month
/// total reaches with it.
async fn rate_for(tx: &mut sqlx::PgConnection, plan: Uuid, user: Uuid, month: NaiveDate, more: Decimal) -> sqlx::Result<Decimal> {
    let (rate, tiers): (Decimal, Value) = sqlx::query!(r#"SELECT rate, tiers FROM comm_plans WHERE id = $1"#, plan).map(|r| (r.rate, r.tiers)).fetch_one(&mut *tx).await?;
    let tiers: Vec<Tier> = serde_json::from_value(tiers).unwrap_or_default();
    if tiers.is_empty() {
        return Ok(rate);
    }
    let so_far: Decimal = sqlx::query_scalar!(r#"SELECT COALESCE(SUM(base), 0) AS "v!" FROM comm_entries WHERE plan_id = $1 AND user_id = $2 AND month = $3"#, plan, user, month)
        .fetch_one(&mut *tx)
        .await?;
    let reached = so_far + more;
    Ok(tiers.iter().filter(|t| t.from <= reached).map(|t| t.rate).next_back().unwrap_or(rate))
}

// ---- statements ----

async fn team(state: &AppState, auth: &AuthUser) -> AppResult<Option<Vec<Uuid>>> {
    if auth.can(perm::COMMISSIONS_MANAGE) {
        return Ok(None);
    }
    Ok(Some(
        sqlx::query_scalar!("WITH RECURSIVE mine AS (SELECT id FROM departments WHERE head_user_id = $1 OR deputy_user_id = $1
                                     UNION SELECT d.id FROM departments d JOIN mine m ON d.parent_id = m.id)
             SELECT DISTINCT ud.user_id FROM user_departments ud WHERE ud.department_id IN (SELECT id FROM mine)", auth.id)
        .fetch_all(&state.db)
        .await?,
    ))
}

fn parse_month(s: Option<&str>) -> AppResult<NaiveDate> {
    match s {
        None | Some("") => Ok(first_of(Utc::now().date_naive())),
        Some(v) => NaiveDate::parse_from_str(&format!("{}-01", v.get(..7).unwrap_or(v)), "%Y-%m-%d").map_err(|_| AppError::bad("month is YYYY-MM")),
    }
}

#[derive(Deserialize)]
struct StatementQ {
    month: Option<String>,
    user: Option<Uuid>,
}

/// One person's month: every entry with its invoice. One's own, a manager's team's, or anyone's
/// for those who manage commissions.
async fn statement(State(state): State<AppState>, auth: AuthUser, Query(q): Query<StatementQ>) -> AppResult<Json<Value>> {
    let month = parse_month(q.month.as_deref())?;
    let user = q.user.unwrap_or(auth.id);
    if user != auth.id && !team(&state, &auth).await?.is_none_or(|t| t.contains(&user)) {
        return Err(AppError::Forbidden);
    }
    let entries: Vec<(Uuid, String, Uuid, i64, String, Decimal, Decimal, Decimal, String, chrono::DateTime<Utc>)> = sqlx::query!(r#"SELECT e.id, p.name, e.invoice_id, i.number, i.title, e.base, e.rate, e.amount, e.reason, e.created_at
         FROM comm_entries e JOIN comm_plans p ON p.id = e.plan_id JOIN crm_items i ON i.id = e.invoice_id
         WHERE e.user_id = $1 AND e.month = $2 ORDER BY e.created_at"#, user, month).map(|r| (r.id, r.name, r.invoice_id, r.number, r.title, r.base, r.rate, r.amount, r.reason, r.created_at))
    .fetch_all(&state.db)
    .await?;
    let earned: Decimal = entries.iter().filter(|e| e.7 > Decimal::ZERO).map(|e| e.7).sum();
    let back: Decimal = entries.iter().filter(|e| e.7 < Decimal::ZERO).map(|e| e.7).sum();
    let locked: Option<chrono::DateTime<Utc>> = sqlx::query_scalar!("SELECT locked_at FROM comm_months WHERE month = $1", month).fetch_optional(&state.db).await?;
    let name: Option<String> = sqlx::query_scalar!(r#"SELECT trim(first_name || ' ' || last_name) AS "v!" FROM users WHERE id = $1"#, user).fetch_optional(&state.db).await?;
    let base = crate::accounting::base_currency(&state).await.unwrap_or_default();
    Ok(Json(json!({
        "user": { "id": user, "name": name }, "month": month.format("%Y-%m").to_string(), "currency": base, "locked_at": locked,
        "earned": earned, "taken_back": back, "net": earned + back,
        "entries": entries.into_iter().map(|e| json!({
            "id": e.0, "plan": e.1, "invoice_id": e.2, "invoice_number": e.3, "invoice_title": e.4, "base": e.5, "rate": e.6, "amount": e.7, "reason": e.8, "at": e.9,
            "link": format!("/crm/invoice/{}", e.2),
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
struct MonthQ {
    month: Option<String>,
}

async fn month_totals(state: &AppState, month: NaiveDate, only: Option<&[Uuid]>) -> AppResult<Vec<(Uuid, String, String, Decimal, Decimal)>> {
    Ok(sqlx::query!(r#"SELECT u.id AS "id!", trim(u.first_name || ' ' || u.last_name) AS "c1!", u.email AS "email!",
                COALESCE(SUM(e.amount) FILTER (WHERE e.amount > 0), 0) AS "c3!", COALESCE(SUM(e.amount) FILTER (WHERE e.amount < 0), 0) AS "c4!"
         FROM comm_entries e JOIN users u ON u.id = e.user_id WHERE e.month = $1 AND ($2::uuid[] IS NULL OR e.user_id = ANY($2))
         GROUP BY u.id ORDER BY 2"#, month, only).map(|r| (r.id, r.c1, r.email, r.c3, r.c4))
    .fetch_all(&state.db)
    .await?)
}

/// Everyone's (or the team's) totals for a month.
async fn summary(State(state): State<AppState>, auth: AuthUser, Query(q): Query<MonthQ>) -> AppResult<Json<Value>> {
    let month = parse_month(q.month.as_deref())?;
    let team = team(&state, &auth).await?;
    if team.as_ref().is_some_and(|t| t.is_empty()) {
        return Err(AppError::Forbidden);
    }
    let rows = month_totals(&state, month, team.as_deref()).await?;
    let locked: Option<chrono::DateTime<Utc>> = sqlx::query_scalar!("SELECT locked_at FROM comm_months WHERE month = $1", month).fetch_optional(&state.db).await?;
    let net: Decimal = rows.iter().map(|r| r.3 + r.4).sum();
    Ok(Json(json!({
        "month": month.format("%Y-%m").to_string(), "locked_at": locked, "net": net, "can_manage": auth.can(perm::COMMISSIONS_MANAGE),
        "people": rows.into_iter().map(|(id, n, e, earned, back)| json!({ "id": id, "name": n, "email": e, "earned": earned, "taken_back": back, "net": earned + back })).collect::<Vec<_>>(),
    })))
}

async fn lock(State(state): State<AppState>, auth: AuthUser, Path(month): Path<String>) -> AppResult<Json<Value>> {
    auth.require(perm::COMMISSIONS_MANAGE)?;
    let m = parse_month(Some(&month))?;
    if m > first_of(Utc::now().date_naive()) {
        return Err(AppError::bad("a month can be locked once it has started"));
    }
    sqlx::query!("INSERT INTO comm_months (month, locked_by) VALUES ($1, $2) ON CONFLICT DO NOTHING", m, auth.id).execute(&state.db).await?;
    audit::record(&state.db, Some(auth.id), "commissions.lock", "comm_month", Some(m.to_string()), json!({}), None).await;
    Ok(Json(json!({ "month": m.format("%Y-%m").to_string(), "locked": true })))
}

/// Unlocking is for mistakes before payroll ran; entries made meanwhile stay where they are.
async fn unlock(State(state): State<AppState>, auth: AuthUser, Path(month): Path<String>) -> AppResult<Json<Value>> {
    auth.require(perm::COMMISSIONS_MANAGE)?;
    let m = parse_month(Some(&month))?;
    sqlx::query!("DELETE FROM comm_months WHERE month = $1", m).execute(&state.db).await?;
    audit::record(&state.db, Some(auth.id), "commissions.unlock", "comm_month", Some(m.to_string()), json!({}), None).await;
    Ok(Json(json!({ "month": m.format("%Y-%m").to_string(), "locked": false })))
}

/// Brings every invoice in the books in line (after changing plans, say). Changes land in the
/// open month like any other.
async fn recalculate(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    auth.require(perm::COMMISSIONS_MANAGE)?;
    let items: Vec<Uuid> = sqlx::query_scalar!("SELECT item_id FROM acc_invoices WHERE status IN ('paid', 'credited') OR item_id IN (SELECT invoice_id FROM comm_entries)
            OR item_id IN (SELECT al.doc_id FROM acc_allocations al WHERE al.doc_kind = 'invoice')")
    .fetch_all(&state.db)
    .await?;
    let before: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM comm_entries"#).fetch_one(&state.db).await?;
    for i in &items {
        sync(&state, *i).await.map_err(AppError::Internal)?;
    }
    let after: i64 = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "v!" FROM comm_entries"#).fetch_one(&state.db).await?;
    audit::record(&state.db, Some(auth.id), "commissions.recalculate", "comm_month", None, json!({ "invoices": items.len(), "new_entries": after - before }), None).await;
    Ok(Json(json!({ "invoices": items.len(), "new_entries": after - before })))
}

/// For payroll: one line per person for the month.
async fn export(State(state): State<AppState>, auth: AuthUser, Query(q): Query<MonthQ>) -> AppResult<Response> {
    auth.require(perm::COMMISSIONS_MANAGE)?;
    let month = parse_month(q.month.as_deref())?;
    let base = crate::accounting::base_currency(&state).await.unwrap_or_default();
    let mut w = csv::Writer::from_writer(vec![]);
    let _ = w.write_record(["month", "employee", "email", "earned", "taken_back", "net", "currency"]);
    for (_, n, e, earned, back) in month_totals(&state, month, None).await? {
        let _ = w.write_record([month.format("%Y-%m").to_string(), n, e, earned.to_string(), back.to_string(), (earned + back).to_string(), base.clone()]);
    }
    let body = w.into_inner().unwrap_or_default();
    audit::record(&state.db, Some(auth.id), "commissions.export", "comm_month", Some(month.to_string()), json!({}), None).await;
    Ok((
        [(header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"commissions-{}.csv\"", month.format("%Y-%m")))],
        body,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months() {
        assert_eq!(parse_month(Some("2026-03")).unwrap(), NaiveDate::from_ymd_opt(2026, 3, 1).unwrap());
        assert_eq!(parse_month(Some("2026-03-17")).unwrap(), NaiveDate::from_ymd_opt(2026, 3, 1).unwrap());
        assert!(parse_month(Some("nope")).is_err());
    }
}
