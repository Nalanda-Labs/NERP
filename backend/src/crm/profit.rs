//! True profit: what a client, a deal, a product or a salesperson really earned the company
//! in a period — not just what was sold.
//!
//! For every invoice in the period (CRM invoices that aren't lost or deleted), in the base
//! currency:
//! - **revenue**: its rows' amounts without tax (after discounts and credit notes), or its amount when it has
//!   no rows; **discounts**: what the discounts took off the list prices;
//! - **goods**: what the products cost (each row's cost, else the product's stock cost);
//! - **support** and **task time**: minutes support spent answering the client's tickets and
//!   time logged on tasks linked to the client's records, at each person's hourly cost (or the
//!   default) — the client's, spread over the client's invoices by revenue;
//! - **late payment**: for invoices in the books, the days paid after the due date (or overdue
//!   so far) × the amount × the yearly financing rate.
//!
//! Profit = revenue − goods − support − task time − late payment. The same invoice figures add
//! up by client, deal (the invoice's deal, through a quote too), salesperson (the invoice's
//! responsible person) or product (the invoice's rows; its costs spread over them by revenue).

use axum::{
    extract::{Query, State},
    routing::{get, put},
    Json, Router,
};
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use super::money::{line, rows_of, unit_cost, ProductRow};
use crate::{
    auth::{permissions as perm, AuthUser},
    error::{AppError, AppResult},
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/crm/profit", get(report))
        .route("/crm/profit/settings", get(get_settings).put(put_settings))
        .route("/crm/profit/costs", put(put_costs))
}

/// Profit shows costs and wages: for the books' readers and CRM administrators.
fn may_see(auth: &AuthUser) -> AppResult<()> {
    if auth.can(perm::ACCOUNTING_VIEW) || auth.can(perm::CRM_ADMIN) { Ok(()) } else { Err(AppError::Forbidden) }
}

fn may_set(auth: &AuthUser) -> AppResult<()> {
    if auth.can(perm::USERS_MANAGE) || auth.can(perm::CRM_ADMIN) { Ok(()) } else { Err(AppError::Forbidden) }
}

