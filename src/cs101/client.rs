// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The IEC 60870-5-101 primary station (master).
//!
//! The client drives the link procedure by itself: request status of link,
//! reset of remote link, then continuous class 2 polling with a class 1 request
//! whenever a response sets the ACD bit. The frame count bit is tracked per
//! station across every FCV frame, and a frame that goes unanswered is repeated
//! once with the same FCB, per IEC 60870-5-2.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, mpsc, watch};

use crate::asdu::{
    Asdu, CauseOfTransmission, CommonAddr, Connect, InfoObjAddr, PARAMS_STANDARD_101, Params,
    QualifierCountCall, QualifierOfInterrogation, QualifierOfResetProcessCmd,
};
use crate::cs101::config::Config;
use crate::cs101::frame::{ControlField, Frame, fcv_required, prim_fc, read_frame, sec_fc};
use crate::cs101::handler::{ClientHandler, dispatch_client};
use crate::cs101::transport::Transporter;
use crate::error::{Error, Result};

/// Default interval between reconnection attempts.
pub const DEFAULT_RECONNECT_INTERVAL: Duration = Duration::from_secs(60);

/// Where a secondary station is in the link initialization procedure, as seen
/// by the primary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Request status of link is due.
    Status,
    /// Reset of remote link is due.
    Reset,
    /// The link layer is active and carrying data.
    Active,
}

/// Per-station link state kept by the primary.
///
/// The FCB alternates per station across **all** frames sent with FCV = 1 —
/// user data, class 1/2 requests and test link alike — as IEC 60870-5-2
/// requires.
#[derive(Debug)]
struct Secondary {
    phase: Phase,
    /// FCB of the next FCV frame sent to this station.
    fcb: bool,
    /// A response set ACD: a class 1 request is due.
    want_class1: bool,
    /// The last response set DFC: the station's buffers are full, so it must
    /// not be sent more user data until it clears the bit.
    ///
    /// See IEC 60870-5-2, subclass 5.1.3. Class polls and link management stay
    /// allowed — they are what lets the station report that it has drained.
    busy: bool,
}

/// What a positive confirmation completed.
struct Confirmed {
    /// Function code of the primary frame that was confirmed.
    fun: u8,
    /// The station whose link it activated, when it was a link reset.
    became_active: Option<u16>,
}

/// What handling one received frame produced.
#[derive(Default)]
struct FrameOutcome {
    /// Stations whose link just became active.
    newly_active: Vec<u16>,
    /// The frame carries user data that is new and belongs to the
    /// application. False for a repeat, an unsolicited response, and anything
    /// from a station this client does not serve.
    deliver: bool,
}

/// A confirmed frame awaiting an acknowledgement.
struct Pending {
    frame: Frame,
    ctrl: ControlField,
    addr: u16,
}

/// Configuration for a [`Client`].
#[derive(Debug, Clone)]
pub struct ClientOption {
    config: Config,
    params: Params,
    auto_reconnect: bool,
    reconnect_interval: Duration,
    secondary_addrs: Vec<u16>,
    client_number: usize,
}

impl Default for ClientOption {
    fn default() -> Self {
        ClientOption {
            config: Config::default(),
            params: PARAMS_STANDARD_101,
            auto_reconnect: true,
            reconnect_interval: DEFAULT_RECONNECT_INTERVAL,
            secondary_addrs: Vec::new(),
            client_number: 0,
        }
    }
}

impl ClientOption {
    /// Default link configuration and [`PARAMS_STANDARD_101`] parameters.
    pub fn new() -> Self {
        ClientOption::default()
    }

    /// Set the link configuration. Rejected if invalid.
    pub fn with_config(mut self, mut config: Config) -> Result<Self> {
        config.valid()?;
        self.config = config;
        Ok(self)
    }

