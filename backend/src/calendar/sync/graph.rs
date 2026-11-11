//! Microsoft 365 calendars through Microsoft Graph: listing, reading, writing and deleting
//! events, and turning Graph's events and repeat patterns into NERP's and back.
//!
//! Graph keeps a changed occurrence of a series as an "exception"; those are read (and come
//! into NERP as changed occurrences). Occurrences removed in NERP are deleted there one by
//! one. Changed occurrences made in NERP's calendar apps aren't sent to Microsoft.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

use super::remote::Item;
use crate::{
    calendar::{ical, Event},
    state::AppState,
    tasks::recurrence::Recurrence,
};

pub struct Graph {
    base: String,
    token: String,
    client: reqwest::Client,
}

pub enum GraphError {
    Conflict,
    Other(String),
}

const DAYS: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
const INDEX: [(&str, i8); 5] = [("first", 1), ("second", 2), ("third", 3), ("fourth", 4), ("last", -1)];

impl Graph {
    pub fn new(state: &AppState, token: String) -> Result<Graph, String> {
        Ok(Graph { base: state.config.oauth.microsoft_graph_url.clone(), token, client: crate::crm::email::oauth::http()? })
    }

    async fn call(&self, method: Method, url: &str, body: Option<Value>, if_match: Option<&str>) -> Result<(StatusCode, Value), String> {
        // Only Graph itself (paging links are absolute).
        if !url.starts_with(&self.base) {
            return Err("Microsoft sent a link to somewhere else".into());
        }
        let mut req = self.client.request(method, url).bearer_auth(&self.token);
        if let Some(b) = body {
            req = req.json(&b);
        }
        if let Some(m) = if_match {
            req = req.header("If-Match", m);
        }
        let res = req.send().await.map_err(|e| format!("Microsoft could not be reached: {e}"))?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if status == StatusCode::UNAUTHORIZED {
            return Err("Microsoft refused the sign-in; connect the account again".into());
        }
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        Ok((status, v))
    }

    fn fail(status: StatusCode, v: &Value) -> String {
        format!("Microsoft answered {}: {}", status.as_u16(), v["error"]["message"].as_str().unwrap_or(""))
    }

    /// The account's default calendar: (id, name).
    pub async fn default_calendar(&self) -> Result<(String, String), String> {
        let (s, v) = self.call(Method::GET, &format!("{}/me/calendar?$select=id,name", self.base), None, None).await?;
        if !s.is_success() {
            return Err(Self::fail(s, &v));
        }
        Ok((v["id"].as_str().unwrap_or_default().to_string(), v["name"].as_str().unwrap_or("Calendar").to_string()))
    }

    /// Events and series in the calendar, with versions.
    pub async fn list(&self, calendar: &str) -> Result<Vec<Item>, String> {
        let mut url = format!("{}/me/calendars/{}/events?$select=id,changeKey,type&$top=200", self.base, enc(calendar));
        let mut out = Vec::new();
        for _ in 0..100 {
            let (s, v) = self.call(Method::GET, &url, None, None).await?;
            if !s.is_success() {
                return Err(Self::fail(s, &v));
            }
            for e in v["value"].as_array().cloned().unwrap_or_default() {
                if matches!(e["type"].as_str(), Some("singleInstance") | Some("seriesMaster") | None) {
                    if let (Some(id), Some(ck)) = (e["id"].as_str(), e["changeKey"].as_str()) {
                        out.push(Item { id: id.to_string(), etag: ck.to_string() });
                    }
                }
            }
            match v["@odata.nextLink"].as_str() {
                Some(next) => url = next.to_string(),
                None => return Ok(out),
            }
        }
        Ok(out)
    }

