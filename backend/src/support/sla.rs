//! Service-level agreements: which one a ticket falls under, when support's reply is due
//! (counted in the agreement's working hours, skipping company holidays), and the job that
//! warns before a deadline, marks overdue tickets and closes tickets the customer abandoned.

use std::collections::{BTreeMap, HashSet};

use anyhow::Result;
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    error::AppResult,
    notify::{notify, NewNotification},
    state::AppState,
};

pub const JOB: &str = "support.sla";

/// Working hours: ISO weekday (1 = Monday) → [from, till) minutes of the day.
pub type Hours = BTreeMap<u32, Vec<(i64, i64)>>;

/// Reads and checks the `hours` JSON of an SLA: {"1": [[540, 1080]], …}.
pub fn parse_hours(v: &Value) -> Result<Hours, String> {
    let obj = v.as_object().ok_or("working hours must be an object of weekdays")?;
    let mut out = Hours::new();
    for (k, spans) in obj {
        let day: u32 = k.parse().ok().filter(|d| (1..=7).contains(d)).ok_or("weekdays are numbered 1 (Monday) to 7 (Sunday)")?;
        let mut list = Vec::new();
        for s in spans.as_array().ok_or("each weekday holds a list of [from, till] minutes")? {
            let pair = s.as_array().filter(|p| p.len() == 2).ok_or("a span is [from, till] in minutes of the day")?;
            let (f, t) = (pair[0].as_i64().ok_or("minutes are whole numbers")?, pair[1].as_i64().ok_or("minutes are whole numbers")?);
            if !(0..=1440).contains(&f) || !(0..=1440).contains(&t) || f >= t {
                return Err("a span runs from 0 to 1440 minutes, and starts before it ends".into());
            }
            list.push((f, t));
        }
        list.sort_unstable();
        if list.windows(2).any(|w| w[0].1 > w[1].0) {
            return Err("the working hours of a day overlap".into());
        }
        if !list.is_empty() {
            out.insert(day, list);
        }
    }
    Ok(out)
}

pub fn hours_json(h: &Hours) -> Value {
    Value::Object(h.iter().map(|(d, spans)| (d.to_string(), serde_json::json!(spans.iter().map(|(f, t)| [f, t]).collect::<Vec<_>>()))).collect())
}

fn local(tz: Tz, day: NaiveDate, minute: i64) -> DateTime<Utc> {
    let naive = day.and_time(NaiveTime::MIN) + Duration::minutes(minute);
    // A time that does not exist (clocks jump forward) takes the next one that does.
    tz.from_local_datetime(&naive)
        .earliest()
        .or_else(|| tz.from_local_datetime(&(naive + Duration::hours(1))).earliest())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|| Utc.from_utc_datetime(&naive))
}

/// The moment `minutes` working minutes after `start`. None when the hours never open
/// (no working time within two years).
pub fn add_working_minutes(start: DateTime<Utc>, minutes: i64, tz: Tz, hours: &Hours, holidays: &HashSet<NaiveDate>) -> Option<DateTime<Utc>> {
    let mut left = minutes.max(0);
    if left == 0 {
        return Some(start);
    }
    let first = start.with_timezone(&tz).date_naive();
    for offset in 0..800 {
        let day = first + Duration::days(offset);
        if holidays.contains(&day) {
            continue;
        }
        let Some(spans) = hours.get(&day.weekday().number_from_monday()) else { continue };
        for &(f, t) in spans {
            let (open, close) = (local(tz, day, f).max(start), local(tz, day, t));
            if close <= open {
                continue;
            }
            let avail = (close - open).num_minutes();
            if avail >= left {
                return Some(open + Duration::minutes(left));
            }
            left -= avail;
        }
    }
    None
}

pub struct Agreement {
    pub response_minutes: i32,
    pub notice_minutes: i32,
    pub hours: Hours,
    pub skip_holidays: bool,
}

pub async fn load(state: &AppState, id: Uuid) -> AppResult<Option<Agreement>> {
    let row: Option<(i32, i32, Value, bool)> =
        sqlx::query_as("SELECT response_minutes, notice_minutes, hours, skip_holidays FROM support_slas WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    Ok(row.map(|(response_minutes, notice_minutes, hours, skip_holidays)| Agreement {
        response_minutes,
        notice_minutes,
        hours: parse_hours(&hours).unwrap_or_default(),
        skip_holidays,
    }))
}

