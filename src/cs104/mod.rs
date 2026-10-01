// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! IEC 60870-5-104: the application layer of 101 carried over TCP/IP.
//!
//! This module implements the transport (APCI) with its full state machine:
//! I/S/U frames, the k and w flow-control windows, the t₀–t₃ timers,
//! StartDT/StopDT activation and TestFR keep-alive.
//!
//! | Type | Role |
//! |------|------|
//! | [`Client`] | controlling station (master): connects to an outstation |
//! | [`Server`] | controlled station (outstation): listens for masters |
//! | [`ServerSpecial`] | controlled station that dials out to the master, for NAT traversal |
//!
//! Default port [`PORT`] (2404), or [`PORT_SECURE`] (19998) with TLS. The
//! default ASDU parameters are [`PARAMS_WIDE`](crate::asdu::PARAMS_WIDE) — cause
//! of transmission 2 with originator address, common address 2, information
//! object address 3 — which is the IEC 104 standard layout.
//!
//! # Controlled station
//!
//! ```no_run
//! use rs_iec60870_5::asdu::*;
//! use rs_iec60870_5::cs104::{Server, ServerHandler};
//!
//! struct Outstation;
//!
//! #[async_trait::async_trait]
//! impl ServerHandler for Outstation {
//!     async fn interrogation(
//!         &self,
//!         c: &dyn Connect,
//!         pack: &Asdu,
//!         qoi: QualifierOfInterrogation,
//!     ) -> rs_iec60870_5::Result<()> {
//!         if qoi != QualifierOfInterrogation::STATION {
//!             return c.send(pack.reply_mirror(Cause::ACTIVATION_CON).negated()).await;
//!         }
//!         c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
//!
//!         let coa = CauseOfTransmission::new(Cause::INTERROGATED_BY_STATION);
//!         c.send_single(false, coa, pack.common_addr(), &[SinglePointInfo::new(100, true)]).await?;
//!         c.send_measured_value_float(
//!             false,
//!             coa,
//!             pack.common_addr(),
//!             &[MeasuredValueFloatInfo { ioa: 400, value: 22.5, ..Default::default() }],
//!         )
//!         .await?;
//!
//!         c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
//!     }
//!
//!     async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
//!         if pack.type_id() == TypeId::C_SC_NA_1 {
//!             let cmd = pack.get_single_cmd()?;
//!             println!("single command ioa={} value={}", cmd.ioa, cmd.value);
//!             c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
//!             return c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await;
//!         }
//!         Ok(())
//!     }
//! }
//!
//! # #[tokio::main]
//! # async fn main() -> rs_iec60870_5::Result<()> {
//! let srv = Server::new(Outstation);
//!
//! // Push spontaneous data to every connected master at any time.
//! let publisher = srv.clone();
//! tokio::spawn(async move {
//!     let coa = CauseOfTransmission::new(Cause::SPONTANEOUS);
//!     let info = SinglePointInfo { ioa: 100, value: false, time: Some(chrono::Utc::now()), ..Default::default() };
//!     let _ = publisher.send_single_cp56time2a(coa, 1, &[info]).await;
//! });
//!
//! srv.listen_and_serve("0.0.0.0:2404").await
//! # }
//! ```
//!
//! # Behaviour notes
//!
//! * Sequence numbers are 15 bit and wrap at 32767; the window bookkeeping
//!   handles the wraparound.
//! * An I-frame with an unexpected send sequence number, or an acknowledgement
//!   outside the window, closes the connection as the standard requires;
//!   the client's automatic reconnection then recovers.
//! * Sends are queued and packed into I-frames by the connection task. A full
//!   queue yields [`Error::BufferFull`](crate::Error::BufferFull) — back off
//!   and retry rather than dropping data silently.
//! * An I-frame received while the connection is stopped closes a controlled
//!   station's connection, since a master may not send one there. A master
//!   accepts one and numbers it, which keeps the link consistent.
//! * STOPDT is a handshake, not a switch: a master sends no more I-frames once
//!   it has sent STOPDT act, and a controlled station acknowledges what it has
//!   received, stops sending, and confirms only when its own I-frames have been
//!   acknowledged.
//! * [`Server`] sends spontaneous data once per redundancy group, to the
//!   group's connection in data transfer (see [`ServerMode`]). A standby is sent
//!   nothing; an optional event buffer keeps data for the next master to start.
//! * t₃ measures how long the peer has been silent: only received frames
//!   restart it, so a station that keeps transmitting still tests the link.

mod apci;
mod client;
mod config;
mod connection;
mod handler;
mod redundancy;
mod server;

pub use apci::{Apci, UFunction};
pub use client::{Client, ClientOption, DEFAULT_RECONNECT_INTERVAL};
pub use config::{
    CONNECT_TIMEOUT0_MAX, CONNECT_TIMEOUT0_MIN, Config, IDLE_TIMEOUT3_MAX, IDLE_TIMEOUT3_MIN, PORT,
    PORT_SECURE, RECV_UNACK_LIMIT_W_MAX, RECV_UNACK_LIMIT_W_MIN, RECV_UNACK_TIMEOUT2_MAX,
    RECV_UNACK_TIMEOUT2_MIN, SEND_UNACK_LIMIT_K_MAX, SEND_UNACK_LIMIT_K_MIN,
    SEND_UNACK_TIMEOUT1_MAX, SEND_UNACK_TIMEOUT1_MIN,
};
pub use connection::{Connection, IoStream};
pub use handler::{ClientContext, ClientHandler, ServerHandler};
pub use redundancy::{RedundancyGroup, ServerMode};
pub use server::{Server, ServerSpecial, Waiting, WaitingServer, waiting};
pub use crate::net::{Endpoint, Stream, TlsClientConfig, TlsServerConfig};
