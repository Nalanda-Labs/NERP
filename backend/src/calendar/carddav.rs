//! CardDAV (RFC 6352): the company's employee list as an address book in phones and mail
//! programs, kept up to date. Read-only; apps sign in like for CalDAV (email and an app
//! password), found from `/.well-known/carddav`.
//!
//! - `/dav/addressbooks/<user>/`: the person's address books;
//! - `/dav/addressbooks/<user>/employees/`: everyone working at the company (not guests or
//!   people who left), one `.vcf` card each, with name, job title, department, emails and
//!   phones.

use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

use super::dav::{multistatus, parse_xml, percent_decode, plain, principal_href, prop_response, status_response, wanted, xml_esc, Me, CARD, CS, DAV};
use crate::state::AppState;

/// Sync tokens: `urn:nerp:cards:<micros of the last change>-<count>`.
const TOKEN_PREFIX: &str = "urn:nerp:cards:";

pub fn home_href(u: Uuid) -> String {
    format!("/dav/addressbooks/{u}/")
}
fn book_href(u: Uuid) -> String {
    format!("/dav/addressbooks/{u}/employees/")
}
fn card_href(u: Uuid, person: Uuid) -> String {
    format!("/dav/addressbooks/{u}/employees/{person}.vcf")
}

enum Target {
    Home,
    Book,
    Card(Uuid),
}

fn target(path: &str, me: Uuid) -> Option<Target> {
    let rest = path.strip_prefix("/dav").unwrap_or(path);
    let segs: Vec<String> = rest.split('/').filter(|s| !s.is_empty()).map(percent_decode).collect();
    let mine = |s: &str| Uuid::parse_str(s).ok() == Some(me);
    Some(match segs.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["addressbooks", u] if mine(u) => Target::Home,
        ["addressbooks", u, "employees"] if mine(u) => Target::Book,
        ["addressbooks", u, "employees", card] if mine(u) => Target::Card(Uuid::parse_str(card.strip_suffix(".vcf")?).ok()?),
        _ => return None,
    })
}

#[derive(FromRow)]
pub(crate) struct Person {
    pub id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub middle_name: Option<String>,
    pub email: String,
    pub position: Option<String>,
    pub phone: Option<String>,
    pub mobile: Option<String>,
    pub birthday: Option<chrono::NaiveDate>,
    pub updated_at: DateTime<Utc>,
    /// Their departments' names (hidden departments left out).
    pub departments: Vec<String>,
    pub is_active: bool,
}

impl Person {
    fn etag(&self) -> String {
        format!("\"{}\"", self.updated_at.timestamp_micros())
    }
}

/// Everyone in the address book; with `with_gone`, also people who left (to tell apps to
/// drop them).
pub(crate) async fn people(state: &AppState, with_gone: bool) -> sqlx::Result<Vec<Person>> {
    sqlx::query_as!(Person, r#"SELECT u.id AS "id!", u.first_name AS "first_name!", u.last_name AS "last_name!", u.middle_name AS "middle_name?", u.email AS "email!", u.position AS "position?", u.phone AS "phone?", u.mobile AS "mobile?", u.birthday AS "birthday?",
                GREATEST(u.updated_at, COALESCE((SELECT MAX(d.updated_at) FROM user_departments ud JOIN departments d ON d.id = ud.department_id WHERE ud.user_id = u.id), u.updated_at)) AS "updated_at!",
                COALESCE((SELECT array_agg(d.name ORDER BY d.name) FROM user_departments ud JOIN departments d ON d.id = ud.department_id WHERE ud.user_id = u.id AND NOT d.hidden), '{}') AS "departments!",
                (u.is_active AND NOT u.join_pending) AS "is_active!"
         FROM users u
         WHERE NOT u.is_guest AND NOT u.is_system AND ($1 OR (u.is_active AND NOT u.join_pending))
         ORDER BY u.last_name, u.first_name"#, with_gone)
    .fetch_all(&state.db)
    .await
}

fn vesc(s: &str) -> String {
    s.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace('\n', "\\n")
}

/// Lines longer than 75 octets folded, as vCard wants.
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut len = 0;
    for c in line.chars() {
        if len + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            len = 1;
        }
        out.push(c);
        len += c.len_utf8();
    }
    out.push_str("\r\n");
    out
}

