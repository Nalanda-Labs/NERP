//! CRM analytics: headline figures, the sales funnel, sales over time, lead conversion,
//! sources, manager ratings and monthly sales targets.
//!
//! Every figure covers only the records the caller may read (the same scopes as lists).
//! Money is in the base currency (`opportunity_base`). Periods are calendar days in the
//! caller's time zone.

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, Months, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::collections::HashMap;
use uuid::Uuid;

use super::{
    access::{is_crm_admin, visible, Visible},
    entity_type, entity_type_by_id,
    money::base_currency,
    EntityType,
};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/analytics/{type}", get(report))
        .route("/crm/analytics/{type}/targets", get(get_targets).put(put_targets))
        .route("/crm/analytics/{type}/profit", get(profit))
}

/// Longest period one report may cover.
const MAX_DAYS: i64 = 3 * 366;

#[derive(Deserialize)]
struct ReportQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    /// A pipeline; omitted means all pipelines.
    category_id: Option<Uuid>,
    /// Comma-separated user ids (responsible managers).
    users: Option<String>,
}

/// The caller's time zone, falling back to UTC.
async fn user_tz(state: &AppState, auth: &AuthUser) -> Tz {
    let name: Option<String> = sqlx::query_scalar("SELECT timezone FROM users WHERE id = $1").bind(auth.id).fetch_optional(&state.db).await.ok().flatten();
    name.and_then(|n| n.parse::<Tz>().ok()).unwrap_or(Tz::UTC)
}

/// Start of a local day as a UTC instant.
fn day_start(tz: Tz, d: NaiveDate) -> DateTime<Utc> {
    let naive = d.and_hms_opt(0, 0, 0).expect("midnight");
    match tz.from_local_datetime(&naive) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => t.with_timezone(&Utc),
        // A day starting inside a DST gap: take the first valid instant.
        chrono::LocalResult::None => tz.from_utc_datetime(&naive).with_timezone(&Utc),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Bucket {
    Day,
    Week,
    Month,
}

impl Bucket {
    fn for_days(days: i64) -> Self {
        if days <= 45 {
            Bucket::Day
        } else if days <= 200 {
            Bucket::Week
        } else {
            Bucket::Month
        }
    }
    fn sql(self) -> &'static str {
        match self {
            Bucket::Day => "day",
            Bucket::Week => "week",
            Bucket::Month => "month",
        }
    }
    fn name(self) -> &'static str {
        self.sql()
    }
    /// The bucket containing `d` (weeks start on Monday, like Postgres `date_trunc`).
    fn floor(self, d: NaiveDate) -> NaiveDate {
        match self {
            Bucket::Day => d,
            Bucket::Week => d - Duration::days(d.weekday().num_days_from_monday() as i64),
            Bucket::Month => d.with_day(1).expect("day 1"),
        }
    }
    fn next(self, d: NaiveDate) -> NaiveDate {
        match self {
            Bucket::Day => d + Duration::days(1),
            Bucket::Week => d + Duration::days(7),
            Bucket::Month => d + Months::new(1),
        }
    }
    /// Every bucket start from the one holding `from` to the one holding `to`.
    fn series(self, from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
        let mut out = Vec::new();
        let mut d = self.floor(from);
        while d <= to {
            out.push(d);
            d = self.next(d);
        }
        out
    }
}

/// Filters shared by every query: the type, pipeline, managers and the caller's scope.
struct Base<'a> {
    t: &'a EntityType,
    category: Option<Uuid>,
    users: Vec<Uuid>,
    scope: &'a Visible,
}

impl Base<'_> {
    fn push(&self, qb: &mut QueryBuilder<Postgres>) {
        qb.push(" i.entity_type = ").push_bind(self.t.id).push(" AND i.deleted_at IS NULL");
        if let Some(c) = self.category {
            qb.push(" AND i.category_id = ").push_bind(c);
        }
        if !self.users.is_empty() {
            qb.push(" AND i.assigned_to = ANY(").push_bind(self.users.clone()).push(")");
        }
        self.scope.push(qb);
    }
}

#[derive(Serialize, FromRow, Default)]
struct Kpi {
    won_count: i64,
    won_amount: Decimal,
    lost_count: i64,
    new_count: i64,
    new_amount: Decimal,
    /// Average days from creation to winning, for records won in the period.
    avg_cycle_days: Option<f64>,
}