    /// Set the ASDU parameters; invalid values fall back to
    /// [`PARAMS_STANDARD_101`].
    pub fn with_params(mut self, params: Params) -> Self {
        self.params = if params.valid().is_ok() {
            params
        } else {
            PARAMS_STANDARD_101
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

    /// Add a secondary station to poll (unbalanced multi-drop).
    ///
    /// With no address added, `Config::link_address` is the single secondary.
    /// Duplicates are ignored. Not used in balanced mode.
    pub fn with_secondary_address(mut self, addr: u16) -> Self {
        if !self.secondary_addrs.contains(&addr) {
            self.secondary_addrs.push(addr);
        }
        self
    }

    /// Set the client index reported to the handler.
    pub fn with_client_number(mut self, n: usize) -> Self {
        self.client_number = n;
        self
    }

    /// The configured link settings.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The configured ASDU parameters.
    pub fn params(&self) -> Params {
        self.params
    }

    /// The link address commands are sent to by default.
    pub fn default_addr(&self) -> u16 {
        if !self.secondary_addrs.is_empty() && !self.config.is_balanced() {
            self.secondary_addrs[0]
        } else {
            self.config.link_address
        }
    }

    /// Every secondary station this client serves.
    fn addresses(&self) -> Vec<u16> {
        if self.secondary_addrs.is_empty() || self.config.is_balanced() {
            vec![self.config.link_address]
        } else {
            self.secondary_addrs.clone()
        }
    }
}

/// One ASDU queued for a specific secondary station.
struct Outgoing {
    asdu: Asdu,
    addr: u16,
}

/// State shared between the application and the protocol task.
struct Shared {
    params: Params,
    max_queue: usize,
    default_addr: u16,
    connected: AtomicBool,
    link_active: AtomicBool,
    queue: Mutex<VecDeque<Outgoing>>,
    link_req: mpsc::UnboundedSender<u8>,
    active_notify: Notify,
}

/// An IEC 60870-5-101 primary station.
///
/// Implements [`Connect`], so every [`ConnectExt`](crate::asdu::ConnectExt)
/// helper works against it; ASDUs are queued and transmitted as confirmed user
/// data once the target station's link is free and active.
pub struct Client<H: ClientHandler> {
    option: ClientOption,
    handler: Arc<H>,
    shared: Arc<Shared>,
    link_req_rx: Mutex<Option<mpsc::UnboundedReceiver<u8>>>,
    shutdown: watch::Sender<bool>,
    started: AtomicBool,
}

impl<H: ClientHandler> Client<H> {
    /// Build a primary station. Call [`Client::start`] to open the line.
    pub fn new(handler: H, option: ClientOption) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        Arc::new(Client {
            shared: Arc::new(Shared {
                params: option.params,
                max_queue: option.config.max_send_queue_size,
                default_addr: option.default_addr(),
                connected: AtomicBool::new(false),
                link_active: AtomicBool::new(false),
                queue: Mutex::new(VecDeque::new()),
                link_req: tx,
                active_notify: Notify::new(),
            }),
            option,
            handler: Arc::new(handler),
            link_req_rx: Mutex::new(Some(rx)),
            shutdown: watch::channel(false).0,
            started: AtomicBool::new(false),
        })
    }

    /// The ASDU parameters in use.
    pub fn params(&self) -> Params {
        self.option.params
    }

    /// Whether the transport is open.
    pub fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::Acquire)
    }

    /// Whether at least one secondary station's link layer is active.
    pub fn is_link_active(&self) -> bool {
        self.is_connected() && self.shared.link_active.load(Ordering::Acquire)
    }

    /// Wait until at least one secondary station's link becomes active.
    pub async fn wait_link_active(&self) {
        loop {
            let notified = self.shared.active_notify.notified();
            if self.is_link_active() {
                return;
            }
            notified.await;
        }
    }

    /// Open the line in the background; returns immediately.
    pub fn start(self: &Arc<Self>) -> Result<()> {
        if self.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let rx = self
            .link_req_rx
            .lock()
            .unwrap()
            .take()
            .ok_or(Error::Config("client already started"))?;
        let this = Arc::clone(self);
        tokio::spawn(async move { this.run_forever(rx).await });
        Ok(())
    }

    /// Stop the client and close the line.
    pub fn close(&self) {
        let _ = self.shutdown.send(true);
    }

    async fn run_forever(self: Arc<Self>, mut link_req: mpsc::UnboundedReceiver<u8>) {
        let transporter = Transporter::new(self.option.config.clone());
        let mut shutdown = self.shutdown.subscribe();

        loop {
            if *shutdown.borrow() {
                return;
            }

            tracing::debug!(
                transport = %self.option.config.transport,
                endpoint = self.option.config.transport_label(),
                "opening the line"
            );
            let opened = tokio::select! {
                _ = shutdown.changed() => return,
                r = transporter.open() => r,
            };

            let (stream, desc) = match opened {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(error = %e, "failed to open the line");
                    if !self.option.auto_reconnect {
                        return;
                    }
                    tokio::select! {
                        _ = shutdown.changed() => return,
                        _ = tokio::time::sleep(self.option.reconnect_interval) => continue,
                    }
                }
            };
            tracing::debug!(%desc, "line open");

            self.shared.connected.store(true, Ordering::Release);
            let err = self.run_link(stream, &mut link_req).await;
            self.shared.connected.store(false, Ordering::Release);
            self.shared.link_active.store(false, Ordering::Release);
            // A new connection starts from an empty queue: anything still
            // queued was addressed to a link that no longer exists.
            self.shared.queue.lock().unwrap().clear();

            match err {
                Some(e) => tracing::warn!(error = %e, "link stopped"),
                None => tracing::debug!("link stopped"),
            }
            self.handler.on_connection_lost(self.as_connect()).await;

            if *shutdown.borrow() || !self.option.auto_reconnect {
                return;
            }
            tokio::select! {
                _ = shutdown.changed() => return,
                _ = tokio::time::sleep(self.option.reconnect_interval) => {}
            }
        }
    }

    fn as_connect(&self) -> &dyn Connect {
        self
    }

    /// Run the link procedure over one open stream, returning the error that
    /// ended it (if any).
    async fn run_link(
        &self,
        stream: crate::cs101::transport::LinkStream,
        link_req: &mut mpsc::UnboundedReceiver<u8>,
    ) -> Option<Error> {
        let cfg = &self.option.config;
        let addr_size = cfg.link_addr_size;
        let balanced = cfg.is_balanced();

        let (mut reader, mut writer) = tokio::io::split(stream);
        let (tx_frame, mut rx_frame) = mpsc::channel::<Frame>(20);
        let (tx_out, mut rx_out) = mpsc::channel::<Vec<u8>>(20);
        let (stop_tx, stop_rx) = watch::channel(false);

        let reader_task = {
            let mut stop = stop_rx.clone();
            tokio::spawn(async move {
                loop {
                    let r = tokio::select! {
                        _ = stop.changed() => return,
                        r = read_frame(&mut reader, addr_size) => r,
                    };
                    match r {
                        Ok(frame) => {
                            if tx_frame.send(frame).await.is_err() {
                                return;
                            }
                        }
                        // A framing or checksum error is line noise: log it and
                        // resynchronise on the next octet.
                        Err(Error::Frame(what)) => {
                            tracing::warn!(reason = what, "discarding malformed frame")
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, "receive stopped");
                            return;
                        }
                    }
                }
            })
        };

        let writer_task = {
            let mut stop = stop_rx.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = stop.changed() => break,
                        raw = rx_out.recv() => {
                            let Some(raw) = raw else { break };
                            tracing::trace!(frame = ?raw, "TX raw");
                            if let Err(e) = writer.write_all(&raw).await {
                                tracing::debug!(error = %e, "send failed");
                                break;
                            }
                        }
                    }
                }
                let _ = writer.shutdown().await;
            })
        };

        let mut link = LinkState::new(&self.option, tx_out.clone());
        let mut shutdown = self.shutdown.subscribe();
        let mut on_connect_called = false;
        let mut fatal: Option<Error> = None;

        let mut tick_at = Instant::now() + cfg.timeout_send_link_msg;
        let mut t3_at = Instant::now() + cfg.timeout_test_t3;

        loop {
            let next = [Some(tick_at), link.t1_at, Some(t3_at)]
                .into_iter()
                .flatten()
                .min()
                .expect("tick is always scheduled");

            tokio::select! {
                _ = shutdown.changed() => break,

                frame = rx_frame.recv() => {
                    let Some(frame) = frame else { break };
                    tracing::debug!(?frame, "RX frame");
                    // Any received frame proves the line is alive.
                    t3_at = Instant::now() + cfg.timeout_test_t3;

                    if !on_connect_called {
                        on_connect_called = true;
                        self.handler.on_connect(self.as_connect()).await;
                    }

                    let outcome = link.handle_frame(&frame, balanced).await;
                    for addr in outcome.newly_active {
                        self.handler.on_link_active(self.as_connect(), addr).await;
                    }
                    self.publish_link_active(&link);

                    if let Some(asdu) = frame.asdu().filter(|_| outcome.deliver) {
                        match Asdu::unmarshal_binary(self.option.params, asdu) {
                            Ok(pack) => {
                                dispatch_client(
                                    &*self.handler,
                                    self.as_connect(),
                                    &pack,
                                    self.option.client_number,
                                )
                                .await
                            }
                            Err(e) => tracing::warn!(error = %e, "discarding undecodable ASDU"),
                        }
                    }
                }

                req = link_req.recv() => {
                    let Some(fun) = req else { break };
                    link.handle_link_request(fun, balanced).await;
                }

                _ = tokio::time::sleep_until(next.into()) => {
                    let now = Instant::now();

                    if link.t1_at.is_some_and(|t| now >= t) {
                        match link.on_t1_timeout(cfg).await {
                            Ok(()) => {}
                            Err(e) => { fatal = Some(e); break; }
                        }
                        self.publish_link_active(&link);
                    }

                    if now >= t3_at {
                        t3_at = now + cfg.timeout_test_t3;
                        link.on_t3_timeout(cfg).await;
                    }

                    if now >= tick_at {
                        tick_at = now + cfg.timeout_send_link_msg;
                        link.tick(&self.shared, balanced).await;
                    }
                }
            }
        }

        let _ = stop_tx.send(true);
        drop(tx_out);
        let _ = tokio::join!(reader_task, writer_task);
        fatal
    }

    fn publish_link_active(&self, link: &LinkState) {
        let active = link.any_active();
        self.shared.link_active.store(active, Ordering::Release);
        if active {
            self.shared.active_notify.notify_waiters();
        }
    }

    /// Queue an ASDU for the secondary station at `link_addr`.
    ///
    /// Use this on a multi-drop line; [`Connect::send`] targets the first
    /// configured address.
    pub async fn send_to(&self, a: Asdu, link_addr: u16) -> Result<()> {
        if !self.is_connected() {
            return Err(Error::UseClosedConnection);
        }
        let mut q = self.shared.queue.lock().unwrap();
        if q.len() >= self.shared.max_queue {
            return Err(Error::SendQueueFull);
        }
        q.push_back(Outgoing {
            asdu: a,
            addr: link_addr,
        });
        Ok(())
    }

    // -- manual link-layer actions ----------------------------------------

    /// Request a reset of the remote link of the default secondary station.
    ///
    /// Executed by the protocol loop once the link is free; the client runs
    /// this procedure automatically, so it is rarely needed.
    pub fn send_reset_link(&self) -> Result<()> {
        self.queue_link_req(prim_fc::RESET_LINK)
    }

    /// Request the status of the default secondary station's link.
    pub fn send_request_link_status(&self) -> Result<()> {
        self.queue_link_req(prim_fc::REQ_STATUS)
    }

    /// Poll the default secondary station for class 2 data.
    pub fn send_req_data_class2(&self) -> Result<()> {
        self.queue_link_req(prim_fc::REQ_DATA2)
    }

    fn queue_link_req(&self, fun: u8) -> Result<()> {
        if !self.is_connected() {
            return Err(Error::UseClosedConnection);
        }
        self.shared
            .link_req
            .send(fun)
            .map_err(|_| Error::UseClosedConnection)
    }

    // -- command wrappers --------------------------------------------------

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
}

