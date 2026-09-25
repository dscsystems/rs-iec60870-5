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
use crate::cs104::redundancy::{Groups, Route, ServerMode, outcome};
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

    fn len(&self) -> usize {
        self.live.lock().unwrap().len()
    }
}

/// An IEC 60870-5-104 controlled station (outstation / slave) that listens for
/// masters.
///
/// Any number of masters may be connected at once, up to
/// [`Server::with_max_connections`]. [`Connect::send`] on the server
/// **broadcasts** spontaneous data, once per redundancy group (see
/// [`ServerMode`]): to the group's connection in data transfer, or into the
/// group's event buffer while none is. Inside a handler, the `&dyn Connect`
/// argument is the single session the request came from, so replies go only
/// to that master.
pub struct Server<H: ServerHandler> {
    config: Config,
    params: Params,
    handler: Arc<H>,
    tls: Option<TlsServerConfig>,
    sessions: Arc<Sessions>,
    mode: ServerMode,
    event_buffer: usize,
    groups: Arc<Mutex<Groups>>,
    max_connections: Option<usize>,
    /// Connections accepted and not yet closed, counted from the accept so a
    /// burst of connections cannot slip past the limit.
    open: Arc<AtomicUsize>,
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
            mode: ServerMode::default(),
            event_buffer: 0,
            groups: Arc::new(Mutex::new(Groups::new(ServerMode::default(), 0))),
            max_connections: None,
            open: Arc::new(AtomicUsize::new(0)),
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

    /// Set how connected masters are grouped for spontaneous data.
    ///
    /// The default, [`ServerMode::ConnectionIsRedundancyGroup`], sends every
    /// broadcast to every master in data transfer. Use
    /// [`ServerMode::SingleRedundancyGroup`] when the masters are one
    /// redundant control system — a main and a standby — so that only the one
    /// in data transfer is sent data, and starting another switches over.
    pub fn with_mode(mut self: Arc<Self>, mode: ServerMode) -> Arc<Self> {
        let s = Arc::get_mut(&mut self).expect("configure before serving");
        s.mode = mode;
        s.groups = Arc::new(Mutex::new(Groups::new(s.mode.clone(), s.event_buffer)));
        self
    }

    /// Keep up to `n` ASDUs per redundancy group while none of its masters is
    /// in data transfer, and replay them, oldest first, to the next one that
    /// starts. Zero, the default, keeps nothing.
    ///
    /// With a fixed group ([`ServerMode::SingleRedundancyGroup`] or
    /// [`ServerMode::MultipleRedundancyGroups`]) the buffer exists even while
    /// no master is connected, so events raised during an outage of the
    /// control centre are delivered when it returns. A full buffer refuses
    /// further data — reported by `send` — rather than overwriting what it
    /// holds.
    pub fn with_event_buffer(mut self: Arc<Self>, n: usize) -> Arc<Self> {
        let s = Arc::get_mut(&mut self).expect("configure before serving");
        s.event_buffer = n;
        s.groups = Arc::new(Mutex::new(Groups::new(s.mode.clone(), n)));
        self
    }

    /// Refuse connections beyond `n` open at once. Unlimited by default.
    pub fn with_max_connections(mut self: Arc<Self>, n: usize) -> Arc<Self> {
        let s = Arc::get_mut(&mut self).expect("configure before serving");
        s.max_connections = Some(n);
        self
    }

    /// The ASDU parameters in use.
    pub fn params(&self) -> Params {
        self.params
    }

    /// How many ASDUs wait in the redundancy groups' event buffers.
    pub fn buffered_count(&self) -> usize {
        self.groups.lock().unwrap().buffered()
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

            if self
                .max_connections
                .is_some_and(|max| self.open.load(Ordering::Acquire) >= max)
            {
                tracing::warn!(%peer, "connection limit reached, refusing the master");
                continue;
            }
            let Some(group) = self.groups.lock().unwrap().admit(peer.ip()) else {
                tracing::warn!(%peer, "the master belongs to no redundancy group, refused");
                continue;
            };

            self.open.fetch_add(1, Ordering::AcqRel);
            let this = Arc::clone(self);
            let open = Arc::clone(&self.open);
            tokio::spawn(async move {
                this.serve_one(tcp, peer, group).await;
                open.fetch_sub(1, Ordering::AcqRel);
            });
        }

