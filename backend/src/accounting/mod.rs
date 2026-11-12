//! Accounting: double-entry books for the company (country-neutral).
//!
//! - Setup makes a starter chart of accounts (editable) in the CRM's base currency.
//! - The journal: manual entries (drafts, then posted), and entries posted by documents:
//!   CRM invoices (`invoices`), purchase bills (`bills`), payments (`payments`) and bank
//!   statement lines (`bank`). Every entry goes through `ledger::post`.
//! - Reports (`reports`): trial balance, profit and loss, balance sheet, ledgers, ageing, tax,
//!   customer statements; year-end closing.
//!
//! Permissions: accounting.view (read the books), accounting.post (record documents and
//! entries), accounting.admin (settings, chart of accounts, taxes, lock date, closing, voids).

pub mod bank;
pub mod bills;
pub mod credit_notes;
pub mod forecast;
pub mod invoices;
pub mod ledger;
pub mod payments;
pub mod reports;

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use uuid::Uuid;

use ledger::Line;

use crate::{
    audit,
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    routes::{clean, required},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/accounting/settings", get(get_settings).put(put_settings))
        .route("/accounting/setup", post(setup))
        .route("/accounting/accounts", get(list_accounts).post(create_account))
        .route("/accounting/accounts/{id}", axum::routing::patch(update_account).delete(delete_account))
        .route("/accounting/taxes", get(list_taxes).post(create_tax))
        .route("/accounting/taxes/{id}", axum::routing::patch(update_tax))
        .route("/accounting/entries", get(list_entries).post(create_entry))
        .route("/accounting/entries/{id}", get(get_entry).put(update_entry).delete(delete_entry))
        .route("/accounting/entries/{id}/post", post(post_entry))
        .route("/accounting/entries/{id}/reverse", post(reverse_entry))
        .route("/accounting/opening", post(opening_balances))
        .route("/accounting/parties", get(parties))
        .merge(invoices::router())
        .merge(credit_notes::router())
        .merge(bills::router())
        .merge(payments::router())
        .merge(bank::router())
        .merge(reports::router())
        .merge(forecast::router())
}

// ---------------------------------------------------------------------------
// Access
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub enum Level {
    View,
    Post,
    Admin,
}

pub fn need(auth: &AuthUser, level: Level) -> AppResult<()> {
    let ok = match level {
        Level::View => auth.can(perm::ACCOUNTING_VIEW) || auth.can(perm::ACCOUNTING_POST) || auth.can(perm::ACCOUNTING_ADMIN),
        Level::Post => auth.can(perm::ACCOUNTING_POST) || auth.can(perm::ACCOUNTING_ADMIN),
        Level::Admin => auth.can(perm::ACCOUNTING_ADMIN),
    };
    if ok { Ok(()) } else { Err(AppError::Forbidden) }
}

/// Files on bills are readable by whoever can read the books.
pub async fn can_view_file(state: &AppState, auth: &AuthUser, file: Uuid) -> AppResult<bool> {
    if need(auth, Level::View).is_err() {
        return Ok(false);
    }
    Ok(sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM acc_bills WHERE $1 = ANY(file_ids))").bind(file).fetch_one(&state.db).await?)
}

// ---------------------------------------------------------------------------
// Settings and setup
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow, Clone)]
pub struct Settings {
    pub ready: bool,
    pub fiscal_year_start: i16,
    pub lock_date: Option<NaiveDate>,
    pub receivable_account: Option<Uuid>,
    pub payable_account: Option<Uuid>,
    pub sales_account: Option<Uuid>,
    pub retained_earnings: Option<Uuid>,
    pub opening_balances: Option<Uuid>,
    pub fx_account: Option<Uuid>,
    pub rounding_account: Option<Uuid>,
    pub mark_invoices_paid: bool,
}

pub async fn settings(state: &AppState) -> AppResult<Settings> {
    Ok(sqlx::query_as(
        "SELECT ready, fiscal_year_start, lock_date, receivable_account, payable_account, sales_account, retained_earnings,
                opening_balances, fx_account, rounding_account, mark_invoices_paid FROM acc_settings WHERE id = 1",
    )
    .fetch_one(&state.db)
    .await?)
}

/// The settings, with the books set up (or a clear error).
pub async fn ready(state: &AppState) -> AppResult<Settings> {
    let s = settings(state).await?;
    if !s.ready {
        return Err(AppError::bad("set up accounting first (Accounting → Settings)"));
    }
    Ok(s)
}

/// A default account that must be set.
pub fn default_account(a: Option<Uuid>, what: &str) -> AppResult<Uuid> {
    a.ok_or_else(|| AppError::bad(format!("choose the {what} account in Accounting → Settings")))
}

pub async fn base_currency(state: &AppState) -> AppResult<String> {
    crate::crm::money::base_currency(state).await
}

/// Units of the base currency per unit of `currency`: given, or the CRM's current rate.
pub async fn fx_rate(state: &AppState, currency: &str, given: Option<Decimal>) -> AppResult<Decimal> {
    let base = base_currency(state).await?;
    if currency == base {
        return Ok(Decimal::ONE);
    }
    if let Some(r) = given {
        if r <= Decimal::ZERO {
            return Err(AppError::bad("the exchange rate must be positive"));
        }
        return Ok(r);
    }
    crate::crm::money::rate_of(state, currency)
        .await?
        .ok_or_else(|| AppError::bad(format!("{currency} is not one of the CRM's currencies; add it or give an exchange rate")))
}

