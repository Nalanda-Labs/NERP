//! Reports from the journal (posted and voided entries with their reversals; drafts never),
//! and year-end closing. Every report can be downloaded as CSV (`format=csv`).

use axum::{
    extract::{Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{Datelike, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{default_account, ledger, need, ready, Level};
use crate::{
    audit,
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/accounting/reports/summary", get(summary))
        .route("/accounting/reports/trial-balance", get(trial_balance))
        .route("/accounting/reports/profit-loss", get(profit_loss))
        .route("/accounting/reports/balance-sheet", get(balance_sheet))
        .route("/accounting/reports/ledger", get(ledger_report))
        .route("/accounting/reports/aging", get(aging))
        .route("/accounting/reports/tax", get(tax_report))
        .route("/accounting/reports/statement", get(statement))
        .route("/accounting/close", post(close_year))
}

#[derive(Deserialize, Default)]
struct Period {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    as_of: Option<NaiveDate>,
    account_id: Option<Uuid>,
    party_id: Option<Uuid>,
    kind: Option<String>,
    format: Option<String>,
}

fn today() -> NaiveDate {
    Utc::now().date_naive()
}

/// The start of the financial year `date` falls in.
pub fn year_start(date: NaiveDate, start_month: u32) -> NaiveDate {
    let y = if date.month() >= start_month { date.year() } else { date.year() - 1 };
    NaiveDate::from_ymd_opt(y, start_month, 1).unwrap()
}

/// A CSV download, or the JSON.
fn respond(q: &Period, name: &str, body: Value, rows: Vec<Vec<String>>) -> Response {
    if q.format.as_deref() != Some("csv") {
        return Json(body).into_response();
    }
    let mut w = csv::Writer::from_writer(vec![]);
    for r in rows {
        let _ = w.write_record(r);
    }
    let bytes = w.into_inner().unwrap_or_default();
    (
        [(header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()), (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}.csv\""))],
        bytes,
    )
        .into_response()
}

fn s(d: Decimal) -> String {
    d.round_dp(2).to_string()
}

/// Balances per account (debits minus credits) between two dates (inclusive; None: unbounded).
/// `no_closing` leaves year-end closing entries out (for profit and loss).
async fn balances(state: &AppState, from: Option<NaiveDate>, to: NaiveDate, no_closing: bool) -> AppResult<Vec<(Uuid, String, String, String, String, Decimal, Decimal)>> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.code, a.name, a.type, a.subtype, COALESCE(SUM(l.debit), 0), COALESCE(SUM(l.credit), 0)
         FROM acc_accounts a JOIN acc_lines l ON l.account_id = a.id JOIN acc_entries e ON e.id = l.entry_id
         WHERE e.status <> 'draft' AND l.date <= $2 AND ($1::date IS NULL OR l.date >= $1) AND (NOT $3 OR e.source <> 'closing')
         GROUP BY a.id, a.code, a.name, a.type, a.subtype ORDER BY a.code",
    )
    .bind(from)
    .bind(to)
    .bind(no_closing)
    .fetch_all(&state.db)
    .await?)
}

// ---------------------------------------------------------------------------

