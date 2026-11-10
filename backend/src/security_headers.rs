//! Security headers on every response.
//!
//! - **Content-Security-Policy:** scripts only from NERP itself, plus the app's one inline
//!   bootstrap script, allowed by its hash (read from the built `index.html`, again whenever a
//!   new build replaces it). So
//!   injected markup cannot run script even if it got past the sanitisers.
//! - **Framing:** the app only inside NERP itself (no clickjacking). The pages other sites
//!   embed (web forms, the chat widget, booking and help pages) may be framed anywhere.
//! - `nosniff`, a referrer policy, browser features limited to the app, and HSTS when cookies
//!   are marked secure (that is, behind HTTPS).
//!
//! A response that sets its own policy (uploaded files are served sandboxed) keeps it.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{header, HeaderName, HeaderValue, Uri},
    response::Response,
};
use base64::Engine;
use sha2::{Digest, Sha256};

/// Pages that other websites embed in a frame.
const EMBEDDABLE: &[&str] = &["/f/", "/c/", "/book/", "/help"];

pub struct Policy {
    index: Option<std::path::PathBuf>,
    livekit: String,
    hsts: bool,
    /// The two policies, built from index.html as it was at the time noted (rebuilt when a new
    /// frontend build replaces it, so a deploy needs no restart).
    current: std::sync::RwLock<(Option<std::time::SystemTime>, HeaderValue, HeaderValue)>,
}

/// Hashes of the inline `<script>` blocks of the built index.html, for `script-src`.
fn inline_script_hashes(index_html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = index_html;
    while let Some(start) = rest.find("<script") {
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else { break };
        let tag = &after[..open_end];
        let body_start = open_end + 1;
        let Some(close) = after[body_start..].find("</script>") else { break };
        let body = &after[body_start..body_start + close];
        if !tag.contains("src=") && !body.trim().is_empty() {
            out.push(format!("'sha256-{}'", base64::engine::general_purpose::STANDARD.encode(Sha256::digest(body.as_bytes()))));
        }
        rest = &after[body_start + close..];
    }
    out
}

impl Policy {
    pub fn new(static_dir: Option<&std::path::Path>, livekit_url: Option<&str>, hsts: bool) -> Self {
        // Calls may use a LiveKit server elsewhere (its WebSocket address).
        let livekit = livekit_url
            .and_then(|u| reqwest::Url::parse(u).ok())
            .map(|u| format!(" {}", u.origin().ascii_serialization()))
            .unwrap_or_default();
        let index = static_dir.map(|d| d.join("index.html"));
        let (app, embeddable) = Self::build(index.as_deref(), &livekit);
        let stamp = index.as_deref().and_then(|i| std::fs::metadata(i).and_then(|m| m.modified()).ok());
        Policy { index, livekit, hsts, current: std::sync::RwLock::new((stamp, app, embeddable)) }
    }

    fn build(index: Option<&std::path::Path>, livekit: &str) -> (HeaderValue, HeaderValue) {
        let hashes = index.and_then(|i| std::fs::read_to_string(i).ok()).map(|h| inline_script_hashes(&h)).unwrap_or_default();
        let base = format!(
            "default-src 'self'; script-src 'self' {hashes}; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
             font-src 'self' data: https://fonts.gstatic.com; img-src 'self' data: blob: https:; media-src 'self' blob:; \
             connect-src 'self'{livekit}; worker-src 'self' blob:; \
             frame-src 'self' blob: https://www.youtube-nocookie.com https://www.youtube.com https://player.vimeo.com; \
             object-src 'none'; base-uri 'self'; form-action 'self'",
            hashes = hashes.join(" ")
        );
        (
            HeaderValue::from_str(&format!("{base}; frame-ancestors 'self'")).expect("valid CSP"),
            HeaderValue::from_str(&format!("{base}; frame-ancestors *")).expect("valid CSP"),
        )
    }

    /// The policies for now: rebuilt if index.html changed since they were made.
    fn headers(&self) -> (HeaderValue, HeaderValue) {
        let stamp = self.index.as_deref().and_then(|i| std::fs::metadata(i).and_then(|m| m.modified()).ok());
        {
            let cur = self.current.read().unwrap();
            if cur.0 == stamp {
                return (cur.1.clone(), cur.2.clone());
            }
        }
        let (app, embeddable) = Self::build(self.index.as_deref(), &self.livekit);
        *self.current.write().unwrap() = (stamp, app.clone(), embeddable.clone());
        (app, embeddable)
    }
}

pub async fn apply(State(policy): State<Arc<Policy>>, uri: Uri, mut res: Response) -> Response {
    let embeddable = EMBEDDABLE.iter().any(|p| uri.path().starts_with(p));
    let h = res.headers_mut();
    if !h.contains_key(header::CONTENT_SECURITY_POLICY) {
        let (app, embed) = policy.headers();
        h.insert(header::CONTENT_SECURITY_POLICY, if embeddable { embed } else { app });
    }
    if !embeddable {
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("SAMEORIGIN"));
    }
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("strict-origin-when-cross-origin"));
    h.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(self), microphone=(self), display-capture=(self), geolocation=(self), payment=(), usb=()"),
    );
    h.insert(HeaderName::from_static("cross-origin-opener-policy"), HeaderValue::from_static("same-origin"));
    if policy.hsts {
        h.insert(header::STRICT_TRANSPORT_SECURITY, HeaderValue::from_static("max-age=31536000; includeSubDomains"));
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_only_inline_scripts() {
        let html = r#"<script src="/a.js"></script><script>
			boot();
		</script><script type="module">x()</script>"#;
        let h = inline_script_hashes(html);
        assert_eq!(h.len(), 2);
        assert!(h.iter().all(|x| x.starts_with("'sha256-") && x.ends_with('\'')));
    }
}
