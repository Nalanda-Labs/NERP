//! Turns a chart's query (dataset, groupings, measures, filters, sorting) into one SQL
//! PostgreSQL statement over the dataset's rows (NERP itself or an outside database). Only the dataset's own column names reach the SQL (checked against the
//! dataset and quoted); every value is a bound parameter. Each result row comes back as
//! one JSON array.

use chrono::{Datelike, Duration, Months, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ty {
    Text,
    Number,
    /// A calendar date.
    Date,
    /// An instant (shown and grouped by the viewer's local date).
    Time,
    Bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Column {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub ty: Ty,
}

#[derive(Debug, Clone, PartialEq)]
pub enum P {
    Text(String),
    Num(f64),
    Bool(bool),
    Date(NaiveDate),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Spec {
    /// `nerp:<key>` for built-in data, else a dataset id.
    pub dataset: String,
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub measures: Vec<Measure>,
    /// A plain table of these columns (no groups or measures).
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// The column the dashboard's period applies to.
    pub date_field: Option<String>,
    pub sort: Option<Sort>,
    pub limit: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Group {
    pub field: String,
    /// For dates: day, week, month, quarter, year.
    pub bucket: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Measure {
    /// count, count_distinct, sum, avg, min, max.
    pub agg: String,
    pub field: Option<String>,
    pub label: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Filter {
    pub field: String,
    /// eq, ne, gt, gte, lt, lte, contains, not_contains, in, between, empty, not_empty.
    pub op: String,
    #[serde(default)]
    pub value: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sort {
    /// g0, g1, m0… for grouped charts; a column key for tables.
    pub by: String,
    #[serde(default)]
    pub desc: bool,
}

/// What the dashboard adds: its period (from, to exclusive) and filters, and the time zone
/// that local dates are taken in (NERP's own data only).
#[derive(Default)]
pub struct Ctx<'a> {
    pub period: Option<(NaiveDate, NaiveDate)>,
    pub extra: &'a [Filter],
    pub tz: Option<&'a str>,
}

#[derive(Debug)]
pub struct Built {
    pub sql: String,
    pub params: Vec<P>,
    pub columns: Vec<Column>,
}

pub const MAX_LIMIT: i64 = 1000;

pub fn valid_key(k: &str) -> bool {
    !k.is_empty() && k.len() <= 63 && k.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') && !k.as_bytes()[0].is_ascii_digit()
}

struct B<'a> {
    params: Vec<P>,
    tz: Option<&'a str>,
    tz_ph: Option<String>,
}

impl B<'_> {
    fn ph(&mut self, p: P) -> String {
        self.params.push(p);
        format!("${}", self.params.len())
    }
    fn q(&self, k: &str) -> String {
        debug_assert!(valid_key(k));
        format!("t.\"{k}\"")
    }
    /// The column's local date.
    fn local(&mut self, c: &Column) -> String {
        let col = self.q(&c.key);
        match c.ty {
            Ty::Time => match self.tz {
                Some(tz) => {
                    let ph = match &self.tz_ph {
                        Some(p) => p.clone(),
                        None => {
                            let p = self.ph(P::Text(tz.to_string()));
                            self.tz_ph = Some(p.clone());
                            p
                        }
                    };
                    format!("({col} AT TIME ZONE {ph}::text)::date")
                }
                None => format!("{col}::date"),
            },
            _ => col,
        }
    }
    fn bucket(&mut self, c: &Column, bucket: &str) -> Result<String, String> {
        let base = self.local(c);
        Ok(match bucket {
            "day" => base,
            "week" | "month" | "quarter" | "year" => format!("date_trunc('{bucket}', ({base})::timestamp)::date"),
            _ => return Err(format!("dates group by day, week, month, quarter or year, not “{bucket}”")),
        })
    }
}

fn col<'c>(cols: &'c [Column], key: &str) -> Result<&'c Column, String> {
    cols.iter().find(|c| c.key == key).ok_or_else(|| format!("the dataset has no column “{key}”"))
}

fn date_of(v: &Value) -> Result<NaiveDate, String> {
    v.as_str().and_then(|s| NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").ok()).ok_or_else(|| "give dates as YYYY-MM-DD".to_string())
}

fn num_of(v: &Value) -> Result<f64, String> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).filter(|n: &f64| n.is_finite()).ok_or_else(|| "give a number".to_string())
}

fn scalar(c: &Column, v: &Value) -> Result<P, String> {
    Ok(match c.ty {
        Ty::Number => P::Num(num_of(v)?),
        Ty::Date | Ty::Time => P::Date(date_of(v)?),
        Ty::Bool => P::Bool(v.as_bool().or_else(|| v.as_str().map(|s| s == "true")).ok_or("give true or false")?),
        Ty::Text => P::Text(match v {
            Value::String(s) => s.clone(),
            Value::Null => return Err("give a value".into()),
            other => other.to_string(),
        }),
    })
}

fn like(s: &str) -> String {
    format!("%{}%", s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))
}