async fn trial_balance(State(state): State<AppState>, auth: AuthUser, Query(q): Query<Period>) -> AppResult<Response> {
    need(&auth, Level::View)?;
    let to = q.to.or(q.as_of).unwrap_or_else(today);
    let closing = balances(&state, None, to, false).await?;
    let opening = match q.from {
        Some(f) => balances(&state, None, f - chrono::Duration::days(1), false).await?,
        None => vec![],
    };
    let mut rows = vec![];
    let (mut td, mut tc) = (Decimal::ZERO, Decimal::ZERO);
    for (id, code, name, kind, _, d, c) in &closing {
        let open = opening.iter().find(|o| o.0 == *id).map(|o| o.5 - o.6).unwrap_or_default();
        let bal = d - c;
        let (pd, pc) = (d - opening.iter().find(|o| o.0 == *id).map(|o| o.5).unwrap_or_default(), c - opening.iter().find(|o| o.0 == *id).map(|o| o.6).unwrap_or_default());
        if bal.is_zero() && pd.is_zero() && pc.is_zero() {
            continue;
        }
        let (debit, credit) = if bal >= Decimal::ZERO { (bal, Decimal::ZERO) } else { (Decimal::ZERO, -bal) };
        td += debit;
        tc += credit;
        rows.push(json!({ "account_id": id, "code": code, "name": name, "type": kind, "opening": open, "period_debit": pd, "period_credit": pc, "debit": debit, "credit": credit }));
    }
    let body = json!({ "from": q.from, "to": to, "items": rows, "total_debit": td, "total_credit": tc, "balanced": td == tc });
    let mut csv = vec![vec!["Code".into(), "Account".into(), "Opening".into(), "Period debit".into(), "Period credit".into(), "Debit".into(), "Credit".into()]];
    for r in body["items"].as_array().unwrap() {
        csv.push(vec![r["code"].as_str().unwrap_or("").into(), r["name"].as_str().unwrap_or("").into(), r["opening"].to_string().trim_matches('"').into(),
            r["period_debit"].to_string().trim_matches('"').into(), r["period_credit"].to_string().trim_matches('"').into(),
            r["debit"].to_string().trim_matches('"').into(), r["credit"].to_string().trim_matches('"').into()]);
    }
    csv.push(vec!["".into(), "Total".into(), "".into(), "".into(), "".into(), s(td), s(tc)]);
    Ok(respond(&q, &format!("trial-balance-{to}"), body, csv))
}

const PL_SECTIONS: &[(&str, &str, i32)] = &[
    ("income", "Income", 1),
    ("cost_of_sales", "Cost of sales", -1),
    ("expense", "Expenses", -1),
    ("other_income", "Other income", 1),
    ("other_expense", "Other expenses", -1),
];

/// Profit and loss between two dates (closing entries left out).
pub async fn pl(state: &AppState, from: NaiveDate, to: NaiveDate) -> AppResult<Value> {
    let rows = balances(state, Some(from), to, true).await?;
    let mut sections = vec![];
    let mut totals = std::collections::HashMap::new();
    for (sub, title, sign) in PL_SECTIONS {
        let items: Vec<Value> = rows
            .iter()
            .filter(|r| r.4 == *sub)
            .map(|r| (r, (r.6 - r.5) * Decimal::from(*sign)))
            .filter(|(_, amt)| !amt.is_zero())
            .map(|(r, amt)| json!({ "account_id": r.0, "code": r.1, "name": r.2, "amount": amt }))
            .collect();
        let total: Decimal = items.iter().map(|i| i["amount"].as_str().and_then(|s| s.parse::<Decimal>().ok()).unwrap_or_default()).sum();
        totals.insert(*sub, total);
        sections.push(json!({ "key": sub, "title": title, "items": items, "total": total }));
    }
    let gross = totals["income"] - totals["cost_of_sales"];
    let operating = gross - totals["expense"];
    let net = operating + totals["other_income"] - totals["other_expense"];
    Ok(json!({ "from": from, "to": to, "sections": sections, "gross_profit": gross, "operating_profit": operating, "net_profit": net }))
}

async fn profit_loss(State(state): State<AppState>, auth: AuthUser, Query(q): Query<Period>) -> AppResult<Response> {
    need(&auth, Level::View)?;
    let st = super::settings(&state).await?;
    let to = q.to.unwrap_or_else(today);
    let from = q.from.unwrap_or_else(|| year_start(to, st.fiscal_year_start as u32));
    if from > to {
        return Err(AppError::bad("the period starts after it ends"));
    }
    let body = pl(&state, from, to).await?;
    let mut csv = vec![vec!["Section".into(), "Code".into(), "Account".into(), "Amount".into()]];
    for sec in body["sections"].as_array().unwrap() {
        for i in sec["items"].as_array().unwrap() {
            csv.push(vec![sec["title"].as_str().unwrap_or("").into(), i["code"].as_str().unwrap_or("").into(), i["name"].as_str().unwrap_or("").into(), i["amount"].as_str().unwrap_or("").into()]);
        }
        csv.push(vec![format!("Total {}", sec["title"].as_str().unwrap_or("").to_lowercase()), "".into(), "".into(), sec["total"].as_str().unwrap_or("").into()]);
    }
    csv.push(vec!["Net profit".into(), "".into(), "".into(), body["net_profit"].as_str().unwrap_or("").into()]);
    Ok(respond(&q, &format!("profit-and-loss-{from}-{to}"), body, csv))
}