async fn kpi(state: &AppState, b: &Base<'_>, start: DateTime<Utc>, end: DateTime<Utc>) -> AppResult<Kpi> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT ");
    let won = |qb: &mut QueryBuilder<Postgres>| {
        qb.push("(i.stage_semantics = 'success' AND i.closed_at >= ").push_bind(start).push(" AND i.closed_at < ").push_bind(end).push(")");
    };
    qb.push("COUNT(*) FILTER (WHERE ");
    won(&mut qb);
    qb.push(")::int8 AS won_count, COALESCE(SUM(i.opportunity_base) FILTER (WHERE ");
    won(&mut qb);
    qb.push("), 0) AS won_amount, COUNT(*) FILTER (WHERE i.stage_semantics = 'failure' AND i.closed_at >= ").push_bind(start);
    qb.push(" AND i.closed_at < ").push_bind(end).push(")::int8 AS lost_count");
    qb.push(", COUNT(*) FILTER (WHERE i.created_at >= ").push_bind(start).push(" AND i.created_at < ").push_bind(end).push(")::int8 AS new_count");
    qb.push(", COALESCE(SUM(i.opportunity_base) FILTER (WHERE i.created_at >= ").push_bind(start).push(" AND i.created_at < ").push_bind(end).push("), 0) AS new_amount");
    qb.push(", (AVG(EXTRACT(EPOCH FROM (i.closed_at - i.created_at))) FILTER (WHERE ");
    won(&mut qb);
    qb.push(") / 86400)::float8 AS avg_cycle_days FROM crm_items i WHERE");
    b.push(&mut qb);
    // Only rows that can count towards any figure.
    qb.push(" AND ((i.closed_at >= ").push_bind(start).push(" AND i.closed_at < ").push_bind(end);
    qb.push(") OR (i.created_at >= ").push_bind(start).push(" AND i.created_at < ").push_bind(end).push("))");
    Ok(qb.build_query_as().fetch_one(&state.db).await?)
}

#[derive(Serialize, FromRow)]
struct FunnelStage {
    stage_id: Uuid,
    name: String,
    color: String,
    semantics: String,
    /// Records (created in the period) that got at least this far.
    reached: i64,
    reached_amount: Decimal,
    /// Records sitting in this stage now.
    current: i64,
}

/// For records created in the period in one pipeline: how many reached each open stage
/// (a won record reached all of them), then how many were won and lost.
async fn funnel(state: &AppState, b: &Base<'_>, category: Uuid, start: DateTime<Utc>, end: DateTime<Utc>) -> AppResult<Vec<FunnelStage>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "WITH d AS (SELECT i.id, i.stage_id, i.stage_semantics, i.opportunity_base FROM crm_items i WHERE",
    );
    b.push(&mut qb);
    qb.push(" AND i.category_id = ").push_bind(category);
    qb.push(" AND i.created_at >= ").push_bind(start).push(" AND i.created_at < ").push_bind(end);
    qb.push(
        "), r AS (SELECT d.id, d.stage_id, d.stage_semantics, d.opportunity_base,
                 COALESCE(MAX(s.sort) FILTER (WHERE s.semantics = 'process'), -2147483648) AS reached
            FROM d LEFT JOIN crm_stage_history h ON h.item_id = d.id LEFT JOIN crm_stages s ON s.id = h.stage_id
           GROUP BY d.id, d.stage_id, d.stage_semantics, d.opportunity_base)
        SELECT st.id AS stage_id, st.name, st.color, st.semantics,
               COUNT(r.id) FILTER (WHERE (st.semantics = 'process' AND (r.stage_semantics = 'success' OR r.reached >= st.sort))
                                      OR (st.semantics <> 'process' AND r.stage_id = st.id))::int8 AS reached,
               COALESCE(SUM(r.opportunity_base) FILTER (WHERE (st.semantics = 'process' AND (r.stage_semantics = 'success' OR r.reached >= st.sort))
                                      OR (st.semantics <> 'process' AND r.stage_id = st.id)), 0) AS reached_amount,
               COUNT(r.id) FILTER (WHERE r.stage_id = st.id)::int8 AS current
          FROM crm_stages st CROSS JOIN r
         WHERE st.category_id = ",
    );
    qb.push_bind(category);
    qb.push(" GROUP BY st.id, st.name, st.color, st.semantics, st.sort ORDER BY (st.semantics <> 'process'), (st.semantics = 'failure'), st.sort");
    let rows: Vec<FunnelStage> = qb.build_query_as().fetch_all(&state.db).await?;
    if !rows.is_empty() {
        return Ok(rows);
    }
    // No records: still show the stages, with zeros.
    Ok(sqlx::query_as(
        "SELECT id AS stage_id, name, color, semantics, 0::int8 AS reached, 0::numeric AS reached_amount, 0::int8 AS current
           FROM crm_stages WHERE category_id = $1 ORDER BY (semantics <> 'process'), (semantics = 'failure'), sort",
    )
    .bind(category)
    .fetch_all(&state.db)
    .await?)
}

#[derive(FromRow)]
struct BucketRow {
    b: NaiveDate,
    won_count: i64,
    won_amount: Decimal,
    new_count: i64,
}

