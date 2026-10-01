// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Configuration of the IEC 60870-5-103 primary station.

use std::time::Duration;

use crate::asdu::TimeZone;
use crate::cs101::{SerialConfig, TcpConfig, TransportType};
use crate::error::Result;

/// Default response timeout t₁.
pub const DEFAULT_TIMEOUT_RESPONSE_T1: Duration = Duration::from_secs(10);
/// Default repetition timeout t₂.
pub const DEFAULT_TIMEOUT_REPEAT_T2: Duration = Duration::from_secs(5);
/// Default idle timeout t₃.
pub const DEFAULT_TIMEOUT_TEST_T3: Duration = Duration::from_secs(20);
/// Default pacing of the poll and transmit scheduler.
pub const DEFAULT_TIMEOUT_SEND_LINK_MSG: Duration = Duration::from_millis(200);
/// Default outbound queue capacity.
pub const DEFAULT_MAX_SEND_QUEUE_SIZE: usize = 100;

/// The link address width of IEC 60870-5-103 is fixed at one octet.
pub const LINK_ADDR_SIZE: u8 = 1;

/// IEC 60870-5-103 primary station configuration.
///
/// The link always runs the unbalanced procedure — the standard defines no
/// balanced one — and the link address and ASDU common address are one octet
/// and conventionally equal.
#[derive(Debug, Clone)]
pub struct Config {
    /// How the FT1.2 frames are carried: a local serial port, or a TCP stream
    /// when the relay is reached through a terminal server.
    pub transport: TransportType,
    /// Serial port settings, used with [`TransportType::Serial`]. 8E1 is the
    /// framing the standard specifies, and the default here.
    pub serial: SerialConfig,
    /// TCP settings, used by the other transports.
    pub tcp: TcpConfig,
    /// Address of the single or default protection device.
    ///
    /// More devices are added with
    /// [`ClientOption::with_secondary_address`](crate::cs103::ClientOption::with_secondary_address).
    pub link_address: u8,
    /// t₁: response timeout for confirmed frames, `1..=255` s.
    pub timeout_response_t1: Duration,
    /// t₂: timeout after the first repetition, `1..=255` s and below t₁.
    pub timeout_repeat_t2: Duration,
    /// t₃: idle time before a keep-alive class 2 poll, 1 s to 48 h.
    pub timeout_test_t3: Duration,
    /// Pacing of the poll and transmit scheduler, 1 ms to 10 s. Every tick the
    /// primary either transmits queued data or polls the next device.
    pub timeout_send_link_msg: Duration,
    /// Capacity of the outbound ASDU queue.
    pub max_send_queue_size: usize,
    /// How many times an unanswered confirmed frame is repeated, t₂ apart,
    /// before the device is given up and its link restarted. Zero means no
    /// repetition. Default 1.
    pub max_repetitions: u8,
    /// Send a time synchronization followed by a general interrogation
    /// automatically whenever a device's link becomes active. On by default.
    pub auto_init: bool,
    /// Time zone the CP32/CP56 time tags of this link are expressed in.
    ///
    /// Decides the SU (summer time) bit as well as the wall clock reading, so
    /// it must match what the device expects. UTC is the standard's
    /// recommendation and the default.
    pub time_zone: TimeZone,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            transport: TransportType::Serial,
            serial: SerialConfig::default(),
            tcp: TcpConfig::default(),
            link_address: 1,
            timeout_response_t1: DEFAULT_TIMEOUT_RESPONSE_T1,
            timeout_repeat_t2: DEFAULT_TIMEOUT_REPEAT_T2,
            timeout_test_t3: DEFAULT_TIMEOUT_TEST_T3,
            time_zone: TimeZone::Utc,
            timeout_send_link_msg: DEFAULT_TIMEOUT_SEND_LINK_MSG,
            max_send_queue_size: DEFAULT_MAX_SEND_QUEUE_SIZE,
            max_repetitions: crate::cs101::DEFAULT_MAX_REPETITIONS,
            auto_init: true,
        }
    }
}

impl Config {
    /// The defaults: serial 8E1, device address 1, automatic initialization.
    pub fn new() -> Self {
        Config::default()
    }

