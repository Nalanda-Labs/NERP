//! Cash forecast: the money expected in and out, week by week, from what the company already
//! knows — and the bank balance it leads to.
//!
//! In (base currency):
//! - **Open invoices in the books**: what is still owed, expected on the due date plus how
//!   late that client usually pays (its paid invoices; else everyone's; else on time). Overdue
//!   ones whose expected date has passed land in the first week.
//! - **CRM invoices not yet in the books** (not paid or lost): their amount, the same way.
//! - **Open deals**: what isn't invoiced yet × the chance of winning (the deal's own
//!   probability, else how often its pipeline's deals were won over the last year), expected
//!   on the close date plus how long that client usually takes to pay an invoice (else 30
//!   days). Deals without a close date are listed apart.
//!
//! Out: open bills, what is still owed, on their due date (or their date).
//!
//! The balance starts from the bank and cash accounts in the books today. "Likely" counts deals
//! weighted; "at best" counts them in full; "invoices only" leaves deals out.

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use chrono::{Datelike, Duration, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use super::{need, Level};
use crate::{auth::AuthUser, error::{AppError, AppResult}, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new().route("/accounting/cash-forecast", get(forecast))
}

#[derive(Deserialize)]
struct ForecastQuery {
    weeks: Option<i64>,
}

#[derive(Clone)]
struct Flow {
    date: NaiveDate,
    /// in | out
    dir: &'static str,
    /// invoice | crm_invoice | deal | bill
    kind: &'static str,
    id: Uuid,
    label: String,
    party: Option<String>,
    amount: Decimal,
    /// Deals: the full amount (before weighting) and the chance.
    full: Decimal,
    chance: Option<Decimal>,
    link: String,
}

async fn forecast(State(state): State<AppState>, auth: AuthUser, Query(q): Query<ForecastQuery>) -> AppResult<Json<Value>> {
    need(&auth, Level::View)?;
    let weeks = q.weeks.unwrap_or(13);
    if !(1..=52).contains(&weeks) {
        return Err(AppError::bad("1 to 52 weeks"));
    }
    let today = Utc::now().date_naive();
    let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let base: String = super::base_currency(&state).await?;

    // How each client pays: days late after the due date, and days from invoice to payment.
    let habits: Vec<(Option<Uuid>, Option<f64>, Option<f64>)> = sqlx::query_as(
        "WITH paid AS (
            SELECT a.party_id, a.date, a.due_date,
                   (SELECT MAX(p.date) FROM acc_allocations al JOIN acc_payments p ON p.id = al.payment_id
                     WHERE al.doc_kind = 'invoice' AND al.doc_id = a.item_id AND p.status = 'posted') AS paid_on
            FROM acc_invoices a WHERE a.status = 'paid')
         SELECT party_id, AVG(GREATEST(paid_on - COALESCE(due_date, date), 0))::float8, AVG(GREATEST(paid_on - date, 0))::float8
         FROM paid WHERE paid_on IS NOT NULL GROUP BY ROLLUP (party_id)",
    )
    .fetch_all(&state.db)
    .await?;
    let mut late: HashMap<Uuid, i64> = HashMap::new();
    let mut term: HashMap<Uuid, i64> = HashMap::new();
    let (mut all_late, mut all_term) = (0i64, 30i64);
    for (party, l, t) in habits {
        let (Some(l), Some(t)) = (l, t) else { continue };
        match party {
            Some(p) => {
                late.insert(p, l.round() as i64);
                term.insert(p, t.round() as i64);
            }
            None => {
                all_late = l.round() as i64;
                all_term = t.round() as i64;
            }
        }
    }
    // A grand-total row also comes back for an empty set: keep the defaults then.
    let has_history: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM acc_invoices WHERE status = 'paid')").fetch_one(&state.db).await?;
    if !has_history {
        all_late = 0;
        all_term = 30;
    }
    let late_of = |p: Option<Uuid>| p.and_then(|p| late.get(&p).copied()).unwrap_or(all_late);
    let term_of = |p: Option<Uuid>| p.and_then(|p| term.get(&p).copied()).unwrap_or(all_term);

    let mut flows: Vec<Flow> = Vec::new();

    // Open invoices in the books: what is still owed.
    let open: Vec<(Uuid, NaiveDate, Option<NaiveDate>, Option<Uuid>, Option<String>, Decimal, Option<String>)> = sqlx::query_as(
        "SELECT a.item_id, a.date, a.due_date, a.party_id, p.title,
                (a.total - COALESCE((SELECT SUM(al.amount) FROM acc_allocations al JOIN acc_payments pm ON pm.id = al.payment_id
                                     WHERE al.doc_kind = 'invoice' AND al.doc_id = a.item_id AND pm.status = 'posted'), 0)) * a.fx_rate,
                i.title
         FROM acc_invoices a JOIN crm_items i ON i.id = a.item_id LEFT JOIN crm_items p ON p.id = a.party_id
         WHERE a.status = 'open'",
    )
    .fetch_all(&state.db)
    .await?;
    for (id, date, due, party, pname, owed, title) in open {
        if owed <= Decimal::ZERO {
            continue;
        }
        let expected = (due.unwrap_or(date + Duration::days(term_of(party))) + Duration::days(late_of(party))).max(today);
        flows.push(Flow { date: expected, dir: "in", kind: "invoice", id, label: title.unwrap_or_default(), party: pname, amount: owed, full: owed, chance: None, link: format!("/accounting/invoices/{id}") });
    }

    // CRM invoices not in the books yet (not paid, not lost).
    let unposted: Vec<(Uuid, String, chrono::DateTime<Utc>, Option<NaiveDate>, Option<Uuid>, Option<String>, Decimal)> = sqlx::query_as(
        "SELECT i.id, i.title, i.created_at, i.close_date, i.company_id, c.title, i.opportunity_base
         FROM crm_items i JOIN crm_entity_types t ON t.id = i.entity_type
         LEFT JOIN crm_stages s ON s.id = i.stage_id LEFT JOIN crm_items c ON c.id = i.company_id
         WHERE t.kind = 'invoice' AND i.deleted_at IS NULL AND COALESCE(s.semantics, 'process') = 'process'
           AND NOT EXISTS (SELECT 1 FROM acc_invoices a WHERE a.item_id = i.id AND a.status <> 'void') AND i.opportunity_base > 0",
    )
    .fetch_all(&state.db)
    .await?;
    for (id, title, created, due, party, pname, amount) in unposted {
        let expected = (due.unwrap_or(created.date_naive() + Duration::days(term_of(party))) + Duration::days(late_of(party))).max(today);
        flows.push(Flow { date: expected, dir: "in", kind: "crm_invoice", id, label: title, party: pname, amount, full: amount, chance: None, link: format!("/crm/invoice/{id}") });
    }

    // Open deals: what isn't invoiced yet, by the chance of winning.
    let win_rates: HashMap<Uuid, Decimal> = sqlx::query_as::<_, (Uuid, Decimal)>(
        "SELECT i.category_id, (COUNT(*) FILTER (WHERE s.semantics = 'success'))::numeric / COUNT(*)
         FROM crm_items i JOIN crm_stages s ON s.id = i.stage_id
         WHERE i.entity_type = 2 AND i.deleted_at IS NULL AND s.semantics IN ('success', 'failure') AND i.updated_at > now() - interval '1 year'
         GROUP BY i.category_id",
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .collect();
    let deals: Vec<(Uuid, String, Option<NaiveDate>, Option<Uuid>, Option<String>, Decimal, Option<i16>, Option<Uuid>, Decimal)> = sqlx::query_as(
        "SELECT i.id, i.title, i.close_date, i.company_id, c.title, i.opportunity_base, i.probability, i.category_id,
                COALESCE((SELECT SUM(x.opportunity_base) FROM crm_items x JOIN crm_entity_types xt ON xt.id = x.entity_type
                          WHERE xt.kind = 'invoice' AND x.deleted_at IS NULL
                            AND (x.source_item_id = i.id OR x.source_item_id IN (SELECT q.id FROM crm_items q WHERE q.source_item_id = i.id))), 0)
         FROM crm_items i JOIN crm_stages s ON s.id = i.stage_id LEFT JOIN crm_items c ON c.id = i.company_id
         WHERE i.entity_type = 2 AND i.deleted_at IS NULL AND s.semantics = 'process' AND i.opportunity_base > 0",
    )
    .fetch_all(&state.db)
    .await?;
    let mut undated: Vec<Value> = Vec::new();
    for (id, title, close, party, pname, amount, prob, cat, invoiced) in deals {
        let left = amount - invoiced;
        if left <= Decimal::ZERO {
            continue;
        }
        let chance = match prob {
            Some(p) => Decimal::from(p) / Decimal::ONE_HUNDRED,
            None => cat.and_then(|c| win_rates.get(&c).copied()).unwrap_or(Decimal::new(5, 1)),
        };
        let Some(close) = close else {
            undated.push(json!({ "id": id, "label": title, "party": pname, "amount": left.round_dp(2), "chance": (chance * Decimal::ONE_HUNDRED).round_dp(0), "link": format!("/crm/deal/{id}") }));
            continue;
        };
        let expected = (close + Duration::days(term_of(party))).max(today);
        flows.push(Flow { date: expected, dir: "in", kind: "deal", id, label: title, party: pname, amount: left * chance, full: left, chance: Some(chance), link: format!("/crm/deal/{id}") });
    }

    // Open bills: what is still owed.
    let bills: Vec<(Uuid, NaiveDate, Option<NaiveDate>, Option<String>, Option<String>, Decimal)> = sqlx::query_as(
        "SELECT b.id, b.date, b.due_date, p.title, b.reference,
                (b.total - COALESCE((SELECT SUM(al.amount) FROM acc_allocations al JOIN acc_payments pm ON pm.id = al.payment_id
                                     WHERE al.doc_kind = 'bill' AND al.doc_id = b.id AND pm.status = 'posted'), 0)) * b.fx_rate
         FROM acc_bills b LEFT JOIN crm_items p ON p.id = b.party_id WHERE b.status = 'open'",
    )
    .fetch_all(&state.db)
    .await?;
    for (id, date, due, pname, reference, owed) in bills {
        if owed <= Decimal::ZERO {
            continue;
        }
        flows.push(Flow {
            date: due.unwrap_or(date).max(today),
            dir: "out",
            kind: "bill",
            id,
            label: reference.unwrap_or_else(|| "Bill".into()),
            party: pname,
            amount: owed,
            full: owed,
            chance: None,
            link: format!("/accounting/bills/{id}"),
        });
    }

    // Where the bank and cash accounts stand today.
    let opening: Decimal = sqlx::query_scalar(
        "SELECT COALESCE(SUM(l.debit - l.credit), 0) FROM acc_lines l JOIN acc_accounts a ON a.id = l.account_id
         JOIN acc_entries e ON e.id = l.entry_id WHERE a.subtype IN ('bank', 'cash') AND e.status <> 'draft' AND l.date <= CURRENT_DATE",
    )
    .fetch_one(&state.db)
    .await?;

    // Week by week.
    let end = monday + Duration::weeks(weeks);
    let mut out_weeks = Vec::new();
    let (mut likely, mut best, mut worst) = (opening, opening, opening);
    let mut later = Decimal::ZERO;
    for w in 0..weeks {
        let start = monday + Duration::weeks(w);
        let stop = start + Duration::weeks(1);
        let these: Vec<&Flow> = flows.iter().filter(|f| f.date >= start && f.date < stop).collect();
        let sum = |pred: &dyn Fn(&Flow) -> bool, full: bool| these.iter().filter(|f| pred(f)).map(|f| if full { f.full } else { f.amount }).sum::<Decimal>();
        let invoices_in = sum(&|f| f.dir == "in" && f.kind != "deal", false);
        let deals_in = sum(&|f| f.kind == "deal", false);
        let deals_full = sum(&|f| f.kind == "deal", true);
        let bills_out = sum(&|f| f.dir == "out", false);
        likely += invoices_in + deals_in - bills_out;
        best += invoices_in + deals_full - bills_out;
        worst += invoices_in - bills_out;
        out_weeks.push(json!({
            "start": start,
            "invoices_in": invoices_in.round_dp(2), "deals_in": deals_in.round_dp(2), "bills_out": bills_out.round_dp(2),
            "net": (invoices_in + deals_in - bills_out).round_dp(2),
            "balance": likely.round_dp(2), "best": best.round_dp(2), "worst": worst.round_dp(2),
            "items": these.iter().map(|f| json!({
                "kind": f.kind, "dir": f.dir, "id": f.id, "label": f.label, "party": f.party, "date": f.date,
                "amount": f.amount.round_dp(2), "full": f.full.round_dp(2),
                "chance": f.chance.map(|c| (c * Decimal::ONE_HUNDRED).round_dp(0)), "link": f.link,
            })).collect::<Vec<_>>(),
        }));
    }
    for f in flows.iter().filter(|f| f.date >= end) {
        later += if f.dir == "in" { f.amount } else { -f.amount };
    }
    let first_negative = out_weeks.iter().find(|w| w["balance"].as_str().and_then(|b| b.parse::<f64>().ok()).is_some_and(|b| b < 0.0)).map(|w| w["start"].clone());
    Ok(Json(json!({
        "currency": base.trim(),
        "opening": opening.round_dp(2),
        "weeks": out_weeks,
        "later": later.round_dp(2),
        "undated_deals": undated,
        "first_shortfall": first_negative,
        "habits": { "late_days": all_late, "term_days": all_term, "from_history": has_history },
    })))
}
