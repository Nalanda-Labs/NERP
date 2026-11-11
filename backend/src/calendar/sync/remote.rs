//! Talking to an outside CalDAV server (iCloud, Nextcloud, Fastmail, Yahoo, Google…): finding
//! a person's calendars, listing what's in one, reading, writing and deleting events.
//!
//! Connections are pinned to the addresses checked when the host was looked up, so a name
//! can't be pointed at an internal service in between; internal addresses are refused unless
//! the server allows them (`CRM_WEBHOOK_ALLOW_PRIVATE`, `MAIL_ALLOW_PRIVATE`), e.g. for a
//! Nextcloud on the office network.

use base64::Engine;
use reqwest::{Method, StatusCode, Url};

use crate::{
    calendar::dav::{parse_xml, xml_esc, El, CAL, DAV},
    outbound::is_internal,
    state::AppState,
};

#[derive(Clone)]
pub enum Auth {
    Basic(String, String),
    Bearer(String),
}

impl Auth {
    fn header(&self) -> String {
        match self {
            Auth::Basic(u, p) => format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}"))),
            Auth::Bearer(t) => format!("Bearer {t}"),
        }
    }
}

/// An item in a calendar: its address (path) and version.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: String,
    pub etag: String,
}

pub struct Dav {
    state: AppState,
    auth: Auth,
    /// The calendar collection (ends with '/').
    pub url: Url,
}

pub enum PutError {
    /// Changed there meanwhile (412).
    Conflict,
    Other(String),
}

/// A client for one host, pinned to its checked addresses.
async fn client_for(state: &AppState, url: &Url) -> Result<reqwest::Client, String> {
    if !["http", "https"].contains(&url.scheme()) {
        return Err("the address must start with https://".into());
    }
    let host = url.host_str().ok_or("the address needs a host")?.trim_start_matches('[').trim_end_matches(']').to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port)).await.map_err(|_| format!("{host} could not be found"))?.collect();
    if addrs.is_empty() {
        return Err(format!("{host} could not be found"));
    }
    let allow = state.config.webhook_allow_private || state.config.mail_allow_private;
    if !allow && addrs.iter().any(|a| is_internal(a.ip())) {
        return Err(format!("{host} is on an internal network; the server must allow that (CRM_WEBHOOK_ALLOW_PRIVATE=true)"));
    }
    let tls = crate::crm::email::imap::client_config(false)?;
    reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(&host, &addrs)
        .user_agent("NERP calendar sync")
        .build()
        .map_err(|e| e.to_string())
}

struct Reply {
    status: StatusCode,
    etag: Option<String>,
    location: Option<String>,
    body: String,
}

impl Dav {
    pub fn new(state: &AppState, url: &str, auth: Auth) -> Result<Dav, String> {
        let mut url = Url::parse(url.trim()).map_err(|_| "enter the full address, starting with https://".to_string())?;
        if !url.path().ends_with('/') {
            let p = format!("{}/", url.path());
            url.set_path(&p);
        }
        Ok(Dav { state: state.clone(), auth, url })
    }

