// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Binary time tags CP56Time2a, CP24Time2a and CP16Time2a.
//!
//! ```text
//! |         Milliseconds(D7--D0)        | Milliseconds = 0..59999
//! |         Milliseconds(D15--D8)       |
//! | IV(D7)   SB(D6)    Minutes(D5--D0)  | Minutes = 0..59, IV: 0 = valid, 1 = invalid
//! | SU(D7)   RES2(D6-D5)  Hours(D4--D0) | Hours = 0..23, SU: 0 = standard, 1 = summer time
//! | DayOfWeek(D7--D5) DayOfMonth(D4--D0)| DayOfMonth = 1..31, DayOfWeek = 1..7
//! | RES3(D7--D4)        Months(D3--D0)  | Months = 1..12
//! | RES4(D7)            Year(D6--D0)    | Year = 0..99
//! ```
//!
//! Bit 6 of the minutes octet, reserved in IEC 60870-5-4, is SB in the
//! companion standards: the time was substituted by an intermediate station
//! rather than tagged by the one that acquired the value. CP24Time2a and
//! CP32Time2a carry IV and SB in the same octet.
//!
//! Two decoders are offered. [`parse_cp56time2a`] returns only a time that can
//! be trusted — `None` for an invalid tag — which is what a clock
//! synchronization must use. [`parse_cp56time2a_tag`] returns the reading
//! whatever its validity, together with the [`TimeTagFlags`], which is what an
//! event log needs: a device whose clock has not been synchronized yet still
//! tags its events, and the order of those tags is worth keeping even when
//! their absolute value is not.

use chrono::{DateTime, Utc};

use crate::asdu::params::TimeZone;

/// Octet length of a CP56Time2a time tag.
pub const CP56TIME2A_SIZE: usize = 7;

/// The validity flags of a CP24Time2a, CP32Time2a or CP56Time2a time tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TimeTagFlags {
    /// IV: the time is invalid — the station's clock was not synchronized, or
    /// could not be read. The reading may still be carried, but must not be
    /// taken as the time of the event.
    pub invalid: bool,
    /// SB: the time was substituted by an intermediate station, not tagged by
    /// the station that acquired the value.
    pub substituted: bool,
}

impl TimeTagFlags {
    /// A time that is neither invalid nor substituted.
    pub const GOOD: TimeTagFlags = TimeTagFlags {
        invalid: false,
        substituted: false,
    };

    /// True when the time may be taken as the time of the event.
    pub const fn is_valid(self) -> bool {
        !self.invalid
    }

    /// Decode from the minutes octet, where IV is bit 7 and SB bit 6.
    pub const fn from_minutes_octet(b: u8) -> Self {
        TimeTagFlags {
            invalid: b & 0x80 != 0,
            substituted: b & 0x40 != 0,
        }
    }

    /// The IV and SB bits of the minutes octet.
    pub const fn minutes_bits(self) -> u8 {
        (if self.invalid { 0x80 } else { 0 }) | (if self.substituted { 0x40 } else { 0 })
    }
}
/// Octet length of a CP24Time2a time tag.
pub const CP24TIME2A_SIZE: usize = 3;
/// Octet length of a CP16Time2a time tag.
pub const CP16TIME2A_SIZE: usize = 2;

/// Encode an instant as a 7-octet CP56Time2a tag in the given zone.
///
/// A `None` instant encodes as all-zero octets with the IV (invalid) bit set,
/// which is how the standard signals "no valid time".
pub fn cp56time2a(t: Option<DateTime<Utc>>, zone: TimeZone) -> [u8; CP56TIME2A_SIZE] {
    let Some(t) = t else {
        // IV bit set in the minutes octet marks the tag invalid.
        return [0, 0, 0x80, 0, 0, 0, 0];
    };
    let (year, month, day, dow, hour, min, sec, milli) = zone.parts(t);
    let msec = milli + sec * 1000;
    [
        msec as u8,
        (msec >> 8) as u8,
        min as u8,
        // D7 of the hour octet is SU: the reading is expressed in summer time.
        hour as u8 | if zone.is_dst(t) { 0x80 } else { 0 },
        ((dow as u8) << 5) | (day as u8),
        month as u8,
        (year - 2000).rem_euclid(100) as u8,
    ]
}