fn condition(b: &mut B, c: &Column, f: &Filter) -> Result<String, String> {
    let dated = matches!(c.ty, Ty::Date | Ty::Time);
    let x = if dated { b.local(c) } else { b.q(&c.key) };
    let cmp = |op: &str| match op {
        "eq" => "=",
        "ne" => "<>",
        "gt" => ">",
        "gte" => ">=",
        "lt" => "<",
        _ => "<=",
    };
    Ok(match f.op.as_str() {
        "empty" if c.ty == Ty::Text => format!("({x} IS NULL OR {x} = '')"),
        "not_empty" if c.ty == Ty::Text => format!("({x} IS NOT NULL AND {x} <> '')"),
        "empty" => format!("{x} IS NULL"),
        "not_empty" => format!("{x} IS NOT NULL"),
        "eq" | "ne" => {
            let p = scalar(c, &f.value)?;
            let ph = b.ph(p);
            if f.op == "eq" { format!("{x} = {ph}") } else { format!("({x} IS NULL OR {x} <> {ph})") }
        }
        "gt" | "gte" | "lt" | "lte" if c.ty == Ty::Number || dated => {
            let p = scalar(c, &f.value)?;
            format!("{x} {} {}", cmp(&f.op), b.ph(p))
        }
        "between" if c.ty == Ty::Number || dated => {
            let arr = f.value.as_array().filter(|a| a.len() == 2).ok_or("“between” takes two values")?;
            let (lo, hi) = (scalar(c, &arr[0])?, scalar(c, &arr[1])?);
            let (lo, hi) = (b.ph(lo), b.ph(hi));
            format!("({x} >= {lo} AND {x} <= {hi})")
        }
        "contains" | "not_contains" if c.ty == Ty::Text => {
            let s = f.value.as_str().ok_or("give some text")?;
            let ph = b.ph(P::Text(like(s)));
            if f.op == "contains" { format!("{x} ILIKE {ph}") } else { format!("({x} IS NULL OR {x} NOT ILIKE {ph})") }
        }
        "in" => {
            let arr = f.value.as_array().filter(|a| !a.is_empty() && a.len() <= 200).ok_or("“is one of” takes 1 to 200 values")?;
            let mut phs = Vec::new();
            for v in arr {
                let p = scalar(c, v)?;
                phs.push(b.ph(p));
            }
            format!("{x} IN ({})", phs.join(", "))
        }
        op => return Err(format!("“{op}” does not apply to “{}”", c.label)),
    })
}

fn measure_label(m: &Measure, c: Option<&Column>) -> String {
    if let Some(l) = m.label.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
        return l.to_string();
    }
    let name = c.map(|c| c.label.as_str()).unwrap_or("");
    match m.agg.as_str() {
        "count" => "Count".into(),
        "count_distinct" => format!("Different {name}"),
        "sum" => format!("Total {name}"),
        "avg" => format!("Average {name}"),
        "min" => format!("Smallest {name}"),
        _ => format!("Largest {name}"),
    }
}

