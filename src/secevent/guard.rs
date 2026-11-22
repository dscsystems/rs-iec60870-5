// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

use std::collections::HashMap;
use std::net::IpAddr;
#[cfg(feature = "cs104")]
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::asdu::{Asdu, Cause, CommonAddr, InfoObjAddr, TypeId};
use crate::netpeer::{Change, Neighbor};

use super::{Action, Event, Kind, PeerInfo, Policy, Rate, Security};

pub(crate) struct Bucket {
    rate: Rate,
    available: f64,
    last: Instant,
}

impl Bucket {
    pub(crate) fn new(rate: Rate, now: Instant) -> Self {
        Self {
            rate,
            available: rate.count.into(),
            last: now,
        }
    }

    pub(crate) fn allow(&mut self, rate: Rate, now: Instant) -> bool {
        if rate.count == 0 || rate.per.is_zero() {
            return true;
        }
        if self.rate.per != rate.per || self.rate.count != rate.count {
            *self = Self::new(rate, now);
        }
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        let capacity = f64::from(rate.count);
        self.available =
            (self.available + elapsed * capacity / rate.per.as_secs_f64()).min(capacity);
        self.last = now;
        if self.available < 1.0 {
            return false;
        }
        self.available -= 1.0;
        true
    }
}

const HISTORY_LIMIT: usize = 4096;

type Command = (TypeId, CommonAddr, Cause, Vec<u8>);

#[derive(Default)]
pub(crate) struct Guard {
    #[cfg(feature = "cs104")]
    sessions: HashMap<IpAddr, usize>,
    #[cfg(feature = "cs104")]
    count: usize,
    #[cfg(feature = "cs104")]
    accepts: HashMap<IpAddr, Bucket>,
    #[cfg(feature = "cs104")]
    global: Option<Bucket>,
    commands: HashMap<Command, Instant>,
}

#[cfg(feature = "cs104")]
pub(crate) struct Admission {
    security: Arc<Security>,
    ip: IpAddr,
}

#[cfg(feature = "cs104")]
impl Drop for Admission {
    fn drop(&mut self) {
        let mut guard = self.security.guard.lock().unwrap();
        guard.count -= 1;
        if let Some(count) = guard.sessions.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                guard.sessions.remove(&self.ip);
            }
        }
    }
}

impl Security {
    /// Check the inexpensive admission rules before making any TLS or MAC queries.
    #[cfg(feature = "cs104")]
    pub(crate) fn admit(
        self: &Arc<Self>,
        peer: SocketAddr,
        policy: &Policy,
        listener_limit: Option<usize>,
        report: impl Fn(Kind, Action, &str),
    ) -> Option<Admission> {
        let ip = peer.ip().to_canonical();
        if !policy.allowed_peers.is_empty()
            && !policy.allowed_peers.iter().any(|net| net.contains(&ip))
        {
            let action = if policy.reject_unlisted {
                Action::Rejected
            } else {
                Action::Observed
            };
            report(
                Kind::ConnUnexpected,
                action,
                "peer address is outside the configured networks",
            );
            if policy.reject_unlisted {
                return None;
            }
        }
        let check_rate = self.enabled() || policy.flood_action() != Action::Observed;
        let mut guard = self.guard.lock().unwrap();
        if listener_limit.is_some_and(|limit| guard.count >= limit)
            || (policy.max_sessions != 0 && guard.count >= policy.max_sessions)
            || (policy.max_sessions_per_ip != 0
                && guard.sessions.get(&ip).copied().unwrap_or(0) >= policy.max_sessions_per_ip)
        {
            report(
                Kind::ConnRejectedLimit,
                Action::Rejected,
                "concurrent session limit reached",
            );
            return None;
        }
        if check_rate {
            let now = Instant::now();
            let global_ok = guard
                .global
                .get_or_insert_with(|| Bucket::new(policy.global_accept_rate, now))
                .allow(policy.global_accept_rate, now);
            if !guard.accepts.contains_key(&ip)
                && guard.accepts.len() >= HISTORY_LIMIT
                && let Some(oldest) = guard
                    .accepts
                    .iter()
                    .min_by_key(|(_, bucket)| bucket.last)
                    .map(|(ip, _)| *ip)
            {
                guard.accepts.remove(&oldest);
            }
            let ip_ok = guard
                .accepts
                .entry(ip)
                .or_insert_with(|| Bucket::new(policy.accept_rate, now))
                .allow(policy.accept_rate, now);
            if !global_ok || !ip_ok {
                let action = policy.flood_action();
                report(Kind::ConnRate, action, "connection rate limit exceeded");
                if action != Action::Observed {
                    return None;
                }
            }
        }
        guard.count += 1;
        *guard.sessions.entry(ip).or_default() += 1;
        Some(Admission {
            security: self.clone(),
            ip,
        })
    }