/// Encode an instant as a CP56Time2a tag carrying the given validity flags.
///
/// A `None` instant is encoded as by [`cp56time2a`], with IV set whatever
/// `flags` says, since there is no time to be valid.
pub fn cp56time2a_tag(
    t: Option<DateTime<Utc>>,
    flags: TimeTagFlags,
    zone: TimeZone,
) -> [u8; CP56TIME2A_SIZE] {
    let mut b = cp56time2a(t, zone);
    b[2] |= flags.minutes_bits();
    b
}

/// Decode a 7-octet CP56Time2a tag interpreted in the given zone.
///
/// Returns `None` when the buffer is short, the IV bit is set, or the calendar
/// fields do not name a real instant: only a time that can be trusted.
pub fn parse_cp56time2a(b: &[u8], zone: TimeZone) -> Option<DateTime<Utc>> {
    if b.len() < CP56TIME2A_SIZE || b[2] & 0x80 == 0x80 {
        return None;
    }
    decode_cp56(b, zone)
}

/// Decode a CP56Time2a tag into its reading and its validity flags.
///
/// The reading is returned even when IV is set, as long as the calendar fields
/// name a real instant; `None` means the octets hold no time at all.
pub fn parse_cp56time2a_tag(b: &[u8], zone: TimeZone) -> (Option<DateTime<Utc>>, TimeTagFlags) {
    if b.len() < CP56TIME2A_SIZE {
        return (None, TimeTagFlags::default());
    }
    (decode_cp56(b, zone), TimeTagFlags::from_minutes_octet(b[2]))
}

fn decode_cp56(b: &[u8], zone: TimeZone) -> Option<DateTime<Utc>> {
    let x = u16::from_le_bytes([b[0], b[1]]) as u32;
    let msec = x % 1000;
    let sec = x / 1000;
    let min = (b[2] & 0x3f) as u32;
    let hour = (b[3] & 0x1f) as u32;
    let day = (b[4] & 0x1f) as u32;
    let month = (b[5] & 0x0f) as u32;
    let year = 2000 + (b[6] & 0x7f) as i32;
    // The year field is 7 bits, so it reaches 2127; the standard defines it as
    // 0..99 within the century. A value beyond that is a fault in the sender,
    // not a time.
    if year > 2099 {
        return None;
    }
    let t = zone.instant_from(year, month, day, hour, min, sec, msec)?;
    Some(zone.resolve_summer_time(t, b[3] & 0x80 != 0))
}

/// Encode an instant as a 3-octet CP24Time2a tag (minutes and milliseconds only).
pub fn cp24time2a(t: Option<DateTime<Utc>>, zone: TimeZone) -> [u8; CP24TIME2A_SIZE] {
    let Some(t) = t else {
        return [0, 0, 0x80];
    };
    let (_, _, _, _, _, min, sec, milli) = zone.parts(t);
    let msec = milli + sec * 1000;
    [msec as u8, (msec >> 8) as u8, min as u8]
}

/// Encode an instant as a CP24Time2a tag carrying the given validity flags.
pub fn cp24time2a_tag(
    t: Option<DateTime<Utc>>,
    flags: TimeTagFlags,
    zone: TimeZone,
) -> [u8; CP24TIME2A_SIZE] {
    let mut b = cp24time2a(t, zone);
    b[2] |= flags.minutes_bits();
    b
}

/// Decode a 3-octet CP24Time2a tag interpreted in the given zone.
///
/// CP24Time2a carries only minutes and milliseconds; the date and hour come
/// from the host clock. A tag whose minute is more than five minutes *ahead*
/// of the current minute is taken to belong to the previous hour, which keeps
/// events that cross an hour boundary in order.
///
/// Returns `None` for an invalid tag; see [`parse_cp24time2a_tag`] for the
/// reading regardless of validity.
pub fn parse_cp24time2a(b: &[u8], zone: TimeZone) -> Option<DateTime<Utc>> {
    if b.len() < CP24TIME2A_SIZE || b[2] & 0x80 == 0x80 {
        return None;
    }
    decode_cp24(b, zone)
}

/// Decode a CP24Time2a tag into its reading and its validity flags.
pub fn parse_cp24time2a_tag(b: &[u8], zone: TimeZone) -> (Option<DateTime<Utc>>, TimeTagFlags) {
    if b.len() < CP24TIME2A_SIZE {
        return (None, TimeTagFlags::default());
    }
    (decode_cp24(b, zone), TimeTagFlags::from_minutes_octet(b[2]))
}