#[derive(Deserialize)]
struct ReportQuery {
    /// client | deal | product | person
    by: Option<String>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

#[derive(Default, Clone)]
struct Figures {
    revenue: Decimal,
    discounts: Decimal,
    goods: Decimal,
    support: Decimal,
    time: Decimal,
    late: Decimal,
    invoices: i64,
    late_days: i64,
}

impl Figures {
    fn add(&mut self, o: &Figures) {
        self.revenue += o.revenue;
        self.discounts += o.discounts;
        self.goods += o.goods;
        self.support += o.support;
        self.time += o.time;
        self.late += o.late;
        self.invoices += o.invoices;
        self.late_days += o.late_days;
    }
    fn scaled(&self, share: Decimal) -> Figures {
        Figures {
            revenue: self.revenue * share,
            discounts: self.discounts * share,
            goods: self.goods * share,
            support: self.support * share,
            time: self.time * share,
            late: self.late * share,
            invoices: self.invoices,
            late_days: self.late_days,
        }
    }
    fn json(&self) -> Value {
        let r2 = |d: Decimal| d.round_dp(2);
        let profit = self.revenue - self.goods - self.support - self.time - self.late;
        json!({
            "revenue": r2(self.revenue), "discounts": r2(self.discounts), "goods": r2(self.goods),
            "support": r2(self.support), "time": r2(self.time), "late": r2(self.late),
            "profit": r2(profit),
            "margin": if self.revenue.is_zero() { Value::Null } else { json!((profit / self.revenue * Decimal::ONE_HUNDRED).round_dp(1)) },
            "invoices": self.invoices,
        })
    }
}

#[derive(sqlx::FromRow)]
struct Invoice {
    id: Uuid,
    opportunity: Decimal,
    company_id: Option<Uuid>,
    contact_id: Option<Uuid>,
    assigned_to: Option<Uuid>,
    source_item_id: Option<Uuid>,
    rate: Decimal,
    due_date: Option<NaiveDate>,
    total_base: Option<Decimal>,
    paid_on: Option<NaiveDate>,
    open: bool,
    /// The share of the invoice taken back by credit notes.
    credited_share: Option<Decimal>,
}

async fn report(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ReportQuery>) -> AppResult<Json<Value>> {
    may_see(&auth)?;
    let by = q.by.as_deref().unwrap_or("client");
    if !["client", "deal", "product", "person"].contains(&by) {
        return Err(AppError::bad("group by client, deal, product or person"));
    }
    let today = Utc::now().date_naive();
    let to = q.to.unwrap_or(today);
    let from = q.from.unwrap_or(to - chrono::Duration::days(90));
    if from > to || (to - from).num_days() > 3660 {
        return Err(AppError::bad("a period of at most ten years, from before to"));
    }
    let (default_hourly, financing): (Decimal, Decimal) = sqlx::query_as("SELECT default_hourly, financing_rate FROM profit_settings WHERE id = 1").fetch_one(&state.db).await?;
    let base: String = super::money::base_currency(&state).await?;

    // The period's invoices (lost or deleted ones left out), with what the books know.
    let invoices: Vec<Invoice> = sqlx::query_as(
        "SELECT i.id, i.opportunity, i.company_id,
                (SELECT contact_id FROM crm_item_contacts c WHERE c.item_id = i.id ORDER BY c.is_primary DESC, c.sort LIMIT 1) AS contact_id,
                i.assigned_to, i.source_item_id,
                COALESCE(a.fx_rate, (SELECT rate FROM crm_currencies c WHERE c.code = i.currency), 1) AS rate,
                a.due_date, a.total * a.fx_rate AS total_base,
                (SELECT MAX(p.date) FROM acc_allocations al JOIN acc_payments p ON p.id = al.payment_id
                  WHERE al.doc_kind = 'invoice' AND al.doc_id = i.id AND p.status = 'posted') AS paid_on,
                COALESCE(a.status = 'open', FALSE) AS open,
                a.credited / NULLIF(a.total, 0) AS credited_share
         FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type
         LEFT JOIN crm_stages s ON s.id = i.stage_id
         LEFT JOIN acc_invoices a ON a.item_id = i.id AND a.status <> 'void'
         WHERE t.kind = 'invoice' AND i.deleted_at IS NULL AND COALESCE(s.semantics, 'process') <> 'failure'
           AND COALESCE(a.date, i.created_at::date) BETWEEN $1 AND $2",
    )
    .bind(from)
    .bind(to)
    .fetch_all(&state.db)
    .await?;

    // Per invoice: revenue, discounts and goods (per row, for products), and late payment.
    struct Inv {
        f: Figures,
        client: Option<Uuid>,
        rows: Vec<(Option<Uuid>, String, Figures)>,
    }
    let mut per: HashMap<Uuid, Inv> = HashMap::new();
    let mut cost_cache: HashMap<Uuid, Option<Decimal>> = HashMap::new();
    for inv in &invoices {
        let rows = rows_of(&state, inv.id).await?;
        let mut f = Figures { invoices: 1, ..Default::default() };
        let mut row_figs = Vec::new();
        for r in &rows {
            let l = line(r);
            let list = line(&ProductRow { discount_value: Decimal::ZERO, ..r.clone() });
            let unit_cost_here = match (r.cost, r.product_id) {
                (Some(c), _) => Some(c),
                (None, Some(p)) => {
                    if !cost_cache.contains_key(&p) {
                        cost_cache.insert(p, unit_cost(&state, p, &base).await?);
                    }
                    // The cache holds the base currency: back to the invoice's.
                    cost_cache[&p].map(|c| if inv.rate.is_zero() { c } else { c / inv.rate })
                }
                _ => None,
            };
            // Credit notes take back their share of each row's revenue (shown with the discounts).
            let kept = Decimal::ONE - inv.credited_share.unwrap_or_default();
            let rf = Figures {
                revenue: l.net * kept * inv.rate,
                discounts: ((list.net - l.net).max(Decimal::ZERO) + l.net * (Decimal::ONE - kept)) * inv.rate,
                goods: unit_cost_here.unwrap_or_default() * r.quantity * inv.rate,
                ..Default::default()
            };
            f.add(&rf);
            row_figs.push((r.product_id, r.name.clone(), rf));
        }
        if rows.is_empty() {
            let kept = Decimal::ONE - inv.credited_share.unwrap_or_default();
            f.revenue = inv.opportunity * kept * inv.rate;
            f.discounts = inv.opportunity * (Decimal::ONE - kept) * inv.rate;
        }
        // Late payment: days after the due date (paid, or overdue so far).
        if let (Some(due), Some(total)) = (inv.due_date, inv.total_base) {
            let end = if inv.open { Some(today) } else { inv.paid_on };
            if let Some(end) = end {
                let days = (end - due).num_days().max(0);
                f.late_days = days;
                f.late = total * Decimal::from(days) / Decimal::from(365) * financing / Decimal::ONE_HUNDRED;
            }
        }
        per.insert(inv.id, Inv { f, client: inv.company_id.or(inv.contact_id), rows: row_figs });
    }

    // Time spent on each client in the period, at each person's hourly cost.
    let costs: HashMap<Uuid, Decimal> = sqlx::query_as::<_, (Uuid, Decimal)>("SELECT user_id, hourly FROM user_costs").fetch_all(&state.db).await?.into_iter().collect();
    let hourly = |u: Option<Uuid>| u.and_then(|u| costs.get(&u).copied()).unwrap_or(default_hourly);
    let clients: Vec<Uuid> = {
        let mut c: Vec<Uuid> = per.values().filter_map(|i| i.client).collect();
        c.sort();
        c.dedup();
        c
    };
    let mut support: HashMap<Uuid, Decimal> = HashMap::new();
    let mut task_time: HashMap<Uuid, Decimal> = HashMap::new();
    if !clients.is_empty() {
        // A client is a company (its contacts count too) or a contact.
        let rows: Vec<(Uuid, Option<Uuid>, i64)> = sqlx::query_as(
            "SELECT cl.id, m.author_id, SUM(m.minutes_spent)::bigint
             FROM unnest($1::uuid[]) AS cl(id)
             JOIN crm_items who ON who.id = cl.id OR (who.company_id = cl.id AND who.entity_type = 3)
             JOIN support_tickets t ON t.contact_id = who.id
             JOIN support_messages m ON m.ticket_id = t.id
             WHERE m.minutes_spent > 0 AND m.created_at::date BETWEEN $2 AND $3
             GROUP BY 1, 2",
        )
        .bind(&clients)
        .bind(from)
        .bind(to)
        .fetch_all(&state.db)
        .await?;
        for (client, who, minutes) in rows {
            *support.entry(client).or_default() += Decimal::from(minutes) / Decimal::from(60) * hourly(who);
        }
        // Tasks linked to the client, its contacts, or any of their deals and invoices.
        let rows: Vec<(Uuid, Uuid, i64)> = sqlx::query_as(
            "SELECT cl.id, e.user_id, SUM(EXTRACT(EPOCH FROM (COALESCE(e.stopped_at, now()) - e.started_at)))::bigint
             FROM unnest($1::uuid[]) AS cl(id)
             JOIN crm_items rec ON rec.id = cl.id OR rec.company_id = cl.id
                OR rec.id IN (SELECT ic.item_id FROM crm_item_contacts ic WHERE ic.contact_id = cl.id)
             JOIN task_crm_links l ON l.item_id = rec.id
             JOIN task_time_entries e ON e.task_id = l.task_id
             WHERE e.started_at::date BETWEEN $2 AND $3
             GROUP BY 1, 2",
        )
        .bind(&clients)
        .bind(from)
        .bind(to)
        .fetch_all(&state.db)
        .await?;
        for (client, who, secs) in rows {
            *task_time.entry(client).or_default() += Decimal::from(secs) / Decimal::from(3600) * hourly(Some(who));
        }
    }
    // Spread each client's time over its invoices by revenue (evenly when they have none).
    let mut client_revenue: HashMap<Uuid, (Decimal, i64)> = HashMap::new();
    for i in per.values() {
        if let Some(c) = i.client {
            let e = client_revenue.entry(c).or_default();
            e.0 += i.f.revenue;
            e.1 += 1;
        }
    }
    for i in per.values_mut() {
        let Some(c) = i.client else { continue };
        let (rev, n) = client_revenue[&c];
        let share = if rev.is_zero() { Decimal::ONE / Decimal::from(n.max(1)) } else { i.f.revenue / rev };
        i.f.support = support.get(&c).copied().unwrap_or_default() * share;
        i.f.time = task_time.get(&c).copied().unwrap_or_default() * share;
    }

    // Group.
    let mut groups: HashMap<Option<Uuid>, (String, Figures)> = HashMap::new();
    let deal_of = |id: Uuid| invoices.iter().find(|x| x.id == id).and_then(|x| x.source_item_id);
    for inv in &invoices {
        let i = &per[&inv.id];
        match by {
            "product" => {
                let overheads = Figures { support: i.f.support, time: i.f.time, late: i.f.late, ..Default::default() };
                for (pid, name, rf) in &i.rows {
                    let share = if i.f.revenue.is_zero() { Decimal::ZERO } else { rf.revenue / i.f.revenue };
                    let mut f = rf.clone();
                    f.add(&overheads.scaled(share));
                    let e = groups.entry(*pid).or_insert_with(|| (name.clone(), Figures::default()));
                    e.1.add(&f);
                    e.1.invoices += 1;
                }
            }
            _ => {
                let key = match by {
                    "client" => i.client,
                    "person" => inv.assigned_to,
                    _ => deal_of(inv.id),
                };
                groups.entry(key).or_insert_with(|| (String::new(), Figures::default())).1.add(&i.f);
            }
        }
    }
    // Names (and for deals: through a quote to its deal).
    let mut keys: Vec<Uuid> = groups.keys().flatten().copied().collect();
    if by == "deal" {
        let mut regrouped: HashMap<Option<Uuid>, (String, Figures)> = HashMap::new();
        for (k, v) in groups.drain() {
            let target = match k {
                Some(src) => {
                    let (kind, up): (Option<String>, Option<Uuid>) = sqlx::query_as("SELECT t.kind, i.source_item_id FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type WHERE i.id = $1")
                        .bind(src)
                        .fetch_optional(&state.db)
                        .await?
                        .unwrap_or((None, None));
                    if kind.as_deref() == Some("quote") { up.or(Some(src)) } else { Some(src) }
                }
                None => None,
            };
            regrouped.entry(target).or_insert_with(|| (String::new(), Figures::default())).1.add(&v.1);
        }
        groups = regrouped;
        keys = groups.keys().flatten().copied().collect();
    }
    let names: HashMap<Uuid, String> = if by == "person" {
        sqlx::query_as::<_, (Uuid, String)>("SELECT id, trim(first_name || ' ' || last_name) FROM users WHERE id = ANY($1)").bind(&keys).fetch_all(&state.db).await?.into_iter().collect()
    } else if by == "product" {
        sqlx::query_as::<_, (Uuid, String)>("SELECT id, name FROM crm_products WHERE id = ANY($1)").bind(&keys).fetch_all(&state.db).await?.into_iter().collect()
    } else {
        sqlx::query_as::<_, (Uuid, String)>("SELECT id, title FROM crm_items WHERE id = ANY($1)").bind(&keys).fetch_all(&state.db).await?.into_iter().collect()
    };
    let links: HashMap<Uuid, String> = if by == "client" || by == "deal" {
        sqlx::query_as::<_, (Uuid, String)>("SELECT i.id, t.code FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type WHERE i.id = ANY($1)")
            .bind(&keys)
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .collect()
    } else {
        HashMap::new()
    };
    let mut total = Figures::default();
    let mut rows: Vec<(Decimal, Value)> = groups
        .into_iter()
        .map(|(k, (fallback, f))| {
            total.revenue += f.revenue;
            total.discounts += f.discounts;
            total.goods += f.goods;
            total.support += f.support;
            total.time += f.time;
            total.late += f.late;
            let name = k.and_then(|k| names.get(&k).cloned()).unwrap_or_else(|| if fallback.is_empty() { match by { "client" => "No client", "deal" => "Not from a deal", "person" => "Nobody", _ => "Other" }.to_string() } else { fallback });
            let profit = f.revenue - f.goods - f.support - f.time - f.late;
            let mut v = f.json();
            v["id"] = json!(k);
            v["name"] = json!(name);
            v["link"] = json!(k.and_then(|k| links.get(&k).map(|code| format!("/crm/{code}/{k}"))));
            (profit, v)
        })
        .collect();
    total.invoices = invoices.len() as i64;
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(Json(json!({
        "by": by, "from": from, "to": to, "currency": base.trim(),
        "settings": { "default_hourly": default_hourly, "financing_rate": financing },
        "total": total.json(),
        "items": rows.into_iter().map(|r| r.1).collect::<Vec<_>>(),
    })))
}

async fn get_settings(State(state): State<AppState>, auth: AuthUser) -> AppResult<Json<Value>> {
    may_see(&auth)?;
    let (default_hourly, financing): (Decimal, Decimal) = sqlx::query_as("SELECT default_hourly, financing_rate FROM profit_settings WHERE id = 1").fetch_one(&state.db).await?;
    let people: Vec<(Uuid, String, Option<Decimal>)> = sqlx::query_as(
        "SELECT u.id, trim(u.first_name || ' ' || u.last_name), c.hourly FROM users u LEFT JOIN user_costs c ON c.user_id = u.id
         WHERE u.is_active AND NOT u.is_guest ORDER BY u.first_name, u.last_name",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "default_hourly": default_hourly, "financing_rate": financing,
        "people": people.into_iter().map(|(id, name, h)| json!({ "id": id, "name": name, "hourly": h })).collect::<Vec<_>>(),
    })))
}

