// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! IEC 60870-5-101: the application layer over the FT1.2 serial link layer.
//!
//! | Type | Role |
//! |------|------|
//! | [`Client`] | primary station (master): drives the link and polls |
//! | [`Server`] | secondary station (outstation): answers polls |
//!
//! Two transmission procedures are supported:
//!
//! * **Unbalanced** (the default): one primary polls one or more secondaries on
//!   a shared line. Only the primary initiates; secondaries answer polls, and
//!   announce pending events with the ACD bit.
//! * **Balanced** (point to point): both stations may transmit spontaneously,
//!   each running a primary role for sending and a secondary role for
//!   acknowledging. Set [`TransmissionMode::Balanced`] on both ends and give
//!   them the same link address.
//!
//! The default ASDU parameters are
//! [`PARAMS_STANDARD_101`](crate::asdu::PARAMS_STANDARD_101) — cause of
//! transmission 1, common address 1, information object address 2. Both ends
//! must agree on these and on `link_addr_size`, `link_address` and `mode`.
//!
//! # Primary station
//!
//! ```no_run
//! use rs_iec60870_5::asdu::*;
//! use rs_iec60870_5::cs101::{Client, ClientHandler, ClientOption, Config, SerialConfig};
//!
//! struct Master;
//!
//! #[async_trait::async_trait]
//! impl ClientHandler for Master {
//!     async fn interrogation(&self, _c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
//!         if pack.type_id() == TypeId::M_SP_NA_1 {
//!             for p in pack.get_single_point()? {
//!                 println!("ioa={} value={}", p.ioa, p.value);
//!             }
//!         }
//!         Ok(())
//!     }
//! }
//!
//! # #[tokio::main]
//! # async fn main() -> rs_iec60870_5::Result<()> {
//! let mut cfg = Config::new();
//! cfg.serial = SerialConfig::new("/dev/ttyUSB0", 9600); // 8E1 by default
//! cfg.link_address = 1;
//!
//! let cli = Client::new(Master, ClientOption::new().with_config(cfg)?);
//! cli.start()?;
//! cli.wait_link_active().await;
//!
//! cli.interrogation_cmd(
//!     CauseOfTransmission::new(Cause::ACTIVATION),
//!     1,
//!     QualifierOfInterrogation::STATION,
//! )
//! .await
//! # }
//! ```
//!
//! # TCP encapsulation
//!
//! The FT1.2 frames can be carried over a TCP stream instead of a local port —
//! the usual arrangement with a terminal server or serial-device server. Only
//! the transport changes; the link procedure, the handlers and the ASDUs are
//! identical.
//!
//! ```
//! use rs_iec60870_5::cs101::{Config, TcpConfig, TransportType};
//!
//! let mut cfg = Config::new();
//! cfg.transport = TransportType::TcpClient; // or TcpServer to listen
//! cfg.tcp = TcpConfig { address: "10.0.0.9:2400".into(), ..Default::default() };
//! # assert_eq!(cfg.transport_label(), "10.0.0.9:2400");
//! ```
//!
//! This is still 101 inside a TCP pipe — FT1.2 framing with FCB, acknowledgement
//! and polling — not IEC 104. Keep t₁ and t₂ generous enough for the added
//! network latency.
//!
//! # Troubleshooting
//!
//! * `unexpected link address` in the log: `link_address` or `link_addr_size`
//!   disagree between the ends.
//! * ASDUs decoding as garbage: the `asdu::Params` differ between the ends.
//! * Checksum and length violations are logged and skipped; they never stop the
//!   receiver, which resynchronises on the next octet.

mod client;
mod config;
mod frame;
mod handler;
mod server;
mod transport;

pub use client::{Client, ClientOption, DEFAULT_RECONNECT_INTERVAL};
pub use config::{
    Config, DEFAULT_LINK_ADDR_SIZE, DEFAULT_MAX_APDU_LENGTH, DEFAULT_MAX_SEND_QUEUE_SIZE,
    DEFAULT_TCP_CONNECT_TIMEOUT, DEFAULT_TIMEOUT_REPEAT_T2, DEFAULT_TIMEOUT_RESPONSE_T1,
    DEFAULT_TIMEOUT_SEND_LINK_MSG, DEFAULT_TIMEOUT_TEST_T3, Parity, SerialConfig, StopBits,
    TcpConfig, TransmissionMode, TransportType,
};
pub use frame::{
    ControlField, Frame, MAX_FRAME_LEN, SINGLE_CHAR_ACK, START_FIXED, START_VARIABLE, prim_fc,
    read_frame, sec_fc,
};
pub use handler::{ClientHandler, ServerHandler};
pub use server::Server;
pub use transport::{LinkStream, Transporter};