async fn balance_sheet(State(state): State<AppState>, auth: AuthUser, Query(q): Query<Period>) -> AppResult<Response> {
    need(&auth, Level::View)?;
    let as_of = q.as_of.or(q.to).unwrap_or_else(today);
    let rows = balances(&state, None, as_of, false).await?;
    let section = |kind: &str, sign: i32| -> (Vec<Value>, Decimal) {
        let items: Vec<Value> = rows
            .iter()
            .filter(|r| r.3 == kind)
            .map(|r| (r, (r.5 - r.6) * Decimal::from(sign)))
            .filter(|(_, a)| !a.is_zero())
            .map(|(r, a)| json!({ "account_id": r.0, "code": r.1, "name": r.2, "subtype": r.4, "amount": a }))
            .collect();
        let total = rows.iter().filter(|r| r.3 == kind).map(|r| (r.5 - r.6) * Decimal::from(sign)).sum();
        (items, total)
    };
    let (assets, ta) = section("asset", 1);
    let (liabilities, tl) = section("liability", -1);
    let (mut equity, mut te) = section("equity", -1);
    // Profit not yet moved to retained earnings by a year-end closing.
    let unclosed: Decimal = rows.iter().filter(|r| r.3 == "income" || r.3 == "expense").map(|r| r.6 - r.5).sum();
    if !unclosed.is_zero() {
        equity.push(json!({ "account_id": null, "code": "", "name": "Profit not yet closed", "subtype": "retained_earnings", "amount": unclosed }));
        te += unclosed;
    }
    let body = json!({
        "as_of": as_of,
        "assets": { "items": assets, "total": ta },
        "liabilities": { "items": liabilities, "total": tl },
        "equity": { "items": equity, "total": te },
        "liabilities_and_equity": tl + te,
        "balanced": ta == tl + te,
    });
    let mut csv = vec![vec!["Section".into(), "Code".into(), "Account".into(), "Amount".into()]];
    for (key, title) in [("assets", "Assets"), ("liabilities", "Liabilities"), ("equity", "Equity")] {
        for i in body[key]["items"].as_array().unwrap() {
            csv.push(vec![title.into(), i["code"].as_str().unwrap_or("").into(), i["name"].as_str().unwrap_or("").into(), i["amount"].as_str().unwrap_or("").into()]);
        }
        csv.push(vec![format!("Total {}", title.to_lowercase()), "".into(), "".into(), body[key]["total"].as_str().unwrap_or("").into()]);
    }
    Ok(respond(&q, &format!("balance-sheet-{as_of}"), body, csv))
}

async fn ledger_report(State(state): State<AppState>, auth: AuthUser, Query(q): Query<Period>) -> AppResult<Response> {
    need(&auth, Level::View)?;
    let account = q.account_id.ok_or_else(|| AppError::bad("choose an account"))?;
    let (code, name): (String, String) = sqlx::query_as("SELECT code, name FROM acc_accounts WHERE id = $1").bind(account).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("account"))?;
    let to = q.to.unwrap_or_else(today);
    let opening: Decimal = match q.from {
        Some(f) => sqlx::query_scalar(
            "SELECT COALESCE(SUM(l.debit - l.credit), 0) FROM acc_lines l JOIN acc_entries e ON e.id = l.entry_id WHERE l.account_id = $1 AND e.status <> 'draft' AND l.date < $2",
        )
        .bind(account)
        .bind(f)
        .fetch_one(&state.db)
        .await?,
        None => Decimal::ZERO,
    };
    type L = (NaiveDate, Option<i64>, Uuid, Option<String>, Option<String>, Option<String>, Decimal, Decimal, String);
    let lines: Vec<L> = sqlx::query_as(
        "SELECT l.date, e.number, e.id, e.memo, l.description, p.title, l.debit, l.credit, e.source
         FROM acc_lines l JOIN acc_entries e ON e.id = l.entry_id LEFT JOIN crm_items p ON p.id = l.party_id
         WHERE l.account_id = $1 AND e.status <> 'draft' AND ($2::date IS NULL OR l.date >= $2) AND l.date <= $3
         ORDER BY l.date, e.number, l.sort LIMIT 20000",
    )
    .bind(account)
    .bind(q.from)
    .bind(to)
    .fetch_all(&state.db)
    .await?;
    let mut running = opening;
    let mut items = vec![];
    let mut csv = vec![vec!["Date".into(), "Entry".into(), "Memo".into(), "Party".into(), "Debit".into(), "Credit".into(), "Balance".into()]];
    csv.push(vec!["".into(), "".into(), "Opening balance".into(), "".into(), "".into(), "".into(), s(opening)]);
    for (d, n, eid, memo, desc, party, debit, credit, source) in lines {
        running += debit - credit;
        csv.push(vec![d.to_string(), n.map(|n| n.to_string()).unwrap_or_default(), desc.clone().or(memo.clone()).unwrap_or_default(), party.clone().unwrap_or_default(), s(debit), s(credit), s(running)]);
        items.push(json!({ "date": d, "entry_number": n, "entry_id": eid, "memo": memo, "description": desc, "party_name": party, "debit": debit, "credit": credit, "balance": running, "source": source }));
    }
    let body = json!({ "account_id": account, "code": code, "name": name, "from": q.from, "to": to, "opening": opening, "items": items, "closing": running });
    Ok(respond(&q, &format!("ledger-{code}-{to}"), body, csv))
}

