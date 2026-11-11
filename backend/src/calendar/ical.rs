//! Writing iCalendar (RFC 5545): events with repeat rules, removed occurrences, organiser and
//! guests, and the time zone definitions repeating events need so they keep their local time
//! across daylight-saving changes in every calendar app.

use chrono::{DateTime, Datelike, Duration, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::{OffsetComponents, OffsetName, Tz};

use crate::tasks::recurrence::Recurrence;

/// Escapes text for an iCalendar value.
pub fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace("\r\n", "\\n").replace('\n', "\\n")
}

/// Folds a content line at 75 octets (continuation lines start with a space).
pub fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut len = 0;
    for c in line.chars() {
        let w = c.len_utf8();
        if len + w > 75 {
            out.push_str("\r\n ");
            len = 1;
        }
        out.push(c);
        len += w;
    }
    out.push_str("\r\n");
    out
}

pub fn stamp(t: DateTime<Utc>) -> String {
    t.format("%Y%m%dT%H%M%SZ").to_string()
}

fn local_stamp(t: NaiveDateTime) -> String {
    t.format("%Y%m%dT%H%M%S").to_string()
}

#[derive(Clone)]
pub struct Person {
    pub name: String,
    pub email: String,
    /// NEEDS-ACTION | ACCEPTED | DECLINED | TENTATIVE
    pub partstat: &'static str,
}

pub fn partstat(status: &str) -> &'static str {
    match status {
        "accepted" => "ACCEPTED",
        "declined" => "DECLINED",
        "tentative" => "TENTATIVE",
        _ => "NEEDS-ACTION",
    }
}

/// One VEVENT.
#[derive(Clone)]
pub struct VEvent {
    pub uid: String,
    pub sequence: i32,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    /// The event's time zone (used for repeating timed events).
    pub timezone: String,
    pub rule: Option<Recurrence>,
    pub exdates: Vec<DateTime<Utc>>,
    /// This is one changed or cancelled occurrence of a repeating event.
    pub recurrence_id: Option<DateTime<Utc>>,
    pub summary: String,
    pub location: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub organizer: Option<Person>,
    pub attendees: Vec<Person>,
    pub cancelled: bool,
    pub private: bool,
    pub last_modified: Option<DateTime<Utc>>,
    /// Minutes before the start for an alarm.
    pub alarm_minutes: Option<i32>,
}

impl VEvent {
    /// Repeating timed events are written in local time (with TZID) so the repeats keep their
    /// wall-clock time; everything else in UTC (or as dates, for all-day events).
    fn zone(&self) -> Option<Tz> {
        if self.all_day || (self.rule.is_none() && self.recurrence_id.is_none() && self.exdates.is_empty()) {
            return None;
        }
        self.timezone.parse::<Tz>().ok().filter(|z| *z != chrono_tz::UTC)
    }

    fn when(&self, name: &str, t: DateTime<Utc>) -> String {
        if self.all_day {
            return format!("{name};VALUE=DATE:{}", t.format("%Y%m%d"));
        }
        match self.zone() {
            Some(z) => format!("{name};TZID={}:{}", z.name(), local_stamp(t.with_timezone(&z).naive_local())),
            None => format!("{name}:{}", stamp(t)),
        }
    }
}

/// The RRULE for a NERP repeat rule.
pub fn rrule(r: &Recurrence, all_day: bool) -> String {
    let mut parts = vec![format!("FREQ={}", r.freq.to_uppercase())];
    if r.interval > 1 {
        parts.push(format!("INTERVAL={}", r.interval));
    }
    match r.freq.as_str() {
        "weekly" => {
            const DAYS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];
            let mut days: Vec<u8> = r.weekdays.clone();
            days.sort();
            parts.push(format!("BYDAY={}", days.iter().filter_map(|d| DAYS.get(*d as usize - 1).copied()).collect::<Vec<_>>().join(",")));
            parts.push("WKST=MO".into());
        }
        "monthly" | "yearly" => {
            if r.freq == "yearly" {
                parts.push(format!("BYMONTH={}", r.month.unwrap_or(1)));
            }
            const DAYS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];
            parts.push(match (r.week_of_month, r.weekdays.first()) {
                (Some(w), Some(d)) => format!("BYDAY={w}{}", DAYS.get(*d as usize - 1).copied().unwrap_or("MO")),
                // Days past the 28th fall back to the month's last day, as NERP repeats them.
                _ => match r.month_day.unwrap_or(1) {
                    31 => "BYMONTHDAY=-1".into(),
                    d @ 29..=30 => format!("BYMONTHDAY={};BYSETPOS=-1", (28..=d).map(|x| x.to_string()).collect::<Vec<_>>().join(",")),
                    d => format!("BYMONTHDAY={d}"),
                },
            });
        }
        _ => {}
    }
    if let Some(n) = r.max_runs {
        parts.push(format!("COUNT={n}"));
    } else if let Some(end) = r.ends_on {
        if all_day {
            parts.push(format!("UNTIL={}", end.format("%Y%m%d")));
        } else {
            let tz: Tz = r.timezone.parse().unwrap_or(chrono_tz::UTC);
            let last = tz.from_local_datetime(&end.and_hms_opt(23, 59, 59).unwrap()).earliest().map(|t| t.with_timezone(&Utc)).unwrap_or_else(|| Utc.from_utc_datetime(&end.and_hms_opt(23, 59, 59).unwrap()));
            parts.push(format!("UNTIL={}", stamp(last)));
        }
    }
    parts.join(";")
}

