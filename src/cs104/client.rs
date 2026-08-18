// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The IEC 60870-5-104 controlling station (master / client).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{Notify, RwLock, watch};

use crate::asdu::{
    Asdu, CauseOfTransmission, CommonAddr, Connect, InfoObjAddr, PARAMS_WIDE, Params,
    QualifierCountCall, QualifierOfInterrogation, QualifierOfResetProcessCmd,
};
use crate::cs104::config::Config;
use crate::cs104::connection::{Callbacks, Connection, Role, RunOptions, run};
use crate::cs104::handler::{ClientContext, ClientDispatcher, ClientHandler};
use crate::net::{Endpoint, TlsClientConfig, connect_endpoint};
use crate::error::{Error, Result};

/// Default interval between reconnection attempts.
pub const DEFAULT_RECONNECT_INTERVAL: Duration = Duration::from_secs(60);

/// Configuration for a [`Client`].
#[derive(Clone, Debug)]
pub struct ClientOption {
    config: Config,
    params: Params,
    server: Option<Endpoint>,
    auto_reconnect: bool,
    reconnect_interval: Duration,
    auto_start_dt: bool,
    client_number: usize,
    tls: Option<TlsClientConfig>,
}

impl Default for ClientOption {
    fn default() -> Self {
        ClientOption {
            config: Config::default(),
            params: PARAMS_WIDE,
            server: None,
            auto_reconnect: true,
            reconnect_interval: DEFAULT_RECONNECT_INTERVAL,
            auto_start_dt: true,
            client_number: 0,
            tls: None,
        }
    }
}

impl ClientOption {
    /// Default timing, [`PARAMS_WIDE`] parameters and automatic reconnection.
    pub fn new() -> Self {
        ClientOption::default()
    }

    /// Set the remote endpoint.
    ///
    /// Accepts `host:port`, `:port` (which means `127.0.0.1`), or a URL with a
    /// `tcp://`, `tls://`, `ssl://` or `tcps://` scheme. The TLS schemes
    /// require the `tls` feature and a configuration set with
    /// [`ClientOption::with_tls`].
    pub fn with_server(mut self, server: &str) -> Result<Self> {
        self.server = Some(Endpoint::parse(server)?);
        Ok(self)
    }

    /// Override the timing configuration; invalid values fall back to defaults.
    pub fn with_config(mut self, config: Config) -> Self {
        self.config = config.or_default();
        self
    }

    /// Override the ASDU parameters; invalid values fall back to [`PARAMS_WIDE`].
    pub fn with_params(mut self, params: Params) -> Self {
        self.params = if params.valid().is_ok() {
            params
        } else {
            PARAMS_WIDE
        };
        self
    }

    /// Enable or disable automatic reconnection. Enabled by default.
    pub fn with_auto_reconnect(mut self, on: bool) -> Self {
        self.auto_reconnect = on;
        self
    }

    /// Set the delay between reconnection attempts.
    pub fn with_reconnect_interval(mut self, interval: Duration) -> Self {
        if !interval.is_zero() {
            self.reconnect_interval = interval;
        }
        self
    }

    /// Send `STARTDT act` automatically once connected. Enabled by default.
    ///
    /// Disable it to drive activation by hand with
    /// [`Connection::send_start_dt`]; until StartDT is confirmed the link stays
    /// in STOPDT and [`Client::send`] returns [`Error::NotActive`].
    pub fn with_auto_start_dt(mut self, on: bool) -> Self {
        self.auto_start_dt = on;
        self
    }

    /// Set the client index handed to the handler through [`ClientContext`],
    /// for gateways that multiplex several masters.
    pub fn with_client_number(mut self, n: usize) -> Self {
        self.client_number = n;
        self
    }

    /// Set the TLS client configuration used by the `tls://` schemes.
    #[cfg(feature = "tls")]
    pub fn with_tls(mut self, tls: TlsClientConfig) -> Self {
        self.tls = Some(tls);
        self
    }

    /// The configured timing.
    pub fn config(&self) -> Config {
        self.config
    }

    /// The configured ASDU parameters.
    pub fn params(&self) -> Params {
        self.params
    }

    /// The configured remote endpoint, if any.
    pub fn server(&self) -> Option<Endpoint> {
        self.server.clone()
    }

    /// Whether automatic reconnection is enabled.
    pub fn auto_reconnect(&self) -> bool {
        self.auto_reconnect
    }

    /// The delay between reconnection attempts.
    pub fn reconnect_interval(&self) -> Duration {
        self.reconnect_interval
    }

    pub(crate) fn tls(&self) -> Option<&TlsClientConfig> {
        self.tls.as_ref()
    }
}

