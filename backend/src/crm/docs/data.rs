//! The data every document template receives, identical for all three engines.
//!
//! Money, numbers and dates are formatted here, on the server, for the template's locale,
//! so a Typst, LaTeX or Word template prints exactly the same figures (the engines have no
//! shared locale library). Raw values are included next to formatted ones.

use chrono::NaiveDate;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

use crate::{
    auth::AuthUser,
    crm::{
        entity_type_by_id,
        fields::custom_fields,
        items::load_detail,
        money::{line, rows_of, sum},
        ufield_text, COMPANY, CONTACT,
    },
    error::AppResult,
    state::AppState,
};

// ---------------------------------------------------------------------------
// Locale formatting
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Locale {
    decimal: char,
    group: &'static str,
    /// Indian grouping: 12,34,567.89
    lakh: bool,
    symbol_after: bool,
    date: &'static str,
}

/// Number and date conventions for a BCP 47 tag like "de-DE", "hi-IN", "en-US".
pub fn locale(tag: &str) -> Locale {
    let lang = tag.split(['-', '_']).next().unwrap_or("en").to_lowercase();
    let region = tag.split(['-', '_']).nth(1).unwrap_or("").to_uppercase();
    let nbsp = "\u{202f}";
    match (lang.as_str(), region.as_str()) {
        ("en", "IN") | ("hi", _) | ("bn", _) | ("ta", _) | ("te", _) | ("mr", _) | ("gu", _) | ("kn", _) | ("ml", _) => {
            Locale { decimal: '.', group: ",", lakh: true, symbol_after: false, date: "%d/%m/%Y" }
        }
        ("en", "US") | ("en", "") => Locale { decimal: '.', group: ",", lakh: false, symbol_after: false, date: "%b %-d, %Y" },
        ("en", _) => Locale { decimal: '.', group: ",", lakh: false, symbol_after: false, date: "%-d %b %Y" },
        ("de", "CH") | ("fr", "CH") | ("it", "CH") => Locale { decimal: '.', group: "’", lakh: false, symbol_after: false, date: "%d.%m.%Y" },
        ("de", _) | ("nl", _) | ("id", _) | ("tr", _) | ("da", _) | ("el", _) | ("pt", "BR") | ("es", "AR") => {
            Locale { decimal: ',', group: ".", lakh: false, symbol_after: true, date: "%d.%m.%Y" }
        }
        ("es", _) | ("it", _) | ("pt", _) | ("ro", _) | ("hr", _) | ("sl", _) => {
            Locale { decimal: ',', group: ".", lakh: false, symbol_after: true, date: "%d/%m/%Y" }
        }
        ("fr", _) | ("ru", _) | ("pl", _) | ("cs", _) | ("sk", _) | ("sv", _) | ("fi", _) | ("nb", _) | ("no", _) | ("uk", _) | ("hu", _) | ("bg", _) => {
            Locale { decimal: ',', group: nbsp, lakh: false, symbol_after: true, date: "%d.%m.%Y" }
        }
        ("ja", _) | ("zh", _) | ("ko", _) => Locale { decimal: '.', group: ",", lakh: false, symbol_after: false, date: "%Y-%m-%d" },
        ("ar", _) | ("he", _) | ("fa", _) | ("ur", _) => Locale { decimal: '.', group: ",", lakh: false, symbol_after: true, date: "%d/%m/%Y" },
        _ => Locale { decimal: '.', group: ",", lakh: false, symbol_after: false, date: "%Y-%m-%d" },
    }
}

/// ISO 4217 minor units (defaults to 2).
pub fn minor_units(code: &str) -> u32 {
    match code {
        "BIF" | "CLP" | "DJF" | "GNF" | "ISK" | "JPY" | "KMF" | "KRW" | "PYG" | "RWF" | "UGX" | "UYI" | "VND" | "VUV" | "XAF" | "XOF" | "XPF" => 0,
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        _ => 2,
    }
}

fn symbol(code: &str) -> &str {
    match code {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "JPY" | "CNY" => "¥",
        "INR" => "₹",
        "RUB" => "₽",
        "KRW" => "₩",
        "ILS" => "₪",
        "TRY" => "₺",
        "UAH" => "₴",
        "NGN" => "₦",
        "VND" => "₫",
        "PHP" => "₱",
        "THB" => "฿",
        "BRL" => "R$",
        "PLN" => "zł",
        "CHF" => "CHF",
        _ => code,
    }
}

fn group(int: &str, loc: &Locale) -> String {
    let digits: Vec<char> = int.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = digits.len();
    let mut first = true;
    while i > 0 {
        let size = if loc.lakh && !first { 2 } else { 3 };
        let start = i.saturating_sub(size);
        out.push(digits[start..i].iter().collect());
        i = start;
        first = false;
    }
    out.reverse();
    out.join(loc.group)
}

