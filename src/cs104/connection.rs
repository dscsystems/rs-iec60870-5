// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The IEC 60870-5-104 connection state machine, shared by the master, the
//! controlled-station session and the reverse-connection server.
//!
//! One connection runs four tasks:
//!
//! * a **reader** that turns the byte stream into complete APDUs,
//! * a **writer** that drains the outbound frame queue onto the socket,
//! * the **driver**, which owns the sequence numbers, the k/w windows and the
//!   t₁–t₃ timers, and
//! * a **dispatcher**, which decodes ASDUs and calls the application handler,
//!   so that slow handlers never stall the protocol machine.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Notify, mpsc, watch};

use crate::asdu::{Asdu, Connect, Params};
use crate::cs104::apci::{
    APDU_SIZE_MAX, Apci, SEQ_MASK, UFunction, new_i_frame, new_s_frame, new_u_frame, parse,
    read_apdu, seq_no_count,
};
use crate::cs104::config::{Config, TIMEOUT_RESOLUTION};
use crate::error::{Error, Result};

/// Any bidirectional byte stream a 104 connection can run over: a plain
/// `TcpStream`, a TLS stream, or an in-memory duplex pipe in tests.
pub trait IoStream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> IoStream for T {}

/// Which end of the connection this is, which decides how U-frames are handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// Controlling station: sends StartDT/StopDT and waits for the confirmation.
    Master,
    /// Controlled station: answers StartDT/StopDT and activates on request.
    Controlled,
}

/// A control action requested by the application, forwarded to the driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ctrl {
    StartDt,
    StopDt,
}

/// The application-visible handle to a live connection.
///
/// Implements [`Connect`], so every `asdu` send helper works against it. Handed
/// to handlers and to the connect/disconnect callbacks.
pub struct Connection {
    params: Params,
    tx_asdu: mpsc::Sender<Vec<u8>>,
    tx_ctrl: mpsc::UnboundedSender<Ctrl>,
    connected: AtomicBool,
    active: AtomicBool,
    activated: Notify,
    peer: Option<SocketAddr>,
    role: Role,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("peer", &self.peer)
            .field("role", &self.role)
            .field("connected", &self.is_connected())
            .field("active", &self.is_active())
            .finish()
    }
}

impl Connection {
    /// The system parameters this connection encodes and decodes with.
    pub fn params(&self) -> Params {
        self.params
    }

    /// Whether the TCP connection is up.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    /// Whether data transfer is active, i.e. StartDT has been confirmed.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// The remote address, when known.
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer
    }

    /// Request start of data transfer (`STARTDT act`).
    ///
    /// A 104 connection begins in the STOPDT state: until the peer confirms,
    /// I-frames are discarded and [`Connect::send`] fails with
    /// [`Error::NotActive`]. Only meaningful on a master.
    pub fn send_start_dt(&self) {
        let _ = self.tx_ctrl.send(Ctrl::StartDt);
    }

    /// Request stop of data transfer (`STOPDT act`). Only meaningful on a master.
    pub fn send_stop_dt(&self) {
        let _ = self.tx_ctrl.send(Ctrl::StopDt);
    }

    /// Wait until data transfer becomes active.
    ///
    /// Returns immediately if it already is. This resolves as soon as the
    /// connection activates; it does not resolve if the connection drops, so
    /// pair it with a timeout when that matters.
    pub async fn wait_active(&self) {
        loop {
            // Register before re-checking, so an activation racing with this
            // call cannot be missed.
            let notified = self.activated.notified();
            if self.is_active() {
                return;
            }
            notified.await;
        }
    }

    fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Release);
        if active {
            self.activated.notify_waiters();
        }
    }
}

#[async_trait::async_trait]
impl Connect for Connection {
    fn params(&self) -> Params {
        self.params
    }

    async fn send(&self, a: Asdu) -> Result<()> {
        if !self.is_connected() {
            return Err(Error::UseClosedConnection);
        }
        // A master must not queue process data before StartDT is confirmed; a
        // controlled station may, and delivers it once the master activates.
        if self.role == Role::Master && !self.is_active() {
            return Err(Error::NotActive);
        }
        let data = a.marshal_binary()?;
        self.tx_asdu.try_send(data).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => Error::BufferFull,
            mpsc::error::TrySendError::Closed(_) => Error::UseClosedConnection,
        })
    }

    fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer
    }
}

