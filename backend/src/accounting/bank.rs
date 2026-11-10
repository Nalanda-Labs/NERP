//! Bank statements: imported (CSV or OFX) into a bank or cash account, then reconciled line by
//! line: matched to an entry already in the books, recorded as a new entry (income, expense,
//! transfer, with tax), recorded as a customer or supplier payment, or ignored.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::str::FromStr;
use uuid::Uuid;

use super::{fx_rate, ledger, need, ready, Level};
use crate::{
    audit,
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/accounting/bank/{account}", get(summary))
        .route("/accounting/bank/{account}/import", post(import))
        .route("/accounting/bank/{account}/lines", get(lines))
        .route("/accounting/bank/lines/{id}/match", post(match_line))
        .route("/accounting/bank/lines/{id}/record", post(record_line))
        .route("/accounting/bank/lines/{id}/payment", post(payment_line))
        .route("/accounting/bank/lines/{id}/ignore", post(ignore_line))
        .route("/accounting/bank/lines/{id}/unmatch", post(unmatch_line))
}

/// The bank or cash account and its currency.
async fn bank_account(state: &AppState, id: Uuid) -> AppResult<(String, String)> {
    let row: Option<(String, String, Option<String>)> = sqlx::query_as("SELECT name, subtype, currency FROM acc_accounts WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
    match row {
        Some((name, s, c)) if s == "bank" || s == "cash" => Ok((name, c.unwrap_or(super::base_currency(state).await?))),
        Some(_) => Err(AppError::bad("statements are imported into bank or cash accounts")),
        None => Err(AppError::NotFound("account")),
    }
}

// ---------------------------------------------------------------------------
// Reading statements
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct StatementLine {
    pub date: NaiveDate,
    pub description: String,
    pub reference: Option<String>,
    pub amount: Decimal,
}

/// An amount as banks write it: 1,234.56 / 1.234,56 / (12.00) / 12.00 CR / -€12.
pub fn parse_amount(raw: &str) -> Option<Decimal> {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return None;
    }
    let mut negative = false;
    let up = s.to_uppercase();
    if up.ends_with("DR") || up.ends_with(" D") {
        negative = true;
        s.truncate(s.len() - 2);
    } else if up.ends_with("CR") {
        s.truncate(s.len() - 2);
    }
    let s = s.trim();
    let (s, paren) = if s.starts_with('(') && s.ends_with(')') { (&s[1..s.len() - 1], true) } else { (s, false) };
    let mut digits: String = s.chars().filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+')).collect();
    if digits.starts_with('-') {
        negative = !negative;
        digits.remove(0);
    } else if digits.starts_with('+') {
        digits.remove(0);
    }
    if digits.contains('-') || digits.is_empty() {
        return None;
    }
    // Which mark is the decimal point: with both, the last one; a lone comma with one or two
    // digits after it; a lone dot (once). Repeated marks separate thousands.
    let after = |i: usize| digits.len() - i - 1;
    let normal = match (digits.rfind('.'), digits.rfind(',')) {
        (Some(d), Some(c)) if c > d => digits.replace('.', "").replace(',', "."),
        (Some(_), Some(_)) => digits.replace(',', ""),
        (None, Some(c)) if digits.matches(',').count() == 1 && after(c) <= 2 => digits.replace(',', "."),
        (None, Some(_)) => digits.replace(',', ""),
        (Some(_), None) if digits.matches('.').count() > 1 => digits.replace('.', ""),
        _ => digits,
    };
    let v = Decimal::from_str(&normal).ok()?;
    Some(if negative ^ paren { -v } else { v })
}

/// Day-first or month-first, for dates like 03/04/2026.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Order {
    Dmy,
    Mdy,
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

