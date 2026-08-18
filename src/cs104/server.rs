// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The IEC 60870-5-104 controlled station: a [`Server`] that listens for
//! masters, and a [`ServerSpecial`] that dials out to one instead.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::net::{TcpListener, ToSocketAddrs};
use tokio::sync::watch;

use crate::asdu::{Asdu, Connect, PARAMS_WIDE, Params};
use crate::cs104::client::ClientOption;
use crate::cs104::config::Config;
use crate::cs104::connection::{Callbacks, Connection, Role, RunOptions, run};
use crate::cs104::handler::{ServerDispatcher, ServerHandler};
use crate::net::{TlsServerConfig, accept_stream, connect_endpoint};
use crate::error::{Error, Result};

/// Tracks the live sessions of a controlled station.
#[derive(Default)]
struct Sessions {
    next_id: AtomicUsize,
    live: Mutex<HashMap<usize, Arc<Connection>>>,
}

impl Sessions {
    fn insert(&self, conn: Arc<Connection>) -> usize {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.live.lock().unwrap().insert(id, conn);
        id
    }

    fn remove(&self, id: usize) {
        self.live.lock().unwrap().remove(&id);
    }

    fn all(&self) -> Vec<Arc<Connection>> {
        self.live.lock().unwrap().values().cloned().collect()
    }

    fn len(&self) -> usize {
        self.live.lock().unwrap().len()
    }
}

/// An IEC 60870-5-104 controlled station (outstation / slave) that listens for
/// masters.
///
/// Any number of masters may be connected at once. [`Connect::send`] on the
/// server **broadcasts a copy to every session**, which is how spontaneous data
/// is published; inside a handler, the `&dyn Connect` argument is the single
/// session the request came from, so replies go only to that master.
pub struct Server<H: ServerHandler> {
    config: Config,
    params: Params,
    handler: Arc<H>,
    tls: Option<TlsServerConfig>,
    sessions: Arc<Sessions>,
    shutdown: watch::Sender<bool>,
    server_number: usize,
    serving: AtomicBool,
}

impl<H: ServerHandler> Server<H> {
    /// Build a controlled station with the default configuration and
    /// [`PARAMS_WIDE`] parameters.
    pub fn new(handler: H) -> Arc<Self> {
        Arc::new(Server {
            config: Config::default(),
            params: PARAMS_WIDE,
            handler: Arc::new(handler),
            tls: None,
            sessions: Arc::new(Sessions::default()),
            shutdown: watch::channel(false).0,
            server_number: 0,
            serving: AtomicBool::new(false),
        })
    }

    /// Replace the timing configuration. Invalid values fall back to defaults.
    pub fn with_config(mut self: Arc<Self>, config: Config) -> Arc<Self> {
        let s = Arc::get_mut(&mut self).expect("configure before serving");
        s.config = config.or_default();
        self
    }

    /// Replace the ASDU parameters. Invalid values fall back to [`PARAMS_WIDE`].
    ///
    /// These must match every connected master.
    pub fn with_params(mut self: Arc<Self>, params: Params) -> Arc<Self> {
        let s = Arc::get_mut(&mut self).expect("configure before serving");
        s.params = if params.valid().is_ok() {
            params
        } else {
            PARAMS_WIDE
        };
        self
    }

    /// Set the time zone used to encode and decode CP24/CP56 time tags.
    pub fn with_time_zone(mut self: Arc<Self>, zone: crate::asdu::TimeZone) -> Arc<Self> {
        let s = Arc::get_mut(&mut self).expect("configure before serving");
        s.params.info_obj_time_zone = zone;
        self
    }

    /// Serve TLS instead of plain TCP. Requires the `tls` feature.
    pub fn with_tls(mut self: Arc<Self>, tls: TlsServerConfig) -> Arc<Self> {
        let s = Arc::get_mut(&mut self).expect("configure before serving");
        s.tls = Some(tls);
        self
    }