/// Decodes received ASDUs and routes them to the application handler.
#[async_trait::async_trait]
pub(crate) trait Dispatcher: Send + Sync + 'static {
    async fn dispatch(&self, conn: &Arc<Connection>, pack: Asdu);
}

/// Callbacks fired when a connection comes up and goes down.
#[derive(Clone, Default)]
pub(crate) struct Callbacks {
    pub on_connect: Option<Arc<dyn Fn(Arc<Connection>) + Send + Sync>>,
    pub on_connection_lost: Option<Arc<dyn Fn(Arc<Connection>) + Send + Sync>>,
    pub on_activated: Option<Arc<dyn Fn(Arc<Connection>) + Send + Sync>>,
    pub on_deactivated: Option<Arc<dyn Fn(Arc<Connection>) + Send + Sync>>,
}

/// Everything one connection run needs.
pub(crate) struct RunOptions {
    pub config: Config,
    pub params: Params,
    pub role: Role,
    pub peer: Option<SocketAddr>,
    /// Send `STARTDT act` as soon as the connection comes up (masters only).
    pub auto_start_dt: bool,
    pub callbacks: Callbacks,
}

/// Run one connection to completion.
///
/// Returns when the peer disconnects, a protocol violation is detected, a
/// timeout expires, or `shutdown` is signalled. The [`Connection`] handle is
/// published through `publish` before the first callback fires.
pub(crate) async fn run<S, D>(
    stream: S,
    opts: RunOptions,
    dispatcher: Arc<D>,
    mut shutdown: watch::Receiver<bool>,
    publish: impl FnOnce(Arc<Connection>),
) where
    S: IoStream,
    D: Dispatcher + ?Sized,
{
    let cfg = opts.config;
    // Queue depths follow go-iecp5: enough headroom that a burst of process
    // data never blocks the protocol machine.
    let (tx_asdu, mut rx_asdu) = mpsc::channel::<Vec<u8>>((cfg.send_unack_limit_k as usize) << 4);
    let (tx_ctrl, mut rx_ctrl) = mpsc::unbounded_channel::<Ctrl>();
    let (tx_frame, mut rx_frame) = mpsc::channel::<Vec<u8>>((cfg.recv_unack_limit_w as usize) << 5);
    let (tx_raw, mut rx_raw) = mpsc::channel::<Vec<u8>>((cfg.send_unack_limit_k as usize) << 5);
    let (tx_pack, mut rx_pack) = mpsc::channel::<Asdu>((cfg.recv_unack_limit_w as usize) << 4);

    let conn = Arc::new(Connection {
        params: opts.params,
        tx_asdu,
        tx_ctrl,
        connected: AtomicBool::new(true),
        active: AtomicBool::new(false),
        activated: Notify::new(),
        peer: opts.peer,
        role: opts.role,
    });
    publish(Arc::clone(&conn));

    // `local` signals the reader, writer and dispatcher to stop as soon as the
    // driver leaves its loop, for any reason.
    let (tx_local, rx_local) = watch::channel(false);
    let (mut reader, mut writer) = tokio::io::split(stream);

    let reader_task = {
        let mut stop = rx_local.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; APDU_SIZE_MAX];
            loop {
                tokio::select! {
                    _ = stop.changed() => return,
                    r = read_apdu(&mut reader, &mut buf) => match r {
                        Ok(n) => {
                            tracing::trace!(apdu = ?&buf[..n], "RX raw");
                            if tx_frame.send(buf[..n].to_vec()).await.is_err() {
                                return;
                            }
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, "receive stopped");
                            return;
                        }
                    },
                }
            }
        })
    };

    let writer_task = {
        let mut stop = rx_local.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.changed() => break,
                    frame = rx_raw.recv() => {
                        let Some(frame) = frame else { break };
                        tracing::trace!(apdu = ?frame, "TX raw");
                        if let Err(e) = writer.write_all(&frame).await {
                            tracing::debug!(error = %e, "send failed");
                            break;
                        }
                    }
                }
            }
            let _ = writer.shutdown().await;
        })
    };

    let dispatch_task = {
        let conn = Arc::clone(&conn);
        let dispatcher = Arc::clone(&dispatcher);
        let mut stop = rx_local.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.changed() => return,
                    pack = rx_pack.recv() => {
                        let Some(pack) = pack else { return };
                        dispatcher.dispatch(&conn, pack).await;
                    }
                }
            }
        })
    };

    if let Some(cb) = opts.callbacks.on_connect.as_ref() {
        cb(Arc::clone(&conn));
    }
    if opts.role == Role::Master && opts.auto_start_dt {
        conn.send_start_dt();
    }

    // -- driver state ------------------------------------------------------
    let mut seq_no_send: u16 = 0; // sequence number of the next outbound I-frame
    let mut ack_no_send: u16 = 0; // outbound sequence number not yet confirmed
    let mut seq_no_rcv: u16 = 0; // sequence number of the next inbound I-frame
    let mut ack_no_rcv: u16 = 0; // inbound sequence number not yet acknowledged
    let mut pending: VecDeque<(u16, Instant)> = VecDeque::new();

    let mut unack_rcv_since: Option<Instant> = None;
    let mut idle_since = Instant::now();
    let mut test_fr_since: Option<Instant> = None;
    let mut start_dt_since: Option<Instant> = None;
    let mut stop_dt_since: Option<Instant> = None;

    let mut ticker = tokio::time::interval(TIMEOUT_RESOLUTION);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        // "k" caps the number of unacknowledged I-frames in flight.
        let window_open = seq_no_count(ack_no_send, seq_no_send) < cfg.send_unack_limit_k;
        let may_send = conn.is_active() && window_open;

        tokio::select! {
            biased;

            _ = shutdown.changed() => {
                tracing::debug!("connection shutdown requested");
                break;
            }

            asdu = rx_asdu.recv(), if may_send => {
                let Some(asdu) = asdu else { break };
                let Ok(frame) = new_i_frame(seq_no_send, seq_no_rcv, &asdu) else {
                    tracing::warn!("dropping oversized ASDU");
                    continue;
                };
                tracing::debug!(%seq_no_send, %seq_no_rcv, "TX I-frame");
                ack_no_rcv = seq_no_rcv;
                pending.push_back((seq_no_send, Instant::now()));
                seq_no_send = (seq_no_send + 1) & SEQ_MASK;
                idle_since = Instant::now();
                if tx_raw.send(frame).await.is_err() {
                    break;
                }
            }

            ctrl = rx_ctrl.recv() => {
                let Some(ctrl) = ctrl else { break };
                let (func, since) = match ctrl {
                    Ctrl::StartDt => (UFunction::StartDtActive, &mut start_dt_since),
                    Ctrl::StopDt => (UFunction::StopDtActive, &mut stop_dt_since),
                };
                *since = Some(Instant::now());
                tracing::debug!(%func, "TX U-frame");
                if tx_raw.send(new_u_frame(func).to_vec()).await.is_err() {
                    break;
                }
            }

            frame = rx_frame.recv() => {
                let Some(frame) = frame else {
                    tracing::debug!("peer closed the connection");
                    break;
                };
                // Any I, S or U frame restarts the t3 idle timer.
                idle_since = Instant::now();
                let (apci, raw_asdu) = parse(&frame);

                match apci {
                    Apci::S { recv_sn } => {
                        tracing::debug!(%apci, "RX S-frame");
                        if !update_ack_no_out(recv_sn, &mut ack_no_send, seq_no_send, &mut pending) {
                            tracing::error!(
                                "acknowledge is either earlier than the previous one or ahead of what was sent"
                            );
                            break;
                        }
                    }

                    Apci::I { send_sn, recv_sn } => {
                        tracing::debug!(%apci, "RX I-frame");
                        if !conn.is_active() {
                            tracing::warn!("station not active, discarding I-frame");
                            continue;
                        }
                        if !update_ack_no_out(recv_sn, &mut ack_no_send, seq_no_send, &mut pending)
                            || send_sn != seq_no_rcv
                        {
                            tracing::error!(
                                expected = seq_no_rcv,
                                got = send_sn,
                                "out of sequence I-frame, closing"
                            );
                            break;
                        }

                        match Asdu::unmarshal_binary(opts.params, raw_asdu) {
                            Ok(pack) => {
                                if tx_pack.send(pack).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => tracing::warn!(error = %e, "discarding undecodable ASDU"),
                        }

                        if ack_no_rcv == seq_no_rcv {
                            // First frame of a new unacknowledged run.
                            unack_rcv_since = Some(Instant::now());
                        }
                        seq_no_rcv = (seq_no_rcv + 1) & SEQ_MASK;
                        if seq_no_count(ack_no_rcv, seq_no_rcv) >= cfg.recv_unack_limit_w {
                            tracing::debug!(seq_no_rcv, "TX S-frame (w reached)");
                            if tx_raw.send(new_s_frame(seq_no_rcv).to_vec()).await.is_err() {
                                break;
                            }
                            ack_no_rcv = seq_no_rcv;
                        }
                    }

                    Apci::U { function } => {
                        tracing::debug!(%apci, "RX U-frame");
                        match (function, opts.role) {
                            (Some(UFunction::StartDtActive), Role::Controlled) => {
                                if tx_raw
                                    .send(new_u_frame(UFunction::StartDtConfirm).to_vec())
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                                conn.set_active(true);
                                if let Some(cb) = opts.callbacks.on_activated.as_ref() {
                                    cb(Arc::clone(&conn));
                                }
                            }
                            (Some(UFunction::StopDtActive), Role::Controlled) => {
                                if tx_raw
                                    .send(new_u_frame(UFunction::StopDtConfirm).to_vec())
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                                conn.set_active(false);
                                if let Some(cb) = opts.callbacks.on_deactivated.as_ref() {
                                    cb(Arc::clone(&conn));
                                }
                            }
                            (Some(UFunction::StartDtConfirm), Role::Master) => {
                                conn.set_active(true);
                                start_dt_since = None;
                                if let Some(cb) = opts.callbacks.on_activated.as_ref() {
                                    cb(Arc::clone(&conn));
                                }
                            }
                            (Some(UFunction::StopDtConfirm), Role::Master) => {
                                conn.set_active(false);
                                stop_dt_since = None;
                                if let Some(cb) = opts.callbacks.on_deactivated.as_ref() {
                                    cb(Arc::clone(&conn));
                                }
                            }
                            (Some(UFunction::TestFrActive), _) => {
                                if tx_raw
                                    .send(new_u_frame(UFunction::TestFrConfirm).to_vec())
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            (Some(UFunction::TestFrConfirm), _) => test_fr_since = None,
                            (Some(f), role) => {
                                tracing::warn!(%f, ?role, "U-frame function not valid for this role");
                            }
                            (None, _) => tracing::error!("illegal U-frame function ignored"),
                        }
                    }
                }
            }

            now = ticker.tick() => {
                let now = now.into_std();

                // t1: an outstanding TestFR, StartDT or StopDT was never confirmed.
                let waiting_since = [test_fr_since, start_dt_since, stop_dt_since];
                if waiting_since
                    .iter()
                    .flatten()
                    .any(|t| now.saturating_duration_since(*t) >= cfg.send_unack_timeout1)
                {
                    tracing::error!("t1 expired waiting for a U-frame confirmation");
                    break;
                }

                // t1: the oldest unacknowledged I-frame was never confirmed.
                if ack_no_send != seq_no_send
                    && pending.front().is_some_and(|(_, t)| {
                        now.saturating_duration_since(*t) >= cfg.send_unack_timeout1
                    })
                {
                    tracing::error!("t1 expired, fatal transmission timeout");
                    break;
                }

                // t2: acknowledge what we received, either because t2 elapsed or
                // because the line fell idle for a tick after a burst.
                if ack_no_rcv != seq_no_rcv
                    && (unack_rcv_since.is_some_and(|t| {
                            now.saturating_duration_since(t) >= cfg.recv_unack_timeout2
                        })
                        || now.saturating_duration_since(idle_since) >= TIMEOUT_RESOLUTION)
                {
                    tracing::debug!(seq_no_rcv, "TX S-frame (t2)");
                    if tx_raw.send(new_s_frame(seq_no_rcv).to_vec()).await.is_err() {
                        break;
                    }
                    ack_no_rcv = seq_no_rcv;
                }

                // t3: the line has been idle, send a keep-alive.
                if now.saturating_duration_since(idle_since) >= cfg.idle_timeout3 {
                    tracing::debug!("TX U-frame TestFrActive (t3)");
                    if tx_raw
                        .send(new_u_frame(UFunction::TestFrActive).to_vec())
                        .await
                        .is_err()
                    {
                        break;
                    }
                    test_fr_since = Some(Instant::now());
                    idle_since = Instant::now();
                }
            }
        }
    }

    // -- teardown ----------------------------------------------------------
    conn.connected.store(false, Ordering::Release);
    conn.set_active(false);
    let _ = tx_local.send(true);
    // Dropping the driver's senders lets the reader and dispatcher finish.
    drop(tx_raw);
    drop(tx_pack);
    let _ = tokio::join!(reader_task, writer_task, dispatch_task);

    if let Some(cb) = opts.callbacks.on_connection_lost.as_ref() {
        cb(Arc::clone(&conn));
    }
    tracing::debug!("connection stopped");
}

