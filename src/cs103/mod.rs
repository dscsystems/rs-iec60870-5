// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! IEC 60870-5-103: the companion standard for the informative interface of
//! protection equipment.
//!
//! The link layer is the same FT1.2 unbalanced procedure as
//! [`cs101`](crate::cs101) — the module reuses its frame codec and transports —
//! while the application layer is 103-specific:
//!
//! ```text
//! ASDU = | type ID | VSQ | cause | common address | FUN | INF | elements... |
//! bytes |    1    |  1  |   1   |       1        |  1  |  1  |      n      |
//! ```
//!
//! Key differences from 101 and 104:
//!
//! * Every identifier field is one octet; there is no `asdu::Params`.
//! * Information objects are addressed by **function type** ([`fun`]) and
//!   **information number** ([`inf`]) instead of an information object address.
//! * Events carry the 4-octet [`cp32time2a`] tag, whose date comes from the
//!   local clock; time synchronization uses CP56Time2a.
//! * Measurands are signed 13-bit values with OV and ER flags;
//!   [`Measurand::f64`] gives the fraction of full scale.
//! * The link address and the ASDU common address are conventionally equal, and
//!   this module uses one address per device for both.
//!
//! # Example
//!
//! ```no_run
//! use rs_iec60870_5::cs103::{
//!     Asdu, Client, ClientHandler, ClientOption, Config, Dco, Link, MeasurandsInfo,
//!     SerialConfig, TimeTaggedInfo, fun, inf,
//! };
//!
//! struct Relays;
//!
//! #[async_trait::async_trait]
//! impl ClientHandler for Relays {
//!     async fn time_tagged(
//!         &self,
//!         _link: &dyn Link,
//!         pack: &Asdu,
//!         info: TimeTaggedInfo,
//!     ) -> rs_iec60870_5::Result<()> {
//!         println!("device {} FUN={} INF={} {}", pack.common_addr, info.fun, info.inf, info.dpi);
//!         Ok(())
//!     }
//!
//!     async fn measurands(
//!         &self,
//!         _link: &dyn Link,
//!         _pack: &Asdu,
//!         info: MeasurandsInfo,
//!     ) -> rs_iec60870_5::Result<()> {
//!         for m in &info.values {
//!             println!("  {:.4} of full scale", m.f64());
//!         }
//!         Ok(())
//!     }
//! }
//!
//! # #[tokio::main]
//! # async fn main() -> rs_iec60870_5::Result<()> {
//! let mut cfg = Config::new();
//! cfg.serial = SerialConfig::new("/dev/ttyUSB0", 9600); // 8E1 by default
//! cfg.link_address = 3;
//!
//! let cli = Client::new(Relays, ClientOption::new().with_config(cfg)?);
//! cli.start()?;
//! cli.wait_link_active().await;
//!
//! // Link init, time sync and general interrogation already ran; issue a command.
//! cli.general_command(3, fun::OVERCURRENT_PROTECTION, inf::AUTO_RECLOSER_ACTIVE, Dco::On, 42)
//! # }
//! ```
//!
//! # Not implemented
//!
//! * Generic services (ASDU 10, 11 and 21): the type identifications and raw
//!   send and receive work through [`ClientHandler::asdu`] and
//!   [`Asdu::new`]/[`Asdu::append`], but there are no structured GIN/GDD/GID
//!   codecs.
//! * Disturbance data transfer (ASDU 23 to 31), the 103 equivalent of file
//!   transfer.
//! * The secondary (device) side; this module is a master only.

mod asdu;
mod client;
mod config;
mod elements;
mod handler;

pub use client::{Client, ClientOption, DEFAULT_RECONNECT_INTERVAL, PRIM_FC_RESET_FCB};
pub use handler::{ClientHandler, Link};
pub use asdu::{
    ASDU_SIZE_MAX, Asdu, Cause, GLOBAL_COMMON_ADDR, GeneralCommandInfo, IDENTIFIER_SIZE,
    IdentificationInfo, MeasurandsInfo, TimeTaggedInfo, TimeTaggedMeasurandsInfo, TypeId,
};
pub use config::{
    Config, DEFAULT_MAX_SEND_QUEUE_SIZE, DEFAULT_TIMEOUT_REPEAT_T2, DEFAULT_TIMEOUT_RESPONSE_T1,
    DEFAULT_TIMEOUT_SEND_LINK_MSG, DEFAULT_TIMEOUT_TEST_T3, LINK_ADDR_SIZE,
};
pub use elements::{CP32TIME2A_SIZE, Dco, Dpi, Measurand, cp32time2a, fun, inf, parse_cp32time2a};

// The 103 link layer is the FT1.2 procedure of 101, so its transport and
// serial settings are shared rather than duplicated.
pub use crate::cs101::{Parity, SerialConfig, StopBits, TcpConfig, TransportType};