async fn get_settings(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    let s = settings(&state).await?;
    let used: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM acc_entries)").fetch_one(&state.db).await?;
    Ok(Json(json!({ "settings": s, "base_currency": base_currency(&state).await?, "has_entries": used })))
}

#[derive(Deserialize)]
struct SettingsBody {
    fiscal_year_start: Option<i16>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    lock_date: Option<Option<NaiveDate>>,
    receivable_account: Option<Uuid>,
    payable_account: Option<Uuid>,
    sales_account: Option<Uuid>,
    retained_earnings: Option<Uuid>,
    opening_balances: Option<Uuid>,
    fx_account: Option<Uuid>,
    rounding_account: Option<Uuid>,
    mark_invoices_paid: Option<bool>,
}

async fn put_settings(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SettingsBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Admin)?;
    ready(&state).await?;
    if let Some(m) = b.fiscal_year_start {
        if !(1..=12).contains(&m) {
            return Err(AppError::bad("the financial year starts in a month from 1 to 12"));
        }
    }
    // Each default account must be of the right kind.
    let checks: [(&Option<Uuid>, &[&str], &str); 7] = [
        (&b.receivable_account, &["receivable"], "receivables"),
        (&b.payable_account, &["payable"], "payables"),
        (&b.sales_account, &["income", "other_income"], "sales"),
        (&b.retained_earnings, &["retained_earnings", "equity"], "retained earnings"),
        (&b.opening_balances, &["equity", "retained_earnings"], "opening balances"),
        (&b.fx_account, &["other_income", "other_expense", "expense", "income"], "exchange differences"),
        (&b.rounding_account, &["other_income", "other_expense", "expense", "income"], "rounding"),
    ];
    for (a, kinds, what) in checks {
        if let Some(id) = a {
            let sub: Option<String> = sqlx::query_scalar("SELECT subtype FROM acc_accounts WHERE id = $1 AND is_active").bind(id).fetch_optional(&state.db).await?;
            if !sub.is_some_and(|s| kinds.contains(&s.as_str())) {
                return Err(AppError::bad(format!("the {what} account must be a {} account", kinds.join(" or "))));
            }
        }
    }
    let lock_change = b.lock_date.is_some();
    sqlx::query(
        "UPDATE acc_settings SET
            fiscal_year_start = COALESCE($1, fiscal_year_start),
            lock_date = CASE WHEN $2 THEN $3 ELSE lock_date END,
            receivable_account = COALESCE($4, receivable_account), payable_account = COALESCE($5, payable_account),
            sales_account = COALESCE($6, sales_account), retained_earnings = COALESCE($7, retained_earnings),
            opening_balances = COALESCE($8, opening_balances), fx_account = COALESCE($9, fx_account),
            rounding_account = COALESCE($10, rounding_account), mark_invoices_paid = COALESCE($11, mark_invoices_paid)
         WHERE id = 1",
    )
    .bind(b.fiscal_year_start)
    .bind(lock_change)
    .bind(b.lock_date.flatten())
    .bind(b.receivable_account)
    .bind(b.payable_account)
    .bind(b.sales_account)
    .bind(b.retained_earnings)
    .bind(b.opening_balances)
    .bind(b.fx_account)
    .bind(b.rounding_account)
    .bind(b.mark_invoices_paid)
    .execute(&state.db)
    .await?;
    if lock_change {
        audit::record(&state.db, Some(auth.id), "accounting.lock_date", "settings", None, json!({ "lock_date": b.lock_date.flatten() }), None).await;
    }
    get_settings(State(state), auth).await
}

/// The starter chart: (code, name, type, subtype). Country-neutral; edit it freely.
const CHART: &[(&str, &str, &str, &str)] = &[
    ("1000", "Cash", "asset", "cash"),
    ("1010", "Bank account", "asset", "bank"),
    ("1100", "Accounts receivable", "asset", "receivable"),
    ("1200", "Inventory", "asset", "inventory"),
    ("1300", "Tax receivable", "asset", "tax_receivable"),
    ("1400", "Prepaid expenses", "asset", "current_asset"),
    ("1500", "Equipment", "asset", "fixed_asset"),
    ("1590", "Accumulated depreciation", "asset", "fixed_asset"),
    ("2000", "Accounts payable", "liability", "payable"),
    ("2100", "Tax payable", "liability", "tax_payable"),
    ("2200", "Accrued liabilities", "liability", "current_liability"),
    ("2300", "Payroll liabilities", "liability", "current_liability"),
    ("2500", "Loans", "liability", "long_term_liability"),
    ("3000", "Owner's capital", "equity", "equity"),
    ("3100", "Retained earnings", "equity", "retained_earnings"),
    ("3200", "Opening balances", "equity", "equity"),
    ("4000", "Sales", "income", "income"),
    ("4100", "Services", "income", "income"),
    ("4900", "Other income", "income", "other_income"),
    ("5000", "Cost of goods sold", "expense", "cost_of_sales"),
    ("6000", "Salaries and wages", "expense", "expense"),
    ("6100", "Rent", "expense", "expense"),
    ("6200", "Utilities", "expense", "expense"),
    ("6300", "Office supplies", "expense", "expense"),
    ("6400", "Travel", "expense", "expense"),
    ("6500", "Marketing and advertising", "expense", "expense"),
    ("6600", "Professional fees", "expense", "expense"),
    ("6700", "Software and subscriptions", "expense", "expense"),
    ("6800", "Bank charges", "expense", "expense"),
    ("6900", "Depreciation", "expense", "expense"),
    ("6990", "Other expenses", "expense", "expense"),
    ("7900", "Exchange rate differences", "expense", "other_expense"),
    ("7910", "Rounding differences", "expense", "other_expense"),
];

