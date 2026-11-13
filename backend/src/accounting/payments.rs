//! Payments: money received from customers (in) or paid to suppliers (out), through a bank or
//! cash account, allocated to invoices (in) or bills (out).
//!
//! A payment is in the currency of its bank account. Receivables (or payables) are cleared at
//! each document's own exchange rate; the difference to the payment's rate is an exchange gain
//! or loss. What isn't allocated stays on the customer's (supplier's) account as an advance and
//! can be allocated later. Voiding a payment reverses its entry and reopens what it paid.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{default_account, fx_rate, ledger, need, party_exists, ready, Level};
use crate::{
    audit,
    auth::AuthUser,
    error::{AppError, AppResult},
    routes::clean,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/accounting/payments", get(list).post(create))
        .route("/accounting/payments/{id}", get(detail))
        .route("/accounting/payments/{id}/void", post(void_payment))
        .route("/accounting/payments/{id}/allocate", post(allocate_later))
}

#[derive(Deserialize, Clone)]
pub struct AllocIn {
    pub doc_kind: String,
    pub doc_id: Uuid,
    pub amount: Decimal,
}

#[derive(Deserialize, Clone)]
pub struct PaymentIn {
    pub direction: String,
    pub date: NaiveDate,
    pub account_id: Uuid,
    pub party_id: Option<Uuid>,
    pub currency: Option<String>,
    pub amount: Decimal,
    pub fx_rate: Option<Decimal>,
    pub reference: Option<String>,
    pub memo: Option<String>,
    #[serde(default)]
    pub allocations: Vec<AllocIn>,
}

/// A document being paid, locked for the transaction: (currency, rate, party, balance).
async fn lock_doc(tx: &mut ledger::Tx<'_>, kind: &str, id: Uuid) -> AppResult<(String, Decimal, Option<Uuid>, Decimal)> {
    let row: Option<(String, Decimal, Option<Uuid>, Decimal, String)> = match kind {
        // What an invoice is owed is its total less its credit notes.
        "invoice" => sqlx::query_as("SELECT currency, fx_rate, party_id, total - credited, status FROM acc_invoices WHERE item_id = $1 FOR UPDATE").bind(id).fetch_optional(&mut **tx).await?,
        "bill" => sqlx::query_as("SELECT currency, fx_rate, party_id, total, status FROM acc_bills WHERE id = $1 FOR UPDATE").bind(id).fetch_optional(&mut **tx).await?,
        _ => return Err(AppError::bad("payments are allocated to invoices or bills")),
    };
    let (cur, fx, party, total, status) = row.ok_or(AppError::NotFound(if kind == "invoice" { "invoice in the books" } else { "bill" }))?;
    if status != "open" {
        return Err(AppError::bad(format!("that {kind} is {status}, not open")));
    }
    let paid = super::invoices::paid(&mut **tx, kind, id).await?;
    Ok((cur, fx, party, total - paid))
}

