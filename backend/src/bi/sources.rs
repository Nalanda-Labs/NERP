//! Where custom datasets come from: outside PostgreSQL databases (a table or a
//! query, read in a read-only session with a time limit), and CSV or JSON files (uploaded,
//! or fetched from a URL and refreshed), whose rows are kept in `bi_rows`.

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::{
    postgres::{PgConnectOptions, PgConnection, PgSslMode},
    Column as _, ConnectOptions, Connection, Executor, Statement as _, TypeInfo,
};
use std::time::Duration;
use uuid::Uuid;

use super::query::{Built, Column, Ty, P};
use crate::state::AppState;

pub const MAX_ROWS: usize = 200_000;
pub const MAX_COLUMNS: usize = 100;
const TIMEOUT_SECS: u64 = 20;

#[derive(sqlx::FromRow, Clone)]
pub struct Source {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub host: String,
    pub port: i32,
    pub database: String,
    pub username: String,
    pub password: Option<Vec<u8>>,
    pub tls: bool,
}

pub const SOURCE_COLS: &str = "id, name, kind, host, port, database, username, password, tls";

/// A read-only session on an outside PostgreSQL database.
pub struct Conn(PgConnection);

fn short(e: impl std::fmt::Display) -> String {
    let s = e.to_string();
    s.chars().take(400).collect()
}

/// A read-only session with a time limit on every statement.
pub async fn connect(state: &AppState, s: &Source) -> Result<Conn, String> {
    let pw = s.password.as_deref().and_then(|p| crate::crypto::open(&state.config.data_key, p)).unwrap_or_default();
    let port = u16::try_from(s.port).map_err(|_| "the port is not valid".to_string())?;
    let wait = Duration::from_secs(10);
    let late = || format!("{}:{} did not answer within 10 seconds", s.host, s.port);
    match s.kind.as_str() {
        "postgres" => {
            let o = PgConnectOptions::new()
                .host(&s.host)
                .port(port)
                .database(&s.database)
                .username(&s.username)
                .password(&pw)
                .ssl_mode(if s.tls { PgSslMode::Require } else { PgSslMode::Prefer })
                .application_name("NERP BI")
                .statement_cache_capacity(0)
                .disable_statement_logging();
            let mut c = tokio::time::timeout(wait, o.connect()).await.map_err(|_| late())?.map_err(short)?;
            c.execute(sqlx::AssertSqlSafe(format!("SET statement_timeout = {}", TIMEOUT_SECS * 1000))).await.map_err(short)?;
            c.execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY").await.map_err(short)?;
            Ok(Conn(c))
        }
        k => Err(format!("unknown kind of database “{k}”")),
    }
}

macro_rules! bind_all {
    ($q:expr, $ps:expr) => {{
        let mut q = $q;
        for p in $ps {
            q = match p {
                P::Text(s) => q.bind(s.clone()),
                P::Num(n) => q.bind(*n),
                P::Bool(b) => q.bind(*b),
                P::Date(d) => q.bind(*d),
            };
        }
        q
    }};
}
pub(crate) use bind_all;

impl Conn {
    pub async fn rows(&mut self, b: &Built) -> Result<Vec<String>, String> {
        let sql = sqlx::AssertSqlSafe(b.sql.clone());
        let fut = async {
            bind_all!(sqlx::query_scalar::<_, String>(sql), &b.params).fetch_all(&mut self.0).await
        };
        tokio::time::timeout(Duration::from_secs(TIMEOUT_SECS + 5), fut).await.map_err(|_| "the database took too long".to_string())?.map_err(short)
    }