/// A person's card (vCard 3.0, which every address book reads).
pub(crate) fn vcard(p: &Person, company: &str) -> String {
    let mut lines = vec![
        "BEGIN:VCARD".to_string(),
        "VERSION:3.0".to_string(),
        format!("UID:urn:uuid:{}", p.id),
        format!("N:{};{};{};;", vesc(&p.last_name), vesc(&p.first_name), vesc(p.middle_name.as_deref().unwrap_or(""))),
        format!("FN:{}", vesc(&format!("{} {}", p.first_name, p.last_name).trim().to_string())),
    ];
    let org = std::iter::once(vesc(company)).chain(p.departments.first().map(|d| vesc(d))).collect::<Vec<_>>().join(";");
    if !company.is_empty() || !p.departments.is_empty() {
        lines.push(format!("ORG:{org}"));
    }
    if let Some(t) = p.position.as_deref().filter(|t| !t.trim().is_empty()) {
        lines.push(format!("TITLE:{}", vesc(t)));
    }
    lines.push(format!("EMAIL;TYPE=INTERNET,WORK:{}", vesc(&p.email)));
    if let Some(t) = p.phone.as_deref().filter(|t| !t.trim().is_empty()) {
        lines.push(format!("TEL;TYPE=WORK,VOICE:{}", vesc(t)));
    }
    if let Some(t) = p.mobile.as_deref().filter(|t| !t.trim().is_empty()) {
        lines.push(format!("TEL;TYPE=CELL:{}", vesc(t)));
    }
    if let Some(b) = p.birthday {
        lines.push(format!("BDAY:{}", b.format("%Y-%m-%d")));
    }
    lines.push("CATEGORIES:Employees".into());
    lines.push(format!("REV:{}", p.updated_at.format("%Y%m%dT%H%M%SZ")));
    lines.push("END:VCARD".into());
    lines.iter().map(|l| fold(l)).collect()
}

pub(crate) async fn company(state: &AppState) -> String {
    crate::routes::auth::company_name(state).await
}

fn token(list: &[Person]) -> String {
    let last = list.iter().map(|p| p.updated_at.timestamp_micros()).max().unwrap_or(0);
    format!("{TOKEN_PREFIX}{last}-{}", list.iter().filter(|p| p.is_active).count())
}

fn book_props(me: &Me, ctag: &str, ns: &str, name: &str) -> Option<String> {
    Some(match (ns, name) {
        (DAV, "resourcetype") => "<d:collection/><card:addressbook/>".into(),
        (DAV, "displayname") => "Employees".into(),
        (DAV, "current-user-principal") | (DAV, "owner") => format!("<d:href>{}</d:href>", principal_href(me.id)),
        (DAV, "sync-token") => xml_esc(ctag),
        (CS, "getctag") => xml_esc(ctag),
        (DAV, "current-user-privilege-set") => "<d:privilege><d:read/></d:privilege><d:privilege><d:read-current-user-privilege-set/></d:privilege>".into(),
        (DAV, "supported-report-set") => ["<card:addressbook-query/>", "<card:addressbook-multiget/>", "<d:sync-collection/>"]
            .iter()
            .map(|r| format!("<d:supported-report><d:report>{r}</d:report></d:supported-report>"))
            .collect(),
        (CARD, "addressbook-description") => "Everyone working at the company (NERP)".into(),
        (CARD, "supported-address-data") => "<card:address-data-type content-type=\"text/vcard\" version=\"3.0\"/>".into(),
        (CARD, "max-resource-size") => "102400".into(),
        _ => return None,
    })
}

fn card_props(p: &Person, data: Option<&str>, ns: &str, name: &str) -> Option<String> {
    Some(match (ns, name) {
        (DAV, "resourcetype") => String::new(),
        (DAV, "getetag") => xml_esc(&p.etag()),
        (DAV, "getcontenttype") => "text/vcard; charset=utf-8".into(),
        (DAV, "displayname") => xml_esc(&format!("{} {}", p.first_name, p.last_name)),
        (CARD, "address-data") => xml_esc(data?),
        _ => return None,
    })
}

