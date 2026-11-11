//! Calling other systems' URLs (automation webhooks, event webhooks) safely:
//! * only http(s) URLs with a host and no credentials in them;
//! * the host is resolved once, every address is checked, and the connection is pinned to
//!   those addresses, so a name cannot point somewhere else between the check and the call;
//! * private, loopback, link-local and similar addresses are refused unless
//!   `CRM_WEBHOOK_ALLOW_PRIVATE=true`;
//! * no redirects, a timeout, and bodies can be signed (HMAC-SHA256).

use hmac::{KeyInit, Mac};
use std::net::{IpAddr, SocketAddr};

use crate::state::AppState;

/// Loopback, private, link-local, carrier-grade NAT, multicast and other special ranges.
pub fn is_internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            v.is_private() || v.is_loopback() || v.is_link_local() || v.is_unspecified() || v.is_broadcast() || v.is_multicast() || v.is_documentation()
                || (o[0] == 100 && (64..128).contains(&o[1])) || o[0] == 0 || o[0] >= 240
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return is_internal(IpAddr::V4(v4));
            }
            let s = v.segments();
            v.is_loopback() || v.is_unspecified() || v.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80
        }
    }
}

/// `sha256=<hex>`: HMAC-SHA256 of the body with the shared secret.
/// Refuses a host that resolves to an internal address (mail servers and other services
/// users name themselves), unless `allow_private`.
pub async fn refuse_internal(host: &str, port: u16, allow_private: bool) -> Result<(), String> {
    if allow_private {
        return Ok(());
    }
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port)).await.map_err(|_| format!("{host} could not be found"))?.collect();
    if addrs.is_empty() {
        return Err(format!("{host} could not be found"));
    }
    if addrs.iter().any(|a| is_internal(a.ip())) {
        return Err(format!("{host} is on an internal network; mail servers there need MAIL_ALLOW_PRIVATE=true on the NERP server"));
    }
    Ok(())
}

pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// A URL other systems may be called at: http(s) with a host and no credentials.
pub fn check_url(url: &str) -> Result<reqwest::Url, String> {
    let u = reqwest::Url::parse(url).map_err(|_| "enter a full URL, starting with https://".to_string())?;
    if !["http", "https"].contains(&u.scheme()) {
        return Err("the URL must start with http:// or https://".into());
    }
    if u.host_str().is_none_or(str::is_empty) {
        return Err("the URL needs a host".into());
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("put credentials in a header, not in the URL".into());
    }
    Ok(u)
}

pub struct Request<'a> {
    pub method: reqwest::Method,
    pub url: reqwest::Url,
    pub headers: Vec<(String, String)>,
    /// Body and its content type (none for GET and DELETE).
    pub body: Option<(String, &'a str)>,
    /// Signs the body into `X-NERP-Signature`.
    pub secret: Option<&'a str>,
    pub timeout_secs: u64,
}

pub struct Response {
    pub status: u16,
    /// The start of the response body.
    pub snippet: String,
    pub summary: String,
}

/// Why a call failed, and whether trying again later may help.
pub struct Failure {
    pub msg: String,
    pub retry: bool,
    pub status: Option<u16>,
    pub snippet: Option<String>,
}

/// An HTTP client pinned to the URL's resolved addresses, refusing internal ones (unless
/// CRM_WEBHOOK_ALLOW_PRIVATE), without redirects.
async fn pinned_client(state: &AppState, url: &reqwest::Url, timeout_secs: u64) -> Result<reqwest::Client, Failure> {
    let fail = |msg: String, retry: bool| Failure { msg, retry, status: None, snippet: None };
    let host = url.host_str().unwrap_or("").trim_start_matches('[').trim_end_matches(']').to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| fail(format!("cannot resolve {host}: {e}"), true))?
        .collect();
    if addrs.is_empty() {
        return Err(fail(format!("cannot resolve {host}"), true));
    }
    if !state.config.webhook_allow_private && addrs.iter().any(|a| is_internal(a.ip())) {
        return Err(fail(format!("{host} is a private network address; only public addresses may be called (see CRM_WEBHOOK_ALLOW_PRIVATE)"), false));
    }
    let tls = crate::crm::email::imap::client_config(false).map_err(|e| fail(e, false))?;
    reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(&host, &addrs)
        .user_agent("NERP/1")
        .build()
        .map_err(|e| fail(e.to_string(), false))
}

