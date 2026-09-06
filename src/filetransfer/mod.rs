// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The IEC 60870-5-101/-104 file transfer procedures, on top of the
//! [`asdu`](crate::asdu) file types `F_FR_NA_1 <120>` … `F_DR_TA_1 <126>`.
//!
//! Two components drive a transfer in the monitor direction — a file
//! travelling from the controlled station to the controlling station, which is
//! the common case, such as fetching disturbance records from a protection
//! relay:
//!
//! | Component | Side | Role |
//! |---|---|---|
//! | [`Sender`] | controlled station | offers files and serves them |
//! | [`Receiver`] | controlling station | requests files and assembles them |
//!
//! Both are transport agnostic: they act on a [`Connect`](crate::asdu::Connect)
//! and consume the ASDUs handed to them by the application's handler, so the
//! same code works with a `cs104` or a `cs101` endpoint.
//!
//! Files are held by a pluggable [`Store`]; [`MemStore`] is the in-memory
//! implementation, and any other backing — a directory on disk, a database —
//! can implement the trait.
//!
//! # Serving files from an outstation
//!
//! ```no_run
//! use std::sync::Arc;
//! use rs_iec60870_5::asdu::{Asdu, Connect, NameOfFile};
//! use rs_iec60870_5::cs104::ServerHandler;
//! use rs_iec60870_5::filetransfer::{MemStore, Sender};
//!
//! struct Outstation {
//!     files: Arc<Sender>,
//! }
//!
//! #[async_trait::async_trait]
//! impl ServerHandler for Outstation {
//!     async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
//!         // The transfer runs itself; anything else is the application's.
//!         if self.files.handle(c, pack).await? {
//!             return Ok(());
//!         }
//!         Ok(())
//!     }
//! }
//!
//! # fn main() {
//! let store = Arc::new(MemStore::new());
//! store.insert(100, NameOfFile::DISTURBANCE_DATA, std::fs::read("record.bin").unwrap());
//! let outstation = Outstation { files: Arc::new(Sender::new(store)) };
//! # let _ = outstation;
//! # }
//! ```
//!
//! # Fetching files from a master
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use rs_iec60870_5::asdu::{Connect, NameOfFile};
//! # use rs_iec60870_5::filetransfer::{MemStore, Receiver};
//! # async fn f(c: &dyn Connect) -> rs_iec60870_5::Result<()> {
//! let files = Arc::new(Receiver::new(Arc::new(MemStore::new())));
//! files.set_file_handler(Box::new(|entry, data| {
//!     println!("received {} octets for IOA {}", data.len(), entry.ioa);
//! }));
//!
//! // Ask what is there, then fetch one.
//! files.request_directory(c, 1).await?;
//! files.request_file(c, 1, 100, NameOfFile::DISTURBANCE_DATA).await?;
//! # Ok(()) }
//! ```
//!
//! Feed every received ASDU to `handle`, exactly as in the outstation example;
//! the section requests, checksum verification and acknowledgements are then
//! handled for you, and the file handler fires when the file is complete.

mod receiver;
mod sender;
mod store;

pub use receiver::{DirectoryHandler, FileHandler, Receiver};
pub use sender::{DEFAULT_SECTION_SIZE, Sender};
pub use store::{Entry, MemStore, Store};

#[cfg(test)]
mod tests;