        self.serving.store(false, Ordering::Release);
        tracing::debug!("server stopped");
        Ok(())
    }

    async fn serve_one(self: Arc<Self>, tcp: tokio::net::TcpStream, peer: SocketAddr, group: usize) {
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
            callbacks: server_callbacks(&self.handler, Some(&self.groups)),
        };

        let sessions = Arc::clone(&self.sessions);
        let groups = Arc::clone(&self.groups);
        let mut joined: Option<(usize, Arc<Connection>)> = None;
        run(
            stream,
            opts,
            dispatcher,
            self.shutdown.subscribe(),
            |c| {
                groups.lock().unwrap().join(group, Arc::clone(&c));
                joined = Some((sessions.insert(Arc::clone(&c)), c));
            },
        )
        .await;

        if let Some((id, conn)) = joined {
            self.sessions.remove(id);
            self.groups.lock().unwrap().leave(&conn);
        }
        tracing::debug!(%peer, "master disconnected");
    }

    /// Stop serving and close every session.
    pub fn close(&self) {
        let _ = self.shutdown.send(true);
    }
}

/// The callbacks of a controlled-station session. `groups` is `None` for the
/// single connection of a [`ServerSpecial`], which belongs to no group.
fn server_callbacks<H: ServerHandler>(
    handler: &Arc<H>,
    groups: Option<&Arc<Mutex<Groups>>>,
) -> Callbacks {
    let mut cb = Callbacks::default();
    if let Some(groups) = groups {
        // Group bookkeeping runs inline, under the groups' lock, so a
        // broadcast racing a switchover sees either the old connection or the
        // new one and the replayed buffer, never a mixture that reorders or
        // drops data.
        let g = Arc::clone(groups);
        cb.on_activated = Some(Arc::new(move |c: Arc<Connection>| {
            g.lock().unwrap().activated(&c);
        }));
        let g = Arc::clone(groups);
        cb.on_deactivated = Some(Arc::new(move |c: Arc<Connection>| {
            g.lock().unwrap().deactivated(&c);
        }));
    }
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

    /// Broadcast a copy of `a`, once per redundancy group: to the group's
    /// connection in data transfer, or into its event buffer while none is.
    ///
    /// Stopped connections — a master that has not sent STARTDT yet, or a
    /// standby — are never sent data directly.
    ///
    /// Returns [`Error::NotActive`] when the ASDU went nowhere — no connection
    /// in data transfer and no buffer to keep it; the group's error when
    /// *every* group refused it; and [`Error::PartialBroadcast`] when only some
    /// did — a connection whose queue is full, or a full buffer, has lost the
    /// ASDU, and reporting that is the only way the caller can tell.
    ///
    /// For bulk replies where the data must go out, use
    /// [`Server::send_wait`], which retries the connections that are merely
    /// behind instead of dropping their copy.
    async fn send(&self, a: Asdu) -> Result<()> {
        let routes = self.groups.lock().unwrap().route(&a);
        let results: Vec<Result<()>> = routes
            .into_iter()
            .map(|r| match r {
                Route::Sent | Route::Buffered => Ok(()),
                Route::Refused(c, e) => {
                    tracing::warn!(peer = ?c.peer_addr(), error = %e, "broadcast to session failed");
                    Err(e)
                }
                Route::Lost(e) => Err(e),
            })
            .collect();
        outcome(&results)
    }
}

