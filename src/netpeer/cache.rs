// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};

use super::MacAddr;

/// Size and time bounds for peer history.
#[derive(Debug, Clone)]
pub struct CacheOptions {
    /// Maximum remembered IP and MAC entries.
    pub max_entries: usize,
    /// Time an idle entry is retained.
    pub ttl: Duration,
    /// Time during which two IPs observed with the same MAC count as concurrent.
    pub active_window: Duration,
}

impl Default for CacheOptions {
    fn default() -> Self {
        Self {
            max_entries: 4096,
            ttl: Duration::from_secs(30 * 24 * 3600),
            active_window: Duration::from_secs(10 * 60),
        }
    }
}

/// Change learned from observing the same peer or identity again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// IP presented a different MAC.
    MacChanged {
        /// Previous MAC at this address.
        previous: MacAddr,
    },
    /// A MAC is now associated with another address.
    IpChanged {
        /// Previously associated address.
        previous: IpAddr,
    },
    /// Two recently observed IP addresses use the same MAC.
    MacShared {
        /// The other IP address.
        other: IpAddr,
    },
}

#[derive(Default)]
struct State {
    ips: HashMap<IpAddr, Record>,
    macs: HashMap<MacAddr, (IpAddr, DateTime<Utc>)>,
}

#[derive(Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct Record {
    mac: Option<MacAddr>,
    #[cfg(feature = "serde")]
    first: DateTime<Utc>,
    last: DateTime<Utc>,
    cert_fp: Option<String>,
    #[cfg_attr(feature = "serde", serde(skip))]
    missing_reported: bool,
}

/// Bounded IP, MAC and certificate history, safe to share across endpoints.
pub struct Cache {
    options: CacheOptions,
    state: Mutex<State>,
}

impl Default for Cache {
    fn default() -> Self {
        Self::new(CacheOptions::default())
    }
}

impl Cache {
    /// Construct an empty cache.
    pub fn new(options: CacheOptions) -> Self {
        Self {
            options,
            state: Mutex::new(State::default()),
        }
    }

    /// Record an observation and return any newly observed changes.
    pub fn observe(&self, ip: IpAddr, mac: MacAddr) -> Vec<Change> {
        let ip = ip.to_canonical();
        let now = Utc::now();
        let mut state = self.state.lock().unwrap();
        let record = peer_record(&mut state, &self.options, ip, now);
        let previous = record.mac.replace(mac);
        let mut changes = Vec::new();
        if let Some(previous) = previous.filter(|previous| *previous != mac) {
            changes.push(Change::MacChanged { previous });
        }
        if let Some(&(other, seen)) = state
            .macs
            .get(&mac)
            .filter(|(other, seen)| *other != ip && within(now, *seen, self.options.ttl))
        {
            changes.push(if within(now, seen, self.options.active_window) {
                Change::MacShared { other }
            } else {
                Change::IpChanged { previous: other }
            });
        }
        make_room(&mut state.macs, mac, self.options.max_entries, |entry| {
            entry.1
        });
        state.macs.insert(mac, (ip, now));
        changes
    }

    /// Record a fingerprint, returning the previous value if it changed.
    pub fn observe_certificate(&self, ip: IpAddr, fingerprint: String) -> Option<String> {
        if fingerprint.is_empty() {
            return None;
        }
        let mut state = self.state.lock().unwrap();
        let record = peer_record(&mut state, &self.options, ip.to_canonical(), Utc::now());
        record
            .cert_fp
            .replace(fingerprint)
            .filter(|old| record.cert_fp.as_ref() != Some(old))
    }

    /// Mark a missing observation, returning true the first time within the retention period.
    pub fn first_missing(&self, ip: IpAddr) -> bool {
        let mut state = self.state.lock().unwrap();
        let record = peer_record(&mut state, &self.options, ip.to_canonical(), Utc::now());
        !std::mem::replace(&mut record.missing_reported, true)
    }

    /// Look up a remembered MAC that has not expired.
    pub fn mac(&self, ip: IpAddr) -> Option<MacAddr> {
        let state = self.state.lock().unwrap();
        let record = state.ips.get(&ip.to_canonical())?;
        within(Utc::now(), record.last, self.options.ttl)
            .then_some(record.mac)
            .flatten()
    }