async fn aging(State(state): State<AppState>, auth: AuthUser, Query(q): Query<Period>) -> AppResult<Response> {
    need(&auth, Level::View)?;
    let as_of = q.as_of.unwrap_or_else(today);
    let receivable = q.kind.as_deref() != Some("payable");
    // Documents posted by then, and what they still owed then.
    type D = (Uuid, Option<Uuid>, Option<String>, String, NaiveDate, Option<NaiveDate>, String, Decimal, Decimal, Decimal);
    let docs: Vec<D> = sqlx::query_as(if receivable {
        "SELECT a.item_id, a.party_id, p.title, 'Invoice ' || i.number, a.date, a.due_date, a.currency, a.fx_rate,
                a.total - COALESCE((SELECT SUM(c.amount) FROM acc_credit_notes c WHERE c.invoice_id = a.item_id AND c.status = 'posted' AND c.date <= $1), 0),
                COALESCE((SELECT SUM(al.amount) FROM acc_allocations al JOIN acc_payments x ON x.id = al.payment_id
                          WHERE al.doc_kind = 'invoice' AND al.doc_id = a.item_id AND x.status = 'posted' AND x.date <= $1), 0)
         FROM acc_invoices a JOIN crm_items i ON i.id = a.item_id LEFT JOIN crm_items p ON p.id = a.party_id
         WHERE a.status <> 'void' AND a.date <= $1"
    } else {
        "SELECT b.id, b.party_id, p.title, 'Bill ' || b.number || COALESCE(' · ' || b.reference, ''), b.date, b.due_date, b.currency, b.fx_rate, b.total,
                COALESCE((SELECT SUM(al.amount) FROM acc_allocations al JOIN acc_payments x ON x.id = al.payment_id
                          WHERE al.doc_kind = 'bill' AND al.doc_id = b.id AND x.status = 'posted' AND x.date <= $1), 0)
         FROM acc_bills b LEFT JOIN crm_items p ON p.id = b.party_id
         WHERE b.status IN ('open', 'paid') AND b.date <= $1"
    })
    .bind(as_of)
    .fetch_all(&state.db)
    .await?;
    // Money received (paid) by then and not allocated by then: a credit on the account, so the
    // ageing adds up to the receivables (payables) account.
    let credits: Vec<(Uuid, Option<Uuid>, Option<String>, i64, NaiveDate, String, Decimal, Decimal, Decimal)> = sqlx::query_as(
        "SELECT p.id, p.party_id, x.title, p.number, p.date, p.currency, p.fx_rate, p.amount,
                COALESCE((SELECT SUM(al.amount) FROM acc_allocations al WHERE al.payment_id = p.id), 0)
         FROM acc_payments p LEFT JOIN crm_items x ON x.id = p.party_id
         WHERE p.status = 'posted' AND p.direction = $1 AND p.date <= $2",
    )
    .bind(if receivable { "in" } else { "out" })
    .bind(as_of)
    .fetch_all(&state.db)
    .await?;
    const BUCKETS: [&str; 5] = ["current", "1-30", "31-60", "61-90", "over 90"];
    let mut parties: std::collections::BTreeMap<String, (Option<Uuid>, [Decimal; 5], Vec<Value>)> = Default::default();
    let mut totals = [Decimal::ZERO; 5];
    for (id, party, pname, label, date, due, cur, fx, total, paid) in docs {
        let left = total - paid;
        if left <= Decimal::ZERO {
            continue;
        }
        let late = (as_of - due.unwrap_or(date)).num_days();
        let b = match late {
            i64::MIN..=0 => 0,
            1..=30 => 1,
            31..=60 => 2,
            61..=90 => 3,
            _ => 4,
        };
        let base_left = ledger::round(left * fx);
        let e = parties.entry(pname.clone().unwrap_or_else(|| "(no one)".into())).or_insert((party, [Decimal::ZERO; 5], vec![]));
        e.1[b] += base_left;
        totals[b] += base_left;
        e.2.push(json!({ "id": id, "label": label, "date": date, "due_date": due, "currency": cur, "balance": left, "balance_base": base_left, "days_overdue": late.max(0), "bucket": BUCKETS[b] }));
    }
    for (id, party, pname, number, date, cur, fx, amount, allocated) in credits {
        let left = amount - allocated;
        if left <= Decimal::ZERO {
            continue;
        }
        let base_left = -ledger::round(left * fx);
        let e = parties.entry(pname.clone().unwrap_or_else(|| "(no one)".into())).or_insert((party, [Decimal::ZERO; 5], vec![]));
        e.1[0] += base_left;
        totals[0] += base_left;
        e.2.push(json!({ "id": id, "label": format!("Payment {number} (not allocated)"), "date": date, "due_date": null, "currency": cur, "balance": -left, "balance_base": base_left, "days_overdue": 0, "bucket": BUCKETS[0] }));
    }
    let items: Vec<Value> = parties
        .into_iter()
        .map(|(name, (id, b, docs))| json!({ "party_id": id, "party_name": name, "buckets": b, "total": b.iter().sum::<Decimal>(), "documents": docs }))
        .collect();
    let mut csv = vec![vec!["Party".into(), "Current".into(), "1-30".into(), "31-60".into(), "61-90".into(), "Over 90".into(), "Total".into()]];
    for i in &items {
        let b: Vec<String> = i["buckets"].as_array().unwrap().iter().map(|x| x.as_str().unwrap_or("").to_string()).collect();
        csv.push([vec![i["party_name"].as_str().unwrap_or("").to_string()], b, vec![i["total"].as_str().unwrap_or("").to_string()]].concat());
    }
    csv.push([vec!["Total".to_string()], totals.iter().map(|t| s(*t)).collect(), vec![s(totals.iter().sum())]].concat());
    let body = json!({ "kind": if receivable { "receivable" } else { "payable" }, "as_of": as_of, "buckets": BUCKETS, "items": items, "totals": totals, "total": totals.iter().sum::<Decimal>() });
    Ok(respond(&q, &format!("{}-ageing-{as_of}", if receivable { "receivables" } else { "payables" }), body, csv))
}