fn offset_str(secs: i32) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.abs();
    format!("{sign}{:02}{:02}", a / 3600, (a % 3600) / 60)
}

/// A VTIMEZONE for `tz` with its transitions between two years (each as an RDATE), found by
/// scanning the zone's offsets.
pub fn vtimezone(tz: Tz, from_year: i32, to_year: i32) -> String {
    let at = |t: DateTime<Utc>| {
        let o = tz.offset_from_utc_datetime(&t.naive_utc());
        (o.fix().local_minus_utc(), o.dst_offset() != Duration::zero(), tz.offset_from_utc_datetime(&t.naive_utc()).abbreviation().map(str::to_string))
    };
    let start = Utc.with_ymd_and_hms(from_year, 1, 1, 0, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(to_year + 1, 1, 1, 0, 0, 0).unwrap();
    // (instant, offset before, offset after, is summer time, abbreviation)
    let mut changes: Vec<(DateTime<Utc>, i32, i32, bool, Option<String>)> = Vec::new();
    let mut t = start;
    let mut prev = at(t);
    while t < end {
        let next = t + Duration::hours(6);
        let cur = at(next);
        if cur.0 != prev.0 || cur.1 != prev.1 {
            // Narrow down to the minute.
            let (mut lo, mut hi) = (t, next);
            while hi - lo > Duration::minutes(1) {
                let mid = lo + (hi - lo) / 2;
                if at(mid).0 == prev.0 && at(mid).1 == prev.1 { lo = mid } else { hi = mid }
            }
            // Changes happen on a whole minute.
            let hi = Utc.timestamp_opt(hi.timestamp() - hi.timestamp().rem_euclid(60), 0).unwrap();
            changes.push((hi, prev.0, cur.0, cur.1, cur.2.clone()));
        }
        prev = cur;
        t = next;
    }
    let mut s = String::new();
    s.push_str(&fold("BEGIN:VTIMEZONE"));
    s.push_str(&fold(&format!("TZID:{}", tz.name())));
    if changes.is_empty() {
        let (off, dst, abbr) = at(start);
        let kind = if dst { "DAYLIGHT" } else { "STANDARD" };
        s.push_str(&fold(&format!("BEGIN:{kind}")));
        s.push_str(&fold("DTSTART:19700101T000000"));
        s.push_str(&fold(&format!("TZOFFSETFROM:{}", offset_str(off))));
        s.push_str(&fold(&format!("TZOFFSETTO:{}", offset_str(off))));
        if let Some(a) = abbr.filter(|a| !a.starts_with(['+', '-'])) {
            s.push_str(&fold(&format!("TZNAME:{a}")));
        }
        s.push_str(&fold(&format!("END:{kind}")));
    } else {
        // Group by (from, to, kind): one component with every onset as an RDATE.
        let mut groups: Vec<((i32, i32, bool, Option<String>), Vec<NaiveDateTime>)> = Vec::new();
        for (when, from, to, dst, abbr) in changes {
            // The onset is written in the local time before the change.
            let local = when.naive_utc() + Duration::seconds(from as i64);
            let key = (from, to, dst, abbr);
            match groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, v)) => v.push(local),
                None => groups.push((key, vec![local])),
            }
        }
        for ((from, to, dst, abbr), onsets) in groups {
            let kind = if dst { "DAYLIGHT" } else { "STANDARD" };
            s.push_str(&fold(&format!("BEGIN:{kind}")));
            s.push_str(&fold(&format!("DTSTART:{}", local_stamp(onsets[0]))));
            for o in &onsets[1..] {
                s.push_str(&fold(&format!("RDATE:{}", local_stamp(*o))));
            }
            s.push_str(&fold(&format!("TZOFFSETFROM:{}", offset_str(from))));
            s.push_str(&fold(&format!("TZOFFSETTO:{}", offset_str(to))));
            if let Some(a) = abbr.filter(|a| !a.starts_with(['+', '-'])) {
                s.push_str(&fold(&format!("TZNAME:{a}")));
            }
            s.push_str(&fold(&format!("END:{kind}")));
        }
    }
    s.push_str(&fold("END:VTIMEZONE"));
    s
}