/// An amount as a number or a string ("12.50").
fn amount(v: &Option<Value>) -> AppResult<Option<Decimal>> {
    use std::str::FromStr;
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => Decimal::from_str(&n.to_string()).map(Some).map_err(|_| AppError::bad("not a number")),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => Decimal::from_str(s.trim()).map(Some).map_err(|_| AppError::bad("not a number")),
        _ => Err(AppError::bad("not a number")),
    }
}

#[derive(Deserialize)]
struct SettingsInput {
    default_hourly: Option<Value>,
    financing_rate: Option<Value>,
}

async fn put_settings(State(state): State<AppState>, auth: AuthUser, Json(b): Json<SettingsInput>) -> AppResult<Json<Value>> {
    may_set(&auth)?;
    let (hourly, rate) = (amount(&b.default_hourly)?, amount(&b.financing_rate)?);
    if hourly.is_some_and(|h| h.is_sign_negative()) || rate.is_some_and(|r| r.is_sign_negative() || r > Decimal::ONE_HUNDRED) {
        return Err(AppError::bad("an hourly cost of 0 or more, a rate of 0 to 100 percent"));
    }
    sqlx::query("UPDATE profit_settings SET default_hourly = COALESCE($1, default_hourly), financing_rate = COALESCE($2, financing_rate) WHERE id = 1")
        .bind(hourly.map(|d| d.round_dp(2)))
        .bind(rate.map(|d| d.round_dp(2)))
        .execute(&state.db)
        .await?;
    crate::audit::record(&state.db, Some(auth.id), "profit.settings", "settings", None, json!({}), None).await;
    get_settings(State(state), auth).await
}