#[derive(Deserialize, Default)]
struct SetupBody {
    fiscal_year_start: Option<i16>,
    /// false: start with an empty chart (and choose the default accounts afterwards).
    starter_chart: Option<bool>,
}

async fn setup(State(state): State<AppState>, auth: AuthUser, body: Option<Json<SetupBody>>) -> AppResult<Json<Value>> {
    need(&auth, Level::Admin)?;
    let b = body.map(|b| b.0).unwrap_or_default();
    let mut tx = state.db.begin().await?;
    let done: bool = sqlx::query_scalar("SELECT ready FROM acc_settings WHERE id = 1 FOR UPDATE").fetch_one(&mut *tx).await?;
    if done {
        return Err(AppError::Conflict("accounting is already set up".into()));
    }
    let fy = b.fiscal_year_start.unwrap_or(1);
    if !(1..=12).contains(&fy) {
        return Err(AppError::bad("the financial year starts in a month from 1 to 12"));
    }
    if b.starter_chart.unwrap_or(true) {
        let mut ids = std::collections::HashMap::new();
        for (code, name, kind, sub) in CHART {
            // An account with that code already there (made before setup) is kept.
            let id: Uuid = sqlx::query_scalar(
                "INSERT INTO acc_accounts (code, name, type, subtype) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (code) DO UPDATE SET code = EXCLUDED.code RETURNING id",
            )
            .bind(code)
            .bind(name)
            .bind(kind)
            .bind(sub)
            .fetch_one(&mut *tx)
            .await?;
            ids.insert(*code, id);
        }
        sqlx::query(
            "UPDATE acc_settings SET receivable_account = $1, payable_account = $2, sales_account = $3, retained_earnings = $4,
                opening_balances = $5, fx_account = $6, rounding_account = $7 WHERE id = 1",
        )
        .bind(ids["1100"])
        .bind(ids["2000"])
        .bind(ids["4000"])
        .bind(ids["3100"])
        .bind(ids["3200"])
        .bind(ids["7900"])
        .bind(ids["7910"])
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE acc_settings SET ready = TRUE, fiscal_year_start = $1 WHERE id = 1").bind(fy).execute(&mut *tx).await?;
    audit::record(&mut *tx, Some(auth.id), "accounting.setup", "settings", None, json!({ "fiscal_year_start": fy }), None).await;
    tx.commit().await?;
    get_settings(State(state), auth).await
}

// ---------------------------------------------------------------------------
// Chart of accounts
// ---------------------------------------------------------------------------

pub const SUBTYPES: &[(&str, &str)] = &[
    ("bank", "asset"),
    ("cash", "asset"),
    ("receivable", "asset"),
    ("current_asset", "asset"),
    ("inventory", "asset"),
    ("fixed_asset", "asset"),
    ("tax_receivable", "asset"),
    ("payable", "liability"),
    ("current_liability", "liability"),
    ("tax_payable", "liability"),
    ("long_term_liability", "liability"),
    ("equity", "equity"),
    ("retained_earnings", "equity"),
    ("income", "income"),
    ("other_income", "income"),
    ("cost_of_sales", "expense"),
    ("expense", "expense"),
    ("other_expense", "expense"),
];

#[derive(Serialize, FromRow)]
struct AccountRow {
    id: Uuid,
    code: String,
    name: String,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    kind: String,
    subtype: String,
    parent_id: Option<Uuid>,
    currency: Option<String>,
    description: Option<String>,
    is_active: bool,
    /// Debits minus credits (base currency).
    balance: Decimal,
    used: bool,
}

#[derive(Deserialize)]
struct AccountsQuery {
    /// Balances up to this day (default: everything).
    as_of: Option<NaiveDate>,
    subtype: Option<String>,
    active: Option<bool>,
}

async fn list_accounts(State(state): State<AppState>, auth: AuthUser, Query(q): Query<AccountsQuery>) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    let rows: Vec<AccountRow> = sqlx::query_as(
        "SELECT a.id, a.code, a.name, a.type, a.subtype, a.parent_id, a.currency, a.description, a.is_active,
                COALESCE((SELECT SUM(l.debit - l.credit) FROM acc_lines l JOIN acc_entries e ON e.id = l.entry_id
                          WHERE l.account_id = a.id AND e.status <> 'draft' AND ($1::date IS NULL OR l.date <= $1)), 0) AS balance,
                EXISTS (SELECT 1 FROM acc_lines l WHERE l.account_id = a.id) AS used
         FROM acc_accounts a
         WHERE ($2::text IS NULL OR a.subtype = ANY(string_to_array($2, ','))) AND ($3::boolean IS NULL OR a.is_active = $3)
         ORDER BY a.code",
    )
    .bind(q.as_of)
    .bind(q.subtype)
    .bind(q.active)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows })))
}

