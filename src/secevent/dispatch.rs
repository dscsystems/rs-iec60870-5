// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant};

use super::{Action, Event, Kind, Sink};
use crate::asdu::{CommonAddr, InfoObjAddr, TypeId};

/// Bounds for queued, sampled and aggregated events.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Number of events the producer may queue.
    pub queue_size: usize,
    /// Maximum raw sample length, in octets.
    pub max_raw: usize,
    /// How long to aggregate repetitions. Zero disables aggregation.
    pub aggregate_window: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            queue_size: 1024,
            max_raw: 64,
            aggregate_window: Duration::from_secs(1),
        }
    }
}

/// Dispatcher counters since creation.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Events successfully queued.
    pub queued: u64,
    /// Events dropped due to a full queue.
    pub dropped: u64,
    /// Observations merged into an aggregate.
    pub aggregated: u64,
    /// Calls to the sink.
    pub delivered: u64,
    /// Panics recovered from the sink.
    pub sink_panics: u64,
}

#[derive(Default)]
struct Counters {
    queued: AtomicU64,
    dropped: AtomicU64,
    delivered: AtomicU64,
    aggregated: AtomicU64,
    sink_panics: AtomicU64,
}

#[derive(PartialEq, Eq, Hash)]
struct Key {
    kind: Kind,
    action: Action,
    session_id: Option<u64>,
    addr: Option<std::net::SocketAddr>,
    protocol: &'static str,
    common_addr: Option<CommonAddr>,
    ioa: Option<InfoObjAddr>,
    type_id: Option<TypeId>,
    link_addr: Option<u16>,
}

struct Aggregate {
    deadline: Instant,
    first_seen: chrono::DateTime<chrono::Utc>,
    repeats: Option<Event>,
}

/// Bounded, nonblocking event producer. Clones share the queue and worker.
///
/// The worker flushes buffered events when the last producer is dropped.
#[derive(Clone)]
pub struct Dispatcher {
    tx: SyncSender<Event>,
    counters: Arc<Counters>,
    max_raw: usize,
}

impl Dispatcher {
    /// Create a dispatcher and start its sink worker.
    pub fn new(sink: impl Sink, options: Options) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::sync_channel(options.queue_size.max(1));
        let counters = Arc::new(Counters::default());
        let worker = Arc::clone(&counters);
        std::thread::Builder::new()
            .name("iec60870-security".into())
            .spawn(move || run(rx, &worker, &sink, options))?;
        Ok(Self {
            tx,
            counters,
            max_raw: options.max_raw,
        })
    }

    /// Queue an event. Returns false if the queue has no room.
    pub fn emit(&self, mut event: Event) -> bool {
        event.raw.truncate(self.max_raw);
        let queued = self.tx.try_send(event).is_ok();
        if queued {
            self.counters.queued.fetch_add(1, Relaxed);
        } else {
            self.counters.dropped.fetch_add(1, Relaxed);
        }
        queued
    }

    /// Read statistics without draining events.
    pub fn stats(&self) -> Stats {
        Stats {
            queued: self.counters.queued.load(Relaxed),
            dropped: self.counters.dropped.load(Relaxed),
            delivered: self.counters.delivered.load(Relaxed),
            aggregated: self.counters.aggregated.load(Relaxed),
            sink_panics: self.counters.sink_panics.load(Relaxed),
        }
    }
}

