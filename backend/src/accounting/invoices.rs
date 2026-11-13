//! CRM invoices in the books.
//!
//! Posting an invoice: receivables (the customer) debited with the total; sales (per product's
//! income account, else the default sales account) and tax payable (per rate, matched to a sales
//! tax) credited; in the base currency at the invoice's exchange rate, with any rounding
//! difference to the rounding account. A posted invoice's amounts, products, currency and client
//! are locked in the CRM until it is voided (a reversing entry), which needs it to be unpaid.

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

use super::{default_account, fx_rate, ledger, need, ready, Level};
use crate::{
    audit,
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

pub const INVOICE_TYPE: i16 = 6;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/accounting/invoices", get(list))
        .route("/accounting/invoices/{item}", get(detail))
        .route("/accounting/invoices/{item}/post", post(post_invoice))
        .route("/accounting/invoices/{item}/void", post(void_invoice))
}

/// Whether the CRM invoice is in the books (and so can't change its amounts).
pub async fn locked(state: &AppState, item: Uuid) -> AppResult<bool> {
    Ok(sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM acc_invoices WHERE item_id = $1 AND status <> 'void') AS "v!""#, item).fetch_one(&state.db).await?)
}

/// What a document still owes: total minus what posted payments allocated to it.
pub async fn paid(db: impl sqlx::PgExecutor<'_>, kind: &str, doc: Uuid) -> AppResult<Decimal> {
    Ok(sqlx::query_scalar!(r#"SELECT COALESCE(SUM(a.amount), 0) AS "v!" FROM acc_allocations a JOIN acc_payments p ON p.id = a.payment_id
         WHERE a.doc_kind = $1 AND a.doc_id = $2 AND p.status = 'posted'"#, kind, doc)
    .fetch_one(db)
    .await?)
}

/// Open or paid, from what has been paid. Returns true when it just became paid.
pub async fn refresh(tx: &mut ledger::Tx<'_>, item: Uuid) -> AppResult<bool> {
    let row: Option<(Decimal, Decimal, String)> = sqlx::query!(r#"SELECT total, credited, status FROM acc_invoices WHERE item_id = $1 FOR UPDATE"#, item).map(|r| (r.total, r.credited, r.status)).fetch_optional(&mut **tx).await?;
    let Some((total, credited, status)) = row else { return Ok(false) };
    if status == "void" {
        return Ok(false);
    }
    let p = paid(&mut **tx, "invoice", item).await?;
    // Settled by payments, or by credit notes (with or without payments).
    let new = if p + credited < total { "open" } else if credited.is_zero() || p >= total { "paid" } else { "credited" };
    sqlx::query!("UPDATE acc_invoices SET status = $2 WHERE item_id = $1", item, new).execute(&mut **tx).await?;
    Ok(new == "paid" && status != "paid")
}

/// After an invoice became fully paid: its CRM stage moves to "paid" (the success stage), if
/// the settings say so.
pub async fn mark_paid_in_crm(state: &AppState, item: Uuid) {
    let Ok(s) = super::settings(state).await else { return };
    if !s.mark_invoices_paid {
        return;
    }
    let stage: Option<(Uuid, String)> = sqlx::query!(r#"SELECT s.id, i.stage_semantics FROM crm_items i JOIN crm_stages s ON s.category_id = i.category_id AND s.semantics = 'success'
         WHERE i.id = $1 AND i.deleted_at IS NULL ORDER BY s.sort LIMIT 1"#, item).map(|r| (r.id, r.stage_semantics))
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let Some((stage, current)) = stage else { return };
    if current == "success" {
        return;
    }
    let Ok(t) = crate::crm::entity_type_by_id(state, INVOICE_TYPE).await else { return };
    let mut m = serde_json::Map::new();
    m.insert("stage_id".into(), json!(stage));
    if let Err(e) = crate::crm::items::update_item(state, &crate::crm::automation::system_auth(), &t, item, m).await {
        tracing::warn!(item = %item, error = ?e, "could not mark a paid invoice as paid in the CRM");
    }
}

/// The date a void is booked on: the document's own date while that period is open, else today.
pub async fn void_date(state: &AppState, orig: NaiveDate) -> AppResult<NaiveDate> {
    let s = super::settings(state).await?;
    Ok(match s.lock_date {
        Some(l) if orig <= l => Utc::now().date_naive().max(l + chrono::Duration::days(1)),
        _ => orig,
    })
}

// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ListQuery {
    /// unposted | open | overdue | paid | void (default: everything not cancelled)
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
    type R = (Uuid, i64, String, Option<String>, Option<Uuid>, Option<String>, Option<String>, Decimal, String, Option<NaiveDate>, Option<NaiveDate>, Option<Decimal>, Option<String>, Decimal, Option<String>);
    let rows: Vec<R> = sqlx::query_as(
        "WITH x AS (
            SELECT i.id, i.number, i.title, i.currency, COALESCE(a.party_id, i.company_id, (SELECT c.contact_id FROM crm_item_contacts c WHERE c.item_id = i.id ORDER BY c.is_primary DESC, c.sort LIMIT 1)) AS party,
                   i.opportunity, i.stage_semantics, a.date, COALESCE(a.due_date, i.close_date) AS due, a.total - a.credited AS total, a.status, s.name AS stage,
                   COALESCE((SELECT SUM(al.amount) FROM acc_allocations al JOIN acc_payments p ON p.id = al.payment_id WHERE al.doc_kind = 'invoice' AND al.doc_id = i.id AND p.status = 'posted'), 0) AS paid
            FROM crm_items i LEFT JOIN acc_invoices a ON a.item_id = i.id LEFT JOIN crm_stages s ON s.id = i.stage_id
            WHERE i.entity_type = 6 AND i.deleted_at IS NULL)
         SELECT x.id, x.number, x.title, x.currency, x.party, p.title, x.stage, x.opportunity, x.stage_semantics, x.date, x.due, x.total, x.status, x.paid, NULL::text
         FROM x LEFT JOIN crm_items p ON p.id = x.party
         WHERE CASE $1
             WHEN 'unposted' THEN (x.status IS NULL OR x.status = 'void') AND x.stage_semantics <> 'failure'
             WHEN 'open' THEN x.status = 'open'
             WHEN 'overdue' THEN x.status = 'open' AND x.due < current_date
             WHEN 'paid' THEN x.status IN ('paid', 'credited')
             WHEN 'void' THEN x.status = 'void'
             ELSE x.stage_semantics <> 'failure' OR x.status IS NOT NULL END
           AND ($2::uuid IS NULL OR x.party = $2)
           AND ($3::text IS NULL OR lower(x.title) LIKE $3 OR x.number::text = trim(both '%' from $3) OR lower(COALESCE(p.title, '')) LIKE $3)
         ORDER BY x.date DESC NULLS FIRST, x.number DESC LIMIT $4 OFFSET $5",
    )
    .bind(q.status.as_deref().unwrap_or("all"))
    .bind(q.party_id)
    .bind(term)
    .bind(limit + 1)
    .bind(q.offset.unwrap_or(0).max(0))
    .fetch_all(&state.db)
    .await?;
    let base = super::base_currency(&state).await?;
    let today = Utc::now().date_naive();
    let more = rows.len() as i64 > limit;
    Ok(Json(json!({
        "items": rows.into_iter().take(limit as usize).map(|r| {
            let total = r.11;
            let balance = total.map(|t| t - r.13);
            json!({
                "item_id": r.0, "number": r.1, "title": r.2, "currency": r.3.clone().unwrap_or_else(|| base.clone()), "party_id": r.4, "party_name": r.5,
                "stage": r.6, "amount": r.7, "date": r.9, "due_date": r.10, "total": total, "paid": r.13, "balance": balance,
                "status": r.12.clone().unwrap_or_else(|| "unposted".into()),
                "overdue_days": match (r.12.as_deref(), r.10) { (Some("open"), Some(d)) if d < today => Some((today - d).num_days()), _ => None },
            })
        }).collect::<Vec<_>>(),
        "more": more,
    })))
}

async fn detail(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    Ok(Json(invoice_json(&state, item).await?))
}

pub async fn invoice_json(state: &AppState, item: Uuid) -> AppResult<Value> {
    type A = (Uuid, NaiveDate, Option<NaiveDate>, Option<Uuid>, Option<String>, String, Decimal, Decimal, String, Option<i64>, Decimal);
    let a: Option<A> = sqlx::query!(r#"SELECT a.entry_id AS "entry_id!", a.date AS "date!", a.due_date AS "due_date?", a.party_id AS "party_id?", p.title AS "title?", a.currency AS "currency!", a.fx_rate AS "fx_rate!", a.total AS "total!", a.status AS "status!", e.number AS "number?", a.credited AS "credited!"
         FROM acc_invoices a LEFT JOIN crm_items p ON p.id = a.party_id LEFT JOIN acc_entries e ON e.id = a.entry_id WHERE a.item_id = $1"#, item).map(|r| (r.entry_id, r.date, r.due_date, r.party_id, r.title, r.currency, r.fx_rate, r.total, r.status, r.number, r.credited))
    .fetch_optional(&state.db)
    .await?;
    let payments: Vec<(Uuid, i64, NaiveDate, Decimal, String, String)> = sqlx::query!(r#"SELECT p.id, p.number, p.date, al.amount, p.status, ac.name FROM acc_allocations al JOIN acc_payments p ON p.id = al.payment_id
         JOIN acc_accounts ac ON ac.id = p.account_id WHERE al.doc_kind = 'invoice' AND al.doc_id = $1 ORDER BY p.date, p.number"#, item).map(|r| (r.id, r.number, r.date, r.amount, r.status, r.name))
    .fetch_all(&state.db)
    .await?;
    let paid = paid(&state.db, "invoice", item).await?;
    let credits: Vec<(Uuid, i64, NaiveDate, Decimal, Option<String>, String)> = sqlx::query!(r#"SELECT id, number, date, amount, reason, status FROM acc_credit_notes WHERE invoice_id = $1 ORDER BY number"#, item).map(|r| (r.id, r.number, r.date, r.amount, r.reason, r.status))
    .fetch_all(&state.db)
    .await?;
    Ok(match a {
        None => json!({ "item_id": item, "status": "unposted", "payments": [] }),
        Some(a) => json!({
            "item_id": item, "entry_id": a.0, "date": a.1, "due_date": a.2, "party_id": a.3, "party_name": a.4, "currency": a.5, "fx_rate": a.6,
            "total": a.7, "status": a.8, "entry_number": a.9, "paid": paid, "credited": a.10,
            "balance": if a.8 == "void" { Decimal::ZERO } else { a.7 - a.10 - paid },
            "credit_notes": credits.into_iter().map(|(id, n, d, amt, r, st)| json!({ "id": id, "number": n, "date": d, "amount": amt, "reason": r, "status": st })).collect::<Vec<_>>(),
            "payments": payments.into_iter().map(|(id, n, d, amt, st, acc)| json!({ "id": id, "number": n, "date": d, "amount": amt, "status": st, "account": acc })).collect::<Vec<_>>(),
        }),
    })
}

#[derive(Deserialize, Default)]
struct PostBody {
    /// The accounting date (default today).
    date: Option<NaiveDate>,
    /// Units of the base currency per unit of the invoice's (default: the CRM's rate).
    fx_rate: Option<Decimal>,
}

async fn post_invoice(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>, body: Option<Json<PostBody>>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let b = body.map(|b| b.0).unwrap_or_default();
    post_one(&state, &auth, item, b.date.unwrap_or_else(|| Utc::now().date_naive()), b.fx_rate).await?;
    Ok(Json(invoice_json(&state, item).await?))
}

pub async fn post_one(state: &AppState, auth: &AuthUser, item: Uuid, date: NaiveDate, given_fx: Option<Decimal>) -> AppResult<()> {
    let s = ready(state).await?;
    let ar = default_account(s.receivable_account, "receivables")?;
    let sales = default_account(s.sales_account, "sales")?;
    let row = crate::crm::items::load_row(state, item, false).await?;
    if row.entity_type != INVOICE_TYPE {
        return Err(AppError::NotFound("invoice"));
    }
    if row.stage_semantics == "failure" {
        return Err(AppError::bad("a cancelled invoice can't be posted"));
    }
    let party: Option<Uuid> = match row.company_id {
        Some(c) => Some(c),
        None => sqlx::query_scalar!("SELECT contact_id FROM crm_item_contacts WHERE item_id = $1 ORDER BY is_primary DESC, sort LIMIT 1", item).fetch_optional(&state.db).await?,
    };
    let party = party.ok_or_else(|| AppError::bad("give the invoice a client (company or contact) first"))?;
    let base = super::base_currency(state).await?;
    let currency = row.currency.clone().unwrap_or_else(|| base.clone());
    let fx = fx_rate(state, &currency, given_fx).await?;

    // What is sold, by income account, and the tax, by rate: in the invoice's currency.
    let rows = crate::crm::money::rows_of(state, item).await?;
    let mut income: BTreeMap<Uuid, Decimal> = BTreeMap::new();
    let mut taxes: BTreeMap<String, (Decimal, Decimal)> = BTreeMap::new(); // rate -> (net, tax)
    let total: Decimal;
    if rows.is_empty() {
        total = ledger::round(row.opportunity);
        *income.entry(sales).or_default() += total;
    } else {
        let mut t = Decimal::ZERO;
        for r in &rows {
            let l = crate::crm::money::line(r);
            let account: Option<Uuid> = match r.product_id {
                Some(p) => sqlx::query_scalar!("SELECT income_account_id FROM acc_product_accounts WHERE product_id = $1", p).fetch_optional(&state.db).await?.flatten(),
                None => None,
            };
            *income.entry(account.unwrap_or(sales)).or_default() += l.net;
            if let Some(rate) = r.tax_rate.filter(|r| !r.is_zero()) {
                let e = taxes.entry(rate.normalize().to_string()).or_default();
                e.0 += l.net;
                e.1 += l.tax;
            }
            t += l.total;
        }
        total = t;
        if row.is_manual_amount && ledger::round(row.opportunity) != total {
            return Err(AppError::bad(format!(
                "the invoice's amount ({}) differs from its products ({total}); make them agree in the CRM first",
                ledger::round(row.opportunity)
            )));
        }
    }
    if total <= Decimal::ZERO {
        return Err(AppError::bad("the invoice's total must be more than zero"));
    }

    let mut lines = vec![ledger::dr(ar, total * fx).party(Some(party)).foreign(&currency, &base, total).text(format!("Invoice {}", row.number))];
    for (account, net) in &income {
        lines.push(ledger::cr(*account, net * fx).foreign(&currency, &base, -net).party(Some(party)));
    }
    for (rate, (net, tax)) in &taxes {
        let tax_row: Option<(Uuid, Uuid)> = sqlx::query_as("SELECT id, account_id FROM acc_taxes WHERE scope = 'sales' AND is_active AND rate = $1::numeric ORDER BY created_at LIMIT 1")
            .bind(rate)
            .fetch_optional(&state.db)
            .await?;
        let (tax_id, account) = tax_row.ok_or_else(|| AppError::bad(format!("there is no sales tax at {rate}%; add one in Accounting → Settings → Taxes")))?;
        lines.push(ledger::cr(account, tax * fx).foreign(&currency, &base, -*tax).taxed(tax_id, net * fx).party(Some(party)));
    }
    // Rounding after conversion: whatever is left goes to the rounding account.
    let diff: Decimal = lines.iter().map(|l| l.debit - l.credit).sum();
    if !diff.is_zero() {
        let rounding = default_account(s.rounding_account, "rounding")?;
        lines.push(ledger::signed(rounding, -diff).text("Rounding"));
    }

    let mut tx = state.db.begin().await?;
    let existing: Option<String> = sqlx::query_scalar!("SELECT status FROM acc_invoices WHERE item_id = $1 FOR UPDATE", item).fetch_optional(&mut *tx).await?;
    if existing.as_deref().is_some_and(|s| s != "void") {
        return Err(AppError::Conflict("this invoice is already in the books".into()));
    }
    let memo = format!("Invoice {} · {}", row.number, row.title);
    let entry = ledger::post(&mut tx, date, Some(&memo), "invoice", Some(item), &lines, Some(auth.id)).await?;
    sqlx::query!("INSERT INTO acc_invoices (item_id, entry_id, date, due_date, party_id, currency, fx_rate, total, status, posted_by, posted_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'open', $9, now())
         ON CONFLICT (item_id) DO UPDATE SET entry_id = EXCLUDED.entry_id, date = EXCLUDED.date, due_date = EXCLUDED.due_date, party_id = EXCLUDED.party_id,
             currency = EXCLUDED.currency, fx_rate = EXCLUDED.fx_rate, total = EXCLUDED.total, status = 'open', posted_by = EXCLUDED.posted_by, posted_at = now()", item, entry, date, row.close_date, party, &currency, fx, total, auth.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let _ = crate::crm::timeline(&state.db, item, "accounting", Some(auth.id), None, json!({ "event": "posted", "total": total, "currency": currency })).await;
    audit::record(&state.db, Some(auth.id), "accounting.invoice_post", "crm_item", Some(item.to_string()), json!({ "total": total, "currency": currency }), None).await;
    Ok(())
}

#[derive(Deserialize, Default)]
struct VoidBody {
    date: Option<NaiveDate>,
}

async fn void_invoice(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>, body: Option<Json<VoidBody>>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let mut tx = state.db.begin().await?;
    let row: Option<(Uuid, String, NaiveDate)> = sqlx::query!(r#"SELECT entry_id, status, date FROM acc_invoices WHERE item_id = $1 FOR UPDATE"#, item).map(|r| (r.entry_id, r.status, r.date)).fetch_optional(&mut *tx).await?;
    let (entry, status, date) = row.ok_or(AppError::NotFound("invoice"))?;
    if status == "void" {
        return Err(AppError::bad("this invoice is already void"));
    }
    if !paid(&mut *tx, "invoice", item).await?.is_zero() {
        return Err(AppError::bad("payments are allocated to this invoice; void those payments first"));
    }
    let credited: bool = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM acc_credit_notes WHERE invoice_id = $1 AND status = 'posted') AS "v!""#, item).fetch_one(&mut *tx).await?;
    if credited {
        return Err(AppError::bad("credit notes are posted against this invoice; void them first"));
    }
    let when = match body.and_then(|b| b.0.date) {
        Some(d) => d,
        None => void_date(&state, date).await?,
    };
    ledger::reverse(&mut tx, entry, when, Some(auth.id)).await?;
    sqlx::query!("UPDATE acc_invoices SET status = 'void' WHERE item_id = $1", item).execute(&mut *tx).await?;
    tx.commit().await?;
    let _ = crate::crm::timeline(&state.db, item, "accounting", Some(auth.id), None, json!({ "event": "voided" })).await;
    audit::record(&state.db, Some(auth.id), "accounting.invoice_void", "crm_item", Some(item.to_string()), json!({}), None).await;
    crate::commissions::sync_invoice(&state, item).await;
    Ok(Json(invoice_json(&state, item).await?))
}
