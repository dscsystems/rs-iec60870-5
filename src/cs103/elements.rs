// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Information elements of IEC 60870-5-103: function types, information
//! numbers, measurands and the CP32Time2a time tag.

use std::fmt;

use chrono::{DateTime, Duration, Utc};

use crate::asdu::TimeZone;

/// Standardized function types (FUN).
///
/// See IEC 60870-5-103, subclass 7.2.5.1.
pub mod fun {
    /// 128: distance protection
    pub const DISTANCE_PROTECTION: u8 = 128;
    /// 160: overcurrent protection
    pub const OVERCURRENT_PROTECTION: u8 = 160;
    /// 176: transformer differential protection
    pub const TRANSFORMER_DIFF: u8 = 176;
    /// 192: line differential protection
    pub const LINE_DIFF: u8 = 192;
    /// 254: generic function type
    pub const GENERIC: u8 = 254;
    /// 255: global function type, used by the system messages
    pub const GLOBAL: u8 = 255;
}

/// Standardized information numbers (INF) in the monitor direction.
///
/// See IEC 60870-5-103, subclass 7.2.5.2. The exact semantics are
/// device-specific; these cover the compatible range.
pub mod inf {
    // -- system functions --
    /// 2: reset frame count bit
    pub const RESET_FCB: u8 = 2;
    /// 3: reset communication unit
    pub const RESET_CU: u8 = 3;
    /// 4: start / restart
    pub const START_RESTART: u8 = 4;
    /// 5: power on
    pub const POWER_ON: u8 = 5;

    // -- status indications --
    /// 16: auto-recloser active
    pub const AUTO_RECLOSER_ACTIVE: u8 = 16;
    /// 17: teleprotection active
    pub const TELEPROTECTION_ACTIVE: u8 = 17;
    /// 18: protection active
    pub const PROTECTION_ACTIVE: u8 = 18;
    /// 19: LED reset
    pub const LED_RESET: u8 = 19;
    /// 20: monitor direction blocked
    pub const MONITOR_BLOCKED: u8 = 20;
    /// 21: test mode
    pub const TEST_MODE: u8 = 21;
    /// 22: local parameter setting
    pub const LOCAL_PARAMETER_SET: u8 = 22;
    /// 23: characteristic 1
    pub const CHARACTERISTIC1: u8 = 23;
    /// 24: characteristic 2
    pub const CHARACTERISTIC2: u8 = 24;
    /// 25: characteristic 3
    pub const CHARACTERISTIC3: u8 = 25;
    /// 26: characteristic 4
    pub const CHARACTERISTIC4: u8 = 26;
    /// 27: auxiliary input 1
    pub const AUX_INPUT1: u8 = 27;
    /// 28: auxiliary input 2
    pub const AUX_INPUT2: u8 = 28;
    /// 29: auxiliary input 3
    pub const AUX_INPUT3: u8 = 29;
    /// 30: auxiliary input 4
    pub const AUX_INPUT4: u8 = 30;

    // -- supervision indications --
    /// 32: measurand supervision, current
    pub const MEASURAND_SUPERVISION_I: u8 = 32;
    /// 33: measurand supervision, voltage
    pub const MEASURAND_SUPERVISION_V: u8 = 33;
    /// 35: phase sequence supervision
    pub const PHASE_SEQ_SUPERVISION: u8 = 35;
    /// 36: trip circuit supervision
    pub const TRIP_CIRCUIT_SUPERVISION: u8 = 36;
    /// 37: backup operation
    pub const BACKUP_OPERATION: u8 = 37;
    /// 38: measuring transformer fuse failure
    pub const VT_FUSE_FAILURE: u8 = 38;
    /// 39: teleprotection disturbed
    pub const TELEPROTECTION_DISTURBED: u8 = 39;
    /// 46: group warning
    pub const GROUP_WARNING: u8 = 46;
    /// 47: group alarm
    pub const GROUP_ALARM: u8 = 47;

    // -- earth fault indications --
    /// 48: earth fault L1
    pub const EARTH_FAULT_L1: u8 = 48;
    /// 49: earth fault L2
    pub const EARTH_FAULT_L2: u8 = 49;
    /// 50: earth fault L3
    pub const EARTH_FAULT_L3: u8 = 50;
    /// 51: earth fault forward
    pub const EARTH_FAULT_FORWARD: u8 = 51;
    /// 52: earth fault reverse
    pub const EARTH_FAULT_REVERSE: u8 = 52;

