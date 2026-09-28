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

fn parts(secs: u64) -> (i64, u32, u32, u64, u64, u64) {
    let (y, mo, d) = civil((secs / 86_400) as i64);
    let rem = secs % 86_400;
    (y, mo, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// `2026-09-27T10:15:00Z`.
pub fn iso_now() -> String {
    iso(since_epoch().as_secs())
}

fn iso(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = parts(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `20260927-101500-123456789`: fixed width, so sorting names sorts by age.
pub(crate) fn sortable_now() -> String {
    let t = since_epoch();
    sortable(t.as_secs(), t.subsec_nanos())
}

fn sortable(secs: u64, nanos: u32) -> String {
    let (y, mo, d, h, mi, s) = parts(secs);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}-{nanos:09}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_instants_format_as_the_calendar_says() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        // 2000-02-29 exercises the leap day a naive formula gets wrong.
        assert_eq!(iso(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso(1_790_417_700), "2026-09-26T10:15:00Z"); // per `date -u -d @…`
        assert_eq!(sortable(0, 5), "19700101-000000-000000005");
    }

    #[test]
    fn sortable_names_sort_by_time_across_a_digit_boundary() {
        // A variable-width stamp sorts 9 s after 10 s.
        assert!(sortable(9, 0) < sortable(10, 0));
        assert!(sortable(10, 999_999_999) < sortable(11, 0));
    }
}