async fn tax_report(State(state): State<AppState>, auth: AuthUser, Query(q): Query<Period>) -> AppResult<Response> {
    need(&auth, Level::View)?;
    let to = q.to.unwrap_or_else(today);
    let from = q.from.unwrap_or_else(|| NaiveDate::from_ymd_opt(to.year(), to.month(), 1).unwrap());
    let rows: Vec<(Uuid, String, Decimal, String, Decimal, Decimal)> = sqlx::query_as(
        "SELECT t.id, t.name, t.rate, t.scope, COALESCE(SUM(l.tax_base), 0),
                COALESCE(SUM(CASE WHEN t.scope = 'sales' THEN l.credit - l.debit ELSE l.debit - l.credit END), 0)
         FROM acc_taxes t JOIN acc_lines l ON l.tax_id = t.id JOIN acc_entries e ON e.id = l.entry_id
         WHERE e.status <> 'draft' AND l.date BETWEEN $1 AND $2 GROUP BY t.id, t.name, t.rate, t.scope ORDER BY t.scope DESC, t.rate",
    )
    .bind(from)
    .bind(to)
    .fetch_all(&state.db)
    .await?;
    let sales: Decimal = rows.iter().filter(|r| r.3 == "sales").map(|r| r.5).sum();
    let purchases: Decimal = rows.iter().filter(|r| r.3 == "purchases").map(|r| r.5).sum();
    let mut csv = vec![vec!["Tax".into(), "Rate".into(), "On".into(), "Taxable amount".into(), "Tax".into()]];
    for r in &rows {
        csv.push(vec![r.1.clone(), r.2.normalize().to_string(), r.3.clone(), s(r.4), s(r.5)]);
    }
    csv.push(vec!["Owed on sales".into(), "".into(), "".into(), "".into(), s(sales)]);
    csv.push(vec!["Reclaimable on purchases".into(), "".into(), "".into(), "".into(), s(purchases)]);
    csv.push(vec!["Net to pay".into(), "".into(), "".into(), "".into(), s(sales - purchases)]);
    let body = json!({
        "from": from, "to": to,
        "items": rows.iter().map(|r| json!({ "tax_id": r.0, "name": r.1, "rate": r.2, "scope": r.3, "base": r.4, "tax": r.5 })).collect::<Vec<_>>(),
        "sales_tax": sales, "purchase_tax": purchases, "net": sales - purchases,
    });
    Ok(respond(&q, &format!("tax-{from}-{to}"), body, csv))
}