impl<H: ServerHandler> Server<H> {
    /// Broadcast `a`, waiting for room on a session whose queue is full
    /// instead of losing its copy.
    ///
    /// A session's send buffer is finite, and [`Connect::send`] refuses the
    /// ASDU with [`Error::BufferFull`] rather than making a protocol task
    /// wait. That is the right default — an outstation must not stall its own
    /// APCI state machine because one master is slow — but it puts the burden
    /// on the caller, and the burden is easy to miss: sending a large
    /// interrogation reply in a loop works perfectly on a small database and
    /// quietly loses ASDUs on a large one. The outstation believes it answered
    /// in full; the master has holes it has no way to detect.
    ///
    /// This waits instead, per session, so the sessions that already accepted
    /// the ASDU are never sent a second copy. It blocks the calling task,
    /// never a session's protocol task, so the APCI state machine keeps
    /// running: the buffer drains as the master acknowledges and the wait
    /// ends. Always bound it with `timeout`, so a master that has stopped
    /// acknowledging cannot block a handler for ever.
    ///
    /// Errors other than a full buffer — a closed connection, an ASDU that
    /// will not encode — are not retried, and are reported exactly as
    /// [`Connect::send`] reports them.
    pub async fn send_wait(&self, a: Asdu, timeout: std::time::Duration) -> Result<()> {
        let routes = self.groups.lock().unwrap().route(&a);
        let mut results = Vec::with_capacity(routes.len());
        for r in routes {
            results.push(match r {
                Route::Sent | Route::Buffered => Ok(()),
                // Only a connection that is merely behind is worth waiting
                // for; the retry runs with the lock released.
                Route::Refused(c, Error::BufferFull) => {
                    send_waiting(c.as_ref(), a.clone(), timeout).await
                }
                Route::Refused(_, e) | Route::Lost(e) => Err(e),
            });
        }
        outcome(&results)
    }

    /// The server as a [`Connect`] whose `send` waits for buffer room.
    ///
    /// Lets the [`ConnectExt`](crate::asdu::ConnectExt) helpers push bulk data
    /// without the silent-loss hazard:
    ///
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use std::time::Duration;
    /// # use rs_iec60870_5::asdu::*;
    /// # use rs_iec60870_5::cs104::{Server, ServerHandler};
    /// # struct H;
    /// # #[async_trait::async_trait] impl ServerHandler for H {}
    /// # async fn f(srv: &Arc<Server<H>>, ca: CommonAddr, points: &[SinglePointInfo])
    /// #     -> rs_iec60870_5::Result<()> {
    /// let w = srv.waiting(Duration::from_secs(30));
    /// w.send_single(false, CauseOfTransmission::new(Cause::SPONTANEOUS), ca, points).await?;
    /// # Ok(()) }
    /// ```
    pub fn waiting(self: &Arc<Self>, timeout: std::time::Duration) -> WaitingServer<H> {
        WaitingServer {
            server: Arc::clone(self),
            timeout,
        }
    }
}

/// Retry `send` on one endpoint while its buffer is full, up to `timeout`.
async fn send_waiting<C: Connect + ?Sized>(
    conn: &C,
    a: Asdu,
    timeout: std::time::Duration,
) -> Result<()> {
    /// Long enough that a busy loop does not burn a core, short enough that a
    /// draining queue is picked up promptly.
    const RETRY: std::time::Duration = std::time::Duration::from_millis(2);

    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match conn.send(a.clone()).await {
            // Anything other than a full buffer — a closed connection, a
            // malformed ASDU — will not be fixed by waiting.
            Err(Error::BufferFull) => {}
            other => return other,
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::BufferFull);
        }
        tokio::time::sleep(RETRY.min(deadline - tokio::time::Instant::now())).await;
    }
}

/// Wrap one endpoint so that [`Connect::send`] waits for send-buffer room
/// instead of failing with [`Error::BufferFull`].
///
/// Use it for bulk replies — an interrogation over a large database — where
/// the data must go out and arriving late is better than not arriving. Inside
/// a [`ServerHandler`] the `&dyn Connect` argument is the single session the
/// request came from, which is exactly what this is for:
///
/// ```no_run
/// # use std::time::Duration;
/// # use rs_iec60870_5::asdu::*;
/// # use rs_iec60870_5::cs104::waiting;
/// # async fn interrogation(c: &dyn Connect, pack: &Asdu,
/// #                        batches: &[Vec<MeasuredValueFloatInfo>])
/// #     -> rs_iec60870_5::Result<()> {
/// c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
///
/// // Every send below waits rather than dropping.
/// let w = waiting(c, Duration::from_secs(30));
/// let coa = CauseOfTransmission::new(Cause::INTERROGATED_BY_STATION);
/// for batch in batches {
///     w.send_measured_value_float(false, coa, pack.common_addr(), batch).await?;
/// }
/// c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
/// # }
/// ```
///
/// It blocks the calling task, never a session's own protocol task, so the
/// APCI state machine keeps running: the buffer drains as the master
/// acknowledges, and the wait ends. Always bound `timeout`, so a master that
/// has stopped acknowledging cannot block a handler for ever.
///
/// Do not wrap a [`Server`] with this: retrying a broadcast re-sends to the
/// sessions that already accepted the ASDU. Use [`Server::waiting`] instead,
/// which retries only the sessions that are behind.
pub fn waiting(conn: &dyn Connect, timeout: std::time::Duration) -> Waiting<'_> {
    Waiting { conn, timeout }
}

