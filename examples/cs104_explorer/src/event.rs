// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Messages pushed into the UI loop from the background client tasks.
//!
//! The protocol runs on its own Tokio tasks; nothing there may touch the
//! terminal, so everything reaches the UI as one of these over a channel.

use tokio::sync::mpsc;

use crate::handler::Point;

/// One message for the UI loop.
#[derive(Debug, Clone)]
pub enum Event {
    /// A line for the protocol log panel.
    Log(String),
    /// Monitored points decoded from a received ASDU.
    Points(Vec<Point>),
    /// A change in connection state, with an optional note for the log.
    Status {
        connected: bool,
        active: bool,
        note: Option<String>,
    },
}

impl Event {
    /// A status change that also logs a note.
    pub fn status(connected: bool, active: bool, note: impl Into<String>) -> Event {
        Event::Status {
            connected,
            active,
            note: Some(note.into()),
        }
    }
}

/// The sending half handed to the client handler and callbacks.
pub type Sender = mpsc::UnboundedSender<Event>;

/// The receiving half the UI loop selects on.
pub type Receiver = mpsc::UnboundedReceiver<Event>;

/// Create the channel connecting the protocol tasks to the UI.
pub fn channel() -> (Sender, Receiver) {
    mpsc::unbounded_channel()
}
