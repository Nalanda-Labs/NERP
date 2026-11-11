//! Recurrence rules for recurring task templates.
//!
//! A rule fires at a wall-clock time in a named time zone ("09:00 in Asia/Kolkata"), so it
//! stays at 09:00 across daylight-saving changes.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Recurrence {
    /// daily | weekly | monthly | yearly
    pub freq: String,
    /// Every N days / weeks / months.
    #[serde(default = "one")]
    pub interval: u32,
    /// For weekly: ISO weekday numbers, 1 = Monday … 7 = Sunday.
    #[serde(default)]
    pub weekdays: Vec<u8>,
    /// For monthly and yearly: day of the month (31 means "last day" in shorter months).
    #[serde(default)]
    pub month_day: Option<u8>,
    /// For yearly: the month (1–12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<u8>,
    /// For monthly and yearly by weekday: which one in the month (1–4, or -1 for the last),
    /// with the weekday as the only entry of `weekdays` ("the second Tuesday").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_of_month: Option<i8>,
    /// Local time of day, "HH:MM".
    pub time: String,
    /// IANA time zone, e.g. "Europe/Berlin".
    pub timezone: String,
    /// First day the rule may fire.
    pub starts_on: NaiveDate,
    /// Last day the rule may fire.
    #[serde(default)]
    pub ends_on: Option<NaiveDate>,
    /// Stop after this many tasks have been created.
    #[serde(default)]
    pub max_runs: Option<i32>,
}

fn one() -> u32 {
    1
}

impl Recurrence {
    pub fn validate(&self) -> AppResult<()> {
        if !["daily", "weekly", "monthly", "yearly"].contains(&self.freq.as_str()) {
            return Err(AppError::bad("repeat must be daily, weekly, monthly or yearly"));
        }
        if !(1..=366).contains(&self.interval) {
            return Err(AppError::bad("repeat interval must be between 1 and 366"));
        }
        if self.freq == "weekly" && (self.weekdays.is_empty() || self.weekdays.iter().any(|d| !(1..=7).contains(d))) {
            return Err(AppError::bad("choose at least one weekday"));
        }
        if ["monthly", "yearly"].contains(&self.freq.as_str()) {
            match self.week_of_month {
                Some(w) => {
                    if !(w == -1 || (1..=4).contains(&w)) || self.weekdays.len() != 1 || !(1..=7).contains(&self.weekdays[0]) {
                        return Err(AppError::bad("choose the first to fourth, or the last, of one weekday"));
                    }
                }
                None => {
                    if !self.month_day.is_some_and(|d| (1..=31).contains(&d)) {
                        return Err(AppError::bad("choose a day of the month between 1 and 31"));
                    }
                }
            }
        }
        if self.freq == "yearly" && !self.month.is_some_and(|m| (1..=12).contains(&m)) {
            return Err(AppError::bad("choose the month a yearly repeat falls in"));
        }
        self.local_time()?;
        self.tz()?;
        if self.ends_on.is_some_and(|e| e < self.starts_on) {
            return Err(AppError::bad("the end date is before the start date"));
        }
        if self.max_runs.is_some_and(|m| m < 1) {
            return Err(AppError::bad("number of repetitions must be at least 1"));
        }
        Ok(())
    }

    fn tz(&self) -> AppResult<Tz> {
        self.timezone.parse().map_err(|_| AppError::bad(format!("unknown time zone: {}", self.timezone)))
    }

    fn local_time(&self) -> AppResult<NaiveTime> {
        NaiveTime::parse_from_str(&self.time, "%H:%M").map_err(|_| AppError::bad("time must look like 09:30"))
    }

    fn matches(&self, date: NaiveDate) -> bool {
        if date < self.starts_on || self.ends_on.is_some_and(|e| date > e) {
            return false;
        }
        let n = self.interval as i64;
        match self.freq.as_str() {
            "daily" => (date - self.starts_on).num_days() % n == 0,
            "weekly" => {
                let wd = date.weekday().number_from_monday() as u8;
                if !self.weekdays.contains(&wd) {
                    return false;
                }
                // Count whole weeks between the Mondays of the two dates.
                let monday = |d: NaiveDate| d - Duration::days(d.weekday().num_days_from_monday() as i64);
                ((monday(date) - monday(self.starts_on)).num_days() / 7) % n == 0
            }
            "monthly" => {
                let months = (date.year() - self.starts_on.year()) as i64 * 12 + date.month() as i64 - self.starts_on.month() as i64;
                months % n == 0 && self.day_matches(date)
            }
            "yearly" => {
                let years = (date.year() - self.starts_on.year()) as i64;
                years % n == 0 && self.month.is_some_and(|m| date.month() == m as u32) && self.day_matches(date)
            }
            _ => false,
        }
    }

