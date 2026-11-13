//! CalDAV (RFC 4791, with WebDAV sync, RFC 6578): calendar apps read and change a person's
//! calendar both ways. Apps sign in with the person's email and an app password.
//!
//! Layout (all under `/dav`, found from `/.well-known/caldav`):
//! - `/dav/principals/<user>/`: the person;
//! - `/dav/calendars/<user>/`: their calendars;
//! - `/dav/calendars/<user>/default/`: their calendar: events they organise and events they
//!   are invited to, one `.ics` file each (a repeating event and its changed occurrences in
//!   one file).
//!
//! What an app does goes through NERP's own rules: an event a person is only invited to
//! can't be changed from their app, only answered (or declined by deleting it). Changes from
//! apps don't email guests outside the company; calendar apps send their own invitations.
//! Colleagues hear about events in NERP as usual.

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
};
use base64::Engine;
use chrono::{DateTime, Duration, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use quick_xml::{events::Event as XmlEvent, name::ResolveResult, NsReader};
use redis::AsyncCommands;
use serde_json::json;
use uuid::Uuid;

use super::{exceptions, ical, invites, load_events, occurrences, Event};
use crate::{
    auth::ClientIp,
    notify::{notify, NewNotification},
    state::AppState,
};

pub(crate) const DAV: &str = "DAV:";
pub(crate) const CAL: &str = "urn:ietf:params:xml:ns:caldav";
pub(crate) const CS: &str = "http://calendarserver.org/ns/";
pub(crate) const CARD: &str = "urn:ietf:params:xml:ns:carddav";
const APPLE: &str = "http://apple.com/ns/ical/";
const MAX_BODY: usize = 2 * 1024 * 1024;
/// Sync tokens: `urn:nerp:sync:<number>`.
const TOKEN_PREFIX: &str = "urn:nerp:sync:";
pub const CLEANUP_JOB: &str = "calendar.dav_cleanup";

// ---------------------------------------------------------------------------
// A small XML tree
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub(crate) struct El {
    pub ns: String,
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<El>,
}

impl El {
    pub(crate) fn is(&self, ns: &str, name: &str) -> bool {
        self.ns == ns && self.name == name
    }
    pub(crate) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    pub(crate) fn child(&self, ns: &str, name: &str) -> Option<&El> {
        self.children.iter().find(|c| c.is(ns, name))
    }
    pub(crate) fn find(&self, ns: &str, name: &str) -> Option<&El> {
        if self.is(ns, name) {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(ns, name))
    }
    pub(crate) fn find_all<'a>(&'a self, ns: &str, name: &str, out: &mut Vec<&'a El>) {
        if self.is(ns, name) {
            out.push(self);
        }
        for c in &self.children {
            c.find_all(ns, name, out);
        }
    }
}

pub(crate) fn parse_xml(body: &[u8]) -> Result<Option<El>, ()> {
    let text = std::str::from_utf8(body).map_err(|_| ())?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    let mut r = NsReader::from_str(text);
    r.config_mut().trim_text(true);
    let mut stack: Vec<El> = vec![El::default()];
    loop {
        let (res, ev) = r.read_resolved_event().map_err(|_| ())?;
        let ns = match res {
            ResolveResult::Bound(n) => n.into_inner().to_string(),
            _ => String::new(),
        };
        match ev {
            XmlEvent::Start(e) => {
                stack.push(El { ns, name: e.local_name().as_ref().to_string(), attrs: attrs(&e), ..Default::default() });
                if stack.len() > 32 {
                    return Err(());
                }
            }
            XmlEvent::Empty(e) => {
                let el = El { ns, name: e.local_name().as_ref().to_string(), attrs: attrs(&e), ..Default::default() };
                stack.last_mut().ok_or(())?.children.push(el);
            }
            XmlEvent::End(_) => {
                let done = stack.pop().ok_or(())?;
                stack.last_mut().ok_or(())?.children.push(done);
            }
            XmlEvent::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t.xml_content(quick_xml::XmlVersion::Implicit1_0));
                }
            }
            XmlEvent::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t.into_inner());
                }
            }
            XmlEvent::GeneralRef(g) => {
                let name = g.into_inner().into_owned();
                let c = match name.as_str() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    n if n.starts_with("#x") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
                    n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
                    _ => None,
                };
                if let (Some(c), Some(top)) = (c, stack.last_mut()) {
                    top.text.push(c);
                }
            }
            XmlEvent::Eof => break,
            _ => {}
        }
    }
    let root = stack.pop().ok_or(())?;
    Ok(root.children.into_iter().next())
}

/// Plain attributes (no namespace prefix needed for the ones CalDAV uses).
fn attrs(e: &quick_xml::events::BytesStart) -> Vec<(String, String)> {
    e.attributes()
        .flatten()
        .map(|a| (a.key.local_name().as_ref().to_string(), a.value.to_string()))
        .collect()
}

pub(crate) fn xml_esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `<prefix:name>` for the namespaces we use; others get their own xmlns.
fn tag(ns: &str, name: &str, inner: Option<&str>) -> String {
    let (open, close) = match ns {
        DAV => (format!("d:{name}"), format!("d:{name}")),
        CAL => (format!("c:{name}"), format!("c:{name}")),
        CS => (format!("cs:{name}"), format!("cs:{name}")),
        CARD => (format!("card:{name}"), format!("card:{name}")),
        APPLE => (format!("a:{name}"), format!("a:{name}")),
        "" => (format!("{name} xmlns=\"\""), name.to_string()),
        other => (format!("x:{name} xmlns:x=\"{}\"", xml_esc(other)), format!("x:{name}")),
    };
    match inner {
        Some(i) if !i.is_empty() => format!("<{open}>{i}</{close}>"),
        _ => format!("<{open}/>"),
    }
}

const XML_HEAD: &str = r#"<?xml version="1.0" encoding="utf-8"?>"#;
const NS_DECL: &str = r#"xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:a="http://apple.com/ns/ical/" xmlns:card="urn:ietf:params:xml:ns:carddav""#;

pub(crate) fn xml_response(status: StatusCode, body: String) -> Response {
    let mut r = (status, body).into_response();
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/xml; charset=utf-8"));
    r
}

pub(crate) fn multistatus(responses: &[String], extra: &str) -> Response {
    xml_response(StatusCode::MULTI_STATUS, format!("{XML_HEAD}<d:multistatus {NS_DECL}>{}{extra}</d:multistatus>", responses.concat()))
}

