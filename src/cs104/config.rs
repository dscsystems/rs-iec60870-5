// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! IEC 60870-5-104 timing and flow-control configuration.

use std::time::Duration;

use crate::error::{Error, Result};

/// The IANA registered port for an unsecured connection.
pub const PORT: u16 = 2404;

/// The IANA registered port for a TLS connection.
pub const PORT_SECURE: u16 = 19998;

/// Timer resolution of the connection state machine.
///
/// Companion standard 104, subclass 6.9 specifies whole seconds; a tenth of a
/// second makes acknowledgement much more responsive without extra traffic.
pub(crate) const TIMEOUT_RESOLUTION: Duration = Duration::from_millis(100);

/// t₀ lower bound: 1 s.
pub const CONNECT_TIMEOUT0_MIN: Duration = Duration::from_secs(1);
/// t₀ upper bound: 255 s.
pub const CONNECT_TIMEOUT0_MAX: Duration = Duration::from_secs(255);
/// t₁ lower bound: 1 s.
pub const SEND_UNACK_TIMEOUT1_MIN: Duration = Duration::from_secs(1);
/// t₁ upper bound: 255 s.
pub const SEND_UNACK_TIMEOUT1_MAX: Duration = Duration::from_secs(255);
/// t₂ lower bound: 1 s.
pub const RECV_UNACK_TIMEOUT2_MIN: Duration = Duration::from_secs(1);
/// t₂ upper bound: 255 s.
pub const RECV_UNACK_TIMEOUT2_MAX: Duration = Duration::from_secs(255);
/// t₃ lower bound: 1 s.
pub const IDLE_TIMEOUT3_MIN: Duration = Duration::from_secs(1);
/// t₃ upper bound: 48 h.
pub const IDLE_TIMEOUT3_MAX: Duration = Duration::from_secs(48 * 3600);
/// k lower bound.
pub const SEND_UNACK_LIMIT_K_MIN: u16 = 1;
/// k upper bound.
pub const SEND_UNACK_LIMIT_K_MAX: u16 = 32767;
/// w lower bound.
pub const RECV_UNACK_LIMIT_W_MIN: u16 = 1;
/// w upper bound.
pub const RECV_UNACK_LIMIT_W_MAX: u16 = 32767;

/// IEC 60870-5-104 configuration.
///
/// The defaults interoperate with virtually every implementation in the field.
/// Both peers should agree on compatible values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// t₀: TCP connection establishment timeout, `1..=255` s. Default 30 s.
    pub connect_timeout0: Duration,

    /// k: maximum number of outstanding (unacknowledged) I-frames.
    ///
    /// Transmission stops once this many frames are in flight.
    /// Range `1..=32767`, default 12. See IEC 60870-5-104, subclass 5.5.
    pub send_unack_limit_k: u16,

    /// t₁: acknowledgement timeout, `1..=255` s. Default 15 s.
    ///
    /// The connection is closed when it expires. See IEC 60870-5-104, figure 18.
    pub send_unack_timeout1: Duration,

    /// w: acknowledge at the latest after this many received I-frames.
    ///
    /// Keep `w <= 2/3 k`. Range `1..=32767`, default 8.
    pub recv_unack_limit_w: u16,

    /// t₂: maximum delay before sending a supervisory acknowledgement,
    /// `1..=255` s. Default 10 s. See IEC 60870-5-104, figure 10.
    pub recv_unack_timeout2: Duration,

    /// t₃: idle time before a TestFR keep-alive is sent, 1 s to 48 h.
    /// Default 20 s. See IEC 60870-5-104, subclass 5.2.
    pub idle_timeout3: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            connect_timeout0: Duration::from_secs(30),
            send_unack_limit_k: 12,
            send_unack_timeout1: Duration::from_secs(15),
            recv_unack_limit_w: 8,
            recv_unack_timeout2: Duration::from_secs(10),
            idle_timeout3: Duration::from_secs(20),
        }
    }
}

impl Config {
    /// The IEC defaults: t₀ = 30 s, k = 12, t₁ = 15 s, w = 8, t₂ = 10 s, t₃ = 20 s.
    pub fn new() -> Self {
        Config::default()
    }

