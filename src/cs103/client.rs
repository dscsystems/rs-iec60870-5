// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The IEC 60870-5-103 primary station (master) for protection equipment.
//!
//! The client runs the whole procedure by itself:
//!
//! 1. request status of link, then reset of the communication unit; the device
//!    reports its identification message (ASDU 5), collected by a class 1 poll;
//! 2. with [`Config::auto_init`], a time synchronization (ASDU 6) followed by a
//!    general interrogation (ASDU 7) for every newly active device;
//! 3. continuous class 2 polling for cyclic measurands, switching to class 1
//!    whenever a response sets the ACD bit.
//!
//! FCB tracking, duplicate suppression, t₁/t₂ repetition and multi-drop
//! round-robin work exactly as in [`cs101`](crate::cs101).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, mpsc, watch};

use crate::cs101::{ControlField, Frame, LinkStream, Transporter, prim_fc, read_frame, sec_fc};
use crate::cs103::asdu::Asdu;
use crate::cs103::config::{Config, LINK_ADDR_SIZE};
use crate::cs103::handler::{ClientHandler, Link, dispatch};
use crate::error::{Error, Result};

/// Default interval between attempts to reopen the line.
pub const DEFAULT_RECONNECT_INTERVAL: Duration = Duration::from_secs(60);

/// Primary function code 7, "reset frame count bit".
///
/// This is the lightweight reset IEC 60870-5-103 adds to the FT1.2 codes it
/// shares with IEC 60870-5-101.
pub const PRIM_FC_RESET_FCB: u8 = 7;

/// Where a device is in the link initialization procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Request status of link is due.
    Status,
    /// Reset of the communication unit is due.
    Reset,
    /// The link layer is active.
    Active,
}

/// Per-device link state kept by the primary.
#[derive(Debug)]
struct Device {
    phase: Phase,
    /// FCB of the next FCV frame sent to this device.
    fcb: bool,
    /// A response set ACD: a class 1 request is due.
    want_class1: bool,
}

/// A confirmed frame awaiting an acknowledgement.
struct Pending {
    frame: Frame,
    ctrl: ControlField,
    addr: u8,
}

/// One ASDU queued for a specific device.
struct Outgoing {
    asdu: Asdu,
    addr: u8,
}

/// A manual link-layer request from the application API.
#[derive(Debug, Clone, Copy)]
struct LinkRequest {
    fun: u8,
    addr: u8,
}

/// Configuration for a [`Client`].
#[derive(Debug, Clone)]
pub struct ClientOption {
    config: Config,
    auto_reconnect: bool,
    reconnect_interval: Duration,
    secondary_addrs: Vec<u8>,
}

impl Default for ClientOption {
    fn default() -> Self {
        ClientOption {
            config: Config::default(),
            auto_reconnect: true,
            reconnect_interval: DEFAULT_RECONNECT_INTERVAL,
            secondary_addrs: Vec::new(),
        }
    }
}

impl ClientOption {
    /// The default 103 configuration. The serial port or TCP address must
    /// still be set through [`ClientOption::with_config`].
    pub fn new() -> Self {
        ClientOption::default()
    }

    /// Set the time zone the device's CP32/CP56 time tags are expressed in.
    ///
    /// Applied to every ASDU this client decodes and to the time
    /// synchronization it sends. UTC is the standard's recommendation and the
    /// default.
    pub fn with_time_zone(mut self, zone: crate::asdu::TimeZone) -> Self {
        self.config.time_zone = zone;
        self
    }

    /// The time zone time tags are interpreted in.
    pub fn time_zone(&self) -> crate::asdu::TimeZone {
        self.config.time_zone
    }

    /// Set the configuration. Rejected if invalid.
    pub fn with_config(mut self, mut config: Config) -> Result<Self> {
        config.valid()?;
        self.config = config;
        Ok(self)
    }

    /// Enable or disable automatic reopening of the line. Enabled by default.
    pub fn with_auto_reconnect(mut self, on: bool) -> Self {
        self.auto_reconnect = on;
        self
    }

    /// Set the delay between attempts to reopen the line.
    pub fn with_reconnect_interval(mut self, interval: Duration) -> Self {
        if !interval.is_zero() {
            self.reconnect_interval = interval;
        }
        self
    }

