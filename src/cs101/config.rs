// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Link-layer, serial and TCP configuration for IEC 60870-5-101 and -103.

use std::fmt;
use std::time::Duration;

use crate::error::{Error, Result};

/// How the FT1.2 frames are carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransportType {
    /// A local serial port. Requires the `serial` feature.
    #[default]
    Serial,
    /// Dial out to `TcpConfig::address` and run FT1.2 over the TCP stream.
    ///
    /// This is the usual arrangement with a terminal server or serial-device
    /// server. It is still 101/103 framing inside a pipe, not IEC 104.
    TcpClient,
    /// Listen on `TcpConfig::address`, serving one connection at a time.
    TcpServer,
}

impl fmt::Display for TransportType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TransportType::Serial => "serial",
            TransportType::TcpClient => "tcp-client",
            TransportType::TcpServer => "tcp-server",
        })
    }
}

/// The transmission procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransmissionMode {
    /// One primary station polls one or more secondaries on a shared line.
    /// Only the primary initiates; secondaries answer polls.
    #[default]
    Unbalanced,
    /// Point-to-point: both stations may transmit spontaneously, each running
    /// a primary role for sending and a secondary role for acknowledging.
    Balanced,
}

/// Parity of the serial line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Parity {
    /// No parity bit.
    None,
    /// Odd parity.
    Odd,
    /// Even parity. This is the IEC 60870-5-101 standard (8E1).
    #[default]
    Even,
}

/// Number of stop bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StopBits {
    /// One stop bit, the IEC 60870-5-101 standard.
    #[default]
    One,
    /// Two stop bits.
    Two,
}

/// Serial port settings.
///
/// 8E1 (eight data bits, even parity, one stop bit) is the framing the standard
/// specifies, and is the default here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialConfig {
    /// Port name, for example `/dev/ttyUSB0` or `COM3`.
    pub address: String,
    /// Line speed in bits per second.
    pub baud_rate: u32,
    /// Data bits, normally 8.
    pub data_bits: u8,
    /// Stop bits.
    pub stop_bits: StopBits,
    /// Parity.
    pub parity: Parity,
    /// Read timeout; `None` blocks indefinitely.
    pub timeout: Option<Duration>,
}

impl Default for SerialConfig {
    fn default() -> Self {
        SerialConfig {
            address: String::new(),
            baud_rate: 9600,
            data_bits: 8,
            stop_bits: StopBits::One,
            parity: Parity::Even,
            timeout: None,
        }
    }
}

impl SerialConfig {
    /// A standard 8E1 port at `baud_rate`.
    pub fn new(address: impl Into<String>, baud_rate: u32) -> Self {
        SerialConfig {
            address: address.into(),
            baud_rate,
            ..Default::default()
        }
    }
}

/// Default TCP connect timeout when none is configured.
pub const DEFAULT_TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Settings for the TCP encapsulation transports.
#[derive(Debug, Clone, Default)]
pub struct TcpConfig {
    /// `host:port` to dial ([`TransportType::TcpClient`]) or to listen on
    /// ([`TransportType::TcpServer`], for example `":2400"`).
    pub address: String,
    /// Bounds dialling and the TLS handshake. Defaults to 30 s.
    pub connect_timeout: Option<Duration>,
    /// Wrap the stream in TLS when set. Requires the `tls` feature.
    #[cfg(feature = "tls")]
    pub tls_client: Option<crate::cs104::TlsClientConfig>,
    /// Wrap accepted connections in TLS when set. Requires the `tls` feature.
    #[cfg(feature = "tls")]
    pub tls_server: Option<crate::cs104::TlsServerConfig>,
}

// -- defaults and ranges --------------------------------------------------
/// Default response timeout t₁.
pub const DEFAULT_TIMEOUT_RESPONSE_T1: Duration = Duration::from_secs(10);
/// t₁ lower bound.
pub const TIMEOUT_RESPONSE_T1_MIN: Duration = Duration::from_secs(1);
/// t₁ upper bound.
pub const TIMEOUT_RESPONSE_T1_MAX: Duration = Duration::from_secs(255);
/// Default repetition timeout t₂.
pub const DEFAULT_TIMEOUT_REPEAT_T2: Duration = Duration::from_secs(5);
/// t₂ lower bound.
pub const TIMEOUT_REPEAT_T2_MIN: Duration = Duration::from_secs(1);
/// t₂ upper bound.
pub const TIMEOUT_REPEAT_T2_MAX: Duration = Duration::from_secs(255);
/// Default idle timeout t₃.
pub const DEFAULT_TIMEOUT_TEST_T3: Duration = Duration::from_secs(20);
/// t₃ lower bound.
pub const TIMEOUT_TEST_T3_MIN: Duration = Duration::from_secs(1);
/// t₃ upper bound: 48 hours.
pub const TIMEOUT_TEST_T3_MAX: Duration = Duration::from_secs(172_800);
/// Default pacing of the poll and transmit scheduler.
pub const DEFAULT_TIMEOUT_SEND_LINK_MSG: Duration = Duration::from_millis(200);
/// Scheduler pacing lower bound.
pub const TIMEOUT_SEND_LINK_MSG_MIN: Duration = Duration::from_millis(1);
/// Scheduler pacing upper bound.
pub const TIMEOUT_SEND_LINK_MSG_MAX: Duration = Duration::from_secs(10);
/// Default outbound queue capacity.
pub const DEFAULT_MAX_SEND_QUEUE_SIZE: usize = 100;
/// Default link address width in octets.
pub const DEFAULT_LINK_ADDR_SIZE: u8 = 1;
/// Default maximum ASDU length carried in one frame.
pub const DEFAULT_MAX_APDU_LENGTH: u8 = 253;