async fn dynamics(state: &AppState, b: &Base<'_>, tz: Tz, bucket: Bucket, from: NaiveDate, to: NaiveDate, start: DateTime<Utc>, end: DateTime<Utc>) -> AppResult<Vec<Value>> {
    let tzname = tz.name().to_string();
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT x.b, SUM(x.won)::int8 AS won_count, COALESCE(SUM(x.amount), 0) AS won_amount, SUM(x.new)::int8 AS new_count FROM (");
    // Won, by close date …
    qb.push("SELECT date_trunc(").push_bind(bucket.sql()).push(", i.closed_at AT TIME ZONE ").push_bind(tzname.clone());
    qb.push(")::date AS b, 1 AS won, i.opportunity_base AS amount, 0 AS new FROM crm_items i WHERE");
    b.push(&mut qb);
    qb.push(" AND i.stage_semantics = 'success' AND i.closed_at >= ").push_bind(start).push(" AND i.closed_at < ").push_bind(end);
    // … and created, by creation date.
    qb.push(" UNION ALL SELECT date_trunc(").push_bind(bucket.sql()).push(", i.created_at AT TIME ZONE ").push_bind(tzname);
    qb.push(")::date AS b, 0, 0, 1 FROM crm_items i WHERE");
    b.push(&mut qb);
    qb.push(" AND i.created_at >= ").push_bind(start).push(" AND i.created_at < ").push_bind(end);
    qb.push(") x GROUP BY x.b");
    let rows: Vec<BucketRow> = qb.build_query_as().fetch_all(&state.db).await?;
    let by: HashMap<NaiveDate, BucketRow> = rows.into_iter().map(|r| (r.b, r)).collect();
    Ok(bucket
        .series(from, to)
        .into_iter()
        .map(|d| {
            let r = by.get(&d);
            json!({
                "start": d,
                "won_count": r.map_or(0, |r| r.won_count),
                "won_amount": r.map_or(Decimal::ZERO, |r| r.won_amount),
                "new_count": r.map_or(0, |r| r.new_count),
            })
        })
        .collect())
}

#[derive(Serialize, FromRow)]
struct Manager {
    user_id: Option<Uuid>,
    name: Option<String>,
    avatar_file_id: Option<Uuid>,
    won_count: i64,
    won_amount: Decimal,
    lost_count: i64,
    new_count: i64,
    open_count: i64,
    open_amount: Decimal,
    activities_done: i64,
    #[sqlx(skip)]
    target: Option<Decimal>,
    #[sqlx(skip)]
    target_won: Decimal,
}

async fn managers(state: &AppState, b: &Base<'_>, start: DateTime<Utc>, end: DateTime<Utc>) -> AppResult<Vec<Manager>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("WITH m AS (SELECT i.assigned_to,");
    qb.push(" COUNT(*) FILTER (WHERE i.stage_semantics = 'success' AND i.closed_at >= ").push_bind(start).push(" AND i.closed_at < ").push_bind(end).push(")::int8 AS won_count,");
    qb.push(" COALESCE(SUM(i.opportunity_base) FILTER (WHERE i.stage_semantics = 'success' AND i.closed_at >= ").push_bind(start).push(" AND i.closed_at < ").push_bind(end).push("), 0) AS won_amount,");
    qb.push(" COUNT(*) FILTER (WHERE i.stage_semantics = 'failure' AND i.closed_at >= ").push_bind(start).push(" AND i.closed_at < ").push_bind(end).push(")::int8 AS lost_count,");
    qb.push(" COUNT(*) FILTER (WHERE i.created_at >= ").push_bind(start).push(" AND i.created_at < ").push_bind(end).push(")::int8 AS new_count,");
    qb.push(" COUNT(*) FILTER (WHERE NOT i.is_closed)::int8 AS open_count, COALESCE(SUM(i.opportunity_base) FILTER (WHERE NOT i.is_closed), 0) AS open_amount");
    qb.push(" FROM crm_items i WHERE");
    b.push(&mut qb);
    qb.push(" AND (NOT i.is_closed OR (i.closed_at >= ").push_bind(start).push(" AND i.closed_at < ").push_bind(end);
    qb.push(") OR (i.created_at >= ").push_bind(start).push(" AND i.created_at < ").push_bind(end).push("))");
    qb.push(" GROUP BY i.assigned_to), a AS (SELECT act.responsible_id, COUNT(*)::int8 AS n FROM crm_activities act JOIN crm_items i ON i.id = act.item_id WHERE");
    b.push(&mut qb);
    qb.push(" AND act.is_completed AND act.completed_at >= ").push_bind(start).push(" AND act.completed_at < ").push_bind(end);
    qb.push(
        " GROUP BY act.responsible_id)
        SELECT m.assigned_to AS user_id, NULLIF(trim(u.first_name || ' ' || u.last_name), '') AS name, u.avatar_file_id,
               m.won_count, m.won_amount, m.lost_count, m.new_count, m.open_count, m.open_amount, COALESCE(a.n, 0)::int8 AS activities_done
          FROM m LEFT JOIN users u ON u.id = m.assigned_to LEFT JOIN a ON a.responsible_id = m.assigned_to
         ORDER BY m.won_amount DESC, m.won_count DESC, m.open_amount DESC, name ASC LIMIT 200",
    );
    Ok(qb.build_query_as().fetch_all(&state.db).await?)
}

#[derive(Serialize, FromRow)]
struct Source {
    source: String,
    created: i64,
    won_count: i64,
    won_amount: Decimal,
}

/// Records created in the period, by source, with how many of them are won by now.
async fn sources(state: &AppState, b: &Base<'_>, start: DateTime<Utc>, end: DateTime<Utc>) -> AppResult<Vec<Source>> {
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT COALESCE(NULLIF(i.source, ''), 'none') AS source, COUNT(*)::int8 AS created,
                COUNT(*) FILTER (WHERE i.stage_semantics = 'success')::int8 AS won_count,
                COALESCE(SUM(i.opportunity_base) FILTER (WHERE i.stage_semantics = 'success'), 0) AS won_amount
           FROM crm_items i WHERE",
    );
    b.push(&mut qb);
    qb.push(" AND i.created_at >= ").push_bind(start).push(" AND i.created_at < ").push_bind(end);
    qb.push(" GROUP BY 1 ORDER BY created DESC LIMIT 20");
    Ok(qb.build_query_as().fetch_all(&state.db).await?)
}