/// The statement for a chart.
pub fn build(spec: &Spec, cols: &[Column], from_sql: &str, ctx: &Ctx) -> Result<Built, String> {
    let mut b = B { params: Vec::new(), tz: ctx.tz, tz_ph: None };
    let raw = spec.groups.is_empty() && spec.measures.is_empty() && !spec.columns.is_empty();
    let mut select = Vec::new();
    let mut out = Vec::new();
    let limit = spec.limit.unwrap_or(if raw { 100 } else { 200 }).clamp(1, MAX_LIMIT);
    if raw {
        if spec.columns.len() > 30 {
            return Err("a table shows at most 30 columns".into());
        }
        for k in &spec.columns {
            let c = col(cols, k)?;
            select.push(b.q(&c.key));
            out.push(c.clone());
        }
    } else {
        if spec.groups.len() > 2 {
            return Err("group by at most two columns".into());
        }
        if spec.measures.len() > 6 {
            return Err("at most six figures per chart".into());
        }
        for (i, g) in spec.groups.iter().enumerate() {
            let c = col(cols, &g.field)?;
            let (expr, label) = match c.ty {
                Ty::Date | Ty::Time => {
                    let bk = g.bucket.as_deref().unwrap_or("month");
                    (b.bucket(c, bk)?, format!("{} ({bk})", c.label))
                }
                _ => (b.q(&c.key), c.label.clone()),
            };
            select.push(format!("{expr} AS g{i}"));
            out.push(Column { key: format!("g{i}"), label, ty: if matches!(c.ty, Ty::Time) { Ty::Date } else { c.ty } });
        }
        let default = [Measure { agg: "count".into(), field: None, label: None }];
        let measures = if spec.measures.is_empty() { &default[..] } else { &spec.measures[..] };
        for (i, m) in measures.iter().enumerate() {
            let c = match m.field.as_deref().filter(|f| !f.is_empty()) {
                Some(f) => Some(col(cols, f)?),
                None => None,
            };
            let need = |ok: &[Ty]| -> Result<String, String> {
                match c {
                    Some(c) if ok.contains(&c.ty) => Ok(b.q(&c.key)),
                    Some(c) => Err(format!("cannot take “{}” of “{}”", m.agg, c.label)),
                    None => Err(format!("“{}” needs a column", m.agg)),
                }
            };
            let (expr, ty) = match m.agg.as_str() {
                "count" => ("COUNT(*)".to_string(), Ty::Number),
                "count_distinct" => (format!("COUNT(DISTINCT {})", need(&[Ty::Text, Ty::Number, Ty::Date, Ty::Time, Ty::Bool])?), Ty::Number),
                "sum" => (format!("SUM({})", need(&[Ty::Number])?), Ty::Number),
                "avg" => {
                    let x = need(&[Ty::Number])?;
                    (format!("ROUND(AVG({x})::numeric, 2)"), Ty::Number)
                }
                "min" | "max" => {
                    let x = need(&[Ty::Number, Ty::Date, Ty::Time])?;
                    (format!("{}({x})", m.agg.to_uppercase()), c.map(|c| c.ty).unwrap_or(Ty::Number))
                }
                a => return Err(format!("unknown figure “{a}”")),
            };
            select.push(format!("{expr} AS m{i}"));
            out.push(Column { key: format!("m{i}"), label: measure_label(m, c), ty });
        }
    }

    let mut conds = Vec::new();
    for f in spec.filters.iter() {
        conds.push(condition(&mut b, col(cols, &f.field)?, f)?);
    }
    // Dashboard filters apply where the dataset has the column.
    for f in ctx.extra {
        if let Ok(c) = col(cols, &f.field) {
            conds.push(condition(&mut b, c, f)?);
        }
    }
    if let (Some((from, to)), Some(df)) = (ctx.period, spec.date_field.as_deref().filter(|s| !s.is_empty())) {
        let c = col(cols, df)?;
        if !matches!(c.ty, Ty::Date | Ty::Time) {
            return Err(format!("“{}” is not a date", c.label));
        }
        let x = b.local(c);
        let (f, t) = (b.ph(P::Date(from)), b.ph(P::Date(to)));
        conds.push(format!("{x} >= {f} AND {x} < {t}"));
    }
    let where_ = if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) };
    let from_sql = from_sql.trim().trim_end_matches(';');
    let wrap = |inner: &str| format!("json_build_array({inner})::text");

    // Sorting: the asked column, else dates in order and other groups by the first figure.
    let sort = match &spec.sort {
        Some(s) => {
            if !out.iter().any(|c| c.key == s.by) {
                return Err(format!("cannot sort by “{}”", s.by));
            }
            Some((s.by.clone(), s.desc))
        }
        None if raw => None,
        None if spec.groups.is_empty() => None,
        None if matches!(out[0].ty, Ty::Date) => Some(("g0".into(), false)),
        None => Some((format!("m{}", 0), true)),
    };
    let sql = if raw {
        let order = match &sort {
            Some((k, desc)) => {
                let x = b.q(k);
                format!(" ORDER BY ({x} IS NULL), {x} {}", if *desc { "DESC" } else { "ASC" })
            }
            None => String::new(),
        };
        format!("SELECT {} FROM ({from_sql}) t{where_}{order} LIMIT {limit}", wrap(&select.join(", ")))
    } else {
        let group = if spec.groups.is_empty() { String::new() } else { format!(" GROUP BY {}", (1..=spec.groups.len()).map(|i| i.to_string()).collect::<Vec<_>>().join(", ")) };
        let outer: Vec<String> = out.iter().map(|c| format!("r.{}", c.key)).collect();
        let order = match &sort {
            Some((k, desc)) => format!(" ORDER BY (r.{k} IS NULL), r.{k} {}", if *desc { "DESC" } else { "ASC" }),
            None => String::new(),
        };
        format!("SELECT {} FROM (SELECT {} FROM ({from_sql}) t{where_}{group}) r{order} LIMIT {limit}", wrap(&outer.join(", ")), select.join(", "))
    };
    Ok(Built { sql, params: b.params, columns: out })
}

