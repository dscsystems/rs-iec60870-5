// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Endpoint addressing and the TCP/TLS byte streams the transports run over.
//!
//! Shared by [`cs104`](crate::cs104) and by the TCP encapsulation transports of
//! [`cs101`](crate::cs101) and [`cs103`](crate::cs103).

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
#[cfg(any(feature = "cs101", feature = "cs104"))]
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

use crate::error::{Error, Result};

/// A remote 104 endpoint: a host and port, and whether to wrap it in TLS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// `host:port`, resolved at connection time.
    pub host: String,
    /// Whether the connection is TLS protected.
    pub tls: bool,
}

impl Endpoint {
    /// Parse an endpoint specification.
    ///
    /// Accepts `host:port`, `:port` (which means `127.0.0.1:port`) and URLs
    /// with a `tcp://`, `tls://`, `ssl://` or `tcps://` scheme. Without a
    /// scheme, `tcp://` is assumed.
    pub fn parse(server: &str) -> Result<Endpoint> {
        let server = server.trim();
        if server.is_empty() {
            return Err(Error::InvalidAddress("empty address".into()));
        }
        // A leading ':' is shorthand for the loopback interface.
        let server = if server.starts_with(':') {
            format!("127.0.0.1{server}")
        } else {
            server.to_string()
        };

        let (scheme, host) = match server.split_once("://") {
            Some((s, h)) => (s.to_ascii_lowercase(), h.to_string()),
            None => ("tcp".to_string(), server),
        };

        let tls = match scheme.as_str() {
            "tcp" => false,
            "tls" | "ssl" | "tcps" => true,
            _ => return Err(Error::InvalidAddress(format!("unknown scheme {scheme:?}"))),
        };

        // Strip any path or query a URL-style address may carry.
        let host = host
            .split(['/', '?'])
            .next()
            .unwrap_or_default()
            .to_string();
        if host.is_empty() || !host.contains(':') {
            return Err(Error::InvalidAddress(format!("{host:?} is not host:port")));
        }
        Ok(Endpoint { host, tls })
    }

    /// The host name without the port, for TLS certificate verification.
    pub fn hostname(&self) -> &str {
        match self.host.rsplit_once(':') {
            // Bracketed IPv6 literal.
            Some((h, _)) if h.starts_with('[') && h.ends_with(']') => &h[1..h.len() - 1],
            Some((h, _)) => h,
            None => &self.host,
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}://{}",
            if self.tls { "tls" } else { "tcp" },
            self.host
        )
    }
}

/// TLS configuration for outgoing connections.
///
/// Requires the `tls` feature; without it this type carries no configuration
/// and any `tls://` endpoint is rejected.
#[derive(Clone)]
pub struct TlsClientConfig {
    #[cfg(feature = "tls")]
    pub(crate) config: std::sync::Arc<tokio_rustls::rustls::ClientConfig>,
    #[cfg(feature = "tls")]
    pub(crate) server_name: Option<String>,
}

impl fmt::Debug for TlsClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TlsClientConfig")
    }
}

#[cfg(feature = "tls")]
impl TlsClientConfig {
    /// Wrap a rustls client configuration.
    pub fn new(config: std::sync::Arc<tokio_rustls::rustls::ClientConfig>) -> Self {
        TlsClientConfig {
            config,
            server_name: None,
        }
    }

    /// Override the server name presented in SNI and checked against the
    /// certificate, when it differs from the endpoint host.
    pub fn with_server_name(mut self, name: impl Into<String>) -> Self {
        self.server_name = Some(name.into());
        self
    }
}

/// TLS configuration for incoming connections.
///
/// Requires the `tls` feature.
#[derive(Clone)]
pub struct TlsServerConfig {
    #[cfg(feature = "tls")]
    pub(crate) config: std::sync::Arc<tokio_rustls::rustls::ServerConfig>,
}

impl fmt::Debug for TlsServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TlsServerConfig")
    }
}

