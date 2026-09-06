// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Pure Rust IEC 60870-5 telecontrol protocol library.
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`asdu`] | Application layer shared by 101 and 104: ASDU encoding and decoding for the standard type identifications, causes of transmission, quality descriptors and time tags |
//! | [`cs104`] | IEC 60870-5-104 client (master) and server (controlled station) over TCP/IP, optionally TLS |
//! | [`cs101`] | IEC 60870-5-101 primary and secondary station over serial FT1.2, unbalanced (multi-drop) and balanced |
//! | [`cs103`] | IEC 60870-5-103 primary station (master) for protection equipment |
//!
//! Vocabulary: *master* = controlling station = client; *outstation* = RTU =
//! slave = controlled station = server. *Monitor direction* is data flowing to
//! the master (`M_*` types); *control direction* is commands to the outstation
//! (`C_*` types).
//!
//! # Quick start: IEC 104 master
//!
//! ```no_run
//! use rs_iec60870_5::asdu::*;
//! use rs_iec60870_5::cs104::{Client, ClientOption};
//!
//! # #[derive(Default)] struct MyHandler;
//! # #[async_trait::async_trait]
//! # impl rs_iec60870_5::cs104::ClientHandler for MyHandler {
//! #     async fn asdu(&self, _: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> { Ok(()) }
//! # }
//! #[tokio::main]
//! async fn main() -> rs_iec60870_5::Result<()> {
//!     let option = ClientOption::new().with_server("127.0.0.1:2404")?;
//!     let client = Client::new(MyHandler::default(), option);
//!
//!     client.start();
//!     client.wait_active().await;
//!     client
//!         .interrogation_cmd(
//!             CauseOfTransmission::new(Cause::ACTIVATION),
//!             1,
//!             QualifierOfInterrogation::STATION,
//!         )
//!         .await?;
//!     Ok(())
//! }
//! ```
//!
//! # Interoperability
//!
//! The wire behaviour is verified against `github.com/riclolsen/go-iecp5` and
//! is designed to interoperate with other publicly available projects and
//! other conforming implementations at the default parameters
//! (k = 12, w = 8, t₁ = 15 s, t₂ = 10 s, t₃ = 20 s).

#![warn(missing_docs)]

pub mod asdu;
mod error;
pub mod net;

#[cfg(feature = "cs101")]
pub mod cs101;
#[cfg(feature = "filetransfer")]
pub mod filetransfer;
#[cfg(feature = "cs103")]
pub mod cs103;
#[cfg(feature = "cs104")]
pub mod cs104;

pub use error::{Error, Result};

/// Compiles the README's code blocks as doctests, so the front page cannot
/// drift from the API. Not part of the public interface.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;