    async fn call(&self, method: &str, url: &Url, depth: Option<&str>, headers: &[(&str, String)], body: Option<(String, &str)>) -> Result<Reply, String> {
        let client = client_for(&self.state, url).await?;
        let mut req = client.request(Method::from_bytes(method.as_bytes()).unwrap(), url.clone()).header("Authorization", self.auth.header());
        if let Some(d) = depth {
            req = req.header("Depth", d);
        }
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        if let Some((b, ct)) = body {
            req = req.header("Content-Type", ct).body(b);
        }
        let res = req.send().await.map_err(|e| format!("{} could not be reached: {}", url.host_str().unwrap_or(""), if e.is_timeout() { "no answer".to_string() } else { e.to_string() }))?;
        let status = res.status();
        let etag = res.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_string);
        let location = res.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string);
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN && method == "PROPFIND" {
            return Err("the calendar refused the user name or password".into());
        }
        let body = res.text().await.unwrap_or_default();
        Ok(Reply { status, etag, location, body })
    }

    /// PROPFIND, following a few redirects (well-known addresses redirect).
    async fn propfind(&self, url: &Url, depth: &str, props: &str) -> Result<(Url, El), String> {
        let mut at = url.clone();
        for _ in 0..4 {
            let body = format!(r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/"><d:prop>{props}</d:prop></d:propfind>"#);
            let r = self.call("PROPFIND", &at, Some(depth), &[], Some((body, "application/xml; charset=utf-8"))).await?;
            if r.status.is_redirection() {
                let loc = r.location.ok_or("the server redirected without saying where")?;
                at = at.join(&loc).map_err(|_| "bad redirect".to_string())?;
                continue;
            }
            if r.status.as_u16() != 207 {
                return Err(format!("the calendar answered {} at {}", r.status.as_u16(), at.path()));
            }
            let doc = parse_xml(r.body.as_bytes()).ok().flatten().ok_or("the calendar sent something that isn't WebDAV")?;
            return Ok((at, doc));
        }
        Err("too many redirects".into())
    }

    /// Finds the calendars for events behind a server address (or a calendar's own address).
    pub async fn discover(&self) -> Result<Vec<(String, String)>, String> {
        // The address may already be a calendar.
        if let Ok((at, doc)) = self.propfind(&self.url, "0", "<d:resourcetype/><d:displayname/><c:supported-calendar-component-set/>").await {
            if let Some((_, name, cal, events)) = responses(&doc).into_iter().next().map(|r| (r.0, r.1.displayname, r.1.calendar, r.1.events)) {
                if cal && events {
                    return Ok(vec![(at.to_string(), name.unwrap_or_else(|| "Calendar".into()))]);
                }
            }
        }
        let mut principal = None;
        for start in [self.url.clone(), self.url.join("/.well-known/caldav").map_err(|e| e.to_string())?] {
            if let Ok((at, doc)) = self.propfind(&start, "0", "<d:current-user-principal/>").await {
                if let Some(h) = doc.find(DAV, "current-user-principal").and_then(|p| p.find(DAV, "href")) {
                    principal = Some(at.join(h.text.trim()).map_err(|e| e.to_string())?);
                    break;
                }
            }
        }
        let principal = principal.ok_or("no CalDAV account found at that address")?;
        let (at, doc) = self.propfind(&principal, "0", "<c:calendar-home-set/>").await?;
        let home = doc.find(CAL, "calendar-home-set").and_then(|p| p.find(DAV, "href")).ok_or("the account has no calendars")?;
        let home = at.join(home.text.trim()).map_err(|e| e.to_string())?;
        let (at, doc) = self.propfind(&home, "1", "<d:resourcetype/><d:displayname/><c:supported-calendar-component-set/>").await?;
        let mut out = Vec::new();
        for (href, p) in responses(&doc) {
            if p.calendar && p.events {
                let url = at.join(&href).map_err(|e| e.to_string())?;
                out.push((url.to_string(), p.displayname.unwrap_or_else(|| "Calendar".into())));
            }
        }
        if out.is_empty() {
            return Err("the account has no calendars for events".into());
        }
        Ok(out)
    }

    /// Everything in the calendar, with versions.
    pub async fn list(&self) -> Result<Vec<Item>, String> {
        let (_, doc) = self.propfind(&self.url, "1", "<d:getetag/><d:resourcetype/>").await?;
        let me = self.url.path().to_string();
        Ok(responses(&doc)
            .into_iter()
            .filter(|(href, p)| !p.collection && href.trim_end_matches('/') != me.trim_end_matches('/') && p.etag.is_some())
            .map(|(href, p)| Item { id: href, etag: p.etag.unwrap() })
            .collect())
    }

    /// The items' iCalendar data, 50 at a time.
    pub async fn fetch(&self, ids: &[String]) -> Result<Vec<(Item, String)>, String> {
        let mut out = Vec::new();
        for chunk in ids.chunks(50) {
            let hrefs: String = chunk.iter().map(|h| format!("<d:href>{}</d:href>", xml_esc(h))).collect();
            let body = format!(r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-multiget xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop>{hrefs}</c:calendar-multiget>"#);
            let r = self.call("REPORT", &self.url, Some("1"), &[], Some((body, "application/xml; charset=utf-8"))).await?;
            if r.status.as_u16() != 207 {
                return Err(format!("reading events failed ({})", r.status.as_u16()));
            }
            let doc = parse_xml(r.body.as_bytes()).ok().flatten().ok_or("the calendar sent something that isn't WebDAV")?;
            for (href, p) in responses(&doc) {
                if let (Some(etag), Some(data)) = (p.etag, p.data) {
                    out.push((Item { id: href, etag }, data));
                }
            }
        }
        Ok(out)
    }

    fn item_url(&self, id: &str) -> Result<Url, String> {
        self.url.join(id).map_err(|e| e.to_string())
    }

    /// Writes an item (new: `existing` None, at `new_name`). Returns its address and version.
    pub async fn put(&self, existing: Option<&Item>, new_name: &str, ics: String) -> Result<Item, PutError> {
        let url = match existing {
            Some(i) => self.item_url(&i.id),
            None => self.item_url(new_name),
        }
        .map_err(PutError::Other)?;
        let cond = match existing {
            Some(i) => ("If-Match", i.etag.clone()),
            None => ("If-None-Match", "*".to_string()),
        };
        let r = self.call("PUT", &url, None, &[cond], Some((ics, "text/calendar; charset=utf-8"))).await.map_err(PutError::Other)?;
        if r.status == StatusCode::PRECONDITION_FAILED {
            return Err(PutError::Conflict);
        }
        if !r.status.is_success() {
            return Err(PutError::Other(format!("saving the event there failed ({}): {}", r.status.as_u16(), r.body.chars().take(200).collect::<String>())));
        }
        let id = url.path().to_string();
        // Servers that changed what they stored send no ETag: ask for it.
        let etag = match r.etag {
            Some(e) => e,
            None => {
                let (_, doc) = self.propfind(&url, "0", "<d:getetag/>").await.map_err(PutError::Other)?;
                responses(&doc).into_iter().next().and_then(|(_, p)| p.etag).ok_or_else(|| PutError::Other("the calendar didn't say the event's version".into()))?
            }
        };
        Ok(Item { id, etag })
    }

    pub async fn delete(&self, item: &Item) -> Result<(), String> {
        let r = self.call("DELETE", &self.item_url(&item.id)?, None, &[], None).await?;
        if r.status.is_success() || r.status == StatusCode::NOT_FOUND || r.status == StatusCode::GONE {
            Ok(())
        } else {
            Err(format!("deleting the event there failed ({})", r.status.as_u16()))
        }
    }
}

#[derive(Default)]
struct Props {
    etag: Option<String>,
    displayname: Option<String>,
    data: Option<String>,
    collection: bool,
    calendar: bool,
    /// Takes events (or doesn't say).
    events: bool,
}

/// (href, the props that came back 200) of each response.
fn responses(doc: &El) -> Vec<(String, Props)> {
    let mut list = Vec::new();
    doc.find_all(DAV, "response", &mut list);
    list.into_iter()
        .filter_map(|r| {
            let href = r.child(DAV, "href")?.text.trim().to_string();
            let mut p = Props { events: true, ..Default::default() };
            for ps in r.children.iter().filter(|c| c.is(DAV, "propstat")) {
                let ok = ps.child(DAV, "status").is_none_or(|s| s.text.contains(" 200"));
                let Some(prop) = ps.child(DAV, "prop").filter(|_| ok) else { continue };
                if let Some(e) = prop.child(DAV, "getetag") {
                    p.etag = Some(e.text.trim().to_string()).filter(|e| !e.is_empty());
                }
                if let Some(d) = prop.child(DAV, "displayname") {
                    p.displayname = Some(d.text.trim().to_string()).filter(|d| !d.is_empty());
                }
                if let Some(d) = prop.child(CAL, "calendar-data") {
                    p.data = Some(d.text.clone());
                }
                if let Some(rt) = prop.child(DAV, "resourcetype") {
                    p.collection = rt.child(DAV, "collection").is_some();
                    p.calendar = rt.child(CAL, "calendar").is_some();
                }
                if let Some(set) = prop.child(CAL, "supported-calendar-component-set") {
                    p.events = set.children.iter().any(|c| c.attr("name").is_some_and(|n| n.eq_ignore_ascii_case("VEVENT")));
                }
            }
            Some((href, p))
        })
        .collect()
}
