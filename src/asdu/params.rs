// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! System parameters that fix the on-wire widths of the data unit identifier.

use chrono::{DateTime, FixedOffset, Local, TimeZone as _, Utc};

use crate::asdu::identifier::{CommonAddr, INVALID_COMMON_ADDR, OriginAddr};
use crate::error::{Error, Result};

/// Time zone used to encode and decode CP24/CP56 time tags.
///
/// The standard leaves the interpretation open; UTC is strongly recommended and
/// is the default of every endpoint in this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeZone {
    /// Encode and decode time tags in UTC (default, recommended).
    #[default]
    Utc,
    /// Encode and decode time tags in the host's local time zone.
    Local,
    /// Encode and decode time tags at a fixed offset from UTC.
    Fixed(FixedOffset),
}

impl TimeZone {
    /// Split an instant into the broken-down calendar fields of this zone.
    ///
    /// Returned as `(year, month, day, weekday_iso, hour, minute, second, millisecond)`
    /// where `weekday_iso` is 1 = Monday .. 7 = Sunday per IEC 60870-5-4 § 6.8.
    pub(crate) fn parts(&self, t: DateTime<Utc>) -> (i32, u32, u32, u32, u32, u32, u32, u32) {
        use chrono::{Datelike, Timelike};
        macro_rules! split {
            ($local:expr) => {{
                let l = $local;
                (
                    l.year(),
                    l.month(),
                    l.day(),
                    l.weekday().number_from_monday(),
                    l.hour(),
                    l.minute(),
                    l.second(),
                    l.timestamp_subsec_millis().min(999),
                )
            }};
        }
        match self {
            TimeZone::Utc => split!(t),
            TimeZone::Local => split!(t.with_timezone(&Local)),
            TimeZone::Fixed(off) => split!(t.with_timezone(off)),
        }
    }

    /// Rebuild an instant from broken-down calendar fields interpreted in this zone.
    ///
    /// Ambiguous or non-existent local times (DST transitions) resolve to the
    /// earliest valid instant, mirroring Go's `time.Date` normalisation.
    // Broken-down calendar fields; grouping them into a struct would only move
    // the same seven values behind a name used in exactly three places.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn instant_from(
        &self,
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        min: u32,
        sec: u32,
        milli: u32,
    ) -> Option<DateTime<Utc>> {
        let nano = milli.checked_mul(1_000_000)?;
        match self {
            TimeZone::Utc => Utc
                .with_ymd_and_hms(year, month, day, hour, min, sec)
                .single()
                .and_then(|t| t.with_nanosecond(nano)),
            TimeZone::Local => Local
                .with_ymd_and_hms(year, month, day, hour, min, sec)
                .earliest()
                .and_then(|t| t.with_nanosecond(nano))
                .map(|t| t.with_timezone(&Utc)),
            TimeZone::Fixed(off) => off
                .with_ymd_and_hms(year, month, day, hour, min, sec)
                .earliest()
                .and_then(|t| t.with_nanosecond(nano))
                .map(|t| t.with_timezone(&Utc)),
        }
    }

    /// "Now" as broken-down fields in this zone, used to complete CP24 time tags.
    pub(crate) fn now_parts(&self) -> (i32, u32, u32, u32, u32) {
        use chrono::{Datelike, Timelike};
        macro_rules! split {
            ($local:expr) => {{
                let l = $local;
                (l.year(), l.month(), l.day(), l.hour(), l.minute())
            }};
        }
        let now = Utc::now();
        match self {
            TimeZone::Utc => split!(now),
            TimeZone::Local => split!(now.with_timezone(&Local)),
            TimeZone::Fixed(off) => split!(now.with_timezone(off)),
        }
    }
}

use chrono::Timelike as _;

/// Specific parameters related to an ASDU.
///
/// See companion standard 101, subclass 7.1. **Both peers must be configured
/// identically** or every ASDU will be mis-parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    /// Octets of the cause of transmission field, 1 or 2.
    /// A width of 2 includes (activates) the originator address.
    pub cause_size: u8,
    /// Originator address `[1, 255]`, or 0 for the default.
    /// Only present on the wire when `cause_size == 2`.
    pub orig_address: OriginAddr,
    /// Octets of the ASDU common (station) address, 1 or 2.
    pub common_addr_size: u8,
    /// Octets of the information object address, 1, 2 or 3.
    pub info_obj_addr_size: u8,
    /// Time zone used to interpret CP24/CP56 time tags.
    pub info_obj_time_zone: TimeZone,
}

/// The smallest configuration: COT 1, CA 1, IOA 1.
pub const PARAMS_NARROW: Params = Params {
    cause_size: 1,
    orig_address: 0,
    common_addr_size: 1,
    info_obj_addr_size: 1,
    info_obj_time_zone: TimeZone::Utc,
};

/// The standard configuration for IEC 60870-5-101: COT 1, CA 1, IOA 2.
pub const PARAMS_STANDARD_101: Params = Params {
    cause_size: 1,
    orig_address: 0,
    common_addr_size: 1,
    info_obj_addr_size: 2,
    info_obj_time_zone: TimeZone::Utc,
};

/// The largest configuration: COT 2 (with originator address), CA 2, IOA 3.
///
/// This is the standard layout for IEC 60870-5-104.
pub const PARAMS_WIDE: Params = Params {
    cause_size: 2,
    orig_address: 0,
    common_addr_size: 2,
    info_obj_addr_size: 3,
    info_obj_time_zone: TimeZone::Utc,
};

/// Alias of [`PARAMS_WIDE`], the IEC 60870-5-104 standard layout.
pub const PARAMS_STANDARD_104: Params = PARAMS_WIDE;

impl Default for Params {
    fn default() -> Self {
        PARAMS_WIDE
    }
}

impl Params {
    /// Validate the field widths.
    pub fn valid(&self) -> Result<()> {
        if !(1..=2).contains(&self.cause_size)
            || !(1..=2).contains(&self.common_addr_size)
            || !(1..=3).contains(&self.info_obj_addr_size)
        {
            return Err(Error::Param);
        }
        Ok(())
    }

    /// Validate a station common address against these parameters.
    pub fn valid_common_addr(&self, addr: CommonAddr) -> Result<()> {
        if addr == INVALID_COMMON_ADDR {
            return Err(Error::CommonAddrZero);
        }
        if (u16::BITS - addr.leading_zeros()) as u8 > self.common_addr_size * 8 {
            return Err(Error::CommonAddrFit);
        }
        Ok(())
    }

    /// Size in octets of the application service data unit identifier.
    pub fn identifier_size(&self) -> usize {
        2 + self.cause_size as usize + self.common_addr_size as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_sizes_match_the_standard_layouts() {
        assert_eq!(PARAMS_NARROW.identifier_size(), 4);
        assert_eq!(PARAMS_STANDARD_101.identifier_size(), 4);
        assert_eq!(PARAMS_WIDE.identifier_size(), 6);
    }

    #[test]
    fn params_reject_out_of_range_widths() {
        assert!(PARAMS_WIDE.valid().is_ok());
        let bad = Params {
            info_obj_addr_size: 4,
            ..PARAMS_WIDE
        };
        assert!(bad.valid().is_err());
    }

    #[test]
    fn common_addr_must_fit_the_configured_width() {
        assert!(PARAMS_STANDARD_101.valid_common_addr(0).is_err());
        assert!(PARAMS_STANDARD_101.valid_common_addr(255).is_ok());
        assert!(PARAMS_STANDARD_101.valid_common_addr(256).is_err());
        assert!(PARAMS_WIDE.valid_common_addr(65535).is_ok());
    }
}
