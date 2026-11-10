//! Bills from suppliers: drafts (editable), then posted (expenses and reclaimable tax debited,
//! payables to the supplier credited), paid with payments, or voided (a reversing entry) while
//! unpaid.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
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
        .route("/accounting/bills", get(list).post(create))
        .route("/accounting/bills/{id}", get(detail).put(update).delete(remove))
        .route("/accounting/bills/{id}/post", post(post_bill))
        .route("/accounting/bills/{id}/void", post(void_bill))
}

#[derive(Deserialize, Clone)]
struct LineIn {
    description: String,
    account_id: Uuid,
    #[serde(default = "one")]
    quantity: Decimal,
    unit_price: Decimal,
    tax_id: Option<Uuid>,
}
fn one() -> Decimal {
    Decimal::ONE
}

#[derive(Deserialize)]
struct BillIn {
    party_id: Option<Uuid>,
    reference: Option<String>,
    date: NaiveDate,
    due_date: Option<NaiveDate>,
    currency: Option<String>,
    fx_rate: Option<Decimal>,
    #[serde(default)]
    tax_inclusive: bool,
    memo: Option<String>,
    #[serde(default)]
    file_ids: Vec<Uuid>,
    lines: Vec<LineIn>,
}

struct Computed {
    /// (description, account, quantity, unit price, tax, net, tax amount)
    lines: Vec<(String, Uuid, Decimal, Decimal, Option<Uuid>, Decimal, Decimal)>,
    subtotal: Decimal,
    tax: Decimal,
    total: Decimal,
}

const NOT_FOR_BILLS: &[&str] = &["receivable", "payable", "bank", "cash", "retained_earnings"];

/// Net and tax of every line (in the bill's currency).
async fn compute(state: &AppState, lines: &[LineIn], inclusive: bool) -> AppResult<Computed> {
    if lines.is_empty() || lines.len() > 200 {
        return Err(AppError::bad("a bill has one to 200 lines"));
    }
    let mut out = Computed { lines: vec![], subtotal: Decimal::ZERO, tax: Decimal::ZERO, total: Decimal::ZERO };
    for l in lines {
        let desc = crate::routes::required("line description", &l.description, 500)?;
        let sub: Option<String> = sqlx::query_scalar("SELECT subtype FROM acc_accounts WHERE id = $1 AND is_active").bind(l.account_id).fetch_optional(&state.db).await?;
        match sub.as_deref() {
            None => return Err(AppError::bad("a line's account doesn't exist or is inactive")),
            Some(s) if NOT_FOR_BILLS.contains(&s) => return Err(AppError::bad("bill lines go to expense, cost or asset accounts")),
            _ => {}
        }
        if l.quantity <= Decimal::ZERO || l.unit_price < Decimal::ZERO {
            return Err(AppError::bad("quantities are positive and prices not negative"));
        }
        let rate = match l.tax_id {
            Some(t) => {
                let tax = super::tax(state, t).await?;
                if tax.scope != "purchases" || !tax.is_active {
                    return Err(AppError::bad(format!("“{}” is not an active purchase tax", tax.name)));
                }
                tax.rate / Decimal::ONE_HUNDRED
            }
            None => Decimal::ZERO,
        };
        let amount = l.quantity * l.unit_price;
        let (net, tax) = if inclusive {
            let gross = ledger::round(amount);
            let net = ledger::round(gross / (Decimal::ONE + rate));
            (net, gross - net)
        } else {
            let net = ledger::round(amount);
            (net, ledger::round(net * rate))
        };
        out.subtotal += net;
        out.tax += tax;
        out.lines.push((desc, l.account_id, l.quantity, l.unit_price, l.tax_id, net, tax));
    }
    out.total = out.subtotal + out.tax;
    Ok(out)
}