/// IEC 60870-5-101 link-layer configuration.
///
/// Both stations must agree on `link_addr_size`, `link_address` and `mode`, and
/// on the `asdu::Params` set separately on the endpoint.
#[derive(Debug, Clone)]
pub struct Config {
    /// How FT1.2 frames are carried.
    pub transport: TransportType,
    /// Serial port settings, used when `transport` is [`TransportType::Serial`].
    pub serial: SerialConfig,
    /// TCP settings, used by the other transports.
    pub tcp: TcpConfig,
    /// Unbalanced (polled) or balanced (point-to-point).
    pub mode: TransmissionMode,
    /// This station's address (secondary) or the station to poll (primary).
    pub link_address: u16,
    /// Link address width on the wire: 1 or 2 octets.
    pub link_addr_size: u8,
    /// t₁: response timeout for confirmed frames, `1..=255` s.
    pub timeout_response_t1: Duration,
    /// t₂: timeout after the first repetition, `1..=255` s and below t₁.
    pub timeout_repeat_t2: Duration,
    /// t₃: idle time before a keep-alive, 1 s to 48 h.
    pub timeout_test_t3: Duration,
    /// Pacing of the poll and transmit scheduler; effectively the polling
    /// period of an unbalanced primary. 1 ms to 10 s.
    pub timeout_send_link_msg: Duration,
    /// Capacity of the outbound queue and of each class buffer.
    pub max_send_queue_size: usize,
    /// Maximum ASDU length carried in one frame, `1..=253`.
    pub max_apdu_length: u8,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            transport: TransportType::Serial,
            serial: SerialConfig::default(),
            tcp: TcpConfig::default(),
            mode: TransmissionMode::Unbalanced,
            link_address: 1,
            link_addr_size: DEFAULT_LINK_ADDR_SIZE,
            timeout_response_t1: DEFAULT_TIMEOUT_RESPONSE_T1,
            timeout_repeat_t2: DEFAULT_TIMEOUT_REPEAT_T2,
            timeout_test_t3: DEFAULT_TIMEOUT_TEST_T3,
            timeout_send_link_msg: DEFAULT_TIMEOUT_SEND_LINK_MSG,
            max_send_queue_size: DEFAULT_MAX_SEND_QUEUE_SIZE,
            max_apdu_length: DEFAULT_MAX_APDU_LENGTH,
        }
    }
}

impl Config {
    /// The defaults: serial 8E1, unbalanced, link address 1 in one octet.
    pub fn new() -> Self {
        Config::default()
    }