#[async_trait::async_trait]
impl<H: ClientHandler> Connect for Client<H> {
    fn params(&self) -> Params {
        self.shared.params
    }

    async fn send(&self, a: Asdu) -> Result<()> {
        self.send_to(a, self.shared.default_addr).await
    }
}

/// The link state machine, owned by the protocol task.
struct LinkState {
    secs: HashMap<u16, Secondary>,
    order: Vec<u16>,
    poll_idx: usize,
    last_sent: Option<Pending>,
    retry_count: u32,
    /// Balanced mode: the FCB expected on the peer's next FCV frame.
    fcb_expected: bool,
    /// Acknowledge the peer's frames with `E5` in balanced mode.
    single_char_ack: bool,
    t1_at: Option<Instant>,
    addr_size: u8,
    dir: bool,
    /// t1, cached so the state machine does not need the whole config.
    t1_duration: Duration,
    /// The station manual link requests are addressed to.
    default_addr: u16,
    out: mpsc::Sender<Vec<u8>>,
}

impl LinkState {
    fn new(option: &ClientOption, out: mpsc::Sender<Vec<u8>>) -> LinkState {
        let order = option.addresses();
        let secs = order
            .iter()
            .map(|a| {
                (
                    *a,
                    Secondary {
                        phase: Phase::Status,
                        fcb: false,
                        want_class1: false,
                        busy: false,
                    },
                )
            })
            .collect();
        LinkState {
            secs,
            order,
            poll_idx: 0,
            last_sent: None,
            retry_count: 0,
            fcb_expected: true,
            t1_at: None,
            addr_size: option.config.link_addr_size,
            // In balanced mode this station is station A and sets DIR.
            dir: option.config.is_balanced(),
            single_char_ack: option.config.use_single_char_ack,
            t1_duration: option.config.timeout_response_t1,
            default_addr: option.default_addr(),
            out,
        }
    }

