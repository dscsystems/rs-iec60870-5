// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

#![cfg(feature = "cs104")]

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{Client, ClientHandler, ClientOption, Server, ServerHandler};
use rs_iec60870_5::netpeer::{Cache, MacAddr, StaticResolver};
use rs_iec60870_5::secevent::{Action, Event, FunctionPolicy, Kind, Policy};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(3);

#[derive(Clone, Default)]
struct Station(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl ServerHandler for Station {
    async fn asdu(&self, conn: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        conn.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await
    }
}

struct Master(mpsc::UnboundedSender<Asdu>);

#[async_trait::async_trait]
impl ClientHandler for Master {
    async fn asdu(&self, _conn: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        self.0.send(pack.clone()).unwrap();
        Ok(())
    }
}

async fn event(rx: &mut mpsc::UnboundedReceiver<Event>, kind: Kind) -> Event {
    timeout(WAIT, async {
        loop {
            let event = rx.recv().await.unwrap();
            if event.kind == kind {
                return event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("event {kind} was not delivered"))
}

#[tokio::test]
async fn reports_peer_identity_and_session_lifecycle() {
    let server = Server::new(Station::default());
    let (tx, mut rx) = mpsc::unbounded_channel();
    server
        .set_security_sink(move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
    let ip: IpAddr = "127.0.0.1".parse().unwrap();
    let mac: MacAddr = "02:01:02:03:04:05".parse().unwrap();
    server.set_security_policy(Policy {
        resolver: Arc::new(StaticResolver([(ip, mac)].into_iter().collect())),
        peer_cache: Arc::new(Cache::default()),
        ..Policy::default()
    });
    let listener = TcpListener::bind((ip, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn({
        let server = server.clone();
        async move { server.serve(listener).await.unwrap() }
    });

    let peer = TcpStream::connect(addr).await.unwrap();
    let accepted = event(&mut rx, Kind::ConnAccepted).await;
    assert_eq!(accepted.peer.addr, Some(peer.local_addr().unwrap()));
    assert_eq!(accepted.peer.local_addr, Some(addr));
    assert_eq!(accepted.peer.mac, Some(mac));
    assert_eq!(accepted.peer.network, "tcp");
    drop(peer);
    let no_data = event(&mut rx, Kind::ConnNoData).await;
    assert_eq!(no_data.session_id, accepted.session_id);
    assert_eq!(
        event(&mut rx, Kind::ConnClosed).await.session_id,
        accepted.session_id
    );
    server.close();
    timeout(WAIT, task).await.unwrap().unwrap();
}

#[tokio::test]
async fn enforces_read_only_even_without_a_sink() {
    let handler = Station::default();
    let server = Server::new(handler.clone());
    server.set_security_policy(Policy {
        functions: FunctionPolicy {
            read_only: true,
            ..FunctionPolicy::default()
        },
        ..Policy::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let option = ClientOption::new()
        .with_server(&listener.local_addr().unwrap().to_string())
        .unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let client = Client::new(Master(tx), option);
    let task = tokio::spawn({
        let server = server.clone();
        async move { server.serve(listener).await.unwrap() }
    });
    client.start().unwrap();
    timeout(WAIT, client.wait_active()).await.unwrap();

    client
        .send_single_cmd(
            TypeId::C_SC_NA_1,
            CauseOfTransmission::new(Cause::ACTIVATION),
            1,
            SingleCommandInfo {
                ioa: 42,
                value: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let reply = timeout(WAIT, rx.recv()).await.unwrap().unwrap();
    assert_eq!(reply.coa().cause, Cause::ACTIVATION_CON);
    assert!(reply.coa().is_negative);
    assert_eq!(reply.first_ioa(), Some(42));
    assert_eq!(handler.0.load(Ordering::Relaxed), 0);
    client.close();
    server.close();
    timeout(WAIT, task).await.unwrap().unwrap();
}

#[tokio::test]
async fn refuses_unlisted_peers_before_publishing_a_session() {
    let server = Server::new(Station::default());
    let (tx, mut rx) = mpsc::unbounded_channel();
    server
        .set_security_sink(move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
    server.set_security_policy(Policy {
        allowed_peers: vec!["192.0.2.0/24".parse().unwrap()],
        reject_unlisted: true,
        ..Policy::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn({
        let server = server.clone();
        async move { server.serve(listener).await.unwrap() }
    });
    let mut peer = TcpStream::connect(addr).await.unwrap();
    assert_eq!(
        timeout(WAIT, peer.read_u8())
            .await
            .unwrap()
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::UnexpectedEof
    );
    assert_eq!(
        event(&mut rx, Kind::ConnUnexpected).await.action,
        Action::Rejected
    );
    assert_eq!(server.session_count(), 0);
    server.close();
    timeout(WAIT, task).await.unwrap().unwrap();
}

#[cfg(feature = "tls")]
#[tokio::test]
async fn reports_tls_rejections_and_exposes_the_verified_peer_certificate() {
    use rs_iec60870_5::net::{TlsClientConfig, TlsServerConfig};
    use tokio_rustls::rustls::{self, pki_types::PrivatePkcs8KeyDer};

    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let station_tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            PrivatePkcs8KeyDer::from(key_pair.serialize_der()).into(),
        )
        .unwrap();
    let server =
        Server::new(Station::default()).with_tls(TlsServerConfig::new(Arc::new(station_tls)));
    let (server_tx, mut server_events) = mpsc::unbounded_channel();
    server
        .set_security_sink(move |event| {
            let _ = server_tx.send(event);
        })
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("tls://{}", listener.local_addr().unwrap());
    let task = tokio::spawn({
        let server = server.clone();
        async move { server.serve(listener).await.unwrap() }
    });

    let (asdu_tx, _asdu_rx) = mpsc::unbounded_channel();
    let option = |roots| {
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        ClientOption::new()
            .with_server(&url)
            .unwrap()
            .with_tls(TlsClientConfig::new(Arc::new(config)).with_server_name("localhost"))
            .with_auto_reconnect(false)
    };
    let untrusted = Client::new(
        Master(asdu_tx.clone()),
        option(rustls::RootCertStore::empty()),
    );
    let (tx, mut events) = mpsc::unbounded_channel();
    untrusted
        .set_security_sink(move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
    untrusted.start().unwrap();
    event(&mut events, Kind::TlsCertRejected).await;
    assert!(!untrusted.is_connected().await);
    event(&mut server_events, Kind::TlsLocalCertRejected).await;

    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.der().clone()).unwrap();
    let trusted = Client::new(Master(asdu_tx), option(roots));
    trusted.start().unwrap();
    timeout(WAIT, trusted.wait_active()).await.unwrap();
    let peer = trusted.connection().await.unwrap().peer_info();
    let tls = peer.tls.unwrap();
    assert!(tls.subject.is_some());
    assert!(tls.not_after.is_some());
    assert_eq!(tls.fingerprint_sha256.unwrap().len(), 64);
    trusted.close();
    server.close();
    timeout(WAIT, task).await.unwrap().unwrap();
}