    /// Substitute the default for each unset (zero) timing value, then check
    /// that everything is in range and mutually consistent.
    pub fn valid(&mut self) -> Result<()> {
        match self.transport {
            TransportType::Serial => {
                if self.serial.address.is_empty() {
                    return Err(Error::Config("the serial port name must be configured"));
                }
                if self.serial.baud_rate == 0 {
                    return Err(Error::Config("the serial baud rate must be positive"));
                }
            }
            TransportType::TcpClient | TransportType::TcpServer => {
                if self.tcp.address.is_empty() {
                    return Err(Error::Config(
                        "the TCP address must be configured for a TCP transport",
                    ));
                }
                if self.tcp.connect_timeout.is_none() {
                    self.tcp.connect_timeout = Some(DEFAULT_TCP_CONNECT_TIMEOUT);
                }
            }
        }

        if !(1..=2).contains(&self.link_addr_size) {
            return Err(Error::Config("the link address size must be 1 or 2"));
        }
        if self.link_addr_size == 1 && self.link_address > 0xff {
            return Err(Error::Config("the link address exceeds one octet"));
        }

        if self.timeout_response_t1.is_zero() {
            self.timeout_response_t1 = DEFAULT_TIMEOUT_RESPONSE_T1;
        } else if !(TIMEOUT_RESPONSE_T1_MIN..=TIMEOUT_RESPONSE_T1_MAX)
            .contains(&self.timeout_response_t1)
        {
            return Err(Error::Config("timeout t1 not in [1, 255]s"));
        }

        if self.timeout_repeat_t2.is_zero() {
            self.timeout_repeat_t2 = DEFAULT_TIMEOUT_REPEAT_T2;
        } else if !(TIMEOUT_REPEAT_T2_MIN..=TIMEOUT_REPEAT_T2_MAX).contains(&self.timeout_repeat_t2)
        {
            return Err(Error::Config("timeout t2 not in [1, 255]s"));
        }
        if self.timeout_repeat_t2 >= self.timeout_response_t1 {
            return Err(Error::Config("timeout t2 must be less than t1"));
        }

        if self.timeout_test_t3.is_zero() {
            self.timeout_test_t3 = DEFAULT_TIMEOUT_TEST_T3;
        } else if !(TIMEOUT_TEST_T3_MIN..=TIMEOUT_TEST_T3_MAX).contains(&self.timeout_test_t3) {
            return Err(Error::Config("timeout t3 not in [1 second, 48 hours]"));
        }

        if self.timeout_send_link_msg.is_zero() {
            self.timeout_send_link_msg = DEFAULT_TIMEOUT_SEND_LINK_MSG;
        } else if !(TIMEOUT_SEND_LINK_MSG_MIN..=TIMEOUT_SEND_LINK_MSG_MAX)
            .contains(&self.timeout_send_link_msg)
        {
            return Err(Error::Config(
                "the link message send timeout is not in [1ms, 10s]",
            ));
        }

        if self.max_send_queue_size == 0 {
            self.max_send_queue_size = DEFAULT_MAX_SEND_QUEUE_SIZE;
        }

        if self.max_apdu_length == 0 {
            self.max_apdu_length = DEFAULT_MAX_APDU_LENGTH;
        } else if self.max_apdu_length > DEFAULT_MAX_APDU_LENGTH {
            return Err(Error::Config("the maximum APDU length is out of range"));
        }

        Ok(())
    }

    /// True when this configuration runs the balanced procedure.
    pub fn is_balanced(&self) -> bool {
        self.mode == TransmissionMode::Balanced
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
        let mut c = serial_cfg();
        assert!(c.valid().is_ok());
    }

    #[test]
    fn a_tcp_config_needs_an_address_and_gets_a_default_timeout() {
        let mut c = Config {
            transport: TransportType::TcpClient,
            ..Default::default()
        };
        assert!(c.valid().is_err());

        c.tcp.address = "10.0.0.9:2400".into();
        c.valid().unwrap();
        assert_eq!(c.tcp.connect_timeout, Some(DEFAULT_TCP_CONNECT_TIMEOUT));
    }

    #[test]
    fn zero_timings_take_the_defaults() {
        let mut c = Config {
            timeout_response_t1: Duration::ZERO,
            timeout_repeat_t2: Duration::ZERO,
            timeout_test_t3: Duration::ZERO,
            timeout_send_link_msg: Duration::ZERO,
            max_send_queue_size: 0,
            max_apdu_length: 0,
            ..serial_cfg()
        };
        c.valid().unwrap();
        assert_eq!(c.timeout_response_t1, DEFAULT_TIMEOUT_RESPONSE_T1);
        assert_eq!(c.timeout_repeat_t2, DEFAULT_TIMEOUT_REPEAT_T2);
        assert_eq!(c.timeout_test_t3, DEFAULT_TIMEOUT_TEST_T3);
        assert_eq!(c.timeout_send_link_msg, DEFAULT_TIMEOUT_SEND_LINK_MSG);
        assert_eq!(c.max_send_queue_size, DEFAULT_MAX_SEND_QUEUE_SIZE);
        assert_eq!(c.max_apdu_length, DEFAULT_MAX_APDU_LENGTH);
    }

    #[test]
    fn t2_must_be_shorter_than_t1() {
        let mut c = Config {
            timeout_response_t1: Duration::from_secs(5),
            timeout_repeat_t2: Duration::from_secs(5),
            ..serial_cfg()
        };
        assert_eq!(c.valid(), Err(Error::Config("timeout t2 must be less than t1")));
    }

    #[test]
    fn the_link_address_must_fit_its_configured_width() {
        let mut c = Config {
            link_addr_size: 1,
            link_address: 300,
            ..serial_cfg()
        };
        assert!(c.valid().is_err());

        let mut c = Config {
            link_addr_size: 2,
            link_address: 300,
            ..serial_cfg()
        };
        assert!(c.valid().is_ok());

        let mut c = Config {
            link_addr_size: 3,
            ..serial_cfg()
        };
        assert!(c.valid().is_err());
    }

    #[test]
    fn out_of_range_timings_are_rejected() {
        for c in [
            Config {
                timeout_response_t1: Duration::from_secs(256),
                ..serial_cfg()
            },
            Config {
                timeout_test_t3: Duration::from_secs(172_801),
                ..serial_cfg()
            },
            Config {
                timeout_send_link_msg: Duration::from_secs(11),
                ..serial_cfg()
            },
        ] {
            let mut c = c;
            assert!(c.valid().is_err());
        }
    }
}