    fn any_active(&self) -> bool {
        self.order
            .iter()
            .any(|a| self.secs[a].phase == Phase::Active)
    }

    fn first_active(&self) -> Option<u16> {
        self.order
            .iter()
            .copied()
            .find(|a| self.secs[a].phase == Phase::Active)
    }

    async fn write(&self, frame: &Frame) -> bool {
        match frame.marshal(self.addr_size) {
            Ok(raw) => self.out.send(raw).await.is_ok(),
            Err(e) => {
                tracing::error!(error = %e, "failed to encode a frame");
                false
            }
        }
    }

    /// Send a confirmed primary frame and arm the t₁ response timer.
    async fn send_prim_confirmed(
        &mut self,
        addr: u16,
        fun: u8,
        fcv: bool,
        t1: Duration,
    ) -> bool {
        let fcb = self.secs[&addr].fcb;
        let ctrl = ControlField::primary(fun, fcv, fcb, self.dir);
        let frame = Frame::Fixed {
            control: ctrl,
            link_addr: addr,
        };
        if !self.write(&frame).await {
            return false;
        }
        self.last_sent = Some(Pending { frame, ctrl, addr });
        self.retry_count = 0;
        self.t1_at = Some(Instant::now() + t1);
        true
    }

    /// Send a secondary (PRM = 0) response; used by the balanced-mode
    /// secondary role of this station.
    async fn send_sec_fixed(&self, fun: u8, addr: u16) -> bool {
        // A positive acknowledgement may be the single character E5 when that
        // is configured; balanced mode has no access demand to lose by it.
        let frame = if fun == sec_fc::CONF_ACK && self.single_char_ack {
            Frame::SingleCharAck
        } else {
            Frame::Fixed {
                control: ControlField::secondary(fun, false, self.dir),
                link_addr: addr,
            }
        };
        self.write(&frame).await
    }

    /// Complete the outstanding transaction positively.
    ///
    /// `addr_valid` is false for a single-character acknowledgement, which
    /// carries no link address. Returns `None` when there was no transaction
    /// from that station to complete.
    fn confirm_positive(&mut self, addr: u16, addr_valid: bool) -> Option<Confirmed> {
        let Some(p) = self.last_sent.take() else {
            tracing::warn!("received a confirmation with no frame outstanding");
            return None;
        };
        if addr_valid && p.addr != addr {
            tracing::warn!(
                got = addr,
                expected = p.addr,
                "confirmation from the wrong station, ignored"
            );
            // Not ours: keep waiting for the real answer.
            self.last_sent = Some(p);
            return None;
        }

        let mut became_active = None;
        if let Some(sec) = self.secs.get_mut(&p.addr) {
            if p.ctrl.fcv {
                // The FCB only toggles once the frame is positively confirmed.
                sec.fcb = !p.ctrl.fcb;
            }
            match p.ctrl.fun {
                prim_fc::RESET_LINK => {
                    tracing::debug!(station = p.addr, "link reset confirmed, link active");
                    sec.phase = Phase::Active;
                    // The first FCV frame after a reset carries FCB = 1.
                    sec.fcb = true;
                    became_active = Some(p.addr);
                }
                prim_fc::REQ_STATUS => {
                    if sec.phase == Phase::Status {
                        sec.phase = Phase::Reset;
                    }
                }
                prim_fc::REQ_DATA1 => sec.want_class1 = false,
                _ => {}
            }
        }
        self.retry_count = 0;
        self.t1_at = None;
        Some(Confirmed {
            fun: p.ctrl.fun,
            became_active,
        })
    }