pub fn parse_date(raw: &str, order: Order) -> Option<NaiveDate> {
    let s = raw.trim();
    let s = s.split(['T', ' ']).next().filter(|p| p.len() >= 8 && p.chars().filter(|c| c.is_ascii_digit()).count() >= 6).unwrap_or(s);
    // 20260304 (OFX)
    if s.len() >= 8 && s[..8].chars().all(|c| c.is_ascii_digit()) {
        return NaiveDate::parse_from_str(&s[..8], "%Y%m%d").ok();
    }
    let parts: Vec<&str> = s.split(['-', '/', '.', ' ', ',']).filter(|p| !p.is_empty()).collect();
    if parts.len() < 3 {
        return None;
    }
    let num = |p: &str| p.parse::<u32>().ok();
    let month = |p: &str| MONTHS.iter().position(|m| p.to_lowercase().starts_with(m)).map(|i| i as u32 + 1);
    let (y, m, d) = if parts[0].len() == 4 {
        (num(parts[0])? as i32, num(parts[1]).or_else(|| month(parts[1]))?, num(parts[2])?)
    } else if let Some(m) = month(parts[1]) {
        (num(parts[2])? as i32, m, num(parts[0])?)
    } else if let Some(m) = month(parts[0]) {
        (num(parts[2])? as i32, m, num(parts[1])?)
    } else {
        let (a, b, y) = (num(parts[0])?, num(parts[1])?, num(parts[2])? as i32);
        let y = if y < 100 { 2000 + y } else { y };
        match order {
            Order::Dmy => (y, b, a),
            Order::Mdy => (y, a, b),
        }
    };
    NaiveDate::from_ymd_opt(y, m, d)
}

/// Which order the dates in a column are in: a first part over 12 means day-first, a second
/// part over 12 month-first. None: can't tell.
fn guess_order(dates: &[&str]) -> Option<Order> {
    for d in dates {
        let parts: Vec<&str> = d.trim().split(['-', '/', '.']).collect();
        if parts.len() == 3 && parts[0].len() <= 2 {
            let (a, b) = (parts[0].parse::<u32>().unwrap_or(0), parts[1].parse::<u32>().unwrap_or(0));
            if a > 12 {
                return Some(Order::Dmy);
            }
            if b > 12 {
                return Some(Order::Mdy);
            }
        }
    }
    None
}

fn ofx(text: &str) -> Vec<StatementLine> {
    let tag = |block: &str, name: &str| -> Option<String> {
        let open = format!("<{name}>");
        let i = block.to_uppercase().find(&open)? + open.len();
        let rest = &block[i..];
        let end = rest.find(['<', '\r', '\n']).unwrap_or(rest.len());
        Some(rest[..end].trim().to_string()).filter(|s| !s.is_empty())
    };
    let up = text.to_uppercase();
    let mut out = vec![];
    let mut from = 0;
    while let Some(i) = up[from..].find("<STMTTRN>") {
        let start = from + i;
        let end = up[start..].find("</STMTTRN>").map(|e| start + e).unwrap_or(up.len().min(start + 2000));
        let block = &text[start..end];
        if let (Some(d), Some(a)) = (tag(block, "DTPOSTED"), tag(block, "TRNAMT")) {
            if let (Some(date), Some(amount)) = (parse_date(&d, Order::Dmy), parse_amount(&a)) {
                let desc = [tag(block, "NAME"), tag(block, "MEMO")].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                out.push(StatementLine { date, description: desc, reference: tag(block, "CHECKNUM").or_else(|| tag(block, "FITID")), amount });
            }
        }
        from = end.max(start + 1);
    }
    out
}