/// The agreement a ticket falls under: the matching one with the highest priority, else
/// the default.
pub async fn choose(state: &AppState, category: Option<Uuid>, criticality: Option<Uuid>, staff: bool) -> AppResult<Option<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM support_slas
         WHERE is_default
            OR ((cardinality(category_ids) = 0 OR $1 = ANY(category_ids))
                AND (cardinality(criticality_ids) = 0 OR $2 = ANY(criticality_ids))
                AND (requesters = 'any' OR requesters = CASE WHEN $3 THEN 'staff' ELSE 'external' END))
         ORDER BY is_default ASC, priority DESC, created_at ASC LIMIT 1",
    )
    .bind(category)
    .bind(criticality)
    .bind(staff)
    .fetch_optional(&state.db)
    .await?)
}

pub async fn timezone(state: &AppState) -> AppResult<Tz> {
    let tz: String = sqlx::query_scalar("SELECT timezone FROM support_settings WHERE id").fetch_one(&state.db).await?;
    Ok(tz.parse().unwrap_or(chrono_tz::UTC))
}

/// When a reply is due for a customer message that came at `since`, and when to warn.
pub async fn due(state: &AppState, sla: Option<Uuid>, since: DateTime<Utc>) -> AppResult<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> {
    let Some(a) = (match sla {
        Some(id) => load(state, id).await?,
        None => None,
    }) else {
        return Ok((None, None));
    };
    let tz = timezone(state).await?;
    let holidays: HashSet<NaiveDate> = if a.skip_holidays {
        sqlx::query_scalar::<_, NaiveDate>("SELECT day FROM hr_holidays WHERE day >= $1")
            .bind(since.date_naive() - Duration::days(1))
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .collect()
    } else {
        HashSet::new()
    };
    let deadline = add_working_minutes(since, a.response_minutes as i64, tz, &a.hours, &holidays);
    let notice = if a.notice_minutes > 0 && a.notice_minutes < a.response_minutes {
        add_working_minutes(since, (a.response_minutes - a.notice_minutes) as i64, tz, &a.hours, &holidays)
    } else {
        None
    };
    Ok((deadline, notice))
}