#[cfg(feature = "tls")]
impl TlsServerConfig {
    /// Wrap a rustls server configuration.
    pub fn new(config: std::sync::Arc<tokio_rustls::rustls::ServerConfig>) -> Self {
        TlsServerConfig { config }
    }
}

enum Inner {
    Tcp(TcpStream),
    #[cfg(feature = "tls")]
    TlsClient(Box<tokio_rustls::client::TlsStream<TcpStream>>),
    #[cfg(feature = "tls")]
    TlsServer(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

/// A connected 104 stream, plain TCP or TLS.
pub struct Stream {
    inner: Inner,
    peer: Option<SocketAddr>,
}

impl Stream {
    /// The remote address, when the platform reported one.
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer
    }

    /// Remote identity and any negotiated TLS information.
    #[cfg(any(feature = "cs101", feature = "cs104"))]
    pub(crate) fn peer_info(&self) -> crate::secevent::PeerInfo {
        let (tcp, tls) = match &self.inner {
            Inner::Tcp(tcp) => (tcp, None),
            #[cfg(feature = "tls")]
            Inner::TlsClient(stream) => {
                let (tcp, connection) = stream.get_ref();
                (tcp, Some(tls_peer(connection)))
            }
            #[cfg(feature = "tls")]
            Inner::TlsServer(stream) => {
                let (tcp, connection) = stream.get_ref();
                (tcp, Some(tls_peer(connection)))
            }
        };
        crate::secevent::PeerInfo {
            network: "tcp",
            addr: self.peer,
            local_addr: tcp.local_addr().ok(),
            connected_at: Some(chrono::Utc::now()),
            tls,
            ..Default::default()
        }
    }

    /// Adopt an already established TCP connection.
    pub fn from_tcp(s: TcpStream) -> Stream {
        let peer = s.peer_addr().ok();
        Stream {
            inner: Inner::Tcp(s),
            peer,
        }
    }
}

#[cfg(all(feature = "tls", any(feature = "cs101", feature = "cs104")))]
fn tls_peer(connection: &tokio_rustls::rustls::CommonState) -> crate::secevent::TlsPeer {
    use sha2::{Digest, Sha256};

    let mut peer = crate::secevent::TlsPeer {
        version: connection
            .protocol_version()
            .map(|version| format!("{version:?}"))
            .unwrap_or_default(),
        cipher_suite: connection
            .negotiated_cipher_suite()
            .map(|suite| format!("{:?}", suite.suite()))
            .unwrap_or_default(),
        ..Default::default()
    };
    if let Some(cert) = connection
        .peer_certificates()
        .and_then(|chain| chain.first())
    {
        peer.fingerprint_sha256 = Some(format!("{:x}", Sha256::digest(cert.as_ref())));
        if let Ok((_, cert)) = x509_parser::parse_x509_certificate(cert.as_ref()) {
            peer.subject = Some(cert.subject().to_string());
            peer.issuer = Some(cert.issuer().to_string());
            peer.serial = Some(cert.raw_serial_as_string());
            peer.not_after =
                chrono::DateTime::from_timestamp(cert.validity().not_after.timestamp(), 0);
        }
    }
    peer
}

#[cfg(any(feature = "cs101", feature = "cs104"))]
#[derive(Debug, thiserror::Error)]
#[error("{source}")]
struct TlsTransportError {
    kind: crate::secevent::Kind,
    #[source]
    source: Error,
}

#[cfg(any(feature = "cs101", feature = "cs104"))]
pub(crate) fn tls_transport_error(kind: crate::secevent::Kind, source: Error) -> Error {
    let io_kind = match &source {
        Error::Io(error) => error.kind(),
        _ => return source,
    };
    io::Error::new(io_kind, TlsTransportError { kind, source }).into()
}

#[cfg(any(feature = "cs101", feature = "cs104"))]
pub(crate) fn dial_failure_kind(error: &Error) -> crate::secevent::Kind {
    use crate::secevent::Kind;
    if let Error::Io(error) = error
        && let Some(error) = error
            .get_ref()
            .and_then(|error| error.downcast_ref::<TlsTransportError>())
    {
        return error.kind;
    }
    Kind::ConnDialFailed
}

#[cfg(any(feature = "cs101", feature = "cs104"))]
pub(crate) fn tls_failure_kind(error: &Error) -> crate::secevent::Kind {
    use crate::secevent::Kind;
    #[cfg(feature = "tls")]
    if let Error::Io(error) = error {
        use tokio_rustls::rustls::{AlertDescription, Error as TlsError, InvalidMessage};
        if let Some(error) = error
            .get_ref()
            .and_then(|error| error.downcast_ref::<TlsError>())
        {
            return match error {
                TlsError::InvalidCertificate(_) => Kind::TlsCertRejected,
                TlsError::NoCertificatesPresented => Kind::TlsNoClientCert,
                TlsError::AlertReceived(
                    AlertDescription::BadCertificate
                    | AlertDescription::UnknownCA
                    | AlertDescription::CertificateExpired
                    | AlertDescription::CertificateRevoked
                    | AlertDescription::CertificateUnknown
                    | AlertDescription::UnsupportedCertificate,
                ) => Kind::TlsLocalCertRejected,
                TlsError::InvalidMessage(InvalidMessage::InvalidContentType) => Kind::TlsPlaintext,
                TlsError::InvalidMessage(InvalidMessage::UnknownProtocolVersion) => {
                    Kind::TlsNonTlsProbe
                }
                _ => Kind::TlsHandshakeFailed,
            };
        }
    }
    let _ = error;
    Kind::TlsHandshakeFailed
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut self.get_mut().inner {
            Inner::Tcp(s) => Pin::new(s).poll_read(cx, buf),
            #[cfg(feature = "tls")]
            Inner::TlsClient(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
            #[cfg(feature = "tls")]
            Inner::TlsServer(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().inner {
            Inner::Tcp(s) => Pin::new(s).poll_write(cx, buf),
            #[cfg(feature = "tls")]
            Inner::TlsClient(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
            #[cfg(feature = "tls")]
            Inner::TlsServer(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().inner {
            Inner::Tcp(s) => Pin::new(s).poll_flush(cx),
            #[cfg(feature = "tls")]
            Inner::TlsClient(s) => Pin::new(s.as_mut()).poll_flush(cx),
            #[cfg(feature = "tls")]
            Inner::TlsServer(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().inner {
            Inner::Tcp(s) => Pin::new(s).poll_shutdown(cx),
            #[cfg(feature = "tls")]
            Inner::TlsClient(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
            #[cfg(feature = "tls")]
            Inner::TlsServer(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Dial `endpoint`, giving up after `timeout` (t₀).
#[cfg(any(feature = "cs101", feature = "cs104"))]
pub(crate) async fn connect_endpoint(
    endpoint: &Endpoint,
    tls: Option<&TlsClientConfig>,
    timeout: Duration,
) -> Result<Stream> {
    use crate::secevent::Kind;

    let mut handshaking = false;
    let connect = async {
        let tcp = TcpStream::connect(&endpoint.host).await?;
        tcp.set_nodelay(true)?;
        if !endpoint.tls {
            return Ok(Stream::from_tcp(tcp));
        }
        handshaking = true;
        upgrade_client_tls(tcp, endpoint, tls)
            .await
            .map_err(|error| tls_transport_error(tls_failure_kind(&error), error))
    };

    match tokio::time::timeout(timeout, connect).await {
        Ok(result) => result,
        Err(_) => {
            let error = Error::from(io::Error::new(
                io::ErrorKind::TimedOut,
                "t0 connection timeout",
            ));
            if handshaking {
                Err(tls_transport_error(Kind::TlsHandshakeTo, error))
            } else {
                Err(error)
            }
        }
    }
}

#[cfg(all(feature = "tls", any(feature = "cs101", feature = "cs104")))]
async fn upgrade_client_tls(
    tcp: TcpStream,
    endpoint: &Endpoint,
    tls: Option<&TlsClientConfig>,
) -> Result<Stream> {
    use tokio_rustls::TlsConnector;
    use tokio_rustls::rustls::pki_types::ServerName;

    let tls = tls.ok_or(Error::Config("tls:// endpoint without a TLS configuration"))?;
    let peer = tcp.peer_addr().ok();
    let name = tls.server_name.as_deref().unwrap_or(endpoint.hostname());
    let server_name = ServerName::try_from(name.to_string())
        .map_err(|_| Error::InvalidAddress(format!("{name:?} is not a valid server name")))?;
    let stream = TlsConnector::from(tls.config.clone())
        .connect(server_name, tcp)
        .await?;
    Ok(Stream {
        inner: Inner::TlsClient(Box::new(stream)),
        peer,
    })
}

#[cfg(all(not(feature = "tls"), any(feature = "cs101", feature = "cs104")))]
async fn upgrade_client_tls(
    _tcp: TcpStream,
    _endpoint: &Endpoint,
    _tls: Option<&TlsClientConfig>,
) -> Result<Stream> {
    Err(Error::Config(
        "TLS endpoints require the \"tls\" feature to be enabled",
    ))
}

/// Wrap an accepted TCP connection, upgrading it to TLS when configured.
#[cfg(any(feature = "cs101", feature = "cs104"))]
pub(crate) async fn accept_stream(tcp: TcpStream, tls: Option<&TlsServerConfig>) -> Result<Stream> {
    tcp.set_nodelay(true)?;
    match tls {
        None => Ok(Stream::from_tcp(tcp)),
        Some(tls) => upgrade_server_tls(tcp, tls).await,
    }
}

#[cfg(all(feature = "tls", any(feature = "cs101", feature = "cs104")))]
async fn upgrade_server_tls(tcp: TcpStream, tls: &TlsServerConfig) -> Result<Stream> {
    use tokio_rustls::TlsAcceptor;

    let peer = tcp.peer_addr().ok();
    let stream = TlsAcceptor::from(tls.config.clone()).accept(tcp).await?;
    Ok(Stream {
        inner: Inner::TlsServer(Box::new(stream)),
        peer,
    })
}

#[cfg(all(not(feature = "tls"), any(feature = "cs101", feature = "cs104")))]
async fn upgrade_server_tls(_tcp: TcpStream, _tls: &TlsServerConfig) -> Result<Stream> {
    Err(Error::Config(
        "TLS listeners require the \"tls\" feature to be enabled",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_host_port_defaults_to_tcp() {
        let e = Endpoint::parse("192.168.0.1:2404").unwrap();
        assert_eq!(e.host, "192.168.0.1:2404");
        assert!(!e.tls);
        assert_eq!(e.to_string(), "tcp://192.168.0.1:2404");
    }

    #[test]
    fn a_leading_colon_means_loopback() {
        assert_eq!(Endpoint::parse(":2404").unwrap().host, "127.0.0.1:2404");
    }

    #[test]
    fn tls_schemes_are_recognised() {
        for s in ["tls://h:1", "ssl://h:1", "tcps://h:1", "TLS://h:1"] {
            assert!(Endpoint::parse(s).unwrap().tls, "{s}");
        }
        assert!(!Endpoint::parse("tcp://h:1").unwrap().tls);
    }

    #[test]
    fn a_url_path_is_stripped() {
        let e = Endpoint::parse("tcp://example.com:2404/path?x=1").unwrap();
        assert_eq!(e.host, "example.com:2404");
    }

    #[test]
    fn malformed_addresses_are_rejected() {
        assert!(Endpoint::parse("").is_err());
        assert!(Endpoint::parse("example.com").is_err(), "missing port");
        assert!(Endpoint::parse("udp://h:1").is_err(), "unknown scheme");
    }

    #[test]
    fn hostname_strips_the_port_and_ipv6_brackets() {
        assert_eq!(Endpoint::parse("host:2404").unwrap().hostname(), "host");
        assert_eq!(
            Endpoint::parse("tls://[::1]:2404").unwrap().hostname(),
            "::1"
        );
    }
}