#[derive(Serialize, FromRow)]
struct LeadConversion {
    created: i64,
    converted: i64,
    junk: i64,
    open: i64,
}

/// Leads created in the period and what became of them. None when leads aren't readable.
async fn lead_conversion(state: &AppState, auth: &AuthUser, users: &[Uuid], start: DateTime<Utc>, end: DateTime<Utc>) -> AppResult<Option<LeadConversion>> {
    let lead = entity_type_by_id(state, 1).await?;
    let scope = visible(state, auth, lead.id, "read").await?;
    if matches!(scope, Visible::Nothing) {
        return Ok(None);
    }
    let b = Base { t: &lead, category: None, users: users.to_vec(), scope: &scope };
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT COUNT(*)::int8 AS created, COUNT(*) FILTER (WHERE i.stage_semantics = 'success')::int8 AS converted,
                COUNT(*) FILTER (WHERE i.stage_semantics = 'failure')::int8 AS junk,
                COUNT(*) FILTER (WHERE i.stage_semantics = 'process')::int8 AS open
           FROM crm_items i WHERE",
    );
    b.push(&mut qb);
    qb.push(" AND i.created_at >= ").push_bind(start).push(" AND i.created_at < ").push_bind(end);
    Ok(Some(qb.build_query_as().fetch_one(&state.db).await?))
}

fn parse_users(s: &Option<String>) -> AppResult<Vec<Uuid>> {
    let Some(s) = s.as_deref().filter(|s| !s.trim().is_empty()) else { return Ok(Vec::new()) };
    let v: Vec<Uuid> = s.split(',').map(|x| Uuid::parse_str(x.trim()).map_err(|_| AppError::bad("users must be user ids"))).collect::<Result<_, _>>()?;
    if v.len() > 200 {
        return Err(AppError::bad("too many users"));
    }
    Ok(v)
}

/// Types that have stages and amounts: deals and money-carrying smart processes.
async fn report_type(state: &AppState, code: &str) -> AppResult<EntityType> {
    let t = entity_type(state, code).await?;
    if !t.has_stages || !t.has_amount {
        return Err(AppError::bad("analytics is available for record types with stages and amounts"));
    }
    Ok(t)
}

fn month_start(d: NaiveDate) -> NaiveDate {
    d.with_day(1).expect("day 1")
}