/// Every minute: warnings before deadlines, overdue tickets, and closing tickets that have
/// waited too long for the customer.
pub async fn run_job(state: &AppState) -> Result<()> {
    let warn: Vec<(Uuid, i64, String, Option<Uuid>, Option<Uuid>, DateTime<Utc>)> = sqlx::query_as(
        "UPDATE support_tickets t SET notice_sent = TRUE FROM support_tickets x LEFT JOIN support_slas s ON s.id = x.sla_id
         WHERE x.id = t.id AND t.closed_at IS NULL AND NOT t.notice_sent AND NOT t.is_overdue AND t.notice_at <= now() AND t.deadline_at IS NOT NULL
         RETURNING t.id, t.number, t.title, t.responsible_id, s.responsible_id, t.deadline_at",
    )
    .fetch_all(&state.db)
    .await?;
    for (id, number, title, responsible, sla_responsible, deadline) in warn {
        let to: Vec<Uuid> = responsible.or(sla_responsible).into_iter().collect();
        notify(state, &to, NewNotification {
            kind: "support.due_soon",
            title: format!("Reply due soon on #{number}: {title}"),
            body: Some(format!("Due {}", deadline.format("%Y-%m-%d %H:%M UTC"))),
            link: Some(format!("/support/tickets/{id}")),
            actor: None,
        })
        .await;
    }

    let overdue: Vec<(Uuid, i64, String, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "UPDATE support_tickets t SET is_overdue = TRUE, overdue_count = t.overdue_count + 1, notice_sent = TRUE
         FROM support_tickets x LEFT JOIN support_slas s ON s.id = x.sla_id
         WHERE x.id = t.id AND t.closed_at IS NULL AND NOT t.is_overdue AND t.deadline_at <= now()
         RETURNING t.id, t.number, t.title, t.responsible_id, s.responsible_id",
    )
    .fetch_all(&state.db)
    .await?;
    for (id, number, title, responsible, sla_responsible) in overdue {
        let mut to: Vec<Uuid> = responsible.into_iter().collect();
        if let Some(s) = sla_responsible.filter(|s| !to.contains(s)) {
            to.push(s);
        }
        notify(state, &to, NewNotification {
            kind: "support.overdue",
            title: format!("Overdue: #{number} {title}"),
            body: Some("The customer is still waiting for a reply.".into()),
            link: Some(format!("/support/tickets/{id}")),
            actor: None,
        })
        .await;
    }

    let days: i32 = sqlx::query_scalar("SELECT auto_close_days FROM support_settings WHERE id").fetch_one(&state.db).await?;
    if days > 0 {
        let stale: Vec<Uuid> = sqlx::query_scalar(
            "UPDATE support_tickets SET closed_at = now(), auto_closed = TRUE, deadline_at = NULL, notice_at = NULL, updated_at = now()
             WHERE closed_at IS NULL AND NOT on_hold AND waiting_for = 'customer' AND last_message_at < now() - make_interval(days => $1)
             RETURNING id",
        )
        .bind(days)
        .fetch_all(&state.db)
        .await?;
        for id in stale {
            if let Err(e) = super::tickets::after_auto_close(state, id, days).await {
                tracing::warn!(ticket = %id, error = ?e, "could not finish closing a ticket");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }
    fn office() -> Hours {
        // Monday to Friday, 9:00–13:00 and 14:00–18:00.
        (1..=5).map(|d| (d, vec![(540, 780), (840, 1080)])).collect()
    }

    #[test]
    fn round_the_clock() {
        let h: Hours = (1..=7).map(|d| (d, vec![(0, 1440)])).collect();
        let start = at("2026-03-06T22:30:00Z");
        assert_eq!(add_working_minutes(start, 120, chrono_tz::UTC, &h, &HashSet::new()), Some(at("2026-03-07T00:30:00Z")));
    }

    #[test]
    fn office_hours_skip_lunch_nights_and_weekends() {
        let h = office();
        let none = HashSet::new();
        // Friday 12:00 + 3 h: 1 h before lunch, 2 h after → Friday 16:00.
        assert_eq!(add_working_minutes(at("2026-03-06T12:00:00Z"), 180, chrono_tz::UTC, &h, &none), Some(at("2026-03-06T16:00:00Z")));
        // Friday 17:00 + 2 h: 1 h on Friday, the rest on Monday at 10:00.
        assert_eq!(add_working_minutes(at("2026-03-06T17:00:00Z"), 120, chrono_tz::UTC, &h, &none), Some(at("2026-03-09T10:00:00Z")));
        // Saturday night: the clock starts Monday 9:00.
        assert_eq!(add_working_minutes(at("2026-03-07T23:00:00Z"), 30, chrono_tz::UTC, &h, &none), Some(at("2026-03-09T09:30:00Z")));
    }

    #[test]
    fn holidays_are_skipped() {
        let h = office();
        let holidays: HashSet<NaiveDate> = [NaiveDate::from_ymd_opt(2026, 3, 9).unwrap()].into();
        assert_eq!(add_working_minutes(at("2026-03-06T17:00:00Z"), 120, chrono_tz::UTC, &h, &holidays), Some(at("2026-03-10T10:00:00Z")));
    }

    #[test]
    fn hours_are_in_the_help_desk_time_zone() {
        let h = office();
        let tz: Tz = "Asia/Kolkata".parse().unwrap();
        // 03:00 UTC is 08:30 in Kolkata: work starts at 09:00 local (03:30 UTC).
        assert_eq!(add_working_minutes(at("2026-03-06T03:00:00Z"), 60, tz, &h, &HashSet::new()), Some(at("2026-03-06T04:30:00Z")));
    }

    #[test]
    fn never_open_gives_nothing() {
        assert_eq!(add_working_minutes(at("2026-03-06T03:00:00Z"), 60, chrono_tz::UTC, &Hours::new(), &HashSet::new()), None);
    }

    #[test]
    fn parsing_checks_the_spans() {
        assert!(parse_hours(&serde_json::json!({ "1": [[540, 1080]] })).is_ok());
        assert!(parse_hours(&serde_json::json!({ "8": [[540, 1080]] })).is_err());
        assert!(parse_hours(&serde_json::json!({ "1": [[600, 540]] })).is_err());
        assert!(parse_hours(&serde_json::json!({ "1": [[540, 800], [700, 900]] })).is_err());
        assert!(parse_hours(&serde_json::json!({ "1": [[0, 1441]] })).is_err());
    }
}