/// Every row of a dataset (all its columns), for downloading.
pub fn export(cols: &[Column], from_sql: &str, limit: i64) -> String {
    let b = B { params: Vec::new(), tz: None, tz_ph: None };
    let sel: Vec<String> = cols.iter().take(100).map(|c| b.q(&c.key)).collect();
    let wrap = format!("json_build_array({})::text", sel.join(", "));
    format!("SELECT {wrap} FROM ({}) t LIMIT {limit}", from_sql.trim().trim_end_matches(';'))
}

/// A dashboard period as dates: from, and to (exclusive). `None` is all time.
pub fn period(v: &Value, today: NaiveDate) -> Result<Option<(NaiveDate, NaiveDate)>, String> {
    if v.is_null() || v.as_object().is_some_and(|o| o.is_empty()) {
        return Ok(None);
    }
    if let (Some(f), Some(t)) = (v.get("from"), v.get("to")) {
        let (f, t) = (date_of(f)?, date_of(t)?);
        if t < f {
            return Err("the period ends before it starts".into());
        }
        return Ok(Some((f, t + Duration::days(1))));
    }
    let preset = v.get("preset").and_then(Value::as_str).unwrap_or("all");
    let month = |d: NaiveDate| d.with_day(1).unwrap();
    let quarter = |d: NaiveDate| NaiveDate::from_ymd_opt(d.year(), (d.month0() / 3) * 3 + 1, 1).unwrap();
    let year = |d: NaiveDate| NaiveDate::from_ymd_opt(d.year(), 1, 1).unwrap();
    let week = |d: NaiveDate| d - Duration::days(d.weekday().num_days_from_monday() as i64);
    let next = |d: NaiveDate, m: u32| d + Months::new(m);
    let prev = |d: NaiveDate, m: u32| d - Months::new(m);
    let p = match preset {
        "all" => return Ok(None),
        "today" => (today, today + Duration::days(1)),
        "yesterday" => (today - Duration::days(1), today),
        "this_week" => (week(today), week(today) + Duration::days(7)),
        "last_week" => (week(today) - Duration::days(7), week(today)),
        "this_month" => (month(today), next(month(today), 1)),
        "last_month" => (prev(month(today), 1), month(today)),
        "this_quarter" => (quarter(today), next(quarter(today), 3)),
        "last_quarter" => (prev(quarter(today), 3), quarter(today)),
        "this_year" => (year(today), next(year(today), 12)),
        "last_year" => (prev(year(today), 12), year(today)),
        "last_7_days" => (today - Duration::days(6), today + Duration::days(1)),
        "last_30_days" => (today - Duration::days(29), today + Duration::days(1)),
        "last_90_days" => (today - Duration::days(89), today + Duration::days(1)),
        "last_365_days" => (today - Duration::days(364), today + Duration::days(1)),
        p => return Err(format!("unknown period “{p}”")),
    };
    Ok(Some(p))
}