    /// One event (or series with its changed occurrences) as NERP reads iCalendar.
    pub async fn fetch(&self, id: &str) -> Result<Option<Vec<ical::Parsed>>, String> {
        let (s, v) = self.call(Method::GET, &format!("{}/me/events/{}", self.base, enc(id)), None, None).await?;
        if s == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !s.is_success() {
            return Err(Self::fail(s, &v));
        }
        let master = from_graph(&v)?;
        let mut all = vec![master.clone()];
        if v["type"] == "seriesMaster" {
            let (from, to) = (Utc::now() - Duration::days(60), Utc::now() + Duration::days(400));
            let url = format!(
                "{}/me/events/{}/instances?startDateTime={}&endDateTime={}&$top=500",
                self.base,
                enc(id),
                from.format("%Y-%m-%dT%H:%M:%SZ"),
                to.format("%Y-%m-%dT%H:%M:%SZ")
            );
            let (s, list) = self.call(Method::GET, &url, None, None).await?;
            if s.is_success() {
                for x in list["value"].as_array().cloned().unwrap_or_default().into_iter().filter(|x| x["type"] == "exception") {
                    let mut p = from_graph(&x)?;
                    p.uid = master.uid.clone();
                    p.rule = None;
                    p.recurrence_id = x["originalStart"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc));
                    if p.recurrence_id.is_some() {
                        all.push(p);
                    }
                }
            }
        }
        Ok(Some(all))
    }

    pub async fn create(&self, calendar: &str, e: &Event, exdates: &[DateTime<Utc>]) -> Result<Item, String> {
        let (s, v) = self.call(Method::POST, &format!("{}/me/calendars/{}/events", self.base, enc(calendar)), Some(to_graph(e)), None).await?;
        if !s.is_success() {
            return Err(Self::fail(s, &v));
        }
        let id = v["id"].as_str().ok_or("Microsoft didn't return the event")?.to_string();
        let mut item = Item { id, etag: v["changeKey"].as_str().unwrap_or_default().to_string() };
        if !exdates.is_empty() {
            self.remove_occurrences(&item.id, exdates).await?;
            item.etag = self.version(&item.id).await?;
        }
        Ok(item)
    }

    pub async fn update(&self, item: &Item, e: &Event, exdates: &[DateTime<Utc>]) -> Result<Item, GraphError> {
        let (s, v) = self
            .call(Method::PATCH, &format!("{}/me/events/{}", self.base, enc(&item.id)), Some(to_graph(e)), Some(&format!("W/\"{}\"", item.etag)))
            .await
            .map_err(GraphError::Other)?;
        if s == StatusCode::PRECONDITION_FAILED || s == StatusCode::CONFLICT {
            return Err(GraphError::Conflict);
        }
        if !s.is_success() {
            return Err(GraphError::Other(Self::fail(s, &v)));
        }
        let mut etag = v["changeKey"].as_str().unwrap_or_default().to_string();
        if !exdates.is_empty() {
            self.remove_occurrences(&item.id, exdates).await.map_err(GraphError::Other)?;
            etag = self.version(&item.id).await.map_err(GraphError::Other)?;
        }
        Ok(Item { id: item.id.clone(), etag })
    }

    async fn version(&self, id: &str) -> Result<String, String> {
        let (s, v) = self.call(Method::GET, &format!("{}/me/events/{}?$select=changeKey", self.base, enc(id)), None, None).await?;
        if !s.is_success() {
            return Err(Self::fail(s, &v));
        }
        Ok(v["changeKey"].as_str().unwrap_or_default().to_string())
    }

    /// Deletes the occurrences of a series that start at these times (already gone: fine).
    async fn remove_occurrences(&self, id: &str, starts: &[DateTime<Utc>]) -> Result<(), String> {
        let (first, last) = (starts.iter().min().copied().unwrap(), starts.iter().max().copied().unwrap());
        let url = format!(
            "{}/me/events/{}/instances?startDateTime={}&endDateTime={}&$select=id,originalStart&$top=500",
            self.base,
            enc(id),
            (first - Duration::days(1)).format("%Y-%m-%dT%H:%M:%SZ"),
            (last + Duration::days(2)).format("%Y-%m-%dT%H:%M:%SZ")
        );
        let (s, v) = self.call(Method::GET, &url, None, None).await?;
        if !s.is_success() {
            return Err(Self::fail(s, &v));
        }
        for x in v["value"].as_array().cloned().unwrap_or_default() {
            let start = x["originalStart"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc));
            if start.is_some_and(|t| starts.contains(&t)) {
                if let Some(oid) = x["id"].as_str() {
                    self.call(Method::DELETE, &format!("{}/me/events/{}", self.base, enc(oid)), None, None).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn delete(&self, item: &Item) -> Result<(), String> {
        let (s, v) = self.call(Method::DELETE, &format!("{}/me/events/{}", self.base, enc(&item.id)), None, None).await?;
        if s.is_success() || s == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(Self::fail(s, &v))
        }
    }
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~=".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Graph's {dateTime, timeZone} as an instant.
fn when(v: &Value) -> Result<(DateTime<Utc>, Option<Tz>), String> {
    let dt = v["dateTime"].as_str().ok_or("an event without a time")?;
    let naive = NaiveDateTime::parse_from_str(dt.split('.').next().unwrap_or(dt).trim_end_matches('Z'), "%Y-%m-%dT%H:%M:%S").map_err(|_| format!("can't read the time {dt}"))?;
    let tz = v["timeZone"].as_str().and_then(|z| ical::zone(z, &ical::Component::default())).unwrap_or(chrono_tz::UTC);
    let at = tz.from_local_datetime(&naive).earliest().ok_or(format!("can't place {dt}"))?;
    Ok((at.with_timezone(&Utc), Some(tz)))
}

fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ").replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").trim().to_string()
}

