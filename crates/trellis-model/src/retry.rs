//! The hosted client's retry policy (ADR 0024 §4): which statuses are
//! retried, how many attempts a model call gets, and how long each wait is,
//! including the `Retry-After` HTTP-date parser. Kept apart from the wire
//! mapping in `hosted` because it changes for different reasons (#613).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Whether an error body is the provider saying the request exceeded the
/// context window. Matched on the documented markers only: OpenAI's
/// `context_length_exceeded` code, Anthropic's `prompt is too long`.
pub fn is_context_overflow(status: u16, body: &str) -> bool {
    status == 413
        || (status == 400
            && (body.contains("context_length_exceeded") || body.contains("prompt is too long")))
}

/// Statuses worth retrying: rate limits and the provider's own failures.
/// 529 is Anthropic's "overloaded".
pub(crate) fn retryable(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504 | 529)
}

/// How many attempts one model call gets, the first included.
pub const MAX_ATTEMPTS: u32 = 4;

/// The longest wait honoured from `Retry-After`. A provider asking for more
/// is asking a run to sit idle for longer than a turn is worth.
pub const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);

/// The wait before attempt `attempt + 1`, `attempt` counting from 1, for a
/// failure with no status of its own (a transport failure), or a status
/// whose overload schedule does not apply (see `status_retry_delay`).
///
/// `Retry-After` in seconds or as an IMF-fixdate HTTP date
/// (`Sun, 06 Nov 1994 08:49:37 GMT`) when present, capped; a date at or
/// before now waits zero. Otherwise 1, 2, 4 s. The obsolete RFC 850 and
/// asctime date forms are not parsed and fall back to the fixed backoff
/// (ADR 0024 §4).
pub fn retry_delay(retry_after: Option<&str>, attempt: u32) -> Duration {
    retry_delay_at(retry_after, attempt, SystemTime::now())
}

/// The waits before the second, third and fourth attempt after a 529 or 503
/// that names no `Retry-After` (#613). Anthropic's 529 never carries one, so
/// on the 1, 2, 4 s schedule a ten-second overload ended the model call and
/// rolled the run back. Each is within `MAX_RETRY_WAIT`.
pub const OVERLOADED_WAITS: [Duration; 3] = [
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_secs(60),
];

/// The wait before attempt `attempt + 1` after a retryable `status`.
///
/// A `Retry-After` is honoured as `retry_delay` honours it, whatever the
/// status. Without one, an overload (529, 503) waits `OVERLOADED_WAITS`,
/// and every other status keeps the 1, 2, 4 s backoff (ADR 0024 §4).
pub fn status_retry_delay(status: u16, retry_after: Option<&str>, attempt: u32) -> Duration {
    if retry_after.is_none() && matches!(status, 503 | 529) {
        let i = attempt.saturating_sub(1) as usize;
        return OVERLOADED_WAITS[i.min(OVERLOADED_WAITS.len() - 1)].min(MAX_RETRY_WAIT);
    }
    retry_delay(retry_after, attempt)
}

/// `retry_delay` against a given clock, so the date form is testable
/// without depending on when the test runs.
fn retry_delay_at(retry_after: Option<&str>, attempt: u32, now: SystemTime) -> Duration {
    let backoff = Duration::from_secs(1u64 << attempt.saturating_sub(1).min(5));
    let Some(v) = retry_after.map(str::trim) else {
        return backoff;
    };
    if let Ok(s) = v.parse::<f64>() {
        return if s.is_finite() && s >= 0.0 {
            // Clamped before converting: `from_secs_f64` panics past u64::MAX
            // seconds, and the value is whatever the provider sent.
            Duration::from_secs_f64(s.min(MAX_RETRY_WAIT.as_secs_f64()))
        } else {
            backoff
        };
    }
    let Some(at) = imf_fixdate_unix_secs(v) else {
        return backoff;
    };
    // A date before 1970 is in the past for any clock this runs on.
    let Ok(at) = u64::try_from(at) else {
        return Duration::ZERO;
    };
    let now = now.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    Duration::from_secs(at)
        .saturating_sub(now)
        .min(MAX_RETRY_WAIT)
}

