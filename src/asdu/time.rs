// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Binary time tags CP56Time2a, CP24Time2a and CP16Time2a.
//!
//! ```text
//! |         Milliseconds(D7--D0)        | Milliseconds = 0..59999
//! |         Milliseconds(D15--D8)       |
//! | IV(D7)   RES1(D6)  Minutes(D5--D0)  | Minutes = 0..59, IV: 0 = valid, 1 = invalid
//! | SU(D7)   RES2(D6-D5)  Hours(D4--D0) | Hours = 0..23, SU: 0 = standard, 1 = summer time
//! | DayOfWeek(D7--D5) DayOfMonth(D4--D0)| DayOfMonth = 1..31, DayOfWeek = 1..7
//! | RES3(D7--D4)        Months(D3--D0)  | Months = 1..12
//! | RES4(D7)            Year(D6--D0)    | Year = 0..99
//! ```
//!
//! An invalid (IV bit set) or truncated tag decodes to `None`, which is the
//! equivalent of Go's zero `time.Time`.

use chrono::{DateTime, Utc};

use crate::asdu::params::TimeZone;

/// Octet length of a CP56Time2a time tag.
pub const CP56TIME2A_SIZE: usize = 7;
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
        hour as u8,
        ((dow as u8) << 5) | (day as u8),
        month as u8,
        (year - 2000).rem_euclid(100) as u8,
    ]
}

/// Decode a 7-octet CP56Time2a tag interpreted in the given zone.
///
/// Returns `None` when the buffer is short, the IV bit is set, or the calendar
/// fields do not name a real instant.
pub fn parse_cp56time2a(b: &[u8], zone: TimeZone) -> Option<DateTime<Utc>> {
    if b.len() < CP56TIME2A_SIZE || b[2] & 0x80 == 0x80 {
        return None;
    }
    let x = u16::from_le_bytes([b[0], b[1]]) as u32;
    let msec = x % 1000;
    let sec = x / 1000;
    let min = (b[2] & 0x3f) as u32;
    let hour = (b[3] & 0x1f) as u32;
    let day = (b[4] & 0x1f) as u32;
    let month = (b[5] & 0x0f) as u32;
    let year = 2000 + (b[6] & 0x7f) as i32;
    zone.instant_from(year, month, day, hour, min, sec, msec)
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

/// Decode a 3-octet CP24Time2a tag interpreted in the given zone.
///
/// CP24Time2a carries only minutes and milliseconds; the date and hour come
/// from the host clock. A tag whose minute is more than five minutes *ahead*
/// of the current minute is taken to belong to the previous hour, which keeps
/// events that cross an hour boundary in order.
pub fn parse_cp24time2a(b: &[u8], zone: TimeZone) -> Option<DateTime<Utc>> {
    if b.len() < CP24TIME2A_SIZE || b[2] & 0x80 == 0x80 {
        return None;
    }
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
    use chrono::TimeZone as _;

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

    #[test]
    fn cp16_round_trips() {
        for ms in [0u16, 1, 999, 59_999, u16::MAX] {
            assert_eq!(parse_cp16time2a(&cp16time2a(ms)), ms);
        }
    }
}