    /// The day within a month: a day number, or the nth (or last) of a weekday.
    fn day_matches(&self, date: NaiveDate) -> bool {
        match self.week_of_month {
            Some(w) => {
                let wd = date.weekday().number_from_monday() as u8;
                if self.weekdays.first() != Some(&wd) {
                    return false;
                }
                if w == -1 {
                    date.day() + 7 > last_day_of_month(date)
                } else {
                    (date.day() - 1) / 7 + 1 == w as u32
                }
            }
            None => {
                let want = self.month_day.unwrap_or(1) as u32;
                date.day() == want.min(last_day_of_month(date))
            }
        }
    }

    /// The first firing strictly after `after`, or `None` when the rule has ended.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let tz = self.tz().ok()?;
        let time = self.local_time().ok()?;
        let mut date = after.with_timezone(&tz).date_naive().max(self.starts_on);
        // Enough days for any rule to find its next match (yearly ones every few years).
        let limit = if self.freq == "yearly" { 366 * (self.interval as usize + 1) + 31 } else { 3700 };
        for _ in 0..limit {
            if self.ends_on.is_some_and(|e| date > e) {
                return None;
            }
            if self.matches(date) {
                // A wall-clock time skipped by a DST jump resolves to the moment just after it.
                let local = tz
                    .from_local_datetime(&date.and_time(time))
                    .earliest()
                    .or_else(|| tz.from_local_datetime(&(date.and_time(time) + Duration::hours(1))).earliest());
                if let Some(at) = local.map(|l| l.with_timezone(&Utc))
                    && at > after {
                        return Some(at);
                    }
            }
            date = date.succ_opt()?;
        }
        None
    }

    /// Plain-English summary for lists: "Every 2 weeks on Mon, Thu at 09:00".
    pub fn describe(&self) -> String {
        let every = |unit: &str| {
            if self.interval == 1 { format!("Every {unit}") } else { format!("Every {} {unit}s", self.interval) }
        };
        let names = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
        let base = match self.freq.as_str() {
            "daily" => every("day"),
            "weekly" => {
                let days: Vec<&str> = self.weekdays.iter().filter_map(|d| names.get(*d as usize - 1).copied()).collect();
                format!("{} on {}", every("week"), days.join(", "))
            }
            "monthly" => format!("{} on {}", every("month"), self.day_words(&names)),
            _ => {
                const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
                let m = MONTHS.get(self.month.unwrap_or(1) as usize - 1).copied().unwrap_or("Jan");
                match self.week_of_month {
                    Some(_) => format!("{} on {} of {m}", every("year"), self.day_words(&names)),
                    None => format!("{} on {} {m}", every("year"), self.month_day.unwrap_or(1)),
                }
            }
        };
        format!("{base} at {}", self.time)
    }
}

impl Recurrence {
    /// "day 14" or "the 2nd Tue" / "the last Fri".
    fn day_words(&self, names: &[&str; 7]) -> String {
        match self.week_of_month {
            Some(w) => {
                let day = self.weekdays.first().and_then(|d| names.get(*d as usize - 1)).copied().unwrap_or("Mon");
                let nth = match w { 1 => "1st", 2 => "2nd", 3 => "3rd", 4 => "4th", _ => "last" };
                format!("the {nth} {day}")
            }
            None => format!("day {}", self.month_day.unwrap_or(1)),
        }
    }
}