    /// Save peer history as JSON.
    #[cfg(feature = "serde")]
    pub fn save(&self, writer: impl std::io::Write) -> serde_json::Result<()> {
        let state = self.state.lock().unwrap();
        let now = Utc::now();
        let ips = state
            .ips
            .iter()
            .filter(|(_, peer)| within(now, peer.last, self.options.ttl))
            .map(|(ip, peer)| (*ip, peer.clone()))
            .collect();
        drop(state);
        serde_json::to_writer(writer, &Saved { ips })
    }

    /// Load JSON peer history, dropping expired and excess records.
    #[cfg(feature = "serde")]
    pub fn load(&self, reader: impl std::io::Read) -> serde_json::Result<()> {
        let saved: Saved = serde_json::from_reader(reader)?;
        let now = Utc::now();
        let mut peers: Vec<_> = saved
            .ips
            .into_iter()
            .filter(|(_, peer)| within(now, peer.last, self.options.ttl))
            .collect();
        peers.sort_unstable_by_key(|(_, peer)| peer.last);
        let mut state = State::default();
        for (ip, peer) in peers
            .into_iter()
            .rev()
            .take(self.options.max_entries.max(1))
        {
            let ip = ip.to_canonical();
            if let Some(mac) = peer.mac {
                state.macs.entry(mac).or_insert((ip, peer.last));
            }
            state.ips.entry(ip).or_insert(peer);
        }
        *self.state.lock().unwrap() = state;
        Ok(())
    }
}

#[cfg(feature = "serde")]
#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    ips: HashMap<IpAddr, Record>,
}

fn within(now: DateTime<Utc>, last: DateTime<Utc>, window: Duration) -> bool {
    (now - last)
        .to_std()
        .map_or(true, |elapsed| elapsed < window)
}

fn make_room<K: Copy + Eq + std::hash::Hash, V>(
    entries: &mut HashMap<K, V>,
    key: K,
    limit: usize,
    last: impl Fn(&V) -> DateTime<Utc>,
) {
    if !entries.contains_key(&key)
        && entries.len() >= limit.max(1)
        && let Some(oldest) = entries
            .iter()
            .min_by_key(|(_, v)| last(v))
            .map(|(key, _)| *key)
    {
        entries.remove(&oldest);
    }
}

fn peer_record<'a>(
    state: &'a mut State,
    options: &CacheOptions,
    ip: IpAddr,
    now: DateTime<Utc>,
) -> &'a mut Record {
    make_room(&mut state.ips, ip, options.max_entries, |entry| entry.last);
    let new = || Record {
        mac: None,
        #[cfg(feature = "serde")]
        first: now,
        last: now,
        cert_fp: None,
        missing_reported: false,
    };
    let record = state.ips.entry(ip).or_insert_with(new);
    if !within(now, record.last, options.ttl) {
        *record = new();
    }
    record.last = now;
    record
}

/// Get the default process-wide peer history.
pub fn default_cache() -> Arc<Cache> {
    static DEFAULT: OnceLock<Arc<Cache>> = OnceLock::new();
    DEFAULT.get_or_init(Arc::default).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_mac_replacement_and_sharing_with_bounded_history() {
        let cache = Cache::new(CacheOptions {
            max_entries: 2,
            ..CacheOptions::default()
        });
        let a = "192.0.2.10".parse().unwrap();
        let b = "192.0.2.11".parse().unwrap();
        let c = "192.0.2.12".parse().unwrap();
        let first = MacAddr([2, 1, 0, 0, 0, 1]);
        let second = MacAddr([2, 1, 0, 0, 0, 2]);

        assert!(cache.observe(a, first).is_empty());
        assert_eq!(cache.observe(b, first), [Change::MacShared { other: a }]);
        assert_eq!(
            cache.observe(b, second),
            [Change::MacChanged { previous: first }]
        );
        cache.observe(c, second);
        assert_eq!(cache.mac(a), None);
        assert_eq!(cache.mac(c), Some(second));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn restores_mac_and_fingerprint_from_saved_history() {
        let original = Cache::default();
        let ip = "192.0.2.10".parse().unwrap();
        let mac = MacAddr([2, 1, 0, 0, 0, 1]);
        original.observe(ip, mac);
        original.observe_certificate(ip, "original".into());
        let mut json = Vec::new();
        original.save(&mut json).unwrap();

        let restored = Cache::default();
        restored.load(&json[..]).unwrap();
        assert_eq!(restored.mac(ip), Some(mac));
        assert_eq!(
            restored
                .observe_certificate(ip, "replacement".into())
                .as_deref(),
            Some("original")
        );
    }
}