/// A `<d:response>` with the props found (200) and not found (404).
pub(crate) fn prop_response(href: &str, found: &[(String, String, String)], missing: &[(String, String)]) -> String {
    let mut s = format!("<d:response><d:href>{}</d:href>", xml_esc(href));
    if !found.is_empty() {
        s.push_str("<d:propstat><d:prop>");
        for (ns, name, inner) in found {
            s.push_str(&tag(ns, name, Some(inner)));
        }
        s.push_str("</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>");
    }
    if !missing.is_empty() {
        s.push_str("<d:propstat><d:prop>");
        for (ns, name) in missing {
            s.push_str(&tag(ns, name, None));
        }
        s.push_str("</d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>");
    }
    s.push_str("</d:response>");
    s
}

pub(crate) fn status_response(href: &str, status: &str) -> String {
    format!("<d:response><d:href>{}</d:href><d:status>HTTP/1.1 {status}</d:status></d:response>", xml_esc(href))
}

fn error(status: StatusCode, ns: &str, name: &str) -> Response {
    xml_response(status, format!("{XML_HEAD}<d:error {NS_DECL}>{}</d:error>", tag(ns, name, None)))
}

pub(crate) fn plain(status: StatusCode, text: &str) -> Response {
    (status, text.to_string()).into_response()
}

// ---------------------------------------------------------------------------
// Signing in
// ---------------------------------------------------------------------------

fn unauthorized() -> Response {
    let mut r = plain(StatusCode::UNAUTHORIZED, "Sign in with your email and an app password (Profile → App passwords).");
    r.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Basic realm=\"NERP\", charset=\"UTF-8\""));
    r
}

pub(crate) struct Me {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub tz: Tz,
}

/// The person signing in with Basic auth and an app password; too many failures from one
/// address make it wait.
pub(crate) async fn sign_in(state: &AppState, headers: &HeaderMap, ip: Option<&str>) -> Result<Me, Response> {
    let mut redis = state.redis.clone();
    let key = format!("dav:fail:{}", ip.unwrap_or("unknown"));
    let fails: i64 = redis.get(&key).await.unwrap_or(0);
    if fails >= 30 {
        return Err(plain(StatusCode::TOO_MANY_REQUESTS, "Too many failed sign-ins; try again in a few minutes."));
    }
    let creds = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic ").or_else(|| v.strip_prefix("basic ")))
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b.trim()).ok())
        .and_then(|b| String::from_utf8(b).ok());
    let Some((email, password)) = creds.as_deref().and_then(|c| c.split_once(':')) else {
        return Err(unauthorized());
    };
    match crate::auth::app_passwords::verify(state, email, password, ip).await {
        Ok(Some(id)) => {
            // The company's IP address rule applies to calendar apps too.
            if crate::auth::ip::check_user(state, id, ip).await.is_err() {
                return Err(plain(StatusCode::FORBIDDEN, "NERP can't be used from this network."));
            }
            let (email, name, tz): (String, String, String) = sqlx::query!(r#"SELECT email AS "email!", trim(first_name || ' ' || last_name) AS "c1!", timezone AS "timezone!" FROM users WHERE id = $1"#, id).map(|r| (r.email, r.c1, r.timezone))
                .fetch_one(&state.db)
                .await
                .map_err(|_| plain(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong"))?;
            Ok(Me { id, email: email.to_lowercase(), name, tz: tz.parse().unwrap_or(chrono_tz::UTC) })
        }
        Ok(None) => {
            let n: i64 = redis.incr(&key, 1).await.unwrap_or(1);
            if n == 1 {
                let _: Result<(), _> = redis.expire(&key, 900).await;
            }
            Err(unauthorized())
        }
        Err(_) => Err(plain(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong")),
    }
}

// ---------------------------------------------------------------------------
// Where a request points
// ---------------------------------------------------------------------------

enum Target {
    Root,
    Principal(Uuid),
    Home(Uuid),
    Calendar(Uuid),
    Object(Uuid, String),
}

fn target(path: &str) -> Option<Target> {
    let rest = path.strip_prefix("/dav").unwrap_or(path);
    let segs: Vec<String> = rest.split('/').filter(|s| !s.is_empty()).map(|s| percent_decode(s)).collect();
    let id = |s: &str| Uuid::parse_str(s).ok();
    Some(match segs.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => Target::Root,
        ["principals", u] => Target::Principal(id(u)?),
        ["calendars", u] => Target::Home(id(u)?),
        ["calendars", u, "default"] => Target::Calendar(id(u)?),
        ["calendars", u, "default", name] if name.ends_with(".ics") && name.len() <= 255 && !name.contains(['/', '\\']) => Target::Object(id(u)?, name.to_string()),
        _ => return None,
    })
}

pub(crate) fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|c| if c.is_ascii_alphanumeric() || b"-._~@+".contains(&c) { (c as char).to_string() } else { format!("%{c:02X}") })
        .collect()
}

pub(crate) fn principal_href(u: Uuid) -> String {
    format!("/dav/principals/{u}/")
}
fn home_href(u: Uuid) -> String {
    format!("/dav/calendars/{u}/")
}
fn calendar_href(u: Uuid) -> String {
    format!("/dav/calendars/{u}/default/")
}
fn object_href(u: Uuid, name: &str) -> String {
    format!("/dav/calendars/{u}/default/{}", percent_encode(name))
}

// ---------------------------------------------------------------------------
// The calendar's contents
// ---------------------------------------------------------------------------

#[derive(sqlx::FromRow, Clone)]
struct Obj {
    id: Uuid,
    name: String,
    seq: i64,
    owner: bool,
}

impl Obj {
    fn etag(&self) -> String {
        format!("\"{}\"", self.seq)
    }
}

const OBJ_SELECT: &str = "SELECT e.id, CASE WHEN e.owner_id = $1 THEN COALESCE(e.dav_name, e.id::text || '.ics') ELSE e.id::text || '.ics' END AS name,
    e.dav_seq AS seq, e.owner_id = $1 AS owner
    FROM cal_events e WHERE e.parent_id IS NULL AND (e.owner_id = $1 OR EXISTS (SELECT 1 FROM cal_attendees a WHERE a.event_id = e.id AND a.user_id = $1))";

async fn objects(state: &AppState, user: Uuid) -> sqlx::Result<Vec<Obj>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!("{OBJ_SELECT} ORDER BY e.starts_at"))).bind(user).fetch_all(&state.db).await
}

async fn object(state: &AppState, user: Uuid, name: &str) -> sqlx::Result<Option<Obj>> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT * FROM ({OBJ_SELECT}) x WHERE x.name = $2"
    )))
    .bind(user)
    .bind(name)
    .fetch_optional(&state.db)
    .await
}