    /// The columns a query returns (named as they come; `key`s made from them).
    pub async fn describe(&mut self, query: &str) -> Result<Vec<Column>, String> {
        let sql = format!("SELECT * FROM ({}) s LIMIT 0", strip(query));
        let cols: Vec<(String, String)> = self.0.prepare(sqlx::SqlSafeStr::into_sql_str(sqlx::AssertSqlSafe(sql))).await.map_err(short)?.columns().iter().map(|c| (c.name().to_string(), c.type_info().name().to_string())).collect();
        if cols.is_empty() {
            return Err("the query returns no columns".into());
        }
        if cols.len() > MAX_COLUMNS {
            return Err(format!("the query returns more than {MAX_COLUMNS} columns"));
        }
        let mut keys = Vec::new();
        Ok(cols
            .into_iter()
            .map(|(name, ty)| {
                let t = ty.to_uppercase();
                let ty = if t.contains("BOOL") {
                    Ty::Bool
                } else if t == "DATE" {
                    Ty::Date
                } else if t.contains("TIMESTAMP") {
                    Ty::Time
                } else if ["INT", "NUMERIC", "DECIMAL", "FLOAT", "DOUBLE", "REAL", "MONEY"].iter().any(|n| t.contains(n)) {
                    Ty::Number
                } else {
                    Ty::Text
                };
                Column { key: unique_key(&name, &mut keys), label: name, ty }
            })
            .collect())
    }

    pub async fn tables(&mut self) -> Result<Vec<String>, String> {
        sqlx::query_scalar!(r#"SELECT table_schema || '.' || table_name AS "v!" FROM information_schema.tables WHERE table_schema NOT IN ('pg_catalog', 'information_schema') ORDER BY 1 LIMIT 1000"#)
        .fetch_all(&mut self.0)
        .await
        .map_err(short)
    }

    pub async fn close(self) {
        let _ = self.0.close().await;
    }
}

pub fn strip(q: &str) -> &str {
    q.trim().trim_end_matches(';').trim_end()
}

/// Quotes a name.
pub fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `schema.table` (or `table`) as a query.
pub fn table_query(table: &str) -> Result<String, String> {
    let parts: Vec<&str> = table.split('.').collect();
    if parts.is_empty() || parts.len() > 2 || parts.iter().any(|p| p.is_empty() || p.len() > 128) {
        return Err("pick a table".into());
    }
    Ok(format!("SELECT * FROM {}", parts.iter().map(|p| quote(p)).collect::<Vec<_>>().join(".")))
}

/// The rows of a query on an outside database, with its columns renamed to their keys.
pub fn outside_sql(query: &str, cols: &[Column]) -> String {
    let sel: Vec<String> = cols.iter().map(|c| format!("s.{} AS {}", quote(&c.label), quote(&c.key))).collect();
    format!("SELECT {} FROM ({}) s", sel.join(", "), strip(query))
}

/// The rows of an imported dataset, typed.
pub fn rows_sql(id: Uuid, cols: &[Column]) -> String {
    let sel: Vec<String> = cols
        .iter()
        .map(|c| {
            let v = format!("(data->>'{}')", c.key);
            let e = match c.ty {
                Ty::Text => v,
                Ty::Number => format!("{v}::numeric"),
                Ty::Date => format!("{v}::date"),
                Ty::Time => format!("{v}::timestamptz"),
                Ty::Bool => format!("{v}::boolean"),
            };
            format!("{e} AS \"{}\"", c.key)
        })
        .collect();
    format!("SELECT {} FROM bi_rows WHERE dataset_id = '{id}'", sel.join(", "))
}

/// A column key from a heading: lower case letters, digits and underscores, unique.
pub fn unique_key(name: &str, taken: &mut Vec<String>) -> String {
    let mut k: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    k.truncate(50);
    if k.is_empty() || k.as_bytes()[0].is_ascii_digit() {
        k = format!("c_{k}");
    }
    let base = k.clone();
    let mut n = 2;
    while taken.contains(&k) {
        k = format!("{base}_{n}");
        n += 1;
    }
    taken.push(k.clone());
    k
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// A table read from a file: headings and text cells.
pub struct Table {
    pub headings: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

pub fn parse_csv(bytes: &[u8]) -> Result<Table, String> {
    let text = String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)).into_owned();
    let first = text.lines().next().unwrap_or("");
    let delim = [b',', b';', b'\t', b'|'].into_iter().max_by_key(|d| first.bytes().filter(|b| b == d).count()).unwrap_or(b',');
    let mut rdr = csv::ReaderBuilder::new().delimiter(delim).flexible(true).from_reader(text.as_bytes());
    let headings: Vec<String> = rdr.headers().map_err(|e| format!("cannot read the file: {e}"))?.iter().map(|h| h.trim().to_string()).collect();
    if headings.is_empty() || headings.iter().all(String::is_empty) {
        return Err("the first line should name the columns".into());
    }
    if headings.len() > MAX_COLUMNS {
        return Err(format!("at most {MAX_COLUMNS} columns"));
    }
    let mut rows = Vec::new();
    for r in rdr.records() {
        let r = r.map_err(|e| format!("cannot read the file: {e}"))?;
        if r.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        if rows.len() >= MAX_ROWS {
            return Err(format!("at most {MAX_ROWS} rows"));
        }
        rows.push((0..headings.len()).map(|i| r.get(i).unwrap_or("").trim().to_string()).collect());
    }
    Ok(Table { headings, rows })
}

/// A JSON array of objects (or an object holding one).
pub fn parse_json(bytes: &[u8]) -> Result<Table, String> {
    let v: Value = serde_json::from_slice(bytes).map_err(|e| format!("not JSON: {e}"))?;
    let items = match &v {
        Value::Array(a) => a,
        Value::Object(o) => o.values().find_map(Value::as_array).ok_or("the JSON holds no list")?,
        _ => return Err("the JSON holds no list".into()),
    };
    if items.len() > MAX_ROWS {
        return Err(format!("at most {MAX_ROWS} rows"));
    }
    let mut headings: Vec<String> = Vec::new();
    for it in items {
        for k in it.as_object().ok_or("the list should hold objects")?.keys() {
            if !headings.contains(k) {
                if headings.len() >= MAX_COLUMNS {
                    return Err(format!("at most {MAX_COLUMNS} columns"));
                }
                headings.push(k.clone());
            }
        }
    }
    let rows = items
        .iter()
        .map(|it| {
            headings
                .iter()
                .map(|h| match it.get(h) {
                    None | Some(Value::Null) => String::new(),
                    Some(Value::String(s)) => s.trim().to_string(),
                    Some(other) => other.to_string(),
                })
                .collect()
        })
        .collect();
    Ok(Table { headings, rows })
}

fn number(s: &str) -> Option<f64> {
    let mut t: String = s.chars().filter(|c| !matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '\'')).collect();
    if t.contains(',') && t.contains('.') {
        // The first one separates thousands.
        let sep = if t.find(',') < t.find('.') { ',' } else { '.' };
        t = t.replace(sep, "");
        t = t.replace(',', ".");
    } else if t.matches(',').count() == 1 {
        t = t.replace(',', ".");
    }
    t.parse::<f64>().ok().filter(|n| n.is_finite())
}