fn run(rx: mpsc::Receiver<Event>, counters: &Counters, sink: &dyn Sink, options: Options) {
    let mut pending: HashMap<Key, Aggregate> = HashMap::new();
    let mut drops_reported = 0;
    let tick = Duration::from_millis(50);
    let mut next_tick = Instant::now() + tick;
    loop {
        let event = rx.recv_timeout(next_tick.saturating_duration_since(Instant::now()));
        let closing = matches!(event, Err(RecvTimeoutError::Disconnected));
        let now = Instant::now();
        if now >= next_tick || closing {
            next_tick = now + tick;
            pending.retain(|_, entry| {
                if !closing && now < entry.deadline {
                    return true;
                }
                if let Some(event) = entry.repeats.take() {
                    deliver(counters, sink, event);
                }
                false
            });
            let dropped = counters.dropped.load(Relaxed);
            if dropped > drops_reported {
                let mut event = Event::new(
                    Kind::MetaEventsDropped,
                    Action::Observed,
                    "security event queue full",
                );
                event.count = dropped - drops_reported;
                drops_reported = dropped;
                deliver(counters, sink, event);
            }
        }
        let mut event = match event {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if options.aggregate_window.is_zero() || event.kind.info().always {
            deliver(counters, sink, event);
            continue;
        }
        let key = Key {
            kind: event.kind,
            action: event.action,
            session_id: event.session_id,
            addr: event.peer.addr,
            protocol: event.protocol,
            common_addr: event.common_addr,
            ioa: event.ioa,
            type_id: event.type_id,
            link_addr: event.link_addr,
        };
        if let Some(entry) = pending.get_mut(&key) {
            if now < entry.deadline {
                counters.aggregated.fetch_add(event.count, Relaxed);
                event.count = event
                    .count
                    .saturating_add(entry.repeats.as_ref().map_or(0, |event| event.count));
                event.first_seen = Some(entry.first_seen);
                entry.repeats = Some(event);
                continue;
            }
            if let Some(event) = entry.repeats.take() {
                deliver(counters, sink, event);
            }
            *entry = Aggregate {
                deadline: now + options.aggregate_window,
                first_seen: event.time,
                repeats: None,
            };
        } else if pending.len() < options.queue_size.max(1) {
            pending.insert(
                key,
                Aggregate {
                    deadline: now + options.aggregate_window,
                    first_seen: event.time,
                    repeats: None,
                },
            );
        }
        deliver(counters, sink, event);
    }
}

fn deliver(counters: &Counters, sink: &dyn Sink, event: Event) {
    counters.delivered.fetch_add(1, Relaxed);
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink.handle(event))).is_err() {
        let earlier = counters.sink_panics.fetch_add(1, Relaxed);
        if earlier == 0 {
            let event = Event::new(
                Kind::MetaSinkPanic,
                Action::Observed,
                "security event sink panicked",
            );
            deliver(counters, sink, event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_first_occurrence_then_aggregates_repeats() {
        let (out, recv) = mpsc::channel();
        let dispatcher = Dispatcher::new(
            move |event| out.send(event).unwrap(),
            Options {
                aggregate_window: Duration::from_secs(5),
                max_raw: 2,
                ..Options::default()
            },
        )
        .unwrap();
        let mut event = Event::new(Kind::ProtoGarbage, Action::Ignored, "");
        event.raw = vec![1, 2, 3];
        assert!(dispatcher.emit(event.clone()));
        let first = recv.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(first.count, 1);
        assert_eq!(first.raw, [1, 2]);
        assert!(dispatcher.emit(event.clone()));
        assert!(dispatcher.emit(event));
        drop(dispatcher); // Closing the last producer flushes the repeats immediately.
        let summary = recv.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(summary.count, 2);
        assert_eq!(summary.first_seen, Some(first.time));
    }

    #[test]
    fn a_busy_sink_does_not_block_the_producer() {
        let (out, recv) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let wait = std::sync::Mutex::new(wait);
        let (entered, started) = mpsc::sync_channel(1);
        let dispatcher = Dispatcher::new(
            move |event: Event| {
                if event.detail == "wait" {
                    entered.send(()).unwrap();
                    wait.lock().unwrap().recv().unwrap();
                }
                let _ = out.send(event);
            },
            Options {
                queue_size: 1,
                aggregate_window: Duration::ZERO,
                ..Options::default()
            },
        )
        .unwrap();

        let event = |detail| Event::new(Kind::ProtoGarbage, Action::Ignored, detail);
        assert!(dispatcher.emit(event("wait")));
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(dispatcher.emit(event("queued")));
        assert!(!dispatcher.emit(event("too many")));
        assert_eq!(dispatcher.stats().dropped, 1);
        release.send(()).unwrap();
        let events: Vec<_> = (0..3)
            .map(|_| recv.recv_timeout(Duration::from_secs(2)).unwrap())
            .collect();
        assert!(
            events
                .iter()
                .any(|event| event.kind == Kind::MetaEventsDropped && event.count == 1)
        );
    }
}
