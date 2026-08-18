// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The IEC 60870-5-101 secondary station (outstation / slave).
//!
//! The server answers the link procedure by itself — status, reset, test, and
//! FCB duplicate detection with retransmission of the previous response — and
//! dispatches received ASDUs to a [`ServerHandler`].
//!
//! In **unbalanced** mode [`Connect::send`] buffers: causes `Periodic` and
//! `Background` go to the class 2 buffer, everything else to class 1, and the
//! primary collects them when it polls. Pending class 1 data is announced with
//! the ACD bit on every response, which makes a standard master issue a class 1
//! request. The standard forbids unsolicited transmission here.
//!
//! In **balanced** mode the station also runs a primary role and transmits
//! queued ASDUs as confirmed user data on its own.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, mpsc, watch};

use crate::asdu::{Asdu, Cause, Connect, PARAMS_STANDARD_101, Params};
use crate::cs101::config::Config;
use crate::cs101::frame::{ControlField, Frame, prim_fc, read_frame, sec_fc};
use crate::cs101::handler::{ServerHandler, dispatch_server};
use crate::cs101::transport::{LinkStream, Transporter};
use crate::error::{Error, Result};

/// Buffers and flags shared between the application and the protocol task.
struct Shared {
    params: Params,
    max_queue: usize,
    balanced: bool,
    connected: AtomicBool,
    link_active: AtomicBool,
    /// Events and everything not cyclic; announced with the ACD bit.
    class1: Mutex<VecDeque<Asdu>>,
    /// Cyclic and background data.
    class2: Mutex<VecDeque<Asdu>>,
    /// Balanced mode: ASDUs to transmit from this station's primary role.
    prim_queue: Mutex<VecDeque<Asdu>>,
    active_notify: Notify,
}

impl Shared {
    fn class1_pending(&self) -> bool {
        !self.class1.lock().unwrap().is_empty()
    }

    /// Take the next buffered ASDU for a class 1 or class 2 request, falling
    /// back to the other class when the requested one is empty — permitted by
    /// IEC 60870-5-101, subclass 6.6.
    fn pop_class(&self, class: u8) -> Option<Asdu> {
        let (first, second) = if class == 2 {
            (&self.class2, &self.class1)
        } else {
            (&self.class1, &self.class2)
        };
        if let Some(a) = first.lock().unwrap().pop_front() {
            return Some(a);
        }
        second.lock().unwrap().pop_front()
    }
}

/// An IEC 60870-5-101 secondary station.
///
/// Implements [`Connect`], so every [`ConnectExt`](crate::asdu::ConnectExt)
/// helper works against it.
pub struct Server<H: ServerHandler> {
    config: Config,
    handler: Arc<H>,
    shared: Arc<Shared>,
    shutdown: watch::Sender<bool>,
    started: AtomicBool,
    server_number: usize,
    reconnect_interval: Duration,
    transporter: Mutex<Option<Arc<Transporter>>>,
}

impl<H: ServerHandler> Server<H> {
    /// Build a secondary station with the default configuration and
    /// [`PARAMS_STANDARD_101`] parameters.
    pub fn new(handler: H) -> Arc<Self> {
        let config = Config::default();
        Arc::new(Server {
            shared: Arc::new(Shared {
                params: PARAMS_STANDARD_101,
                max_queue: config.max_send_queue_size,
                balanced: config.is_balanced(),
                connected: AtomicBool::new(false),
                link_active: AtomicBool::new(false),
                class1: Mutex::new(VecDeque::new()),
                class2: Mutex::new(VecDeque::new()),
                prim_queue: Mutex::new(VecDeque::new()),
                active_notify: Notify::new(),
            }),
            config,
            handler: Arc::new(handler),
            shutdown: watch::channel(false).0,
            started: AtomicBool::new(false),
            server_number: 0,
            reconnect_interval: Duration::from_secs(60),
            transporter: Mutex::new(None),
        })
    }

    /// Set the link configuration. Rejected if invalid. Call before `start`.
    pub fn with_config(mut self: Arc<Self>, mut config: Config) -> Result<Arc<Self>> {
        config.valid()?;
        let s = Arc::get_mut(&mut self).ok_or(Error::Config("configure before starting"))?;
        let shared = Arc::get_mut(&mut s.shared).ok_or(Error::Config("configure before starting"))?;
        shared.max_queue = config.max_send_queue_size;
        shared.balanced = config.is_balanced();
        s.config = config;
        Ok(self)
    }

