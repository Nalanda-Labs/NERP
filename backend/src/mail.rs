//! Outgoing email. Handlers never send directly: they enqueue an `email.send` job
//! (see `jobs`), so a slow or unavailable mail server never blocks a request and
//! failed deliveries are retried.

use anyhow::{Context, Result};
use lettre::{
    message::{Mailbox, MultiPart},
    transport::smtp::{
        authentication::Credentials,
        client::{Tls, TlsParameters},
    },
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
use serde::{Deserialize, Serialize};

use crate::config::SmtpConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Email {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: String,
    /// Stored files to attach (ids in the `files` table).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<uuid::Uuid>,
    /// An iCalendar invitation, update or cancellation (with its METHOD), sent as a calendar
    /// part mail apps show as "Accept / Decline", and as invite.ics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<String>,
}

/// A file attached to an outgoing email.
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl Mailer {
    pub fn new(cfg: &SmtpConfig) -> Result<Self> {
        let tls_params = || {
            TlsParameters::builder(cfg.host.clone())
                .dangerous_accept_invalid_certs(cfg.accept_invalid_certs)
                // "Invalid" covers a certificate for another name too (e.g. a self-signed one
                // without the address among its names).
                .dangerous_accept_invalid_hostnames(cfg.accept_invalid_certs)
                .build()
        };
        let tls = match cfg.security.as_str() {
            "tls" => Tls::Wrapper(tls_params()?),
            "starttls" => Tls::Required(tls_params()?),
            _ => Tls::None,
        };
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.host)
            .port(cfg.port)
            .tls(tls);
        if let (Some(user), Some(pass)) = (&cfg.username, &cfg.password) {
            builder = builder.credentials(Credentials::new(user.clone(), pass.clone()));
        }
        Ok(Self {
            transport: builder.build(),
            from: cfg.from.parse().context("MAIL_FROM is not a valid mailbox")?,
        })
    }

    /// Connects to the mail server (and signs in, when there are credentials) without sending.
    pub async fn test_connection(&self) -> Result<bool> {
        Ok(self.transport.test_connection().await?)
    }

    pub async fn send(&self, email: &Email, attachments: Vec<Attachment>) -> Result<()> {
        let builder = Message::builder()
            .from(self.from.clone())
            .to(email.to.parse().with_context(|| format!("invalid recipient {}", email.to))?)
            .subject(&email.subject);
        let mut body = MultiPart::alternative_plain_html(email.text.clone(), email.html.clone());
        let mut attachments = attachments;
        if let Some(ics) = &email.calendar {
            let method = ics.lines().find_map(|l| l.strip_prefix("METHOD:")).unwrap_or("PUBLISH").trim().to_string();
            let ctype = lettre::message::header::ContentType::parse(&format!("text/calendar; charset=utf-8; method={method}"))?;
            body = body.singlepart(lettre::message::SinglePart::builder().header(ctype).body(ics.clone()));
            attachments.push(Attachment { name: "invite.ics".into(), mime: "application/ics".into(), bytes: ics.clone().into_bytes() });
        }
        let message = if attachments.is_empty() {
            builder.multipart(body)?
        } else {
            let mut mixed = MultiPart::mixed().multipart(body);
            for a in attachments {
                let mime = lettre::message::header::ContentType::parse(&a.mime).unwrap_or(lettre::message::header::ContentType::parse("application/octet-stream")?);
                mixed = mixed.singlepart(lettre::message::Attachment::new(a.name).body(a.bytes, mime));
            }
            builder.multipart(mixed)?
        };
        self.transport.send(message).await.context("SMTP delivery failed")?;
        Ok(())
    }
}

/// A marketing email: shown as from `from_name` (on the configured address), with a reply-to
/// and one-click unsubscribe headers (RFC 8058) so mail clients offer "Unsubscribe".
pub struct BulkEmail<'a> {
    pub to: &'a str,
    pub from_name: Option<&'a str>,
    pub reply_to: Option<&'a str>,
    pub subject: &'a str,
    pub text: String,
    pub html: String,
    pub unsubscribe_url: &'a str,
}

#[derive(Clone)]
struct ListUnsubscribe(String);
impl lettre::message::header::Header for ListUnsubscribe {
    fn name() -> lettre::message::header::HeaderName {
        lettre::message::header::HeaderName::new_from_ascii_str("List-Unsubscribe")
    }
    fn parse(s: &str) -> std::result::Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self(s.to_string()))
    }
    fn display(&self) -> lettre::message::header::HeaderValue {
        lettre::message::header::HeaderValue::new(Self::name(), self.0.clone())
    }
}

#[derive(Clone)]
struct ListUnsubscribePost;
impl lettre::message::header::Header for ListUnsubscribePost {
    fn name() -> lettre::message::header::HeaderName {
        lettre::message::header::HeaderName::new_from_ascii_str("List-Unsubscribe-Post")
    }
    fn parse(_: &str) -> std::result::Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self)
    }
    fn display(&self) -> lettre::message::header::HeaderValue {
        lettre::message::header::HeaderValue::new(Self::name(), "List-Unsubscribe=One-Click".into())
    }
}