pub fn fmt_number(n: Decimal, decimals: u32, loc: &Locale) -> String {
    let r = n.round_dp_with_strategy(decimals, RoundingStrategy::MidpointAwayFromZero);
    let neg = r.is_sign_negative() && !r.is_zero();
    let s = r.abs().to_string();
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let mut out = group(int, loc);
    if decimals > 0 {
        let mut f = frac.to_string();
        while (f.len() as u32) < decimals {
            f.push('0');
        }
        out.push(loc.decimal);
        out.push_str(&f[..decimals as usize]);
    }
    if neg { format!("-{out}") } else { out }
}

pub fn fmt_money(n: Decimal, currency: &str, loc: &Locale) -> String {
    let num = fmt_number(n, minor_units(currency), loc);
    let sym = symbol(currency);
    let sep = if sym.chars().count() > 1 { "\u{a0}" } else { "" };
    if loc.symbol_after { format!("{num}\u{a0}{sym}") } else { format!("{sym}{sep}{num}") }
}

pub fn fmt_date(d: NaiveDate, loc: &Locale) -> String {
    d.format(loc.date).to_string()
}

fn money(n: Decimal, currency: &str, loc: &Locale) -> Value {
    json!({ "text": fmt_money(n, currency, loc), "value": n.to_string() })
}

// ---------------------------------------------------------------------------
// Building the data
// ---------------------------------------------------------------------------

fn s(v: &Option<String>) -> String {
    v.clone().unwrap_or_default()
}

/// Seller block from company settings.
pub async fn seller(state: &AppState) -> AppResult<(Value, Option<Uuid>)> {
    let st = crate::routes::settings::load(state).await?;
    let g = |k: &str| st.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let logo = Uuid::parse_str(&g("company_logo_file_id")).ok();
    Ok((
        json!({
            "name": g("company_name"),
            "legal_name": if g("company_legal_name").is_empty() { g("company_name") } else { g("company_legal_name") },
            "address": g("company_address"),
            "tax_id": g("company_tax_id"),
            "registration_no": g("company_registration_no"),
            "phone": g("company_phone"),
            "email": g("company_email"),
            "website": g("company_website"),
            "bank_details": g("company_bank_details"),
            "has_logo": logo.is_some(),
        }),
        logo,
    ))
}