    /// Set the ASDU parameters; invalid values fall back to
    /// [`PARAMS_STANDARD_101`]. Call before `start`.
    pub fn with_params(mut self: Arc<Self>, params: Params) -> Arc<Self> {
        {
            let s = Arc::get_mut(&mut self).expect("configure before starting");
            let shared = Arc::get_mut(&mut s.shared).expect("configure before starting");
            shared.params = if params.valid().is_ok() {
                params
            } else {
                PARAMS_STANDARD_101
            };
        }
        self
    }

    /// Set the retry interval used by the TCP transports after a failed open.
    pub fn with_reconnect_interval(mut self: Arc<Self>, interval: Duration) -> Arc<Self> {
        if !interval.is_zero() {
            let s = Arc::get_mut(&mut self).expect("configure before starting");
            s.reconnect_interval = interval;
        }
        self
    }

    /// Set the index reported to `ServerHandler::asdu_all`.
    pub fn with_server_number(mut self: Arc<Self>, n: usize) -> Arc<Self> {
        let s = Arc::get_mut(&mut self).expect("configure before starting");
        s.server_number = n;
        self
    }

    /// The ASDU parameters in use.
    pub fn params(&self) -> Params {
        self.shared.params
    }

    /// Whether the transport is open.
    pub fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::Acquire)
    }

    /// Whether the link layer has been activated.
    ///
    /// In unbalanced mode this means the primary reset the link; in balanced
    /// mode it means the transport is open.
    pub fn is_link_active(&self) -> bool {
        if self.shared.balanced {
            self.is_connected()
        } else {
            self.is_connected() && self.shared.link_active.load(Ordering::Acquire)
        }
    }

    /// Wait until the link layer becomes active.
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
    ///
    /// Useful with [`TransportType::TcpServer`](crate::cs101::TransportType)
    /// and port 0.
    pub fn listen_addr(&self) -> Option<std::net::SocketAddr> {
        self.transporter
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|t| t.listen_addr())
    }

    /// How many ASDUs are waiting in the class 1 and class 2 buffers.
    pub fn buffered(&self) -> (usize, usize) {
        (
            self.shared.class1.lock().unwrap().len(),
            self.shared.class2.lock().unwrap().len(),
        )
    }

    /// Open the line in the background; returns immediately.
    pub fn start(self: &Arc<Self>) -> Result<()> {
        let mut cfg = self.config.clone();
        cfg.valid()?;
        if self.started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        // Reuse the transporter `bind` may already have created, so a listener
        // bound to an ephemeral port keeps the port the caller observed.
        let transporter = {
            let mut guard = self.transporter.lock().unwrap();
            if guard.is_none() {
                *guard = Some(Arc::new(Transporter::new(cfg)));
            }
            Arc::clone(guard.as_ref().expect("just set"))
        };

        let this = Arc::clone(self);
        tokio::spawn(async move { this.run_forever(transporter).await });
        Ok(())
    }

    /// Bind the TCP listener without waiting for a peer, so
    /// [`Server::listen_addr`] is observable before the first connection.
    pub async fn bind(self: &Arc<Self>) -> Result<()> {
        let mut cfg = self.config.clone();
        cfg.valid()?;
        let t = {
            let mut guard = self.transporter.lock().unwrap();
            if guard.is_none() {
                *guard = Some(Arc::new(Transporter::new(cfg)));
            }
            Arc::clone(guard.as_ref().expect("just set"))
        };
        t.bind().await
    }

    /// Stop the server and close the line.
    pub fn close(&self) {
        let _ = self.shutdown.send(true);
    }

    async fn run_forever(self: Arc<Self>, transporter: Arc<Transporter>) {
        let mut shutdown = self.shutdown.subscribe();
        let serial = self.config.transport == crate::cs101::TransportType::Serial;

        loop {
            if *shutdown.borrow() {
                return;
            }

            let opened = tokio::select! {
                _ = shutdown.changed() => return,
                r = transporter.open() => r,
            };

            let (stream, desc) = match opened {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(error = %e, "failed to open the line");
                    // A serial port is opened once; a TCP transport retries.
                    if serial {
                        return;
                    }
                    tokio::select! {
                        _ = shutdown.changed() => return,
                        _ = tokio::time::sleep(self.reconnect_interval) => continue,
                    }
                }
            };
            tracing::debug!(%desc, "line open");

            self.shared.connected.store(true, Ordering::Release);
            self.run_link(stream).await;
            self.shared.connected.store(false, Ordering::Release);
            self.shared.link_active.store(false, Ordering::Release);
            self.handler.on_connection_lost(self.as_connect()).await;
            tracing::debug!("line closed");

            if serial || *shutdown.borrow() {
                return;
            }
        }
    }

    fn as_connect(&self) -> &dyn Connect {
        self
    }

    async fn run_link(&self, stream: LinkStream) {
        let cfg = &self.config;
        let addr_size = cfg.link_addr_size;

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

        let mut link = SecondaryState::new(cfg, tx_out.clone());
        let mut shutdown = self.shutdown.subscribe();
        let mut tick_at = Instant::now() + cfg.timeout_send_link_msg;

        loop {
            tokio::select! {
                _ = shutdown.changed() => break,

                frame = rx_frame.recv() => {
                    let Some(frame) = frame else { break };
                    tracing::debug!(?frame, "RX frame");
                    let outcome = link.handle_frame(&frame, &self.shared, cfg).await;

                    if outcome.link_became_active {
                        self.shared.link_active.store(true, Ordering::Release);
                        self.shared.active_notify.notify_waiters();
                        self.handler.on_link_active(self.as_connect()).await;
                    }
                    if let Some(raw) = outcome.asdu {
                        match Asdu::unmarshal_binary(self.shared.params, &raw) {
                            Ok(pack) => {
                                dispatch_server(
                                    &*self.handler,
                                    self.as_connect(),
                                    &pack,
                                    self.server_number,
                                )
                                .await
                            }
                            Err(e) => tracing::warn!(error = %e, "discarding undecodable ASDU"),
                        }
                    }
                }

                _ = tokio::time::sleep_until(tick_at.into()) => {
                    tick_at = Instant::now() + cfg.timeout_send_link_msg;
                    if self.shared.balanced {
                        link.prim_tick(&self.shared, cfg).await;
                    }
                }
            }
        }

        let _ = stop_tx.send(true);
        drop(tx_out);
        let _ = tokio::join!(reader_task, writer_task);
    }
}