    /// Abort the outstanding transaction without toggling the FCB and send the
    /// station back to the start of the initialization procedure.
    fn fail_transaction(&mut self, addr: u16) {
        let Some(p) = self.last_sent.take() else { return };
        if p.addr != addr {
            self.last_sent = Some(p);
            return;
        }
        if let Some(sec) = self.secs.get_mut(&p.addr) {
            sec.phase = Phase::Status;
            sec.want_class1 = false;
            sec.busy = false;
        }
        self.retry_count = 0;
        self.t1_at = None;
    }

    /// Process one received frame.
    async fn handle_frame(&mut self, frame: &Frame, balanced: bool) -> FrameOutcome {
        let mut out = FrameOutcome::default();

        // A single-character acknowledgement carries neither address nor
        // control field.
        if matches!(frame, Frame::SingleCharAck) {
            if let Some(c) = self.confirm_positive(0, false) {
                out.newly_active.extend(c.became_active);
            }
            return out;
        }

        let addr = frame.link_addr().unwrap_or(0);
        if self.addr_size > 0 && !self.secs.contains_key(&addr) {
            tracing::warn!(addr, "ignoring a frame from an unexpected link address");
            return out;
        }
        let ctrl = frame.control().expect("not a single character ack");

        if ctrl.prm {
            // Only meaningful in balanced mode, where the peer also acts as a
            // primary station.
            if !balanced {
                tracing::warn!(%ctrl, "unexpected PRM=1 frame in unbalanced mode");
                return out;
            }
            out.deliver = self.handle_peer_primary_frame(ctrl, addr).await;
            return out;
        }

        // DFC is carried by every secondary frame, so each one is also the
        // station's latest word on whether it can take more user data.
        if let Some(sec) = self.secs.get_mut(&addr) {
            if ctrl.dfc && !sec.busy {
                tracing::warn!(addr, "station signals data flow control, holding user data");
            } else if !ctrl.dfc && sec.busy {
                tracing::debug!(addr, "station cleared data flow control");
            }
            sec.busy = ctrl.dfc;
        }

        let mut confirm = |link: &mut LinkState| {
            if let Some(c) = link.confirm_positive(addr, true) {
                out.newly_active.extend(c.became_active);
                Some(c.fun)
            } else {
                None
            }
        };

        match frame {
            Frame::Fixed { .. } => match ctrl.fun {
                sec_fc::CONF_ACK => {
                    confirm(self);
                }
                // Link busy: keep the frame outstanding, t1 will repeat it.
                sec_fc::CONF_NACK => tracing::warn!(addr, "station reports the link busy"),
                sec_fc::USER_DATA_NO_REP => {
                    tracing::debug!(addr, "requested data not available");
                    confirm(self);
                }
                sec_fc::RESP_STATUS => {
                    tracing::debug!(addr, dfc = ctrl.dfc, "received link status");
                    confirm(self);
                }
                sec_fc::RESP_LINK_NF | sec_fc::RESP_LINK_NI => {
                    tracing::warn!(addr, fc = ctrl.fun, "link service not available");
                    self.fail_transaction(addr);
                }
                _ => tracing::warn!(addr, %ctrl, "unhandled fixed-length frame"),
            },

            Frame::Variable { .. } => match ctrl.fun {
                sec_fc::USER_DATA_CONF => {
                    // User data is only ever the answer to a class 1 or class 2
                    // request. Anything else is a response that arrives after
                    // its request was repeated — the second copy of the same
                    // ASDU — or one from a station that was never asked, and
                    // delivering it would hand the application the same event
                    // or command confirmation twice.
                    match confirm(self) {
                        Some(prim_fc::REQ_DATA1 | prim_fc::REQ_DATA2) => {
                            tracing::debug!(addr, "received user data");
                            out.deliver = true;
                        }
                        _ => tracing::warn!(addr, "unsolicited user data discarded"),
                    }
                }
                sec_fc::RESP_STATUS | sec_fc::USER_DATA_NO_REP => {
                    tracing::debug!(addr, fc = ctrl.fun, "response without user data");
                    confirm(self);
                }
                _ => tracing::warn!(addr, %ctrl, "unhandled variable-length frame"),
            },

            Frame::SingleCharAck => unreachable!("handled above"),
        }

        // Access demand: the station has class 1 data waiting. Fetch it now
        // if the link just went idle, otherwise the next poll will.
        if ctrl.acd && !balanced && self.secs.contains_key(&addr) {
            let due = {
                let sec = self.secs.get_mut(&addr).expect("checked");
                sec.want_class1 = true;
                sec.phase == Phase::Active
            };
            if due && self.last_sent.is_none() {
                let t1 = self.t1_duration;
                self.send_prim_confirmed(addr, prim_fc::REQ_DATA1, true, t1)
                    .await;
            }
        }
        out
    }

