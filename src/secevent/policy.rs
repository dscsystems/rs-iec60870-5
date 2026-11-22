// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::asdu::{Asdu, CommonAddr, InfoObjAddr, TypeId};
use crate::netpeer::{Cache, MacAddr, Resolver, default_cache, default_resolver};

use super::Action;

/// A burst and refill rate. Zero events or a zero period disables the limit.
#[derive(Debug, Default, Clone, Copy)]
pub struct Rate {
    /// Events allowed in a full bucket.
    pub count: u32,
    /// Duration over which a bucket refills.
    pub per: Duration,
}

impl Rate {
    /// Construct a rate from a count and refill period.
    pub const fn new(count: u32, per: Duration) -> Self {
        Self { count, per }
    }
}

/// Per-session thresholds for application and link activity.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Process commands.
    pub commands: Rate,
    /// General interrogations.
    pub interrogation: Rate,
    /// Counter interrogations.
    pub counter_interrogation: Rate,
    /// Read requests.
    pub read: Rate,
    /// Clock synchronisations.
    pub clock_sync: Rate,
    /// Reset requests.
    pub reset_process: Rate,
    /// TESTFR activations.
    pub test_fr: Rate,
    /// STARTDT and STOPDT activations.
    pub u_toggle: Rate,
    /// S-frames acknowledging no new data.
    pub s_frames: Rate,
    /// All received frames.
    pub frames: Rate,
}

impl Default for Limits {
    fn default() -> Self {
        let ten = Duration::from_secs(10);
        let minute = Duration::from_secs(60);
        Self {
            commands: Rate::new(20, ten),
            interrogation: Rate::new(6, minute),
            counter_interrogation: Rate::new(6, minute),
            read: Rate::new(500, ten),
            clock_sync: Rate::new(6, minute),
            reset_process: Rate::new(2, minute * 10),
            test_fr: Rate::new(20, ten),
            u_toggle: Rate::new(5, ten),
            s_frames: Rate::new(500, ten),
            frames: Rate::new(20_000, ten),
        }
    }
}

/// Station and point inventory supplied by the application.
pub trait AddressSpace: Send + Sync + 'static {
    /// Whether the station exists.
    fn known_ca(&self, ca: CommonAddr) -> bool;
    /// Whether the point exists for the requested type.
    fn known_ioa(&self, ca: CommonAddr, t: TypeId, ioa: InfoObjAddr) -> bool;
}

/// Controls which requests a station accepts.
#[derive(Debug, Default, Clone)]
pub struct FunctionPolicy {
    /// Explicitly denied types.
    pub deny_types: Vec<TypeId>,
    /// Refuse process commands and parameter operations.
    pub read_only: bool,
    /// Refuse restarts and resets.
    pub deny_reset_process: bool,
    /// Refuse clock synchronisation.
    pub deny_clock_sync: bool,
    /// Refuse parameter operations.
    pub deny_parameters: bool,
    /// Refuse file transfer services.
    pub deny_file_transfer: bool,
    /// Refuse control-direction requests to the global address.
    pub deny_broadcast_commands: bool,
    /// Use cause 44 when rejecting a function.
    pub deny_with_unknown_type: bool,
}

impl FunctionPolicy {
    /// Whether this request is disallowed.
    pub fn denies(&self, a: &Asdu) -> bool {
        let t = a.type_id();
        self.deny_types.contains(&t)
            || self.read_only && (t.is_process_command() || t.is_parameter())
            || self.deny_reset_process && t == TypeId::C_RP_NA_1
            || self.deny_clock_sync && t == TypeId::C_CS_NA_1
            || self.deny_parameters && t.is_parameter()
            || self.deny_file_transfer && t.is_file()
            || self.deny_broadcast_commands && t.is_control_direction() && a.is_global()
    }
}

