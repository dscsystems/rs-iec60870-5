// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Opening the byte stream an FT1.2 link runs over: a local serial port, or a
//! TCP connection when the line is reached through a terminal server.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::{Semaphore, mpsc, watch};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};

use crate::cs101::config::{Config, TransportType};
use crate::cs101::frame::{Frame, read_frame};
use crate::error::{Error, Result};
use crate::secevent::{Action, Kind, PeerInfo, Security, Session};

enum Inner {
    Net(crate::net::Stream),
    #[cfg(feature = "serial")]
    Serial(tokio_serial::SerialStream),
}

/// The byte stream carrying FT1.2 frames.
pub struct LinkStream {
    inner: Inner,
    _listener: Option<ActivePeer>,
}

impl AsyncRead for LinkStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut self.get_mut().inner {
            Inner::Net(s) => Pin::new(s).poll_read(cx, buf),
            #[cfg(feature = "serial")]
            Inner::Serial(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for LinkStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().inner {
            Inner::Net(s) => Pin::new(s).poll_write(cx, buf),
            #[cfg(feature = "serial")]
            Inner::Serial(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().inner {
            Inner::Net(s) => Pin::new(s).poll_flush(cx),
            #[cfg(feature = "serial")]
            Inner::Serial(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().inner {
            Inner::Net(s) => Pin::new(s).poll_shutdown(cx),
            #[cfg(feature = "serial")]
            Inner::Serial(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Opens connections for a configured transport.
///
/// With [`TransportType::TcpServer`] the listener is created on first use and
/// kept open across connections, so a dropped peer is replaced by the next one
/// without rebinding the port.
pub struct Transporter {
    config: Config,
    listener: Mutex<Option<Arc<TcpListener>>>,
    binding: tokio::sync::Mutex<()>,
    slot: Arc<Semaphore>,
    security: Option<(Arc<Security>, &'static str, &'static str, usize)>,
}

impl Transporter {
    /// Build a transporter for `config`.
    pub fn new(config: Config) -> Transporter {
        Transporter {
            config,
            listener: Mutex::new(None),
            binding: tokio::sync::Mutex::new(()),
            slot: Arc::new(Semaphore::new(1)),
            security: None,
        }
    }

    pub(crate) fn with_security(
        mut self,
        security: Arc<Security>,
        protocol: &'static str,
        role: &'static str,
        endpoint: usize,
    ) -> Self {
        self.security = Some((security, protocol, role, endpoint));
        self
    }

    /// The address the TCP listener bound to, once it exists.
    ///
    /// Useful when the configured address used port 0.
    pub fn listen_addr(&self) -> Option<std::net::SocketAddr> {
        self.listener
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|l| l.local_addr().ok())
    }

    /// Bind the TCP listener, without waiting for a peer.
    ///
    /// Only meaningful for [`TransportType::TcpServer`]; a no-op otherwise.
    /// Calling it makes [`Transporter::listen_addr`] observable before the
    /// first connection, which is what a caller configuring port 0 needs.
    pub async fn bind(&self) -> Result<()> {
        if self.config.transport != TransportType::TcpServer {
            return Ok(());
        }
        let _binding = self.binding.lock().await;
        if self.listener.lock().unwrap().is_some() {
            return Ok(());
        }
        let l = TcpListener::bind(&self.config.tcp.address).await?;
        tracing::debug!(local = ?l.local_addr().ok(), "FT1.2 TCP listener bound");
        *self.listener.lock().unwrap() = Some(Arc::new(l));
        Ok(())
    }

    /// Establish one connection, returning it with a description for logging.
    ///
    /// With [`TransportType::TcpServer`] this waits for a peer to connect.
    pub async fn open(&self) -> Result<(LinkStream, String)> {
        match self.config.transport {
            TransportType::Serial => self.open_serial(),
            TransportType::TcpClient => self.open_tcp_client().await,
            TransportType::TcpServer => self.open_tcp_server().await,
        }
    }

    #[cfg(feature = "serial")]
    fn open_serial(&self) -> Result<(LinkStream, String)> {
        use tokio_serial::SerialPortBuilderExt;

        let cfg = &self.config.serial;
        let port = tokio_serial::new(&cfg.address, cfg.baud_rate)
            .data_bits(match cfg.data_bits {
                5 => tokio_serial::DataBits::Five,
                6 => tokio_serial::DataBits::Six,
                7 => tokio_serial::DataBits::Seven,
                _ => tokio_serial::DataBits::Eight,
            })
            .parity(match cfg.parity {
                crate::cs101::config::Parity::None => tokio_serial::Parity::None,
                crate::cs101::config::Parity::Odd => tokio_serial::Parity::Odd,
                crate::cs101::config::Parity::Even => tokio_serial::Parity::Even,
            })
            .stop_bits(match cfg.stop_bits {
                crate::cs101::config::StopBits::One => tokio_serial::StopBits::One,
                crate::cs101::config::StopBits::Two => tokio_serial::StopBits::Two,
            })
            .open_native_async()
            .map_err(|e| Error::from(io::Error::other(e)))?;

        Ok((
            LinkStream {
                inner: Inner::Serial(port),
                _listener: None,
            },
            format!("serial {}", cfg.address),
        ))
    }

    #[cfg(not(feature = "serial"))]
    fn open_serial(&self) -> Result<(LinkStream, String)> {
        Err(Error::Config(
            "the serial transport requires the \"serial\" feature to be enabled",
        ))
    }

    async fn open_tcp_client(&self) -> Result<(LinkStream, String)> {
        let cfg = &self.config.tcp;
        let timeout = cfg
            .connect_timeout
            .unwrap_or(crate::cs101::config::DEFAULT_TCP_CONNECT_TIMEOUT);

        #[cfg(feature = "tls")]
        let endpoint = crate::net::Endpoint {
            host: cfg.address.clone(),
            tls: cfg.tls_client.is_some(),
        };
        #[cfg(not(feature = "tls"))]
        let endpoint = crate::net::Endpoint {
            host: cfg.address.clone(),
            tls: false,
        };

        #[cfg(feature = "tls")]
        let stream =
            crate::net::connect_endpoint(&endpoint, cfg.tls_client.as_ref(), timeout).await?;
        #[cfg(not(feature = "tls"))]
        let stream = crate::net::connect_endpoint(&endpoint, None, timeout).await?;

        let desc = format!("tcp -> {}", cfg.address);
        Ok((
            LinkStream {
                inner: Inner::Net(stream),
                _listener: None,
            },
            desc,
        ))
    }

    async fn open_tcp_server(&self) -> Result<(LinkStream, String)> {
        self.bind().await?;

        let permit = Arc::clone(&self.slot)
            .acquire_owned()
            .await
            .expect("the transport slot is never closed");
        let listener = self.listener.lock().unwrap().as_ref().unwrap().clone();
        let (tcp, peer) = listener.accept().await?;
        let security = self.security.clone();
        let reject = tokio::spawn(async move {
            let _permit = permit;
            let initial_backoff = Duration::from_millis(5);
            let mut backoff = initial_backoff;
            loop {
                let peer = match listener.accept().await {
                    Ok((_, peer)) => {
                        backoff = initial_backoff;
                        peer
                    }
                    Err(error) => {
                        tracing::warn!(%error, "FT1.2 accept failed");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(1));
                        continue;
                    }
                };
                if let Some((security, protocol, role, endpoint)) = &security {
                    let peer = PeerInfo {
                        network: "tcp",
                        addr: Some(peer),
                        ..PeerInfo::default()
                    };
                    Session::new(security.clone(), protocol, role, *endpoint, peer).note(
                        Kind::ConnRejectedBusy,
                        Action::Rejected,
                        "FT1.2 transport already has a peer",
                    );
                }
                tracing::debug!(%peer, "FT1.2 transport already has a peer");
            }
        });
        let active = ActivePeer { reject };

        #[cfg(feature = "tls")]
        let tls = self.config.tcp.tls_server.as_ref();
        #[cfg(not(feature = "tls"))]
        let tls = None;
        let deadline = self
            .config
            .tcp
            .connect_timeout
            .unwrap_or(crate::cs101::config::DEFAULT_TCP_CONNECT_TIMEOUT);
        let stream = match tokio::time::timeout(deadline, crate::net::accept_stream(tcp, tls)).await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) if tls.is_some() => {
                return Err(crate::net::tls_transport_error(
                    crate::net::tls_failure_kind(&error),
                    error,
                ));
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                let error = io::Error::new(io::ErrorKind::TimedOut, "FT1.2 TLS handshake timeout");
                return Err(crate::net::tls_transport_error(
                    Kind::TlsHandshakeTo,
                    error.into(),
                ));
            }
        };

        Ok((
            LinkStream {
                inner: Inner::Net(stream),
                _listener: Some(active),
            },
            format!("tcp-listen (peer {peer})"),
        ))
    }
}

pub(crate) async fn receive_frames(
    mut reader: impl AsyncRead + Unpin,
    sender: mpsc::Sender<Frame>,
    mut stop: watch::Receiver<bool>,
    addr_size: u8,
    session: Session,
) {
    loop {
        let frame = tokio::select! {
            _ = stop.changed() => return,
            frame = read_frame(&mut reader, addr_size) => frame,
        };
        match frame {
            Ok(frame) => tokio::select! {
                _ = stop.changed() => return,
                result = sender.send(frame) => {
                    if result.is_err() {
                        return;
                    }
                }
            },
            Err(Error::Frame(reason)) => {
                session.link_event(Kind::ProtoCs101Frame, Action::Ignored, reason, None);
                tracing::warn!(reason, "discarding malformed frame");
            }
            Err(error) => {
                tracing::debug!(%error, "receive stopped");
                return;
            }
        }
    }
}

pub(crate) async fn write_frames(
    mut writer: impl AsyncWrite + Unpin,
    mut frames: mpsc::Receiver<Vec<u8>>,
    mut stop: watch::Receiver<bool>,
    deadline: Duration,
    session: Session,
) {
    loop {
        let frame = tokio::select! {
            _ = stop.changed() => break,
            frame = frames.recv() => frame,
        };
        let Some(raw) = frame else { break };
        tracing::trace!(frame = ?raw, "TX raw");
        let result = tokio::select! {
            _ = stop.changed() => return,
            result = tokio::time::timeout(deadline, writer.write_all(&raw)) => result,
        };
        if !matches!(result, Ok(Ok(()))) {
            if result.is_err() {
                session.note(
                    Kind::DosWriteStall,
                    Action::Disconnected,
                    "FT1.2 write deadline expired",
                );
            }
            tracing::debug!(?result, "send failed");
            return;
        }
    }
    let _ = tokio::time::timeout(deadline, writer.shutdown()).await;
}

struct ActivePeer {
    reject: tokio::task::JoinHandle<()>,
}

impl Drop for ActivePeer {
    fn drop(&mut self) {
        self.reject.abort();
    }
}

/// Wrap an already established stream, for tests and for callers that bring
/// their own transport (a virtual serial pair, an SSH tunnel, ...).
impl LinkStream {
    pub(crate) fn peer_info(&self, _serial_port: &str) -> PeerInfo {
        match &self.inner {
            Inner::Net(stream) => stream.peer_info(),
            #[cfg(feature = "serial")]
            Inner::Serial(_) => PeerInfo {
                network: "serial",
                serial_port: Some(_serial_port.to_owned()),
                connected_at: Some(chrono::Utc::now()),
                ..PeerInfo::default()
            },
        }
    }

    /// Adopt an existing TCP connection as the link.
    pub fn from_tcp(tcp: TcpStream) -> LinkStream {
        LinkStream {
            inner: Inner::Net(crate::net::Stream::from_tcp(tcp)),
            _listener: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cs101::config::TcpConfig;

    #[tokio::test]
    async fn the_tcp_server_transport_binds_once_and_serves_successive_peers() {
        let t = Transporter::new(Config {
            transport: TransportType::TcpServer,
            tcp: TcpConfig {
                address: "127.0.0.1:0".into(),
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(t.listen_addr(), None, "not bound before bind()");

        t.bind().await.unwrap();
        let addr = t.listen_addr().expect("bound");

        for _ in 0..2 {
            let peer = tokio::spawn(async move { TcpStream::connect(addr).await });
            let (_stream, desc) = t.open().await.unwrap();
            peer.await.unwrap().unwrap();
            assert!(desc.starts_with("tcp-listen"));
            // The same listener serves the next connection.
            assert_eq!(t.listen_addr(), Some(addr));
        }
    }

    #[tokio::test]
    async fn an_additional_tcp_peer_is_closed_while_the_active_one_remains_open() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::time::{Duration, timeout};

        let transporter = Transporter::new(Config {
            transport: TransportType::TcpServer,
            tcp: TcpConfig {
                address: "127.0.0.1:0".into(),
                ..TcpConfig::default()
            },
            ..Config::default()
        });
        transporter.bind().await.unwrap();
        let addr = transporter.listen_addr().unwrap();
        let mut first = TcpStream::connect(addr).await.unwrap();
        let (mut stream, _) = transporter.open().await.unwrap();

        let mut extra = TcpStream::connect(addr).await.unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), extra.read_u8())
                .await
                .unwrap()
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
        first.write_u8(42).await.unwrap();
        assert_eq!(stream.read_u8().await.unwrap(), 42);
    }

    #[tokio::test]
    async fn the_tcp_client_transport_dials_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { listener.accept().await });

        let t = Transporter::new(Config {
            transport: TransportType::TcpClient,
            tcp: TcpConfig {
                address: addr.to_string(),
                ..Default::default()
            },
            ..Default::default()
        });
        let (_stream, desc) = t.open().await.unwrap();
        assert!(desc.contains(&addr.to_string()));
    }

    #[cfg(not(feature = "serial"))]
    #[tokio::test]
    async fn the_serial_transport_reports_the_missing_feature() {
        let t = Transporter::new(Config {
            serial: crate::cs101::SerialConfig::new("/dev/null", 9600),
            ..Default::default()
        });
        assert!(matches!(t.open().await, Err(Error::Config(_))));
    }
}
