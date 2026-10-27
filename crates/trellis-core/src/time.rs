//! Proleptic Gregorian dates from day counts and back, std only.
//!
//! The queue's timestamps and the ticket sweep's `triaged_at` both need it,
//! the workspace has no date crate, and trellis-tickets may depend only on
//! this crate (ADR 0040), so one copy of Howard Hinnant's algorithms lives
//! here rather than one per caller.

/// Year, month, day from days since 1970-01-01 (Hinnant's `civil_from_days`).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
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

/// Days since 1970-01-01 from a civil date (Hinnant's `days_from_civil`).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_epoch_a_leap_day_and_a_pre_epoch_day_map_both_ways() {
        // The leap day is what a naive formula gets wrong, and the day before
        // the epoch is what truncating division (rather than euclidean) gets
        // wrong; either moves a routine's fire or a budget month by a day.
        for (days, date) in [
            (0, (1970, 1, 1)),
            (11_016, (2000, 2, 29)),
            (-1, (1969, 12, 31)),
            (-719_468, (0, 3, 1)),
        ] {
            assert_eq!(civil_from_days(days), date, "{days}");
            assert_eq!(days_from_civil(date.0, date.1, date.2), days, "{date:?}");
        }
    }
}