/// A customer's (or supplier's) account: what they were invoiced and paid, with a running balance.
async fn statement(State(state): State<AppState>, auth: AuthUser, Query(q): Query<Period>) -> AppResult<Response> {
    need(&auth, Level::View)?;
    let party = q.party_id.ok_or_else(|| AppError::bad("choose a customer or supplier"))?;
    let st = ready(&state).await?;
    let receivable = q.kind.as_deref() != Some("payable");
    let control = if receivable { default_account(st.receivable_account, "receivables")? } else { default_account(st.payable_account, "payables")? };
    let name: String = sqlx::query_scalar("SELECT title FROM crm_items WHERE id = $1").bind(party).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("party"))?;
    let to = q.to.unwrap_or_else(today);
    let sign = if receivable { Decimal::ONE } else { -Decimal::ONE };
    let opening: Decimal = match q.from {
        Some(f) => sqlx::query_scalar::<_, Decimal>(
            "SELECT COALESCE(SUM(l.debit - l.credit), 0) FROM acc_lines l JOIN acc_entries e ON e.id = l.entry_id
             WHERE l.account_id = $1 AND l.party_id = $2 AND e.status <> 'draft' AND l.date < $3",
        )
        .bind(control)
        .bind(party)
        .bind(f)
        .fetch_one(&state.db)
        .await?
            * sign,
        None => Decimal::ZERO,
    };
    let lines: Vec<(NaiveDate, Option<i64>, Option<String>, Option<String>, Decimal, Decimal)> = sqlx::query_as(
        "SELECT l.date, e.number, e.memo, l.description, l.debit, l.credit FROM acc_lines l JOIN acc_entries e ON e.id = l.entry_id
         WHERE l.account_id = $1 AND l.party_id = $2 AND e.status <> 'draft' AND ($3::date IS NULL OR l.date >= $3) AND l.date <= $4 ORDER BY l.date, e.number",
    )
    .bind(control)
    .bind(party)
    .bind(q.from)
    .bind(to)
    .fetch_all(&state.db)
    .await?;
    let mut running = opening;
    let mut items = vec![];
    let mut csv = vec![vec!["Date".into(), "Entry".into(), "Details".into(), "Charges".into(), "Payments".into(), "Balance".into()]];
    for (d, n, memo, desc, debit, credit) in lines {
        let (charge, pay) = if receivable { (debit, credit) } else { (credit, debit) };
        running += charge - pay;
        let details = memo.or(desc).unwrap_or_default();
        csv.push(vec![d.to_string(), n.map(|n| n.to_string()).unwrap_or_default(), details.clone(), s(charge), s(pay), s(running)]);
        items.push(json!({ "date": d, "entry_number": n, "details": details, "charge": charge, "payment": pay, "balance": running }));
    }
    let body = json!({ "party_id": party, "party_name": name, "kind": if receivable { "receivable" } else { "payable" }, "from": q.from, "to": to, "opening": opening, "items": items, "closing": running });
    Ok(respond(&q, &format!("statement-{}-{to}", crate::routes::slugify(&name, "party")), body, csv))
}