impl Mailer {
    pub async fn send_bulk(&self, e: &BulkEmail<'_>) -> Result<()> {
        let from = Mailbox::new(e.from_name.map(str::to_string).or(self.from.name.clone()), self.from.email.clone());
        let mut builder = Message::builder()
            .from(from)
            .to(e.to.parse().with_context(|| format!("invalid recipient {}", e.to))?)
            .subject(e.subject)
            .header(ListUnsubscribe(format!("<{}>", e.unsubscribe_url)))
            .header(ListUnsubscribePost);
        if let Some(r) = e.reply_to.filter(|r| !r.is_empty()) {
            builder = builder.reply_to(r.parse().with_context(|| format!("invalid reply-to {r}"))?);
        }
        let message = builder.multipart(MultiPart::alternative_plain_html(e.text.clone(), e.html.clone()))?;
        self.transport.send(message).await.context("SMTP delivery failed")?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Wraps content in a simple, email-client-safe layout. `paragraphs` are plain text.
fn layout(company: &str, heading: &str, paragraphs: &[&str], button: Option<(&str, &str)>, footer: &str) -> String {
    let body: String = paragraphs
        .iter()
        .map(|p| format!(r#"<p style="margin:0 0 16px;font-size:15px;line-height:1.6;color:#334155">{}</p>"#, esc(p).replace('\n', "<br>")))
        .collect();
    let button = button
        .map(|(label, url)| {
            format!(
                r#"<p style="margin:24px 0"><a href="{url}" style="display:inline-block;background:#4f46e5;color:#ffffff;text-decoration:none;font-weight:600;font-size:15px;padding:12px 22px;border-radius:8px">{label}</a></p>
<p style="margin:0 0 16px;font-size:13px;line-height:1.6;color:#64748b">Or paste this link into your browser:<br><a href="{url}" style="color:#4f46e5;word-break:break-all">{url}</a></p>"#,
                url = esc(url),
                label = esc(label)
            )
        })
        .unwrap_or_default();
    format!(
        r#"<!doctype html><html><body style="margin:0;background:#f1f5f9;font-family:-apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="padding:32px 12px"><tr><td align="center">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="max-width:560px;background:#ffffff;border-radius:12px;border:1px solid #e2e8f0">
<tr><td style="padding:28px 32px 8px"><span style="display:inline-block;background:#4f46e5;color:#fff;font-weight:700;border-radius:8px;width:32px;height:32px;line-height:32px;text-align:center">N</span>
<span style="font-weight:600;font-size:16px;color:#0f172a;margin-left:8px;vertical-align:middle">{company}</span></td></tr>
<tr><td style="padding:16px 32px 24px"><h1 style="margin:0 0 16px;font-size:20px;color:#0f172a">{heading}</h1>{body}{button}</td></tr>
<tr><td style="padding:16px 32px;border-top:1px solid #e2e8f0;font-size:12px;color:#94a3b8">{footer}</td></tr>
</table></td></tr></table></body></html>"#,
        company = esc(company),
        heading = esc(heading),
        footer = esc(footer),
    )
}

pub fn password_reset(to: &str, first_name: &str, company: &str, link: &str) -> Email {
    let hello = format!("Hi {first_name},");
    let lines = [
        hello.as_str(),
        "Someone asked to reset the password for your NERP account. If it was you, use the button below to choose a new password. The link works once and expires in 1 hour.",
    ];
    Email {
        to: to.into(),
        subject: format!("Reset your {company} password"),
        text: format!(
            "{}\n\n{}\n\n{link}\n\nIf you did not ask for this, you can ignore this email; your password stays the same.",
            lines[0], lines[1]
        ),
        html: layout(
            company,
            "Reset your password",
            &lines,
            Some(("Choose a new password", link)),
            "If you did not ask for this, you can ignore this email; your password stays the same.",
        ),
        attachments: vec![],
        calendar: None,
    }
}

pub fn invitation(to: &str, first_name: &str, company: &str, inviter: &str, link: &str) -> Email {
    let hello = format!("Hi {first_name},");
    let line = format!("{inviter} has invited you to join {company} on NERP. Accept the invitation to choose your password and sign in. The link expires in 7 days.");
    Email {
        to: to.into(),
        subject: format!("{inviter} invited you to {company}"),
        text: format!("{hello}\n\n{line}\n\n{link}"),
        html: layout(
            company,
            &format!("Join {company}"),
            &[&hello, &line],
            Some(("Accept invitation", link)),
            "If you were not expecting this invitation, you can ignore this email.",
        ),
        attachments: vec![],
        calendar: None,
    }
}

pub fn test_message(to: &str, company: &str) -> Email {
    let line = "This is a test message from NERP. If you can read it, outgoing email is configured correctly.";
    Email {
        to: to.into(),
        subject: format!("{company}: test email"),
        text: line.into(),
        html: layout(company, "Email works", &[line], None, "Sent from the NERP administration panel."),
        attachments: vec![],
        calendar: None,
    }
}

/// A message to a shop customer: plain paragraphs (blank lines kept), an optional button.
pub fn customer_notice(to: &str, company: &str, subject: &str, heading: &str, paragraphs: &[String], button: Option<(&str, &str)>) -> Email {
    notice(to, company, subject, heading, paragraphs, button, &format!("You get this email because you ordered from {company}."))
}

/// A plain message to someone outside the company: paragraphs, an optional button, a footer.
pub fn notice(to: &str, company: &str, subject: &str, heading: &str, paragraphs: &[String], button: Option<(&str, &str)>, footer: &str) -> Email {
    let refs: Vec<&str> = paragraphs.iter().map(String::as_str).collect();
    let mut text = paragraphs.join("\n\n");
    if let Some((label, url)) = button {
        text.push_str(&format!("\n\n{label}: {url}"));
    }
    Email {
        to: to.into(),
        subject: subject.into(),
        text,
        html: layout(company, heading, &refs, button, footer),
        attachments: vec![],
        calendar: None,
    }
}
