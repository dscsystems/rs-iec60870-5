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
    /// Encode and decode time tags in a named IANA time zone.
    ///
    /// Unlike [`TimeZone::Local`] this does not depend on how the host is
    /// configured, and unlike [`TimeZone::Fixed`] it observes summer time, so
    /// the SU bit of a CP56Time2a tag is set and honoured. Use it for a device
    /// whose profile fixes a zone the host does not share.
    ///
    /// Requires the `tz` feature.
    #[cfg(feature = "tz")]
    Named(chrono_tz::Tz),
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
            #[cfg(feature = "tz")]
            TimeZone::Named(tz) => split!(t.with_timezone(tz)),
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
        // Build the whole reading first and resolve it against the zone once.
        // Setting the nanoseconds afterwards would re-resolve the local time,
        // and `with_nanosecond` answers `None` for a reading the zone sees
        // twice — which is exactly the hour the SU bit exists to disambiguate,
        // so every tag inside it would decode as "not a time".
        //
        // `and_hms_nano_opt` is also what bounds the fields: the octets are
        // wider than the ranges the standard defines, and an hour of 31 or a
        // minute of 60 is a fault in the sender, not a time.
        let naive = chrono::NaiveDate::from_ymd_opt(year, month, day)?
            .and_hms_nano_opt(hour, min, sec, nano)?;
        match self {
            TimeZone::Utc => Some(Utc.from_utc_datetime(&naive)),
            TimeZone::Local => Local
                .from_local_datetime(&naive)
                .earliest()
                .map(|t| t.with_timezone(&Utc)),
            TimeZone::Fixed(off) => off
                .from_local_datetime(&naive)
                .earliest()
                .map(|t| t.with_timezone(&Utc)),
            #[cfg(feature = "tz")]
            TimeZone::Named(tz) => tz
                .from_local_datetime(&naive)
                .earliest()
                .map(|t| t.with_timezone(&Utc)),
        }
    }

    /// The offset from UTC this zone is at during `t`, in seconds.
    fn offset_secs(&self, t: DateTime<Utc>) -> i32 {
        match self {
            TimeZone::Utc => 0,
            TimeZone::Local => t.with_timezone(&Local).offset().local_minus_utc(),
            TimeZone::Fixed(off) => off.local_minus_utc(),
            #[cfg(feature = "tz")]
            TimeZone::Named(tz) => {
                use chrono::Offset;
                t.with_timezone(tz).offset().fix().local_minus_utc()
            }
        }
    }

    /// Whether this zone is on summer time during `t`.
    ///
    /// This is the SU bit of a CP56Time2a or CP32Time2a tag: the reading is
    /// expressed in summer time, which is what lets a receiver resolve the
    /// hour that occurs twice when the clocks go back. It is always false for
    /// UTC and for any fixed offset, because neither observes summer time.
    ///
    /// A zone is taken to be on summer time when its offset differs from the
    /// smallest offset it uses that year. That is the standard offset in both
    /// hemispheres — January is summer time in Sydney and standard time in
    /// Berlin, and taking the minimum gets both right.
    pub(crate) fn is_dst(&self, t: DateTime<Utc>) -> bool {
        match self {
            TimeZone::Utc | TimeZone::Fixed(_) => return false,
            #[cfg(feature = "tz")]
            TimeZone::Named(_) => {}
            TimeZone::Local => {}
        }
        let (year, ..) = self.parts(t);
        let at = |month| {
            Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0)
                .single()
                .map(|r| self.offset_secs(r))
        };
        let (Some(jan), Some(jul)) = (at(1), at(7)) else {
            return false;
        };
        self.offset_secs(t) != jan.min(jul)
    }

    /// Pick the instant matching a CP56Time2a or CP32Time2a SU flag.
    ///
    /// A wall clock reading is ambiguous for one hour a year: when the clocks
    /// go back the same local time occurs twice, once in summer time and once
    /// in standard time. [`instant_from`](Self::instant_from) resolves that to
    /// the earlier of the two — the standard provides the SU bit to settle it.
    ///
    /// Returns `t` unchanged when it already agrees with `su`, and when no
    /// instant with the same wall clock reading agrees. The latter happens
    /// when the sender's summer time rules differ from this zone's, and there
    /// the wall clock is the only thing the two ends agree on.
    pub(crate) fn resolve_summer_time(&self, t: DateTime<Utc>, su: bool) -> DateTime<Utc> {
        if self.is_dst(t) == su {
            return t;
        }
        let wall = self.parts(t);
        // A summer time offset is an hour almost everywhere and half an hour
        // in a few places; the shifted instant is only the right one if it
        // still reads as the same wall clock.
        for minutes in [-60, 60, -30, 30, -120, 120] {
            let Some(alt) = t.checked_add_signed(chrono::Duration::minutes(minutes)) else {
                continue;
            };
            if self.is_dst(alt) == su && self.parts(alt) == wall {
                return alt;
            }
        }
        t
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
            #[cfg(feature = "tz")]
            TimeZone::Named(tz) => split!(now.with_timezone(tz)),
        }
    }
}

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
    /// Accept an ASDU whose information objects are longer than its variable
    /// structure qualifier accounts for, silently discarding the surplus.
    ///
    /// The default — reject, with [`Error::TrailingOctets`] — is what the
    /// standard implies: an ASDU's length is fixed by the frame that carries
    /// it, its object count by the qualifier and its object size by the type
    /// identification, so a conforming sender cannot produce a surplus octet.
    /// Discarding one means executing a command that arrived in a frame nobody
    /// can account for.
    ///
    /// Set this only for a device that is known to pad, and knowing that a
    /// truncated interrogation reply then looks the same as a complete one.
    pub allow_trailing_octets: bool,
}

/// The smallest configuration: COT 1, CA 1, IOA 1.
pub const PARAMS_NARROW: Params = Params {
    cause_size: 1,
    orig_address: 0,
    common_addr_size: 1,
    info_obj_addr_size: 1,
    info_obj_time_zone: TimeZone::Utc,
    allow_trailing_octets: false,
};

/// The standard configuration for IEC 60870-5-101: COT 1, CA 1, IOA 2.
pub const PARAMS_STANDARD_101: Params = Params {
    cause_size: 1,
    orig_address: 0,
    common_addr_size: 1,
    info_obj_addr_size: 2,
    info_obj_time_zone: TimeZone::Utc,
    allow_trailing_octets: false,
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
    allow_trailing_octets: false,
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