    // -- fault indications --
    /// 64: start / pick-up L1
    pub const START_L1: u8 = 64;
    /// 65: start / pick-up L2
    pub const START_L2: u8 = 65;
    /// 66: start / pick-up L3
    pub const START_L3: u8 = 66;
    /// 67: start / pick-up N
    pub const START_N: u8 = 67;
    /// 68: general trip
    pub const GENERAL_TRIP: u8 = 68;
    /// 69: trip L1
    pub const TRIP_L1: u8 = 69;
    /// 70: trip L2
    pub const TRIP_L2: u8 = 70;
    /// 71: trip L3
    pub const TRIP_L3: u8 = 71;
    /// 72: trip by backup protection
    pub const TRIP_BACKUP: u8 = 72;
    /// 73: fault location in ohms
    pub const FAULT_LOCATION: u8 = 73;
    /// 74: fault forward
    pub const FAULT_FORWARD: u8 = 74;
    /// 75: fault reverse
    pub const FAULT_REVERSE: u8 = 75;
    /// 76: teleprotection signal transmitted
    pub const TELEPROT_SENT: u8 = 76;
    /// 77: teleprotection signal received
    pub const TELEPROT_RECEIVED: u8 = 77;
    /// 78: protection zone 1
    pub const ZONE1: u8 = 78;
    /// 79: protection zone 2
    pub const ZONE2: u8 = 79;
    /// 80: protection zone 3
    pub const ZONE3: u8 = 80;
    /// 81: protection zone 4
    pub const ZONE4: u8 = 81;
    /// 82: protection zone 5
    pub const ZONE5: u8 = 82;
    /// 83: protection zone 6
    pub const ZONE6: u8 = 83;
    /// 84: general start of operation
    pub const GENERAL_START: u8 = 84;
    /// 85: breaker failure
    pub const BREAKER_FAILURE: u8 = 85;

    // -- auto-reclosure indications --
    /// 128: circuit breaker on by auto-recloser
    pub const CB_ON_BY_AR: u8 = 128;
    /// 129: circuit breaker on by long-time auto-recloser
    pub const CB_ON_BY_LONG_AR: u8 = 129;
    /// 130: auto-recloser blocked
    pub const AR_BLOCKED: u8 = 130;

    // -- measurands --
    /// 144: measurand I
    pub const MEASURAND_I: u8 = 144;
    /// 145: measurands I, V
    pub const MEASURAND_IV: u8 = 145;
    /// 146: measurands I, V, P, Q
    pub const MEASURAND_IVPQ: u8 = 146;
    /// 147: measurands IN, VEN
    pub const MEASURAND_INVEN: u8 = 147;
    /// 148: measurands IL1..3, VL1..3, P, Q, f
    pub const MEASURAND_IL123VL123: u8 = 148;
}

/// Double-point information of IEC 60870-5-103, two bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Dpi {
    /// 0: transient / not used
    #[default]
    Transient = 0,
    /// 1: off
    Off = 1,
    /// 2: on
    On = 2,
    /// 3: indeterminate
    Unknown = 3,
}

impl Dpi {
    /// Decode from the low two bits of an octet.
    pub const fn parse(b: u8) -> Dpi {
        match b & 0x03 {
            0 => Dpi::Transient,
            1 => Dpi::Off,
            2 => Dpi::On,
            _ => Dpi::Unknown,
        }
    }

    /// Encode to the two-bit value.
    pub const fn value(self) -> u8 {
        self as u8
    }
}

impl fmt::Display for Dpi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Dpi::Transient => "Transient",
            Dpi::Off => "Off",
            Dpi::On => "On",
            Dpi::Unknown => "Unknown",
        })
    }
}

/// Double command of the general command ASDU (type 20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Dco {
    /// 0: not permitted
    #[default]
    Invalid = 0,
    /// 1: off
    Off = 1,
    /// 2: on
    On = 2,
}

impl Dco {
    /// Decode from the low two bits of an octet.
    pub const fn parse(b: u8) -> Dco {
        match b & 0x03 {
            1 => Dco::Off,
            2 => Dco::On,
            _ => Dco::Invalid,
        }
    }

    /// Encode to the two-bit value.
    pub const fn value(self) -> u8 {
        self as u8
    }
}

impl fmt::Display for Dco {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Dco::Off => "Off",
            Dco::On => "On",
            Dco::Invalid => "Invalid",
        })
    }
}

/// One IEC 60870-5-103 measurand: a signed 13 bit value with overflow and
/// error flags.
///
/// See IEC 60870-5-103, subclass 7.2.6.8:
///
/// ```text
/// bit 0: OV overflow, bit 1: ER error, bit 2: reserved,
/// bits 3..15: MVAL, signed, -4096..4095
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Measurand {
    /// The measured value, `-4096..=4095`.
    pub val: i16,
    /// OV: the value overflowed its range.
    pub overflow: bool,
    /// ER: the value is erroneous or not available.
    pub invalid: bool,
}

impl Measurand {
    /// Decode a little-endian measurand octet pair.
    pub const fn parse(u: u16) -> Measurand {
        Measurand {
            // An arithmetic shift of the signed reinterpretation keeps the sign.
            val: (u as i16) >> 3,
            overflow: u & 0x01 != 0,
            invalid: u & 0x02 != 0,
        }
    }

    /// Encode to the octet pair value.
    pub const fn value(self) -> u16 {
        let mut u = (self.val as u16) << 3;
        if self.overflow {
            u |= 0x01;
        }
        if self.invalid {
            u |= 0x02;
        }
        u
    }

    /// The measurand as a fraction of full scale, in `[-1, 1)`.
    ///
    /// The rated value corresponds to 1/1.2 or 1/2.4 of full scale, depending
    /// on how the device is parameterised.
    pub fn f64(self) -> f64 {
        self.val as f64 / 4096.0
    }
}