    pub(crate) fn seen_command(&self, a: &Asdu, window: Duration) -> bool {
        let now = Instant::now();
        let mut guard = self.guard.lock().unwrap();
        guard
            .commands
            .retain(|_, last| now.duration_since(*last) < window);
        let key = (
            a.type_id(),
            a.common_addr(),
            a.coa().cause,
            a.info_obj.clone(),
        );
        if !guard.commands.contains_key(&key)
            && guard.commands.len() >= HISTORY_LIMIT
            && let Some(oldest) = guard
                .commands
                .iter()
                .min_by_key(|(_, seen)| *seen)
                .map(|(key, _)| key.clone())
        {
            guard.commands.remove(&oldest);
        }
        guard.commands.insert(key, now).is_some()
    }
}

async fn resolve_mac(policy: &Policy, ip: IpAddr) -> Neighbor {
    // Timed out blocking lookups may still be running. Bound those as well.
    static LOOKUPS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(32);
    let resolver = policy.resolver.clone();
    let result = tokio::time::timeout(policy.mac_lookup_timeout, async move {
        let permit = LOOKUPS.acquire().await.ok()?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            resolver.resolve(ip.to_canonical())
        })
        .await
        .ok()
    })
    .await;
    result.ok().flatten().unwrap_or_default()
}

#[derive(Clone)]
pub(crate) struct Session {
    pub(crate) security: Arc<Security>,
    pub(crate) policy: Arc<Policy>,
    pub(crate) id: u64,
    pub(crate) protocol: &'static str,
    pub(crate) role: &'static str,
    pub(crate) endpoint: usize,
    peer: Arc<std::sync::RwLock<PeerInfo>>,
}

impl Session {
    pub(crate) fn new(
        security: Arc<Security>,
        protocol: &'static str,
        role: &'static str,
        endpoint: usize,
        peer: PeerInfo,
    ) -> Self {
        let policy = security.policy();
        Self {
            security,
            policy,
            id: super::next_session_id(),
            protocol,
            role,
            endpoint,
            peer: Arc::new(std::sync::RwLock::new(peer)),
        }
    }

    pub(crate) fn peer(&self) -> PeerInfo {
        self.peer.read().unwrap().clone()
    }

    #[cfg(feature = "cs104")]
    pub(crate) fn set_transport(&self, mut transport: PeerInfo) {
        let mut peer = self.peer.write().unwrap();
        transport.mac = peer.mac;
        transport.mac_source = peer.mac_source;
        transport.on_link = peer.on_link;
        transport.interface = peer.interface.take();
        *peer = transport;
    }

    pub(crate) fn observe_tls(&self) {
        if !self.security.enabled() {
            return;
        }
        let peer = self.peer();
        let Some(tls) = peer.tls else { return };
        if let Some(expiry) = tls.not_after
            && !self.policy.cert_expiry_warning.is_zero()
            && (expiry - chrono::Utc::now())
                .to_std()
                .map_or(true, |left| left <= self.policy.cert_expiry_warning)
        {
            self.note(
                Kind::TlsCertExpiring,
                Action::Observed,
                format!("peer certificate expires at {expiry}"),
            );
        }
        if matches!(tls.version.as_str(), "TLSv1_0" | "TLSv1_1") {
            self.note(
                Kind::TlsWeakParams,
                Action::Observed,
                "negotiated TLS version is below 1.2",
            );
        }
        if let (Some(addr), Some(fingerprint)) = (peer.addr, tls.fingerprint_sha256) {
            let ip = addr.ip().to_canonical();
            if self
                .policy
                .peer_cache
                .observe_certificate(ip, fingerprint)
                .is_some()
            {
                self.note(
                    Kind::TlsCertChanged,
                    Action::Observed,
                    "peer certificate fingerprint changed",
                );
            }
        }
    }

    pub(crate) fn emit(&self, event: impl FnOnce() -> Event) {
        self.security.emit(|| {
            let mut event = event();
            event.protocol = self.protocol;
            event.role = self.role;
            event.endpoint = self.endpoint;
            event.session_id = Some(self.id);
            if event.peer.addr.is_none() {
                event.peer = self.peer();
            }
            event
        });
    }