fn date(s: &str) -> Option<NaiveDate> {
    ["%Y-%m-%d", "%d.%m.%Y", "%Y/%m/%d"].iter().find_map(|f| NaiveDate::parse_from_str(s, f).ok())
}

fn time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .ok()
        .or_else(|| ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M", "%d.%m.%Y %H:%M"].iter().find_map(|f| NaiveDateTime::parse_from_str(s, f).ok()).map(|t| t.and_utc()))
}

fn boolean(s: &str) -> Option<bool> {
    match s.to_lowercase().as_str() {
        "true" | "yes" => Some(true),
        "false" | "no" => Some(false),
        _ => None,
    }
}

/// Column types read from the values, and each row as a JSON object of normalised values.
pub fn typed(t: Table) -> (Vec<Column>, Vec<Value>) {
    let mut keys = Vec::new();
    let mut cols: Vec<Column> = t
        .headings
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let label = if h.is_empty() { format!("Column {}", i + 1) } else { h.chars().take(100).collect() };
            Column { key: unique_key(&label, &mut keys), label, ty: Ty::Text }
        })
        .collect();
    for (i, c) in cols.iter_mut().enumerate() {
        let vals: Vec<&str> = t.rows.iter().map(|r| r[i].as_str()).filter(|v| !v.is_empty()).collect();
        if vals.is_empty() {
            continue;
        }
        c.ty = if vals.iter().all(|v| boolean(v).is_some()) {
            Ty::Bool
        } else if vals.iter().all(|v| number(v).is_some()) {
            Ty::Number
        } else if vals.iter().all(|v| date(v).is_some()) {
            Ty::Date
        } else if vals.iter().all(|v| time(v).is_some() || date(v).is_some()) {
            Ty::Time
        } else {
            Ty::Text
        };
    }
    let rows = t
        .rows
        .iter()
        .map(|r| {
            let mut m = Map::new();
            for (c, v) in cols.iter().zip(r) {
                if v.is_empty() {
                    continue;
                }
                let x = match c.ty {
                    Ty::Text => Some(json!(v.chars().take(2000).collect::<String>())),
                    Ty::Number => number(v).map(|n| json!(n)),
                    Ty::Bool => boolean(v).map(|b| json!(b)),
                    Ty::Date => date(v).map(|d| json!(d.to_string())),
                    Ty::Time => time(v).or_else(|| date(v).map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc())).map(|t| json!(t.to_rfc3339())),
                };
                if let Some(x) = x {
                    m.insert(c.key.clone(), x);
                }
            }
            Value::Object(m)
        })
        .collect();
    (cols, rows)
}