async fn report(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Query(q): Query<ReportQuery>) -> AppResult<Json<Value>> {
    let t = report_type(&state, &code).await?;
    let scope = visible(&state, &auth, t.id, "read").await?;
    if matches!(scope, Visible::Nothing) {
        return Err(AppError::Forbidden);
    }
    let tz = user_tz(&state, &auth).await;
    let today = Utc::now().with_timezone(&tz).date_naive();
    // Default: this month so far.
    let from = q.from.unwrap_or_else(|| month_start(today));
    let to = q.to.unwrap_or(today);
    if to < from {
        return Err(AppError::bad("the period ends before it starts"));
    }
    let days = (to - from).num_days() + 1;
    if days > MAX_DAYS {
        return Err(AppError::bad("the period can be at most three years"));
    }
    let (start, end) = (day_start(tz, from), day_start(tz, to + Duration::days(1)));
    let (prev_from, prev_to) = (from - Duration::days(days), from - Duration::days(1));
    let (prev_start, prev_end) = (day_start(tz, prev_from), start);

    if let Some(c) = q.category_id {
        let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_categories WHERE id = $1 AND entity_type = $2)")
            .bind(c)
            .bind(t.id)
            .fetch_one(&state.db)
            .await?;
        if !ok {
            return Err(AppError::bad("that pipeline belongs to another record type"));
        }
    }
    let users = parse_users(&q.users)?;
    let b = Base { t: &t, category: q.category_id, users: users.clone(), scope: &scope };

    let bucket = Bucket::for_days(days);
    let current = kpi(&state, &b, start, end).await?;
    let previous = kpi(&state, &b, prev_start, prev_end).await?;

    // Open pipeline right now (not tied to the period).
    let mut oq: QueryBuilder<Postgres> = QueryBuilder::new("SELECT COUNT(*)::int8, COALESCE(SUM(i.opportunity_base), 0) FROM crm_items i WHERE");
    b.push(&mut oq);
    oq.push(" AND NOT i.is_closed");
    let (open_count, open_amount): (i64, Decimal) = oq.build_query_as().fetch_one(&state.db).await?;

    // The funnel needs one pipeline: the chosen one, or the only one.
    let funnel_category = match q.category_id {
        Some(c) => Some(c),
        None => {
            let cats: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM crm_categories WHERE entity_type = $1 LIMIT 2").bind(t.id).fetch_all(&state.db).await?;
            if cats.len() == 1 { cats.first().copied() } else { None }
        }
    };
    let funnel = match funnel_category {
        Some(c) => Some(funnel(&state, &b, c, start, end).await?),
        None => None,
    };

    let mut managers = managers(&state, &b, start, end).await?;
    // Targets for the month the period ends in.
    let month = month_start(to);
    let targets = targets_for(&state, &t, &scope, &users, q.category_id, month, tz).await?;
    for m in &mut managers {
        if let Some(u) = m.user_id
            && let Some(x) = targets.users.iter().find(|x| x.user_id == u)
        {
            m.target = x.target;
            m.target_won = x.won;
        }
    }
    let lead = if t.id == 2 { lead_conversion(&state, &auth, &users, start, end).await? } else { None };

    Ok(Json(json!({
        "type": t.code,
        "currency": base_currency(&state).await?,
        "timezone": tz.name(),
        "period": { "from": from, "to": to, "days": days, "prev_from": prev_from, "prev_to": prev_to, "bucket": bucket.name() },
        "kpi": {
            "current": current,
            "previous": previous,
            "open_count": open_count,
            "open_amount": open_amount,
        },
        "funnel_category_id": funnel_category,
        "funnel": funnel,
        "dynamics": dynamics(&state, &b, tz, bucket, from, to, start, end).await?,
        "managers": managers,
        "sources": sources(&state, &b, start, end).await?,
        "lead_conversion": lead,
        "targets": targets,
        "can_edit_targets": is_crm_admin(&auth),
    })))
}

// ---------------------------------------------------------------------------
// Sales targets
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct UserTarget {
    user_id: Uuid,
    name: String,
    target: Option<Decimal>,
    /// Won in the target month, in the base currency.
    won: Decimal,
}

#[derive(Serialize)]
struct Targets {
    month: NaiveDate,
    /// The whole team's target, and what everyone in scope won that month.
    team_target: Option<Decimal>,
    team_won: Decimal,
    users: Vec<UserTarget>,
}

async fn targets_for(state: &AppState, t: &EntityType, scope: &Visible, users: &[Uuid], category: Option<Uuid>, month: NaiveDate, tz: Tz) -> AppResult<Targets> {
    let (start, end) = (day_start(tz, month), day_start(tz, month + Months::new(1)));
    let rows: Vec<(Option<Uuid>, Decimal)> = sqlx::query_as("SELECT user_id, amount FROM crm_sales_targets WHERE entity_type = $1 AND month = $2")
        .bind(t.id)
        .bind(month)
        .fetch_all(&state.db)
        .await?;
    let team_target = rows.iter().find(|r| r.0.is_none()).map(|r| r.1);
    let b = Base { t, category, users: users.to_vec(), scope };
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new("SELECT i.assigned_to, COALESCE(SUM(i.opportunity_base), 0) FROM crm_items i WHERE");
    b.push(&mut qb);
    qb.push(" AND i.stage_semantics = 'success' AND i.closed_at >= ").push_bind(start).push(" AND i.closed_at < ").push_bind(end);
    qb.push(" GROUP BY i.assigned_to");
    let won: Vec<(Option<Uuid>, Decimal)> = qb.build_query_as().fetch_all(&state.db).await?;
    let team_won = won.iter().map(|w| w.1).sum();

    // People with a target, plus people who won something that month.
    let mut ids: Vec<Uuid> = rows.iter().filter_map(|r| r.0).chain(won.iter().filter_map(|w| w.0)).collect();
    if !users.is_empty() {
        ids.retain(|u| users.contains(u));
    }
    ids.sort();
    ids.dedup();
    let names: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, trim(first_name || ' ' || last_name) FROM users WHERE id = ANY($1)").bind(&ids).fetch_all(&state.db).await?;
    let mut list: Vec<UserTarget> = names
        .into_iter()
        .map(|(id, name)| UserTarget {
            user_id: id,
            name,
            target: rows.iter().find(|r| r.0 == Some(id)).map(|r| r.1),
            won: won.iter().find(|w| w.0 == Some(id)).map_or(Decimal::ZERO, |w| w.1),
        })
        .collect();
    list.sort_by(|a, b| b.won.cmp(&a.won).then_with(|| a.name.cmp(&b.name)));
    Ok(Targets { month, team_target, team_won, users: list })
}