/// The overview: money in the bank, what is owed both ways, this year so far.
async fn summary(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    let st = super::settings(&state).await?;
    if !st.ready {
        return Ok(Json(json!({ "ready": false })));
    }
    let t = today();
    let money: Vec<(Uuid, String, String, Option<String>, Decimal, Decimal)> = sqlx::query_as(
        "SELECT a.id, a.code, a.name, a.currency,
                COALESCE((SELECT SUM(l.debit - l.credit) FROM acc_lines l JOIN acc_entries e ON e.id = l.entry_id WHERE l.account_id = a.id AND e.status <> 'draft'), 0),
                COALESCE((SELECT SUM(l.amount_currency) FROM acc_lines l JOIN acc_entries e ON e.id = l.entry_id WHERE l.account_id = a.id AND e.status <> 'draft' AND l.currency IS NOT NULL), 0)
         FROM acc_accounts a WHERE a.subtype IN ('bank', 'cash') AND a.is_active ORDER BY a.code",
    )
    .fetch_all(&state.db)
    .await?;
    let db = &state.db;
    let owed = |kind: &'static str| async move {
        let q = if kind == "invoice" {
            "SELECT COALESCE(SUM((a.total - a.credited - COALESCE(x.paid, 0)) * a.fx_rate), 0), COALESCE(SUM((a.total - a.credited - COALESCE(x.paid, 0)) * a.fx_rate) FILTER (WHERE a.due_date < current_date), 0), COUNT(*) FILTER (WHERE a.due_date < current_date)
             FROM acc_invoices a LEFT JOIN (SELECT al.doc_id, SUM(al.amount) AS paid FROM acc_allocations al JOIN acc_payments p ON p.id = al.payment_id WHERE al.doc_kind = 'invoice' AND p.status = 'posted' GROUP BY al.doc_id) x ON x.doc_id = a.item_id
             WHERE a.status = 'open'"
        } else {
            "SELECT COALESCE(SUM((b.total - COALESCE(x.paid, 0)) * b.fx_rate), 0), COALESCE(SUM((b.total - COALESCE(x.paid, 0)) * b.fx_rate) FILTER (WHERE b.due_date < current_date), 0), COUNT(*) FILTER (WHERE b.due_date < current_date)
             FROM acc_bills b LEFT JOIN (SELECT al.doc_id, SUM(al.amount) AS paid FROM acc_allocations al JOIN acc_payments p ON p.id = al.payment_id WHERE al.doc_kind = 'bill' AND p.status = 'posted' GROUP BY al.doc_id) x ON x.doc_id = b.id
             WHERE b.status = 'open'"
        };
        sqlx::query_as::<_, (Decimal, Decimal, i64)>(q).fetch_one(db).await
    };
    let (ar, ar_late, ar_late_n) = owed("invoice").await?;
    let (ap, ap_late, ap_late_n) = owed("bill").await?;
    let year = pl(&state, year_start(t, st.fiscal_year_start as u32), t).await?;
    let month = pl(&state, NaiveDate::from_ymd_opt(t.year(), t.month(), 1).unwrap(), t).await?;
    let unposted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM crm_items i WHERE i.entity_type = 6 AND i.deleted_at IS NULL AND i.stage_semantics <> 'failure'
           AND NOT EXISTS (SELECT 1 FROM acc_invoices a WHERE a.item_id = i.id AND a.status <> 'void')",
    )
    .fetch_one(&state.db)
    .await?;
    let unmatched: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM acc_bank_lines WHERE status = 'unmatched'").fetch_one(&state.db).await?;
    let drafts: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM acc_bills WHERE status = 'draft') + (SELECT COUNT(*) FROM acc_entries WHERE status = 'draft')").fetch_one(&state.db).await?;
    Ok(Json(json!({
        "ready": true, "base_currency": super::base_currency(&state).await?, "lock_date": st.lock_date,
        "money": money.into_iter().map(|(id, code, name, cur, bal, foreign)| json!({ "account_id": id, "code": code, "name": name, "currency": cur, "balance": bal, "balance_currency": if cur.is_some() { Some(foreign) } else { None } })).collect::<Vec<_>>(),
        "receivable": ledger::round(ar), "receivable_overdue": ledger::round(ar_late), "overdue_invoices": ar_late_n,
        "payable": ledger::round(ap), "payable_overdue": ledger::round(ap_late), "overdue_bills": ap_late_n,
        "year": { "from": year["from"], "income": year["sections"][0]["total"], "net_profit": year["net_profit"] },
        "month": { "from": month["from"], "income": month["sections"][0]["total"], "net_profit": month["net_profit"] },
        "unposted_invoices": unposted, "unmatched_bank_lines": unmatched, "drafts": drafts,
    })))
}