#[derive(Deserialize)]
struct AccountBody {
    code: Option<String>,
    name: Option<String>,
    subtype: Option<String>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    parent_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    currency: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::routes::double_option")]
    description: Option<Option<String>>,
    is_active: Option<bool>,
}

fn type_of(subtype: &str) -> AppResult<&'static str> {
    SUBTYPES.iter().find(|(s, _)| *s == subtype).map(|(_, t)| *t).ok_or_else(|| AppError::bad(format!("unknown account kind: {subtype}")))
}

fn clean_code(c: &str) -> AppResult<String> {
    let c = required("code", c, 20)?;
    if !c.chars().all(|ch| ch.is_alphanumeric() || ch == '-' || ch == '.') {
        return Err(AppError::bad("account codes use letters, digits, dots and dashes"));
    }
    Ok(c)
}

async fn check_currency(state: &AppState, c: &Option<String>) -> AppResult<Option<String>> {
    match c.as_deref().map(|s| s.trim().to_uppercase()).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(c) if c == base_currency(state).await? => Ok(None),
        Some(c) => {
            if crate::crm::money::rate_of(state, &c).await?.is_none() {
                return Err(AppError::bad(format!("{c} is not one of the CRM's currencies")));
            }
            Ok(Some(c))
        }
    }
}

async fn create_account(State(state): State<AppState>, auth: AuthUser, Json(b): Json<AccountBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Admin)?;
    let code = clean_code(b.code.as_deref().unwrap_or(""))?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 200)?;
    let subtype = b.subtype.unwrap_or_default();
    let kind = type_of(&subtype)?;
    let currency = check_currency(&state, &b.currency.flatten()).await?;
    if currency.is_some() && !["bank", "cash"].contains(&subtype.as_str()) {
        return Err(AppError::bad("only bank and cash accounts can be kept in another currency"));
    }
    let id: Uuid = sqlx::query_scalar("INSERT INTO acc_accounts (code, name, type, subtype, parent_id, currency, description) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id")
        .bind(&code)
        .bind(&name)
        .bind(kind)
        .bind(&subtype)
        .bind(b.parent_id.flatten())
        .bind(currency)
        .bind(clean(b.description.flatten()))
        .fetch_one(&state.db)
        .await
        .map_err(|e| if crate::error::is_unique_violation(&e) { AppError::Conflict(format!("there is already an account {code}")) } else { e.into() })?;
    audit::record(&state.db, Some(auth.id), "accounting.account_create", "account", Some(id.to_string()), json!({ "code": code, "name": name }), None).await;
    Ok(Json(json!({ "id": id })))
}