    /// Add a protection device to poll (multi-drop).
    ///
    /// With no address added, `Config::link_address` is the single device.
    /// Duplicates are ignored.
    pub fn with_secondary_address(mut self, addr: u8) -> Self {
        if !self.secondary_addrs.contains(&addr) {
            self.secondary_addrs.push(addr);
        }
        self
    }

    /// The configuration in use.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The device address commands go to by default.
    pub fn default_addr(&self) -> u8 {
        self.secondary_addrs
            .first()
            .copied()
            .unwrap_or(self.config.link_address)
    }

    /// Every device this client serves.
    fn addresses(&self) -> Vec<u8> {
        if self.secondary_addrs.is_empty() {
            vec![self.config.link_address]
        } else {
            self.secondary_addrs.clone()
        }
    }
}

/// State shared between the application and the protocol task.
struct Shared {
    max_queue: usize,
    default_addr: u8,
    connected: AtomicBool,
    link_active: AtomicBool,
    queue: Mutex<VecDeque<Outgoing>>,
    link_req: mpsc::UnboundedSender<LinkRequest>,
    active_notify: Notify,
}

impl Shared {
    /// Queue an ASDU, reporting when the queue is full.
    fn enqueue(&self, asdu: Asdu, addr: u8) -> Result<()> {
        let mut q = self.queue.lock().unwrap();
        if q.len() >= self.max_queue {
            return Err(Error::SendQueueFull);
        }
        q.push_back(Outgoing { asdu, addr });
        Ok(())
    }
}

/// An IEC 60870-5-103 primary station.
pub struct Client<H: ClientHandler> {
    option: ClientOption,
    handler: Arc<H>,
    shared: Arc<Shared>,
    link_req_rx: Mutex<Option<mpsc::UnboundedReceiver<LinkRequest>>>,
    shutdown: watch::Sender<bool>,
    started: AtomicBool,
    transporter: Mutex<Option<Arc<Transporter>>>,
}