/// Replaces an imported dataset's rows and columns.
pub async fn store(state: &AppState, id: Uuid, cols: &[Column], rows: Vec<Value>) -> anyhow::Result<()> {
    let mut tx = state.db.begin().await?;
    sqlx::query!("DELETE FROM bi_rows WHERE dataset_id = $1", id).execute(&mut *tx).await?;
    for (i, chunk) in rows.chunks(2000).enumerate() {
        sqlx::query!("INSERT INTO bi_rows (dataset_id, n, data) SELECT $1, ($3 + o)::int, e FROM jsonb_array_elements($2::jsonb) WITH ORDINALITY AS x(e, o)", id, Value::Array(chunk.to_vec()), (i * 2000) as i64)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query!("UPDATE bi_datasets SET columns = $2, row_count = $3, refreshed_at = now(), last_error = NULL WHERE id = $1", id, serde_json::to_value(cols)?, rows.len() as i32)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Fetches a URL's CSV or JSON.
pub async fn fetch(state: &AppState, url: &str) -> Result<Table, String> {
    let u = crate::outbound::check_url(url)?;
    let (_, body) = crate::outbound::fetch(state, &u, &[], 50_000_000, 60).await?;
    // Servers often name the type wrongly; the first character tells.
    let looks_json = body.iter().find(|b| !b.is_ascii_whitespace()).is_some_and(|b| *b == b'[' || *b == b'{');
    if looks_json { parse_json(&body) } else { parse_csv(&body) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_types_a_csv() {
        let t = parse_csv("\u{feff}Region;Sales 2026;Day;When;Paid;Note\nNorth;1.234,50;2026-01-05;2026-01-05 10:00;yes;a\nSouth;99;05.01.2026;2026-01-06;no;\n;;;;;\n".as_bytes()).unwrap();
        assert_eq!(t.rows.len(), 2);
        let (cols, rows) = typed(t);
        let k: Vec<(&str, Ty)> = cols.iter().map(|c| (c.key.as_str(), c.ty)).collect();
        assert_eq!(k, vec![("region", Ty::Text), ("sales_2026", Ty::Number), ("day", Ty::Date), ("when", Ty::Time), ("paid", Ty::Bool), ("note", Ty::Text)]);
        assert_eq!(rows[0]["sales_2026"], json!(1234.5));
        assert_eq!(rows[1]["day"], json!("2026-01-05"));
        assert_eq!(rows[1]["when"], json!("2026-01-06T00:00:00+00:00"));
        assert!(rows[1].get("note").is_none());
    }

    #[test]
    fn reads_json_and_makes_keys() {
        let t = parse_json(br#"{"data": [{"Name": "a", "n": 1}, {"n": 2.5, "x y": true}]}"#).unwrap();
        assert_eq!(t.headings, vec!["Name", "n", "x y"]);
        let (cols, _) = typed(t);
        assert_eq!(cols[1].ty, Ty::Number);
        let mut taken = vec![];
        assert_eq!(unique_key("Größe (€)", &mut taken), "gr_e");
        assert_eq!(unique_key("gr e", &mut taken), "gr_e_2");
        assert_eq!(unique_key("2026", &mut taken), "c_2026");
        assert_eq!(quote("a\"b"), "\"a\"\"b\"");
        assert_eq!(table_query("shop.orders").unwrap(), "SELECT * FROM \"shop\".\"orders\"");
    }
}