    /// Substitute the default for each unset (zero) timing value, then check
    /// that everything is in range and mutually consistent.
    pub fn valid(&mut self) -> Result<()> {
        // The link-layer timings share their ranges with cs101, so validate
        // them through a cs101 config rather than duplicating the bounds.
        let mut link = crate::cs101::Config {
            transport: self.transport,
            serial: self.serial.clone(),
            tcp: self.tcp.clone(),
            link_address: self.link_address as u16,
            link_addr_size: LINK_ADDR_SIZE,
            timeout_response_t1: self.timeout_response_t1,
            timeout_repeat_t2: self.timeout_repeat_t2,
            timeout_test_t3: self.timeout_test_t3,
            timeout_send_link_msg: self.timeout_send_link_msg,
            max_send_queue_size: self.max_send_queue_size,
            max_repetitions: self.max_repetitions,
            ..Default::default()
        };
        link.valid()?;

        self.tcp = link.tcp;
        self.timeout_response_t1 = link.timeout_response_t1;
        self.timeout_repeat_t2 = link.timeout_repeat_t2;
        self.timeout_test_t3 = link.timeout_test_t3;
        self.timeout_send_link_msg = link.timeout_send_link_msg;
        self.max_send_queue_size = link.max_send_queue_size;
        Ok(())
    }

    /// The link-layer configuration this 103 station runs over.
    pub(crate) fn link_config(&self) -> crate::cs101::Config {
        crate::cs101::Config {
            transport: self.transport,
            serial: self.serial.clone(),
            tcp: self.tcp.clone(),
            link_address: self.link_address as u16,
            link_addr_size: LINK_ADDR_SIZE,
            timeout_response_t1: self.timeout_response_t1,
            timeout_repeat_t2: self.timeout_repeat_t2,
            timeout_test_t3: self.timeout_test_t3,
            timeout_send_link_msg: self.timeout_send_link_msg,
            max_send_queue_size: self.max_send_queue_size,
            max_repetitions: self.max_repetitions,
            ..Default::default()
        }
    }

    /// A short description of the configured endpoint, for logging.
    pub fn transport_label(&self) -> &str {
        match self.transport {
            TransportType::Serial => &self.serial.address,
            _ => &self.tcp.address,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial_cfg() -> Config {
        Config {
            serial: SerialConfig::new("/dev/ttyUSB0", 9600),
            ..Default::default()
        }
    }

    #[test]
    fn a_serial_config_needs_a_port_name() {
        let mut c = Config::default();
        assert!(c.valid().is_err());
        assert!(serial_cfg().valid().is_ok());
    }

    #[test]
    fn zero_timings_take_the_defaults() {
        let mut c = Config {
            timeout_response_t1: Duration::ZERO,
            timeout_repeat_t2: Duration::ZERO,
            timeout_test_t3: Duration::ZERO,
            timeout_send_link_msg: Duration::ZERO,
            max_send_queue_size: 0,
            ..serial_cfg()
        };
        c.valid().unwrap();
        assert_eq!(c.timeout_response_t1, DEFAULT_TIMEOUT_RESPONSE_T1);
        assert_eq!(c.timeout_repeat_t2, DEFAULT_TIMEOUT_REPEAT_T2);
        assert_eq!(c.timeout_test_t3, DEFAULT_TIMEOUT_TEST_T3);
        assert_eq!(c.timeout_send_link_msg, DEFAULT_TIMEOUT_SEND_LINK_MSG);
        assert_eq!(c.max_send_queue_size, DEFAULT_MAX_SEND_QUEUE_SIZE);
    }

    #[test]
    fn t2_must_be_shorter_than_t1() {
        let mut c = Config {
            timeout_response_t1: Duration::from_secs(5),
            timeout_repeat_t2: Duration::from_secs(5),
            ..serial_cfg()
        };
        assert!(c.valid().is_err());
    }

    #[test]
    fn a_tcp_config_needs_an_address() {
        let mut c = Config {
            transport: TransportType::TcpClient,
            ..Default::default()
        };
        assert!(c.valid().is_err());
        c.tcp.address = "10.0.0.9:2400".into();
        assert!(c.valid().is_ok());
        assert_eq!(c.transport_label(), "10.0.0.9:2400");
    }

    #[test]
    fn the_link_config_fixes_a_one_octet_address() {
        let c = Config {
            link_address: 3,
            ..serial_cfg()
        };
        let link = c.link_config();
        assert_eq!(link.link_addr_size, LINK_ADDR_SIZE);
        assert_eq!(link.link_address, 3);
        assert!(!link.is_balanced(), "103 has no balanced procedure");
    }

    #[test]
    fn auto_init_is_on_by_default() {
        assert!(Config::new().auto_init);
    }
}