    /// Set the server index reported to `ServerHandler::asdu_all`.
    pub fn with_server_number(mut self: Arc<Self>, n: usize) -> Arc<Self> {
        let s = Arc::get_mut(&mut self).expect("configure before serving");
        s.server_number = n;
        self
    }

    /// The ASDU parameters in use.
    pub fn params(&self) -> Params {
        self.params
    }

    /// How many masters are connected.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Bind `addr` and serve until [`Server::close`] is called or the listener
    /// fails.
    ///
    /// The usual address is `"0.0.0.0:2404"`.
    pub async fn listen_and_serve<A: ToSocketAddrs>(self: &Arc<Self>, addr: A) -> Result<()> {
        let listener = TcpListener::bind(addr).await?;
        self.serve(listener).await
    }

    /// Serve on an already bound listener.
    ///
    /// Useful when the caller needs the resolved local address first, for
    /// instance after binding port 0.
    pub async fn serve(self: &Arc<Self>, listener: TcpListener) -> Result<()> {
        self.serving.store(true, Ordering::Release);
        let mut shutdown = self.shutdown.subscribe();
        tracing::debug!(local = ?listener.local_addr().ok(), "server listening");

        loop {
            let accepted = tokio::select! {
                _ = shutdown.changed() => break,
                r = listener.accept() => r,
            };

            let (tcp, peer) = match accepted {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(error = %e, "accept failed");
                    self.serving.store(false, Ordering::Release);
                    return Err(e.into());
                }
            };

            let this = Arc::clone(self);
            tokio::spawn(async move { this.serve_one(tcp, peer).await });
        }

        self.serving.store(false, Ordering::Release);
        tracing::debug!("server stopped");
        Ok(())
    }

    async fn serve_one(self: Arc<Self>, tcp: tokio::net::TcpStream, peer: SocketAddr) {
        let stream = match accept_stream(tcp, self.tls.as_ref()).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(%peer, error = %e, "rejecting connection");
                return;
            }
        };
        tracing::debug!(%peer, "master connected");

        let dispatcher = Arc::new(ServerDispatcher {
            handler: Arc::clone(&self.handler),
            server_number: self.server_number,
        });
        let opts = RunOptions {
            config: self.config,
            params: self.params,
            role: Role::Controlled,
            peer: Some(peer),
            auto_start_dt: false,
            callbacks: server_callbacks(&self.handler),
        };

        let sessions = Arc::clone(&self.sessions);
        let mut id = None;
        run(
            stream,
            opts,
            dispatcher,
            self.shutdown.subscribe(),
            |c| id = Some(sessions.insert(c)),
        )
        .await;

        if let Some(id) = id {
            self.sessions.remove(id);
        }
        tracing::debug!(%peer, "master disconnected");
    }

    /// Stop serving and close every session.
    pub fn close(&self) {
        let _ = self.shutdown.send(true);
    }
}

fn server_callbacks<H: ServerHandler>(handler: &Arc<H>) -> Callbacks {
    let mut cb = Callbacks::default();
    let h = Arc::clone(handler);
    cb.on_connect = Some(Arc::new(move |c: Arc<Connection>| {
        let h = Arc::clone(&h);
        tokio::spawn(async move { h.on_connect(c.as_ref()).await });
    }));
    let h = Arc::clone(handler);
    cb.on_connection_lost = Some(Arc::new(move |c: Arc<Connection>| {
        let h = Arc::clone(&h);
        tokio::spawn(async move { h.on_connection_lost(c.as_ref()).await });
    }));
    cb
}

#[async_trait::async_trait]
impl<H: ServerHandler> Connect for Server<H> {
    fn params(&self) -> Params {
        self.params
    }

    /// Broadcast a copy of `a` to every connected session.
    ///
    /// A session that cannot take it right now (its queue is full) is skipped
    /// with a warning rather than failing the whole broadcast.
    async fn send(&self, a: Asdu) -> Result<()> {
        for session in self.sessions.all() {
            if let Err(e) = session.send(a.clone()).await {
                tracing::warn!(peer = ?session.peer_addr(), error = %e, "broadcast to session failed");
            }
        }
        Ok(())
    }
}