fn vevent(e: &VEvent, now: DateTime<Utc>) -> String {
    let mut s = String::new();
    let mut put = |l: String| s.push_str(&fold(&l));
    put("BEGIN:VEVENT".into());
    put(format!("UID:{}", e.uid));
    put(format!("DTSTAMP:{}", stamp(now)));
    if let Some(m) = e.last_modified {
        put(format!("LAST-MODIFIED:{}", stamp(m)));
    }
    put(format!("SEQUENCE:{}", e.sequence));
    if let Some(r) = e.recurrence_id {
        put(e.when("RECURRENCE-ID", r));
    }
    put(e.when("DTSTART", e.start));
    if e.all_day {
        put(e.when("DTEND", e.end.max(e.start + Duration::days(1))));
    } else {
        put(e.when("DTEND", e.end));
    }
    if let (Some(r), None) = (&e.rule, e.recurrence_id) {
        put(format!("RRULE:{}", rrule(r, e.all_day)));
        for x in &e.exdates {
            put(e.when("EXDATE", *x));
        }
    }
    put(format!("SUMMARY:{}", esc(&e.summary)));
    if let Some(l) = &e.location {
        put(format!("LOCATION:{}", esc(l)));
    }
    if let Some(d) = &e.description {
        put(format!("DESCRIPTION:{}", esc(d)));
    }
    if let Some(u) = &e.url {
        put(format!("URL:{u}"));
    }
    if let Some(o) = &e.organizer {
        put(format!("ORGANIZER;CN=\"{}\":mailto:{}", o.name.replace('"', "'"), o.email));
    }
    for a in &e.attendees {
        put(format!("ATTENDEE;CN=\"{}\";ROLE=REQ-PARTICIPANT;PARTSTAT={};RSVP=TRUE:mailto:{}", a.name.replace('"', "'"), a.partstat, a.email));
    }
    if e.private {
        put("CLASS:PRIVATE".into());
    }
    put(format!("STATUS:{}", if e.cancelled { "CANCELLED" } else { "CONFIRMED" }));
    if let (Some(m), false) = (e.alarm_minutes, e.cancelled) {
        put("BEGIN:VALARM".into());
        put("ACTION:DISPLAY".into());
        put(format!("DESCRIPTION:{}", esc(&e.summary)));
        put(format!("TRIGGER:-PT{m}M"));
        put("END:VALARM".into());
    }
    put("END:VEVENT".into());
    s
}

/// A whole calendar object: the time zones the events need, then the events.
/// `method`: REQUEST / CANCEL for invitations, None for stored objects (CalDAV, feeds).
pub fn calendar(method: Option<&str>, name: Option<&str>, events: &[VEvent]) -> String {
    let now = Utc::now();
    let mut s = String::new();
    for l in ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//NERP//Calendar//EN", "CALSCALE:GREGORIAN"] {
        s.push_str(&fold(l));
    }
    if let Some(m) = method {
        s.push_str(&fold(&format!("METHOD:{m}")));
    }
    if let Some(n) = name {
        s.push_str(&fold(&format!("X-WR-CALNAME:{}", esc(n))));
    }
    let mut zones: Vec<Tz> = Vec::new();
    let mut years = (now.year(), now.year() + 2);
    for e in events {
        if let Some(z) = e.zone() {
            if !zones.contains(&z) {
                zones.push(z);
            }
            years.0 = years.0.min(e.start.year());
        }
    }
    for z in zones {
        s.push_str(&vtimezone(z, years.0, years.1));
    }
    for e in events {
        s.push_str(&vevent(e, now));
    }
    s.push_str(&fold("END:VCALENDAR"));
    s
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// One content line: NAME;PARAM=VALUE:value.
#[derive(Debug, Clone)]
pub struct Prop {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub value: String,
}

impl Prop {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Component {
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }
    pub fn all(&self, name: &str) -> impl Iterator<Item = &Prop> {
        self.props.iter().filter(move |p| p.name == name)
    }
    pub fn text(&self, name: &str) -> Option<String> {
        self.prop(name).map(|p| unescape(&p.value)).filter(|t| !t.trim().is_empty())
    }
}

pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(x) => out.push(x),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Splits one unfolded line into its name, parameters and value (colons and semicolons
/// inside quoted parameter values don't count).
fn content_line(line: &str) -> Option<Prop> {
    let mut quoted = false;
    let mut split = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                split = Some(i);
                break;
            }
            _ => {}
        }
    }
    let i = split?;
    let (head, value) = (&line[..i], &line[i + 1..]);
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    quoted = false;
    for c in head.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ';' if !quoted => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    let name = parts.first()?.trim().to_ascii_uppercase();
    if name.is_empty() {
        return None;
    }
    let params = parts[1..]
        .iter()
        .filter_map(|p| p.split_once('=').map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim().trim_matches('"').to_string())))
        .collect();
    Some(Prop { name, params, value: value.to_string() })
}