#[derive(Deserialize)]
struct MonthQuery {
    /// YYYY-MM
    month: Option<String>,
}

fn parse_month(s: Option<&str>, today: NaiveDate) -> AppResult<NaiveDate> {
    match s.filter(|s| !s.is_empty()) {
        None => Ok(month_start(today)),
        Some(s) => NaiveDate::parse_from_str(&format!("{s}-01"), "%Y-%m-%d").map_err(|_| AppError::bad("month must look like 2026-11")),
    }
}

/// The targets of one month, for editing: every active person, with their target if set.
async fn get_targets(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Query(q): Query<MonthQuery>) -> AppResult<Json<Value>> {
    let t = report_type(&state, &code).await?;
    if !is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    let tz = user_tz(&state, &auth).await;
    let month = parse_month(q.month.as_deref(), Utc::now().with_timezone(&tz).date_naive())?;
    let rows: Vec<(Option<Uuid>, Decimal)> = sqlx::query_as("SELECT user_id, amount FROM crm_sales_targets WHERE entity_type = $1 AND month = $2")
        .bind(t.id)
        .bind(month)
        .fetch_all(&state.db)
        .await?;
    let people: Vec<(Uuid, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, trim(first_name || ' ' || last_name), avatar_file_id FROM users WHERE is_active AND NOT is_system ORDER BY first_name, last_name LIMIT 1000",
    )
    .fetch_all(&state.db)
    .await?;
    let users: Vec<Value> = people
        .into_iter()
        .map(|(id, name, avatar)| json!({ "user_id": id, "name": name, "avatar_file_id": avatar, "target": rows.iter().find(|r| r.0 == Some(id)).map(|r| r.1) }))
        .collect();
    Ok(Json(json!({
        "month": month,
        "currency": base_currency(&state).await?,
        "team_target": rows.iter().find(|r| r.0.is_none()).map(|r| r.1),
        "users": users,
    })))
}

#[derive(Deserialize)]
struct TargetRow {
    user_id: Uuid,
    target: Option<Decimal>,
}

#[derive(Deserialize)]
struct PutTargets {
    month: String,
    team_target: Option<Decimal>,
    users: Vec<TargetRow>,
}

/// Replaces one month's targets. A missing or empty amount removes that target.
async fn put_targets(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Json(b): Json<PutTargets>) -> AppResult<Json<Value>> {
    let t = report_type(&state, &code).await?;
    if !is_crm_admin(&auth) {
        return Err(AppError::Forbidden);
    }
    let month = parse_month(Some(&b.month), Utc::now().date_naive())?;
    if b.users.len() > 1000 {
        return Err(AppError::bad("too many people"));
    }
    let max = Decimal::new(1, 0) * Decimal::from(10_i64.pow(15));
    let check = |v: &Option<Decimal>| -> AppResult<()> {
        match v {
            Some(x) if x.is_sign_negative() => Err(AppError::bad("targets can't be negative")),
            Some(x) if *x >= max => Err(AppError::bad("that target is too large")),
            _ => Ok(()),
        }
    };
    check(&b.team_target)?;
    for r in &b.users {
        check(&r.target)?;
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM crm_sales_targets WHERE entity_type = $1 AND month = $2").bind(t.id).bind(month).execute(&mut *tx).await?;
    let entries = std::iter::once((None, b.team_target)).chain(b.users.iter().map(|r| (Some(r.user_id), r.target)));
    for (user, amount) in entries {
        let Some(amount) = amount.filter(|a| !a.is_zero()) else { continue };
        sqlx::query("INSERT INTO crm_sales_targets (entity_type, user_id, month, amount, updated_by) VALUES ($1, $2, $3, $4, $5)")
            .bind(t.id)
            .bind(user)
            .bind(month)
            .bind(amount.round_dp(4))
            .bind(auth.id)
            .execute(&mut *tx)
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(d) if d.is_foreign_key_violation() => AppError::bad("unknown user"),
                _ => e.into(),
            })?;
    }
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Gross profit and margin
// ---------------------------------------------------------------------------

