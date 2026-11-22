// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! On-link peer identity for security event correlation.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, OnceLock, RwLock};

mod cache;
pub use cache::{Cache, CacheOptions, Change, default_cache};

/// An Ethernet MAC address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MacAddr(pub [u8; 6]);

impl std::str::FromStr for MacAddr {
    type Err = std::io::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid =
            || std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid MAC address");
        let mut parts = value.split([':', '-']);
        let mut octets = [0; 6];
        for octet in &mut octets {
            let part = parts.next().ok_or_else(invalid)?;
            if part.is_empty() || part.len() > 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(invalid());
            }
            *octet = u8::from_str_radix(part, 16).map_err(|_| invalid())?;
        }
        if parts.next().is_some() {
            return Err(invalid());
        }
        Ok(Self(octets))
    }
}

impl std::fmt::Display for MacAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{g:02x}")
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for MacAddr {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for MacAddr {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(de)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// Source of the observed MAC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(rename_all = "snake_case")
)]
pub enum Source {
    /// Kernel neighbour table.
    Neighbor,
    /// Explicit address map.
    Static,
    /// Application-supplied resolver.
    Custom,
}

/// Link-layer reachability and identity of an IP address.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Neighbor {
    /// Ethernet address when known.
    pub mac: Option<MacAddr>,
    /// Where the MAC came from.
    pub source: Option<Source>,
    /// Whether the IP is directly reachable.
    pub on_link: bool,
    /// Whether the IP denotes the local host.
    pub loopback: bool,
    /// Network interface when known.
    pub interface: Option<String>,
}

/// Looks up the peer itself in the local neighbour table.
///
/// Implementations must return no address for routed peers. Endpoints run
/// lookups on a blocking worker and bound how long they wait for a result.
pub trait Resolver: Send + Sync + 'static {
    /// Get the currently cached MAC, if available.
    fn lookup(&self, ip: IpAddr) -> Option<MacAddr>;

    /// Resolve MAC and reachability metadata.
    fn resolve(&self, ip: IpAddr) -> Neighbor {
        let mac = self.lookup(ip);
        Neighbor {
            mac,
            source: mac.map(|_| Source::Custom),
            on_link: mac.is_some(),
            ..Neighbor::default()
        }
    }
}

impl<F: Fn(IpAddr) -> Option<MacAddr> + Send + Sync + 'static> Resolver for F {
    fn lookup(&self, ip: IpAddr) -> Option<MacAddr> {
        self(ip)
    }
}

/// Resolves a peer through the host's neighbour table.
#[derive(Debug, Default)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn lookup(&self, ip: IpAddr) -> Option<MacAddr> {
        self.resolve(ip).mac
    }

    fn resolve(&self, ip: IpAddr) -> Neighbor {
        let ip = ip.to_canonical();
        if ip.is_loopback() {
            return Neighbor {
                on_link: true,
                loopback: true,
                ..Neighbor::default()
            };
        }
        if ip.is_unspecified() || ip.is_multicast() {
            return Neighbor::default();
        }
        let mut neighbor = system_lookup(ip);
        neighbor.mac = neighbor.mac.filter(|mac| mac.0 != [0; 6]);
        neighbor.source = neighbor.mac.map(|_| Source::Neighbor);
        neighbor
    }
}

/// Resolver backed by a caller-provided map of on-link peers.
#[derive(Debug, Default)]
pub struct StaticResolver(pub HashMap<IpAddr, MacAddr>);

impl Resolver for StaticResolver {
    fn lookup(&self, ip: IpAddr) -> Option<MacAddr> {
        self.0.get(&ip.to_canonical()).copied()
    }

    fn resolve(&self, ip: IpAddr) -> Neighbor {
        let mac = self.lookup(ip);
        Neighbor {
            mac,
            on_link: mac.is_some(),
            source: mac.map(|_| Source::Static),
            ..Neighbor::default()
        }
    }
}

fn resolver_cell() -> &'static RwLock<Arc<dyn Resolver>> {
    static DEFAULT: OnceLock<RwLock<Arc<dyn Resolver>>> = OnceLock::new();
    DEFAULT.get_or_init(|| RwLock::new(Arc::new(SystemResolver)))
}

/// Get the process-wide default resolver.
pub fn default_resolver() -> Arc<dyn Resolver> {
    resolver_cell().read().unwrap().clone()
}

/// Replace the resolver used by new policies and standalone lookups.
pub fn set_default_resolver(resolver: Arc<dyn Resolver>) {
    *resolver_cell().write().unwrap() = resolver;
}

/// Resolve a peer with the process-wide default resolver.
pub fn lookup(ip: IpAddr) -> Neighbor {
    default_resolver().resolve(ip.to_canonical())
}

