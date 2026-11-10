//! IMAP connections: implicit TLS (993), STARTTLS (143) or plain (local test servers),
//! with an opt-in to accept self-signed certificates.

use async_imap::{Client, Session};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};
use tokio_rustls::{
    rustls::{
        self,
        client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        pki_types::{CertificateDer, ServerName, UnixTime},
        DigitallySignedStruct, SignatureScheme,
    },
    TlsConnector,
};

/// Plain or TLS stream behind one type.
#[derive(Debug)]
pub enum Stream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for Stream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Accepts any certificate. Only used when a mailbox explicitly allows self-signed
/// certificates (local or test servers).
#[derive(Debug)]
struct AcceptAny(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(&self, _: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// TLS client settings (ring crypto, Mozilla roots), shared with the OAuth HTTP client.
pub fn client_config(allow_invalid: bool) -> Result<rustls::ClientConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions().map_err(|e| e.to_string())?;
    Ok(if allow_invalid {
        builder.dangerous().with_custom_certificate_verifier(Arc::new(AcceptAny(provider))).with_no_client_auth()
    } else {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        builder.with_root_certificates(roots).with_no_client_auth()
    })
}

fn connector(allow_invalid: bool) -> Result<TlsConnector, String> {
    Ok(TlsConnector::from(Arc::new(client_config(allow_invalid)?)))
}

/// SASL XOAUTH2 (Gmail, Microsoft 365): the user and a bearer access token.
struct XOAuth2<'a> {
    user: &'a str,
    token: &'a str,
}

impl async_imap::Authenticator for XOAuth2<'_> {
    type Response = String;
    fn process(&mut self, _challenge: &[u8]) -> String {
        format!("user={}\x01auth=Bearer {}\x01\x01", self.user, self.token)
    }
}

async fn tls(host: &str, tcp: TcpStream, allow_invalid: bool) -> Result<Stream, String> {
    let name = ServerName::try_from(host.to_string()).map_err(|_| format!("invalid server name {host}"))?;
    let s = connector(allow_invalid)?.connect(name, tcp).await.map_err(|e| format!("TLS handshake failed: {e}"))?;
    Ok(Stream::Tls(Box::new(s)))
}

pub struct Settings<'a> {
    pub host: &'a str,
    pub port: u16,
    /// tls | starttls | none
    pub security: &'a str,
    pub username: &'a str,
    /// The password, or the OAuth access token when `xoauth2` is set.
    pub password: &'a str,
    pub allow_invalid_certs: bool,
    pub xoauth2: bool,
}

/// Connects and logs in. Errors are short messages for the user.
pub async fn connect(s: &Settings<'_>) -> Result<Session<Stream>, String> {
    tokio::time::timeout(Duration::from_secs(30), connect_inner(s)).await.map_err(|_| format!("{}:{} did not answer in time", s.host, s.port))?
}

async fn connect_inner(s: &Settings<'_>) -> Result<Session<Stream>, String> {
    let tcp = TcpStream::connect((s.host, s.port)).await.map_err(|e| format!("cannot reach {}:{}: {e}", s.host, s.port))?;
    let client = match s.security {
        "tls" => {
            let mut c = Client::new(tls(s.host, tcp, s.allow_invalid_certs).await?);
            c.read_response().await.map_err(|e| e.to_string())?.ok_or("the server closed the connection")?;
            c
        }
        "starttls" => {
            let mut c = Client::new(Stream::Plain(tcp));
            c.read_response().await.map_err(|e| e.to_string())?.ok_or("the server closed the connection")?;
            c.run_command_and_check_ok("STARTTLS", None).await.map_err(|e| format!("STARTTLS failed: {e}"))?;
            let Stream::Plain(tcp) = c.into_inner() else { unreachable!() };
            Client::new(tls(s.host, tcp, s.allow_invalid_certs).await?)
        }
        _ => {
            let mut c = Client::new(Stream::Plain(tcp));
            c.read_response().await.map_err(|e| e.to_string())?.ok_or("the server closed the connection")?;
            c
        }
    };
    if s.xoauth2 {
        return client
            .authenticate("XOAUTH2", XOAuth2 { user: s.username, token: s.password })
            .await
            .map_err(|(e, _)| format!("the server refused the sign-in ({}); reconnect the mailbox", server_text(&e.to_string())));
    }
    client.login(s.username, s.password).await.map_err(|(e, _)| format!("login failed: {}", server_text(&e.to_string())))
}

/// The server's own words from an IMAP error ("[AUTHENTICATIONFAILED] Invalid credentials").
fn server_text(e: &str) -> String {
    match e.find("info: Some(\"") {
        Some(i) => e[i + 12..].split("\")").next().unwrap_or(e).replace("\\\"", "\"").to_string(),
        None => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn server_text() {
        assert_eq!(super::server_text(r#"no response: code: None, info: Some("[AUTHENTICATIONFAILED] Invalid credentials (Failure)")"#), "[AUTHENTICATIONFAILED] Invalid credentials (Failure)");
        assert_eq!(super::server_text("connection lost"), "connection lost");
    }
}
