// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Structured protocol security events and opt-in endpoint policies.

mod dispatch;
#[cfg(any(feature = "cs101", feature = "cs104"))]
mod guard;
#[cfg(any(feature = "cs101", feature = "cs104"))]
pub(crate) use guard::{Buckets, Session, UnknownObjects, over_limit};
#[cfg(any(feature = "cs101", feature = "cs104"))]
mod inspect;
#[cfg(feature = "cs104")]
pub(crate) use inspect::Requests;
#[cfg(any(feature = "cs101", feature = "cs104"))]
pub(crate) use inspect::{Decision, Inspector};
mod kind;
mod policy;

pub use dispatch::{Dispatcher, Options, Stats};
pub use kind::{Category, Kind, KindInfo, Severity};
pub use policy::{AddressSpace, FunctionPolicy, Limits, Policy, Rate};

use chrono::{DateTime, Utc};
use std::net::SocketAddr;
use std::sync::Arc;
#[cfg(any(feature = "cs101", feature = "cs104"))]
use std::sync::atomic::{AtomicU64, Ordering};

use crate::asdu::{Asdu, Cause, CommonAddr, InfoObjAddr, TypeId};

/// What the endpoint did with the activity that raised an event.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(rename_all = "snake_case")
)]
pub enum Action {
    /// Reported the activity.
    #[default]
    Observed,
    /// Discarded the offending data.
    Ignored,
    /// Refused the request.
    Rejected,
    /// Refused the request due to a rate limit.
    Throttled,
    /// Closed the connection.
    Disconnected,
}

/// Transport identity known to the endpoint.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PeerInfo {
    /// Transport family for this peer.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "str::is_empty"))]
    pub network: &'static str,
    /// TCP peer address, when available.
    #[cfg_attr(
        feature = "serde",
        serde(rename = "remote", skip_serializing_if = "Option::is_none")
    )]
    pub addr: Option<SocketAddr>,
    /// Link-layer address, when resolved.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub mac: Option<crate::netpeer::MacAddr>,
    /// Where the MAC was learned.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub mac_source: Option<crate::netpeer::Source>,
    /// Whether the transport peer is directly reachable.
    pub on_link: bool,
    /// Network interface used for the MAC lookup.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub interface: Option<String>,
    /// TCP local address, when available.
    #[cfg_attr(
        feature = "serde",
        serde(rename = "local", skip_serializing_if = "Option::is_none")
    )]
    pub local_addr: Option<SocketAddr>,
    /// Time the transport became available.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub connected_at: Option<DateTime<Utc>>,
    /// Serial port, when this is a local serial transport.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub serial_port: Option<String>,
    /// Negotiated TLS and peer certificate summary.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub tls: Option<TlsPeer>,
}

/// TLS state and public information from the peer certificate.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct TlsPeer {
    /// TLS protocol version.
    pub version: String,
    /// Negotiated cipher suite.
    pub cipher_suite: String,
    /// Peer certificate subject.
    pub subject: Option<String>,
    /// Peer certificate issuer.
    pub issuer: Option<String>,
    /// Peer certificate serial number.
    pub serial: Option<String>,
    /// Peer certificate expiry.
    pub not_after: Option<chrono::DateTime<chrono::Utc>>,
    /// SHA-256 fingerprint of the peer leaf certificate.
    pub fingerprint_sha256: Option<String>,
}