/// Seconds since the Unix epoch of an IMF-fixdate (RFC 9110 §5.6.7), the one
/// form senders must use. Parsed by hand rather than adding a dependency for
/// one header.
///
/// Anything else is `None`: the obsolete RFC 850 and asctime forms, a zone
/// other than `GMT`, a day that does not exist in its month (`31 Feb`), or an
/// out-of-range time. The day name must be a day name but is not checked
/// against the date; the wait is computed from the date.
fn imf_fixdate_unix_secs(v: &str) -> Option<i64> {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // `Sun, 06 Nov 1994 08:49:37 GMT` is fixed width, so every field sits at
    // a fixed offset and any deviation fails a check below.
    let b = v.as_bytes();
    if b.len() != 29
        || !DAYS.iter().any(|d| d.as_bytes() == &b[0..3])
        || &b[3..5] != b", "
        || b[7] != b' '
        || b[11] != b' '
        || b[16] != b' '
        || b[19] != b':'
        || b[22] != b':'
        || &b[25..29] != b" GMT"
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let d = &b[r];
        d.iter()
            .all(u8::is_ascii_digit)
            .then(|| d.iter().fold(0i64, |n, c| n * 10 + i64::from(c - b'0')))
    };
    let day = num(5..7)?;
    let month = MONTHS.iter().position(|m| m.as_bytes() == &b[8..11])? as i64 + 1;
    let year = num(12..16)?;
    let (hour, minute, second) = (num(17..19)?, num(20..22)?, num(23..25)?);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_len = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    // RFC 9110's grammar allows second 60, a leap second.
    if !(1..=month_len).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Days from 1970-01-01 to a proleptic Gregorian date (H. Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_is_honoured_and_capped() {
        assert_eq!(retry_delay(Some("3"), 1), Duration::from_secs(3));
        assert_eq!(retry_delay(Some("0.5"), 1), Duration::from_millis(500));
        assert_eq!(retry_delay(Some("86400"), 1), MAX_RETRY_WAIT);
        // Missing: the fixed backoff.
        assert_eq!(retry_delay(None, 1), Duration::from_secs(1));
        assert_eq!(retry_delay(None, 3), Duration::from_secs(4));
        // An HTTP date: the time until it, against a fixed clock.
        assert_eq!(
            retry_delay_at(
                Some("Wed, 21 Oct 2026 07:28:00 GMT"),
                2,
                unix(1_792_567_650)
            ),
            Duration::from_secs(30)
        );
        // Negative or infinite seconds: the backoff, not a panic in
        // `Duration::from_secs_f64` and not the cap.
        for v in ["-1", "inf"] {
            assert_eq!(retry_delay(Some(v), 2), Duration::from_secs(2), "{v}");
        }
    }

    /// A fixed clock: the date tests must not depend on when they run.
    fn unix(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn an_http_date_in_the_future_waits_the_difference() {
        // Sun, 06 Nov 1994 08:49:37 GMT is 784111777 s after the epoch.
        let v = Some("Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(
            retry_delay_at(v, 1, unix(784_111_777 - 42)),
            Duration::from_secs(42)
        );
        // A leap day, so a wrong month length would shift the answer.
        assert_eq!(
            imf_fixdate_unix_secs("Thu, 29 Feb 2024 00:00:00 GMT"),
            Some(1_709_164_800)
        );
        // 2000 is a leap year by the 400 rule, and second 60 is a leap
        // second RFC 9110 allows.
        assert_eq!(
            imf_fixdate_unix_secs("Tue, 29 Feb 2000 00:00:00 GMT"),
            Some(951_782_400)
        );
        assert_eq!(
            imf_fixdate_unix_secs("Sun, 06 Nov 1994 08:49:60 GMT"),
            Some(784_111_800)
        );
    }

    #[test]
    fn an_http_date_now_or_past_waits_zero_not_the_backoff() {
        // A date already reached says "retry now", as `Retry-After: 0` does.
        let v = Some("Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(retry_delay_at(v, 3, unix(784_111_777)), Duration::ZERO);
        assert_eq!(retry_delay_at(v, 3, unix(1_800_000_000)), Duration::ZERO);
        // Before the epoch: negative seconds, still the past, not a wrap.
        assert_eq!(
            retry_delay_at(Some("Mon, 01 Jan 1900 00:00:00 GMT"), 3, unix(0)),
            Duration::ZERO
        );
    }

    #[test]
    fn an_http_date_beyond_the_cap_waits_max_retry_wait() {
        // A date is as untrusted as a number of seconds: a provider naming a
        // far-off date must not park the run until then.
        assert_eq!(
            retry_delay_at(Some("Fri, 31 Dec 9999 23:59:59 GMT"), 1, unix(0)),
            MAX_RETRY_WAIT
        );
        let v = Some("Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(retry_delay_at(v, 1, unix(784_111_777 - 61)), MAX_RETRY_WAIT);
    }

    #[test]
    fn a_malformed_or_obsolete_http_date_falls_back_to_the_backoff() {
        // Each names a future instant if misparsed, so a lenient parser shows
        // up as a wait other than the 2 s backoff.
        let now = unix(0);
        for v in [
            // RFC 850 and asctime: obsolete, and not parsed.
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
            // Days that do not exist.
            "Tue, 31 Feb 2026 00:00:00 GMT",
            "Sat, 29 Feb 2025 00:00:00 GMT",
            "Mon, 00 Jan 2026 00:00:00 GMT",
            "Fri, 31 Apr 2026 00:00:00 GMT",
            // 1900 is not a leap year: divisible by 100 and not by 400.
            "Thu, 29 Feb 1900 00:00:00 GMT",
            // A zone other than GMT.
            "Sun, 06 Nov 1994 08:49:37 UTC",
            "Sun, 06 Nov 1994 08:49:37 +0000",
            // Out-of-range time, unknown names, wrong widths, garbage.
            "Sun, 06 Nov 1994 24:00:00 GMT",
            "Sun, 06 Nov 1994 08:60:00 GMT",
            "Sun, 06 Nov 1994 08:49:61 GMT",
            "Xyz, 06 Nov 1994 08:49:37 GMT",
            "Sun, 06 Noz 1994 08:49:37 GMT",
            "Sun, 6 Nov 1994 08:49:37 GMT",
            "Sun, +6 Nov 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 08:49:37 GMTX",
            "Sun, 06 Nov 1994 08:49:37 gmt",
            // One separator wrong at a time, the rest a valid date.
            "Sun, 06-Nov 1994 08:49:37 GMT",
            "Sun, 06 Nov-1994 08:49:37 GMT",
            "Sun, 06 Nov 1994T08:49:37 GMT",
            "Sun, 06 Nov 1994 08.49:37 GMT",
            "Sun, 06 Nov 1994 08:49.37 GMT",
            "soon",
            "",
        ] {
            assert_eq!(
                retry_delay_at(Some(v), 2, now),
                Duration::from_secs(2),
                "{v:?}"
            );
        }
    }

    #[test]
    fn a_retry_after_too_large_for_a_duration_is_capped_not_a_panic() {
        // `Duration::from_secs_f64` panics above u64::MAX seconds, and the
        // cap was applied after it: a provider's `1e20` panicked the run
        // mid-flight, before rollback.
        for v in ["1e20", "1.8446744073709552e19", "1e308"] {
            assert_eq!(retry_delay(Some(v), 1), MAX_RETRY_WAIT, "{v}");
        }
    }

    #[test]
    fn overflow_is_recognised_by_the_documented_markers_only() {
        assert!(is_context_overflow(
            400,
            r#"{"error":{"code":"context_length_exceeded"}}"#
        ));
        assert!(is_context_overflow(
            400,
            r#"{"error":{"message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#
        ));
        assert!(is_context_overflow(413, ""));
        // OpenAI sends this text beside the `context_length_exceeded` code,
        // which is the documented marker; the text alone is not one.
        assert!(!is_context_overflow(
            400,
            r#"{"error":{"message":"This model's maximum context length is 8192 tokens"}}"#
        ));
        assert!(!is_context_overflow(
            400,
            r#"{"error":"roles must alternate"}"#
        ));
    }

    #[test]
    fn an_overloaded_529_or_503_without_retry_after_waits_15_30_60_s() {
        // The risk (#613): Anthropic's 529 carries no Retry-After, so on the
        // 1, 2, 4 s schedule a ten-second overload ended the model call and
        // the run was rolled back.
        for status in [529, 503] {
            let waits: Vec<_> = (1..MAX_ATTEMPTS)
                .map(|a| status_retry_delay(status, None, a))
                .collect();
            assert_eq!(
                waits,
                [15, 30, 60].map(Duration::from_secs),
                "status {status}"
            );
            assert!(waits.iter().all(|w| *w <= MAX_RETRY_WAIT));
        }
    }

    #[test]
    fn an_overloaded_status_with_retry_after_waits_what_it_asked_for() {
        // The provider's own figure wins over the overload schedule, and is
        // still capped.
        assert_eq!(status_retry_delay(529, Some("0"), 1), Duration::ZERO);
        assert_eq!(
            status_retry_delay(503, Some("7"), 3),
            Duration::from_secs(7)
        );
        assert_eq!(status_retry_delay(503, Some("86400"), 1), MAX_RETRY_WAIT);
    }

    #[test]
    fn not_overloaded_statuses_keep_the_1_2_4_s_schedule() {
        // The risk: the longer overload waits spreading to a rate limit or a
        // plain 5xx, which #613 leaves as they were.
        for status in [429, 500, 502, 504] {
            let waits: Vec<_> = (1..MAX_ATTEMPTS)
                .map(|a| status_retry_delay(status, None, a))
                .collect();
            assert_eq!(waits, [1, 2, 4].map(Duration::from_secs), "status {status}");
        }
        assert_eq!(
            status_retry_delay(429, Some("3"), 1),
            Duration::from_secs(3)
        );
    }
}