async fn update_account(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<AccountBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Admin)?;
    let (subtype, used): (String, bool) = sqlx::query_as("SELECT subtype, EXISTS (SELECT 1 FROM acc_lines WHERE account_id = $1) FROM acc_accounts WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("account"))?;
    if let Some(s) = &b.subtype {
        let new_type = type_of(s)?;
        if used && new_type != type_of(&subtype)? {
            return Err(AppError::bad("an account with entries can't change between asset, liability, equity, income and expense"));
        }
    }
    if b.currency.is_some() && used {
        return Err(AppError::bad("an account with entries can't change currency"));
    }
    if let Some(Some(p)) = b.parent_id {
        if p == id {
            return Err(AppError::bad("an account can't be its own parent"));
        }
    }
    if b.is_active == Some(false) {
        let s = settings(&state).await?;
        let defaults = [s.receivable_account, s.payable_account, s.sales_account, s.retained_earnings, s.opening_balances, s.fx_account, s.rounding_account];
        if defaults.contains(&Some(id)) {
            return Err(AppError::bad("this account is a default in Accounting → Settings; choose another first"));
        }
    }
    let currency = match &b.currency {
        Some(c) => Some(check_currency(&state, c).await?),
        None => None,
    };
    let mut qb = sqlx::QueryBuilder::<sqlx::Postgres>::new("UPDATE acc_accounts SET is_active = is_active");
    if let Some(c) = &b.code {
        qb.push(", code = ").push_bind(clean_code(c)?);
    }
    if let Some(n) = &b.name {
        qb.push(", name = ").push_bind(required("name", n, 200)?);
    }
    if let Some(s) = &b.subtype {
        qb.push(", subtype = ").push_bind(s.clone()).push(", type = ").push_bind(type_of(s)?);
    }
    if let Some(p) = b.parent_id {
        qb.push(", parent_id = ").push_bind(p);
    }
    if let Some(c) = currency {
        qb.push(", currency = ").push_bind(c);
    }
    if let Some(d) = b.description {
        qb.push(", description = ").push_bind(clean(d));
    }
    if let Some(a) = b.is_active {
        qb.push(", is_active = ").push_bind(a);
    }
    qb.push(" WHERE id = ").push_bind(id);
    qb.build().execute(&state.db).await.map_err(|e| if crate::error::is_unique_violation(&e) { AppError::Conflict("there is already an account with that code".into()) } else { e.into() })?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_account(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&auth, Level::Admin)?;
    let refs: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM acc_lines WHERE account_id = $1) OR EXISTS (SELECT 1 FROM acc_taxes WHERE account_id = $1)
             OR EXISTS (SELECT 1 FROM acc_bill_lines WHERE account_id = $1) OR EXISTS (SELECT 1 FROM acc_payments WHERE account_id = $1)
             OR EXISTS (SELECT 1 FROM acc_bank_lines WHERE account_id = $1)
             OR EXISTS (SELECT 1 FROM acc_settings WHERE $1 IN (receivable_account, payable_account, sales_account, retained_earnings, opening_balances, fx_account, rounding_account))
             OR EXISTS (SELECT 1 FROM acc_product_accounts WHERE income_account_id = $1)",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if refs {
        return Err(AppError::bad("this account is in use; make it inactive instead"));
    }
    let n = sqlx::query("DELETE FROM acc_accounts WHERE id = $1").bind(id).execute(&state.db).await?.rows_affected();
    if n == 0 {
        return Err(AppError::NotFound("account"));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Taxes
// ---------------------------------------------------------------------------

#[derive(Serialize, FromRow)]
pub struct TaxRow {
    pub id: Uuid,
    pub name: String,
    pub rate: Decimal,
    pub scope: String,
    pub account_id: Uuid,
    pub is_active: bool,
}

async fn list_taxes(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    let rows: Vec<TaxRow> = sqlx::query_as("SELECT id, name, rate, scope, account_id, is_active FROM acc_taxes ORDER BY scope, rate, name").fetch_all(&state.db).await?;
    Ok(Json(json!({ "items": rows })))
}

#[derive(Deserialize)]
struct TaxBody {
    name: Option<String>,
    rate: Option<Decimal>,
    scope: Option<String>,
    account_id: Option<Uuid>,
    is_active: Option<bool>,
}

async fn check_tax_account(state: &AppState, scope: &str, account: Uuid) -> AppResult<()> {
    let sub: Option<String> = sqlx::query_scalar("SELECT subtype FROM acc_accounts WHERE id = $1 AND is_active").bind(account).fetch_optional(&state.db).await?;
    let want = if scope == "sales" { "tax_payable" } else { "tax_receivable" };
    if sub.as_deref() != Some(want) && sub.as_deref() != Some("tax_payable") {
        return Err(AppError::bad(format!("a {scope} tax goes to a tax {} account", if scope == "sales" { "payable" } else { "receivable (or payable)" })));
    }
    Ok(())
}

async fn create_tax(State(state): State<AppState>, auth: AuthUser, Json(b): Json<TaxBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Admin)?;
    ready(&state).await?;
    let name = required("name", b.name.as_deref().unwrap_or(""), 100)?;
    let rate = b.rate.ok_or_else(|| AppError::bad("give the rate in percent"))?;
    if rate < Decimal::ZERO || rate > Decimal::ONE_HUNDRED || rate.scale() > 4 {
        return Err(AppError::bad("the rate is a percentage from 0 to 100"));
    }
    let scope = b.scope.unwrap_or_default();
    if scope != "sales" && scope != "purchases" {
        return Err(AppError::bad("a tax is on sales or on purchases"));
    }
    let account = b.account_id.ok_or_else(|| AppError::bad("choose the tax account"))?;
    check_tax_account(&state, &scope, account).await?;
    let id: Uuid = sqlx::query_scalar("INSERT INTO acc_taxes (name, rate, scope, account_id) VALUES ($1, $2, $3, $4) RETURNING id")
        .bind(&name)
        .bind(rate)
        .bind(&scope)
        .bind(account)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(json!({ "id": id })))
}