/// The statement's lines, and the date order used (CSV).
pub fn read_statement(text: &str, order: Option<Order>) -> Result<(Vec<StatementLine>, Option<Order>), String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.to_uppercase().contains("<STMTTRN>") {
        return Ok((ofx(text), None));
    }
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let delim = [b',', b';', b'\t'].into_iter().max_by_key(|d| first.matches(*d as char).count()).unwrap_or(b',');
    let mut rdr = csv::ReaderBuilder::new().delimiter(delim).flexible(true).from_reader(text.as_bytes());
    let headers: Vec<String> = rdr.headers().map_err(|e| e.to_string())?.iter().map(|h| h.trim().to_lowercase()).collect();
    let find = |names: &[&str]| headers.iter().position(|h| names.iter().any(|n| h == n || h.contains(n)));
    let date = find(&["transaction date", "posting date", "value date", "booking date", "date"]).ok_or("no date column")?;
    let desc = find(&["description", "narration", "details", "particulars", "payee", "memo", "remarks", "transaction", "name"]);
    let reference = find(&["reference", "ref", "cheque", "check", "transaction id"]).filter(|i| Some(*i) != desc);
    let amount = headers.iter().position(|h| h == "amount" || (h.contains("amount") && !h.contains("balance")));
    let debit = find(&["debit", "withdrawal", "paid out", "money out", "outflow"]);
    let credit = find(&["credit", "deposit", "paid in", "money in", "inflow"]);
    if amount.is_none() && (debit.is_none() || credit.is_none()) {
        return Err("no amount column (or debit and credit columns)".into());
    }
    let records: Vec<csv::StringRecord> = rdr.records().filter_map(Result::ok).collect();
    let dates: Vec<&str> = records.iter().filter_map(|r| r.get(date)).collect();
    let order = order.or_else(|| guess_order(&dates));
    let mut out = vec![];
    for r in &records {
        let get = |i: Option<usize>| i.and_then(|i| r.get(i)).map(str::trim).unwrap_or("");
        let raw_date = get(Some(date));
        if raw_date.is_empty() {
            continue;
        }
        let d = parse_date(raw_date, order.unwrap_or(Order::Dmy)).ok_or_else(|| format!("can't read the date “{raw_date}”"))?;
        let a = match amount {
            Some(_) if debit.is_none() || !get(amount).is_empty() => parse_amount(get(amount)),
            _ => match (parse_amount(get(credit)), parse_amount(get(debit))) {
                (Some(c), _) if !c.is_zero() => Some(c.abs()),
                (_, Some(d)) => Some(-d.abs()),
                (c, None) => c,
            },
        };
        let Some(a) = a else { continue };
        if a.is_zero() {
            continue;
        }
        out.push(StatementLine { date: d, description: get(desc).to_string(), reference: Some(get(reference).to_string()).filter(|s| !s.is_empty()), amount: ledger::round(a) });
    }
    Ok((out, order))
}

#[derive(Deserialize)]
struct ImportBody {
    /// An upload (POST /files) with the statement.
    file_id: Uuid,
    /// dmy | mdy, when the dates can't tell.
    date_order: Option<String>,
}