/// An IEC 60870-5-104 master.
///
/// Connects to a controlled station, keeps the connection up (reconnecting on
/// its own unless disabled), and delivers received ASDUs to a
/// [`ClientHandler`].
///
/// The client itself implements [`Connect`], so every
/// [`ConnectExt`](crate::asdu::ConnectExt) helper works against it and is
/// routed to the current connection.
pub struct Client<H: ClientHandler> {
    option: ClientOption,
    handler: Arc<H>,
    conn: RwLock<Option<Arc<Connection>>>,
    activated: Notify,
    shutdown: watch::Sender<bool>,
    started: AtomicBool,
}

impl<H: ClientHandler> Client<H> {
    /// Build a master. Call [`Client::start`] to bring the connection up.
    pub fn new(handler: H, option: ClientOption) -> Arc<Self> {
        Arc::new(Client {
            option,
            handler: Arc::new(handler),
            conn: RwLock::new(None),
            activated: Notify::new(),
            shutdown: watch::channel(false).0,
            started: AtomicBool::new(false),
        })
    }

    /// The ASDU parameters in use.
    pub fn params(&self) -> Params {
        self.option.params
    }

    /// The client index reported to the handler.
    pub fn client_number(&self) -> usize {
        self.option.client_number
    }

    /// Whether the TCP connection is currently up.
    pub async fn is_connected(&self) -> bool {
        self.conn
            .read()
            .await
            .as_ref()
            .is_some_and(|c| c.is_connected())
    }

    /// Whether data transfer is active, i.e. StartDT has been confirmed.
    pub async fn is_active(&self) -> bool {
        self.conn.read().await.as_ref().is_some_and(|c| c.is_active())
    }

    /// The current connection handle, if any.
    pub async fn connection(&self) -> Option<Arc<Connection>> {
        self.conn.read().await.clone()
    }

    /// Start connecting in the background; returns immediately.
    ///
    /// Calling it more than once is a no-op.
    pub fn start(self: &Arc<Self>) -> Result<()> {
        if self.option.server.is_none() {
            return Err(Error::Config("no remote server configured"));
        }
        if self.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let this = Arc::clone(self);
        tokio::spawn(async move { this.run_forever().await });
        Ok(())
    }

    /// Stop the client and close the connection.
    pub fn close(&self) {
        let _ = self.shutdown.send(true);
    }

    /// Wait until data transfer is active.
    ///
    /// Returns immediately when it already is. Combine with
    /// [`tokio::time::timeout`] when the wait must be bounded.
    pub async fn wait_active(&self) {
        loop {
            let notified = self.activated.notified();
            if self.is_active().await {
                return;
            }
            notified.await;
        }
    }

    async fn run_forever(self: Arc<Self>) {
        let endpoint = self.option.server.clone().expect("checked in start()");
        let mut shutdown = self.shutdown.subscribe();

        loop {
            if *shutdown.borrow() {
                return;
            }

            tracing::debug!(%endpoint, "connecting");
            let stream = tokio::select! {
                _ = shutdown.changed() => return,
                r = connect_endpoint(
                    &endpoint,
                    self.option.tls.as_ref(),
                    self.option.config.connect_timeout0,
                ) => r,
            };

            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(%endpoint, error = %e, "connect failed");
                    if !self.option.auto_reconnect {
                        return;
                    }
                    tokio::select! {
                        _ = shutdown.changed() => return,
                        _ = tokio::time::sleep(self.option.reconnect_interval) => continue,
                    }
                }
            };
            tracing::debug!(%endpoint, "connected");

            let peer = stream.peer_addr();
            let dispatcher = Arc::new(ClientDispatcher {
                handler: Arc::clone(&self.handler),
                ctx: ClientContext {
                    client_number: self.option.client_number,
                },
            });

            let opts = RunOptions {
                config: self.option.config,
                params: self.option.params,
                role: Role::Master,
                peer,
                auto_start_dt: self.option.auto_start_dt,
                callbacks: self.callbacks(),
            };

            let this = Arc::clone(&self);
            run(
                stream,
                opts,
                dispatcher,
                self.shutdown.subscribe(),
                move |c| {
                    // Publishing is synchronous inside `run`; the write lock is
                    // never contended here because no connection is live yet.
                    if let Ok(mut guard) = this.conn.try_write() {
                        *guard = Some(c);
                    }
                },
            )
            .await;

            *self.conn.write().await = None;
            tracing::debug!(%endpoint, "disconnected");