#[derive(Deserialize)]
struct CostInput {
    user_id: Uuid,
    /// None: the default applies.
    hourly: Option<Value>,
}

/// What an hour of these people's time costs (wages and overheads), in the base currency.
async fn put_costs(State(state): State<AppState>, auth: AuthUser, Json(b): Json<Vec<CostInput>>) -> AppResult<Json<Value>> {
    may_set(&auth)?;
    let parsed: Vec<(Uuid, Option<Decimal>)> = b.iter().map(|c| amount(&c.hourly).map(|h| (c.user_id, h))).collect::<AppResult<_>>()?;
    if parsed.len() > 2000 || parsed.iter().any(|(_, h)| h.is_some_and(|h| h.is_sign_negative())) {
        return Err(AppError::bad("hourly costs are 0 or more"));
    }
    let mut tx = state.db.begin().await?;
    for (user_id, hourly) in &parsed {
        let c = CostRef { user_id: *user_id, hourly: *hourly };
        match c.hourly {
            Some(h) => {
                sqlx::query("INSERT INTO user_costs (user_id, hourly) VALUES ($1, $2) ON CONFLICT (user_id) DO UPDATE SET hourly = $2, updated_at = now()")
                    .bind(c.user_id)
                    .bind(h.round_dp(2))
                    .execute(&mut *tx)
                    .await
                    .map_err(|_| AppError::bad("one of them isn't an employee"))?;
            }
            None => {
                sqlx::query("DELETE FROM user_costs WHERE user_id = $1").bind(c.user_id).execute(&mut *tx).await?;
            }
        }
    }
    crate::audit::record(&mut *tx, Some(auth.id), "profit.costs", "settings", None, json!({ "people": parsed.len() }), None).await;
    tx.commit().await?;
    get_settings(State(state), auth).await
}

struct CostRef {
    user_id: Uuid,
    hourly: Option<Decimal>,
}