/// Most won records one profit report reads.
const PROFIT_CAP: i64 = 50_000;

#[derive(Default, Clone)]
struct Pl {
    /// Net sales (after discounts, without tax) of the rows whose cost is known.
    revenue: Decimal,
    cost: Decimal,
    /// Net sales of rows whose cost isn't known (left out of profit and margin).
    uncosted: Decimal,
    deals: i64,
}

impl Pl {
    fn add(&mut self, o: &Pl) {
        self.revenue += o.revenue;
        self.cost += o.cost;
        self.uncosted += o.uncosted;
        self.deals += o.deals;
    }
    fn json(&self) -> Value {
        let profit = self.revenue - self.cost;
        let margin = if self.revenue.is_zero() { None } else { Some((profit * Decimal::ONE_HUNDRED / self.revenue).round_dp(1)) };
        json!({
            "revenue": self.revenue.round_dp(2).to_string(),
            "cost": self.cost.round_dp(2).to_string(),
            "profit": profit.round_dp(2).to_string(),
            "margin": margin.map(|m| m.to_string()),
            "uncosted_revenue": self.uncosted.round_dp(2).to_string(),
            "deals": self.deals,
        })
    }
}

#[derive(FromRow)]
struct ItemRow {
    item_id: Uuid,
    #[sqlx(flatten)]
    row: super::money::ProductRow,
}

#[derive(FromRow)]
struct WonDeal {
    id: Uuid,
    number: i64,
    title: String,
    assigned_to: Option<Uuid>,
    assigned_name: Option<String>,
    month: String,
    opportunity: Decimal,
    opportunity_base: Decimal,
}