    /// Substitute the default for each unset (zero) value, then range-check.
    ///
    /// Mirrors `Config.Valid()` in go-iecp5: a zero field is "unspecified" and
    /// takes the standard default rather than being rejected.
    pub fn valid(&mut self) -> Result<()> {
        let d = Config::default();

        if self.connect_timeout0.is_zero() {
            self.connect_timeout0 = d.connect_timeout0;
        } else if !(CONNECT_TIMEOUT0_MIN..=CONNECT_TIMEOUT0_MAX).contains(&self.connect_timeout0) {
            return Err(Error::Config(r#"ConnectTimeout0 "t0" not in [1, 255]s"#));
        }

        if self.send_unack_limit_k == 0 {
            self.send_unack_limit_k = d.send_unack_limit_k;
        } else if !(SEND_UNACK_LIMIT_K_MIN..=SEND_UNACK_LIMIT_K_MAX)
            .contains(&self.send_unack_limit_k)
        {
            return Err(Error::Config(r#"SendUnAckLimitK "k" not in [1, 32767]"#));
        }

        if self.send_unack_timeout1.is_zero() {
            self.send_unack_timeout1 = d.send_unack_timeout1;
        } else if !(SEND_UNACK_TIMEOUT1_MIN..=SEND_UNACK_TIMEOUT1_MAX)
            .contains(&self.send_unack_timeout1)
        {
            return Err(Error::Config(r#"SendUnAckTimeout1 "t1" not in [1, 255]s"#));
        }

        if self.recv_unack_limit_w == 0 {
            self.recv_unack_limit_w = d.recv_unack_limit_w;
        } else if !(RECV_UNACK_LIMIT_W_MIN..=RECV_UNACK_LIMIT_W_MAX)
            .contains(&self.recv_unack_limit_w)
        {
            return Err(Error::Config(r#"RecvUnAckLimitW "w" not in [1, 32767]"#));
        }

        if self.recv_unack_timeout2.is_zero() {
            self.recv_unack_timeout2 = d.recv_unack_timeout2;
        } else if !(RECV_UNACK_TIMEOUT2_MIN..=RECV_UNACK_TIMEOUT2_MAX)
            .contains(&self.recv_unack_timeout2)
        {
            return Err(Error::Config(r#"RecvUnAckTimeout2 "t2" not in [1, 255]s"#));
        }

        if self.idle_timeout3.is_zero() {
            self.idle_timeout3 = d.idle_timeout3;
        } else if !(IDLE_TIMEOUT3_MIN..=IDLE_TIMEOUT3_MAX).contains(&self.idle_timeout3) {
            return Err(Error::Config(r#"IdleTimeout3 "t3" not in [1 second, 48 hours]"#));
        }

        Ok(())
    }

    /// This config with out-of-range fields replaced by the defaults.
    pub(crate) fn or_default(mut self) -> Config {
        if self.valid().is_err() {
            return Config::default();
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_standard() {
        let c = Config::default();
        assert_eq!(c.connect_timeout0, Duration::from_secs(30));
        assert_eq!(c.send_unack_limit_k, 12);
        assert_eq!(c.send_unack_timeout1, Duration::from_secs(15));
        assert_eq!(c.recv_unack_limit_w, 8);
        assert_eq!(c.recv_unack_timeout2, Duration::from_secs(10));
        assert_eq!(c.idle_timeout3, Duration::from_secs(20));
    }

    #[test]
    fn zero_fields_take_the_default() {
        let mut c = Config {
            connect_timeout0: Duration::ZERO,
            send_unack_limit_k: 0,
            send_unack_timeout1: Duration::ZERO,
            recv_unack_limit_w: 0,
            recv_unack_timeout2: Duration::ZERO,
            idle_timeout3: Duration::ZERO,
        };
        c.valid().unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn out_of_range_fields_are_rejected() {
        let mut c = Config {
            send_unack_timeout1: Duration::from_secs(256),
            ..Default::default()
        };
        assert!(c.valid().is_err());

        let mut c = Config {
            idle_timeout3: Duration::from_secs(49 * 3600),
            ..Default::default()
        };
        assert!(c.valid().is_err());

        // ... and fall back to the defaults wholesale.
        assert_eq!(c.or_default(), Config::default());
    }
}