/// Parses an iCalendar object into its components.
pub fn parse(text: &str) -> Result<Component, String> {
    let unfolded = text.replace("\r\n ", "").replace("\r\n\t", "").replace("\n ", "").replace("\n\t", "");
    let mut stack: Vec<Component> = vec![Component { name: "ROOT".into(), ..Default::default() }];
    for raw in unfolded.lines() {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let Some(p) = content_line(line) else {
            return Err(format!("can't read the line “{}”", line.chars().take(60).collect::<String>()));
        };
        match p.name.as_str() {
            "BEGIN" => {
                if stack.len() > 8 {
                    return Err("components nest too deep".into());
                }
                stack.push(Component { name: p.value.trim().to_ascii_uppercase(), ..Default::default() });
            }
            "END" => {
                let done = stack.pop().ok_or("END without BEGIN")?;
                if done.name != p.value.trim().to_ascii_uppercase() || stack.is_empty() {
                    return Err(format!("END:{} doesn't close {}", p.value.trim(), done.name));
                }
                stack.last_mut().unwrap().children.push(done);
            }
            _ => stack.last_mut().ok_or("content outside a component")?.props.push(p),
        }
    }
    if stack.len() != 1 {
        return Err("a component isn't closed".into());
    }
    stack.pop().unwrap().children.into_iter().find(|c| c.name == "VCALENDAR").ok_or_else(|| "no VCALENDAR".to_string())
}

/// Common Windows time zone names (Outlook) and their IANA zones.
const WINDOWS_ZONES: &[(&str, &str)] = &[
    ("W. Europe Standard Time", "Europe/Berlin"), ("Central Europe Standard Time", "Europe/Budapest"), ("Romance Standard Time", "Europe/Paris"),
    ("Central European Standard Time", "Europe/Warsaw"), ("GMT Standard Time", "Europe/London"), ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("E. Europe Standard Time", "Europe/Chisinau"), ("FLE Standard Time", "Europe/Kiev"), ("GTB Standard Time", "Europe/Bucharest"),
    ("Russian Standard Time", "Europe/Moscow"), ("Turkey Standard Time", "Europe/Istanbul"), ("Israel Standard Time", "Asia/Jerusalem"),
    ("Arabian Standard Time", "Asia/Dubai"), ("India Standard Time", "Asia/Kolkata"), ("China Standard Time", "Asia/Shanghai"),
    ("Tokyo Standard Time", "Asia/Tokyo"), ("Singapore Standard Time", "Asia/Singapore"), ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("New Zealand Standard Time", "Pacific/Auckland"), ("Eastern Standard Time", "America/New_York"), ("Central Standard Time", "America/Chicago"),
    ("Mountain Standard Time", "America/Denver"), ("Pacific Standard Time", "America/Los_Angeles"), ("Atlantic Standard Time", "America/Halifax"),
    ("E. South America Standard Time", "America/Sao_Paulo"), ("Argentina Standard Time", "America/Buenos_Aires"), ("South Africa Standard Time", "Africa/Johannesburg"),
    ("Egypt Standard Time", "Africa/Cairo"), ("W. Central Africa Standard Time", "Africa/Lagos"), ("UTC", "UTC"),
];

/// The IANA zone for a TZID: the name itself, its end (`/mozilla.org/…/Europe/Berlin`), a
/// Windows name, or the X-LIC-LOCATION of the object's VTIMEZONE.
pub fn zone(tzid: &str, cal: &Component) -> Option<Tz> {
    let id = tzid.trim().trim_matches('"');
    if let Ok(z) = id.parse::<Tz>() {
        return Some(z);
    }
    let segs: Vec<&str> = id.split('/').filter(|s| !s.is_empty()).collect();
    for n in [3, 2, 1] {
        if segs.len() >= n {
            if let Ok(z) = segs[segs.len() - n..].join("/").parse::<Tz>() {
                return Some(z);
            }
        }
    }
    if let Some((_, z)) = WINDOWS_ZONES.iter().find(|(w, _)| w.eq_ignore_ascii_case(id)) {
        return z.parse().ok();
    }
    cal.children
        .iter()
        .filter(|c| c.name == "VTIMEZONE" && c.prop("TZID").is_some_and(|t| t.value.trim() == id))
        .find_map(|c| c.prop("X-LIC-LOCATION").and_then(|l| l.value.trim().parse::<Tz>().ok()))
}

/// A date or date-time value: (instant, all-day, zone it was given in).
fn when_of(p: &Prop, cal: &Component, floating: Tz) -> Result<(DateTime<Utc>, bool, Option<Tz>), String> {
    let v = p.value.trim();
    let bad = || format!("can't read the time {v}");
    if p.param("VALUE").is_some_and(|x| x.eq_ignore_ascii_case("DATE")) || (v.len() == 8 && v.chars().all(|c| c.is_ascii_digit())) {
        let d = chrono::NaiveDate::parse_from_str(&v[..8.min(v.len())], "%Y%m%d").map_err(|_| bad())?;
        return Ok((Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap()), true, None));
    }
    if let Some(u) = v.strip_suffix('Z') {
        let t = NaiveDateTime::parse_from_str(u, "%Y%m%dT%H%M%S").map_err(|_| bad())?;
        return Ok((Utc.from_utc_datetime(&t), false, None));
    }
    let t = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").map_err(|_| bad())?;
    let tz = match p.param("TZID") {
        Some(id) => zone(id, cal).unwrap_or(floating),
        None => floating,
    };
    let at = tz.from_local_datetime(&t).earliest().or_else(|| tz.from_local_datetime(&(t + Duration::hours(1))).earliest()).ok_or_else(bad)?;
    Ok((at.with_timezone(&Utc), false, Some(tz)))
}