    /// Service the secondary role of this station in balanced mode: the peer
    /// acts as a primary and sends link commands and user data to acknowledge.
    ///
    /// Returns whether the frame's user data is new and should be delivered.
    async fn handle_peer_primary_frame(&mut self, ctrl: ControlField, addr: u16) -> bool {
        // A frame whose FCV contradicts its function code was not produced by a
        // conforming peer, and acting on it would move the FCB.
        if fcv_required(ctrl.fun).is_some_and(|fcv| fcv != ctrl.fcv) {
            tracing::warn!(%ctrl, "FCV does not match the function code, frame ignored");
            return false;
        }

        // Duplicate detection over the peer's FCV frames. A repeat means the
        // acknowledgement was lost: acknowledge again, but the data was
        // already delivered the first time.
        if ctrl.fcv {
            if ctrl.fcb != self.fcb_expected {
                tracing::warn!("repeated peer frame, re-sending the acknowledgement");
                self.send_sec_fixed(sec_fc::CONF_ACK, addr).await;
                return false;
            }
            self.fcb_expected = !self.fcb_expected;
        }

        match ctrl.fun {
            prim_fc::RESET_LINK => {
                tracing::debug!("the peer requested a link reset");
                // The first FCV frame after a reset carries FCB = 1.
                self.fcb_expected = true;
                self.send_sec_fixed(sec_fc::CONF_ACK, addr).await;
            }
            prim_fc::RESET_USER | prim_fc::TEST_LINK => {
                self.send_sec_fixed(sec_fc::CONF_ACK, addr).await;
            }
            prim_fc::USER_DATA_CONF => {
                tracing::debug!("the peer sent confirmed user data");
                self.send_sec_fixed(sec_fc::CONF_ACK, addr).await;
                return true;
            }
            prim_fc::USER_DATA_NO_CONF => {
                tracing::debug!("the peer sent unconfirmed user data");
                return true;
            }
            prim_fc::REQ_STATUS => {
                self.send_sec_fixed(sec_fc::RESP_STATUS, addr).await;
            }
            _ => {
                tracing::warn!(%ctrl, "unhandled peer primary frame");
                self.send_sec_fixed(sec_fc::RESP_LINK_NI, addr).await;
            }
        }
        false
    }

    /// Drive the link when it is free: queued user data first, then link
    /// initialization and class polling, round-robin over the stations.
    async fn tick(&mut self, shared: &Shared, balanced: bool) {
        if self.last_sent.is_some() {
            return; // busy; t1 governs
        }
        let t1 = self.t1_duration;
        if self.try_send_queued(shared, t1).await {
            return;
        }

        for _ in 0..self.order.len() {
            let addr = self.order[self.poll_idx];
            self.poll_idx = (self.poll_idx + 1) % self.order.len();
            match self.secs[&addr].phase {
                Phase::Status => {
                    tracing::debug!(station = addr, "requesting the link status");
                    self.send_prim_confirmed(addr, prim_fc::REQ_STATUS, false, t1)
                        .await;
                    return;
                }
                Phase::Reset => {
                    tracing::debug!(station = addr, "resetting the remote link");
                    self.send_prim_confirmed(addr, prim_fc::RESET_LINK, false, t1)
                        .await;
                    return;
                }
                Phase::Active => {
                    // Balanced mode does not poll; the peer transmits on its own.
                    if balanced {
                        continue;
                    }
                    let fun = if self.secs[&addr].want_class1 {
                        prim_fc::REQ_DATA1
                    } else {
                        prim_fc::REQ_DATA2
                    };
                    self.send_prim_confirmed(addr, fun, true, t1).await;
                    return;
                }
            }
        }
    }