/// The calendar's version: the newest change to anything in it, or anything that left it.
async fn sync_seq(state: &AppState, user: Uuid) -> sqlx::Result<i64> {
    let a: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT MAX(seq) FROM ({OBJ_SELECT}) x"))).bind(user).fetch_one(&state.db).await?;
    let b: Option<i64> = sqlx::query_scalar!(r#"SELECT MAX(seq) AS "v?" FROM cal_dav_tombstones WHERE user_id = $1"#, user).fetch_one(&state.db).await?;
    Ok(a.unwrap_or(0).max(b.unwrap_or(0)))
}

/// The iCalendar file for an event: the event (or series) and its changed occurrences.
pub(crate) async fn ics_of(state: &AppState, id: Uuid) -> crate::error::AppResult<String> {
    ics_with(state, id, true).await
}

/// `people`: with organiser and guests (left out when sending NERP's events to an outside
/// calendar, so it doesn't invite anyone).
pub(crate) async fn ics_with(state: &AppState, id: Uuid, people: bool) -> crate::error::AppResult<String> {
    let (e, _) = load_events(state, &[id]).await?.into_iter().next().ok_or(crate::error::AppError::NotFound("event"))?;
    let uid = e.uid.clone().unwrap_or_else(|| format!("{}@nerp", e.id));
    let mut master = invites::vevent(state, &e).await?;
    master.uid = uid.clone();
    master.private = e.is_private;
    master.alarm_minutes = e.remind_minutes;
    master.last_modified = Some(e.updated_at);
    master.url = Some(format!("{}/calendar?event={}", state.config.public_url, e.id));
    let child_ids: Vec<Uuid> = sqlx::query_scalar!("SELECT id FROM cal_events WHERE parent_id = $1 ORDER BY recurrence_id", id).fetch_all(&state.db).await?;
    let mut list = vec![];
    let mut moved = vec![];
    for (c, _) in load_events(state, &child_ids).await? {
        let Some(rid) = c.recurrence_id else { continue };
        let mut v = invites::vevent(state, &c).await?;
        v.uid = uid.clone();
        v.rule = None;
        v.exdates = vec![];
        v.recurrence_id = Some(rid);
        v.timezone = e.timezone.clone();
        v.private = c.is_private;
        v.alarm_minutes = c.remind_minutes;
        v.last_modified = Some(c.updated_at);
        moved.push(rid);
        list.push(v);
    }
    master.exdates.retain(|x| !moved.contains(x));
    let mut all = vec![master];
    all.extend(list);
    if !people {
        for v in &mut all {
            v.organizer = None;
            v.attendees.clear();
            v.url = None;
        }
    }
    Ok(ical::calendar(None, None, &all))
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

enum Res<'a> {
    Root,
    Principal,
    Home,
    Calendar(i64),
    Object(&'a Obj, Option<&'a str>),
}

fn privileges() -> String {
    ["read", "write", "write-properties", "write-content", "bind", "unbind", "read-current-user-privilege-set"]
        .iter()
        .map(|p| format!("<d:privilege><d:{p}/></d:privilege>"))
        .collect()
}

/// A property's inner XML, or None when the resource doesn't have it.
fn prop(me: &Me, state: &AppState, r: &Res, ns: &str, name: &str) -> Option<String> {
    let principal = format!("<d:href>{}</d:href>", principal_href(me.id));
    Some(match (ns, name, r) {
        (DAV, "current-user-principal", _) => principal,
        (DAV, "principal-URL", Res::Principal) => principal,
        (DAV, "owner", Res::Home | Res::Calendar(_) | Res::Principal) => principal,
        (DAV, "resourcetype", Res::Root | Res::Home) => "<d:collection/>".into(),
        (DAV, "resourcetype", Res::Principal) => "<d:principal/>".into(),
        (DAV, "resourcetype", Res::Calendar(_)) => "<d:collection/><c:calendar/>".into(),
        (DAV, "resourcetype", Res::Object(..)) => String::new(),
        (DAV, "displayname", Res::Principal) => xml_esc(&me.name),
        (DAV, "displayname", Res::Calendar(_)) => xml_esc(&format!("{} (NERP)", me.name)),
        (DAV, "displayname", Res::Home) => "Calendars".into(),
        (DAV, "displayname", Res::Root) => "NERP".into(),
        (DAV, "current-user-privilege-set", Res::Calendar(_) | Res::Object(..) | Res::Home) => privileges(),
        (DAV, "supported-report-set", Res::Calendar(_)) => ["calendar-query", "calendar-multiget"]
            .iter()
            .map(|r| format!("<d:supported-report><d:report><c:{r}/></d:report></d:supported-report>"))
            .chain(std::iter::once("<d:supported-report><d:report><d:sync-collection/></d:report></d:supported-report>".to_string()))
            .collect(),
        (DAV, "sync-token", Res::Calendar(seq)) => format!("{TOKEN_PREFIX}{seq}"),
        (CS, "getctag", Res::Calendar(seq)) => seq.to_string(),
        (DAV, "getetag", Res::Object(o, _)) => xml_esc(&o.etag()),
        (DAV, "getcontenttype", Res::Object(..)) => "text/calendar; charset=utf-8; component=vevent".into(),
        (CAL, "calendar-data", Res::Object(_, Some(data))) => xml_esc(data),
        (CAL, "calendar-home-set", Res::Principal) => format!("<d:href>{}</d:href>", home_href(me.id)),
        (CARD, "addressbook-home-set", Res::Principal) => format!("<d:href>{}</d:href>", super::carddav::home_href(me.id)),
        (CARD, "principal-address", Res::Principal) => String::new(),
        (CAL, "calendar-user-address-set", Res::Principal) => format!("<d:href>mailto:{}</d:href><d:href>{}</d:href>", xml_esc(&me.email), principal_href(me.id)),
        (CS, "email-address-set", Res::Principal) => format!("<cs:email-address>{}</cs:email-address>", xml_esc(&me.email)),
        (CAL, "supported-calendar-component-set", Res::Calendar(_)) => "<c:comp name=\"VEVENT\"/>".into(),
        (CAL, "supported-calendar-data", Res::Calendar(_)) => "<c:calendar-data content-type=\"text/calendar\" version=\"2.0\"/>".into(),
        (CAL, "calendar-description", Res::Calendar(_)) => xml_esc(&format!("{}'s calendar in NERP", me.name)),
        (CAL, "max-resource-size", Res::Calendar(_)) => MAX_BODY.to_string(),
        (APPLE, "calendar-color", Res::Calendar(_)) => "#1A73E8FF".into(),
        (APPLE, "calendar-order", Res::Calendar(_)) => "1".into(),
        _ => {
            let _ = state;
            return None;
        }
    })
}

/// What allprop (or an empty PROPFIND) returns.
fn default_props(r: &Res) -> Vec<(&'static str, &'static str)> {
    let mut p = vec![(DAV, "resourcetype"), (DAV, "displayname"), (DAV, "current-user-principal")];
    match r {
        Res::Principal => p.extend([(CAL, "calendar-home-set"), (CARD, "addressbook-home-set"), (CAL, "calendar-user-address-set"), (DAV, "principal-URL")]),
        Res::Calendar(_) => p.extend([(CS, "getctag"), (DAV, "sync-token"), (CAL, "supported-calendar-component-set"), (DAV, "current-user-privilege-set"), (APPLE, "calendar-color")]),
        Res::Object(..) => p = vec![(DAV, "resourcetype"), (DAV, "getetag"), (DAV, "getcontenttype")],
        _ => {}
    }
    p
}

/// The props a PROPFIND or REPORT asks for (None: all of the defaults).
pub(crate) fn wanted(body: Option<&El>) -> Option<Vec<(String, String)>> {
    let prop = body.and_then(|b| b.child(DAV, "prop"))?;
    Some(prop.children.iter().map(|c| (c.ns.clone(), c.name.clone())).collect())
}

fn respond_props(me: &Me, state: &AppState, href: &str, r: &Res, want: &Option<Vec<(String, String)>>) -> String {
    let list: Vec<(String, String)> = match want {
        Some(w) => w.clone(),
        None => default_props(r).into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
    };
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for (ns, name) in list {
        match prop(me, state, r, &ns, &name) {
            Some(v) => found.push((ns, name, v)),
            None => missing.push((ns, name)),
        }
    }
    prop_response(href, &found, &missing)
}

// ---------------------------------------------------------------------------
// The handler
// ---------------------------------------------------------------------------

/// `/.well-known/caldav` points apps at the server.
pub async fn well_known() -> Response {
    let mut r = StatusCode::MOVED_PERMANENTLY.into_response();
    r.headers_mut().insert(header::LOCATION, HeaderValue::from_static("/dav/"));
    r
}

pub async fn handle(State(state): State<AppState>, ClientIp(ip): ClientIp, req: Request) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let headers = req.headers().clone();
    if method == Method::OPTIONS {
        let mut r = StatusCode::OK.into_response();
        let h = r.headers_mut();
        h.insert("DAV", HeaderValue::from_static("1, 3, calendar-access, addressbook, extended-mkcol"));
        h.insert(header::ALLOW, HeaderValue::from_static("OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, PROPPATCH, REPORT"));
        return r;
    }
    let me = match sign_in(&state, &headers, ip.as_deref()).await {
        Ok(m) => m,
        Err(r) => return r,
    };
    // Address books (CardDAV) have their own module.
    if path.strip_prefix("/dav").unwrap_or(&path).starts_with("/addressbooks") {
        let body = match axum::body::to_bytes(req.into_body(), MAX_BODY).await {
            Ok(b) => b,
            Err(_) => return plain(StatusCode::PAYLOAD_TOO_LARGE, "Too large"),
        };
        return super::carddav::handle(&state, &me, method.as_str(), &path, &headers, &body).await.unwrap_or_else(|e| {
            tracing::warn!("carddav {method} {path}: {e}");
            plain(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong")
        });
    }
    let Some(t) = target(&path) else {
        return plain(StatusCode::NOT_FOUND, "Not found");
    };
    // Only one's own principal and calendars.
    let owner = match &t {
        Target::Root => None,
        Target::Principal(u) | Target::Home(u) | Target::Calendar(u) | Target::Object(u, _) => Some(*u),
    };
    if owner.is_some_and(|u| u != me.id) {
        return plain(StatusCode::FORBIDDEN, "Only your own calendar is shared with calendar apps.");
    }
    let body = match axum::body::to_bytes(req.into_body(), MAX_BODY).await {
        Ok(b) => b,
        Err(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, CAL, "max-resource-size"),
    };
    let result = match method.as_str() {
        "PROPFIND" => propfind(&state, &me, &t, &headers, &body).await,
        "PROPPATCH" => Ok(proppatch(&path, &body)),
        "REPORT" => report(&state, &me, &t, &body).await,
        "GET" | "HEAD" => get(&state, &me, &t, method == Method::HEAD).await,
        "PUT" => put(&state, &me, &t, &headers, &body).await,
        "DELETE" => delete(&state, &me, &t, &headers).await,
        "MKCALENDAR" | "MKCOL" => Ok(plain(StatusCode::FORBIDDEN, "NERP keeps one calendar per person.")),
        _ => Ok(plain(StatusCode::METHOD_NOT_ALLOWED, "Not supported")),
    };
    result.unwrap_or_else(|e| {
        tracing::warn!("caldav {method} {path}: {e}");
        plain(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong")
    })
}

type Out = anyhow::Result<Response>;

async fn propfind(state: &AppState, me: &Me, t: &Target, headers: &HeaderMap, body: &[u8]) -> Out {
    let Ok(doc) = parse_xml(body) else {
        return Ok(plain(StatusCode::BAD_REQUEST, "Bad XML"));
    };
    // allprop / propname / prop
    let want = wanted(doc.as_ref());
    let depth1 = headers.get("Depth").and_then(|v| v.to_str().ok()).is_some_and(|d| d.trim() == "1" || d.trim().eq_ignore_ascii_case("infinity"));
    let mut out = Vec::new();
    match t {
        Target::Root => {
            out.push(respond_props(me, state, "/dav/", &Res::Root, &want));
            if depth1 {
                out.push(respond_props(me, state, &principal_href(me.id), &Res::Principal, &want));
            }
        }
        Target::Principal(_) => out.push(respond_props(me, state, &principal_href(me.id), &Res::Principal, &want)),
        Target::Home(_) => {
            out.push(respond_props(me, state, &home_href(me.id), &Res::Home, &want));
            if depth1 {
                let seq = sync_seq(state, me.id).await?;
                out.push(respond_props(me, state, &calendar_href(me.id), &Res::Calendar(seq), &want));
            }
        }
        Target::Calendar(_) => {
            let seq = sync_seq(state, me.id).await?;
            out.push(respond_props(me, state, &calendar_href(me.id), &Res::Calendar(seq), &want));
            if depth1 {
                let data_wanted = want.as_ref().is_some_and(|w| w.iter().any(|(n, x)| n == CAL && x == "calendar-data"));
                for o in objects(state, me.id).await? {
                    let data = if data_wanted { Some(ics_of(state, o.id).await?) } else { None };
                    out.push(respond_props(me, state, &object_href(me.id, &o.name), &Res::Object(&o, data.as_deref()), &want));
                }
            }
        }
        Target::Object(_, name) => {
            let Some(o) = object(state, me.id, name).await? else {
                return Ok(plain(StatusCode::NOT_FOUND, "Not found"));
            };
            let data_wanted = want.as_ref().is_some_and(|w| w.iter().any(|(n, x)| n == CAL && x == "calendar-data"));
            let data = if data_wanted { Some(ics_of(state, o.id).await?) } else { None };
            out.push(respond_props(me, state, &object_href(me.id, &o.name), &Res::Object(&o, data.as_deref()), &want));
        }
    }
    Ok(multistatus(&out, ""))
}

/// Apps set colours and order on calendars; NERP keeps its own, and says yes.
fn proppatch(path: &str, body: &[u8]) -> Response {
    let mut names = Vec::new();
    if let Ok(Some(doc)) = parse_xml(body) {
        let mut props = Vec::new();
        doc.find_all(DAV, "prop", &mut props);
        for p in props {
            for c in &p.children {
                names.push(tag(&c.ns, &c.name, None));
            }
        }
    }
    multistatus(&[format!("<d:response><d:href>{}</d:href><d:propstat><d:prop>{}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>", xml_esc(path), names.concat())], "")
}

/// Parses `20260302T090000Z` (a time-range bound).
fn utc_stamp(s: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(s.trim().trim_end_matches('Z'), "%Y%m%dT%H%M%S").ok().map(|t| Utc.from_utc_datetime(&t))
}

async fn report(state: &AppState, me: &Me, t: &Target, body: &[u8]) -> Out {
    let Ok(Some(doc)) = parse_xml(body) else {
        return Ok(plain(StatusCode::BAD_REQUEST, "Bad XML"));
    };
    if !matches!(t, Target::Calendar(_) | Target::Object(..)) {
        return Ok(error(StatusCode::FORBIDDEN, DAV, "supported-report"));
    }
    let want = wanted(Some(&doc));
    let data_wanted = want.as_ref().is_some_and(|w| w.iter().any(|(n, x)| n == CAL && x == "calendar-data"));
    let mut out = Vec::new();
    if doc.is(CAL, "calendar-multiget") {
        let mut hrefs = Vec::new();
        doc.find_all(DAV, "href", &mut hrefs);
        for h in hrefs {
            let href = h.text.trim();
            let path = href.split_once("://").map(|(_, rest)| rest.find('/').map(|i| &rest[i..]).unwrap_or("/")).unwrap_or(href);
            let found = match target(path) {
                Some(Target::Object(u, name)) if u == me.id => object(state, me.id, &name).await?,
                _ => None,
            };
            match found {
                Some(o) => {
                    let data = if data_wanted { Some(ics_of(state, o.id).await?) } else { None };
                    out.push(respond_props(me, state, &object_href(me.id, &o.name), &Res::Object(&o, data.as_deref()), &want));
                }
                None => out.push(status_response(href, "404 Not Found")),
            }
        }
        return Ok(multistatus(&out, ""));
    }
    if doc.is(CAL, "calendar-query") {
        // Only events: a query for to-dos (or anything else under VCALENDAR) finds nothing.
        let mut comps = Vec::new();
        doc.find_all(CAL, "comp-filter", &mut comps);
        let named: Vec<&str> = comps.iter().filter_map(|c| c.attr("name")).collect();
        let only_other = named.len() >= 2 && !named.contains(&"VEVENT");
        let range = doc.find(CAL, "time-range");
        let (from, to) = match range {
            Some(r) => (r.attr("start").and_then(utc_stamp), r.attr("end").and_then(utc_stamp)),
            None => (None, None),
        };
        let objs = if only_other { vec![] } else { objects(state, me.id).await? };
        let objs = match (from, to) {
            (None, None) => objs,
            (f, t) => in_range(state, objs, f.unwrap_or(Utc::now() - Duration::days(36500)), t.unwrap_or(Utc::now() + Duration::days(36500))).await?,
        };
        for o in &objs {
            let data = if data_wanted { Some(ics_of(state, o.id).await?) } else { None };
            out.push(respond_props(me, state, &object_href(me.id, &o.name), &Res::Object(o, data.as_deref()), &want));
        }
        return Ok(multistatus(&out, ""));
    }
    if doc.is(DAV, "sync-collection") {
        let token = doc.child(DAV, "sync-token").map(|t| t.text.trim().to_string()).unwrap_or_default();
        let since: i64 = if token.is_empty() {
            0
        } else {
            match token.strip_prefix(TOKEN_PREFIX).and_then(|n| n.parse::<i64>().ok()) {
                Some(n) => n,
                None => return Ok(error(StatusCode::FORBIDDEN, DAV, "valid-sync-token")),
            }
        };
        // Deletions older than the ones still kept can't be told: start over.
        let purged: Option<serde_json::Value> = sqlx::query_scalar!("SELECT value FROM settings WHERE key = 'cal_dav_purged_seq'").fetch_optional(&state.db).await?;
        if since > 0 && purged.and_then(|v| v.as_i64()).is_some_and(|p| since <= p) {
            return Ok(error(StatusCode::FORBIDDEN, DAV, "valid-sync-token"));
        }
        let now = sync_seq(state, me.id).await?;
        let objs = objects(state, me.id).await?;
        for o in objs.iter().filter(|o| o.seq > since) {
            let data = if data_wanted { Some(ics_of(state, o.id).await?) } else { None };
            out.push(respond_props(me, state, &object_href(me.id, &o.name), &Res::Object(o, data.as_deref()), &want));
        }
        if since > 0 {
            let gone: Vec<String> = sqlx::query_scalar!("SELECT DISTINCT name FROM cal_dav_tombstones WHERE user_id = $1 AND seq > $2", me.id, since).fetch_all(&state.db).await?;
            for g in gone.iter().filter(|g| !objs.iter().any(|o| &o.name == *g)) {
                out.push(status_response(&object_href(me.id, g), "404 Not Found"));
            }
        }
        return Ok(multistatus(&out, &format!("<d:sync-token>{TOKEN_PREFIX}{now}</d:sync-token>")));
    }
    Ok(error(StatusCode::FORBIDDEN, DAV, "supported-report"))
}

/// The objects with an occurrence (or a changed occurrence) overlapping [from, to).
async fn in_range(state: &AppState, objs: Vec<Obj>, from: DateTime<Utc>, to: DateTime<Utc>) -> anyhow::Result<Vec<Obj>> {
    let ids: Vec<Uuid> = objs.iter().map(|o| o.id).collect();
    let events = load_events(state, &ids).await?;
    let ex = exceptions(state, &ids).await?;
    let children: Vec<(Uuid, DateTime<Utc>, DateTime<Utc>)> = sqlx::query!(r#"SELECT parent_id AS "parent_id!", starts_at AS "starts_at!", ends_at AS "ends_at!" FROM cal_events WHERE parent_id = ANY($1)"#, &ids).map(|r| (r.parent_id, r.starts_at, r.ends_at)).fetch_all(&state.db).await?;
    let mut keep = Vec::new();
    for o in objs {
        let Some((e, _)) = events.iter().find(|(e, _)| e.id == o.id) else { continue };
        let excs: Vec<DateTime<Utc>> = ex.iter().filter(|(id, _)| *id == e.id).map(|(_, t)| *t).collect();
        let hit = !occurrences(e, &excs, from, to).is_empty() || children.iter().any(|(p, s, en)| *p == e.id && *s < to && (*en > from || (s == en && *s >= from)));
        if hit {
            keep.push(o);
        }
    }
    Ok(keep)
}

async fn get(state: &AppState, me: &Me, t: &Target, head: bool) -> Out {
    let Target::Object(_, name) = t else {
        return Ok(plain(StatusCode::METHOD_NOT_ALLOWED, "Use PROPFIND or REPORT on collections"));
    };
    let Some(o) = object(state, me.id, name).await? else {
        return Ok(plain(StatusCode::NOT_FOUND, "Not found"));
    };
    let data = ics_of(state, o.id).await?;
    let mut r = if head { StatusCode::OK.into_response() } else { data.into_response() };
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/calendar; charset=utf-8"));
    h.insert(header::ETAG, HeaderValue::from_str(&o.etag())?);
    Ok(r)
}

/// If-Match / If-None-Match against the object as it is now.
fn preconditions(headers: &HeaderMap, existing: Option<&Obj>) -> Option<Response> {
    let get = |h: &str| headers.get(h).and_then(|v| v.to_str().ok()).map(str::trim);
    if let Some(m) = get("If-Match") {
        let ok = existing.is_some_and(|o| m == "*" || m.split(',').any(|x| x.trim() == o.etag() || x.trim().trim_start_matches("W/") == o.etag()));
        if !ok {
            return Some(plain(StatusCode::PRECONDITION_FAILED, "The event changed meanwhile"));
        }
    }
    if let Some(m) = get("If-None-Match") {
        if existing.is_some() && (m == "*" || existing.is_some_and(|o| m.split(',').any(|x| x.trim() == o.etag()))) {
            return Some(plain(StatusCode::PRECONDITION_FAILED, "It exists already"));
        }
    }
    None
}

async fn delete(state: &AppState, me: &Me, t: &Target, headers: &HeaderMap) -> Out {
    let Target::Object(_, name) = t else {
        return Ok(plain(StatusCode::FORBIDDEN, "NERP keeps one calendar per person."));
    };
    let existing = object(state, me.id, name).await?;
    if let Some(r) = preconditions(headers, existing.as_ref()) {
        return Ok(r);
    }
    let Some(o) = existing else {
        return Ok(plain(StatusCode::NOT_FOUND, "Not found"));
    };
    if !o.owner {
        // Removing an invitation from one's calendar declines it.
        super::answer(state, me.id, o.id, "declined").await?;
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let (e, guests) = load_events(state, &[o.id]).await?.remove(0);
    let chat = crate::im::linked_chat_people(state, crate::im::Link::Event(o.id)).await?;
    sqlx::query!("DELETE FROM cal_events WHERE id = $1", o.id).execute(&state.db).await?;
    crate::im::linked_chat_gone(state, chat).await;
    let first: String = me.name.split_whitespace().next().unwrap_or_default().to_string();
    notify(state, &guests, NewNotification { kind: "calendar.cancelled", title: format!("{first} cancelled “{}”", e.title), body: None, link: Some("/calendar".into()), actor: Some(me.id) }).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// Saving an event from an app
// ---------------------------------------------------------------------------

fn invalid(why: &str) -> Response {
    let mut r = error(StatusCode::FORBIDDEN, CAL, "valid-calendar-object-resource");
    r.headers_mut().insert("X-NERP-Reason", HeaderValue::from_str(&why.chars().filter(|c| c.is_ascii() && !c.is_control()).take(200).collect::<String>()).unwrap_or(HeaderValue::from_static("invalid")));
    r
}

async fn put(state: &AppState, me: &Me, t: &Target, headers: &HeaderMap, body: &[u8]) -> Out {
    let Target::Object(_, name) = t else {
        return Ok(plain(StatusCode::METHOD_NOT_ALLOWED, "Put events into the calendar"));
    };
    let Ok(text) = std::str::from_utf8(body) else {
        return Ok(invalid("not UTF-8"));
    };
    let cal = match ical::parse(text) {
        Ok(c) => c,
        Err(e) => return Ok(invalid(&e)),
    };
    if cal.children.iter().any(|c| c.name == "VTODO" || c.name == "VJOURNAL") && !cal.children.iter().any(|c| c.name == "VEVENT") {
        return Ok(error(StatusCode::FORBIDDEN, CAL, "supported-calendar-component"));
    }
    let parsed = match ical::events(&cal, me.tz) {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => return Ok(invalid("no event in it")),
        Err(e) => return Ok(invalid(&e)),
    };
    let uid = parsed[0].uid.clone();
    if parsed.iter().any(|p| p.uid != uid) {
        return Ok(invalid("all events in one file need the same UID"));
    }
    let existing = object(state, me.id, name).await?;
    if let Some(r) = preconditions(headers, existing.as_ref()) {
        return Ok(r);
    }
    // The same UID under another name is a conflict.
    if existing.is_none() {
        let other: Option<(Uuid, Option<String>)> = sqlx::query!(r#"SELECT id, dav_name FROM cal_events WHERE owner_id = $1 AND parent_id IS NULL AND (uid = $2 OR id::text || '@nerp' = $2)"#, me.id, &uid).map(|r| (r.id, r.dav_name))
            .fetch_optional(&state.db)
            .await?;
        if let Some((id, dav_name)) = other {
            let href = object_href(me.id, &dav_name.unwrap_or_else(|| format!("{id}.ics")));
            return Ok(xml_response(StatusCode::FORBIDDEN, format!("{XML_HEAD}<d:error {NS_DECL}><c:no-uid-conflict><d:href>{}</d:href></c:no-uid-conflict></d:error>", xml_esc(&href))));
        }
    }
    let master = parsed.iter().find(|p| p.recurrence_id.is_none()).cloned().unwrap_or_else(|| ical::Parsed { recurrence_id: None, rule: None, ..parsed[0].clone() });
    if let Some(o) = &existing {
        if !o.owner {
            // A guest's app can only answer.
            if let Some((_, _, ps)) = master.attendees.iter().find(|(m, ..)| *m == me.email) {
                let status = ical::status_of(ps);
                if status != "pending" {
                    super::answer(state, me.id, o.id, status).await?;
                }
            }
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
    }
    for p in &parsed {
        if !p.all_day && p.end - p.start > Duration::days(31) {
            return Ok(invalid("an event can last at most 31 days"));
        }
    }
    store(state, me, existing.as_ref().map(|o| o.id), &parsed, Some(name), None, false).await?;
    Ok(if existing.is_some() { StatusCode::NO_CONTENT } else { StatusCode::CREATED }.into_response())
}

/// Saves events read from iCalendar (a calendar app's PUT, or an outside calendar's item) as
/// one NERP event and its changed occurrences: a new one (`existing` None, saved under
/// `name`) or an update. `origin`: the outside account it came from. `keep_people`: leave
/// the guests as they are in NERP (for NERP's own events coming back from outside).
pub(crate) async fn store(state: &AppState, me: &Me, existing: Option<Uuid>, parsed: &[ical::Parsed], name: Option<&str>, origin: Option<Uuid>, keep_people: bool) -> anyhow::Result<Uuid> {
    let master = parsed.iter().find(|p| p.recurrence_id.is_none()).cloned().unwrap_or_else(|| ical::Parsed { recurrence_id: None, rule: None, ..parsed[0].clone() });
    let overrides: Vec<&ical::Parsed> = parsed.iter().filter(|p| p.recurrence_id.is_some() && master.rule.is_some()).collect();
    let mut tx = state.db.begin().await?;
    let (id, old): (Uuid, Option<Event>) = match existing {
        Some(eid) => (eid, load_events(state, &[eid]).await?.into_iter().next().map(|x| x.0)),
        None => {
            let id: Uuid = sqlx::query_scalar!("INSERT INTO cal_events (owner_id, created_by, title, starts_at, ends_at, uid, dav_name, ext_account_id) VALUES ($1, $1, $2, $3, $4, $5, $6, $7) RETURNING id", me.id, &master.summary, master.start, master.end, &master.uid, name, origin)
                .fetch_one(&mut *tx)
                .await?;
            (id, None)
        }
    };
    let room = match &old {
        Some(e) => match e.room_id {
            Some(r) => sqlx::query_scalar!("SELECT name FROM booking_resources WHERE id = $1", r).fetch_optional(&mut *tx).await?,
            None => None,
        },
        None => None,
    };
    save_fields(&mut tx, id, &master, room.as_deref(), old.as_ref()).await?;
    // Removed occurrences: the app's EXDATEs and every changed one.
    let mut gone: Vec<DateTime<Utc>> = master.exdates.clone();
    gone.extend(overrides.iter().filter_map(|o| o.recurrence_id));
    if master.rule.is_some() {
        sqlx::query!("DELETE FROM cal_exceptions WHERE event_id = $1 AND NOT occurrence_start = ANY($2)", id, &gone).execute(&mut *tx).await?;
        for g in &gone {
            sqlx::query!("INSERT INTO cal_exceptions (event_id, occurrence_start) VALUES ($1, $2) ON CONFLICT DO NOTHING", id, g).execute(&mut *tx).await?;
        }
    } else {
        sqlx::query!("DELETE FROM cal_exceptions WHERE event_id = $1", id).execute(&mut *tx).await?;
    }
    // Changed occurrences: one row each (cancelled ones are just removed).
    let keep: Vec<DateTime<Utc>> = overrides.iter().filter(|o| !o.cancelled).filter_map(|o| o.recurrence_id).collect();
    sqlx::query!("DELETE FROM cal_events WHERE parent_id = $1 AND NOT recurrence_id = ANY($2)", id, &keep).execute(&mut *tx).await?;
    let mut people_changes = vec![(id, master.attendees.clone())];
    for o in overrides.iter().filter(|o| !o.cancelled) {
        let rid = o.recurrence_id.unwrap();
        let child: Option<Uuid> = sqlx::query_scalar!("SELECT id FROM cal_events WHERE parent_id = $1 AND recurrence_id = $2", id, rid).fetch_optional(&mut *tx).await?;
        let cid = match child {
            Some(c) => c,
            None => sqlx::query_scalar!("INSERT INTO cal_events (owner_id, created_by, title, starts_at, ends_at, parent_id, recurrence_id) VALUES ($1, $1, $2, $3, $4, $5, $6) RETURNING id", me.id, &o.summary, o.start, o.end, id, rid)
                .fetch_one(&mut *tx)
                .await?,
        };
        let one = ical::Parsed { rule: None, exdates: vec![], ..(*o).clone() };
        save_fields(&mut tx, cid, &one, room.as_deref(), None).await?;
        people_changes.push((cid, o.attendees.clone()));
    }
    let mut invited: Vec<(Uuid, Vec<Uuid>)> = Vec::new();
    if !keep_people {
        for (eid, list) in people_changes {
            invited.push((eid, sync_people(&mut tx, eid, me, &list).await?));
        }
    }
    tx.commit().await?;
    for (eid, who) in invited {
        if who.is_empty() {
            continue;
        }
        if let Some((e, _)) = load_events(state, &[eid]).await?.into_iter().next() {
            super::invite(state, me.id, &e, &who, "invited you to").await;
        }
    }
    Ok(id)
}

/// Writes an app's version of the event's own fields; NERP-only ones (room, colour, CRM
/// record, workgroup) stay. The room's name, which NERP adds to the place, is taken off again.
async fn save_fields(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, id: Uuid, p: &ical::Parsed, room: Option<&str>, old: Option<&Event>) -> anyhow::Result<()> {
    let location = match (p.location.clone(), room) {
        (Some(l), Some(r)) if l == r => None,
        (Some(l), Some(r)) => Some(l.strip_prefix(&format!("{r}, ")).map(str::to_string).unwrap_or(l)),
        (l, _) => l,
    };
    let rule = p.rule.as_ref().map(|r| serde_json::to_value(r).unwrap());
    // Apps send the time zone of the start; all-day events are kept in UTC days.
    let changed = old.is_some_and(|o| o.starts_at != p.start || o.ends_at != p.end || o.title != p.summary || o.location != location || o.recurrence != rule);
    sqlx::query!("UPDATE cal_events SET title = $2, description = $3, location = $4, starts_at = $5, ends_at = $6, all_day = $7, timezone = $8, recurrence = $9,
             remind_minutes = $10, is_private = $11, sequence = sequence + CASE WHEN $12 THEN 1 ELSE 0 END, updated_at = now()
         WHERE id = $1", id, &p.summary, p.description.as_deref(), location, p.start, p.end, p.all_day, &p.timezone, rule, p.alarm_minutes, p.private, changed)
    .execute(&mut **tx)
    .await?;
    if changed && old.is_some_and(|o| o.starts_at != p.start || o.ends_at != p.end) {
        sqlx::query!("UPDATE cal_attendees SET status = 'pending' WHERE event_id = $1", id).execute(&mut **tx).await?;
        sqlx::query!("DELETE FROM cal_reminders_sent WHERE event_id = $1", id).execute(&mut **tx).await?;
    }
    Ok(())
}

/// Matches the app's attendee list: colleagues (by email) become guests, everyone else a
/// guest by email (not emailed: the app sends its own invitations). Returns the colleagues
/// newly invited.
async fn sync_people(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, event: Uuid, me: &Me, list: &[(String, Option<String>, String)]) -> anyhow::Result<Vec<Uuid>> {
    let emails: Vec<String> = list.iter().map(|(m, ..)| m.clone()).filter(|m| *m != me.email).collect();
    let users: Vec<(Uuid, String)> = sqlx::query!(r#"SELECT id AS "id!", lower(email) AS "c1!" FROM users WHERE lower(email) = ANY($1) AND is_active AND id <> $2"#, &emails, me.id).map(|r| (r.id, r.c1)).fetch_all(&mut **tx).await?;
    let ids: Vec<Uuid> = users.iter().map(|(id, _)| *id).collect();
    let before: Vec<Uuid> = sqlx::query_scalar!("SELECT user_id FROM cal_attendees WHERE event_id = $1", event).fetch_all(&mut **tx).await?;
    sqlx::query!("DELETE FROM cal_attendees WHERE event_id = $1 AND NOT user_id = ANY($2)", event, &ids).execute(&mut **tx).await?;
    let mut fresh = Vec::new();
    for id in &ids {
        if !before.contains(id) {
            sqlx::query!("INSERT INTO cal_attendees (event_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING", event, id).execute(&mut **tx).await?;
            fresh.push(*id);
        }
    }
    let outside: Vec<&(String, Option<String>, String)> = list.iter().filter(|(m, ..)| *m != me.email && !users.iter().any(|(_, e)| e == m)).take(100).collect();
    let keep: Vec<String> = outside.iter().map(|(m, ..)| m.clone()).collect();
    sqlx::query!("DELETE FROM cal_email_guests WHERE event_id = $1 AND NOT email = ANY($2)", event, &keep).execute(&mut **tx).await?;
    for (m, name, ps) in outside {
        if m.len() > 254 {
            continue;
        }
        let status = ical::status_of(ps);
        sqlx::query!("INSERT INTO cal_email_guests (event_id, email, name, status) VALUES ($1, $2, $3, $4)
             ON CONFLICT (event_id, email) DO UPDATE SET name = COALESCE(EXCLUDED.name, cal_email_guests.name),
                 status = CASE WHEN EXCLUDED.status = 'pending' THEN cal_email_guests.status ELSE EXCLUDED.status END", event, m, name.as_ref().map(|n| n.chars().take(200).collect::<String>()), status)
        .execute(&mut **tx)
        .await?;
    }
    Ok(fresh)
}

/// Daily: forgets deletions older than 60 days, noting how far, so older sync tokens start over.
pub async fn cleanup(state: &AppState) -> anyhow::Result<()> {
    let purged: Option<i64> = sqlx::query_scalar!(r#"WITH d AS (DELETE FROM cal_dav_tombstones WHERE deleted_at < now() - interval '60 days' RETURNING seq) SELECT MAX(seq) AS "v?" FROM d"#)
        .fetch_one(&state.db)
        .await?;
    if let Some(p) = purged {
        sqlx::query!("INSERT INTO settings (key, value) VALUES ('cal_dav_purged_seq', $1)
             ON CONFLICT (key) DO UPDATE SET value = to_jsonb(GREATEST((settings.value)::text::bigint, ($1)::text::bigint)), updated_at = now()", json!(p))
        .execute(&state.db)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_propfind_bodies() {
        let body = br#"<?xml version="1.0"?><A:propfind xmlns:A="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav"><A:prop><A:current-user-principal/><C:calendar-home-set/><B:x xmlns:B="urn:other"/></A:prop></A:propfind>"#;
        let doc = parse_xml(body).unwrap().unwrap();
        assert!(doc.is(DAV, "propfind"));
        let w = wanted(Some(&doc)).unwrap();
        assert_eq!(w, vec![(DAV.into(), "current-user-principal".into()), (CAL.into(), "calendar-home-set".into()), ("urn:other".into(), "x".into())]);
        assert!(parse_xml(b"").unwrap().is_none());
        assert!(parse_xml(b"<a><b></a>").is_err());
    }

    #[test]
    fn paths() {
        let u = Uuid::new_v4();
        assert!(matches!(target("/dav/"), Some(Target::Root)));
        assert!(matches!(target(&format!("/dav/calendars/{u}/default/")), Some(Target::Calendar(x)) if x == u));
        assert!(matches!(target(&format!("/dav/calendars/{u}/default/a%20b.ics")), Some(Target::Object(_, n)) if n == "a b.ics"));
        assert!(target(&format!("/dav/calendars/{u}/other/")).is_none());
        assert_eq!(percent_encode("a b@c.ics"), "a%20b@c.ics");
        let q = parse_xml(br#"<c:calendar-query xmlns:c="urn:ietf:params:xml:ns:caldav"><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="20260101T000000Z" end='20260201T000000Z'/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#).unwrap().unwrap();
        assert_eq!(q.find(CAL, "time-range").and_then(|r| r.attr("end")), Some("20260201T000000Z"));
    }
}