#[async_trait::async_trait]
impl<H: ServerHandler> Connect for Server<H> {
    fn params(&self) -> Params {
        self.shared.params
    }

    /// Buffer an ASDU for delivery to the primary station.
    ///
    /// Unbalanced: `Periodic` and `Background` go to class 2, everything else
    /// to class 1. Balanced: queued for spontaneous transmission.
    async fn send(&self, a: Asdu) -> Result<()> {
        if !self.is_connected() {
            return Err(Error::UseClosedConnection);
        }
        let queue = if self.shared.balanced {
            &self.shared.prim_queue
        } else if a.coa().cause == Cause::PERIODIC || a.coa().cause == Cause::BACKGROUND {
            &self.shared.class2
        } else {
            &self.shared.class1
        };
        let mut q = queue.lock().unwrap();
        if q.len() >= self.shared.max_queue {
            return Err(Error::SendQueueFull);
        }
        q.push_back(a);
        Ok(())
    }
}

/// What handling one received frame produced.
#[derive(Default)]
struct Outcome {
    /// The link layer was just reset and is now active.
    link_became_active: bool,
    /// A user data ASDU to dispatch to the handler.
    asdu: Option<Vec<u8>>,
}

/// The secondary-role link state machine, plus the balanced-mode primary role.
struct SecondaryState {
    link_addr: u16,
    addr_size: u8,
    /// The FCB expected on the primary's next FCV frame.
    fcb_expected: bool,
    /// The last response sent, retransmitted when a request is repeated.
    last_resp: Option<Frame>,
    out: mpsc::Sender<Vec<u8>>,

    // -- balanced mode primary role --
    prim_out: Option<Frame>,
    prim_out_ctrl: ControlField,
    prim_fcb: bool,
    prim_active: bool,
    prim_retry: u32,
    prim_deadline: Option<Instant>,
    prim_next_init: Instant,
}