async fn check_files(state: &AppState, auth: &AuthUser, files: &[Uuid]) -> AppResult<()> {
    if files.len() > 20 {
        return Err(AppError::bad("at most 20 files"));
    }
    let own: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM files WHERE id = ANY($1) AND (owner_id = $2 OR id IN (SELECT unnest(file_ids) FROM acc_bills))")
        .bind(files)
        .bind(auth.id)
        .fetch_one(&state.db)
        .await?;
    if own as usize != files.len() {
        return Err(AppError::bad("attach files you uploaded"));
    }
    Ok(())
}

async fn save(state: &AppState, auth: &AuthUser, id: Option<Uuid>, b: BillIn) -> AppResult<Uuid> {
    let s = ready(state).await?;
    let _ = s;
    if let Some(p) = b.party_id {
        party_exists(state, p).await?;
    }
    if b.due_date.is_some_and(|d| d < b.date) {
        return Err(AppError::bad("the due date can't be before the bill's date"));
    }
    let base = super::base_currency(state).await?;
    let currency = b.currency.as_deref().map(|c| c.trim().to_uppercase()).filter(|c| !c.is_empty()).unwrap_or(base.clone());
    let fx = fx_rate(state, &currency, b.fx_rate).await?;
    check_files(state, auth, &b.file_ids).await?;
    let c = compute(state, &b.lines, b.tax_inclusive).await?;
    let mut tx = state.db.begin().await?;
    let id = match id {
        Some(id) => {
            let st: Option<String> = sqlx::query_scalar("SELECT status FROM acc_bills WHERE id = $1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
            match st.as_deref() {
                None => return Err(AppError::NotFound("bill")),
                Some("draft") => {}
                Some(_) => return Err(AppError::bad("only draft bills can be changed; void it and enter it again")),
            }
            sqlx::query(
                "UPDATE acc_bills SET party_id = $2, reference = $3, date = $4, due_date = $5, currency = $6, fx_rate = $7, tax_inclusive = $8,
                     subtotal = $9, tax = $10, total = $11, memo = $12, file_ids = $13, updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .bind(b.party_id)
            .bind(clean(b.reference.clone()))
            .bind(b.date)
            .bind(b.due_date)
            .bind(&currency)
            .bind(fx)
            .bind(b.tax_inclusive)
            .bind(c.subtotal)
            .bind(c.tax)
            .bind(c.total)
            .bind(clean(b.memo.clone()))
            .bind(&b.file_ids)
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM acc_bill_lines WHERE bill_id = $1").bind(id).execute(&mut *tx).await?;
            id
        }
        None => sqlx::query_scalar(
            "INSERT INTO acc_bills (party_id, reference, date, due_date, currency, fx_rate, tax_inclusive, subtotal, tax, total, memo, file_ids, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING id",
        )
        .bind(b.party_id)
        .bind(clean(b.reference.clone()))
        .bind(b.date)
        .bind(b.due_date)
        .bind(&currency)
        .bind(fx)
        .bind(b.tax_inclusive)
        .bind(c.subtotal)
        .bind(c.tax)
        .bind(c.total)
        .bind(clean(b.memo.clone()))
        .bind(&b.file_ids)
        .bind(auth.id)
        .fetch_one(&mut *tx)
        .await?,
    };
    for (i, (desc, account, q, price, tax_id, net, tax)) in c.lines.iter().enumerate() {
        sqlx::query("INSERT INTO acc_bill_lines (bill_id, description, account_id, quantity, unit_price, tax_id, net, tax, sort) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)")
            .bind(id)
            .bind(desc)
            .bind(account)
            .bind(q)
            .bind(price)
            .bind(tax_id)
            .bind(net)
            .bind(tax)
            .bind(i as i32)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(id)
}

pub async fn bill_json(state: &AppState, id: Uuid) -> AppResult<Value> {
    type B = (Uuid, Option<i64>, Option<String>, Option<Uuid>, Option<String>, NaiveDate, Option<NaiveDate>, String, Decimal, bool, Decimal, Decimal, Decimal, String, Option<Uuid>, Option<String>);
    let b: B = sqlx::query_as(
        "SELECT b.id, b.number, b.reference, b.party_id, p.title, b.date, b.due_date, b.currency, b.fx_rate, b.tax_inclusive, b.subtotal, b.tax, b.total,
                b.status, b.entry_id, b.memo
         FROM acc_bills b LEFT JOIN crm_items p ON p.id = b.party_id WHERE b.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("bill"))?;
    let (file_ids, entry_number): (Vec<Uuid>, Option<i64>) =
        sqlx::query_as("SELECT b.file_ids, e.number FROM acc_bills b LEFT JOIN acc_entries e ON e.id = b.entry_id WHERE b.id = $1").bind(id).fetch_one(&state.db).await?;
    let lines: Vec<(Uuid, String, Uuid, String, String, Decimal, Decimal, Option<Uuid>, Option<String>, Decimal, Decimal)> = sqlx::query_as(
        "SELECT l.id, l.description, l.account_id, a.code, a.name, l.quantity, l.unit_price, l.tax_id, t.name, l.net, l.tax
         FROM acc_bill_lines l JOIN acc_accounts a ON a.id = l.account_id LEFT JOIN acc_taxes t ON t.id = l.tax_id WHERE l.bill_id = $1 ORDER BY l.sort",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let files: Vec<(Uuid, String, i64)> = sqlx::query_as("SELECT id, original_name, size_bytes FROM files WHERE id = ANY($1)").bind(&file_ids).fetch_all(&state.db).await?;
    let payments: Vec<(Uuid, i64, NaiveDate, Decimal, String)> = sqlx::query_as(
        "SELECT p.id, p.number, p.date, al.amount, p.status FROM acc_allocations al JOIN acc_payments p ON p.id = al.payment_id
         WHERE al.doc_kind = 'bill' AND al.doc_id = $1 ORDER BY p.date, p.number",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let paid = super::invoices::paid(&state.db, "bill", id).await?;
    Ok(json!({
        "id": b.0, "number": b.1, "reference": b.2, "party_id": b.3, "party_name": b.4, "date": b.5, "due_date": b.6, "currency": b.7, "fx_rate": b.8,
        "tax_inclusive": b.9, "subtotal": b.10, "tax": b.11, "total": b.12, "status": b.13, "entry_id": b.14, "memo": b.15, "entry_number": entry_number,
        "paid": paid, "balance": if b.13 == "void" || b.13 == "draft" { Decimal::ZERO } else { b.12 - paid },
        "files": files.into_iter().map(|(id, n, s)| json!({ "id": id, "name": n, "size": s })).collect::<Vec<_>>(),
        "lines": lines.into_iter().map(|l| json!({ "id": l.0, "description": l.1, "account_id": l.2, "account_code": l.3, "account_name": l.4, "quantity": l.5, "unit_price": l.6, "tax_id": l.7, "tax_name": l.8, "net": l.9, "tax": l.10 })).collect::<Vec<_>>(),
        "payments": payments.into_iter().map(|(id, n, d, a, s)| json!({ "id": id, "number": n, "date": d, "amount": a, "status": s })).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
struct ListQuery {
    /// draft | open | overdue | paid | void
    status: Option<String>,
    party_id: Option<Uuid>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ListQuery>) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let term = q.q.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(|t| format!("%{}%", t.to_lowercase().replace('%', "\\%").replace('_', "\\_")));
    type R = (Uuid, Option<i64>, Option<String>, Option<Uuid>, Option<String>, NaiveDate, Option<NaiveDate>, String, Decimal, String, Decimal);
    let rows: Vec<R> = sqlx::query_as(
        "SELECT b.id, b.number, b.reference, b.party_id, p.title, b.date, b.due_date, b.currency, b.total, b.status,
                COALESCE((SELECT SUM(al.amount) FROM acc_allocations al JOIN acc_payments x ON x.id = al.payment_id WHERE al.doc_kind = 'bill' AND al.doc_id = b.id AND x.status = 'posted'), 0)
         FROM acc_bills b LEFT JOIN crm_items p ON p.id = b.party_id
         WHERE CASE $1 WHEN 'overdue' THEN b.status = 'open' AND b.due_date < current_date WHEN 'all' THEN TRUE ELSE b.status = $1 END
           AND ($2::uuid IS NULL OR b.party_id = $2)
           AND ($3::text IS NULL OR lower(COALESCE(b.reference, '')) LIKE $3 OR lower(COALESCE(p.title, '')) LIKE $3 OR lower(COALESCE(b.memo, '')) LIKE $3 OR b.number::text = trim(both '%' from $3))
         ORDER BY b.date DESC, b.number DESC NULLS FIRST LIMIT $4 OFFSET $5",
    )
    .bind(q.status.as_deref().unwrap_or("all"))
    .bind(q.party_id)
    .bind(term)
    .bind(limit + 1)
    .bind(q.offset.unwrap_or(0).max(0))
    .fetch_all(&state.db)
    .await?;
    let today = Utc::now().date_naive();
    let more = rows.len() as i64 > limit;
    Ok(Json(json!({
        "items": rows.into_iter().take(limit as usize).map(|r| json!({
            "id": r.0, "number": r.1, "reference": r.2, "party_id": r.3, "party_name": r.4, "date": r.5, "due_date": r.6, "currency": r.7,
            "total": r.8, "status": r.9, "paid": r.10, "balance": if r.9 == "open" || r.9 == "paid" { r.8 - r.10 } else { Decimal::ZERO },
            "overdue_days": match (r.9.as_str(), r.6) { ("open", Some(d)) if d < today => Some((today - d).num_days()), _ => None },
        })).collect::<Vec<_>>(),
        "more": more,
    })))
}

async fn detail(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    Ok(Json(bill_json(&state, id).await?))
}

async fn create(State(state): State<AppState>, auth: AuthUser, Json(b): Json<BillIn>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let id = save(&state, &auth, None, b).await?;
    Ok(Json(bill_json(&state, id).await?))
}

async fn update(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<BillIn>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    save(&state, &auth, Some(id), b).await?;
    Ok(Json(bill_json(&state, id).await?))
}

async fn remove(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let n = sqlx::query("DELETE FROM acc_bills WHERE id = $1 AND status = 'draft'").bind(id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::bad("only draft bills can be deleted; void a posted one"));
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
struct PostBody {
    fx_rate: Option<Decimal>,
}

async fn post_bill(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, body: Option<Json<PostBody>>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let s = ready(&state).await?;
    let ap = default_account(s.payable_account, "payables")?;
    let base = super::base_currency(&state).await?;
    let mut tx = state.db.begin().await?;
    type B = (String, Option<Uuid>, NaiveDate, String, Decimal, Decimal, Option<String>);
    let (status, party, date, currency, mut fx, total, reference): B =
        sqlx::query_as("SELECT status, party_id, date, currency, fx_rate, total, reference FROM acc_bills WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AppError::NotFound("bill"))?;
    if status != "draft" {
        return Err(AppError::bad("this bill is already posted"));
    }
    let party = party.ok_or_else(|| AppError::bad("choose the supplier first"))?;
    if let Some(r) = body.and_then(|b| b.0.fx_rate) {
        fx = fx_rate(&state, &currency, Some(r)).await?;
    }
    if total <= Decimal::ZERO {
        return Err(AppError::bad("the bill's total must be more than zero"));
    }
    let lines: Vec<(String, Uuid, Option<Uuid>, Decimal, Decimal)> =
        sqlx::query_as("SELECT description, account_id, tax_id, net, tax FROM acc_bill_lines WHERE bill_id = $1 ORDER BY sort").bind(id).fetch_all(&mut *tx).await?;
    let mut entry = Vec::new();
    let mut taxes: BTreeMap<Uuid, (Decimal, Decimal)> = BTreeMap::new();
    for (desc, account, tax_id, net, tax) in &lines {
        entry.push(ledger::dr(*account, net * fx).foreign(&currency, &base, *net).text(desc.clone()).party(Some(party)));
        if let Some(t) = tax_id {
            let e = taxes.entry(*t).or_default();
            e.0 += *net;
            e.1 += *tax;
        }
    }
    for (t, (net, tax)) in &taxes {
        if tax.is_zero() {
            continue;
        }
        let account: Uuid = sqlx::query_scalar("SELECT account_id FROM acc_taxes WHERE id = $1").bind(t).fetch_one(&mut *tx).await?;
        entry.push(ledger::dr(account, tax * fx).foreign(&currency, &base, *tax).taxed(*t, net * fx).party(Some(party)));
    }
    entry.push(ledger::cr(ap, total * fx).foreign(&currency, &base, -total).party(Some(party)));
    let diff: Decimal = entry.iter().map(|l| l.debit - l.credit).sum();
    if !diff.is_zero() {
        entry.push(ledger::signed(default_account(s.rounding_account, "rounding")?, -diff).text("Rounding"));
    }
    let number = ledger::next_number(&mut tx, "bill").await?;
    let memo = format!("Bill {number}{}", reference.map(|r| format!(" · {r}")).unwrap_or_default());
    let e = ledger::post(&mut tx, date, Some(&memo), "bill", Some(id), &entry, Some(auth.id)).await?;
    sqlx::query("UPDATE acc_bills SET status = 'open', number = $2, entry_id = $3, fx_rate = $4, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(number)
        .bind(e)
        .bind(fx)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    audit::record(&state.db, Some(auth.id), "accounting.bill_post", "bill", Some(id.to_string()), json!({ "number": number, "total": total }), None).await;
    Ok(Json(bill_json(&state, id).await?))
}

/// Open or paid, from what has been paid.
pub async fn refresh(tx: &mut ledger::Tx<'_>, id: Uuid) -> AppResult<()> {
    let row: Option<(Decimal, String)> = sqlx::query_as("SELECT total, status FROM acc_bills WHERE id = $1 FOR UPDATE").bind(id).fetch_optional(&mut **tx).await?;
    let Some((total, status)) = row else { return Ok(()) };
    if status != "open" && status != "paid" {
        return Ok(());
    }
    let p = super::invoices::paid(&mut **tx, "bill", id).await?;
    sqlx::query("UPDATE acc_bills SET status = $2 WHERE id = $1").bind(id).bind(if p >= total { "paid" } else { "open" }).execute(&mut **tx).await?;
    Ok(())
}

#[derive(Deserialize, Default)]
struct VoidBody {
    date: Option<NaiveDate>,
}

async fn void_bill(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, body: Option<Json<VoidBody>>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let mut tx = state.db.begin().await?;
    let row: Option<(String, Option<Uuid>, NaiveDate)> = sqlx::query_as("SELECT status, entry_id, date FROM acc_bills WHERE id = $1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
    let (status, entry, date) = row.ok_or(AppError::NotFound("bill"))?;
    if status == "draft" {
        return Err(AppError::bad("a draft bill is deleted, not voided"));
    }
    if status == "void" {
        return Err(AppError::bad("this bill is already void"));
    }
    if !super::invoices::paid(&mut *tx, "bill", id).await?.is_zero() {
        return Err(AppError::bad("payments are allocated to this bill; void those payments first"));
    }
    let when = match body.and_then(|b| b.0.date) {
        Some(d) => d,
        None => super::invoices::void_date(&state, date).await?,
    };
    if let Some(e) = entry {
        ledger::reverse(&mut tx, e, when, Some(auth.id)).await?;
    }
    sqlx::query("UPDATE acc_bills SET status = 'void', updated_at = now() WHERE id = $1").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    audit::record(&state.db, Some(auth.id), "accounting.bill_void", "bill", Some(id.to_string()), json!({}), None).await;
    Ok(Json(bill_json(&state, id).await?))
}
