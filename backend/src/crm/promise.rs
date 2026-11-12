//! Promise checker: can we deliver this deal by the date we are about to promise?
//!
//! - **Goods:** each catalog product on the deal against its stock, less what other open deals
//!   due by then need of it, plus receipts already drafted with a date by then.
//! - **Work:** service rows measured in hours (or days) against the free working time of the
//!   people doing it (the deal's responsible person by default): working days (company work
//!   week or their rota, without holidays and approved time off), less their calendar's
//!   meetings and their open tasks due by then (estimate less time logged).
//! - **Absences:** someone on leave on the day itself, or pending leave in the period.
//!
//! The verdict is ok, risky (holds only if a receipt arrives or with little room to spare) or
//! no, with the reasons and the earliest date that would work.
use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use rust_decimal::{prelude::ToPrimitive, Decimal};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{access, DEAL};
use crate::{
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new().route("/crm/items/{id}/promise", get(check))
}

#[derive(Deserialize)]
struct Q {
    /// The date to check (default: the deal's close date).
    date: Option<NaiveDate>,
    /// Who does the work, comma-separated (default: the responsible person).
    users: Option<String>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Verdict {
    Ok,
    Risky,
    No,
}

const HOUR_UNITS: &[&str] = &["h", "hr", "hrs", "hour", "hours"];
const DAY_UNITS: &[&str] = &["d", "day", "days", "person-day", "man-day"];
/// How far ahead the earliest workable date is looked for.
const HORIZON_DAYS: i64 = 365;

async fn company_tz(state: &AppState) -> chrono_tz::Tz {
    let v: Option<Value> = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'default_timezone'").fetch_optional(&state.db).await.ok().flatten();
    v.and_then(|v| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(chrono_tz::UTC)
}

fn num(d: Decimal) -> f64 {
    d.to_f64().unwrap_or(0.0)
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

#[derive(sqlx::FromRow)]
struct Line {
    product_id: Option<Uuid>,
    name: String,
    quantity: Decimal,
    unit: Option<String>,
    is_service: Option<bool>,
}

/// One person's working hours per day over a range.
struct Person {
    id: Uuid,
    name: String,
    /// day → (hours, busy hours from the calendar)
    days: Vec<(NaiveDate, f64, f64)>,
    task_hours: f64,
    tasks_without_estimate: i64,
    absences: Vec<Value>,
}

async fn person(state: &AppState, tz: chrono_tz::Tz, id: Uuid, from: NaiveDate, to: NaiveDate, due: NaiveDate) -> AppResult<Option<Person>> {
    let name: Option<String> = sqlx::query_scalar("SELECT trim(first_name || ' ' || last_name) FROM users WHERE id = $1 AND is_active").bind(id).fetch_optional(&state.db).await?;
    let Some(name) = name else { return Ok(None) };
    let (weekdays, workday): (Vec<i32>, Decimal) = sqlx::query_as("SELECT work_weekdays, workday_hours FROM hr_settings LIMIT 1")
        .fetch_optional(&state.db)
        .await?
        .unwrap_or((vec![1, 2, 3, 4, 5], Decimal::from(8)));
    let workday = num(workday);
    let holidays: Vec<NaiveDate> = sqlx::query_scalar("SELECT day FROM hr_holidays WHERE day BETWEEN $1 AND $2").bind(from).bind(to).fetch_all(&state.db).await?;
    let rota: HashMap<NaiveDate, f64> = sqlx::query_as::<_, (NaiveDate, i32, i32, i32)>(
        "SELECT r.day, s.start_min, s.end_min, s.break_minutes FROM hr_rota r JOIN hr_shifts s ON s.id = r.shift_id WHERE r.user_id = $1 AND r.day BETWEEN $2 AND $3",
    )
    .bind(id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|(d, s, e, b)| {
        let len = if e > s { e - s } else { e + 1440 - s };
        (d, ((len - b).max(0)) as f64 / 60.0)
    })
    .collect();
    let absences: Vec<(String, NaiveDate, NaiveDate, String)> = sqlx::query_as(
        "SELECT kind, starts_on, ends_on, status FROM hr_absences WHERE user_id = $1 AND status IN ('approved', 'pending') AND starts_on <= $3 AND ends_on >= $2 ORDER BY starts_on",
    )
    .bind(id)
    .bind(from)
    .bind(to)
    .fetch_all(&state.db)
    .await?;
    let off = |d: NaiveDate| absences.iter().any(|(_, s, e, st)| st == "approved" && *s <= d && d <= *e);
    // Meetings: the person's own timed events (single occurrences).
    let start = tz.from_local_datetime(&from.and_hms_opt(0, 0, 0).expect("midnight")).earliest().map(|t| t.with_timezone(&Utc)).unwrap_or_else(Utc::now);
    let end = tz.from_local_datetime(&(to + Duration::days(1)).and_hms_opt(0, 0, 0).expect("midnight")).earliest().map(|t| t.with_timezone(&Utc)).unwrap_or_else(Utc::now);
    // Their meetings: events they own or were invited to (and didn't decline), each
    // occurrence of repeating ones, overlaps counted once.
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT e.id FROM cal_events e LEFT JOIN cal_attendees a ON a.event_id = e.id AND a.user_id = $1
         WHERE (e.owner_id = $1 OR (a.user_id IS NOT NULL AND a.status <> 'declined')) AND NOT e.all_day
           AND e.starts_at < $3 AND (e.recurrence IS NOT NULL OR e.ends_at > $2)
         LIMIT 5000",
    )
    .bind(id)
    .bind(start)
    .bind(end)
    .fetch_all(&state.db)
    .await?;
    let events = crate::calendar::load_events(state, &ids).await?;
    let ex = crate::calendar::exceptions(state, &ids).await?;
    let mut spans: Vec<(DateTime<Utc>, DateTime<Utc>)> = vec![];
    for (e, _) in &events {
        let excs: Vec<DateTime<Utc>> = ex.iter().filter(|(x, _)| *x == e.id).map(|(_, t)| *t).collect();
        let len = e.ends_at - e.starts_at;
        for s in crate::calendar::occurrences(e, &excs, start, end) {
            spans.push((s, s + len));
        }
    }
    spans.sort();
    let mut merged: Vec<(DateTime<Utc>, DateTime<Utc>)> = vec![];
    for (s, e) in spans {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    let mut busy: HashMap<NaiveDate, f64> = HashMap::new();
    for (s, e) in merged {
        let (s, e) = (s.with_timezone(&tz), e.with_timezone(&tz));
        let mut d = s.date_naive();
        while d <= e.date_naive() && d <= to {
            let ds = tz.from_local_datetime(&d.and_hms_opt(0, 0, 0).expect("midnight")).earliest().unwrap_or(s);
            let de = ds + Duration::days(1);
            let h = (e.min(de) - s.max(ds)).num_minutes().max(0) as f64 / 60.0;
            *busy.entry(d).or_default() += h;
            d += Duration::days(1);
        }
    }
    let mut days = vec![];
    let mut d = from;
    while d <= to {
        let iso = d.weekday().number_from_monday() as i32;
        let hours = if off(d) || holidays.contains(&d) {
            0.0
        } else if let Some(h) = rota.get(&d) {
            *h
        } else if weekdays.contains(&iso) {
            workday
        } else {
            0.0
        };
        let b = busy.get(&d).copied().unwrap_or(0.0).min(hours);
        days.push((d, hours, b));
        d += Duration::days(1);
    }
    // Open tasks due by the date: what is left of their estimate.
    let due_end = tz.from_local_datetime(&(due + Duration::days(1)).and_hms_opt(0, 0, 0).expect("midnight")).earliest().map(|t| t.with_timezone(&Utc)).unwrap_or_else(Utc::now);
    let (task_secs, no_estimate): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(GREATEST(t.time_estimate_secs - COALESCE((SELECT SUM(e.seconds) FROM task_time_entries e WHERE e.task_id = t.id), 0), 0)) FILTER (WHERE t.time_estimate_secs > 0), 0)::bigint,
                COUNT(*) FILTER (WHERE COALESCE(t.time_estimate_secs, 0) = 0)
         FROM tasks t WHERE t.responsible_id = $1 AND t.status NOT IN ('done', 'deferred') AND t.deadline IS NOT NULL AND t.deadline < $2",
    )
    .bind(id)
    .bind(due_end)
    .fetch_one(&state.db)
    .await?;
    Ok(Some(Person {
        id,
        name,
        days,
        task_hours: task_secs as f64 / 3600.0,
        tasks_without_estimate: no_estimate,
        absences: absences.into_iter().map(|(k, s, e, st)| json!({ "kind": k, "starts_on": s, "ends_on": e, "status": st })).collect(),
    }))
}

async fn check(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Query(q): Query<Q>) -> AppResult<Json<Value>> {
    let row = super::items::load_row(&state, id, false).await?;
    access::require_read(&state, &auth, row.entity_type, id, row.assigned_to).await?;
    if row.entity_type != DEAL {
        return Err(AppError::bad("the promise checker is for deals"));
    }
    let close: Option<NaiveDate> = sqlx::query_scalar("SELECT close_date FROM crm_items WHERE id = $1").bind(id).fetch_one(&state.db).await?;
    let tz = company_tz(&state).await;
    let today = Utc::now().with_timezone(&tz).date_naive();
    let Some(date) = q.date.or(close) else { return Err(AppError::bad("choose a date to check (the deal has no close date)")) };
    if date < today {
        return Err(AppError::bad("the date has passed"));
    }
    let mut users: Vec<Uuid> = q.users.as_deref().unwrap_or("").split(',').filter_map(|s| s.trim().parse().ok()).collect();
    if users.is_empty() {
        users.extend(row.assigned_to);
    }
    users.dedup();
    if users.len() > 20 {
        return Err(AppError::bad("at most 20 people"));
    }

    let lines: Vec<Line> = sqlx::query_as(
        "SELECT r.product_id, COALESCE(p.name, r.name) AS name, r.quantity, COALESCE(NULLIF(r.unit, ''), p.unit) AS unit, p.is_service
         FROM crm_item_products r LEFT JOIN crm_products p ON p.id = r.product_id WHERE r.item_id = $1 ORDER BY r.sort",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    // A day of work is the company's working day.
    let workday: f64 = sqlx::query_scalar::<_, Decimal>("SELECT workday_hours FROM hr_settings LIMIT 1").fetch_optional(&state.db).await?.map(num).unwrap_or(8.0);
    let mut reasons: Vec<String> = vec![];
    let mut verdict = Verdict::Ok;
    let mut earliest: Option<NaiveDate> = Some(date);
    let bump = |e: &mut Option<NaiveDate>, d: Option<NaiveDate>| {
        *e = match (*e, d) {
            (Some(a), Some(b)) => Some(a.max(b)),
            _ => None,
        }
    };

    // ---- goods ----
    let mut need: Vec<(Uuid, String, Decimal)> = vec![];
    let mut unlinked = vec![];
    let mut needed_hours = 0.0;
    let mut unknown_services = vec![];
    for l in &lines {
        let unit = l.unit.as_deref().unwrap_or("").trim().to_lowercase();
        let qty = num(l.quantity);
        if l.is_service == Some(true) || HOUR_UNITS.contains(&unit.as_str()) || DAY_UNITS.contains(&unit.as_str()) {
            if HOUR_UNITS.contains(&unit.as_str()) {
                needed_hours += qty;
            } else if DAY_UNITS.contains(&unit.as_str()) {
                needed_hours += qty * workday;
            } else {
                unknown_services.push(l.name.clone());
            }
            continue;
        }
        match l.product_id {
            Some(p) => match need.iter_mut().find(|x| x.0 == p) {
                Some(x) => x.2 += l.quantity,
                None => need.push((p, l.name.clone(), l.quantity)),
            },
            None => unlinked.push(l.name.clone()),
        }
    }
    let mut goods = vec![];
    for (pid, name, qty) in &need {
        let (tracked, on_hand): (bool, Option<Decimal>) = sqlx::query_as(
            "SELECT EXISTS (SELECT 1 FROM inv_stock WHERE product_id = $1),
                    COALESCE((SELECT SUM(quantity) FROM inv_stock WHERE product_id = $1), (SELECT stock FROM crm_products WHERE id = $1))",
        )
        .bind(pid)
        .fetch_one(&state.db)
        .await?;
        let Some(on_hand) = on_hand else {
            goods.push(json!({ "product_id": pid, "name": name, "needed": qty, "status": "unknown", "text": "Stock isn't tracked for this product" }));
            continue;
        };
        // Other open deals due by then that need it.
        let committed: Decimal = sqlx::query_scalar(
            "SELECT COALESCE(SUM(r.quantity), 0) FROM crm_item_products r JOIN crm_items i ON i.id = r.item_id JOIN crm_stages s ON s.id = i.stage_id
             WHERE r.product_id = $1 AND i.id <> $2 AND i.entity_type = 2 AND i.deleted_at IS NULL AND s.semantics = 'process' AND i.close_date IS NOT NULL AND i.close_date <= $3",
        )
        .bind(pid)
        .bind(id)
        .bind(date)
        .fetch_one(&state.db)
        .await?;
        // Receipts drafted with a date; those up to a year ahead count for the earliest date.
        let incoming: Vec<(NaiveDate, Decimal, String)> = sqlx::query_as(
            "SELECT GREATEST(d.doc_date, $2), SUM(l.quantity), d.number FROM inv_document_lines l JOIN inv_documents d ON d.id = l.document_id
             WHERE l.product_id = $1 AND d.kind = 'receipt' AND d.status = 'draft' AND d.doc_date IS NOT NULL AND d.doc_date <= $3
             GROUP BY 1, 3 ORDER BY 1",
        )
        .bind(pid)
        .bind(today)
        .bind(date + Duration::days(HORIZON_DAYS))
        .fetch_all(&state.db)
        .await?;
        let free = on_hand - committed;
        let by_date: Decimal = incoming.iter().filter(|x| x.0 <= date).map(|x| x.1).sum();
        let (status, text, ready) = if *qty <= free {
            (Verdict::Ok, format!("{} in stock, {} of it needed by other deals due by then", on_hand.normalize(), committed.normalize()), Some(today))
        } else {
            // The day the receipts bring enough.
            let mut have = free;
            let mut when = None;
            for (d, n, _) in &incoming {
                have += *n;
                if have >= *qty {
                    when = Some(*d);
                    break;
                }
            }
            let short = *qty - free;
            match when {
                Some(d) if d <= date => (Verdict::Risky, format!("Short by {} now; enough once receipt {} arrives ({})", short.normalize(), incoming.iter().find(|x| x.0 >= d).map(|x| x.2.as_str()).unwrap_or(""), d), Some(d)),
                Some(d) => (Verdict::No, format!("Short by {}; the receipts expected bring enough on {d}", short.normalize()), Some(d)),
                None => (Verdict::No, format!("Short by {} ({} in stock, {} needed by other deals due by then) and no receipt expected", short.normalize(), on_hand.normalize(), committed.normalize()), None),
            }
        };
        if status > Verdict::Ok {
            reasons.push(format!("{name}: {text}"));
        }
        verdict = verdict.max(status);
        bump(&mut earliest, ready);
        goods.push(json!({
            "product_id": pid, "name": name, "needed": qty, "on_hand": on_hand, "warehouse_tracked": tracked, "committed": committed,
            "free": free, "incoming": incoming.iter().map(|(d, n, no)| json!({ "date": d, "quantity": n, "document": no })).collect::<Vec<_>>(),
            "incoming_by_date": by_date, "status": status, "text": text,
        }));
    }

    // ---- people ----
    let horizon = date + Duration::days(HORIZON_DAYS);
    let last_work_day = date.pred_opt().unwrap_or(date).max(today);
    let mut people = vec![];
    let (mut free_total, mut free_by_day): (f64, HashMap<NaiveDate, f64>) = (0.0, HashMap::new());
    let mut task_total = 0.0;
    for u in &users {
        let Some(p) = person(&state, tz, *u, today, horizon, date).await? else { continue };
        let mut hours = 0.0;
        let mut busy = 0.0;
        let mut working = 0;
        for (d, h, b) in &p.days {
            *free_by_day.entry(*d).or_default() += h - b;
            if *d <= last_work_day && *d < date || (date == today && *d == today) {
                hours += h;
                busy += b;
                if *h > 0.0 {
                    working += 1;
                }
            }
        }
        let free = (hours - busy - p.task_hours).max(0.0);
        free_total += free;
        task_total += p.task_hours;
        let on_the_day = p.absences.iter().any(|a| a["status"] == json!("approved") && a["starts_on"].as_str() <= Some(&date.to_string()) && a["ends_on"].as_str() >= Some(&date.to_string()));
        if on_the_day {
            reasons.push(format!("{} is away on {date}", p.name));
            verdict = verdict.max(Verdict::Risky);
        }
        if p.absences.iter().any(|a| a["status"] == json!("pending")) {
            reasons.push(format!("{} has time off waiting for approval in this period", p.name));
        }
        people.push(json!({
            "id": p.id, "name": p.name, "working_days": working, "hours": round1(hours), "meeting_hours": round1(busy),
            "task_hours": round1(p.task_hours), "tasks_without_estimate": p.tasks_without_estimate,
            "absences": p.absences.into_iter().filter(|a| a["starts_on"].as_str() <= Some(&date.to_string())).collect::<Vec<_>>(),
            "free_hours": round1(free), "away_on_the_day": on_the_day,
        }));
    }
    let work_status = if needed_hours <= 0.0 {
        Verdict::Ok
    } else if users.is_empty() || people.is_empty() {
        reasons.push("No one is set to do the work".into());
        Verdict::No
    } else if free_total >= needed_hours * 1.2 {
        Verdict::Ok
    } else if free_total >= needed_hours {
        reasons.push(format!("{} hours of work against {} hours free: little room to spare", round1(needed_hours), round1(free_total)));
        Verdict::Risky
    } else {
        reasons.push(format!("{} hours of work but only {} hours free by then (after {} hours of meetings and tasks due)", round1(needed_hours), round1(free_total), round1(task_total)));
        Verdict::No
    };
    verdict = verdict.max(work_status);
    if needed_hours > 0.0 && work_status == Verdict::No {
        // Keep adding the free hours of the days after until the work fits.
        let mut have = free_total;
        let mut d = date;
        let mut when = None;
        while d < horizon {
            have += free_by_day.get(&d).copied().unwrap_or(0.0).max(0.0);
            d += Duration::days(1);
            // With the same room to spare that counts as "can be done".
            if have >= needed_hours * 1.2 {
                when = Some(d);
                break;
            }
        }
        bump(&mut earliest, when);
    }
    if !unknown_services.is_empty() {
        reasons.push(format!("Not measured in hours or days, so not checked: {}", unknown_services.join(", ")));
    }
    if !unlinked.is_empty() {
        reasons.push(format!("Not catalog products, so stock isn't checked: {}", unlinked.join(", ")));
    }
    Ok(Json(json!({
        "date": date,
        "today": today,
        "verdict": verdict,
        "earliest": if verdict == Verdict::Ok { Some(date) } else { earliest.filter(|e| *e > date || verdict == Verdict::Risky) },
        "reasons": reasons,
        "goods": goods,
        "work": { "needed_hours": round1(needed_hours), "free_hours": round1(free_total), "status": work_status, "unknown_services": unknown_services },
        "people": people,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_order() {
        assert!(Verdict::No > Verdict::Risky && Verdict::Risky > Verdict::Ok);
        assert_eq!(round1(2.34), 2.3);
    }
}