    #[cfg(feature = "cs101")]
    pub(crate) fn link_event(
        &self,
        kind: Kind,
        action: Action,
        detail: &str,
        link_addr: Option<u16>,
    ) {
        self.emit(|| {
            let mut event = Event::new(kind, action, detail);
            event.link_addr = link_addr;
            event
        });
    }

    pub(crate) fn invalid_asdu(&self, error: &crate::Error, raw: &[u8], link_addr: Option<u16>) {
        self.emit(|| {
            let mut event = Event::new(Kind::ProtoAsduInvalid, Action::Ignored, error.to_string());
            event.raw = raw.to_vec();
            event.link_addr = link_addr;
            event
        });
    }

    pub(crate) fn note(&self, kind: Kind, action: Action, detail: impl Into<String>) {
        self.emit(|| Event::new(kind, action, detail));
    }

    pub(crate) async fn identify(&self) -> bool {
        let Some(ip) = self.peer().addr.map(|addr| addr.ip().to_canonical()) else {
            return true;
        };
        let binding = self.policy.peer_bindings.get(&ip).copied();
        if !self.security.enabled()
            && binding.is_none()
            && self.policy.mac_recheck_interval.is_zero()
        {
            return true;
        }
        let neighbor = resolve_mac(&self.policy, ip).await;
        let mac = neighbor.mac;
        {
            let mut peer = self.peer.write().unwrap();
            peer.mac = mac;
            peer.mac_source = neighbor.source;
            peer.on_link = neighbor.on_link;
            peer.interface = neighbor.interface;
        }
        if let Some(mac) = mac {
            for change in self.policy.peer_cache.observe(ip, mac) {
                match change {
                    Change::MacChanged { previous } => self.note(
                        Kind::PeerMacChanged,
                        Action::Observed,
                        format!("peer MAC changed from {previous} to {mac}"),
                    ),
                    Change::IpChanged { previous } => self.note(
                        Kind::PeerIpChanged,
                        Action::Observed,
                        format!("MAC {mac} was previously associated with {previous}"),
                    ),
                    Change::MacShared { other } => self.note(
                        Kind::PeerMacShared,
                        Action::Observed,
                        format!("MAC {mac} is also associated with {other}"),
                    ),
                }
            }
        } else if neighbor.on_link
            && !neighbor.loopback
            && self.security.enabled()
            && self.policy.peer_cache.first_missing(ip)
        {
            self.note(
                Kind::PeerMacUnavailable,
                Action::Observed,
                "no MAC could be learned for the on-link peer",
            );
        }
        if binding.is_some() && binding != mac {
            let action = if self.policy.reject_binding_mismatch {
                Action::Rejected
            } else {
                Action::Observed
            };
            self.note(
                Kind::ConnUnexpectedMac,
                action,
                "peer MAC does not match its configured binding",
            );
            self.note(
                Kind::PeerMacPinning,
                action,
                "peer MAC does not match its configured binding",
            );
            return !self.policy.reject_binding_mismatch;
        }
        true
    }

    pub(crate) async fn watch_mac(&self) {
        let Some(ip) = self.peer().addr.map(|addr| addr.ip().to_canonical()) else {
            std::future::pending::<()>().await;
            return;
        };
        if self.policy.mac_recheck_interval.is_zero() {
            std::future::pending::<()>().await;
            return;
        }
        loop {
            tokio::time::sleep(self.policy.mac_recheck_interval).await;
            let neighbor = resolve_mac(&self.policy, ip).await;
            let Some(mac) = neighbor.mac else { continue };
            let previous = {
                let mut peer = self.peer.write().unwrap();
                peer.mac_source = neighbor.source;
                peer.on_link = neighbor.on_link;
                peer.interface = neighbor.interface;
                peer.mac.replace(mac)
            };
            if previous.is_some_and(|previous| previous != mac) {
                self.policy.peer_cache.observe(ip, mac);
                let disconnect = self.policy.disconnect_on_mac_change;
                let action = if disconnect {
                    Action::Disconnected
                } else {
                    Action::Observed
                };
                self.note(
                    Kind::PeerMacChangedLive,
                    action,
                    "peer MAC changed during this session",
                );
                if disconnect {
                    return;
                }
            }
        }
    }
}

pub(crate) type Buckets = HashMap<Kind, Bucket>;

pub(crate) fn over_limit(buckets: &mut Buckets, kind: Kind, rate: Rate) -> bool {
    let now = Instant::now();
    !buckets
        .entry(kind)
        .or_insert_with(|| Bucket::new(rate, now))
        .allow(rate, now)
}

pub(crate) type UnknownObjects = HashMap<(TypeId, CommonAddr, Option<InfoObjAddr>), Instant>;