impl<H: ClientHandler> Client<H> {
    /// Build a primary station. Call [`Client::start`] to open the line.
    pub fn new(handler: H, option: ClientOption) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        Arc::new(Client {
            shared: Arc::new(Shared {
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
            transporter: Mutex::new(None),
        })
    }

    /// Whether the transport is open.
    pub fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::Acquire)
    }

    /// Wait until at least one device's link becomes active.
    pub async fn wait_link_active(&self) {
        loop {
            let notified = self.shared.active_notify.notified();
            if self.is_link_active() {
                return;
            }
            notified.await;
        }
    }

    /// The address a TCP listener bound to, once it exists.
    pub fn listen_addr(&self) -> Option<std::net::SocketAddr> {
        self.transporter
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|t| t.listen_addr())
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

        let transporter = {
            let mut guard = self.transporter.lock().unwrap();
            if guard.is_none() {
                *guard = Some(Arc::new(Transporter::new(self.option.config.link_config())));
            }
            Arc::clone(guard.as_ref().expect("just set"))
        };

        let this = Arc::clone(self);
        tokio::spawn(async move { this.run_forever(transporter, rx).await });
        Ok(())
    }

    /// Bind the TCP listener without waiting for a peer, so
    /// [`Client::listen_addr`] is observable before the first connection.
    pub async fn bind(self: &Arc<Self>) -> Result<()> {
        let t = {
            let mut guard = self.transporter.lock().unwrap();
            if guard.is_none() {
                *guard = Some(Arc::new(Transporter::new(self.option.config.link_config())));
            }
            Arc::clone(guard.as_ref().expect("just set"))
        };
        t.bind().await
    }

    /// Stop the client and close the line.
    pub fn close(&self) {
        let _ = self.shutdown.send(true);
    }

    async fn run_forever(
        self: Arc<Self>,
        transporter: Arc<Transporter>,
        mut link_req: mpsc::UnboundedReceiver<LinkRequest>,
    ) {
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
            // A new connection starts from an empty queue.
            self.shared.queue.lock().unwrap().clear();

            match err {
                Some(e) => tracing::warn!(error = %e, "link stopped"),
                None => tracing::debug!("link stopped"),
            }
            self.handler.on_connection_lost(self.as_link()).await;

            if *shutdown.borrow() || !self.option.auto_reconnect {
                return;
            }
            tokio::select! {
                _ = shutdown.changed() => return,
                _ = tokio::time::sleep(self.option.reconnect_interval) => {}
            }
        }
    }

    fn as_link(&self) -> &dyn Link {
        self
    }

    /// Run the 103 procedure over one open stream, returning the error that
    /// ended it (if any).
    async fn run_link(
        &self,
        stream: LinkStream,
        link_req: &mut mpsc::UnboundedReceiver<LinkRequest>,
    ) -> Option<Error> {
        let cfg = &self.option.config;

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
                        r = read_frame(&mut reader, LINK_ADDR_SIZE) => r,
                    };
                    match r {
                        Ok(frame) => {
                            if tx_frame.send(frame).await.is_err() {
                                return;
                            }
                        }
                        // Line noise: log it and resynchronise on the next octet.
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
                        self.handler.on_connect(self.as_link()).await;
                    }

                    let activated = link.handle_frame(&frame).await;
                    self.publish_link_active(&link);

                    for addr in activated {
                        if cfg.auto_init {
                            // The standard 103 start-up: set the clock, then
                            // read the whole process image.
                            tracing::debug!(device = addr, "auto-init: time sync + interrogation");
                            let _ = self
                                .shared
                                .enqueue(Asdu::time_sync(addr, Utc::now(), cfg.time_zone), addr);
                            let _ = self
                                .shared
                                .enqueue(Asdu::general_interrogation(addr, 0), addr);
                        }
                        self.handler.on_device_active(self.as_link(), addr).await;
                    }

                    if let Some(raw) = frame.asdu() {
                        match Asdu::unmarshal_binary(raw).map(|a| a.with_time_zone(cfg.time_zone)) {
                            Ok(pack) => dispatch(&*self.handler, self.as_link(), &pack).await,
                            Err(e) => tracing::warn!(error = %e, "discarding undecodable ASDU"),
                        }
                    }
                }

                req = link_req.recv() => {
                    let Some(req) = req else { break };
                    link.handle_link_request(req).await;
                }

                _ = tokio::time::sleep_until(next.into()) => {
                    let now = Instant::now();

                    if link.t1_at.is_some_and(|t| now >= t) {
                        if let Err(e) = link.on_t1_timeout(cfg).await {
                            fatal = Some(e);
                            break;
                        }
                        self.publish_link_active(&link);
                    }

                    if now >= t3_at {
                        t3_at = now + cfg.timeout_test_t3;
                        link.on_t3_timeout().await;
                    }

                    if now >= tick_at {
                        tick_at = now + cfg.timeout_send_link_msg;
                        link.tick(&self.shared).await;
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

    /// Queue an ASDU for the default device.
    pub fn send(&self, a: Asdu) -> Result<()> {
        self.send_to(a, self.shared.default_addr)
    }

    // -- manual link-layer actions ----------------------------------------

    /// Request a reset of the communication unit (FC 0) of a device.
    ///
    /// Executed by the protocol loop once the link is free. The client runs
    /// this procedure automatically, so it is rarely needed.
    pub fn send_reset_cu(&self, addr: u8) -> Result<()> {
        self.queue_link_req(prim_fc::RESET_LINK, addr)
    }

    /// Request a reset of the frame count bit (FC 7) of a device, the
    /// 103-specific lightweight reset.
    pub fn send_reset_fcb(&self, addr: u8) -> Result<()> {
        self.queue_link_req(PRIM_FC_RESET_FCB, addr)
    }

    /// Request the status of a device's link (FC 9).
    pub fn send_request_link_status(&self, addr: u8) -> Result<()> {
        self.queue_link_req(prim_fc::REQ_STATUS, addr)
    }

    /// Poll a device for class 2 data (FC 11).
    pub fn send_req_data_class2(&self, addr: u8) -> Result<()> {
        self.queue_link_req(prim_fc::REQ_DATA2, addr)
    }

    fn queue_link_req(&self, fun: u8, addr: u8) -> Result<()> {
        if !self.is_connected() {
            return Err(Error::UseClosedConnection);
        }
        self.shared
            .link_req
            .send(LinkRequest { fun, addr })
            .map_err(|_| Error::UseClosedConnection)
    }
}

impl<H: ClientHandler> Link for Client<H> {
    fn send_to(&self, a: Asdu, addr: u8) -> Result<()> {
        if !self.is_connected() {
            return Err(Error::UseClosedConnection);
        }
        self.shared.enqueue(a, addr)
    }

    fn is_link_active(&self) -> bool {
        self.is_connected() && self.shared.link_active.load(Ordering::Acquire)
    }

    fn time_zone(&self) -> crate::asdu::TimeZone {
        self.option.time_zone()
    }
}

/// The link state machine, owned by the protocol task.
struct LinkState {
    devices: HashMap<u8, Device>,
    order: Vec<u8>,
    poll_idx: usize,
    last_sent: Option<Pending>,
    retry_count: u32,
    t1_at: Option<Instant>,
    t1_duration: Duration,
    out: mpsc::Sender<Vec<u8>>,
}

impl LinkState {
    fn new(option: &ClientOption, out: mpsc::Sender<Vec<u8>>) -> LinkState {
        let order = option.addresses();
        let devices = order
            .iter()
            .map(|a| {
                (
                    *a,
                    Device {
                        phase: Phase::Status,
                        fcb: false,
                        want_class1: false,
                    },
                )
            })
            .collect();
        LinkState {
            devices,
            order,
            poll_idx: 0,
            last_sent: None,
            retry_count: 0,
            t1_at: None,
            t1_duration: option.config.timeout_response_t1,
            out,
        }
    }

    fn any_active(&self) -> bool {
        self.order
            .iter()
            .any(|a| self.devices[a].phase == Phase::Active)
    }

    fn first_active(&self) -> Option<u8> {
        self.order
            .iter()
            .copied()
            .find(|a| self.devices[a].phase == Phase::Active)
    }

    async fn write(&self, frame: &Frame) -> bool {
        match frame.marshal(LINK_ADDR_SIZE) {
            Ok(raw) => self.out.send(raw).await.is_ok(),
            Err(e) => {
                tracing::error!(error = %e, "failed to encode a frame");
                false
            }
        }
    }

    /// Send a confirmed primary frame and arm the t₁ response timer.
    async fn send_prim_confirmed(&mut self, addr: u8, fun: u8, fcv: bool) -> bool {
        let fcb = self.devices[&addr].fcb;
        // IEC 60870-5-103 is unbalanced only, so DIR is never set.
        let ctrl = ControlField::primary(fun, fcv, fcb, false);
        let frame = Frame::Fixed {
            control: ctrl,
            link_addr: addr as u16,
        };
        if !self.write(&frame).await {
            return false;
        }
        self.last_sent = Some(Pending { frame, ctrl, addr });
        self.retry_count = 0;
        self.t1_at = Some(Instant::now() + self.t1_duration);
        true
    }

    /// Complete the outstanding transaction positively, returning the device
    /// whose link just became active.
    fn confirm_positive(&mut self, addr: u8, addr_valid: bool) -> Option<u8> {
        let Some(p) = self.last_sent.take() else {
            tracing::warn!("received a confirmation with no frame outstanding");
            return None;
        };
        if addr_valid && p.addr != addr {
            tracing::warn!(
                got = addr,
                expected = p.addr,
                "confirmation from the wrong device, ignored"
            );
            self.last_sent = Some(p);
            return None;
        }

        let mut became_active = None;
        if let Some(dev) = self.devices.get_mut(&p.addr) {
            if p.ctrl.fcv {
                // The FCB only toggles once the frame is positively confirmed.
                dev.fcb = !p.ctrl.fcb;
            }
            match p.ctrl.fun {
                prim_fc::RESET_LINK | PRIM_FC_RESET_FCB => {
                    tracing::debug!(device = p.addr, "reset confirmed, link active");
                    dev.phase = Phase::Active;
                    // The first FCV frame after a reset carries FCB = 1.
                    dev.fcb = true;
                    became_active = Some(p.addr);
                }
                prim_fc::REQ_STATUS => {
                    if dev.phase == Phase::Status {
                        dev.phase = Phase::Reset;
                    }
                }
                prim_fc::REQ_DATA1 => dev.want_class1 = false,
                _ => {}
            }
        }
        self.retry_count = 0;
        self.t1_at = None;
        became_active
    }

    /// Abort the outstanding transaction without toggling the FCB, sending the
    /// device back to the start of the initialization procedure.
    fn fail_transaction(&mut self, addr: u8) {
        let Some(p) = self.last_sent.take() else { return };
        if p.addr != addr {
            self.last_sent = Some(p);
            return;
        }
        if let Some(dev) = self.devices.get_mut(&p.addr) {
            dev.phase = Phase::Status;
            dev.want_class1 = false;
        }
        self.retry_count = 0;
        self.t1_at = None;
    }

    /// Process one received frame; returns the devices that became active.
    async fn handle_frame(&mut self, frame: &Frame) -> Vec<u8> {
        let mut active = Vec::new();

        // A single-character acknowledgement carries neither address nor
        // control field.
        if matches!(frame, Frame::SingleCharAck) {
            active.extend(self.confirm_positive(0, false));
            return active;
        }

        let addr = frame.link_addr().unwrap_or(0) as u8;
        if !self.devices.contains_key(&addr) {
            tracing::warn!(addr, "ignoring a frame from an unexpected link address");
            return active;
        }
        let ctrl = frame.control().expect("not a single character ack");

        if ctrl.prm {
            // 103 defines no balanced procedure, so a device never sends PRM=1.
            tracing::warn!(%ctrl, "unexpected PRM=1 frame; 103 is unbalanced only");
            return active;
        }
        if ctrl.dfc {
            tracing::warn!(addr, "the device signals data flow control (buffers full)");
        }

        match frame {
            Frame::Fixed { .. } => match ctrl.fun {
                sec_fc::CONF_ACK => active.extend(self.confirm_positive(addr, true)),
                // Link busy: keep the frame outstanding, t1 will repeat it.
                sec_fc::CONF_NACK => tracing::warn!(addr, "device reports the link busy"),
                sec_fc::USER_DATA_NO_REP => {
                    tracing::debug!(addr, "requested data not available");
                    active.extend(self.confirm_positive(addr, true));
                }
                sec_fc::RESP_STATUS => {
                    tracing::debug!(addr, dfc = ctrl.dfc, "received link status");
                    active.extend(self.confirm_positive(addr, true));
                }
                sec_fc::RESP_LINK_NF | sec_fc::RESP_LINK_NI => {
                    tracing::warn!(addr, fc = ctrl.fun, "link service not available");
                    self.fail_transaction(addr);
                }
                _ => tracing::warn!(addr, %ctrl, "unhandled fixed-length frame"),
            },

            Frame::Variable { .. } => match ctrl.fun {
                sec_fc::USER_DATA_CONF | sec_fc::RESP_STATUS => {
                    tracing::debug!(addr, "received user data");
                    active.extend(self.confirm_positive(addr, true));
                }
                sec_fc::USER_DATA_NO_REP => {
                    tracing::debug!(addr, "no user data available");
                    active.extend(self.confirm_positive(addr, true));
                }
                _ => tracing::warn!(addr, %ctrl, "unhandled variable-length frame"),
            },

            Frame::SingleCharAck => unreachable!("handled above"),
        }

        // Access demand: the device has events waiting. Fetch them now if the
        // link just went idle, otherwise the next poll will.
        if ctrl.acd {
            let due = {
                let dev = self.devices.get_mut(&addr).expect("checked");
                dev.want_class1 = true;
                dev.phase == Phase::Active
            };
            if due && self.last_sent.is_none() {
                self.send_prim_confirmed(addr, prim_fc::REQ_DATA1, true)
                    .await;
            }
        }
        active
    }

    /// Drive the link when it is free: queued user data first, then link
    /// initialization and class polling, round-robin over the devices.
    async fn tick(&mut self, shared: &Shared) {
        if self.last_sent.is_some() {
            return; // busy; t1 governs
        }
        if self.try_send_queued(shared).await {
            return;
        }

        // Service exactly one device per tick and advance the cursor, so a
        // multi-drop line is served round-robin across successive ticks.
        let addr = self.order[self.poll_idx];
        self.poll_idx = (self.poll_idx + 1) % self.order.len();

        let (fun, fcv) = match self.devices[&addr].phase {
            Phase::Status => {
                tracing::debug!(device = addr, "requesting the link status");
                (prim_fc::REQ_STATUS, false)
            }
            Phase::Reset => {
                tracing::debug!(device = addr, "resetting the communication unit");
                (prim_fc::RESET_LINK, false)
            }
            Phase::Active if self.devices[&addr].want_class1 => (prim_fc::REQ_DATA1, true),
            Phase::Active => (prim_fc::REQ_DATA2, true),
        };
        self.send_prim_confirmed(addr, fun, fcv).await;
    }

    /// Transmit the first queued ASDU whose device is active.
    async fn try_send_queued(&mut self, shared: &Shared) -> bool {
        let next = {
            let mut q = shared.queue.lock().unwrap();
            let mut found = None;
            let mut i = 0;
            while i < q.len() {
                let addr = q[i].addr;
                match self.devices.get(&addr) {
                    None => {
                        tracing::error!(addr, "dropping a queued ASDU for an unknown device");
                        q.remove(i);
                    }
                    Some(dev) if dev.phase == Phase::Active => {
                        found = q.remove(i);
                        break;
                    }
                    // Not active yet: leave it queued and look further on.
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

        let fcb = self.devices[&out.addr].fcb;
        let ctrl = ControlField::primary(prim_fc::USER_DATA_CONF, true, fcb, false);
        let frame = Frame::Variable {
            control: ctrl,
            link_addr: out.addr as u16,
            asdu: raw,
        };
        tracing::debug!(device = out.addr, asdu = %out.asdu, "sending user data");
        if !self.write(&frame).await {
            return false;
        }
        self.last_sent = Some(Pending {
            frame,
            ctrl,
            addr: out.addr,
        });
        self.retry_count = 0;
        self.t1_at = Some(Instant::now() + self.t1_duration);
        true
    }

    /// A confirmed frame went unanswered. Repeat it once with the same FCB,
    /// then give up on the device.
    async fn on_t1_timeout(&mut self, cfg: &Config) -> Result<()> {
        let Some(p) = self.last_sent.as_ref() else {
            self.t1_at = None;
            return Ok(());
        };

        if self.retry_count < 1 {
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
        tracing::error!(device = addr, "t1 expired after the repetition");
        self.last_sent = None;
        self.retry_count = 0;
        self.t1_at = None;
        if let Some(dev) = self.devices.get_mut(&addr) {
            dev.phase = Phase::Status;
            dev.want_class1 = false;
        }
        // Point to point: losing the only device means losing the connection,
        // so the manager can reopen it. On a multi-drop line keep serving the
        // remaining devices.
        if self.order.len() == 1 {
            return Err(Error::TimeoutT1);
        }
        Ok(())
    }

    /// The line has been idle: poll an active device for class 2 data, which
    /// is how 103 keeps the link alive.
    async fn on_t3_timeout(&mut self) {
        if self.last_sent.is_some() {
            return;
        }
        if let Some(addr) = self.first_active() {
            tracing::debug!(device = addr, "t3 expired, keep-alive class 2 poll");
            self.send_prim_confirmed(addr, prim_fc::REQ_DATA2, true)
                .await;
        }
    }

    /// Execute a manual link-layer request from the application API.
    async fn handle_link_request(&mut self, req: LinkRequest) {
        if self.last_sent.is_some() {
            tracing::debug!(fun = req.fun, "manual link request dropped: the link is busy");
            return;
        }
        if !self.devices.contains_key(&req.addr) {
            return;
        }
        match req.fun {
            prim_fc::RESET_LINK | PRIM_FC_RESET_FCB => {
                if let Some(dev) = self.devices.get_mut(&req.addr) {
                    dev.phase = Phase::Reset;
                }
                self.send_prim_confirmed(req.addr, req.fun, false).await;
            }
            prim_fc::REQ_STATUS => {
                self.send_prim_confirmed(req.addr, prim_fc::REQ_STATUS, false)
                    .await;
            }
            prim_fc::REQ_DATA2 if self.devices[&req.addr].phase == Phase::Active => {
                self.send_prim_confirmed(req.addr, prim_fc::REQ_DATA2, true)
                    .await;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cs101::{SerialConfig, TcpConfig, TransportType};

    struct Noop;
    impl ClientHandler for Noop {}

    fn tcp_option() -> ClientOption {
        ClientOption::new()
            .with_config(Config {
                transport: TransportType::TcpClient,
                tcp: TcpConfig {
                    address: "127.0.0.1:1".into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap()
    }

    #[test]
    fn an_invalid_configuration_is_rejected_up_front() {
        // A serial transport with no port name cannot be opened.
        assert!(
            ClientOption::new()
                .with_config(Config::default())
                .is_err()
        );
        assert!(
            ClientOption::new()
                .with_config(Config {
                    serial: SerialConfig::new("/dev/ttyUSB0", 9600),
                    ..Default::default()
                })
                .is_ok()
        );
    }

    #[test]
    fn the_default_address_follows_the_configured_devices() {
        let o = tcp_option();
        assert_eq!(o.default_addr(), 1, "falls back to Config::link_address");
        assert_eq!(o.addresses(), vec![1]);

        let o = o.with_secondary_address(7).with_secondary_address(9);
        assert_eq!(o.default_addr(), 7, "the first added device wins");
        assert_eq!(o.addresses(), vec![7, 9]);

        // Duplicates are ignored.
        let o = o.with_secondary_address(7);
        assert_eq!(o.addresses(), vec![7, 9]);
    }

    #[tokio::test]
    async fn sending_before_the_line_is_open_is_refused() {
        let cli = Client::new(Noop, tcp_option());
        assert!(!cli.is_connected());
        assert!(!cli.is_link_active());
        assert_eq!(
            cli.send(Asdu::general_interrogation(1, 0)),
            Err(Error::UseClosedConnection)
        );
        assert_eq!(cli.send_reset_cu(1), Err(Error::UseClosedConnection));
    }

    #[tokio::test]
    async fn a_full_queue_reports_send_queue_full() {
        let shared = Shared {
            max_queue: 2,
            default_addr: 1,
            connected: AtomicBool::new(true),
            link_active: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
            link_req: mpsc::unbounded_channel().0,
            active_notify: Notify::new(),
        };
        assert!(shared.enqueue(Asdu::general_interrogation(1, 0), 1).is_ok());
        assert!(shared.enqueue(Asdu::general_interrogation(1, 1), 1).is_ok());
        assert_eq!(
            shared.enqueue(Asdu::general_interrogation(1, 2), 1),
            Err(Error::SendQueueFull)
        );
    }

    /// Drive the state machine directly, with the frames a device would send.
    async fn state() -> (LinkState, mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = mpsc::channel(32);
        (LinkState::new(&tcp_option(), tx), rx)
    }

    fn ack(addr: u8, acd: bool) -> Frame {
        Frame::Fixed {
            control: ControlField::secondary(sec_fc::CONF_ACK, acd, false),
            link_addr: addr as u16,
        }
    }

    /// Decode the control field of the next frame the state machine wrote.
    fn sent_fun(raw: &[u8]) -> ControlField {
        ControlField::parse(raw[1])
    }

    #[tokio::test]
    async fn the_link_procedure_runs_status_then_reset_then_polls() {
        let (mut link, mut out) = state().await;
        let shared = Shared {
            max_queue: 10,
            default_addr: 1,
            connected: AtomicBool::new(true),
            link_active: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
            link_req: mpsc::unbounded_channel().0,
            active_notify: Notify::new(),
        };

        // First tick: request status of link.
        link.tick(&shared).await;
        let f = out.recv().await.unwrap();
        assert_eq!(sent_fun(&f).fun, prim_fc::REQ_STATUS);
        assert!(!sent_fun(&f).fcv, "a status request carries FCV=0");

        // The device answers with its link status.
        let status = Frame::Fixed {
            control: ControlField::secondary(sec_fc::RESP_STATUS, false, false),
            link_addr: 1,
        };
        assert!(link.handle_frame(&status).await.is_empty());
        assert!(!link.any_active(), "still waiting for the reset");

        // Second tick: reset of the communication unit.
        link.tick(&shared).await;
        let f = out.recv().await.unwrap();
        assert_eq!(sent_fun(&f).fun, prim_fc::RESET_LINK);

        // The device acknowledges; the link becomes active.
        assert_eq!(link.handle_frame(&ack(1, false)).await, vec![1]);
        assert!(link.any_active());

        // Third tick: class 2 poll, now with FCV=1 and FCB=1.
        link.tick(&shared).await;
        let f = out.recv().await.unwrap();
        let ctrl = sent_fun(&f);
        assert_eq!(ctrl.fun, prim_fc::REQ_DATA2);
        assert!(ctrl.fcv);
        assert!(ctrl.fcb, "the first FCV frame after a reset carries FCB=1");
    }

    #[tokio::test]
    async fn the_frame_count_bit_alternates_across_every_fcv_frame() {
        let (mut link, mut out) = state().await;
        let shared = Shared {
            max_queue: 10,
            default_addr: 1,
            connected: AtomicBool::new(true),
            link_active: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
            link_req: mpsc::unbounded_channel().0,
            active_notify: Notify::new(),
        };

        // Bring the device to the active phase.
        link.tick(&shared).await;
        out.recv().await.unwrap();
        link.handle_frame(&Frame::Fixed {
            control: ControlField::secondary(sec_fc::RESP_STATUS, false, false),
            link_addr: 1,
        })
        .await;
        link.tick(&shared).await;
        out.recv().await.unwrap();
        link.handle_frame(&ack(1, false)).await;

        // Every acknowledged FCV frame flips the bit.
        for expected in [true, false, true, false] {
            link.tick(&shared).await;
            let f = out.recv().await.unwrap();
            assert_eq!(sent_fun(&f).fcb, expected);
            link.handle_frame(&ack(1, false)).await;
        }
    }

    #[tokio::test]
    async fn an_access_demand_triggers_a_class_one_request() {
        let (mut link, mut out) = state().await;
        let shared = Shared {
            max_queue: 10,
            default_addr: 1,
            connected: AtomicBool::new(true),
            link_active: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
            link_req: mpsc::unbounded_channel().0,
            active_notify: Notify::new(),
        };

        link.tick(&shared).await;
        out.recv().await.unwrap();
        link.handle_frame(&Frame::Fixed {
            control: ControlField::secondary(sec_fc::RESP_STATUS, false, false),
            link_addr: 1,
        })
        .await;
        link.tick(&shared).await;
        out.recv().await.unwrap();

        // The reset acknowledgement also sets ACD: events are waiting, so the
        // class 1 request goes out immediately rather than at the next tick.
        link.handle_frame(&ack(1, true)).await;
        let f = out.recv().await.unwrap();
        assert_eq!(sent_fun(&f).fun, prim_fc::REQ_DATA1);
    }

    #[tokio::test]
    async fn a_repeated_frame_keeps_its_frame_count_bit() {
        let (mut link, mut out) = state().await;
        let cfg = Config {
            timeout_response_t1: Duration::from_millis(20),
            timeout_repeat_t2: Duration::from_millis(10),
            ..Default::default()
        };
        let shared = Shared {
            max_queue: 10,
            default_addr: 1,
            connected: AtomicBool::new(true),
            link_active: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::new()),
            link_req: mpsc::unbounded_channel().0,
            active_notify: Notify::new(),
        };

        link.tick(&shared).await;
        let first = out.recv().await.unwrap();

        // Nothing answered: the frame is repeated byte for byte.
        link.on_t1_timeout(&cfg).await.unwrap();
        let repeat = out.recv().await.unwrap();
        assert_eq!(first, repeat, "a repetition keeps the same FCB");

        // The second timeout gives up; with a single device that ends the link.
        assert_eq!(link.on_t1_timeout(&cfg).await, Err(Error::TimeoutT1));
        assert!(!link.any_active());
    }
}