fn last_day_of_month(d: NaiveDate) -> u32 {
    let (y, m) = if d.month() == 12 { (d.year() + 1, 1) } else { (d.year(), d.month() + 1) };
    NaiveDate::from_ymd_opt(y, m, 1).unwrap().pred_opt().unwrap().day()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(freq: &str) -> Recurrence {
        Recurrence {
            freq: freq.into(),
            interval: 1,
            weekdays: vec![],
            month_day: None,
            month: None,
            week_of_month: None,
            time: "09:00".into(),
            timezone: "UTC".into(),
            starts_on: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            ends_on: None,
            max_runs: None,
        }
    }
    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn daily_same_day_and_next_day() {
        let r = rule("daily");
        assert_eq!(r.next_after(utc("2026-03-10T08:00:00Z")), Some(utc("2026-03-10T09:00:00Z")));
        assert_eq!(r.next_after(utc("2026-03-10T09:00:00Z")), Some(utc("2026-03-11T09:00:00Z")));
    }

    #[test]
    fn every_three_days_counts_from_start() {
        let mut r = rule("daily");
        r.interval = 3; // Jan 1, 4, 7, …
        assert_eq!(r.next_after(utc("2026-01-02T00:00:00Z")), Some(utc("2026-01-04T09:00:00Z")));
    }

    #[test]
    fn weekly_on_weekdays_every_other_week() {
        let mut r = rule("weekly");
        r.weekdays = vec![1, 4]; // Mon, Thu
        r.interval = 2;
        r.starts_on = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap(); // a Monday
        let a = r.next_after(utc("2026-11-02T10:00:00Z")).unwrap();
        assert_eq!(a, utc("2026-11-05T09:00:00Z")); // Thu same week
        let b = r.next_after(a).unwrap();
        assert_eq!(b, utc("2026-11-16T09:00:00Z")); // skips the week of Nov 9
    }

    #[test]
    fn monthly_31st_falls_back_to_last_day() {
        let mut r = rule("monthly");
        r.month_day = Some(31);
        assert_eq!(r.next_after(utc("2026-02-01T00:00:00Z")), Some(utc("2026-02-28T09:00:00Z")));
        assert_eq!(r.next_after(utc("2026-04-01T00:00:00Z")), Some(utc("2026-04-30T09:00:00Z")));
    }

    #[test]
    fn local_time_survives_daylight_saving() {
        let mut r = rule("daily");
        r.timezone = "Europe/Berlin".into();
        // Winter: 09:00 CET = 08:00 UTC. Summer: 09:00 CEST = 07:00 UTC.
        assert_eq!(r.next_after(utc("2026-01-15T00:00:00Z")), Some(utc("2026-01-15T08:00:00Z")));
        assert_eq!(r.next_after(utc("2026-07-15T00:00:00Z")), Some(utc("2026-07-15T07:00:00Z")));
    }

    #[test]
    fn ends_on_stops_the_rule() {
        let mut r = rule("daily");
        r.ends_on = NaiveDate::from_ymd_opt(2026, 1, 3);
        assert_eq!(r.next_after(utc("2026-01-03T09:00:00Z")), None);
    }

    #[test]
    fn monthly_on_the_second_tuesday_and_the_last_friday() {
        let mut r = rule("monthly");
        r.week_of_month = Some(2);
        r.weekdays = vec![2];
        assert_eq!(r.next_after(utc("2026-11-01T00:00:00Z")), Some(utc("2026-11-10T09:00:00Z")));
        assert_eq!(r.next_after(utc("2026-11-10T09:00:00Z")), Some(utc("2026-12-08T09:00:00Z")));
        r.week_of_month = Some(-1);
        r.weekdays = vec![5];
        assert_eq!(r.next_after(utc("2026-11-01T00:00:00Z")), Some(utc("2026-11-27T09:00:00Z")));
        assert_eq!(r.describe(), "Every month on the last Fri at 09:00");
    }

    #[test]
    fn yearly_by_date_and_by_weekday() {
        let mut r = rule("yearly");
        r.month = Some(3);
        r.month_day = Some(14);
        assert_eq!(r.next_after(utc("2026-03-14T10:00:00Z")), Some(utc("2027-03-14T09:00:00Z")));
        assert_eq!(r.describe(), "Every year on 14 Mar at 09:00");
        r.month_day = None;
        r.month = Some(11);
        r.week_of_month = Some(4);
        r.weekdays = vec![4]; // the fourth Thursday of November
        assert_eq!(r.next_after(utc("2026-01-01T00:00:00Z")), Some(utc("2026-11-26T09:00:00Z")));
        assert!(r.validate().is_ok());
        r.month = None;
        assert!(r.validate().is_err());
    }

    #[test]
    fn describe_reads_naturally() {
        let mut r = rule("weekly");
        r.weekdays = vec![1, 5];
        r.interval = 2;
        assert_eq!(r.describe(), "Every 2 weeks on Mon, Fri at 09:00");
    }
}
