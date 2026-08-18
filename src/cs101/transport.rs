// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Opening the byte stream an FT1.2 link runs over: a local serial port, or a
//! TCP connection when the line is reached through a terminal server.

use std::io;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};

use crate::cs101::config::{Config, TransportType};
use crate::error::{Error, Result};

enum Inner {
    Net(crate::net::Stream),
    #[cfg(feature = "serial")]
    Serial(tokio_serial::SerialStream),
}

/// The byte stream carrying FT1.2 frames.
pub struct LinkStream {
    inner: Inner,
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
    listener: Mutex<Option<TcpListener>>,
}

impl Transporter {
    /// Build a transporter for `config`.
    pub fn new(config: Config) -> Transporter {
        Transporter {
            config,
            listener: Mutex::new(None),
        }
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
        if self.listener.lock().unwrap().is_some() {
            return Ok(());
        }
        let l = TcpListener::bind(&self.config.tcp.address).await?;
        tracing::debug!(local = ?l.local_addr().ok(), "FT1.2 TCP listener bound");
        *self.listener.lock().unwrap() = Some(l);
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
        let stream = crate::net::connect_endpoint(&endpoint, cfg.tls_client.as_ref(), timeout).await?;
        #[cfg(not(feature = "tls"))]
        let stream = crate::net::connect_endpoint(&endpoint, None, timeout).await?;

        let desc = format!("tcp -> {}", cfg.address);
        Ok((
            LinkStream {
                inner: Inner::Net(stream),
            },
            desc,
        ))
    }

    async fn open_tcp_server(&self) -> Result<(LinkStream, String)> {
        self.bind().await?;

        // `accept` needs the listener across an await, so take it out and put
        // it back; only one task ever calls `open` for a given transporter.
        let listener = self
            .listener
            .lock()
            .unwrap()
            .take()
            .ok_or(Error::Config("the TCP listener disappeared"))?;
        let accepted = listener.accept().await;
        *self.listener.lock().unwrap() = Some(listener);

        let (tcp, peer) = accepted?;
        tcp.set_nodelay(true)?;

        #[cfg(feature = "tls")]
        let stream = crate::net::accept_stream(tcp, self.config.tcp.tls_server.as_ref()).await?;
        #[cfg(not(feature = "tls"))]
        let stream = crate::net::accept_stream(tcp, None).await?;

        Ok((
            LinkStream {
                inner: Inner::Net(stream),
            },
            format!("tcp-listen (peer {peer})"),
        ))
    }
}

/// Wrap an already established stream, for tests and for callers that bring
/// their own transport (a virtual serial pair, an SSH tunnel, ...).
impl LinkStream {
    /// Adopt an existing TCP connection as the link.
    pub fn from_tcp(tcp: TcpStream) -> LinkStream {
        LinkStream {
            inner: Inner::Net(crate::net::Stream::from_tcp(tcp)),
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
