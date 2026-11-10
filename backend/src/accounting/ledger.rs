//! The posting engine: every journal entry goes through here, so the rules hold everywhere.
//!
//! - The books must be set up, and the date must be after the lock date.
//! - Each line is a debit or a credit (2 decimals, not negative); accounts exist and are active.
//! - Debits equal credits, and the entry moves some money.
//! - Numbers come from a counter locked for the transaction, so they have no gaps.
//! - Posted entries never change: a reversal posts the same lines the other way round.

use chrono::NaiveDate;
use rust_decimal::{Decimal, RoundingStrategy};
use serde::Serialize;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub type Tx<'a> = Transaction<'a, Postgres>;

pub fn round(d: Decimal) -> Decimal {
    d.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

/// One line to post. Use `dr` / `cr` to build them.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Line {
    pub account: Uuid,
    pub debit: Decimal,
    pub credit: Decimal,
    pub currency: Option<String>,
    pub amount_currency: Option<Decimal>,
    pub description: Option<String>,
    pub party: Option<Uuid>,
    pub tax: Option<Uuid>,
    pub tax_base: Option<Decimal>,
}

/// A line moving `amount` (base currency) to the debit side, or the credit side if negative.
pub fn signed(account: Uuid, amount: Decimal) -> Line {
    let a = round(amount);
    if a >= Decimal::ZERO {
        Line { account, debit: a, ..Default::default() }
    } else {
        Line { account, credit: -a, ..Default::default() }
    }
}
pub fn dr(account: Uuid, amount: Decimal) -> Line {
    Line { account, debit: round(amount), ..Default::default() }
}
pub fn cr(account: Uuid, amount: Decimal) -> Line {
    Line { account, credit: round(amount), ..Default::default() }
}

impl Line {
    pub fn party(mut self, p: Option<Uuid>) -> Self {
        self.party = p;
        self
    }
    pub fn text(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }
    /// The amount in a foreign currency (signed: debit positive), when it isn't the base one.
    pub fn foreign(mut self, currency: &str, base: &str, amount: Decimal) -> Self {
        if currency != base {
            self.currency = Some(currency.to_string());
            self.amount_currency = Some(round(amount));
        }
        self
    }
    pub fn taxed(mut self, tax: Uuid, base_amount: Decimal) -> Self {
        self.tax = Some(tax);
        self.tax_base = Some(round(base_amount));
        self
    }
}

/// The lock date: nothing on or before it may be posted, changed or reversed.
pub async fn check_open(tx: &mut Tx<'_>, date: NaiveDate) -> AppResult<()> {
    let (ready, lock): (bool, Option<NaiveDate>) = sqlx::query_as("SELECT ready, lock_date FROM acc_settings WHERE id = 1").fetch_one(&mut **tx).await?;
    if !ready {
        return Err(AppError::bad("set up accounting first"));
    }
    if let Some(l) = lock {
        if date <= l {
            return Err(AppError::bad(format!("the books are locked up to {l}; choose a later date or ask an administrator to move the lock date")));
        }
    }
    Ok(())
}

/// The next number of a kind (entry, bill, payment), without gaps: the counter row stays
/// locked until the transaction ends, and a rolled-back transaction gives its number back.
pub async fn next_number(tx: &mut Tx<'_>, kind: &str) -> AppResult<i64> {
    let n: i64 = sqlx::query_scalar("UPDATE acc_counters SET next = next + 1 WHERE kind = $1 RETURNING next - 1").bind(kind).fetch_one(&mut **tx).await?;
    Ok(n)
}

/// Checks the lines of an entry (without posting it).
pub async fn validate(tx: &mut Tx<'_>, lines: &[Line]) -> AppResult<()> {
    let live: Vec<&Line> = lines.iter().filter(|l| !l.debit.is_zero() || !l.credit.is_zero()).collect();
    if live.len() < 2 {
        return Err(AppError::bad("an entry needs at least two lines with amounts"));
    }
    let (mut d, mut c) = (Decimal::ZERO, Decimal::ZERO);
    for l in &live {
        if l.debit < Decimal::ZERO || l.credit < Decimal::ZERO {
            return Err(AppError::bad("amounts can't be negative; use the other column"));
        }
        if !l.debit.is_zero() && !l.credit.is_zero() {
            return Err(AppError::bad("a line is either a debit or a credit"));
        }
        if l.debit != round(l.debit) || l.credit != round(l.credit) {
            return Err(AppError::bad("amounts have at most two decimals"));
        }
        d += l.debit;
        c += l.credit;
    }
    if d != c {
        return Err(AppError::bad(format!("debits ({d}) and credits ({c}) must be equal")));
    }
    if d > Decimal::from(1_000_000_000_000_000i64) {
        return Err(AppError::bad("the amounts are too large"));
    }
    let ids: Vec<Uuid> = live.iter().map(|l| l.account).collect();
    let ok: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT id) FROM acc_accounts WHERE id = ANY($1) AND is_active").bind(&ids).fetch_one(&mut **tx).await?;
    let distinct = ids.iter().collect::<std::collections::HashSet<_>>().len();
    if ok as usize != distinct {
        return Err(AppError::bad("an account on the entry doesn't exist or is inactive"));
    }
    Ok(())
}