fn respond(href: &str, want: &[(String, String)], f: impl Fn(&str, &str) -> Option<String>) -> String {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for (ns, name) in want {
        match f(ns, name) {
            Some(v) => found.push((ns.clone(), name.clone(), v)),
            None => missing.push((ns.clone(), name.clone())),
        }
    }
    prop_response(href, &found, &missing)
}

fn defaults_book() -> Vec<(String, String)> {
    [(DAV, "resourcetype"), (DAV, "displayname"), (CS, "getctag"), (DAV, "sync-token"), (DAV, "current-user-privilege-set")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}
fn defaults_card() -> Vec<(String, String)> {
    [(DAV, "resourcetype"), (DAV, "getetag"), (DAV, "getcontenttype")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

pub async fn handle(state: &AppState, me: &Me, method: &str, path: &str, headers: &HeaderMap, body: &[u8]) -> anyhow::Result<Response> {
    let Some(t) = target(path, me.id) else {
        return Ok(plain(StatusCode::NOT_FOUND, "Not found"));
    };
    match method {
        "PROPFIND" => {
            let Ok(doc) = parse_xml(body) else { return Ok(plain(StatusCode::BAD_REQUEST, "Bad XML")) };
            let want = wanted(doc.as_ref());
            let depth1 = headers.get("Depth").and_then(|v| v.to_str().ok()).is_some_and(|d| d.trim() == "1" || d.trim().eq_ignore_ascii_case("infinity"));
            let list = people(state, false).await?;
            let ctag = token(&list);
            let mut out = Vec::new();
            match t {
                Target::Home => {
                    let w = want.clone().unwrap_or_else(|| vec![(DAV.into(), "resourcetype".into()), (DAV.into(), "displayname".into())]);
                    out.push(respond(&home_href(me.id), &w, |ns, name| match (ns, name) {
                        (DAV, "resourcetype") => Some("<d:collection/>".into()),
                        (DAV, "displayname") => Some("Address books".into()),
                        (DAV, "current-user-principal") | (DAV, "owner") => Some(format!("<d:href>{}</d:href>", principal_href(me.id))),
                        _ => None,
                    }));
                    if depth1 {
                        out.push(respond(&book_href(me.id), &want.clone().unwrap_or_else(defaults_book), |ns, n| book_props(me, &ctag, ns, n)));
                    }
                }
                Target::Book => {
                    out.push(respond(&book_href(me.id), &want.clone().unwrap_or_else(defaults_book), |ns, n| book_props(me, &ctag, ns, n)));
                    if depth1 {
                        let w = want.clone().unwrap_or_else(defaults_card);
                        let co = company(state).await;
                        let data_wanted = w.iter().any(|(n, x)| n == CARD && x == "address-data");
                        for p in &list {
                            let data = data_wanted.then(|| vcard(p, &co));
                            out.push(respond(&card_href(me.id, p.id), &w, |ns, n| card_props(p, data.as_deref(), ns, n)));
                        }
                    }
                }
                Target::Card(id) => {
                    let Some(p) = list.iter().find(|p| p.id == id) else { return Ok(plain(StatusCode::NOT_FOUND, "Not found")) };
                    let w = want.unwrap_or_else(defaults_card);
                    let data = w.iter().any(|(n, x)| n == CARD && x == "address-data").then(|| String::new());
                    let co = company(state).await;
                    let data = data.map(|_| vcard(p, &co));
                    out.push(respond(&card_href(me.id, p.id), &w, |ns, n| card_props(p, data.as_deref(), ns, n)));
                }
            }
            Ok(multistatus(&out, ""))
        }
        "REPORT" => {
            if !matches!(t, Target::Book) {
                return Ok(plain(StatusCode::FORBIDDEN, "Reports run on the address book"));
            }
            let Ok(Some(doc)) = parse_xml(body) else { return Ok(plain(StatusCode::BAD_REQUEST, "Bad XML")) };
            let w = wanted(Some(&doc)).unwrap_or_else(defaults_card);
            let co = company(state).await;
            let data_wanted = w.iter().any(|(n, x)| n == CARD && x == "address-data");
            if doc.is(CARD, "addressbook-multiget") {
                let list = people(state, false).await?;
                let mut hrefs = Vec::new();
                doc.find_all(DAV, "href", &mut hrefs);
                let mut out = Vec::new();
                for h in hrefs {
                    let href = h.text.trim();
                    let id = href.rsplit('/').next().and_then(|n| n.strip_suffix(".vcf")).and_then(|n| Uuid::parse_str(&percent_decode(n)).ok());
                    match id.and_then(|i| list.iter().find(|p| p.id == i)) {
                        Some(p) => {
                            let data = data_wanted.then(|| vcard(p, &co));
                            out.push(respond(href, &w, |ns, n| card_props(p, data.as_deref(), ns, n)));
                        }
                        None => out.push(status_response(href, "404 Not Found")),
                    }
                }
                return Ok(multistatus(&out, ""));
            }
            if doc.is(CARD, "addressbook-query") {
                // Filters are left to the app: everyone is returned.
                let list = people(state, false).await?;
                let out: Vec<String> = list
                    .iter()
                    .map(|p| {
                        let data = data_wanted.then(|| vcard(p, &co));
                        respond(&card_href(me.id, p.id), &w, |ns, n| card_props(p, data.as_deref(), ns, n))
                    })
                    .collect();
                return Ok(multistatus(&out, ""));
            }
            if doc.is(DAV, "sync-collection") {
                // Cards changed since the token (all of them for a first sync), and those who
                // left as gone.
                let since: Option<i64> = doc
                    .child(DAV, "sync-token")
                    .map(|t| t.text.trim())
                    .and_then(|t| t.strip_prefix(TOKEN_PREFIX))
                    .and_then(|t| t.split('-').next())
                    .and_then(|t| t.parse().ok());
                let all = people(state, true).await?;
                let mut out = Vec::new();
                for p in &all {
                    if since.is_some_and(|s| p.updated_at.timestamp_micros() <= s) {
                        continue;
                    }
                    if !p.is_active {
                        if since.is_some() {
                            out.push(status_response(&card_href(me.id, p.id), "404 Not Found"));
                        }
                        continue;
                    }
                    let data = data_wanted.then(|| vcard(p, &co));
                    out.push(respond(&card_href(me.id, p.id), &w, |ns, n| card_props(p, data.as_deref(), ns, n)));
                }
                let current = token(&people(state, false).await?);
                return Ok(multistatus(&out, &format!("<d:sync-token>{}</d:sync-token>", xml_esc(&current))));
            }
            Ok(plain(StatusCode::FORBIDDEN, "Unsupported report"))
        }
        "GET" | "HEAD" => {
            let Target::Card(id) = t else { return Ok(plain(StatusCode::METHOD_NOT_ALLOWED, "Not a card")) };
            let list = people(state, false).await?;
            let Some(p) = list.iter().find(|p| p.id == id) else { return Ok(plain(StatusCode::NOT_FOUND, "Not found")) };
            let card = vcard(p, &company(state).await);
            let mut r = if method == "HEAD" { StatusCode::OK.into_response() } else { card.into_response() };
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/vcard; charset=utf-8"));
            r.headers_mut().insert(header::ETAG, HeaderValue::from_str(&p.etag()).unwrap());
            Ok(r)
        }
        "PUT" | "DELETE" | "MKCOL" | "PROPPATCH" => Ok(plain(StatusCode::FORBIDDEN, "The employee list is kept by NERP; it can't be changed from an app.")),
        _ => Ok(plain(StatusCode::METHOD_NOT_ALLOWED, "Not supported")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards() {
        let p = Person {
            id: Uuid::nil(), first_name: "Ann".into(), last_name: "Lee, Jr".into(), middle_name: None, email: "ann@acme.example".into(),
            position: Some("Head of sales; EMEA".into()), phone: Some("+1 555 0100".into()), mobile: None, birthday: None,
            updated_at: DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z").unwrap().with_timezone(&Utc), departments: vec!["Sales".into()], is_active: true,
        };
        let v = vcard(&p, "Acme");
        assert!(v.contains("N:Lee\\, Jr;Ann;;;\r\n"));
        assert!(v.contains("ORG:Acme;Sales\r\n"));
        assert!(v.contains("TITLE:Head of sales\\; EMEA\r\n"));
        assert!(v.contains("TEL;TYPE=WORK,VOICE:+1 555 0100\r\n"));
        assert!(v.contains("REV:20260102T030405Z"));
        assert_eq!(fold(&"x".repeat(80)), format!("{}\r\n {}\r\n", "x".repeat(75), "x".repeat(5)));
    }
}