async fn import(State(state): State<AppState>, auth: AuthUser, Path(account): Path<Uuid>, Json(b): Json<ImportBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    ready(&state).await?;
    bank_account(&state, account).await?;
    let (_, _, owner, bytes) = crate::routes::files::read_bytes(&state, b.file_id).await?;
    if owner != Some(auth.id) {
        return Err(AppError::bad("import a file you uploaded"));
    }
    let text = String::from_utf8(bytes).or_else(|e| Ok::<_, AppError>(e.into_bytes().iter().map(|&c| c as char).collect()))?;
    let order = match b.date_order.as_deref() {
        Some("dmy") => Some(Order::Dmy),
        Some("mdy") => Some(Order::Mdy),
        _ => None,
    };
    let (lines, used) = read_statement(&text, order).map_err(|e| AppError::bad(format!("can't read the statement: {e}")))?;
    if lines.is_empty() {
        return Err(AppError::bad("the statement has no transactions"));
    }
    if lines.len() > 20_000 {
        return Err(AppError::bad("at most 20,000 lines at once"));
    }
    let mut seen = std::collections::HashMap::<String, u32>::new();
    let (mut added, mut skipped) = (0, 0);
    let mut tx = state.db.begin().await?;
    for l in &lines {
        let key = format!("{}|{}|{}|{}", l.date, l.amount, l.description, l.reference.clone().unwrap_or_default());
        let n = seen.entry(key.clone()).or_default();
        *n += 1;
        let fp = hex::encode(Sha256::digest(format!("{key}|{n}").as_bytes()));
        let r = sqlx::query(
            "INSERT INTO acc_bank_lines (account_id, date, description, reference, amount, fingerprint, imported_by) VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (account_id, fingerprint) DO NOTHING",
        )
        .bind(account)
        .bind(l.date)
        .bind(l.description.chars().take(1000).collect::<String>())
        .bind(&l.reference)
        .bind(l.amount)
        .bind(fp)
        .bind(auth.id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if r == 1 { added += 1 } else { skipped += 1 }
    }
    tx.commit().await?;
    audit::record(&state.db, Some(auth.id), "accounting.bank_import", "account", Some(account.to_string()), json!({ "added": added, "skipped": skipped }), None).await;
    Ok(Json(json!({ "added": added, "skipped": skipped, "date_order": used.map(|o| if o == Order::Dmy { "dmy" } else { "mdy" }) })))
}

// ---------------------------------------------------------------------------
// Reconciling
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct LinesQuery {
    /// unmatched (default) | matched | ignored | all
    status: Option<String>,
    limit: Option<i64>,
}

/// Journal lines on the account that no statement line accounts for yet (posted, not reversed).
const UNRECONCILED: &str = "SELECT l.id, l.date, l.debit - l.credit AS amount, e.number, e.memo, p.title, e.id AS entry_id
     FROM acc_lines l JOIN acc_entries e ON e.id = l.entry_id LEFT JOIN crm_items p ON p.id = l.party_id
     WHERE l.account_id = $1 AND e.status = 'posted' AND e.reverses IS NULL AND NOT EXISTS (SELECT 1 FROM acc_bank_lines b WHERE b.line_id = l.id)";

async fn lines(State(state): State<AppState>, auth: AuthUser, Path(account): Path<Uuid>, Query(q): Query<LinesQuery>) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    bank_account(&state, account).await?;
    let status = q.status.unwrap_or_else(|| "unmatched".into());
    let rows: Vec<(Uuid, NaiveDate, String, Option<String>, Decimal, String, Option<Uuid>, Option<i64>)> = sqlx::query_as(
        "SELECT b.id, b.date, b.description, b.reference, b.amount, b.status, b.line_id, e.number
         FROM acc_bank_lines b LEFT JOIN acc_lines l ON l.id = b.line_id LEFT JOIN acc_entries e ON e.id = l.entry_id
         WHERE b.account_id = $1 AND ($2 = 'all' OR b.status = $2) ORDER BY b.date, b.amount LIMIT $3",
    )
    .bind(account)
    .bind(&status)
    .bind(q.limit.unwrap_or(500).clamp(1, 2000))
    .fetch_all(&state.db)
    .await?;
    // Suggestions for unmatched lines: same amount, within 15 days.
    let open: Vec<(Uuid, NaiveDate, Decimal, Option<i64>, Option<String>, Option<String>, Uuid)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{UNRECONCILED} ORDER BY l.date"))).bind(account).fetch_all(&state.db).await?;
    Ok(Json(json!({
        "items": rows.into_iter().map(|r| {
            let suggestions: Vec<Value> = if r.5 == "unmatched" {
                let mut s: Vec<_> = open.iter().filter(|o| o.2 == r.4 && (o.1 - r.1).num_days().abs() <= 15).collect();
                s.sort_by_key(|o| (o.1 - r.1).num_days().abs());
                s.into_iter().take(3).map(|o| json!({ "line_id": o.0, "date": o.1, "amount": o.2, "entry_number": o.3, "memo": o.4, "party_name": o.5 })).collect()
            } else { vec![] };
            json!({ "id": r.0, "date": r.1, "description": r.2, "reference": r.3, "amount": r.4, "status": r.5, "line_id": r.6, "entry_number": r.7, "suggestions": suggestions })
        }).collect::<Vec<_>>(),
    })))
}

async fn summary(State(state): State<AppState>, auth: AuthUser, Path(account): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    let (name, currency) = bank_account(&state, account).await?;
    let base = super::base_currency(&state).await?;
    // In the account's own currency for foreign-currency accounts.
    let book: Decimal = sqlx::query_scalar(
        "SELECT COALESCE(SUM(CASE WHEN l.currency IS NOT NULL THEN l.amount_currency ELSE l.debit - l.credit END), 0)
         FROM acc_lines l JOIN acc_entries e ON e.id = l.entry_id WHERE l.account_id = $1 AND e.status <> 'draft'",
    )
    .bind(account)
    .fetch_one(&state.db)
    .await?;
    let (count, unmatched, unmatched_sum, last): (i64, i64, Decimal, Option<NaiveDate>) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE status = 'unmatched'), COALESCE(SUM(amount) FILTER (WHERE status = 'unmatched'), 0), MAX(date)
         FROM acc_bank_lines WHERE account_id = $1",
    )
    .bind(account)
    .fetch_one(&state.db)
    .await?;
    let open: Vec<(Uuid, NaiveDate, Decimal, Option<i64>, Option<String>, Option<String>, Uuid)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{UNRECONCILED} ORDER BY l.date LIMIT 200"))).bind(account).fetch_all(&state.db).await?;
    Ok(Json(json!({
        "account_id": account, "name": name, "currency": currency, "foreign": currency != base, "book_balance": book,
        "statement_lines": count, "unmatched": unmatched, "unmatched_amount": unmatched_sum, "last_statement_date": last,
        "unreconciled": open.into_iter().map(|o| json!({ "line_id": o.0, "date": o.1, "amount": o.2, "entry_number": o.3, "memo": o.4, "party_name": o.5, "entry_id": o.6 })).collect::<Vec<_>>(),
    })))
}

