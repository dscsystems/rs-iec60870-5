// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

use chrono::{TimeZone as _, Utc};
use rs_iec60870_5::asdu::{TimeZone, time::*};
#[test]
fn invalid_iec_time_fields_are_rejected() {
    let good = cp56time2a(
        Some(Utc.with_ymd_and_hms(2026, 8, 18, 12, 34, 56).unwrap()),
        TimeZone::Utc,
    );
    for (index, value) in [(2, 60), (3, 24), (4, 0), (5, 0), (6, 100), (0, 255)] {
        let mut bad = good;
        bad[index] = value;
        if index == 0 {
            bad[1] = 255;
        }
        assert_eq!(parse_cp56time2a(&bad, TimeZone::Utc), None, "field {index}");
    }
    assert_eq!(parse_cp24time2a(&[0, 0, 60], TimeZone::Utc), None);
    assert_eq!(parse_cp24time2a(&[255, 255, 10], TimeZone::Utc), None);
    #[cfg(feature = "cs103")]
    assert_eq!(
        rs_iec60870_5::cs103::parse_cp32time2a(&[0, 0, 10, 24], TimeZone::Utc),
        None
    );
}
#[cfg(unix)]
#[test]
fn local_summer_time_round_trips_both_occurrences_of_the_autumn_hour() {
    // Set TZ in an isolated process: changing it in the test process races
    // other local-time users and is unsafe in a multi-threaded program.
    if std::env::var_os("IEC60870_DST_TEST_CHILD").is_none() {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_summer_time_round_trips_both_occurrences_of_the_autumn_hour",
            ])
            .env("TZ", "Europe/Berlin")
            .env("IEC60870_DST_TEST_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        return;
    }
    for (hour, su) in [(0, 128), (1, 0)] {
        let instant = Utc.with_ymd_and_hms(2026, 10, 25, hour, 30, 0).unwrap();
        let tag = cp56time2a(Some(instant), TimeZone::Local);
        assert_eq!(tag[3] & 31, 2);
        assert_eq!(tag[3] & 128, su);
        assert_eq!(parse_cp56time2a(&tag, TimeZone::Local), Some(instant));
        #[cfg(feature = "cs103")]
        assert_eq!(
            rs_iec60870_5::cs103::cp32time2a(Some(instant), TimeZone::Local)[3] & 128,
            su
        );
    }
}