/// An iCalendar duration (P1W, PT15M, -P1DT2H…) in seconds.
pub fn duration_secs(s: &str) -> Option<i64> {
    let s = s.trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let s = s.strip_prefix('P')?;
    let (mut total, mut num, mut in_time) = (0i64, String::new(), false);
    for c in s.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            _ => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += n * match (c, in_time) {
                    ('W', false) => 7 * 86400,
                    ('D', false) => 86400,
                    ('H', true) => 3600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    if !num.is_empty() {
        return None;
    }
    Some(if neg { -total } else { total })
}

/// What NERP keeps of one VEVENT.
#[derive(Debug, Clone)]
pub struct Parsed {
    pub uid: String,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub timezone: String,
    pub rule: Option<Recurrence>,
    pub exdates: Vec<DateTime<Utc>>,
    pub recurrence_id: Option<DateTime<Utc>>,
    pub private: bool,
    pub cancelled: bool,
    pub alarm_minutes: Option<i32>,
    /// (email, name, PARTSTAT)
    pub attendees: Vec<(String, Option<String>, String)>,
}

fn mailto(v: &str) -> Option<String> {
    let v = v.trim();
    let addr = if v.len() > 7 && v[..7].eq_ignore_ascii_case("mailto:") { &v[7..] } else { v };
    let addr = addr.trim().to_lowercase();
    addr.contains('@').then_some(addr)
}

const RRULE_DAYS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

/// The NERP repeat rule for an RRULE, or why it can't be kept.
pub fn rule_from(rrule: &str, start: DateTime<Utc>, tz: Tz, all_day: bool) -> Result<Recurrence, String> {
    let local = if all_day { start.naive_utc() } else { start.with_timezone(&tz).naive_local() };
    let mut parts: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for kv in rrule.split(';').filter(|x| !x.trim().is_empty()) {
        let (k, v) = kv.split_once('=').ok_or_else(|| format!("can't read the repeat rule {rrule}"))?;
        parts.insert(k.trim().to_ascii_uppercase(), v.trim().to_ascii_uppercase());
    }
    let unsupported = || format!("this repeat rule isn't supported: {rrule}");
    for k in parts.keys() {
        if !["FREQ", "INTERVAL", "BYDAY", "BYMONTHDAY", "BYMONTH", "BYSETPOS", "UNTIL", "COUNT", "WKST"].contains(&k.as_str()) {
            return Err(unsupported());
        }
    }
    let freq = match parts.get("FREQ").map(String::as_str) {
        Some("DAILY") => "daily",
        Some("WEEKLY") => "weekly",
        Some("MONTHLY") => "monthly",
        Some("YEARLY") => "yearly",
        _ => return Err(unsupported()),
    };
    let interval: u32 = parts.get("INTERVAL").map(|i| i.parse().map_err(|_| unsupported())).transpose()?.unwrap_or(1);
    let day_no = |d: &str| RRULE_DAYS.iter().position(|x| *x == d).map(|i| i as u8 + 1);
    let start_wd = local.date().weekday().number_from_monday() as u8;
    let mut weekdays: Vec<u8> = Vec::new();
    let mut month_day: Option<u8> = None;
    let mut month: Option<u8> = None;
    let mut week_of_month: Option<i8> = None;
    let setpos = parts.get("BYSETPOS").cloned();
    match freq {
        "daily" => {
            if parts.contains_key("BYDAY") || parts.contains_key("BYMONTHDAY") || parts.contains_key("BYMONTH") || setpos.is_some() {
                return Err(unsupported());
            }
        }
        "weekly" => {
            if parts.contains_key("BYMONTHDAY") || parts.contains_key("BYMONTH") || setpos.is_some() {
                return Err(unsupported());
            }
            match parts.get("BYDAY") {
                Some(list) => {
                    for d in list.split(',') {
                        weekdays.push(day_no(d.trim()).ok_or_else(unsupported)?);
                    }
                }
                None => weekdays.push(start_wd),
            }
            weekdays.sort();
            weekdays.dedup();
        }
        _ => {
            if freq == "yearly" {
                month = Some(match parts.get("BYMONTH") {
                    Some(m) => m.parse::<u8>().ok().filter(|m| (1..=12).contains(m)).ok_or_else(unsupported)?,
                    None => local.date().month() as u8,
                });
            } else if parts.contains_key("BYMONTH") {
                return Err(unsupported());
            }
            match (parts.get("BYMONTHDAY"), parts.get("BYDAY")) {
                (Some(md), None) => {
                    let days: Vec<i32> = md.split(',').map(|x| x.trim().parse::<i32>()).collect::<Result<_, _>>().map_err(|_| unsupported())?;
                    month_day = Some(match (days.as_slice(), setpos.as_deref()) {
                        ([-1], None) => 31,
                        ([d], None) if (1..=31).contains(d) => *d as u8,
                        // "The 30th, or the last day of shorter months" (as NERP writes it).
                        (list, Some("-1")) if list.len() >= 2 && list[0] == 28 && list.windows(2).all(|w| w[1] == w[0] + 1) && *list.last().unwrap() <= 30 => *list.last().unwrap() as u8,
                        _ => return Err(unsupported()),
                    });
                }
                (None, Some(bd)) => {
                    // "2TU", "-1FR", or "TU" with BYSETPOS=2.
                    let bd = bd.trim();
                    let (n, d) = bd.split_at(bd.len().saturating_sub(2));
                    let day = day_no(d).ok_or_else(unsupported)?;
                    let n: i8 = match (n, setpos.as_deref()) {
                        ("", Some(p)) => p.parse().map_err(|_| unsupported())?,
                        (n, None) if !n.is_empty() => n.trim_start_matches('+').parse().map_err(|_| unsupported())?,
                        _ => return Err(unsupported()),
                    };
                    if !(n == -1 || (1..=4).contains(&n)) {
                        return Err(unsupported());
                    }
                    week_of_month = Some(n);
                    weekdays = vec![day];
                }
                (None, None) => month_day = Some(local.date().day() as u8),
                _ => return Err(unsupported()),
            }
        }
    }
    let max_runs = parts.get("COUNT").map(|c| c.parse::<i32>().map_err(|_| unsupported())).transpose()?;
    let ends_on = match parts.get("UNTIL") {
        Some(u) => {
            let p = Prop { name: "UNTIL".into(), params: vec![], value: u.clone() };
            let (t, date, _) = when_of(&p, &Component::default(), chrono_tz::UTC).map_err(|_| unsupported())?;
            Some(if date || all_day { t.date_naive() } else { t.with_timezone(&tz).date_naive() })
        }
        None => None,
    };
    let rule = Recurrence {
        freq: freq.into(),
        interval,
        weekdays,
        month_day,
        month,
        week_of_month,
        time: local.format("%H:%M").to_string(),
        timezone: if all_day { "UTC".into() } else { tz.name().to_string() },
        starts_on: local.date(),
        ends_on,
        max_runs,
    };
    rule.validate().map_err(|e| e.to_string())?;
    Ok(rule)
}

/// The VEVENTs of a calendar object. `floating`: the zone for times given without one.
pub fn events(cal: &Component, floating: Tz) -> Result<Vec<Parsed>, String> {
    let mut out = Vec::new();
    for v in cal.children.iter().filter(|c| c.name == "VEVENT") {
        let uid = v.prop("UID").map(|p| p.value.trim().to_string()).filter(|u| !u.is_empty() && u.len() <= 500).ok_or("each event needs a UID")?;
        let dtstart = v.prop("DTSTART").ok_or("each event needs a start (DTSTART)")?;
        let (start, all_day, tz) = when_of(dtstart, cal, floating)?;
        let end = match (v.prop("DTEND"), v.prop("DURATION")) {
            (Some(e), _) => when_of(e, cal, floating)?.0,
            (None, Some(d)) => start + Duration::seconds(duration_secs(&d.value).ok_or("can't read the DURATION")?),
            (None, None) if all_day => start + Duration::days(1),
            (None, None) => start,
        };
        if end < start {
            return Err("an event ends before it starts".into());
        }
        let zone_used = tz.unwrap_or(if all_day { chrono_tz::UTC } else { floating_or_utc(dtstart, floating) });
        let rule = match v.prop("RRULE") {
            Some(r) => Some(rule_from(&r.value, start, zone_used, all_day)?),
            None => None,
        };
        let mut exdates = Vec::new();
        for x in v.all("EXDATE") {
            for part in x.value.split(',') {
                let p = Prop { value: part.to_string(), ..x.clone() };
                exdates.push(when_of(&p, cal, floating)?.0);
            }
        }
        let recurrence_id = v.prop("RECURRENCE-ID").map(|p| when_of(p, cal, floating).map(|x| x.0)).transpose()?;
        let class = v.prop("CLASS").map(|c| c.value.trim().to_ascii_uppercase());
        let alarm_minutes = v
            .children
            .iter()
            .filter(|a| a.name == "VALARM")
            .filter_map(|a| a.prop("TRIGGER"))
            .filter(|t| !t.param("VALUE").is_some_and(|x| x.eq_ignore_ascii_case("DATE-TIME")) && !t.param("RELATED").is_some_and(|x| x.eq_ignore_ascii_case("END")))
            .filter_map(|t| duration_secs(&t.value))
            .filter(|s| *s <= 0)
            .map(|s| ((-s) / 60).min(40320) as i32)
            .max();
        let attendees = v
            .all("ATTENDEE")
            .filter_map(|a| mailto(&a.value).map(|m| (m, a.param("CN").map(str::to_string).filter(|c| !c.is_empty()), a.param("PARTSTAT").unwrap_or("NEEDS-ACTION").to_ascii_uppercase())))
            .collect();
        out.push(Parsed {
            uid,
            summary: v.text("SUMMARY").map(|s| s.chars().take(300).collect()).unwrap_or_else(|| "(no title)".into()),
            description: v.text("DESCRIPTION"),
            location: v.text("LOCATION").map(|s| s.chars().take(300).collect()),
            start,
            end,
            all_day,
            timezone: if all_day { "UTC".into() } else { zone_used.name().to_string() },
            rule,
            exdates,
            recurrence_id,
            private: matches!(class.as_deref(), Some("PRIVATE") | Some("CONFIDENTIAL")),
            cancelled: v.prop("STATUS").is_some_and(|s| s.value.trim().eq_ignore_ascii_case("CANCELLED")),
            alarm_minutes,
            attendees,
        });
    }
    Ok(out)
}

/// UTC times stay in UTC; floating ones are in the person's zone.
fn floating_or_utc(p: &Prop, floating: Tz) -> Tz {
    if p.value.trim().ends_with('Z') { chrono_tz::UTC } else { floating }
}

/// NERP's status for a PARTSTAT.
pub fn status_of(partstat: &str) -> &'static str {
    match partstat {
        "ACCEPTED" => "accepted",
        "DECLINED" => "declined",
        "TENTATIVE" => "tentative",
        _ => "pending",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn rule(freq: &str) -> Recurrence {
        Recurrence {
            freq: freq.into(), interval: 1, weekdays: vec![], month_day: None, month: None, week_of_month: None, time: "10:00".into(), timezone: "Europe/Berlin".into(),
            starts_on: NaiveDate::from_ymd_opt(2026, 3, 2).unwrap(), ends_on: None, max_runs: None,
        }
    }

    #[test]
    fn escaping_and_folding() {
        assert_eq!(esc("a;b,c\\d\ne"), "a\\;b\\,c\\\\d\\ne");
        let long = format!("SUMMARY:{}", "é".repeat(60));
        let folded = fold(&long);
        assert!(folded.split("\r\n").all(|l| l.len() <= 75));
        assert_eq!(folded.replace("\r\n ", "").trim_end(), long);
    }

    #[test]
    fn rrules() {
        let mut r = rule("weekly");
        r.weekdays = vec![4, 1];
        r.interval = 2;
        r.max_runs = Some(5);
        assert_eq!(rrule(&r, false), "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,TH;WKST=MO;COUNT=5");
        let mut m = rule("monthly");
        m.month_day = Some(31);
        m.ends_on = NaiveDate::from_ymd_opt(2026, 6, 30);
        assert_eq!(rrule(&m, false), "FREQ=MONTHLY;BYMONTHDAY=-1;UNTIL=20260630T215959Z");
        m.month_day = Some(30);
        assert!(rrule(&m, true).starts_with("FREQ=MONTHLY;BYMONTHDAY=28,29,30;BYSETPOS=-1;UNTIL=20260630"));
    }

    #[test]
    fn berlin_has_both_changes() {
        let z = vtimezone(chrono_tz::Europe::Berlin, 2026, 2026);
        assert!(z.contains("BEGIN:DAYLIGHT\r\nDTSTART:20260329T020000\r\nTZOFFSETFROM:+0100\r\nTZOFFSETTO:+0200"), "{z}");
        assert!(z.contains("BEGIN:STANDARD\r\nDTSTART:20261025T030000\r\nTZOFFSETFROM:+0200\r\nTZOFFSETTO:+0100"), "{z}");
        let k = vtimezone(chrono_tz::Asia::Kolkata, 2026, 2027);
        assert!(k.contains("TZOFFSETFROM:+0530\r\nTZOFFSETTO:+0530"), "{k}");
    }

    #[test]
    fn repeating_event_in_local_time() {
        let start = Utc.with_ymd_and_hms(2026, 3, 2, 9, 0, 0).unwrap();
        let e = VEvent {
            uid: "x@nerp".into(), sequence: 2, start, end: start + Duration::minutes(30), all_day: false, timezone: "Europe/Berlin".into(),
            rule: Some(rule("daily")), exdates: vec![start + Duration::days(1)], recurrence_id: None, summary: "Stand-up".into(), location: None,
            description: None, url: None, organizer: None, attendees: vec![], cancelled: false, private: false, last_modified: None, alarm_minutes: Some(10),
        };
        let c = calendar(Some("REQUEST"), None, &[e]);
        assert!(c.contains("DTSTART;TZID=Europe/Berlin:20260302T100000"), "{c}");
        assert!(c.contains("EXDATE;TZID=Europe/Berlin:20260303T100000"), "{c}");
        assert!(c.contains("RRULE:FREQ=DAILY\r\n"), "{c}");
        assert!(c.contains("BEGIN:VTIMEZONE"), "{c}");
        assert!(c.contains("TRIGGER:-PT10M"), "{c}");
    }

    #[test]
    fn reads_what_apps_send() {
        let text = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Apple Inc.//macOS//EN\r\nBEGIN:VTIMEZONE\r\nTZID:/citadel.org/20250101_1/Europe/Berlin\r\nX-LIC-LOCATION:Europe/Berlin\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:abc-123\r\nDTSTART;TZID=/citadel.org/20250101_1/Europe/Berlin:20260302T100000\r\nDURATION:PT45M\r\nSUMMARY:Stand-up\\, daily\r\nDESCRIPTION:Line one\\nline two\r\n\
RRULE:FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20260331T225959Z\r\nEXDATE;TZID=Europe/Berlin:20260304T100000,20260309T100000\r\nCLASS:PRIVATE\r\n\
ATTENDEE;CN=\"Doe, Jane\";PARTSTAT=ACCEPTED:mailto:Jane@Example.com\r\nATTENDEE;CN=Bob:MAILTO:bob@example.com\r\nBEGIN:VALARM\r\nTRIGGER:-PT15M\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:abc-123\r\nRECURRENCE-ID;TZID=Europe/Berlin:20260311T100000\r\nDTSTART;TZID=Europe/Berlin:20260311T140000\r\nDTEND;TZID=Europe/Berlin:20260311T143000\r\nSUMMARY:Moved\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let cal = parse(text).unwrap();
        let ev = events(&cal, chrono_tz::UTC).unwrap();
        assert_eq!(ev.len(), 2);
        let m = &ev[0];
        assert_eq!(m.summary, "Stand-up, daily");
        assert_eq!(m.description.as_deref(), Some("Line one\nline two"));
        assert_eq!(m.start, Utc.with_ymd_and_hms(2026, 3, 2, 9, 0, 0).unwrap());
        assert_eq!(m.end - m.start, Duration::minutes(45));
        assert_eq!(m.timezone, "Europe/Berlin");
        let r = m.rule.as_ref().unwrap();
        assert_eq!((r.freq.as_str(), r.weekdays.clone(), r.time.as_str()), ("weekly", vec![1, 3], "10:00"));
        assert_eq!(r.ends_on, NaiveDate::from_ymd_opt(2026, 4, 1));
        assert_eq!(m.exdates.len(), 2);
        assert!(m.private);
        assert_eq!(m.alarm_minutes, Some(15));
        assert_eq!(m.attendees[0], ("jane@example.com".into(), Some("Doe, Jane".into()), "ACCEPTED".into()));
        assert_eq!(m.attendees[1].0, "bob@example.com");
        assert_eq!(ev[1].recurrence_id, Some(Utc.with_ymd_and_hms(2026, 3, 11, 9, 0, 0).unwrap()));
    }

    #[test]
    fn repeat_rules_round_trip() {
        let start = Utc.with_ymd_and_hms(2026, 1, 13, 9, 0, 0).unwrap(); // a Tuesday
        for rr in ["FREQ=DAILY;INTERVAL=3;COUNT=5", "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,TH;WKST=MO", "FREQ=MONTHLY;BYMONTHDAY=-1", "FREQ=MONTHLY;BYMONTHDAY=28,29,30;BYSETPOS=-1",
                   "FREQ=MONTHLY;BYDAY=2TU", "FREQ=MONTHLY;BYDAY=-1FR", "FREQ=YEARLY;BYMONTH=11;BYDAY=4TH", "FREQ=YEARLY;BYMONTH=3;BYMONTHDAY=14"] {
            let r = rule_from(rr, start, chrono_tz::UTC, false).unwrap_or_else(|e| panic!("{rr}: {e}"));
            let back = rrule(&r, false);
            let again = rule_from(&back, start, chrono_tz::UTC, false).unwrap();
            assert_eq!(r, again, "{rr} → {back}");
        }
        assert!(rule_from("FREQ=MONTHLY;BYDAY=TU;BYSETPOS=2", start, chrono_tz::UTC, false).unwrap().week_of_month == Some(2));
        assert!(rule_from("FREQ=HOURLY", start, chrono_tz::UTC, false).is_err());
        assert!(rule_from("FREQ=WEEKLY;BYHOUR=9", start, chrono_tz::UTC, false).is_err());
        let r = rule_from("FREQ=YEARLY", start, chrono_tz::UTC, false).unwrap();
        assert_eq!((r.month, r.month_day), (Some(1), Some(13)));
    }

    #[test]
    fn durations_and_zones() {
        assert_eq!(duration_secs("-PT15M"), Some(-900));
        assert_eq!(duration_secs("P1DT2H"), Some(93600));
        assert_eq!(duration_secs("P2W"), Some(1209600));
        assert_eq!(duration_secs("PT"), Some(0));
        assert_eq!(duration_secs("X"), None);
        let empty = Component::default();
        assert_eq!(zone("W. Europe Standard Time", &empty), Some(chrono_tz::Europe::Berlin));
        assert_eq!(zone("/mozilla.org/20070129_1/America/New_York", &empty), Some(chrono_tz::America::New_York));
        assert_eq!(zone("Nowhere", &empty), None);
    }
}