#[derive(Deserialize)]
struct CloseBody {
    /// The last day of the financial year being closed.
    year_end: NaiveDate,
}

/// Year-end closing: income and expense accounts are brought to zero against retained earnings,
/// and the books are locked up to that day. Reopen by reversing the closing entry (after moving
/// the lock date back).
async fn close_year(State(state): State<AppState>, auth: AuthUser, Json(b): Json<CloseBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Admin)?;
    let st = ready(&state).await?;
    let re = default_account(st.retained_earnings, "retained earnings")?;
    let next = b.year_end + chrono::Duration::days(1);
    if next.day() != 1 || next.month() as i16 != st.fiscal_year_start {
        return Err(AppError::bad("that is not the last day of a financial year"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT id FROM acc_settings WHERE id = 1 FOR UPDATE").execute(&mut *tx).await?;
    let done: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM acc_entries WHERE source = 'closing' AND date = $1 AND status = 'posted')").bind(b.year_end).fetch_one(&mut *tx).await?;
    if done {
        return Err(AppError::Conflict("that year is already closed".into()));
    }
    let drafts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM acc_entries WHERE status = 'draft' AND date <= $1").bind(b.year_end).fetch_one(&mut *tx).await?;
    if drafts > 0 {
        return Err(AppError::bad(format!("{drafts} draft entr{} dated in that year; post or delete them first", if drafts == 1 { "y is" } else { "ies are" })));
    }
    let rows: Vec<(Uuid, Decimal)> = sqlx::query_as(
        "SELECT a.id, SUM(l.debit - l.credit) FROM acc_accounts a JOIN acc_lines l ON l.account_id = a.id JOIN acc_entries e ON e.id = l.entry_id
         WHERE a.type IN ('income', 'expense') AND e.status <> 'draft' AND l.date <= $1 GROUP BY a.id HAVING SUM(l.debit - l.credit) <> 0",
    )
    .bind(b.year_end)
    .fetch_all(&mut *tx)
    .await?;
    let mut entry = None;
    if !rows.is_empty() {
        let mut lines: Vec<ledger::Line> = rows.iter().map(|(a, bal)| ledger::signed(*a, -*bal)).collect();
        let profit: Decimal = rows.iter().map(|(_, b)| -*b).sum::<Decimal>();
        lines.push(ledger::signed(re, -profit).text("Profit for the year"));
        entry = Some(ledger::post(&mut tx, b.year_end, Some(&format!("Year-end closing {}", b.year_end)), "closing", None, &lines, Some(auth.id)).await?);
    }
    sqlx::query("UPDATE acc_settings SET lock_date = GREATEST(COALESCE(lock_date, $1), $1) WHERE id = 1").bind(b.year_end).execute(&mut *tx).await?;
    tx.commit().await?;
    audit::record(&state.db, Some(auth.id), "accounting.close_year", "settings", None, json!({ "year_end": b.year_end }), None).await;
    Ok(Json(json!({ "entry_id": entry, "lock_date": b.year_end })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn financial_years() {
        let d = |y, m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
        assert_eq!(year_start(d(2026, 3, 15), 4), d(2025, 4, 1));
        assert_eq!(year_start(d(2026, 4, 1), 4), d(2026, 4, 1));
        assert_eq!(year_start(d(2026, 12, 31), 1), d(2026, 1, 1));
    }
}