/// One endpoint viewed as a [`Connect`] whose `send` waits for buffer room.
///
/// Built by [`waiting`].
pub struct Waiting<'a> {
    conn: &'a dyn Connect,
    timeout: std::time::Duration,
}

#[async_trait::async_trait]
impl Connect for Waiting<'_> {
    fn params(&self) -> Params {
        self.conn.params()
    }

    async fn send(&self, a: Asdu) -> Result<()> {
        send_waiting(self.conn, a, self.timeout).await
    }

    fn peer_addr(&self) -> Option<SocketAddr> {
        self.conn.peer_addr()
    }
}

/// A [`Server`] viewed as a [`Connect`] whose `send` waits for buffer room.
///
/// Built by [`Server::waiting`].
pub struct WaitingServer<H: ServerHandler> {
    server: Arc<Server<H>>,
    timeout: std::time::Duration,
}

#[async_trait::async_trait]
impl<H: ServerHandler> Connect for WaitingServer<H> {
    fn params(&self) -> Params {
        self.server.params
    }

    async fn send(&self, a: Asdu) -> Result<()> {
        self.server.send_wait(a, self.timeout).await
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
                callbacks: server_callbacks(&self.handler, None),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::{Cause, CauseOfTransmission, Identifier, TypeId, VariableStruct};
    use std::sync::atomic::AtomicU32;

    fn asdu() -> Asdu {
        Asdu::new(
            PARAMS_WIDE,
            Identifier::new(
                TypeId::C_IC_NA_1,
                VariableStruct::single(),
                CauseOfTransmission::new(Cause::ACTIVATION),
                1,
            ),
        )
    }

    /// An endpoint that refuses the first `full_for` sends with `BufferFull`,
    /// standing in for a session whose queue is draining.
    struct Flaky {
        full_for: AtomicU32,
        accepted: AtomicU32,
        fatal: bool,
    }

    #[async_trait::async_trait]
    impl Connect for Flaky {
        fn params(&self) -> Params {
            PARAMS_WIDE
        }
        async fn send(&self, _: Asdu) -> Result<()> {
            if self.fatal {
                return Err(Error::UseClosedConnection);
            }
            if self.full_for.load(Ordering::Relaxed) > 0 {
                self.full_for.fetch_sub(1, Ordering::Relaxed);
                return Err(Error::BufferFull);
            }
            self.accepted.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    fn flaky(full_for: u32, fatal: bool) -> Flaky {
        Flaky {
            full_for: AtomicU32::new(full_for),
            accepted: AtomicU32::new(0),
            fatal,
        }
    }

    #[tokio::test]
    async fn waiting_retries_a_full_buffer_until_it_drains() {
        let c = flaky(3, false);
        send_waiting(&c, asdu(), std::time::Duration::from_secs(5))
            .await
            .expect("the buffer drains before the deadline");
        assert_eq!(c.accepted.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn waiting_gives_up_at_the_deadline() {
        let c = flaky(u32::MAX, false);
        assert_eq!(
            send_waiting(&c, asdu(), std::time::Duration::from_millis(20)).await,
            Err(Error::BufferFull)
        );
        assert_eq!(c.accepted.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn waiting_does_not_retry_an_error_that_waiting_cannot_fix() {
        // A closed connection is not going to open again; failing fast is the
        // point, otherwise the caller blocks for the whole timeout.
        let c = flaky(0, true);
        let start = tokio::time::Instant::now();
        assert_eq!(
            send_waiting(&c, asdu(), std::time::Duration::from_secs(30)).await,
            Err(Error::UseClosedConnection)
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn a_partial_broadcast_is_distinguishable_from_a_total_one() {
        assert_ne!(
            Error::PartialBroadcast {
                failed: 1,
                total: 3
            },
            Error::PartialBroadcast {
                failed: 2,
                total: 3
            }
        );
        assert_eq!(
            Error::PartialBroadcast {
                failed: 1,
                total: 3
            },
            Error::PartialBroadcast {
                failed: 1,
                total: 3
            }
        );
    }
}