/// A security observation from a transport, protocol or application handler.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Event {
    /// When the activity was observed.
    pub time: DateTime<Utc>,
    /// Time of the first observation in an aggregate.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub first_seen: Option<DateTime<Utc>>,
    /// Stable event identifier shared with go-iecp5.
    pub kind: Kind,
    /// Event category.
    pub category: Category,
    /// Event severity.
    pub severity: Severity,
    /// Action taken by the endpoint.
    pub action: Action,
    /// MITRE ATT&CK for ICS technique hint, when applicable.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub technique: Option<&'static str>,
    /// Protocol name when known.
    pub protocol: &'static str,
    /// Role of the local endpoint.
    pub role: &'static str,
    /// Configured client or server index.
    pub endpoint: usize,
    /// Process-local session identity.
    #[cfg_attr(
        feature = "serde",
        serde(rename = "session", skip_serializing_if = "Option::is_none")
    )]
    pub session_id: Option<u64>,
    /// Remote peer identity.
    pub peer: PeerInfo,
    /// Number of observations represented by this event.
    pub count: u64,
    /// Human-readable context.
    pub detail: String,
    /// Decoded command value or qualifier, if available.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub value: Option<String>,
    /// Frame sample, truncated by the dispatcher.
    #[cfg_attr(
        feature = "serde",
        serde(
            serialize_with = "serialize_raw",
            skip_serializing_if = "Vec::is_empty"
        )
    )]
    pub raw: Vec<u8>,
    /// FT1.2 link address if present.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub link_addr: Option<u16>,
    /// ASDU type identification.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub type_id: Option<TypeId>,
    /// Cause of transmission.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub cause: Option<Cause>,
    /// Originator address when present.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub originator: Option<u8>,
    /// The cause of transmission test flag.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub test: Option<bool>,
    /// The cause of transmission negative flag.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub negative: Option<bool>,
    /// ASDU common address.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub common_addr: Option<CommonAddr>,
    /// Information object address.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub ioa: Option<InfoObjAddr>,
    /// APCI receive sequence.
    #[cfg_attr(
        feature = "serde",
        serde(rename = "recv_sn", skip_serializing_if = "Option::is_none")
    )]
    pub nr: Option<u16>,
    /// APCI send sequence.
    #[cfg_attr(
        feature = "serde",
        serde(rename = "send_sn", skip_serializing_if = "Option::is_none")
    )]
    pub ns: Option<u16>,
    /// Expected APCI sequence.
    #[cfg_attr(
        feature = "serde",
        serde(rename = "expected_sn", skip_serializing_if = "Option::is_none")
    )]
    pub expected: Option<u16>,
}

impl Event {
    /// Build an observation using the kind's standard event metadata.
    pub fn new(kind: Kind, action: Action, detail: impl Into<String>) -> Self {
        let info = kind.info();
        Self {
            time: Utc::now(),
            first_seen: None,
            kind,
            category: info.category,
            severity: info.severity,
            action,
            technique: info.technique,
            protocol: "",
            role: "",
            endpoint: 0,
            session_id: None,
            peer: PeerInfo::default(),
            count: 1,
            detail: detail.into(),
            value: None,
            raw: Vec::new(),
            link_addr: None,
            type_id: None,
            cause: None,
            originator: None,
            test: None,
            negative: None,
            common_addr: None,
            ioa: None,
            nr: None,
            ns: None,
            expected: None,
        }
    }

    /// Attach the decoded ASDU address and cause.
    pub fn with_asdu(mut self, a: &Asdu) -> Self {
        self.type_id = Some(a.type_id());
        self.cause = Some(a.coa().cause);
        self.originator = Some(a.orig_addr());
        self.test = Some(a.coa().is_test);
        self.negative = Some(a.coa().is_negative);
        self.common_addr = Some(a.common_addr());
        self.ioa = a.reader().info_obj_addr().ok();
        self
    }
}

#[cfg(feature = "serde")]
fn serialize_raw<S: serde::Serializer>(raw: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    use std::fmt::Write;
    let mut hex = String::with_capacity(raw.len() * 2);
    for byte in raw {
        let _ = write!(&mut hex, "{byte:02x}");
    }
    serializer.serialize_str(&hex)
}

/// Receives events on the dispatcher's worker.
pub trait Sink: Send + Sync + 'static {
    /// Handle one event.
    fn handle(&self, event: Event);
}

impl<F: Fn(Event) + Send + Sync + 'static> Sink for F {
    fn handle(&self, event: Event) {
        self(event);
    }
}

/// Send events to multiple sinks.
pub fn multi_sink(sinks: Vec<Arc<dyn Sink>>) -> impl Sink {
    move |event: Event| {
        for sink in &sinks {
            sink.handle(event.clone());
        }
    }
}