/// A Graph event as NERP reads iCalendar.
pub fn from_graph(v: &Value) -> Result<ical::Parsed, String> {
    let all_day = v["isAllDay"].as_bool().unwrap_or(false);
    let (mut start, _) = when(&v["start"])?;
    let (mut end, _) = when(&v["end"])?;
    // The zone the organiser uses (for repeats); Graph's own may be a Windows name.
    let zone = v["originalStartTimeZone"].as_str().and_then(|z| ical::zone(z, &ical::Component::default())).or_else(|| v["start"]["timeZone"].as_str().and_then(|z| ical::zone(z, &ical::Component::default()))).unwrap_or(chrono_tz::UTC);
    if all_day {
        let d = |t: DateTime<Utc>, field: &Value| {
            let raw = field["dateTime"].as_str().unwrap_or("");
            let date = NaiveDate::parse_from_str(&raw[..10.min(raw.len())], "%Y-%m-%d").unwrap_or(t.date_naive());
            Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        };
        start = d(start, &v["start"]);
        end = d(end, &v["end"]);
    }
    let body = v["body"]["content"].as_str().map(|c| if v["body"]["contentType"] == "html" { strip_html(c) } else { c.trim().to_string() }).filter(|c| !c.is_empty());
    let rule = match v.get("recurrence").filter(|r| !r.is_null()) {
        Some(r) => Some(rule_from(r, start, zone, all_day)?),
        None => None,
    };
    let attendees = v["attendees"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|a| {
            let email = a["emailAddress"]["address"].as_str()?.to_lowercase();
            let name = a["emailAddress"]["name"].as_str().map(str::to_string).filter(|n| !n.is_empty());
            let ps = match a["status"]["response"].as_str() {
                Some("accepted") | Some("organizer") => "ACCEPTED",
                Some("declined") => "DECLINED",
                Some("tentativelyAccepted") => "TENTATIVE",
                _ => "NEEDS-ACTION",
            };
            Some((email, name, ps.to_string()))
        })
        .collect();
    Ok(ical::Parsed {
        uid: v["iCalUId"].as_str().or(v["id"].as_str()).unwrap_or_default().to_string(),
        summary: v["subject"].as_str().filter(|s| !s.trim().is_empty()).map(|s| s.chars().take(300).collect()).unwrap_or_else(|| "(no title)".into()),
        description: body,
        location: v["location"]["displayName"].as_str().map(str::to_string).filter(|l| !l.is_empty()),
        start,
        end: end.max(start),
        all_day,
        timezone: if all_day { "UTC".into() } else { zone.name().to_string() },
        rule,
        exdates: vec![],
        recurrence_id: None,
        private: matches!(v["sensitivity"].as_str(), Some("private") | Some("confidential")),
        cancelled: v["isCancelled"].as_bool().unwrap_or(false),
        alarm_minutes: if v["isReminderOn"].as_bool().unwrap_or(false) { v["reminderMinutesBeforeStart"].as_i64().map(|m| m.clamp(0, 40320) as i32) } else { None },
        attendees,
    })
}