/// A statement line, locked, that is still to be matched: (account, date, amount, description).
async fn open_line(tx: &mut ledger::Tx<'_>, id: Uuid) -> AppResult<(Uuid, NaiveDate, Decimal, String, Option<String>)> {
    let row: Option<(Uuid, NaiveDate, Decimal, String, Option<String>, String)> =
        sqlx::query_as("SELECT account_id, date, amount, description, reference, status FROM acc_bank_lines WHERE id = $1 FOR UPDATE").bind(id).fetch_optional(&mut **tx).await?;
    let (a, d, amt, desc, r, st) = row.ok_or(AppError::NotFound("statement line"))?;
    if st != "unmatched" {
        return Err(AppError::bad(format!("this statement line is already {st}")));
    }
    Ok((a, d, amt, desc, r))
}

#[derive(Deserialize)]
struct MatchBody {
    line_id: Uuid,
}

async fn match_line(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<MatchBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let mut tx = state.db.begin().await?;
    let (account, _, amount, _, _) = open_line(&mut tx, id).await?;
    let gl: Option<(Uuid, Decimal, Option<Decimal>, String, Option<Uuid>, bool)> = sqlx::query_as(
        "SELECT l.account_id, l.debit - l.credit, l.amount_currency, e.status, e.reverses, EXISTS (SELECT 1 FROM acc_bank_lines b WHERE b.line_id = l.id)
         FROM acc_lines l JOIN acc_entries e ON e.id = l.entry_id WHERE l.id = $1",
    )
    .bind(b.line_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (gl_account, gl_amount, gl_foreign, status, reverses, taken) = gl.ok_or(AppError::NotFound("journal line"))?;
    if gl_account != account {
        return Err(AppError::bad("that journal line is on another account"));
    }
    if status != "posted" || reverses.is_some() {
        return Err(AppError::bad("match a posted, unreversed entry"));
    }
    if taken {
        return Err(AppError::bad("that journal line is already matched to another statement line"));
    }
    if gl_foreign.unwrap_or(gl_amount) != amount {
        return Err(AppError::bad(format!("the amounts differ ({} in the books, {amount} on the statement)", gl_foreign.unwrap_or(gl_amount))));
    }
    sqlx::query("UPDATE acc_bank_lines SET status = 'matched', line_id = $2 WHERE id = $1").bind(id).bind(b.line_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct RecordBody {
    /// Where the money came from or went: income, expense, another bank account (a transfer)…
    account_id: Uuid,
    tax_id: Option<Uuid>,
    party_id: Option<Uuid>,
    description: Option<String>,
}

/// A new entry for the statement line (bank charges, interest, a card purchase, a transfer…).
/// With a tax, the line's amount includes it.
async fn record_line(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<RecordBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    ready(&state).await?;
    let base = super::base_currency(&state).await?;
    let mut tx = state.db.begin().await?;
    let (bank, date, amount, desc, _) = open_line(&mut tx, id).await?;
    if b.account_id == bank {
        return Err(AppError::bad("choose the other side of the transaction, not the bank account itself"));
    }
    let sub: Option<String> = sqlx::query_scalar("SELECT subtype FROM acc_accounts WHERE id = $1 AND is_active").bind(b.account_id).fetch_optional(&mut *tx).await?;
    match sub.as_deref() {
        None => return Err(AppError::bad("that account doesn't exist or is inactive")),
        Some("receivable") | Some("payable") => return Err(AppError::bad("for customers' and suppliers' money, record it as a payment")),
        _ => {}
    }
    if let Some(p) = b.party_id {
        super::party_exists(&state, p).await?;
    }
    let (_, currency) = bank_account(&state, bank).await?;
    let fx = fx_rate(&state, &currency, None).await?;
    let inbound = amount > Decimal::ZERO;
    let gross = amount.abs();
    let (net, tax, tax_account) = match b.tax_id {
        Some(t) => {
            let tax = super::tax(&state, t).await?;
            if !tax.is_active || tax.scope != if inbound { "sales" } else { "purchases" } {
                return Err(AppError::bad(format!("use an active {} tax", if inbound { "sales" } else { "purchase" })));
            }
            let rate = tax.rate / Decimal::ONE_HUNDRED;
            let net = ledger::round(gross / (Decimal::ONE + rate));
            (net, gross - net, Some((t, tax.account_id)))
        }
        None => (gross, Decimal::ZERO, None),
    };
    let text = crate::routes::clean(b.description).unwrap_or(desc);
    let sign = if inbound { Decimal::ONE } else { -Decimal::ONE };
    let mut lines = vec![ledger::signed(bank, sign * gross * fx).foreign(&currency, &base, sign * gross).text(text.clone()).party(b.party_id)];
    lines.push(ledger::signed(b.account_id, -sign * net * fx).foreign(&currency, &base, -sign * net).text(text.clone()).party(b.party_id));
    if let Some((t, acc)) = tax_account {
        lines.push(ledger::signed(acc, -sign * tax * fx).foreign(&currency, &base, -sign * tax).taxed(t, net * fx).party(b.party_id));
    }
    let diff: Decimal = lines.iter().map(|l| l.debit - l.credit).sum();
    if !diff.is_zero() {
        let s = super::settings(&state).await?;
        lines.push(ledger::signed(super::default_account(s.rounding_account, "rounding")?, -diff).text("Rounding"));
    }
    let entry = ledger::post(&mut tx, date, Some(&text), "bank", Some(id), &lines, Some(auth.id)).await?;
    let bank_line: Uuid = sqlx::query_scalar("SELECT id FROM acc_lines WHERE entry_id = $1 AND account_id = $2 ORDER BY sort LIMIT 1").bind(entry).bind(bank).fetch_one(&mut *tx).await?;
    sqlx::query("UPDATE acc_bank_lines SET status = 'matched', line_id = $2 WHERE id = $1").bind(id).bind(bank_line).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(super::entry_json(&state, entry).await?))
}

#[derive(Deserialize)]
struct PaymentBody {
    party_id: Option<Uuid>,
    #[serde(default)]
    allocations: Vec<super::payments::AllocIn>,
}

/// The statement line is a customer's payment (money in) or a payment to a supplier (out).
async fn payment_line(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<PaymentBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let (account, date, amount, desc, reference): (Uuid, NaiveDate, Decimal, String, Option<String>) = {
        let mut tx = state.db.begin().await?;
        let r = open_line(&mut tx, id).await?;
        tx.commit().await?;
        r
    };
    let p = super::payments::PaymentIn {
        direction: if amount > Decimal::ZERO { "in".into() } else { "out".into() },
        date,
        account_id: account,
        party_id: b.party_id,
        currency: None,
        amount: amount.abs(),
        fx_rate: None,
        reference: reference.or(Some(desc).filter(|d| !d.is_empty())),
        memo: None,
        allocations: b.allocations,
    };
    let (payment, _) = super::payments::record(&state, &auth, p).await?;
    let line: Uuid = sqlx::query_scalar("SELECT l.id FROM acc_payments p JOIN acc_lines l ON l.entry_id = p.entry_id AND l.account_id = p.account_id WHERE p.id = $1 LIMIT 1")
        .bind(payment)
        .fetch_one(&state.db)
        .await?;
    let n = sqlx::query("UPDATE acc_bank_lines SET status = 'matched', line_id = $2 WHERE id = $1 AND status = 'unmatched'").bind(id).bind(line).execute(&state.db).await?.rows_affected();
    if n == 0 {
        tracing::warn!(line = %id, "a statement line was matched by someone else while its payment was recorded");
    }
    Ok(Json(super::payments::payment_json(&state, payment).await?))
}

async fn ignore_line(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let n = sqlx::query("UPDATE acc_bank_lines SET status = 'ignored' WHERE id = $1 AND status = 'unmatched'").bind(id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::bad("only unmatched statement lines can be ignored"));
    }
    Ok(Json(json!({ "ok": true })))
}

/// Undoes a match (or an ignore); an entry recorded from the line stays (reverse it if wrong).
async fn unmatch_line(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    sqlx::query("UPDATE acc_bank_lines SET status = 'unmatched', line_id = NULL WHERE id = $1").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn amounts_as_banks_write_them() {
        assert_eq!(parse_amount("1,234.56"), Some(d("1234.56")));
        assert_eq!(parse_amount("1.234,56"), Some(d("1234.56")));
        assert_eq!(parse_amount("-12.50"), Some(d("-12.50")));
        assert_eq!(parse_amount("(12.50)"), Some(d("-12.50")));
        assert_eq!(parse_amount("12.50 DR"), Some(d("-12.50")));
        assert_eq!(parse_amount("12.50CR"), Some(d("12.50")));
        assert_eq!(parse_amount("€ 1 234,50"), Some(d("1234.50")));
        assert_eq!(parse_amount("12,5"), Some(d("12.5")));
        assert_eq!(parse_amount("1,000"), Some(d("1000")));
        assert_eq!(parse_amount(""), None);
        assert_eq!(parse_amount("abc"), None);
    }

    #[test]
    fn dates_in_many_shapes() {
        let x = NaiveDate::from_ymd_opt(2026, 3, 4).unwrap();
        assert_eq!(parse_date("2026-03-04", Order::Dmy), Some(x));
        assert_eq!(parse_date("04/03/2026", Order::Dmy), Some(x));
        assert_eq!(parse_date("03/04/2026", Order::Mdy), Some(x));
        assert_eq!(parse_date("04.03.2026", Order::Dmy), Some(x));
        assert_eq!(parse_date("04 Mar 2026", Order::Mdy), Some(x));
        assert_eq!(parse_date("Mar 4, 2026", Order::Dmy), Some(x));
        assert_eq!(parse_date("20260304120000[0:GMT]", Order::Dmy), Some(x));
        assert_eq!(parse_date("04/03/26", Order::Dmy), Some(x));
        assert_eq!(parse_date("31/02/2026", Order::Dmy), None);
    }

    #[test]
    fn csv_with_debit_and_credit_columns() {
        let csv = "Date;Narration;Ref;Withdrawal;Deposit;Balance\n25/03/2026;Office rent;R1;1.200,00;;5.000,00\n26/03/2026;Customer ACME;;;300,50;5.300,50\n";
        let (lines, order) = read_statement(csv, None).unwrap();
        assert_eq!(order, Some(Order::Dmy));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].amount, d("-1200.00"));
        assert_eq!(lines[0].reference.as_deref(), Some("R1"));
        assert_eq!(lines[1].amount, d("300.50"));
        assert_eq!(lines[1].description, "Customer ACME");
    }

    #[test]
    fn ofx_statements() {
        let ofx = "OFXHEADER:100\n<OFX><BANKMSGSRSV1><STMTTRNRS><STMTRS><BANKTRANLIST>\n<STMTTRN>\n<TRNTYPE>DEBIT\n<DTPOSTED>20260304\n<TRNAMT>-42.10\n<FITID>abc1\n<NAME>COFFEE SHOP\n</STMTTRN>\n<STMTTRN><TRNTYPE>CREDIT<DTPOSTED>20260305120000<TRNAMT>100.00<FITID>abc2<NAME>REFUND<MEMO>March</STMTTRN>\n</BANKTRANLIST></STMTRS></STMTTRNRS></BANKMSGSRSV1></OFX>";
        let (lines, _) = read_statement(ofx, None).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].amount, d("-42.10"));
        assert_eq!(lines[0].description, "COFFEE SHOP");
        assert_eq!(lines[1].description, "REFUND · March");
        assert_eq!(lines[1].date, NaiveDate::from_ymd_opt(2026, 3, 5).unwrap());
    }
}