fn decode_cp24(b: &[u8], zone: TimeZone) -> Option<DateTime<Utc>> {
    let x = u16::from_le_bytes([b[0], b[1]]) as u32;
    let msec = x % 1000;
    let sec = x / 1000;
    let min = (b[2] & 0x3f) as u32;

    let (year, month, day, hour, current_min) = zone.now_parts();
    let val = zone.instant_from(year, month, day, hour, min, sec, msec)?;
    if min > current_min + 5 {
        Some(val - chrono::Duration::hours(1))
    } else {
        Some(val)
    }
}

/// Encode a millisecond count as a 2-octet CP16Time2a tag.
pub fn cp16time2a(msec: u16) -> [u8; CP16TIME2A_SIZE] {
    msec.to_le_bytes()
}

/// Decode a 2-octet CP16Time2a tag into a millisecond count.
pub fn parse_cp16time2a(b: &[u8]) -> u16 {
    if b.len() < CP16TIME2A_SIZE {
        return 0;
    }
    u16::from_le_bytes([b[0], b[1]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone as _};

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32, ms: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s)
            .unwrap()
            .with_timezone(&Utc)
            + chrono::Duration::milliseconds(ms as i64)
    }

    #[test]
    fn cp56_encodes_the_documented_octet_layout() {
        // 2019-06-05 (Wednesday) 21:17:45.678 UTC
        let t = utc(2019, 6, 5, 21, 17, 45, 678);
        let b = cp56time2a(Some(t), TimeZone::Utc);
        let msec = 45 * 1000 + 678u32;
        assert_eq!(b[0], msec as u8);
        assert_eq!(b[1], (msec >> 8) as u8);
        assert_eq!(b[2], 17);
        assert_eq!(b[3], 21);
        assert_eq!(b[4] & 0x1f, 5);
        assert_eq!(b[4] >> 5, 3, "Wednesday is ISO weekday 3");
        assert_eq!(b[5], 6);
        assert_eq!(b[6], 19);
    }

    #[test]
    fn cp56_round_trips_to_millisecond_resolution() {
        for t in [
            utc(2000, 1, 1, 0, 0, 0, 0),
            utc(2026, 12, 31, 23, 59, 59, 999),
            utc(2024, 2, 29, 12, 0, 0, 500),
        ] {
            let b = cp56time2a(Some(t), TimeZone::Utc);
            assert_eq!(parse_cp56time2a(&b, TimeZone::Utc), Some(t));
        }
    }

    #[test]
    fn cp56_sunday_maps_to_iso_weekday_seven() {
        // 2026-08-16 is a Sunday.
        let b = cp56time2a(Some(utc(2026, 8, 16, 0, 0, 0, 0)), TimeZone::Utc);
        assert_eq!(b[4] >> 5, 7);
    }

    #[test]
    fn cp56_rejects_invalid_and_short_tags() {
        assert_eq!(parse_cp56time2a(&[0; 6], TimeZone::Utc), None);
        let mut b = cp56time2a(Some(utc(2020, 1, 2, 3, 4, 5, 6)), TimeZone::Utc);
        b[2] |= 0x80; // IV
        assert_eq!(parse_cp56time2a(&b, TimeZone::Utc), None);
        assert_eq!(cp56time2a(None, TimeZone::Utc)[2] & 0x80, 0x80);
    }

    #[test]
    fn cp24_carries_minutes_and_milliseconds() {
        let t = utc(2021, 3, 4, 5, 6, 7, 800);
        let b = cp24time2a(Some(t), TimeZone::Utc);
        assert_eq!(b, [(7800u32 % 256) as u8, (7800u32 / 256) as u8, 6]);
        assert_eq!(parse_cp16time2a(&b[..2]), 7800);
    }

    #[test]
    fn cp24_round_trips_the_minute_and_second_of_now() {
        use chrono::Timelike;
        let now = Utc::now();
        let b = cp24time2a(Some(now), TimeZone::Utc);
        let got = parse_cp24time2a(&b, TimeZone::Utc).expect("valid tag");
        assert_eq!(got.minute(), now.minute());
        assert_eq!(got.second(), now.second());
    }

    // Bit 7 of the hour octet of CP56Time2a is SU: the reading is expressed in
    // summer time. IEC 60870-5-4 lays the octet out as
    //
    //     | SU(D7) | RES2(D6-D5) | Hours(D4-D0) |
    //
    // Omitting it makes every summer timestamp look like standard time to a
    // peer that honours the flag — an hour of skew, twice a year, in the
    // direction that makes an event look like it happened before its cause.

    #[test]
    fn cp56_leaves_su_clear_for_utc_and_fixed_zones() {
        // Neither UTC nor a fixed offset observes summer time.
        let midsummer = utc(2026, 7, 1, 12, 0, 0, 0);
        for zone in [
            TimeZone::Utc,
            TimeZone::Fixed(FixedOffset::east_opt(2 * 3600).unwrap()),
        ] {
            assert_eq!(cp56time2a(Some(midsummer), zone)[3] & 0x80, 0, "{zone:?}");
        }
    }

    /// A named zone, so these do not depend on how the host is configured.
    /// Berlin is +01:00 standard / +02:00 summer, Sydney +10:00 / +11:00 with
    /// the seasons reversed, and Kolkata +05:30 all year.
    #[cfg(feature = "tz")]
    fn named(tz: chrono_tz::Tz) -> TimeZone {
        TimeZone::Named(tz)
    }


    #[cfg(feature = "tz")]
    #[test]
    fn cp56_sets_su_exactly_when_the_zone_is_on_summer_time() {
        use chrono_tz::{Asia::Kolkata, Australia::Sydney, Europe::Berlin};
        // (zone, instant, expected SU). The seasons are reversed in Sydney,
        // and Kolkata never observes summer time.
        for (tz, t, want) in [
            (Berlin, utc(2026, 1, 15, 12, 0, 0, 0), false),
            (Berlin, utc(2026, 7, 15, 12, 0, 0, 0), true),
            (Sydney, utc(2026, 1, 15, 12, 0, 0, 0), true),
            (Sydney, utc(2026, 7, 15, 12, 0, 0, 0), false),
            (Kolkata, utc(2026, 1, 15, 12, 0, 0, 0), false),
            (Kolkata, utc(2026, 7, 15, 12, 0, 0, 0), false),
        ] {
            let zone = named(tz);
            assert_eq!(zone.is_dst(t), want, "{tz} at {t}");
            let su = cp56time2a(Some(t), zone)[3] & 0x80 != 0;
            assert_eq!(su, want, "SU octet for {tz} at {t}");
            // The hour octet must still carry the local hour in D4..D0.
            let (.., hour, _, _, _) = zone.parts(t);
            assert_eq!((cp56time2a(Some(t), zone)[3] & 0x1f) as u32, hour);
        }
    }

    #[cfg(feature = "tz")]
    #[test]
    fn cp56_round_trips_through_a_summer_time_transition() {
        use chrono_tz::{Australia::Sydney, Europe::Berlin};
        // Berlin's clocks go back at 01:00 UTC on 2026-10-25, so 02:30 local
        // occurs twice: once at 00:30 UTC in summer time and once at 01:30 UTC
        // in standard time. SU is the only thing that tells the two apart, and
        // without it the second reading decodes as the first — an hour of skew
        // in the direction that makes an event look like it happened before
        // its cause.
        let ambiguous = [utc(2026, 10, 25, 0, 30, 0, 0), utc(2026, 10, 25, 1, 30, 0, 0)];
        let tags: Vec<_> = ambiguous
            .iter()
            .map(|t| cp56time2a(Some(*t), named(Berlin)))
            .collect();
        assert_eq!(
            tags[0][3] & 0x1f,
            tags[1][3] & 0x1f,
            "the two readings share a wall clock hour"
        );
        assert_ne!(tags[0][3] & 0x80, tags[1][3] & 0x80, "and differ only in SU");

        for (t, tag) in ambiguous.iter().zip(&tags) {
            assert_eq!(parse_cp56time2a(tag, named(Berlin)), Some(*t), "at {t}");
        }

        // And ordinary instants either side of both transitions, in both
        // hemispheres.
        for tz in [Berlin, Sydney] {
            for t in [
                utc(2026, 1, 15, 12, 0, 0, 0),
                utc(2026, 3, 29, 3, 0, 0, 0),
                utc(2026, 7, 15, 12, 0, 0, 0),
                utc(2026, 10, 25, 12, 0, 0, 0),
            ] {
                let b = cp56time2a(Some(t), named(tz));
                assert_eq!(parse_cp56time2a(&b, named(tz)), Some(t), "{tz} at {t}");
            }
        }
    }

    #[cfg(feature = "tz")]
    #[test]
    fn a_tag_from_a_peer_with_different_rules_keeps_its_wall_clock() {
        use chrono_tz::Europe::Berlin;
        // A sender that never sets SU still has to be understood: with no
        // instant matching the flag, the wall clock is the only thing the two
        // ends agree on, so the reading is kept as-is rather than shifted.
        let t = utc(2026, 7, 15, 12, 0, 0, 0);
        let mut b = cp56time2a(Some(t), named(Berlin));
        assert_eq!(b[3] & 0x80, 0x80, "Berlin is on summer time in July");
        b[3] &= 0x7f; // the peer omits SU
        let got = parse_cp56time2a(&b, named(Berlin)).expect("still a valid tag");
        assert_eq!(
            named(Berlin).parts(got),
            named(Berlin).parts(t),
            "the wall clock reading is preserved"
        );
    }

    #[test]
    fn cp56_rejects_a_year_beyond_the_century() {
        // The year field is 7 bits and reaches 2127; the standard defines 0..99.
        let mut b = cp56time2a(Some(utc(2026, 1, 2, 3, 4, 5, 6)), TimeZone::Utc);
        b[6] = 100;
        assert_eq!(parse_cp56time2a(&b, TimeZone::Utc), None);
        b[6] = 99;
        assert!(parse_cp56time2a(&b, TimeZone::Utc).is_some());
    }

    #[test]
    fn an_invalid_tag_keeps_its_reading_for_those_who_ask() {
        let t = utc(2026, 3, 4, 5, 6, 7, 800);
        let flags = TimeTagFlags {
            invalid: true,
            substituted: false,
        };
        let b = cp56time2a_tag(Some(t), flags, TimeZone::Utc);
        assert_eq!(b[2] & 0xc0, 0x80, "IV set, SB clear");
        assert_eq!(b[2] & 0x3f, 6, "the minutes are untouched");

        // The trusted-time decoder refuses it, as a clock sync must...
        assert_eq!(parse_cp56time2a(&b, TimeZone::Utc), None);
        // ...while the tag decoder keeps the reading and says it is invalid.
        assert_eq!(parse_cp56time2a_tag(&b, TimeZone::Utc), (Some(t), flags));
    }

    #[test]
    fn the_substituted_flag_round_trips_and_leaves_the_time_valid() {
        let t = utc(2026, 3, 4, 5, 6, 7, 800);
        let flags = TimeTagFlags {
            invalid: false,
            substituted: true,
        };
        let b = cp56time2a_tag(Some(t), flags, TimeZone::Utc);
        assert_eq!(b[2] & 0xc0, 0x40);
        assert_eq!(parse_cp56time2a_tag(&b, TimeZone::Utc), (Some(t), flags));
        assert_eq!(
            parse_cp56time2a(&b, TimeZone::Utc),
            Some(t),
            "a substituted time is still a valid time"
        );
    }

    #[test]
    fn cp24_carries_the_same_flags_in_its_minutes_octet() {
        for flags in [
            TimeTagFlags::GOOD,
            TimeTagFlags { invalid: true, substituted: false },
            TimeTagFlags { invalid: false, substituted: true },
            TimeTagFlags { invalid: true, substituted: true },
        ] {
            let b = cp24time2a_tag(Some(Utc::now()), flags, TimeZone::Utc);
            let (t, got) = parse_cp24time2a_tag(&b, TimeZone::Utc);
            assert_eq!(got, flags);
            assert!(t.is_some(), "the reading survives {flags:?}");
        }
    }

    #[test]
    fn no_time_is_always_invalid() {
        let b = cp56time2a_tag(None, TimeTagFlags::GOOD, TimeZone::Utc);
        let (t, flags) = parse_cp56time2a_tag(&b, TimeZone::Utc);
        assert_eq!(t, None);
        assert!(flags.invalid);
    }

    #[test]
    fn cp16_round_trips() {
        for ms in [0u16, 1, 999, 59_999, u16::MAX] {
            assert_eq!(parse_cp16time2a(&cp16time2a(ms)), ms);
        }
    }
}