async fn update_tax(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<TaxBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Admin)?;
    let (scope, used): (String, bool) = sqlx::query_as("SELECT scope, EXISTS (SELECT 1 FROM acc_lines WHERE tax_id = $1) OR EXISTS (SELECT 1 FROM acc_bill_lines WHERE tax_id = $1) FROM acc_taxes WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(AppError::NotFound("tax"))?;
    if used && (b.rate.is_some() || b.scope.is_some()) {
        return Err(AppError::bad("a tax that has been used keeps its rate; make a new tax and deactivate this one"));
    }
    if let Some(a) = b.account_id {
        check_tax_account(&state, b.scope.as_deref().unwrap_or(&scope), a).await?;
    }
    sqlx::query("UPDATE acc_taxes SET name = COALESCE($2, name), rate = COALESCE($3, rate), scope = COALESCE($4, scope), account_id = COALESCE($5, account_id), is_active = COALESCE($6, is_active) WHERE id = $1")
        .bind(id)
        .bind(b.name.as_deref().map(|n| required("name", n, 100)).transpose()?)
        .bind(b.rate)
        .bind(b.scope)
        .bind(b.account_id)
        .bind(b.is_active)
        .execute(&state.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn tax(state: &AppState, id: Uuid) -> AppResult<TaxRow> {
    sqlx::query_as("SELECT id, name, rate, scope, account_id, is_active FROM acc_taxes WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("tax"))
}

// ---------------------------------------------------------------------------
// Journal entries
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct EntriesQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    source: Option<String>,
    account_id: Option<Uuid>,
    status: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_entries(State(state): State<AppState>, auth: AuthUser, Query(q): Query<EntriesQuery>) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let term = q.q.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(|t| format!("%{}%", t.to_lowercase().replace('%', "\\%").replace('_', "\\_")));
    let rows: Vec<(Uuid, Option<i64>, NaiveDate, Option<String>, String, Option<Uuid>, String, Decimal, Option<Uuid>)> = sqlx::query_as(
        "SELECT e.id, e.number, e.date, e.memo, e.source, e.source_id, e.status,
                COALESCE((SELECT SUM(debit) FROM acc_lines l WHERE l.entry_id = e.id), 0), e.reverses
         FROM acc_entries e
         WHERE ($1::date IS NULL OR e.date >= $1) AND ($2::date IS NULL OR e.date <= $2) AND ($3::text IS NULL OR e.source = $3)
           AND ($4::uuid IS NULL OR EXISTS (SELECT 1 FROM acc_lines l WHERE l.entry_id = e.id AND l.account_id = $4))
           AND ($5::text IS NULL OR e.status = $5)
           AND ($6::text IS NULL OR lower(COALESCE(e.memo, '')) LIKE $6 OR e.number::text = trim(both '%' from $6))
         ORDER BY e.date DESC, e.number DESC NULLS FIRST LIMIT $7 OFFSET $8",
    )
    .bind(q.from)
    .bind(q.to)
    .bind(q.source)
    .bind(q.account_id)
    .bind(q.status)
    .bind(term)
    .bind(limit + 1)
    .bind(q.offset.unwrap_or(0).max(0))
    .fetch_all(&state.db)
    .await?;
    let more = rows.len() as i64 > limit;
    Ok(Json(json!({
        "items": rows.into_iter().take(limit as usize).map(|(id, n, d, m, s, sid, st, amt, rev)| json!({ "id": id, "number": n, "date": d, "memo": m, "source": s, "source_id": sid, "status": st, "amount": amt, "reverses": rev })).collect::<Vec<_>>(),
        "more": more,
    })))
}

pub async fn entry_json(state: &AppState, id: Uuid) -> AppResult<Value> {
    type E = (Uuid, Option<i64>, NaiveDate, Option<String>, String, Option<Uuid>, String, Option<Uuid>, Option<Uuid>, Option<String>, chrono::DateTime<Utc>);
    let e: E = sqlx::query_as(
        "SELECT e.id, e.number, e.date, e.memo, e.source, e.source_id, e.status, e.reverses, e.reversed_by,
                trim(u.first_name || ' ' || u.last_name), e.created_at
         FROM acc_entries e LEFT JOIN users u ON u.id = e.created_by WHERE e.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(AppError::NotFound("entry"))?;
    type L = (Uuid, Uuid, String, String, Decimal, Decimal, Option<String>, Option<Decimal>, Option<String>, Option<Uuid>, Option<String>, Option<Uuid>, Option<String>);
    let lines: Vec<L> = sqlx::query_as(
        "SELECT l.id, l.account_id, a.code, a.name, l.debit, l.credit, l.currency, l.amount_currency, l.description, l.party_id, p.title, l.tax_id, t.name
         FROM acc_lines l JOIN acc_accounts a ON a.id = l.account_id LEFT JOIN crm_items p ON p.id = l.party_id LEFT JOIN acc_taxes t ON t.id = l.tax_id
         WHERE l.entry_id = $1 ORDER BY l.sort",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let reversal_no = |x: Option<Uuid>| async move {
        match x {
            Some(r) => sqlx::query_scalar::<_, Option<i64>>("SELECT number FROM acc_entries WHERE id = $1").bind(r).fetch_one(&state.db).await.ok().flatten(),
            None => None,
        }
    };
    Ok(json!({
        "id": e.0, "number": e.1, "date": e.2, "memo": e.3, "source": e.4, "source_id": e.5, "status": e.6,
        "reverses": e.7, "reverses_number": reversal_no(e.7).await, "reversed_by": e.8, "reversed_by_number": reversal_no(e.8).await,
        "created_by_name": e.9, "created_at": e.10,
        "lines": lines.into_iter().map(|l| json!({
            "id": l.0, "account_id": l.1, "account_code": l.2, "account_name": l.3, "debit": l.4, "credit": l.5,
            "currency": l.6, "amount_currency": l.7, "description": l.8, "party_id": l.9, "party_name": l.10, "tax_id": l.11, "tax_name": l.12,
        })).collect::<Vec<_>>(),
    }))
}

async fn get_entry(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    Ok(Json(entry_json(&state, id).await?))
}

#[derive(Deserialize)]
pub struct LineIn {
    pub account_id: Uuid,
    #[serde(default)]
    pub debit: Decimal,
    #[serde(default)]
    pub credit: Decimal,
    pub description: Option<String>,
    pub party_id: Option<Uuid>,
}

#[derive(Deserialize)]
struct EntryBody {
    date: NaiveDate,
    memo: Option<String>,
    lines: Vec<LineIn>,
    /// true: post now; false: keep as a draft.
    #[serde(default)]
    post: bool,
}

async fn check_parties(state: &AppState, lines: &[LineIn]) -> AppResult<()> {
    let ids: Vec<Uuid> = lines.iter().filter_map(|l| l.party_id).collect();
    if ids.is_empty() {
        return Ok(());
    }
    let n: i64 = sqlx::query_scalar("SELECT COUNT(DISTINCT id) FROM crm_items WHERE id = ANY($1) AND entity_type IN (3, 4) AND deleted_at IS NULL").bind(&ids).fetch_one(&state.db).await?;
    if n as usize != ids.iter().collect::<std::collections::HashSet<_>>().len() {
        return Err(AppError::bad("a customer or supplier on the entry is not a CRM contact or company"));
    }
    Ok(())
}

fn to_lines(lines: &[LineIn]) -> AppResult<Vec<Line>> {
    if lines.len() > 500 {
        return Err(AppError::bad("at most 500 lines"));
    }
    Ok(lines
        .iter()
        .map(|l| Line { account: l.account_id, debit: l.debit, credit: l.credit, description: clean(l.description.clone()), party: l.party_id, ..Default::default() })
        .collect())
}

async fn create_entry(State(state): State<AppState>, auth: AuthUser, Json(b): Json<EntryBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    ready(&state).await?;
    check_parties(&state, &b.lines).await?;
    let lines = to_lines(&b.lines)?;
    let memo = clean(b.memo.clone());
    let mut tx = state.db.begin().await?;
    let id = if b.post {
        ledger::post(&mut tx, b.date, memo.as_deref(), "manual", None, &lines, Some(auth.id)).await?
    } else {
        // A draft: checked when it is posted (it may still be unbalanced).
        let id: Uuid = sqlx::query_scalar("INSERT INTO acc_entries (date, memo, source, status, created_by) VALUES ($1, $2, 'manual', 'draft', $3) RETURNING id")
            .bind(b.date)
            .bind(&memo)
            .bind(auth.id)
            .fetch_one(&mut *tx)
            .await?;
        ledger::insert_lines(&mut tx, id, b.date, &lines).await?;
        id
    };
    tx.commit().await?;
    if b.post {
        audit::record(&state.db, Some(auth.id), "accounting.entry_post", "entry", Some(id.to_string()), json!({}), None).await;
    }
    Ok(Json(entry_json(&state, id).await?))
}

async fn draft(state: &AppState, id: Uuid) -> AppResult<()> {
    let st: Option<String> = sqlx::query_scalar("SELECT status FROM acc_entries WHERE id = $1").bind(id).fetch_optional(&state.db).await?;
    match st.as_deref() {
        Some("draft") => Ok(()),
        Some(_) => Err(AppError::bad("posted entries can't be changed; reverse them instead")),
        None => Err(AppError::NotFound("entry")),
    }
}

async fn update_entry(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, Json(b): Json<EntryBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    draft(&state, id).await?;
    check_parties(&state, &b.lines).await?;
    let lines = to_lines(&b.lines)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE acc_entries SET date = $2, memo = $3 WHERE id = $1 AND status = 'draft'").bind(id).bind(b.date).bind(clean(b.memo)).execute(&mut *tx).await?;
    ledger::insert_lines(&mut tx, id, b.date, &lines).await?;
    tx.commit().await?;
    if b.post {
        return post_entry(State(state), auth, Path(id)).await;
    }
    Ok(Json(entry_json(&state, id).await?))
}

async fn post_entry(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let mut tx = state.db.begin().await?;
    let row: Option<(String, NaiveDate)> = sqlx::query_as("SELECT status, date FROM acc_entries WHERE id = $1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
    let (status, date) = row.ok_or(AppError::NotFound("entry"))?;
    if status != "draft" {
        return Err(AppError::bad("this entry is already posted"));
    }
    type L = (Uuid, Decimal, Decimal, Option<String>, Option<Uuid>);
    let lines: Vec<L> = sqlx::query_as("SELECT account_id, debit, credit, description, party_id FROM acc_lines WHERE entry_id = $1 ORDER BY sort").bind(id).fetch_all(&mut *tx).await?;
    let lines: Vec<Line> = lines.into_iter().map(|(a, d, c, desc, p)| Line { account: a, debit: d, credit: c, description: desc, party: p, ..Default::default() }).collect();
    ledger::check_open(&mut tx, date).await?;
    ledger::validate(&mut tx, &lines).await?;
    let n = ledger::next_number(&mut tx, "entry").await?;
    sqlx::query("UPDATE acc_entries SET status = 'posted', number = $2, posted_at = now() WHERE id = $1").bind(id).bind(n).execute(&mut *tx).await?;
    tx.commit().await?;
    audit::record(&state.db, Some(auth.id), "accounting.entry_post", "entry", Some(id.to_string()), json!({ "number": n }), None).await;
    Ok(Json(entry_json(&state, id).await?))
}

async fn delete_entry(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    draft(&state, id).await?;
    sqlx::query("DELETE FROM acc_entries WHERE id = $1 AND status = 'draft'").bind(id).execute(&state.db).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
struct ReverseBody {
    date: Option<NaiveDate>,
}

/// Manual and opening entries; documents are voided through their own pages.
async fn reverse_entry(State(state): State<AppState>, auth: AuthUser, Path(id): Path<Uuid>, body: Option<Json<ReverseBody>>) -> AppResult<Json<Value>> {
    need(&auth, Level::Post)?;
    let (source, date): (String, NaiveDate) = sqlx::query_as("SELECT source, date FROM acc_entries WHERE id = $1").bind(id).fetch_optional(&state.db).await?.ok_or(AppError::NotFound("entry"))?;
    if !["manual", "opening", "bank", "closing"].contains(&source.as_str()) {
        return Err(AppError::bad(format!("this entry belongs to a {source}; void the {source} instead")));
    }
    // Reopening a closed year is for administrators.
    if source == "closing" || source == "opening" {
        need(&auth, Level::Admin)?;
    }
    let when = body.and_then(|b| b.0.date).unwrap_or(date);
    let mut tx = state.db.begin().await?;
    // A bank-statement line matched to this entry goes back to unmatched.
    sqlx::query("UPDATE acc_bank_lines SET status = 'unmatched', line_id = NULL WHERE line_id IN (SELECT id FROM acc_lines WHERE entry_id = $1)").bind(id).execute(&mut *tx).await?;
    let rid = ledger::reverse(&mut tx, id, when, Some(auth.id)).await?;
    tx.commit().await?;
    audit::record(&state.db, Some(auth.id), "accounting.entry_reverse", "entry", Some(id.to_string()), json!({ "reversal": rid }), None).await;
    Ok(Json(entry_json(&state, id).await?))
}

#[derive(Deserialize)]
struct OpeningBody {
    date: NaiveDate,
    lines: Vec<LineIn>,
}

/// Balances carried over from before NERP: whatever doesn't balance goes to the opening
/// balances account.
async fn opening_balances(State(state): State<AppState>, auth: AuthUser, Json(b): Json<OpeningBody>) -> AppResult<Json<Value>> {
    need(&auth, Level::Admin)?;
    let s = ready(&state).await?;
    let ob = default_account(s.opening_balances, "opening balances")?;
    check_parties(&state, &b.lines).await?;
    let mut lines = to_lines(&b.lines)?;
    let diff: Decimal = lines.iter().map(|l| l.debit - l.credit).sum();
    if !diff.is_zero() {
        lines.push(ledger::signed(ob, -diff).text("Opening balances"));
    }
    let mut tx = state.db.begin().await?;
    let id = ledger::post(&mut tx, b.date, Some("Opening balances"), "opening", None, &lines, Some(auth.id)).await?;
    tx.commit().await?;
    Ok(Json(entry_json(&state, id).await?))
}

// ---------------------------------------------------------------------------
// Customers and suppliers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PartyQuery {
    q: Option<String>,
}

/// CRM companies and contacts to pick as a customer or supplier.
async fn parties(State(state): State<AppState>, auth: AuthUser, Query(q): Query<PartyQuery>) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    let term = format!("%{}%", q.q.unwrap_or_default().trim().to_lowercase().replace('%', "\\%").replace('_', "\\_"));
    let rows: Vec<(Uuid, String, i16, Option<String>)> = sqlx::query_as(
        "SELECT id, title, entity_type, company_kind FROM crm_items WHERE entity_type IN (3, 4) AND deleted_at IS NULL AND lower(title) LIKE $1
         ORDER BY entity_type DESC, title LIMIT 30",
    )
    .bind(term)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "items": rows.into_iter().map(|(id, t, e, k)| json!({ "id": id, "name": t, "kind": if e == 4 { "company" } else { "contact" }, "company_kind": k })).collect::<Vec<_>>() })))
}