fn address_text(a: &Option<Value>) -> String {
    let Some(a) = a else { return String::new() };
    let g = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("");
    let city = [g("postal_code"), g("city")].iter().filter(|x| !x.is_empty()).copied().collect::<Vec<_>>().join(" ");
    [g("line1"), g("line2"), &city, g("region"), g("country")]
        .iter()
        .filter(|x| !x.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(", ")
}

pub struct Built {
    pub data: Value,
    pub logo_file: Option<Uuid>,
}

/// Full data for one CRM record. Checks the caller can read it.
pub async fn build(state: &AppState, auth: &AuthUser, item: Uuid, entity_type: i16, number: &str, title: &str, locale_tag: &str) -> AppResult<Built> {
    let loc = locale(locale_tag);
    let t = entity_type_by_id(state, entity_type).await?;
    let d = load_detail(state, auth, &t, item).await?;
    let r = &d.row;
    let currency = r.currency.clone().unwrap_or_else(|| "USD".into());
    let (seller, logo_file) = seller(state).await?;

    // Who the document is addressed to.
    let primary_contact = d.contacts.first();
    let mut client = Map::new();
    let mf = |kind: &str| d.multifields.iter().find(|m| m.kind == kind).map(|m| m.value.clone()).unwrap_or_default();
    match t.id {
        CONTACT => {
            client.insert("name".into(), json!(r.title));
            client.insert("company".into(), json!(s(&r.company_title)));
        }
        COMPANY => {
            client.insert("name".into(), json!(r.title));
            client.insert("company".into(), json!(r.title));
        }
        _ => {
            let name = r.company_title.clone().or_else(|| primary_contact.map(|c| c.title.clone())).or_else(|| r.company_name.clone()).unwrap_or_else(|| r.title.clone());
            client.insert("name".into(), json!(name));
            client.insert("company".into(), json!(r.company_title.clone().or_else(|| r.company_name.clone()).unwrap_or_default()));
        }
    }
    client.insert("contact_person".into(), json!(primary_contact.map(|c| c.title.clone()).unwrap_or_default()));
    client.insert("address".into(), json!(address_text(&r.address)));
    client.insert("email".into(), json!(mf("email")));
    client.insert("phone".into(), json!(mf("phone")));

    // Company address/contacts, when the client is a linked company.
    if let Some(cid) = r.company_id
        && let Ok(ct) = entity_type_by_id(state, COMPANY).await
            && let Ok(co) = load_detail(state, auth, &ct, cid).await {
                let get = |k: &str| co.multifields.iter().find(|m| m.kind == k).map(|m| m.value.clone()).unwrap_or_default();
                client.insert("address".into(), json!(address_text(&co.row.address)));
                if client["email"] == "" {
                    client.insert("email".into(), json!(get("email")));
                }
                if client["phone"] == "" {
                    client.insert("phone".into(), json!(get("phone")));
                }
            }

    // Legal details from requisites: registered name, legal address, tax numbers.
    let req = crate::crm::requisites::buyer_of(state, item).await?;
    let ids = req.as_ref().map(crate::crm::requisites::tax_ids).unwrap_or_default();
    client.insert("legal_name".into(), json!(req.as_ref().map(|r| r.name.clone()).unwrap_or_else(|| client["name"].as_str().unwrap_or("").to_string())));
    client.insert("tax_ids".into(), json!(ids.iter().map(|(l, v)| json!({ "label": l, "value": v })).collect::<Vec<_>>()));
    client.insert("tax_line".into(), json!(ids.iter().map(|(l, v)| format!("{l} {v}")).collect::<Vec<_>>().join(" · ")));
    client.insert("country".into(), json!(req.as_ref().map(|r| r.country.clone()).unwrap_or_default()));
    client.insert("ids".into(), req.as_ref().map(|r| r.fields.clone()).unwrap_or_else(|| json!({})));
    let bank = req.as_ref().and_then(|r| r.banks.first().map(|b| crate::crm::requisites::bank_lines(&r.country, b))).unwrap_or_default();
    client.insert("bank_line".into(), json!(bank.iter().map(|(l, v)| format!("{l} {v}")).collect::<Vec<_>>().join(" · ")));
    if let Some(r) = &req {
        client.insert("name".into(), json!(r.name));
        if r.address.is_some() {
            client.insert("address".into(), json!(address_text(&r.address)));
        }
    }

    // Products, totals and tax breakdown.
    let rows = rows_of(state, item).await?;
    let mut products = Vec::new();
    let mut by_rate: BTreeMap<String, (Decimal, Decimal)> = BTreeMap::new();
    for (i, p) in rows.iter().enumerate() {
        let l = line(p);
        let rate = p.tax_rate.unwrap_or_default();
        let e = by_rate.entry(rate.normalize().to_string()).or_default();
        e.0 += l.net;
        e.1 += l.tax;
        products.push(json!({
            "n": i + 1,
            "name": p.name,
            "quantity": fmt_number(p.quantity.normalize(), p.quantity.normalize().scale(), &loc),
            "unit": p.unit,
            "price": money(p.price, &currency, &loc),
            "discount": if p.discount_value.is_zero() { String::new() } else if p.discount_kind == "percent" { format!("{}%", p.discount_value.normalize()) } else { fmt_money(p.discount_value, &currency, &loc) },
            "tax_rate": p.tax_rate.map(|t| format!("{}%", t.normalize())).unwrap_or_default(),
            "net": money(l.net, &currency, &loc),
            "tax": money(l.tax, &currency, &loc),
            "total": money(l.total, &currency, &loc),
        }));
    }
    let tot = sum(&rows);
    let has_products = !rows.is_empty();
    let total = if has_products { tot.total } else { r.opportunity };
    let taxes: Vec<Value> = by_rate
        .iter()
        .filter(|(_, (_, tax))| !tax.is_zero())
        .map(|(rate, (base, tax))| json!({ "rate": format!("{rate}%"), "base": money(*base, &currency, &loc), "tax": money(*tax, &currency, &loc) }))
        .collect();

    // Custom fields as text.
    let mut uf = Map::new();
    for f in custom_fields(state, t.id).await? {
        uf.insert(f.code.clone(), json!(ufield_text(&f, r.uf.get(&f.code))));
    }

    let today = chrono::Utc::now().date_naive();
    Ok(Built {
        logo_file,
        data: json!({
            "doc": {
                "title": title,
                "number": number,
                "date": fmt_date(today, &loc),
                "date_iso": today.to_string(),
                "locale": locale_tag,
                "currency": currency,
            },
            "seller": seller,
            "client": Value::Object(client),
            "record": {
                "type": t.code,
                "number": r.number,
                "title": r.title,
                "stage": s(&r.stage_name),
                "amount": money(r.opportunity, &currency, &loc),
                "close_date": r.close_date.map(|d| fmt_date(d, &loc)).unwrap_or_default(),
                "comments": s(&r.comments),
                "source": s(&r.source),
                "responsible": s(&r.assigned_name),
            },
            "contacts": d.contacts.iter().map(|c| json!({ "name": c.title })).collect::<Vec<_>>(),
            "products": products,
            "has_products": has_products,
            "totals": {
                "subtotal": money(if has_products { tot.subtotal } else { r.opportunity }, &currency, &loc),
                "discount": money(tot.discount, &currency, &loc),
                "tax": money(tot.tax, &currency, &loc),
                "total": money(total, &currency, &loc),
                "has_discount": !tot.discount.is_zero(),
                "has_tax": !tot.tax.is_zero(),
            },
            "taxes": taxes,
            "uf": Value::Object(uf),
        }),
    })
}

/// Realistic sample data for template previews, in many scripts so authors see how
/// their fonts cope.
pub fn sample(locale_tag: &str, title: &str) -> Value {
    let loc = locale(locale_tag);
    let cur = "EUR";
    let m = |s: &str| money(s.parse().unwrap(), cur, &loc);
    json!({
        "doc": { "title": title, "number": "INV-2026-0042", "date": fmt_date(chrono::Utc::now().date_naive(), &loc), "date_iso": chrono::Utc::now().date_naive().to_string(), "locale": locale_tag, "currency": cur },
        "seller": { "name": "Acme Ltd", "legal_name": "Acme Trading Ltd", "address": "1 Market Street, 10115 Berlin, DE", "tax_id": "DE123456789", "registration_no": "HRB 12345", "phone": "+49 30 1234567", "email": "billing@acme.example", "website": "acme.example", "bank_details": "IBAN DE89 3704 0044 0532 0130 00 · BIC COBADEFFXXX", "has_logo": false },
        "client": { "name": "Globex Corporation", "company": "Globex Corporation", "contact_person": "Ananya Sharma (अनन्या शर्मा)", "address": "MG Road 12, 560001 Bengaluru, IN", "email": "ananya@globex.example", "phone": "+91 98765 43210",
            "legal_name": "Globex Corporation Pvt Ltd", "country": "IN",
            "tax_ids": [{ "label": "GSTIN", "value": "27AAPFU0939F1ZV" }, { "label": "PAN", "value": "AAPFU0939F" }],
            "tax_line": "GSTIN 27AAPFU0939F1ZV · PAN AAPFU0939F", "bank_line": "Bank HDFC Bank · IFSC HDFC0000123 · Account number 50100012345678", "ids": { "gstin": "27AAPFU0939F1ZV", "pan": "AAPFU0939F" } },
        "record": { "type": "deal", "number": 42, "title": "Annual licence renewal", "stage": "Proposal sent", "amount": m("2832.00"), "close_date": "", "comments": "Payment due within 30 days. شكراً لتعاملكم معنا — 谢谢惠顾 — Спасибо!", "source": "website", "responsible": "Ada Admin" },
        "contacts": [{ "name": "Ananya Sharma" }],
        "products": [
            { "n": 1, "name": "Platform licence (12 months)", "quantity": "3", "unit": "pcs", "price": m("800.00"), "discount": "10%", "tax_rate": "19%", "net": m("2160.00"), "tax": m("410.40"), "total": m("2570.40") },
            { "n": 2, "name": "Onboarding workshop — Ελληνικά, 日本語, עברית", "quantity": "1", "unit": "day", "price": m("220.00"), "discount": "", "tax_rate": "19%", "net": m("220.00"), "tax": m("41.80"), "total": m("261.80") }
        ],
        "has_products": true,
        "totals": { "subtotal": m("2380.00"), "discount": m("240.00"), "tax": m("452.20"), "total": m("2832.20"), "has_discount": true, "has_tax": true },
        "taxes": [{ "rate": "19%", "base": m("2380.00"), "tax": m("452.20") }],
        "uf": {},
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        s.parse().unwrap()
    }

    #[test]
    fn money_in_many_locales() {
        assert_eq!(fmt_money(d("1234.5"), "USD", &locale("en-US")), "$1,234.50");
        assert_eq!(fmt_money(d("1234.5"), "EUR", &locale("de-DE")), "1.234,50\u{a0}€");
        assert_eq!(fmt_money(d("1234567.891"), "INR", &locale("en-IN")), "₹12,34,567.89");
        assert_eq!(fmt_money(d("1234.5"), "JPY", &locale("ja-JP")), "¥1,235");
        assert_eq!(fmt_money(d("1234.5"), "BHD", &locale("ar-BH")), "1,234.500\u{a0}BHD");
        assert_eq!(fmt_money(d("1234.5"), "EUR", &locale("fr-FR")), "1\u{202f}234,50\u{a0}€");
        assert_eq!(fmt_money(d("-5"), "CHF", &locale("de-CH")), "CHF\u{a0}-5.00");
    }

    #[test]
    fn dates_follow_locale() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 7).unwrap();
        assert_eq!(fmt_date(day, &locale("en-US")), "Nov 7, 2026");
        assert_eq!(fmt_date(day, &locale("de-DE")), "07.11.2026");
        assert_eq!(fmt_date(day, &locale("ja")), "2026-11-07");
    }
}