/// GETs a URL's whole body (at most `max_bytes`): (content type, body).
pub async fn fetch(state: &AppState, url: &reqwest::Url, headers: &[(String, String)], max_bytes: usize, timeout_secs: u64) -> Result<(String, Vec<u8>), String> {
    let client = pinned_client(state, url, timeout_secs).await.map_err(|f| f.msg)?;
    let mut req = client.get(url.clone());
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let mut res = req.send().await.map_err(|e| if e.is_timeout() { format!("no answer within {timeout_secs} s") } else { e.to_string() })?;
    if !res.status().is_success() {
        return Err(format!("the server answered {}", res.status()));
    }
    let ct = res.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > max_bytes {
            return Err(format!("the file is larger than {} MB", max_bytes / 1_000_000));
        }
    }
    Ok((ct, body))
}

pub async fn send(state: &AppState, r: Request<'_>) -> Result<Response, Failure> {
    let host = r.url.host_str().unwrap_or("").trim_start_matches('[').trim_end_matches(']').to_string();
    let client = pinned_client(state, &r.url, r.timeout_secs).await?;
    let fail = |msg: String, retry: bool| Failure { msg, retry, status: None, snippet: None };
    let method = r.method.clone();
    let mut req = client.request(r.method, r.url.clone());
    for (k, v) in &r.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some((body, content_type)) = r.body {
        if let Some(secret) = r.secret.filter(|s| !s.is_empty()) {
            req = req.header("X-NERP-Signature", sign(secret, body.as_bytes()));
        }
        req = req.header("Content-Type", content_type).body(body);
    }
    let res = req.send().await.map_err(|e| {
        fail(format!("{method} {host} failed: {}", if e.is_timeout() { format!("no answer within {} s", r.timeout_secs) } else { e.to_string() }), true)
    })?;
    let status = res.status();
    let snippet: String = res.text().await.unwrap_or_default().chars().take(500).collect();
    let short: String = snippet.trim().chars().take(200).collect();
    let summary = format!("{method} {host}{} → {status}{}", r.url.path(), if short.is_empty() { String::new() } else { format!(": {short}") });
    if status.is_success() {
        Ok(Response { status: status.as_u16(), snippet, summary })
    } else {
        // Server errors and rate limits may pass; a 4xx means the request itself is wrong.
        Err(Failure { msg: summary, retry: status.is_server_error() || status.as_u16() == 429, status: Some(status.as_u16()), snippet: Some(snippet) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mail_hosts_on_internal_networks_are_refused_unless_allowed() {
        assert!(refuse_internal("localhost", 993, false).await.unwrap_err().contains("MAIL_ALLOW_PRIVATE"));
        assert!(refuse_internal("127.0.0.1", 25, false).await.is_err());
        assert!(refuse_internal("127.0.0.1", 25, true).await.is_ok());
    }

    #[test]
    fn internal_addresses() {
        for ip in ["127.0.0.1", "10.1.2.3", "192.168.0.1", "172.16.5.5", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1"] {
            assert!(is_internal(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(!is_internal(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn signature() {
        // RFC 4231 test case 2.
        assert_eq!(sign("Jefe", b"what do ya want for nothing?"), "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn urls() {
        assert!(check_url("https://example.com/hook").is_ok());
        assert!(check_url("ftp://example.com").is_err());
        assert!(check_url("https://user:pw@example.com").is_err());
        assert!(check_url("example.com").is_err());
    }
}