/// Posts an entry; returns its id.
#[allow(clippy::too_many_arguments)]
pub async fn post(tx: &mut Tx<'_>, date: NaiveDate, memo: Option<&str>, source: &str, source_id: Option<Uuid>, lines: &[Line], user: Option<Uuid>) -> AppResult<Uuid> {
    check_open(tx, date).await?;
    validate(tx, lines).await?;
    let number = next_number(tx, "entry").await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO acc_entries (number, date, memo, source, source_id, status, created_by, posted_at) VALUES ($1, $2, $3, $4, $5, 'posted', $6, now()) RETURNING id",
    )
    .bind(number)
    .bind(date)
    .bind(memo)
    .bind(source)
    .bind(source_id)
    .bind(user)
    .fetch_one(&mut **tx)
    .await?;
    insert_lines(tx, id, date, lines).await?;
    Ok(id)
}

pub async fn insert_lines(tx: &mut Tx<'_>, entry: Uuid, date: NaiveDate, lines: &[Line]) -> AppResult<()> {
    sqlx::query("DELETE FROM acc_lines WHERE entry_id = $1").bind(entry).execute(&mut **tx).await?;
    for (i, l) in lines.iter().filter(|l| !l.debit.is_zero() || !l.credit.is_zero()).enumerate() {
        sqlx::query(
            "INSERT INTO acc_lines (entry_id, account_id, date, debit, credit, currency, amount_currency, description, party_id, tax_id, tax_base, sort)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(entry)
        .bind(l.account)
        .bind(date)
        .bind(l.debit)
        .bind(l.credit)
        .bind(&l.currency)
        .bind(l.amount_currency)
        .bind(&l.description)
        .bind(l.party)
        .bind(l.tax)
        .bind(l.tax_base)
        .bind(i as i32)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Reverses a posted entry: a new entry with every line the other way round, dated `date`
/// (after the lock date, and not before the original). The original is marked void and its
/// lines stay as they were. Returns the reversal's id.
pub async fn reverse(tx: &mut Tx<'_>, entry: Uuid, date: NaiveDate, user: Option<Uuid>) -> AppResult<Uuid> {
    let row: Option<(String, NaiveDate, Option<i64>, Option<String>, String, Option<Uuid>)> =
        sqlx::query_as("SELECT status, date, number, memo, source, source_id FROM acc_entries WHERE id = $1 FOR UPDATE")
            .bind(entry)
            .fetch_optional(&mut **tx)
            .await?;
    let (status, orig_date, number, _memo, source, source_id) = row.ok_or(AppError::NotFound("entry"))?;
    if status != "posted" {
        return Err(AppError::bad("only posted entries can be reversed, and only once"));
    }
    // The original may be in a locked period: the reversal itself must be in an open one.
    if date < orig_date {
        return Err(AppError::bad("a reversal can't be dated before the entry it reverses"));
    }
    type L = (Uuid, Decimal, Decimal, Option<String>, Option<Decimal>, Option<String>, Option<Uuid>, Option<Uuid>, Option<Decimal>);
    let lines: Vec<L> = sqlx::query_as(
        "SELECT account_id, debit, credit, currency, amount_currency, description, party_id, tax_id, tax_base FROM acc_lines WHERE entry_id = $1 ORDER BY sort",
    )
    .bind(entry)
    .fetch_all(&mut **tx)
    .await?;
    let back: Vec<Line> = lines
        .into_iter()
        .map(|(account, d, c, cur, amt, desc, party, tax, base)| Line {
            account,
            debit: c,
            credit: d,
            currency: cur,
            amount_currency: amt.map(|a| -a),
            description: desc,
            party,
            tax,
            tax_base: base.map(|b| -b),
        })
        .collect();
    // Accounts made inactive since are fine for a reversal.
    check_open(tx, date).await?;
    let n = next_number(tx, "entry").await?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO acc_entries (number, date, memo, source, source_id, status, reverses, created_by, posted_at) VALUES ($1, $2, $3, $4, $5, 'posted', $6, $7, now()) RETURNING id",
    )
    .bind(n)
    .bind(date)
    .bind(format!("Reversal of entry {}", number.unwrap_or_default()))
    .bind(source)
    .bind(source_id)
    .bind(entry)
    .bind(user)
    .fetch_one(&mut **tx)
    .await?;
    insert_lines(tx, id, date, &back).await?;
    sqlx::query("UPDATE acc_entries SET status = 'void', reversed_by = $2 WHERE id = $1").bind(entry).bind(id).execute(&mut **tx).await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn rounding_is_half_away_from_zero() {
        assert_eq!(round(Decimal::from_str("2.345").unwrap()), Decimal::from_str("2.35").unwrap());
        assert_eq!(round(Decimal::from_str("-2.345").unwrap()), Decimal::from_str("-2.35").unwrap());
        assert_eq!(round(Decimal::from_str("2.344").unwrap()), Decimal::from_str("2.34").unwrap());
    }

    #[test]
    fn signed_lines_pick_the_side() {
        let a = Uuid::nil();
        let l = signed(a, Decimal::from_str("-10.004").unwrap());
        assert_eq!((l.debit, l.credit), (Decimal::ZERO, Decimal::from_str("10.00").unwrap()));
        let l = signed(a, Decimal::from_str("3.5").unwrap());
        assert_eq!((l.debit, l.credit), (Decimal::from_str("3.50").unwrap(), Decimal::ZERO));
    }
}