/// Graph's patternedRecurrence as a NERP repeat rule.
fn rule_from(r: &Value, start: DateTime<Utc>, tz: Tz, all_day: bool) -> Result<Recurrence, String> {
    let p = &r["pattern"];
    let range = &r["range"];
    let local = if all_day { start.naive_utc() } else { start.with_timezone(&tz).naive_local() };
    let day_no = |d: &str| DAYS.iter().position(|x| x.eq_ignore_ascii_case(d)).map(|i| i as u8 + 1);
    let days: Vec<u8> = p["daysOfWeek"].as_array().cloned().unwrap_or_default().iter().filter_map(|d| d.as_str().and_then(day_no)).collect();
    let index = p["index"].as_str().and_then(|i| INDEX.iter().find(|(n, _)| *n == i).map(|(_, x)| *x));
    let unsupported = || format!("this repeat pattern isn't supported: {}", p["type"]);
    let (freq, weekdays, month_day, month, week_of_month) = match p["type"].as_str().unwrap_or("") {
        "daily" => ("daily", vec![], None, None, None),
        "weekly" => ("weekly", if days.is_empty() { vec![local.date().weekday().number_from_monday() as u8] } else { days.clone() }, None, None, None),
        "absoluteMonthly" => ("monthly", vec![], p["dayOfMonth"].as_u64().map(|d| d as u8), None, None),
        "relativeMonthly" => ("monthly", days.iter().take(1).copied().collect(), None, None, Some(index.ok_or_else(unsupported)?)),
        "absoluteYearly" => ("yearly", vec![], p["dayOfMonth"].as_u64().map(|d| d as u8), p["month"].as_u64().map(|m| m as u8), None),
        "relativeYearly" => ("yearly", days.iter().take(1).copied().collect(), None, p["month"].as_u64().map(|m| m as u8), Some(index.ok_or_else(unsupported)?)),
        _ => return Err(unsupported()),
    };
    let rule = Recurrence {
        freq: freq.into(),
        interval: p["interval"].as_u64().unwrap_or(1).max(1) as u32,
        weekdays,
        month_day,
        month,
        week_of_month,
        time: local.format("%H:%M").to_string(),
        timezone: if all_day { "UTC".into() } else { tz.name().to_string() },
        starts_on: local.date(),
        ends_on: if range["type"] == "endDate" { range["endDate"].as_str().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()) } else { None },
        max_runs: if range["type"] == "numbered" { range["numberOfOccurrences"].as_i64().map(|n| n as i32) } else { None },
    };
    rule.validate().map_err(|e| e.to_string())?;
    Ok(rule)
}