            if *shutdown.borrow() {
                return;
            }
            // Jittered 500-1000 ms pause, so a server that rejects connections
            // is not hammered and many clients do not retry in lockstep.
            tokio::select! {
                _ = shutdown.changed() => return,
                _ = tokio::time::sleep(retry_jitter()) => {}
            }
        }
    }

    fn callbacks(self: &Arc<Self>) -> Callbacks {
        let mut cb = Callbacks::default();
        let h = Arc::clone(&self.handler);
        cb.on_connect = Some(Arc::new(move |c: Arc<Connection>| {
            let h = Arc::clone(&h);
            tokio::spawn(async move { h.on_connect(c.as_ref()).await });
        }));

        let h = Arc::clone(&self.handler);
        let this = Arc::clone(self);
        cb.on_activated = Some(Arc::new(move |c: Arc<Connection>| {
            this.activated.notify_waiters();
            let h = Arc::clone(&h);
            tokio::spawn(async move { h.on_activated(c.as_ref()).await });
        }));

        let h = Arc::clone(&self.handler);
        cb.on_deactivated = Some(Arc::new(move |c: Arc<Connection>| {
            let h = Arc::clone(&h);
            tokio::spawn(async move { h.on_deactivated(c.as_ref()).await });
        }));

        let h = Arc::clone(&self.handler);
        cb.on_connection_lost = Some(Arc::new(move |c: Arc<Connection>| {
            let h = Arc::clone(&h);
            tokio::spawn(async move { h.on_connection_lost(c.as_ref()).await });
        }));
        cb
    }

    // -- command wrappers, mirroring go-iecp5's Client methods -------------

    /// Send `C_IC_NA_1`: an interrogation command.
    pub async fn interrogation_cmd(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        qoi: QualifierOfInterrogation,
    ) -> Result<()> {
        self.send(Asdu::interrogation_cmd(self.params(), coa, ca, qoi)?)
            .await
    }

    /// Send `C_CI_NA_1`: a counter interrogation command.
    pub async fn counter_interrogation_cmd(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        qcc: QualifierCountCall,
    ) -> Result<()> {
        self.send(Asdu::counter_interrogation_cmd(self.params(), coa, ca, qcc)?)
            .await
    }

    /// Send `C_RD_NA_1`: a read command.
    pub async fn read_cmd(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        ioa: InfoObjAddr,
    ) -> Result<()> {
        self.send(Asdu::read_cmd(self.params(), coa, ca, ioa)?).await
    }

    /// Send `C_CS_NA_1`: a clock synchronization command.
    pub async fn clock_synchronization_cmd(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        t: DateTime<Utc>,
    ) -> Result<()> {
        self.send(Asdu::clock_synchronization_cmd(self.params(), coa, ca, t)?)
            .await
    }

    /// Send `C_RP_NA_1`: a reset process command.
    pub async fn reset_process_cmd(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        qrp: QualifierOfResetProcessCmd,
    ) -> Result<()> {
        self.send(Asdu::reset_process_cmd(self.params(), coa, ca, qrp)?)
            .await
    }

    /// Send `C_CD_NA_1`: a delay acquisition command.
    pub async fn delay_acquire_command(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        msec: u16,
    ) -> Result<()> {
        self.send(Asdu::delay_acquire_command(self.params(), coa, ca, msec)?)
            .await
    }

    /// Send `C_TS_NA_1`: a test command.
    pub async fn test_command(&self, coa: CauseOfTransmission, ca: CommonAddr) -> Result<()> {
        self.send(Asdu::test_command(self.params(), coa, ca)?).await
    }

    /// Request start of data transfer on the current connection.
    pub async fn send_start_dt(&self) {
        if let Some(c) = self.connection().await {
            c.send_start_dt();
        }
    }

    /// Request stop of data transfer on the current connection.
    pub async fn send_stop_dt(&self) {
        if let Some(c) = self.connection().await {
            c.send_stop_dt();
        }
    }
}

#[async_trait::async_trait]
impl<H: ClientHandler> Connect for Client<H> {
    fn params(&self) -> Params {
        self.option.params
    }

    async fn send(&self, a: Asdu) -> Result<()> {
        let conn = self
            .conn
            .read()
            .await
            .clone()
            .ok_or(Error::UseClosedConnection)?;
        conn.send(a).await
    }
}

/// A 500-1000 ms pause, derived from the clock so that many clients restarting
/// together do not retry in lockstep.
fn retry_jitter() -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    Duration::from_millis(500 + u64::from(nanos % 500))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_jitter_stays_in_range() {
        for _ in 0..100 {
            let d = retry_jitter();
            assert!(d >= Duration::from_millis(500) && d < Duration::from_millis(1000));
        }
    }

    #[test]
    fn option_falls_back_to_defaults_on_invalid_input() {
        let o = ClientOption::new()
            .with_config(Config {
                send_unack_timeout1: Duration::from_secs(999),
                ..Default::default()
            })
            .with_params(Params {
                info_obj_addr_size: 9,
                ..PARAMS_WIDE
            });
        assert_eq!(o.config(), Config::default());
        assert_eq!(o.params(), PARAMS_WIDE);
    }

    #[test]
    fn starting_without_a_server_is_an_error() {
        struct H;
        impl ClientHandler for H {}
        let c = Client::new(H, ClientOption::new());
        assert_eq!(
            c.start(),
            Err(Error::Config("no remote server configured"))
        );
    }
}