/// Forward events meeting a minimum severity and optional category filter.
pub fn filter_sink(min: Severity, categories: Vec<Category>, sink: impl Sink) -> impl Sink {
    move |event: Event| {
        if event.severity >= min && (categories.is_empty() || categories.contains(&event.category))
        {
            sink.handle(event);
        }
    }
}

/// Write one JSON record per event.
#[cfg(feature = "serde")]
pub fn json_lines_sink(writer: impl std::io::Write + Send + 'static) -> impl Sink {
    let writer = std::sync::Mutex::new(writer);
    move |event: Event| {
        let mut writer = writer.lock().unwrap();
        if let Ok(line) = serde_json::to_string(&event) {
            let _ = writeln!(writer, "{line}");
        }
    }
}

/// Send an event using the crate's existing tracing integration.
pub fn tracing_sink(event: Event) {
    tracing::info!(target: "iec60870.security", kind = event.kind.as_str(), ?event.severity,
        ?event.action, ?event.peer, event.count, detail = event.detail);
}

/// Deliver an event to a bounded channel, dropping it when the channel is full.
pub fn channel_sink(sender: tokio::sync::mpsc::Sender<Event>) -> impl Sink {
    move |event| {
        let _ = sender.try_send(event);
    }
}

/// Report an event through an application connection.
pub fn report(connection: &dyn crate::asdu::Connect, event: Event) {
    connection.report_security_event(event);
}

/// Get the identity attached to an application connection.
pub fn peer_of(connection: &dyn crate::asdu::Connect) -> PeerInfo {
    connection.peer_info()
}

#[cfg(any(feature = "cs101", feature = "cs104"))]
#[derive(Default)]
pub(crate) struct Security {
    enabled: std::sync::atomic::AtomicBool,
    guard: std::sync::Mutex<guard::Guard>,
    dispatcher: std::sync::RwLock<Option<Dispatcher>>,
    policy: std::sync::RwLock<Arc<Policy>>,
}

#[cfg(any(feature = "cs101", feature = "cs104"))]
impl Security {
    pub(crate) fn set_dispatcher(&self, dispatcher: Option<Dispatcher>) {
        let mut active = self.dispatcher.write().unwrap();
        *active = dispatcher;
        self.enabled.store(active.is_some(), Ordering::Release);
    }

    pub(crate) fn set_policy(&self, policy: Policy) {
        *self.policy.write().unwrap() = Arc::new(policy);
    }

    pub(crate) fn policy(&self) -> Arc<Policy> {
        self.policy.read().unwrap().clone()
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub(crate) fn stats(&self) -> Stats {
        self.dispatcher
            .read()
            .unwrap()
            .as_ref()
            .map_or_else(Stats::default, Dispatcher::stats)
    }

    pub(crate) fn emit(&self, build: impl FnOnce() -> Event) {
        if self.enabled()
            && let Some(dispatcher) = self.dispatcher.read().unwrap().as_ref()
        {
            dispatcher.emit(build());
        }
    }
}

#[cfg(any(feature = "cs101", feature = "cs104"))]
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(any(feature = "cs101", feature = "cs104"))]
pub(crate) fn next_session_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

// Endpoint settings are shared by the transport and the application-facing handle.
#[cfg(any(feature = "cs101", feature = "cs104"))]
macro_rules! endpoint_security {
    () => {
        /// Route security events to a shared dispatcher. Pass None to disable delivery.
        pub fn set_security_dispatcher(&self, dispatcher: Option<crate::secevent::Dispatcher>) {
            self.security.set_dispatcher(dispatcher);
        }

        /// Send security events to a sink on a bounded dispatcher worker.
        pub fn set_security_sink(&self, sink: impl crate::secevent::Sink) -> std::io::Result<()> {
            self.set_security_dispatcher(Some(crate::secevent::Dispatcher::new(
                sink,
                Default::default(),
            )?));
            Ok(())
        }

        /// Apply security settings to subsequent connections.
        pub fn set_security_policy(&self, policy: crate::secevent::Policy) {
            self.security.set_policy(policy);
        }

        /// Read the active dispatcher's counters.
        pub fn security_stats(&self) -> crate::secevent::Stats {
            self.security.stats()
        }
    };
}

#[cfg(any(feature = "cs101", feature = "cs104"))]
pub(crate) use endpoint_security;