    /// Transmit the first queued ASDU whose station is active.
    async fn try_send_queued(&mut self, shared: &Shared, t1: Duration) -> bool {
        let next = {
            let mut q = shared.queue.lock().unwrap();
            let mut found = None;
            let mut i = 0;
            while i < q.len() {
                let addr = q[i].addr;
                match self.secs.get(&addr) {
                    None => {
                        tracing::error!(addr, "dropping a queued ASDU for an unknown station");
                        q.remove(i);
                    }
                    Some(sec) if sec.phase == Phase::Active && !sec.busy => {
                        found = q.remove(i);
                        break;
                    }
                    // Not active, or holding off under DFC: leave it queued
                    // and look further on.
                    Some(_) => i += 1,
                }
            }
            found
        };
        let Some(out) = next else { return false };

        let raw = match out.asdu.marshal_binary() {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "dropping an ASDU that cannot be encoded");
                return false;
            }
        };

        let fcb = self.secs[&out.addr].fcb;
        let ctrl = ControlField::primary(prim_fc::USER_DATA_CONF, true, fcb, self.dir);
        let frame = Frame::Variable {
            control: ctrl,
            link_addr: out.addr,
            asdu: raw,
        };
        tracing::debug!(station = out.addr, asdu = %out.asdu, "sending user data");
        if !self.write(&frame).await {
            return false;
        }
        self.last_sent = Some(Pending {
            frame,
            ctrl,
            addr: out.addr,
        });
        self.retry_count = 0;
        self.t1_at = Some(Instant::now() + t1);
        true
    }

    /// A confirmed frame went unanswered. Repeat it with the same FCB up to
    /// the configured number of times, then give up on the station.
    async fn on_t1_timeout(&mut self, cfg: &Config) -> Result<()> {
        let Some(p) = self.last_sent.as_ref() else {
            self.t1_at = None;
            return Ok(());
        };

        if self.retry_count < u32::from(cfg.max_repetitions) {
            self.retry_count += 1;
            tracing::warn!(retry = self.retry_count, "t1 expired, repeating the frame");
            let frame = p.frame.clone();
            if !self.write(&frame).await {
                return Err(Error::UseClosedConnection);
            }
            self.t1_at = Some(Instant::now() + cfg.timeout_repeat_t2);
            return Ok(());
        }

        let addr = p.addr;
        tracing::error!(station = addr, "t1 expired after the repetitions");
        self.last_sent = None;
        self.retry_count = 0;
        self.t1_at = None;
        if let Some(sec) = self.secs.get_mut(&addr) {
            sec.phase = Phase::Status;
            sec.want_class1 = false;
            sec.busy = false;
        }
        // Point to point: losing the only peer means losing the connection, so
        // the manager can reopen it. On a multi-drop line keep serving the rest.
        if self.order.len() == 1 {
            return Err(Error::TimeoutT1);
        }
        Ok(())
    }

    /// The line has been idle: send a test link frame to keep it alive.
    async fn on_t3_timeout(&mut self, cfg: &Config) {
        if self.last_sent.is_some() {
            return;
        }
        if let Some(addr) = self.first_active() {
            tracing::debug!(station = addr, "t3 expired, testing the link");
            self.send_prim_confirmed(addr, prim_fc::TEST_LINK, true, cfg.timeout_response_t1)
                .await;
        }
    }

    /// Execute a manual link-layer request from the application API.
    async fn handle_link_request(&mut self, fun: u8, balanced: bool) {
        if self.last_sent.is_some() {
            tracing::debug!(fun, "manual link request dropped: the link is busy");
            return;
        }
        let addr = self.default_addr;
        if !self.secs.contains_key(&addr) {
            return;
        }
        let t1 = self.t1_duration;
        match fun {
            prim_fc::RESET_LINK => {
                if let Some(sec) = self.secs.get_mut(&addr) {
                    sec.phase = Phase::Reset;
                }
                self.send_prim_confirmed(addr, prim_fc::RESET_LINK, false, t1)
                    .await;
            }
            prim_fc::REQ_STATUS => {
                self.send_prim_confirmed(addr, prim_fc::REQ_STATUS, false, t1)
                    .await;
            }
            prim_fc::REQ_DATA2 if self.secs[&addr].phase == Phase::Active && !balanced => {
                self.send_prim_confirmed(addr, prim_fc::REQ_DATA2, true, t1)
                    .await;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::{Cause, CauseOfTransmission, Identifier, TypeId, VariableStruct};

    fn shared() -> Shared {
        Shared {
            params: PARAMS_STANDARD_101,
            max_queue: 16,
            default_addr: 1,
            connected: AtomicBool::new(true),
            link_active: AtomicBool::new(true),
            queue: Mutex::new(VecDeque::new()),
            link_req: mpsc::unbounded_channel().0,
            active_notify: Notify::new(),
        }
    }

    fn asdu() -> Asdu {
        Asdu::new(
            PARAMS_STANDARD_101,
            Identifier::new(
                TypeId::C_IC_NA_1,
                VariableStruct::single(),
                CauseOfTransmission::new(Cause::ACTIVATION),
                1,
            ),
        )
    }

    /// A primary with one active secondary at address 1, plus the receiver its
    /// outbound frames land in.
    fn primary() -> (LinkState, mpsc::Receiver<Vec<u8>>) {
        let option = ClientOption::new();
        let (tx, rx) = mpsc::channel(16);
        let mut link = LinkState::new(&option, tx);
        link.secs.get_mut(&1).unwrap().phase = Phase::Active;
        (link, rx)
    }

    fn secondary_frame(fun: u8, acd: bool, dfc: bool) -> Frame {
        let mut control = ControlField::secondary(fun, acd, false);
        control.dfc = dfc;
        Frame::Fixed { control, link_addr: 1 }
    }

    // DFC is the secondary saying its buffers are full. IEC 60870-5-2 subclass
    // 5.1.3 requires the primary to stop sending user data until it clears —
    // sending anyway is how a station's buffer overruns and drops data.

    #[tokio::test]
    async fn user_data_is_held_while_the_station_signals_dfc() {
        let (mut link, mut rx) = primary();
        let sh = shared();
        sh.queue.lock().unwrap().push_back(Outgoing { asdu: asdu(), addr: 1 });

        link.handle_frame(&secondary_frame(sec_fc::CONF_ACK, false, true), false)
            .await;
        assert!(link.secs[&1].busy, "the station reported DFC");

        let sent = link.try_send_queued(&sh, Duration::from_secs(1)).await;
        assert!(!sent, "no user data may go out while DFC is set");
        assert_eq!(sh.queue.lock().unwrap().len(), 1, "and it stays queued");
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn queued_data_goes_out_once_the_station_clears_dfc() {
        let (mut link, _rx) = primary();
        let sh = shared();
        sh.queue.lock().unwrap().push_back(Outgoing { asdu: asdu(), addr: 1 });

        link.handle_frame(&secondary_frame(sec_fc::CONF_ACK, false, true), false)
            .await;
        assert!(!link.try_send_queued(&sh, Duration::from_secs(1)).await);

        // The next response clears DFC: the station has drained.
        link.last_sent = None;
        link.handle_frame(&secondary_frame(sec_fc::CONF_ACK, false, false), false)
            .await;
        assert!(!link.secs[&1].busy);

        assert!(link.try_send_queued(&sh, Duration::from_secs(1)).await);
        assert!(sh.queue.lock().unwrap().is_empty(), "the ASDU went out");
    }

    #[tokio::test]
    async fn a_busy_station_is_still_polled() {
        // Class polls and link management stay allowed under DFC — they are
        // what lets the station report that it has drained. Only user data is
        // held back.
        let (mut link, mut rx) = primary();
        let sh = shared();
        link.secs.get_mut(&1).unwrap().busy = true;

        link.tick(&sh, false).await;
        assert!(rx.try_recv().is_ok(), "the poll still goes out");
    }

    fn user_data(addr: u16) -> Frame {
        Frame::Variable {
            control: ControlField::secondary(sec_fc::USER_DATA_CONF, false, false),
            link_addr: addr,
            asdu: asdu().marshal_binary().unwrap(),
        }
    }

    /// Put a class 2 request to station 1 on the wire, as the poll would.
    async fn poll(link: &mut LinkState) {
        link.send_prim_confirmed(1, prim_fc::REQ_DATA2, true, Duration::from_secs(1))
            .await;
    }

    // User data from a secondary is only ever the answer to a class 1 or
    // class 2 request. A second copy — a late answer to a request that was
    // then repeated — must not reach the application again.

    #[tokio::test]
    async fn the_answer_to_a_poll_is_delivered_once() {
        let (mut link, _rx) = primary();
        poll(&mut link).await;
        assert!(link.handle_frame(&user_data(1), false).await.deliver);

        // The same response again, with no request outstanding.
        assert!(
            !link.handle_frame(&user_data(1), false).await.deliver,
            "a repeated response was delivered twice"
        );
    }

    #[tokio::test]
    async fn user_data_nobody_asked_for_is_not_delivered() {
        let (mut link, _rx) = primary();
        assert!(!link.handle_frame(&user_data(1), false).await.deliver);
    }

    #[tokio::test]
    async fn user_data_from_an_unknown_station_is_not_delivered() {
        // The client logs that it ignores such a frame; it must not then hand
        // the ASDU to the application anyway.
        let (mut link, _rx) = primary();
        poll(&mut link).await;
        assert!(!link.handle_frame(&user_data(42), false).await.deliver);
        assert!(link.last_sent.is_some(), "the real answer is still awaited");
    }

    #[tokio::test]
    async fn user_data_answering_a_non_request_is_not_delivered() {
        // A station that answers confirmed user data with user data is not
        // following the standard; the frame completes the transaction but its
        // payload is not an answer to anything.
        let (mut link, _rx) = primary();
        link.send_prim_confirmed(1, prim_fc::TEST_LINK, true, Duration::from_secs(1))
            .await;
        assert!(!link.handle_frame(&user_data(1), false).await.deliver);
    }

    fn peer_user_data(fcb: bool, fcv: bool) -> Frame {
        Frame::Variable {
            control: ControlField::primary(prim_fc::USER_DATA_CONF, fcv, fcb, false),
            link_addr: 1,
            asdu: asdu().marshal_binary().unwrap(),
        }
    }

    #[tokio::test]
    async fn a_repeated_balanced_frame_is_acknowledged_but_not_delivered_again() {
        let (mut link, mut rx) = primary();
        // The peer's first FCV frame after a reset carries FCB = 1.
        assert!(link.handle_frame(&peer_user_data(true, true), true).await.deliver);
        assert!(rx.try_recv().is_ok(), "acknowledged");

        // Our acknowledgement was lost; the peer repeats with the same FCB.
        assert!(
            !link.handle_frame(&peer_user_data(true, true), true).await.deliver,
            "the repeat was delivered as new data"
        );
        assert!(rx.try_recv().is_ok(), "the repeat is acknowledged again");

        // The next genuine frame toggles the FCB and is delivered.
        assert!(link.handle_frame(&peer_user_data(false, true), true).await.deliver);
    }

    #[tokio::test]
    async fn a_balanced_frame_whose_fcv_contradicts_its_function_is_ignored() {
        let (mut link, mut rx) = primary();
        let before = link.fcb_expected;
        // Confirmed user data always counts frames; FCV = 0 is malformed.
        assert!(!link.handle_frame(&peer_user_data(true, false), true).await.deliver);
        assert!(rx.try_recv().is_err(), "not answered");
        assert_eq!(link.fcb_expected, before, "the FCB did not move");
    }

    #[tokio::test]
    async fn an_unanswered_frame_is_repeated_the_configured_number_of_times() {
        for repetitions in [0u8, 1, 3] {
            let cfg = Config {
                max_repetitions: repetitions,
                ..Config::default()
            };
            let (mut link, mut rx) = primary();
            poll_station(&mut link).await;
            let first = rx.try_recv().unwrap();

            for n in 0..repetitions {
                link.on_t1_timeout(&cfg).await.unwrap();
                assert_eq!(
                    rx.try_recv().unwrap(),
                    first,
                    "repetition {n} must be the same frame, FCB included"
                );
            }
            // One more timeout gives the only station up, which ends the line.
            assert_eq!(link.on_t1_timeout(&cfg).await, Err(Error::TimeoutT1));
            assert!(rx.try_recv().is_err(), "no repetition beyond {repetitions}");
        }
    }

    async fn poll_station(link: &mut LinkState) {
        link.send_prim_confirmed(1, prim_fc::REQ_DATA2, true, Duration::from_secs(1))
            .await;
    }

    #[tokio::test]
    async fn the_balanced_secondary_role_may_acknowledge_with_the_single_character() {
        let mut option = ClientOption::new();
        option.config.use_single_char_ack = true;
        let (tx, mut rx) = mpsc::channel(16);
        let mut link = LinkState::new(&option, tx);
        let f = Frame::Fixed {
            control: ControlField::primary(prim_fc::RESET_LINK, false, false, false),
            link_addr: 1,
        };
        link.handle_frame(&f, true).await;
        assert_eq!(rx.try_recv().unwrap(), vec![crate::cs101::SINGLE_CHAR_ACK]);
    }
}