impl SecondaryState {
    fn new(cfg: &Config, out: mpsc::Sender<Vec<u8>>) -> SecondaryState {
        SecondaryState {
            link_addr: cfg.link_address,
            addr_size: cfg.link_addr_size,
            fcb_expected: true,
            last_resp: None,
            out,
            prim_out: None,
            prim_out_ctrl: ControlField::default(),
            prim_fcb: false,
            prim_active: false,
            prim_retry: 0,
            prim_deadline: None,
            prim_next_init: Instant::now(),
        }
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

    /// Send a response and remember it, so a repeated request (FCB mismatch)
    /// is answered with exactly the same frame.
    async fn send_resp(&mut self, frame: Frame) -> bool {
        self.last_resp = Some(frame.clone());
        self.write(&frame).await
    }

    fn sec_frame(&self, fun: u8, acd: bool) -> Frame {
        Frame::Fixed {
            // The secondary never sets DIR; station B keeps it clear.
            control: ControlField::secondary(fun, acd, false),
            link_addr: self.link_addr,
        }
    }

    async fn send_link_ack(&mut self, shared: &Shared) -> bool {
        let f = self.sec_frame(sec_fc::CONF_ACK, shared.class1_pending());
        self.send_resp(f).await
    }

    async fn send_link_status(&mut self, shared: &Shared) -> bool {
        let f = self.sec_frame(sec_fc::RESP_STATUS, shared.class1_pending());
        self.send_resp(f).await
    }

    /// Answer a class 1 or class 2 poll with buffered data, or with
    /// "requested data not available" when there is none.
    async fn respond_class_data(&mut self, class: u8, shared: &Shared) -> bool {
        let Some(a) = shared.pop_class(class) else {
            tracing::debug!(class, "no data buffered, answering FC 9");
            let f = self.sec_frame(sec_fc::USER_DATA_NO_REP, false);
            return self.send_resp(f).await;
        };

        let raw = match a.marshal_binary() {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "dropping a buffered ASDU that cannot be encoded");
                let f = self.sec_frame(sec_fc::USER_DATA_NO_REP, shared.class1_pending());
                return self.send_resp(f).await;
            }
        };
        tracing::debug!(class, asdu = %a, "answering the poll with user data");
        let f = Frame::Variable {
            control: ControlField::secondary(
                sec_fc::USER_DATA_CONF,
                shared.class1_pending(),
                false,
            ),
            link_addr: self.link_addr,
            asdu: raw,
        };
        self.send_resp(f).await
    }

    /// True when `addr` addresses this station, including the broadcast address.
    fn addressed_to_us(&self, addr: u16) -> bool {
        let broadcast = match self.addr_size {
            1 => 0x00ff,
            2 => 0xffff,
            _ => return true,
        };
        addr == self.link_addr || addr == broadcast
    }

    async fn handle_frame(&mut self, frame: &Frame, shared: &Shared, cfg: &Config) -> Outcome {
        let mut out = Outcome::default();

        // A single-character acknowledgement only makes sense as a confirmation
        // of this station's primary role in balanced mode.
        if matches!(frame, Frame::SingleCharAck) {
            if cfg.is_balanced() {
                self.prim_confirm(true);
            } else {
                tracing::warn!("single character ack received by an unbalanced secondary; ignored");
            }
            return out;
        }

        let addr = frame.link_addr().unwrap_or(0);
        if self.addr_size > 0 && !self.addressed_to_us(addr) {
            // Not for us: another station on the multi-drop line will answer.
            return out;
        }
        let ctrl = frame.control().expect("not a single character ack");

        if !ctrl.prm {
            if cfg.is_balanced() {
                self.handle_peer_secondary_frame(ctrl, cfg);
            } else {
                tracing::warn!("ignoring a frame with PRM=0");
            }
            return out;
        }

        // Duplicate detection: the FCB alternates on every FCV frame,
        // whatever the function code. A mismatch means the primary repeated
        // itself, so repeat the previous response without acting on it again.
        // See IEC 60870-5-2, subclass 5.1.2.
        if ctrl.fcv {
            if ctrl.fcb != self.fcb_expected {
                tracing::warn!(
                    expected = self.fcb_expected,
                    "repeated frame, re-sending the previous response"
                );
                match self.last_resp.clone() {
                    Some(f) => {
                        self.write(&f).await;
                    }
                    None => {
                        self.send_link_ack(shared).await;
                    }
                }
                return out;
            }
            self.fcb_expected = !self.fcb_expected;
        }

        match ctrl.fun {
            prim_fc::RESET_LINK => {
                tracing::debug!("link reset requested");
                // The first FCV frame after a reset carries FCB = 1.
                self.fcb_expected = true;
                self.last_resp = None;
                self.send_link_ack(shared).await;
                out.link_became_active = true;
            }
            prim_fc::RESET_USER | prim_fc::TEST_LINK => {
                self.send_link_ack(shared).await;
            }
            prim_fc::USER_DATA_CONF => {
                if !ctrl.fcv {
                    tracing::warn!("confirmed user data with FCV=0, ignored");
                    return out;
                }
                out.asdu = frame.asdu().map(|a| a.to_vec());
                self.send_link_ack(shared).await;
            }
            prim_fc::USER_DATA_NO_CONF => {
                // Unconfirmed data gets no link-layer response.
                out.asdu = frame.asdu().map(|a| a.to_vec());
            }
            prim_fc::REQ_ACCESS | prim_fc::REQ_STATUS => {
                self.send_link_status(shared).await;
            }
            prim_fc::REQ_DATA1 => {
                self.respond_class_data(1, shared).await;
            }
            prim_fc::REQ_DATA2 => {
                self.respond_class_data(2, shared).await;
            }
            _ => {
                tracing::warn!(%ctrl, "unhandled primary function code");
                let f = self.sec_frame(sec_fc::RESP_LINK_NI, false);
                self.send_resp(f).await;
            }
        }
        out
    }

    // -- balanced mode primary role ---------------------------------------

    /// Complete this station's outstanding primary-role transaction.
    fn prim_confirm(&mut self, positive: bool) {
        if self.prim_out.is_none() {
            tracing::warn!("confirmation received with no primary frame outstanding");
            return;
        }
        if positive {
            if self.prim_out_ctrl.fcv {
                self.prim_fcb = !self.prim_out_ctrl.fcb;
            }
            if self.prim_out_ctrl.fun == prim_fc::RESET_LINK {
                tracing::debug!("primary role: link reset confirmed, transmit direction active");
                self.prim_active = true;
                self.prim_fcb = true;
            }
        }
        self.prim_out = None;
        self.prim_retry = 0;
        self.prim_deadline = None;
    }

    /// Handle a response from the peer to this station's primary role.
    fn handle_peer_secondary_frame(&mut self, ctrl: ControlField, cfg: &Config) {
        match ctrl.fun {
            sec_fc::CONF_ACK | sec_fc::RESP_STATUS => self.prim_confirm(true),
            // Link busy: keep the frame outstanding, the deadline retries it.
            sec_fc::CONF_NACK => tracing::warn!("the peer reports the link busy"),
            sec_fc::RESP_LINK_NF | sec_fc::RESP_LINK_NI => {
                tracing::warn!(fc = ctrl.fun, "the peer's link service is not available");
                self.prim_out = None;
                self.prim_retry = 0;
                self.prim_deadline = None;
                self.prim_active = false;
                self.prim_next_init = Instant::now() + cfg.timeout_response_t1;
            }
            _ => tracing::warn!(%ctrl, "unhandled secondary frame from the peer"),
        }
    }

    async fn prim_send_confirmed(&mut self, frame: Frame, cfg: &Config) {
        let ctrl = frame.control().expect("a fixed or variable frame");
        if !self.write(&frame).await {
            return;
        }
        self.prim_out = Some(frame);
        self.prim_out_ctrl = ctrl;
        self.prim_retry = 0;
        self.prim_deadline = Some(Instant::now() + cfg.timeout_response_t1);
    }

    /// Supervise the balanced-mode primary role: initialization, response
    /// timeouts with one repetition, and transmission of queued data.
    async fn prim_tick(&mut self, shared: &Shared, cfg: &Config) {
        let now = Instant::now();

        if self.prim_out.is_some() {
            if self.prim_deadline.is_some_and(|d| now >= d) {
                if self.prim_retry < 1 {
                    self.prim_retry += 1;
                    tracing::warn!("primary role: response timeout, repeating the frame");
                    if let Some(f) = self.prim_out.clone() {
                        self.write(&f).await;
                    }
                    self.prim_deadline = Some(now + cfg.timeout_repeat_t2);
                } else {
                    tracing::error!("primary role: response timeout after the repetition");
                    self.prim_out = None;
                    self.prim_retry = 0;
                    self.prim_deadline = None;
                    self.prim_active = false;
                    self.prim_next_init = now + cfg.timeout_response_t1;
                }
            }
            return;
        }

        if !self.prim_active {
            if now >= self.prim_next_init {
                tracing::debug!("primary role: resetting the link to open the transmit direction");
                // Back off before trying again, so a dead peer is not hammered.
                self.prim_next_init = now + 2 * cfg.timeout_response_t1;
                let f = Frame::Fixed {
                    control: ControlField::primary(prim_fc::RESET_LINK, false, false, false),
                    link_addr: self.link_addr,
                };
                self.prim_send_confirmed(f, cfg).await;
            }
            return;
        }

        let Some(next) = shared.prim_queue.lock().unwrap().pop_front() else {
            return;
        };
        let raw = match next.marshal_binary() {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "dropping an ASDU that cannot be encoded");
                return;
            }
        };
        tracing::debug!(asdu = %next, fcb = self.prim_fcb, "primary role: sending user data");
        let f = Frame::Variable {
            control: ControlField::primary(prim_fc::USER_DATA_CONF, true, self.prim_fcb, false),
            link_addr: self.link_addr,
            asdu: raw,
        };
        self.prim_send_confirmed(f, cfg).await;
    }
}
