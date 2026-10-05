//! UTC timestamps without a date library.
//!
//! Two forms: ISO 8601 for people reading a task file, and a fixed-width
//! digit string for file names, where lexical order has to be age order.

use std::time::{SystemTime, UNIX_EPOCH};

fn since_epoch() -> std::time::Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

/// Year, month, day from days since 1970-01-01 (Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `2026-09-27T10:15:00Z`.
pub fn iso_now() -> String {
    secs_to_iso(now_secs())
}

/// `20260927-101500-123456789`: fixed width, so sorting names sorts by age.
pub(crate) fn sortable_now() -> String {
    let t = since_epoch();
    sortable(t.as_secs(), t.subsec_nanos())
}

fn sortable(secs: u64, nanos: u32) -> String {
    let (y, mo, d, h, mi, s) = civil_from_secs(secs as i64);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}-{nanos:09}")
}

/// Seconds since the Unix epoch, now.
pub fn now_secs() -> i64 {
    since_epoch().as_secs() as i64
}

/// Days since 1970-01-01 from a civil date (Hinnant's `days_from_civil`).
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `(year, month, day, hour, minute, second)` for seconds since the epoch.
pub fn civil_from_secs(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let (y, mo, d) = civil(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    (
        y,
        mo,
        d,
        (rem / 3600) as u32,
        (rem % 3600 / 60) as u32,
        (rem % 60) as u32,
    )
}

pub(crate) fn secs_from_civil(y: i64, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    days_from_civil(y, mo, d) * 86_400 + i64::from(h) * 3600 + i64::from(mi) * 60 + i64::from(s)
}

/// `2026-09-27T10:15:00Z` for any instant, including before the epoch.
pub fn secs_to_iso(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_secs(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Reads the `YYYY-MM-DDTHH:MM:SSZ` form this crate and the trail write.
/// Anything else is `None` rather than a guess.
pub fn parse_iso(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[19] != b'Z' {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<u32>().ok();
    let (y, mo, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (h, mi, sec) = (n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    Some(secs_from_civil(i64::from(y), mo, d, h, mi, sec))
}

/// `YYYY-MM`, the calendar month (UTC) an ISO instant falls in: the budget
/// window (ADR 0026 §5).
pub fn month_of(iso: &str) -> Option<String> {
    let s = parse_iso(iso)?;
    let (y, m, ..) = civil_from_secs(s);
    Some(format!("{y:04}-{m:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trips_through_seconds() {
        for s in [0, 951_782_400, 1_790_417_700, 4_102_444_800] {
            assert_eq!(parse_iso(&secs_to_iso(s)), Some(s));
        }
        assert_eq!(parse_iso("2026-09-27 10:15:00"), None, "not guessed at");
        assert_eq!(month_of("2026-12-31T23:59:59Z").as_deref(), Some("2026-12"));
    }

    #[test]
    fn known_instants_format_as_the_calendar_says() {
        assert_eq!(secs_to_iso(0), "1970-01-01T00:00:00Z");
        // 2000-02-29 exercises the leap day a naive formula gets wrong.
        assert_eq!(secs_to_iso(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(secs_to_iso(1_790_417_700), "2026-09-26T10:15:00Z"); // per `date -u -d @…`
        assert_eq!(sortable(0, 5), "19700101-000000-000000005");
    }

    #[test]
    fn sortable_names_sort_by_time_across_a_digit_boundary() {
        // A variable-width stamp sorts 9 s after 10 s.
        assert!(sortable(9, 0) < sortable(10, 0));
        assert!(sortable(10, 999_999_999) < sortable(11, 0));
    }

    #[test]
    fn every_day_for_two_thousand_years_round_trips_through_the_calendar() {
        // Hinnant's formulas have century and 400-year corrections that a
        // handful of dates never reach; a wrong one moves a routine's fire
        // or a budget month by a day.
        for days in -365_250..365_250 {
            let (y, m, d) = civil(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
        assert_eq!(secs_to_iso(951_868_800), "2000-03-01T00:00:00Z");
    }

    #[test]
    fn the_clock_reads_now() {
        // After this crate was written, so not a constant.
        assert!(now_secs() > 1_790_000_000, "{}", now_secs());
    }

    #[test]
    fn a_stamp_wrong_in_any_one_place_is_refused_and_a_leap_second_is_read() {
        assert!(parse_iso("2026-09-27T23:59:59Z").is_some());
        assert!(parse_iso("2026-12-31T23:59:60Z").is_some(), "a leap second");
        for bad in [
            "2026-09-27T10:15:00Z0",
            "2026x09-27T10:15:00Z",
            "2026-09x27T10:15:00Z",
            "2026-09-27x10:15:00Z",
            "2026-09-27T10:15:00x",
            "2026-13-27T10:15:00Z",
            "2026-09-32T10:15:00Z",
            "2026-09-27T24:15:00Z",
            "2026-09-27T10:60:00Z",
            "2026-09-27T10:15:61Z",
        ] {
            assert_eq!(parse_iso(bad), None, "{bad}");
        }
    }
}