/// The period just before: whole months stay whole months.
pub fn previous((from, to): (NaiveDate, NaiveDate)) -> (NaiveDate, NaiveDate) {
    if from.day() == 1 && to.day() == 1 {
        let months = (to.year() - from.year()) * 12 + to.month() as i32 - from.month() as i32;
        if months > 0 {
            return (from - Months::new(months as u32), from);
        }
    }
    (from - (to - from), from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cols() -> Vec<Column> {
        [("stage", Ty::Text), ("amount", Ty::Number), ("created_at", Ty::Time), ("close_date", Ty::Date), ("won", Ty::Bool)]
            .into_iter()
            .map(|(k, ty)| Column { key: k.into(), label: k.into(), ty })
            .collect()
    }

    fn spec(v: Value) -> Spec {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn grouped_by_month_in_the_viewers_zone() {
        let s = spec(json!({ "dataset": "nerp:deals", "groups": [{ "field": "created_at", "bucket": "month" }], "measures": [{ "agg": "sum", "field": "amount" }],
            "filters": [{ "field": "stage", "op": "contains", "value": "50%_" }], "date_field": "close_date" }));
        let f = [Filter { field: "won".into(), op: "eq".into(), value: json!(true) }, Filter { field: "nope".into(), op: "eq".into(), value: json!(1) }];
        let from = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let b = build(&s, &cols(), "SELECT 1", &Ctx { period: Some((from, from + Months::new(12))), extra: &f, tz: Some("Europe/Berlin") }).unwrap();
        assert_eq!(
            b.sql,
            "SELECT json_build_array(r.g0, r.m0)::text FROM (SELECT date_trunc('month', ((t.\"created_at\" AT TIME ZONE $1::text)::date)::timestamp)::date AS g0, SUM(t.\"amount\") AS m0 FROM (SELECT 1) t \
             WHERE t.\"stage\" ILIKE $2 AND t.\"won\" = $3 AND t.\"close_date\" >= $4 AND t.\"close_date\" < $5 GROUP BY 1) r ORDER BY (r.g0 IS NULL), r.g0 ASC LIMIT 200"
        );
        assert_eq!(b.params[1], P::Text("%50\\%\\_%".into()));
        assert_eq!(b.columns[0].ty, Ty::Date);
        assert_eq!(b.columns[1].label, "Total amount");
    }

    #[test]
    fn tables() {
        let s = spec(json!({ "dataset": "x", "columns": ["stage", "amount"], "filters": [{ "field": "amount", "op": "in", "value": [1, "2"] }], "sort": { "by": "amount", "desc": true }, "limit": 5000 }));
        let b = build(&s, &cols(), "SELECT * FROM deals;", &Ctx::default()).unwrap();
        assert_eq!(b.sql, "SELECT json_build_array(t.\"stage\", t.\"amount\")::text FROM (SELECT * FROM deals) t WHERE t.\"amount\" IN ($1, $2) ORDER BY (t.\"amount\" IS NULL), t.\"amount\" DESC LIMIT 1000");
        assert_eq!(b.params, vec![P::Num(1.0), P::Num(2.0)]);
        let g = spec(json!({ "dataset": "x", "groups": [{ "field": "created_at", "bucket": "quarter" }] }));
        assert!(build(&g, &cols(), "t", &Ctx::default()).unwrap().sql.contains("date_trunc('quarter', (t.\"created_at\"::date)::timestamp)::date AS g0, COUNT(*) AS m0"));
    }

    #[test]
    fn refuses_what_does_not_fit() {
        let bad = [
            json!({ "dataset": "x", "groups": [{ "field": "stage\"; DROP" }] }),
            json!({ "dataset": "x", "measures": [{ "agg": "sum", "field": "stage" }] }),
            json!({ "dataset": "x", "measures": [{ "agg": "median", "field": "amount" }] }),
            json!({ "dataset": "x", "filters": [{ "field": "stage", "op": "gt", "value": "a" }] }),
            json!({ "dataset": "x", "groups": [{ "field": "created_at", "bucket": "hour" }] }),
            json!({ "dataset": "x", "groups": [{ "field": "stage" }], "sort": { "by": "m3" } }),
        ];
        for v in bad {
            assert!(build(&spec(v.clone()), &cols(), "t", &Ctx::default()).is_err(), "{v}");
        }
        assert!(!valid_key("Stage") && !valid_key("1a") && !valid_key("a b") && valid_key("a_1"));
    }

    #[test]
    fn periods() {
        let d = NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();
        let ymd = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert_eq!(period(&json!({ "preset": "this_month" }), d).unwrap(), Some((ymd(2026, 11, 1), ymd(2026, 12, 1))));
        assert_eq!(period(&json!({ "preset": "last_quarter" }), d).unwrap(), Some((ymd(2026, 7, 1), ymd(2026, 10, 1))));
        assert_eq!(period(&json!({ "preset": "this_week" }), d).unwrap(), Some((ymd(2026, 11, 9), ymd(2026, 11, 16))));
        assert_eq!(period(&json!({ "from": "2026-01-05", "to": "2026-01-05" }), d).unwrap(), Some((ymd(2026, 1, 5), ymd(2026, 1, 6))));
        assert_eq!(period(&json!({}), d).unwrap(), None);
        assert_eq!(previous((ymd(2026, 11, 1), ymd(2026, 12, 1))), (ymd(2026, 10, 1), ymd(2026, 11, 1)));
        assert_eq!(previous((ymd(2026, 11, 5), ymd(2026, 11, 12))), (ymd(2026, 10, 29), ymd(2026, 11, 5)));
    }
}
