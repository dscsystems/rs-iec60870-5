// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! `cs104-explorer` is an interactive terminal IEC 60870-5-104 master.
//!
//! It connects to controlled stations, issues requests (general and counter
//! interrogation, clock synchronization, test, reset), builds and sends control
//! commands, and shows the received monitored points beside a live protocol
//! log.
//!
//! ```sh
//! cargo run                      # start; set the target and connect in the UI
//! cargo run -- 10.0.0.5:2404     # preset the server address
//! ```
//!
//! Try it against the library's own outstation:
//!
//! ```sh
//! cargo run --example cs104_server        # in the repository root
//! cargo run -- 127.0.0.1:2404             # here
//! ```
//!
//! The keys are listed in the footer of the running program.

mod app;
#[cfg(test)]
mod e2e;
mod event;
mod form;
mod handler;
mod ui;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{Event as TermEvent, EventStream, KeyEventKind};
use futures::StreamExt;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

use crate::app::App;
use crate::event::{Event, Sender};

/// Routes the library's `tracing` output into the log panel.
///
/// Writing to stdout would corrupt the alternate screen, so nothing may be
/// printed; the 'v' key toggles `enabled`, mirroring go-iecp5's `LogMode`.
struct LogLayer {
    tx: Sender,
    enabled: Arc<AtomicBool>,
}

/// Collects a tracing event's fields into one line.
#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: Vec<String>,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
            // Debug-formatting a &str quotes it; the log reads better without.
            if self.message.starts_with('"') && self.message.ends_with('"') {
                self.message = self.message[1..self.message.len() - 1].to_string();
            }
        } else {
            self.fields.push(format!("{}={value:?}", field.name()));
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        let level = match *event.metadata().level() {
            tracing::Level::ERROR => "[E]",
            tracing::Level::WARN => "[W]",
            tracing::Level::INFO => "[I]",
            _ => "[D]",
        };
        let mut line = format!("{level} {}", visitor.message);
        if !visitor.fields.is_empty() {
            line.push(' ');
            line.push_str(&visitor.fields.join(" "));
        }
        let _ = self.tx.send(Event::Log(line));
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let (tx, mut rx) = event::channel();
    let verbose = Arc::new(AtomicBool::new(false));

    // Install the log bridge before anything protocol-related starts, so no
    // library output can ever reach the terminal directly.
    tracing_subscriber::registry()
        .with(LogLayer {
            tx: tx.clone(),
            enabled: Arc::clone(&verbose),
        })
        .init();

    let mut app = App::new(tx, verbose, std::env::args().nth(1));
    app.log("[app] ready — press 'e' to set the target, 'c' to connect, '?' is the footer");

    let mut terminal = ratatui::init();
    let mut keys = EventStream::new();

    let result = loop {
        if let Err(e) = terminal.draw(|frame| ui::draw(frame, &app)) {
            break Err(e);
        }
        if app.should_quit {
            break Ok(());
        }

        tokio::select! {
            // Messages from the protocol tasks: logs, points, status changes.
            Some(msg) = rx.recv() => {
                app.apply(msg);
                // Drain whatever else is queued before redrawing, so a burst
                // of ASDUs costs one frame rather than one frame each.
                while let Ok(msg) = rx.try_recv() {
                    app.apply(msg);
                }
            }

            // Terminal input.
            maybe = keys.next() => {
                match maybe {
                    Some(Ok(TermEvent::Key(key))) if key.kind == KeyEventKind::Press => {
                        app.key(key);
                    }
                    // Resize redraws on the next loop; other events are ignored.
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break Err(e),
                    None => break Ok(()),
                }
            }
        }
    };

    ratatui::restore();

    // Stop the background client before leaving.
    if let Some(client) = app.client.take() {
        client.close();
    }
    result
}