/// A NERP event as a Graph event (no guests: Microsoft would invite them).
pub fn to_graph(e: &Event) -> Value {
    let tz: Tz = if e.all_day { chrono_tz::UTC } else { e.timezone.parse().unwrap_or(chrono_tz::UTC) };
    let local = |t: DateTime<Utc>| t.with_timezone(&tz).naive_local().format("%Y-%m-%dT%H:%M:%S").to_string();
    let mut v = json!({
        "subject": e.title,
        "body": { "contentType": "text", "content": e.description.clone().unwrap_or_default() },
        "start": { "dateTime": local(e.starts_at), "timeZone": tz.name() },
        "end": { "dateTime": local(e.ends_at.max(e.starts_at + if e.all_day { Duration::days(1) } else { Duration::zero() })), "timeZone": tz.name() },
        "isAllDay": e.all_day,
        "location": { "displayName": e.location.clone().unwrap_or_default() },
        "sensitivity": if e.is_private { "private" } else { "normal" },
        "isReminderOn": e.remind_minutes.is_some(),
        "reminderMinutesBeforeStart": e.remind_minutes.unwrap_or(15),
        "recurrence": Value::Null,
    });
    if let Some(r) = e.recurrence.clone().and_then(|r| serde_json::from_value::<Recurrence>(r).ok()) {
        let day = |d: u8| DAYS.get(d as usize - 1).copied().unwrap_or("monday");
        let index = r.week_of_month.and_then(|w| INDEX.iter().find(|(_, x)| *x == w).map(|(n, _)| *n));
        let mut pattern = json!({ "interval": r.interval, "firstDayOfWeek": "monday" });
        match (r.freq.as_str(), index) {
            ("daily", _) => pattern["type"] = json!("daily"),
            ("weekly", _) => {
                pattern["type"] = json!("weekly");
                pattern["daysOfWeek"] = json!(r.weekdays.iter().map(|d| day(*d)).collect::<Vec<_>>());
            }
            ("monthly", None) => {
                pattern["type"] = json!("absoluteMonthly");
                pattern["dayOfMonth"] = json!(r.month_day.unwrap_or(1));
            }
            ("monthly", Some(i)) => {
                pattern["type"] = json!("relativeMonthly");
                pattern["daysOfWeek"] = json!([day(*r.weekdays.first().unwrap_or(&1))]);
                pattern["index"] = json!(i);
            }
            (_, None) => {
                pattern["type"] = json!("absoluteYearly");
                pattern["dayOfMonth"] = json!(r.month_day.unwrap_or(1));
                pattern["month"] = json!(r.month.unwrap_or(1));
            }
            (_, Some(i)) => {
                pattern["type"] = json!("relativeYearly");
                pattern["daysOfWeek"] = json!([day(*r.weekdays.first().unwrap_or(&1))]);
                pattern["index"] = json!(i);
                pattern["month"] = json!(r.month.unwrap_or(1));
            }
        }
        let start_date = r.starts_on.format("%Y-%m-%d").to_string();
        let range = match (r.max_runs, r.ends_on) {
            (Some(n), _) => json!({ "type": "numbered", "startDate": start_date, "numberOfOccurrences": n, "recurrenceTimeZone": tz.name() }),
            (None, Some(end)) => json!({ "type": "endDate", "startDate": start_date, "endDate": end.format("%Y-%m-%d").to_string(), "recurrenceTimeZone": tz.name() }),
            (None, None) => json!({ "type": "noEnd", "startDate": start_date, "recurrenceTimeZone": tz.name() }),
        };
        v["recurrence"] = json!({ "pattern": pattern, "range": range });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_events_both_ways() {
        let g = json!({
            "id": "AAMk1", "changeKey": "ck1", "iCalUId": "040000008200E", "subject": "Planning", "type": "seriesMaster",
            "body": { "contentType": "html", "content": "<p>Bring <b>numbers</b> &amp; plans</p>" },
            "start": { "dateTime": "2026-03-02T09:00:00.0000000", "timeZone": "UTC" }, "end": { "dateTime": "2026-03-02T10:00:00.0000000", "timeZone": "UTC" },
            "originalStartTimeZone": "W. Europe Standard Time", "isAllDay": false, "sensitivity": "private", "isReminderOn": true, "reminderMinutesBeforeStart": 30,
            "location": { "displayName": "Room 4" },
            "recurrence": { "pattern": { "type": "relativeMonthly", "interval": 1, "daysOfWeek": ["tuesday"], "index": "second" }, "range": { "type": "numbered", "startDate": "2026-03-02", "numberOfOccurrences": 6 } },
            "attendees": [{ "emailAddress": { "address": "Ann@Example.com", "name": "Ann" }, "status": { "response": "tentativelyAccepted" } }]
        });
        let p = from_graph(&g).unwrap();
        assert_eq!(p.description.as_deref(), Some("Bring numbers & plans"));
        assert_eq!(p.timezone, "Europe/Berlin");
        let r = p.rule.clone().unwrap();
        assert_eq!((r.freq.as_str(), r.week_of_month, r.weekdays.clone(), r.max_runs, r.time.as_str()), ("monthly", Some(2), vec![2], Some(6), "10:00"));
        assert!(p.private && p.alarm_minutes == Some(30));
        assert_eq!(p.attendees[0], ("ann@example.com".into(), Some("Ann".into()), "TENTATIVE".into()));
        // And back.
        let e = Event {
            id: uuid::Uuid::nil(), owner_id: uuid::Uuid::nil(), title: p.summary.clone(), description: p.description.clone(), location: p.location.clone(), starts_at: p.start, ends_at: p.end,
            all_day: false, timezone: p.timezone.clone(), recurrence: Some(serde_json::to_value(&r).unwrap()), color: None, item_id: None, remind_minutes: Some(30), is_private: true,
            project_id: None, sequence: 0, room_id: None, slot_link_id: None, updated_at: Utc::now(), uid: None, parent_id: None, recurrence_id: None,
        };
        let back = to_graph(&e);
        assert_eq!(back["start"]["dateTime"], "2026-03-02T10:00:00");
        assert_eq!(back["start"]["timeZone"], "Europe/Berlin");
        assert_eq!(back["recurrence"]["pattern"]["type"], "relativeMonthly");
        assert_eq!(back["recurrence"]["pattern"]["index"], "second");
        assert_eq!(back["recurrence"]["range"]["numberOfOccurrences"], 6);
        let again = from_graph(&json!({ "subject": "x", "start": { "dateTime": back["start"]["dateTime"], "timeZone": "Europe/Berlin" }, "end": { "dateTime": back["end"]["dateTime"], "timeZone": "Europe/Berlin" }, "recurrence": back["recurrence"] })).unwrap();
        assert_eq!(again.rule.unwrap(), r);
        assert_eq!(again.start, p.start);
    }

    #[test]
    fn all_day_events() {
        let g = json!({ "subject": "Holiday", "isAllDay": true, "start": { "dateTime": "2026-12-24T00:00:00.0000000", "timeZone": "UTC" }, "end": { "dateTime": "2026-12-27T00:00:00.0000000", "timeZone": "UTC" } });
        let p = from_graph(&g).unwrap();
        assert!(p.all_day);
        assert_eq!(p.end - p.start, Duration::days(3));
    }
}