/// The bank or cash account, and its currency.
async fn money_account(state: &AppState, id: Uuid) -> AppResult<String> {
    let row: Option<(String, Option<String>, bool)> = sqlx::query!(r#"SELECT subtype, currency, is_active FROM acc_accounts WHERE id = $1"#, id).map(|r| (r.subtype, r.currency, r.is_active)).fetch_optional(&state.db).await?;
    match row {
        Some((s, c, true)) if s == "bank" || s == "cash" => Ok(c.unwrap_or(super::base_currency(state).await?)),
        Some(_) => Err(AppError::bad("payments go through an active bank or cash account")),
        None => Err(AppError::NotFound("account")),
    }
}

/// Records and posts a payment (in its own transaction). Returns its id and the invoices that
/// became fully paid.
pub async fn record(state: &AppState, auth: &AuthUser, p: PaymentIn) -> AppResult<(Uuid, Vec<Uuid>)> {
    let s = ready(state).await?;
    let (inbound, control) = match p.direction.as_str() {
        "in" => (true, default_account(s.receivable_account, "receivables")?),
        "out" => (false, default_account(s.payable_account, "payables")?),
        _ => return Err(AppError::bad("a payment is in or out")),
    };
    let doc_kind = if inbound { "invoice" } else { "bill" };
    let amount = p.amount;
    if amount <= Decimal::ZERO || amount != ledger::round(amount) {
        return Err(AppError::bad("the amount is positive, with at most two decimals"));
    }
    let currency = money_account(state, p.account_id).await?;
    if let Some(c) = p.currency.as_deref().map(|c| c.trim().to_uppercase()).filter(|c| !c.is_empty()) {
        if c != currency {
            return Err(AppError::bad(format!("this account is in {currency}; the payment must be too")));
        }
    }
    let base = super::base_currency(state).await?;
    let fx = fx_rate(state, &currency, p.fx_rate).await?;
    if p.allocations.len() > 200 {
        return Err(AppError::bad("at most 200 allocations"));
    }
    let mut party = p.party_id;
    if let Some(pid) = party {
        party_exists(state, pid).await?;
    }

    let mut tx = state.db.begin().await?;
    ledger::check_open(&mut tx, p.date).await?;
    let mut lines = vec![];
    let mut allocated = Decimal::ZERO;
    let mut seen = std::collections::HashSet::new();
    for a in &p.allocations {
        if a.doc_kind != doc_kind {
            return Err(AppError::bad(if inbound { "money received is allocated to invoices" } else { "money paid is allocated to bills" }));
        }
        if !seen.insert(a.doc_id) {
            return Err(AppError::bad("each document once per payment"));
        }
        if a.amount <= Decimal::ZERO || a.amount != ledger::round(a.amount) {
            return Err(AppError::bad("allocations are positive, with at most two decimals"));
        }
        let (dcur, dfx, dparty, balance) = lock_doc(&mut tx, doc_kind, a.doc_id).await?;
        if dcur != currency {
            return Err(AppError::bad(format!("that {doc_kind} is in {dcur}; pay it from a {dcur} account")));
        }
        match (party, dparty) {
            (None, d) => party = d,
            (Some(x), Some(d)) if x != d => return Err(AppError::bad(format!("that {doc_kind} belongs to someone else"))),
            _ => {}
        }
        if a.amount > balance {
            return Err(AppError::bad(format!("that {doc_kind} has only {balance} {dcur} left to pay")));
        }
        allocated += a.amount;
        // Clears the document at its own rate.
        let l = if inbound { ledger::cr(control, a.amount * dfx).foreign(&currency, &base, -a.amount) } else { ledger::dr(control, a.amount * dfx).foreign(&currency, &base, a.amount) };
        lines.push(l.party(party));
    }
    if allocated > amount {
        return Err(AppError::bad("more is allocated than the payment's amount"));
    }
    let party = party.ok_or_else(|| AppError::bad(if inbound { "choose who paid" } else { "choose who was paid" }))?;
    for l in lines.iter_mut() {
        l.party = Some(party);
    }
    let rest = amount - allocated;
    if !rest.is_zero() {
        let l = if inbound { ledger::cr(control, rest * fx).foreign(&currency, &base, -rest) } else { ledger::dr(control, rest * fx).foreign(&currency, &base, rest) };
        lines.push(l.party(Some(party)).text("Not allocated"));
    }
    let money = if inbound { ledger::dr(p.account_id, amount * fx).foreign(&currency, &base, amount) } else { ledger::cr(p.account_id, amount * fx).foreign(&currency, &base, -amount) };
    lines.insert(0, money.party(Some(party)));
    let diff: Decimal = lines.iter().map(|l| l.debit - l.credit).sum();
    if !diff.is_zero() {
        lines.push(ledger::signed(default_account(s.fx_account, "exchange differences")?, -diff).text("Exchange difference"));
    }

    let number = ledger::next_number(&mut tx, "payment").await?;
    let id: Uuid = sqlx::query_scalar!("INSERT INTO acc_payments (number, direction, date, account_id, party_id, currency, amount, fx_rate, reference, memo, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id", number, &p.direction, p.date, p.account_id, party, &currency, amount, fx, clean(p.reference.clone()), clean(p.memo.clone()), auth.id)
    .fetch_one(&mut *tx)
    .await?;
    for a in &p.allocations {
        sqlx::query!("INSERT INTO acc_allocations (payment_id, doc_kind, doc_id, amount) VALUES ($1, $2, $3, $4)", id, &a.doc_kind, a.doc_id, a.amount).execute(&mut *tx).await?;
    }
    let memo = format!("Payment {number} {} {}", if inbound { "from" } else { "to" }, party_name(state, party).await);
    let entry = ledger::post(&mut tx, p.date, Some(&memo), "payment", Some(id), &lines, Some(auth.id)).await?;
    sqlx::query!("UPDATE acc_payments SET entry_id = $2 WHERE id = $1", id, entry).execute(&mut *tx).await?;
    let mut now_paid = vec![];
    for a in &p.allocations {
        if inbound {
            if super::invoices::refresh(&mut tx, a.doc_id).await? {
                now_paid.push(a.doc_id);
            }
        } else {
            super::bills::refresh(&mut tx, a.doc_id).await?;
        }
    }
    tx.commit().await?;
    audit::record(&state.db, Some(auth.id), "accounting.payment", "payment", Some(id.to_string()), json!({ "number": number, "amount": amount, "currency": currency }), None).await;
    for item in &now_paid {
        super::invoices::mark_paid_in_crm(state, *item).await;
    }
    if inbound {
        for a in &p.allocations {
            crate::commissions::sync_invoice(state, a.doc_id).await;
        }
    }
    Ok((id, now_paid))
}

async fn party_name(state: &AppState, id: Uuid) -> String {
    sqlx::query_scalar!("SELECT title FROM crm_items WHERE id = $1", id).fetch_optional(&state.db).await.ok().flatten().unwrap_or_default()
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(p): Json<PaymentIn>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let (id, _) = record(&state, &auth, p).await?;
    Ok(Json(payment_json(&state, id).await?))
}

pub async fn payment_json(state: &AppState, id: Uuid) -> AppResult<Value> {
    type P = (Uuid, i64, String, NaiveDate, Uuid, String, Option<Uuid>, Option<String>, String, Decimal, Decimal, Option<String>, Option<String>, String, Option<Uuid>, Option<i64>);
    let p: P = sqlx::query_as(
        "SELECT p.id, p.number, p.direction, p.date, p.account_id, a.name, p.party_id, x.title, p.currency, p.amount, p.fx_rate, p.reference, p.memo, p.status, p.entry_id, e.number
         FROM acc_payments p JOIN acc_accounts a ON a.id = p.account_id LEFT JOIN crm_items x ON x.id = p.party_id LEFT JOIN acc_entries e ON e.id = p.entry_id WHERE p.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("payment"))?;
    let allocs: Vec<(String, Uuid, Decimal, Option<String>)> = sqlx::query!(r#"SELECT al.doc_kind, al.doc_id, al.amount,
                CASE al.doc_kind WHEN 'invoice' THEN (SELECT 'Invoice ' || i.number || ' · ' || i.title FROM crm_items i WHERE i.id = al.doc_id)
                                 ELSE (SELECT 'Bill ' || COALESCE(b.number::text, '') || COALESCE(' · ' || b.reference, '') FROM acc_bills b WHERE b.id = al.doc_id) END AS "c3"
         FROM acc_allocations al WHERE al.payment_id = $1"#, id).map(|r| (r.doc_kind, r.doc_id, r.amount, r.c3))
    .fetch_all(&state.db)
    .await?;
    let allocated: Decimal = allocs.iter().map(|a| a.2).sum();
    Ok(json!({
        "id": p.0, "number": p.1, "direction": p.2, "date": p.3, "account_id": p.4, "account_name": p.5, "party_id": p.6, "party_name": p.7,
        "currency": p.8, "amount": p.9, "fx_rate": p.10, "reference": p.11, "memo": p.12, "status": p.13, "entry_id": p.14, "entry_number": p.15,
        "allocated": allocated, "unallocated": p.9 - allocated,
        "allocations": allocs.into_iter().map(|(k, d, a, label)| json!({ "doc_kind": k, "doc_id": d, "amount": a, "label": label })).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
struct ListQuery {
    direction: Option<String>,
    party_id: Option<Uuid>,
    account_id: Option<Uuid>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    /// true: only those with money not yet allocated.
    unallocated: Option<bool>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    type R = (Uuid, i64, String, NaiveDate, String, Option<Uuid>, Option<String>, String, Decimal, String, Decimal, Option<String>);
    let rows: Vec<R> = sqlx::query_as(
        "SELECT * FROM (SELECT p.id, p.number, p.direction, p.date, a.name, p.party_id, x.title, p.currency, p.amount, p.status,
                p.amount - COALESCE((SELECT SUM(amount) FROM acc_allocations al WHERE al.payment_id = p.id), 0) AS unallocated, p.reference
         FROM acc_payments p JOIN acc_accounts a ON a.id = p.account_id LEFT JOIN crm_items x ON x.id = p.party_id
         WHERE ($1::text IS NULL OR p.direction = $1) AND ($2::uuid IS NULL OR p.party_id = $2) AND ($3::uuid IS NULL OR p.account_id = $3)
           AND ($4::date IS NULL OR p.date >= $4) AND ($5::date IS NULL OR p.date <= $5)) y
         WHERE NOT COALESCE($6, FALSE) OR (y.unallocated > 0 AND y.status = 'posted')
         ORDER BY y.date DESC, y.number DESC LIMIT $7 OFFSET $8",
    )
    .bind(q.direction)
    .bind(q.party_id)
    .bind(q.account_id)
    .bind(q.from)
    .bind(q.to)
    .bind(q.unallocated)
    .bind(limit + 1)
    .bind(q.offset.unwrap_or(0).max(0))
    .fetch_all(&state.db)
    .await?;
    let more = rows.len() as i64 > limit;
    Ok(Json(json!({
        "items": rows.into_iter().take(limit as usize).map(|r| json!({
            "id": r.0, "number": r.1, "direction": r.2, "date": r.3, "account_name": r.4, "party_id": r.5, "party_name": r.6,
            "currency": r.7, "amount": r.8, "status": r.9, "unallocated": r.10, "reference": r.11,
        })).collect::<Vec<_>>(),
        "more": more,
    })))
}

async fn detail(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    Ok(Json(payment_json(&state, id).await?))
}

#[derive(Deserialize, Default)]
struct VoidBody {
    date: Option<NaiveDate>,
}

async fn void_payment(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, body: Option<Json<VoidBody>>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let mut tx = state.db.begin().await?;
    let row: Option<(String, Option<Uuid>, NaiveDate)> = sqlx::query!(r#"SELECT status, entry_id, date FROM acc_payments WHERE id = $1 FOR UPDATE"#, id).map(|r| (r.status, r.entry_id, r.date)).fetch_optional(&mut *tx).await?;
    let (status, entry, date) = row.ok_or(AppError::NotFound("payment"))?;
    if status != "posted" {
        return Err(AppError::bad("this payment is already void"));
    }
    let when = match body.and_then(|b| b.0.date) {
        Some(d) => d,
        None => super::invoices::void_date(&state, date).await?,
    };
    // Every entry this payment made (the payment, and later allocations' exchange differences).
    let entries: Vec<Uuid> = sqlx::query_scalar!("SELECT id FROM acc_entries WHERE source = 'payment' AND source_id = $1 AND status = 'posted' AND reverses IS NULL", id)
        .fetch_all(&mut *tx)
        .await?;
    sqlx::query!("UPDATE acc_bank_lines SET status = 'unmatched', line_id = NULL WHERE line_id IN (SELECT id FROM acc_lines WHERE entry_id = ANY($1))", &entries).execute(&mut *tx).await?;
    for e in entries.iter().chain(entry.iter().filter(|e| !entries.contains(e))) {
        ledger::reverse(&mut tx, *e, when, Some(auth.id)).await?;
    }
    sqlx::query!("UPDATE acc_payments SET status = 'void' WHERE id = $1", id).execute(&mut *tx).await?;
    let docs: Vec<(String, Uuid)> = sqlx::query!(r#"SELECT doc_kind, doc_id FROM acc_allocations WHERE payment_id = $1"#, id).map(|r| (r.doc_kind, r.doc_id)).fetch_all(&mut *tx).await?;
    for (k, d) in &docs {
        if k == "invoice" {
            super::invoices::refresh(&mut tx, *d).await?;
        } else {
            super::bills::refresh(&mut tx, *d).await?;
        }
    }
    tx.commit().await?;
    // A payment taken back takes back the commission on it.
    for (k, d) in &docs {
        if k == "invoice" {
            crate::commissions::sync_invoice(&state, *d).await;
        }
    }
    audit::record(&state.db, Some(auth.id), "accounting.payment_void", "payment", Some(id.to_string()), json!({}), None).await;
    Ok(Json(payment_json(&state, id).await?))
}

#[derive(Deserialize)]
struct LaterBody {
    allocations: Vec<AllocIn>,
    date: Option<NaiveDate>,
}

/// Allocates money that was paid in advance (not allocated) to invoices or bills. In another
/// currency, the difference between the two exchange rates is booked as an exchange difference.
async fn allocate_later(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<LaterBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let s = ready(&state).await?;
    let base = super::base_currency(&state).await?;
    let mut tx = state.db.begin().await?;
    type P = (String, String, Option<Uuid>, String, Decimal, Decimal, NaiveDate);
    let (status, direction, party, currency, amount, pfx, pdate): P =
        sqlx::query!(r#"SELECT status, direction, party_id, currency, amount, fx_rate, date FROM acc_payments WHERE id = $1 FOR UPDATE"#, id).map(|r| (r.status, r.direction, r.party_id, r.currency, r.amount, r.fx_rate, r.date))
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AppError::NotFound("payment"))?;
    if status != "posted" {
        return Err(AppError::bad("this payment is void"));
    }
    let inbound = direction == "in";
    let doc_kind = if inbound { "invoice" } else { "bill" };
    let control = if inbound { default_account(s.receivable_account, "receivables")? } else { default_account(s.payable_account, "payables")? };
    let done: Decimal = sqlx::query_scalar!(r#"SELECT COALESCE(SUM(amount), 0) AS "v!" FROM acc_allocations WHERE payment_id = $1"#, id).fetch_one(&mut *tx).await?;
    let mut free = amount - done;
    let mut fx_lines = vec![];
    let mut now_paid = vec![];
    for a in &b.allocations {
        if a.doc_kind != doc_kind || a.amount <= Decimal::ZERO || a.amount != ledger::round(a.amount) {
            return Err(AppError::bad(format!("allocate positive amounts to {doc_kind}s")));
        }
        let (dcur, dfx, dparty, balance) = lock_doc(&mut tx, doc_kind, a.doc_id).await?;
        if dcur != currency || dparty != party {
            return Err(AppError::bad(format!("that {doc_kind} is not {}'s in {currency}", if inbound { "this customer" } else { "this supplier" })));
        }
        if a.amount > balance || a.amount > free {
            return Err(AppError::bad("that is more than is left to pay, or left on the payment"));
        }
        free -= a.amount;
        sqlx::query!("INSERT INTO acc_allocations (payment_id, doc_kind, doc_id, amount) VALUES ($1, $2, $3, $4) ON CONFLICT (payment_id, doc_kind, doc_id) DO UPDATE SET amount = acc_allocations.amount + EXCLUDED.amount", id, doc_kind, a.doc_id, a.amount)
            .execute(&mut *tx)
            .await?;
        // The advance was booked at the payment's rate, the document at its own.
        let diff = ledger::round(a.amount * dfx) - ledger::round(a.amount * pfx);
        if !diff.is_zero() {
            let sign = if inbound { -diff } else { diff };
            fx_lines.push(ledger::signed(control, sign).party(party).foreign(&currency, &base, Decimal::ZERO));
        }
    }
    if !fx_lines.is_empty() {
        let total: Decimal = fx_lines.iter().map(|l| l.debit - l.credit).sum();
        if !total.is_zero() {
            fx_lines.push(ledger::signed(default_account(s.fx_account, "exchange differences")?, -total).text("Exchange difference"));
            let date = b.date.unwrap_or_else(|| chrono::Utc::now().date_naive()).max(pdate);
            ledger::post(&mut tx, date, Some("Exchange difference on allocating an advance"), "payment", Some(id), &fx_lines, Some(auth.id)).await?;
        }
    }
    for a in &b.allocations {
        if inbound {
            if super::invoices::refresh(&mut tx, a.doc_id).await? {
                now_paid.push(a.doc_id);
            }
        } else {
            super::bills::refresh(&mut tx, a.doc_id).await?;
        }
    }
    tx.commit().await?;
    for item in now_paid {
        super::invoices::mark_paid_in_crm(&state, item).await;
    }
    if inbound {
        for a in &b.allocations {
            crate::commissions::sync_invoice(&state, a.doc_id).await;
        }
    }
    Ok(Json(payment_json(&state, id).await?))
}