/// CRM records the books refer to (posted invoices, customers and suppliers on entries,
/// bills and payments): they can't be deleted for good.
pub async fn in_books(state: &AppState, ids: &[Uuid]) -> AppResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT x FROM unnest($1::uuid[]) x WHERE EXISTS (SELECT 1 FROM acc_invoices WHERE item_id = x)
            OR EXISTS (SELECT 1 FROM acc_lines WHERE party_id = x) OR EXISTS (SELECT 1 FROM acc_bills WHERE party_id = x)
            OR EXISTS (SELECT 1 FROM acc_payments WHERE party_id = x) OR EXISTS (SELECT 1 FROM acc_invoices WHERE party_id = x)",
    )
    .bind(ids)
    .fetch_all(&state.db)
    .await?)
}

/// After two CRM records were merged: the books follow the one kept.
pub async fn merged(tx: &mut ledger::Tx<'_>, from: Uuid, into: Uuid) -> AppResult<()> {
    for t in ["acc_lines", "acc_invoices", "acc_bills", "acc_payments"] {
        sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE {t} SET party_id = $1 WHERE party_id = $2"))).bind(into).bind(from).execute(&mut **tx).await?;
    }
    Ok(())
}

pub async fn party_exists(state: &AppState, id: Uuid) -> AppResult<()> {
    let ok: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM crm_items WHERE id = $1 AND entity_type IN (3, 4) AND deleted_at IS NULL)").bind(id).fetch_one(&state.db).await?;
    if ok { Ok(()) } else { Err(AppError::bad("the customer or supplier must be a CRM contact or company")) }
}