/// Apply a received acknowledgement to the outbound window.
///
/// Returns `false` when the acknowledgement is outside the window, which the
/// standard requires to close the connection.
fn update_ack_no_out(
    ack_no: u16,
    ack_no_send: &mut u16,
    seq_no_send: u16,
    pending: &mut VecDeque<(u16, Instant)>,
) -> bool {
    if ack_no == *ack_no_send {
        return true;
    }
    // An acknowledgement may never run ahead of what has actually been sent.
    if seq_no_count(*ack_no_send, seq_no_send) < seq_no_count(ack_no, seq_no_send) {
        return false;
    }
    // Sequence numbers live in [0, 32767], so the predecessor of 0 is 32767.
    let last_acked = ack_no.wrapping_sub(1) & SEQ_MASK;
    if let Some(pos) = pending.iter().position(|(seq, _)| *seq == last_acked) {
        pending.drain(..=pos);
    }
    *ack_no_send = ack_no;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pend(seqs: &[u16]) -> VecDeque<(u16, Instant)> {
        seqs.iter().map(|s| (*s, Instant::now())).collect()
    }

    #[test]
    fn repeating_the_current_acknowledge_is_a_no_op() {
        let mut ack = 5;
        let mut p = pend(&[5, 6]);
        assert!(update_ack_no_out(5, &mut ack, 7, &mut p));
        assert_eq!(ack, 5);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn acknowledging_confirms_and_drops_pending_frames() {
        let mut ack = 0;
        let mut p = pend(&[0, 1, 2, 3]);
        assert!(update_ack_no_out(3, &mut ack, 4, &mut p));
        assert_eq!(ack, 3);
        assert_eq!(p.iter().map(|(s, _)| *s).collect::<Vec<_>>(), vec![3]);
    }

    #[test]
    fn an_acknowledge_ahead_of_the_send_sequence_is_rejected() {
        let mut ack = 0;
        let mut p = pend(&[0, 1]);
        // Only frames 0 and 1 were sent; acknowledging 5 is a protocol violation.
        assert!(!update_ack_no_out(5, &mut ack, 2, &mut p));
        assert_eq!(ack, 0);
    }

    #[test]
    fn acknowledging_works_across_the_wraparound() {
        let mut ack = 32766;
        let mut p = pend(&[32766, 32767, 0]);
        // Acknowledge everything up to and including sequence number 0.
        assert!(update_ack_no_out(1, &mut ack, 1, &mut p));
        assert_eq!(ack, 1);
        assert!(p.is_empty());
    }
}