/// Gross profit and margin of the records won in the period, from their product rows' prices
/// and costs, in the base currency: in total, by manager, by product, by month, and the
/// records themselves (most profitable first).
async fn profit(State(state): State<AppState>, auth: AuthUser, Path(code): Path<String>, Query(q): Query<ReportQuery>) -> AppResult<Json<Value>> {
    let t = report_type(&state, &code).await?;
    let scope = visible(&state, &auth, t.id, "read").await?;
    if matches!(scope, Visible::Nothing) {
        return Err(AppError::Forbidden);
    }
    let tz = user_tz(&state, &auth).await;
    let today = Utc::now().with_timezone(&tz).date_naive();
    let from = q.from.unwrap_or_else(|| month_start(today));
    let to = q.to.unwrap_or(today);
    if to < from {
        return Err(AppError::bad("the period ends before it starts"));
    }
    if (to - from).num_days() + 1 > MAX_DAYS {
        return Err(AppError::bad("the period can be at most three years"));
    }
    let (start, end) = (day_start(tz, from), day_start(tz, to + Duration::days(1)));
    let b = Base { t: &t, category: q.category_id, users: parse_users(&q.users)?, scope: &scope };

    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT i.id, i.number, i.title, i.assigned_to,
                CASE WHEN u.id IS NULL THEN NULL ELSE u.first_name || ' ' || u.last_name END AS assigned_name,
                to_char(i.closed_at AT TIME ZONE ",
    );
    qb.push_bind(tz.name().to_string()).push(", 'YYYY-MM') AS month, i.opportunity, i.opportunity_base
         FROM crm_items i LEFT JOIN users u ON u.id = i.assigned_to WHERE");
    b.push(&mut qb);
    qb.push(" AND i.stage_semantics = 'success' AND i.closed_at >= ").push_bind(start).push(" AND i.closed_at < ").push_bind(end);
    qb.push(" ORDER BY i.closed_at LIMIT ").push_bind(PROFIT_CAP + 1);
    let mut deals: Vec<WonDeal> = qb.build_query_as().fetch_all(&state.db).await?;
    let capped = deals.len() as i64 > PROFIT_CAP;
    deals.truncate(PROFIT_CAP as usize);

    let ids: Vec<Uuid> = deals.iter().map(|d| d.id).collect();
    let rows: Vec<(Uuid, super::money::ProductRow)> = {
        let r: Vec<ItemRow> = sqlx::query_as(
            "SELECT item_id, id, product_id, name, price, quantity, unit, discount_kind, discount_value, tax_rate, tax_included, cost
             FROM crm_item_products WHERE item_id = ANY($1) ORDER BY item_id, sort",
        )
        .bind(&ids)
        .fetch_all(&state.db)
        .await?;
        r.into_iter().map(|x| (x.item_id, x.row)).collect()
    };
    let mut by_item: HashMap<Uuid, Vec<super::money::ProductRow>> = HashMap::new();
    for (i, r) in rows {
        by_item.entry(i).or_default().push(r);
    }

    let mut total = Pl::default();
    let mut without_products = 0i64;
    let mut managers: HashMap<Option<Uuid>, (String, Pl)> = HashMap::new();
    let mut products: HashMap<String, (Option<Uuid>, String, Decimal, Pl)> = HashMap::new();
    let mut months: std::collections::BTreeMap<String, Pl> = std::collections::BTreeMap::new();
    let mut per_deal: Vec<(Value, Decimal)> = Vec::new();
    for d in &deals {
        let Some(rs) = by_item.get(&d.id) else {
            without_products += 1;
            continue;
        };
        // Into the base currency at the record's own rate.
        let factor = if d.opportunity.is_zero() { Decimal::ONE } else { d.opportunity_base / d.opportunity };
        let mut pl = Pl { deals: 1, ..Default::default() };
        for r in rs {
            let net = super::money::line(r).net * factor;
            let mut one = Pl::default();
            match r.cost {
                Some(c) => {
                    one.revenue = net;
                    one.cost = c * r.quantity * factor;
                }
                None => one.uncosted = net,
            }
            pl.add(&one);
            let key = r.product_id.map(|p| p.to_string()).unwrap_or_else(|| format!("name:{}", r.name.to_lowercase()));
            let e = products.entry(key).or_insert_with(|| (r.product_id, r.name.clone(), Decimal::ZERO, Pl::default()));
            e.2 += r.quantity;
            e.3.add(&one);
        }
        total.add(&pl);
        managers.entry(d.assigned_to).or_insert_with(|| (d.assigned_name.clone().unwrap_or_default(), Pl::default())).1.add(&pl);
        months.entry(d.month.clone()).or_default().add(&pl);
        let profit = pl.revenue - pl.cost;
        let mut v = pl.json();
        v["id"] = json!(d.id);
        v["number"] = json!(d.number);
        v["title"] = json!(d.title);
        v["assigned_name"] = json!(d.assigned_name);
        per_deal.push((v, profit));
    }
    // Products count each record once per product, not once per row.
    for (_, (_, _, _, pl)) in products.iter_mut() {
        pl.deals = 0;
    }
    let mut by_manager: Vec<(Decimal, Value)> = managers
        .into_iter()
        .map(|(id, (name, pl))| {
            let mut v = pl.json();
            v["user_id"] = json!(id);
            v["name"] = json!(name);
            (pl.revenue - pl.cost, v)
        })
        .collect();
    by_manager.sort_by(|a, b| b.0.cmp(&a.0));
    let mut by_product: Vec<(Decimal, Value)> = products
        .into_values()
        .map(|(id, name, qty, pl)| {
            let mut v = pl.json();
            v["product_id"] = json!(id);
            v["name"] = json!(name);
            v["quantity"] = json!(qty.normalize().to_string());
            (pl.revenue - pl.cost, v)
        })
        .collect();
    by_product.sort_by(|a, b| b.0.cmp(&a.0));
    by_product.truncate(200);
    per_deal.sort_by(|a, b| b.1.cmp(&a.1));
    per_deal.truncate(100);
    Ok(Json(json!({
        "period": { "from": from, "to": to },
        "base_currency": super::money::base_currency(&state).await?,
        "total": total.json(),
        "without_products": without_products,
        "capped": capped,
        "by_manager": by_manager.into_iter().map(|x| x.1).collect::<Vec<_>>(),
        "by_product": by_product.into_iter().map(|x| x.1).collect::<Vec<_>>(),
        "by_month": months.into_iter().map(|(m, pl)| { let mut v = pl.json(); v["month"] = json!(m); v }).collect::<Vec<_>>(),
        "deals": per_deal.into_iter().map(|x| x.0).collect::<Vec<_>>(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn buckets_cover_the_period() {
        assert_eq!(Bucket::for_days(30), Bucket::Day);
        assert_eq!(Bucket::for_days(90), Bucket::Week);
        assert_eq!(Bucket::for_days(365), Bucket::Month);
        assert_eq!(Bucket::Day.series(d("2026-11-01"), d("2026-11-03")).len(), 3);
        // 2026-11-04 is a Wednesday; weeks start on Monday 2026-11-02.
        assert_eq!(Bucket::Week.series(d("2026-11-04"), d("2026-11-20")), vec![d("2026-11-02"), d("2026-11-09"), d("2026-11-16")]);
        assert_eq!(Bucket::Month.series(d("2026-01-31"), d("2026-03-01")), vec![d("2026-01-01"), d("2026-02-01"), d("2026-03-01")]);
    }

    #[test]
    fn local_midnight() {
        let tz: Tz = "Asia/Kolkata".parse().unwrap();
        assert_eq!(day_start(tz, d("2026-11-08")).to_rfc3339(), "2026-11-07T18:30:00+00:00");
        // Spring-forward day still has a start.
        let ny: Tz = "America/Sao_Paulo".parse().unwrap();
        let _ = day_start(ny, d("2026-11-01"));
    }
}