/// A controlled station that **dials out** to a master instead of listening.
///
/// Useful when the outstation sits behind NAT. It answers interrogations and
/// StartDT exactly like a [`Server`] session, but initiates the TCP connection
/// and reconnects on its own.
pub struct ServerSpecial<H: ServerHandler> {
    option: ClientOption,
    handler: Arc<H>,
    conn: Mutex<Option<Arc<Connection>>>,
    shutdown: watch::Sender<bool>,
    started: AtomicBool,
    server_number: usize,
}

impl<H: ServerHandler> ServerSpecial<H> {
    /// Build a reverse-connection controlled station.
    ///
    /// The endpoint, timing and parameters come from a [`ClientOption`],
    /// since this end initiates the connection.
    pub fn new(handler: H, option: ClientOption) -> Arc<Self> {
        Arc::new(ServerSpecial {
            option,
            handler: Arc::new(handler),
            conn: Mutex::new(None),
            shutdown: watch::channel(false).0,
            started: AtomicBool::new(false),
            server_number: 0,
        })
    }

    /// The ASDU parameters in use.
    pub fn params(&self) -> Params {
        self.option.params()
    }

    /// Whether the connection to the master is up.
    pub fn is_connected(&self) -> bool {
        self.conn
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|c| c.is_connected())
    }

    /// Whether the master has activated data transfer.
    pub fn is_active(&self) -> bool {
        self.conn
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|c| c.is_active())
    }

    /// Start dialling out in the background; returns immediately.
    pub fn start(self: &Arc<Self>) -> Result<()> {
        if self.option.server().is_none() {
            return Err(Error::Config("no remote server configured"));
        }
        if self.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let this = Arc::clone(self);
        tokio::spawn(async move { this.run_forever().await });
        Ok(())
    }

    /// Stop and close the connection.
    pub fn close(&self) {
        let _ = self.shutdown.send(true);
    }

    async fn run_forever(self: Arc<Self>) {
        let endpoint = self.option.server().expect("checked in start()");
        let mut shutdown = self.shutdown.subscribe();

        loop {
            if *shutdown.borrow() {
                return;
            }

            tracing::debug!(%endpoint, "connecting to master");
            let stream = tokio::select! {
                _ = shutdown.changed() => return,
                r = connect_endpoint(
                    &endpoint,
                    self.option.tls(),
                    self.option.config().connect_timeout0,
                ) => r,
            };

            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(%endpoint, error = %e, "connect failed");
                    if !self.option.auto_reconnect() {
                        return;
                    }
                    tokio::select! {
                        _ = shutdown.changed() => return,
                        _ = tokio::time::sleep(self.option.reconnect_interval()) => continue,
                    }
                }
            };

            let peer = stream.peer_addr();
            let dispatcher = Arc::new(ServerDispatcher {
                handler: Arc::clone(&self.handler),
                server_number: self.server_number,
            });
            let opts = RunOptions {
                config: self.option.config(),
                params: self.option.params(),
                role: Role::Controlled,
                peer,
                auto_start_dt: false,
                callbacks: server_callbacks(&self.handler),
            };

            let this = Arc::clone(&self);
            run(
                stream,
                opts,
                dispatcher,
                self.shutdown.subscribe(),
                move |c| *this.conn.lock().unwrap() = Some(c),
            )
            .await;

            *self.conn.lock().unwrap() = None;
            if *shutdown.borrow() {
                return;
            }
            tokio::select! {
                _ = shutdown.changed() => return,
                _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
        }
    }
}

#[async_trait::async_trait]
impl<H: ServerHandler> Connect for ServerSpecial<H> {
    fn params(&self) -> Params {
        self.option.params()
    }

    async fn send(&self, a: Asdu) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .unwrap()
            .clone()
            .ok_or(Error::UseClosedConnection)?;
        conn.send(a).await
    }
}