/// Octet length of a CP32Time2a time tag.
pub const CP32TIME2A_SIZE: usize = 4;

/// Encode an instant as a 4-octet CP32Time2a tag: milliseconds, minutes and
/// hours. See IEC 60870-5-4.
pub fn cp32time2a(t: Option<DateTime<Utc>>, zone: TimeZone) -> [u8; CP32TIME2A_SIZE] {
    let Some(t) = t else {
        // The IV bit in the minutes octet marks the tag invalid.
        return [0, 0, 0x80, 0];
    };
    let (_, _, _, _, hour, min, sec, milli) = zone.parts(t);
    let msec = milli + sec * 1000;
    [msec as u8, (msec >> 8) as u8, min as u8, hour as u8]
}

/// Decode a 4-octet CP32Time2a tag.
///
/// The tag carries only the time of day, so the date comes from the host clock.
/// A time of day more than five minutes *ahead* of now is taken to belong to
/// the previous day, which keeps events that cross midnight in order. An
/// invalid (IV) or short tag decodes to `None`.
pub fn parse_cp32time2a(b: &[u8], zone: TimeZone) -> Option<DateTime<Utc>> {
    if b.len() < CP32TIME2A_SIZE || b[2] & 0x80 != 0 {
        return None;
    }
    let x = u16::from_le_bytes([b[0], b[1]]) as u32;
    let msec = x % 1000;
    let sec = x / 1000;
    let min = (b[2] & 0x3f) as u32;
    let hour = (b[3] & 0x1f) as u32;

    let (year, month, day, _, _) = zone.now_parts();
    let val = zone.instant_from(year, month, day, hour, min, sec, msec)?;
    if val > Utc::now() + Duration::minutes(5) {
        Some(val - Duration::days(1))
    } else {
        Some(val)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone as _, Timelike};

    #[test]
    fn measurand_sign_extends_the_thirteen_bit_field() {
        for v in [-4096i16, -1, 0, 1, 4095] {
            for overflow in [false, true] {
                for invalid in [false, true] {
                    let m = Measurand {
                        val: v,
                        overflow,
                        invalid,
                    };
                    assert_eq!(Measurand::parse(m.value()), m, "val {v}");
                }
            }
        }
        assert_eq!(Measurand::parse(0xffff).val, -1);
        assert_eq!(Measurand::parse(0x8000).val, -4096);
        assert_eq!(Measurand::parse(0x7ff8).val, 4095);
    }

    #[test]
    fn measurand_scales_to_full_scale_fraction() {
        assert_eq!(Measurand { val: 4096 / 2, ..Default::default() }.f64(), 0.5);
        assert_eq!(Measurand { val: -4096, ..Default::default() }.f64(), -1.0);
        assert_eq!(Measurand::default().f64(), 0.0);
    }

    #[test]
    fn dpi_and_dco_round_trip() {
        for b in 0u8..4 {
            assert_eq!(Dpi::parse(b).value(), b);
        }
        assert_eq!(Dco::parse(1), Dco::Off);
        assert_eq!(Dco::parse(2), Dco::On);
        assert_eq!(Dco::parse(0), Dco::Invalid);
        assert_eq!(Dco::parse(3), Dco::Invalid);
    }

    #[test]
    fn cp32_encodes_the_time_of_day() {
        let t = Utc.with_ymd_and_hms(2026, 8, 17, 21, 17, 45).unwrap()
            + Duration::milliseconds(678);
        let b = cp32time2a(Some(t), TimeZone::Utc);
        let msec = 45 * 1000 + 678u32;
        assert_eq!(b, [msec as u8, (msec >> 8) as u8, 17, 21]);
    }

    #[test]
    fn cp32_round_trips_the_time_of_day_of_now() {
        let now = Utc::now();
        let b = cp32time2a(Some(now), TimeZone::Utc);
        let got = parse_cp32time2a(&b, TimeZone::Utc).expect("valid tag");
        assert_eq!(got.hour(), now.hour());
        assert_eq!(got.minute(), now.minute());
        assert_eq!(got.second(), now.second());
    }

    #[test]
    fn cp32_rejects_invalid_and_short_tags() {
        assert_eq!(parse_cp32time2a(&[0, 0, 0], TimeZone::Utc), None);
        assert_eq!(parse_cp32time2a(&[0, 0, 0x80, 0], TimeZone::Utc), None);
        assert_eq!(cp32time2a(None, TimeZone::Utc)[2] & 0x80, 0x80);
    }

    #[test]
    fn a_time_of_day_far_ahead_belongs_to_the_previous_day() {
        // Build a tag one hour ahead of now; it must decode to yesterday.
        let ahead = Utc::now() + Duration::hours(1);
        let b = cp32time2a(Some(ahead), TimeZone::Utc);
        let got = parse_cp32time2a(&b, TimeZone::Utc).expect("valid tag");
        assert!(got < Utc::now(), "{got} should be in the past");
        assert_eq!(got.hour(), ahead.hour());
    }
}
