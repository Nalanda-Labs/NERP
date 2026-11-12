//! Credit notes: part or all of an invoice in the books taken back (a return, a discount given
//! afterwards, a billing mistake). Each is a ledger entry reversing the invoice's own lines in
//! proportion — receivable, sales by account and each tax — and lowers what the invoice is
//! owed; once payments and credit notes cover it, the invoice is settled. A credit note on a
//! paid invoice leaves the client in credit (to refund or to use later). Voiding one reverses
//! its entry. Commissions follow (see src/commissions.rs).
use axum::{
    extract::{Path, State},
    routing::post,
    Json, Router,
};
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{ledger, need, Level};
use crate::{
    audit,
    auth::AuthUser,
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/accounting/invoices/{item}/credit-notes", post(create))
        .route("/accounting/credit-notes/{id}/void", post(void))
}

#[derive(Deserialize)]
struct CreateInput {
    amount: Decimal,
    date: Option<NaiveDate>,
    reason: Option<String>,
}

type L = (Uuid, Decimal, Decimal, Option<String>, Option<Decimal>, Option<Uuid>, Option<Uuid>, Option<Decimal>);

async fn create(State(state): State<AppState>, auth: AuthUser, Path(item): Path<Uuid>, Json(b): Json<CreateInput>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let amount = ledger::round(b.amount);
    if amount <= Decimal::ZERO {
        return Err(AppError::bad("the amount must be more than zero"));
    }
    if b.reason.as_deref().is_some_and(|r| r.len() > 500) {
        return Err(AppError::bad("the reason is too long"));
    }
    let date = b.date.unwrap_or_else(|| Utc::now().date_naive());
    let s = super::settings(&state).await?;
    let mut tx = state.db.begin().await?;
    let row: Option<(Uuid, Decimal, Decimal, String, String, Option<Uuid>, i64)> = sqlx::query_as(
        "SELECT a.entry_id, a.total, a.credited, a.status, a.currency, a.party_id, i.number FROM acc_invoices a JOIN crm_items i ON i.id = a.item_id WHERE a.item_id = $1 FOR UPDATE OF a",
    )
    .bind(item)
    .fetch_optional(&mut *tx)
    .await?;
    let (entry, total, credited, status, currency, party, number) = row.ok_or(AppError::NotFound("invoice in the books"))?;
    if status == "void" {
        return Err(AppError::bad("this invoice is void"));
    }
    if amount > total - credited {
        return Err(AppError::bad(format!("at most {} is left to credit on this invoice", (total - credited).normalize())));
    }
    let base = super::base_currency(&state).await?;
    // The invoice's lines, reversed and scaled to the credited share.
    let lines: Vec<L> = sqlx::query_as("SELECT account_id, debit, credit, currency, amount_currency, party_id, tax_id, tax_base FROM acc_lines WHERE entry_id = $1 ORDER BY sort")
        .bind(entry)
        .fetch_all(&mut *tx)
        .await?;
    let share = amount / total;
    let mut out: Vec<ledger::Line> = lines
        .into_iter()
        .map(|(account, debit, credit, cur, amt, p, tax, tax_base)| {
            let mut l = ledger::signed(account, (credit - debit) * share).party(p);
            if let (Some(c), Some(a)) = (cur.as_deref(), amt) {
                l = l.foreign(c, &base, -a * share);
            }
            if let Some(t) = tax {
                l = l.taxed(t, -tax_base.unwrap_or_default() * share);
            }
            l
        })
        .collect();
    out.retain(|l| !l.debit.is_zero() || !l.credit.is_zero());
    let diff: Decimal = out.iter().map(|l| l.debit - l.credit).sum();
    if !diff.is_zero() {
        let rounding = s.rounding_account.ok_or_else(|| AppError::bad("set the rounding account in Accounting → Settings"))?;
        out.push(ledger::signed(rounding, -diff).text("Rounding"));
    }
    let n = ledger::next_number(&mut tx, "credit_note").await?;
    let memo = format!("Credit note {n} for invoice {number}{}", b.reason.as_deref().map(|r| format!(" · {r}")).unwrap_or_default());
    let entry_id = ledger::post(&mut tx, date, Some(&memo), "credit_note", Some(item), &out, Some(auth.id)).await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO acc_credit_notes (number, invoice_id, date, amount, reason, entry_id, created_by) VALUES ($1, $2, $3, $4, NULLIF(trim($5), ''), $6, $7) RETURNING id")
        .bind(n)
        .bind(item)
        .bind(date)
        .bind(amount)
        .bind(&b.reason)
        .bind(entry_id)
        .bind(auth.id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("UPDATE acc_invoices SET credited = credited + $2 WHERE item_id = $1").bind(item).bind(amount).execute(&mut *tx).await?;
    super::invoices::refresh(&mut tx, item).await?;
    tx.commit().await?;
    let _ = crate::crm::timeline(&state.db, item, "accounting", Some(auth.id), None, json!({ "event": "credited", "amount": amount, "currency": currency, "number": n })).await;
    audit::record(&state.db, Some(auth.id), "accounting.credit_note", "crm_item", Some(item.to_string()), json!({ "amount": amount, "number": n, "party": party }), None).await;
    crate::commissions::sync_invoice(&state, item).await;
    Ok(Json(json!({ "id": id, "number": n, "invoice": super::invoices::invoice_json(&state, item).await? })))
}

#[derive(Deserialize, Default)]
struct VoidInput {
    date: Option<NaiveDate>,
}

async fn void(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, body: Option<Json<VoidInput>>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let mut tx = state.db.begin().await?;
    let row: Option<(Uuid, Uuid, Decimal, String, NaiveDate, i64)> = sqlx::query_as("SELECT invoice_id, entry_id, amount, status, date, number FROM acc_credit_notes WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let (item, entry, amount, status, date, n) = row.ok_or(AppError::NotFound("credit note"))?;
    if status == "void" {
        return Err(AppError::bad("this credit note is already void"));
    }
    sqlx::query("SELECT 1 FROM acc_invoices WHERE item_id = $1 FOR UPDATE").bind(item).execute(&mut *tx).await?;
    let when = match body.and_then(|b| b.0.date) {
        Some(d) => d,
        None => super::invoices::void_date(&state, date).await?,
    };
    ledger::reverse(&mut tx, entry, when, Some(auth.id)).await?;
    sqlx::query("UPDATE acc_credit_notes SET status = 'void' WHERE id = $1").bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE acc_invoices SET credited = credited - $2 WHERE item_id = $1").bind(item).bind(amount).execute(&mut *tx).await?;
    super::invoices::refresh(&mut tx, item).await?;
    tx.commit().await?;
    let _ = crate::crm::timeline(&state.db, item, "accounting", Some(auth.id), None, json!({ "event": "credit_voided", "number": n })).await;
    audit::record(&state.db, Some(auth.id), "accounting.credit_note_void", "crm_item", Some(item.to_string()), json!({ "number": n }), None).await;
    crate::commissions::sync_invoice(&state, item).await;
    Ok(Json(super::invoices::invoice_json(&state, item).await?))
}