/// Optional enforcement and thresholds. Defaults observe activity only.
#[derive(Clone)]
pub struct Policy {
    /// Expected networks for incoming IEC 104 connections. Empty allows any.
    pub allowed_peers: Vec<ipnet::IpNet>,
    /// Reject peers outside the configured networks.
    pub reject_unlisted: bool,
    /// Expected MAC for specific IP addresses.
    pub peer_bindings: HashMap<IpAddr, MacAddr>,
    /// Reject a missing or different MAC on a bound address.
    pub reject_binding_mismatch: bool,
    /// Concurrent IEC 104 server sessions. Zero is unlimited.
    pub max_sessions: usize,
    /// Concurrent IEC 104 server sessions per IP. Zero is unlimited.
    pub max_sessions_per_ip: usize,
    /// Rate of new IEC 104 connections per IP.
    pub accept_rate: Rate,
    /// Overall rate of new IEC 104 connections.
    pub global_accept_rate: Rate,
    /// Treatment of activity over the rate. Only Throttled and Disconnected enforce.
    pub flood_action: Action,
    /// Ignore unsolicited STARTDT and STOPDT confirmations at IEC 104 clients.
    pub ignore_unsolicited_u: bool,
    /// Suppress the audit trail.
    pub disable_audit: bool,
    /// Station and point inventory.
    pub address_space: Option<Arc<dyn AddressSpace>>,
    /// Automatically refuse requests for unknown stations or points.
    pub auto_reject_unknown: bool,
    /// Permitted request functions.
    pub functions: FunctionPolicy,
    /// Suppress profile notices for private ASDU types.
    pub allow_private_types: bool,
    /// Expected originator addresses.
    pub allowed_originators: Vec<u8>,
    /// Maximum elapsed age of a command. Zero disables the check.
    pub max_command_age: Duration,
    /// Maximum future skew of a command. Zero disables the check.
    pub max_command_future: Duration,
    /// Send a negative confirmation for a stale command.
    pub reject_stale_commands: bool,
    /// Time during which identical commands raise a duplicate event.
    pub duplicate_window: Duration,
    /// Clock change that triggers a notice. Zero disables the check.
    pub clock_step_threshold: Duration,
    /// Distinct unknown requests that trigger a scan notice.
    pub scan_threshold: usize,
    /// Time over which distinct unknown requests are counted.
    pub scan_window: Duration,
    /// Per-session activity thresholds.
    pub limits: Limits,
    /// Time allowed before the first IEC 104 data transfer activation. Zero disables.
    pub inactive_timeout: Duration,
    /// Source of on-link MAC addresses.
    pub resolver: Arc<dyn Resolver>,
    /// Shared history of peer MAC addresses and certificates.
    pub peer_cache: Arc<Cache>,
    /// Time allowed to wait for a MAC lookup.
    pub mac_lookup_timeout: Duration,
    /// Interval for rechecking a live IEC 104 peer. Zero disables.
    pub mac_recheck_interval: Duration,
    /// Disconnect if a known MAC changes during a connection.
    pub disconnect_on_mac_change: bool,
    /// Warn when a peer certificate expires within this period. Zero disables.
    pub cert_expiry_warning: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            allowed_peers: Vec::new(),
            reject_unlisted: false,
            peer_bindings: HashMap::new(),
            reject_binding_mismatch: false,
            max_sessions: 0,
            max_sessions_per_ip: 0,
            accept_rate: Rate::new(10, Duration::from_secs(10)),
            global_accept_rate: Rate::new(100, Duration::from_secs(10)),
            flood_action: Action::Observed,
            ignore_unsolicited_u: false,
            disable_audit: false,
            address_space: None,
            auto_reject_unknown: false,
            functions: FunctionPolicy::default(),
            allow_private_types: false,
            allowed_originators: Vec::new(),
            max_command_age: Duration::ZERO,
            max_command_future: Duration::ZERO,
            reject_stale_commands: false,
            duplicate_window: Duration::ZERO,
            clock_step_threshold: Duration::from_secs(60),
            scan_threshold: 20,
            scan_window: Duration::from_secs(60),
            limits: Limits::default(),
            inactive_timeout: Duration::ZERO,
            resolver: default_resolver(),
            peer_cache: default_cache(),
            mac_lookup_timeout: Duration::from_millis(200),
            mac_recheck_interval: Duration::ZERO,
            disconnect_on_mac_change: false,
            cert_expiry_warning: Duration::from_secs(30 * 24 * 3600),
        }
    }
}

#[cfg(any(feature = "cs101", feature = "cs104"))]
impl Policy {
    pub(crate) fn flood_action(&self) -> Action {
        match self.flood_action {
            Action::Throttled | Action::Disconnected => self.flood_action,
            _ => Action::Observed,
        }
    }
}