/// Look up a MAC with the process-wide default resolver.
pub fn lookup_mac(ip: IpAddr) -> Option<MacAddr> {
    lookup(ip).mac
}

#[cfg(target_os = "linux")]
fn system_lookup(ip: IpAddr) -> Neighbor {
    let address = ip.to_string();
    let route = std::process::Command::new("ip")
        .args(["route", "get", &address])
        .output();
    if let Ok(route) = route
        && route.status.success()
    {
        let text = String::from_utf8_lossy(&route.stdout);
        let mut fields = text.split_whitespace();
        if fields.next() != Some(address.as_str()) || fields.clone().any(|field| field == "via") {
            return Neighbor::default();
        }
        if let Some(device) = fields
            .find(|field| *field == "dev")
            .and_then(|_| fields.next())
        {
            let neighbours = std::process::Command::new("ip")
                .args(["neigh", "show", "to", &address, "dev", device])
                .output()
                .ok();
            let mac = neighbours.as_ref().and_then(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .find_map(|line| {
                        let mut fields = line.split_whitespace();
                        if fields.next()? != address {
                            return None;
                        }
                        fields
                            .find(|field| *field == "lladdr")
                            .and_then(|_| fields.next())
                            .and_then(|mac| mac.parse().ok())
                    })
            });
            return Neighbor {
                mac: mac.or_else(|| linux_arp(&address, Some(device)).map(|(mac, _)| mac)),
                on_link: true,
                interface: Some(device.into()),
                ..Neighbor::default()
            };
        }
    }
    // With no ip utility, /proc still reports IPv4 neighbours by exact address.
    linux_arp(&address, None).map_or_else(Neighbor::default, |(mac, interface)| Neighbor {
        mac: Some(mac),
        on_link: true,
        interface: Some(interface),
        ..Neighbor::default()
    })
}

#[cfg(target_os = "linux")]
fn linux_arp(address: &str, device: Option<&str>) -> Option<(MacAddr, String)> {
    let table = std::fs::read_to_string("/proc/net/arp").ok()?;
    table.lines().skip(1).find_map(|line| {
        let mut fields = line.split_whitespace();
        if fields.next()? != address {
            return None;
        }
        let flags = u32::from_str_radix(fields.nth(1)?.trim_start_matches("0x"), 16).ok()?;
        if flags & 2 == 0 {
            return None;
        }
        let mac = fields.next()?.parse().ok()?;
        let interface = fields.nth(1)?;
        if device.is_some_and(|expected| expected != interface) {
            return None;
        }
        Some((mac, interface.to_owned()))
    })
}

#[cfg(any(
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn platform_mac(ip: IpAddr) -> Option<MacAddr> {
    let address = ip.to_string();
    let (program, flag) = if ip.is_ipv4() {
        ("arp", "-n")
    } else {
        ("ndp", "-n")
    };
    let output = std::process::Command::new(program)
        .args([flag, &address])
        .output()
        .ok()?;
    let output = std::str::from_utf8(&output.stdout).ok()?;
    output.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let matched = if ip.is_ipv4() {
            fields.any(|field| {
                field.strip_prefix('(').and_then(|f| f.strip_suffix(')')) == Some(address.as_str())
            })
        } else {
            fields.next().and_then(|field| field.split('%').next()) == Some(address.as_str())
        };
        if matched {
            fields.find_map(|field| field.parse().ok())
        } else {
            None
        }
    })
}

#[cfg(target_os = "windows")]
fn platform_mac(ip: IpAddr) -> Option<MacAddr> {
    let IpAddr::V4(ip) = ip else { return None };
    #[link(name = "iphlpapi")]
    unsafe extern "system" {
        fn SendARP(dest_ip: u32, src_ip: u32, mac_addr: *mut u32, len: *mut u32) -> u32;
    }
    let mut mac = [0_u32; 2];
    let mut len = 6;
    // SendARP takes an IPv4 address in network byte order and a buffer of at
    // least two ULONGs. The returned bytes are in Ethernet order.
    let status = unsafe {
        SendARP(
            u32::from_ne_bytes(ip.octets()),
            0,
            mac.as_mut_ptr(),
            &mut len,
        )
    };
    if status != 0 || len != 6 {
        return None;
    }
    let [a, b, c, d] = mac[0].to_ne_bytes();
    let [e, f, ..] = mac[1].to_ne_bytes();
    Some(MacAddr([a, b, c, d, e, f]))
}

#[cfg(not(target_os = "linux"))]
fn system_lookup(ip: IpAddr) -> Neighbor {
    let mac = platform_mac(ip);
    Neighbor {
        mac,
        on_link: mac.is_some(),
        ..Neighbor::default()
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
fn platform_mac(_: IpAddr) -> Option<MacAddr> {
    None
}
