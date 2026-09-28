//! Five-field cron and fixed-offset timezones, by hand (ADR 0026 §7).
//!
//! `minute hour day-of-month month day-of-week`, each `*`, a number, a
//! range `a-b`, a step (`*/n`, `a-b/n`, `a/n`) or a comma list of those.
//! Month names `jan`–`dec`, day names `sun`–`sat`, day-of-week 0–7 with both
//! 0 and 7 Sunday. When day-of-month and day-of-week are both restricted a
//! day matches if either does — the rule every cron since Vixie's follows,
//! and the one a person reading `0 9 1 * mon` expects.
//!
//! Not supported: `L`, `W`, `#`, `?`, `@daily`-style names. Each is refused
//! with its name rather than misread.
//!
//! Timezones are `UTC` or a fixed offset. An IANA name needs the tz
//! database, which is a dependency and a dataset that goes stale; a fixed
//! offset does not follow daylight saving, and the ADR says so.

use crate::stamp::civil_from_secs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u32,
    doms: u32,
    months: u16,
    dows: u8,
    dom_star: bool,
    dow_star: bool,
    text: String,
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

fn value(s: &str, lo: u32, hi: u32, names: Option<(&[&str], u32)>) -> Result<u32, String> {
    if let Some((names, base)) = names
        && let Some(i) = names.iter().position(|n| n.eq_ignore_ascii_case(s))
    {
        return Ok(i as u32 + base);
    }
    let v: u32 = s
        .parse()
        .map_err(|_| format!("`{s}` is not a number in {lo}-{hi}"))?;
    if v < lo || v > hi {
        return Err(format!("{v} is outside {lo}-{hi}"));
    }
    Ok(v)
}

/// A field as a bit set over `lo..=hi`, and whether it was `*`.
fn field(s: &str, lo: u32, hi: u32, names: Option<(&[&str], u32)>) -> Result<(u64, bool), String> {
    let mut bits = 0u64;
    // Vixie cron's test: the field's first character, so `*/2` counts as a
    // star for the day-of-month / day-of-week rule, not only `*`.
    let star = s.starts_with('*');
    for part in s.split(',') {
        if part.is_empty() {
            return Err(format!("empty list item in `{s}`"));
        }
        let (range, step) = match part.split_once('/') {
            Some((r, st)) => {
                let st: u32 = st
                    .parse()
                    .map_err(|_| format!("step `{st}` is not a number"))?;
                if st == 0 {
                    return Err("a step of 0 never advances".into());
                }
                (r, st)
            }
            None => (part, 1),
        };
        let (a, b) = if range == "*" {
            (lo, hi)
        } else if let Some((a, b)) = range.split_once('-') {
            (value(a, lo, hi, names)?, value(b, lo, hi, names)?)
        } else {
            let a = value(range, lo, hi, names)?;
            // `a/n` means "from a to the end, every n", as in Vixie cron.
            (a, if part.contains('/') { hi } else { a })
        };
        if a > b {
            return Err(format!("range `{range}` runs backwards"));
        }
        let mut v = a;
        while v <= b {
            bits |= 1 << v;
            v += step;
        }
    }
    Ok((bits, star))
}

impl Cron {
    pub fn parse(s: &str) -> Result<Cron, String> {
        let t = s.trim();
        if t.starts_with('@') {
            return Err(format!(
                "`{t}`: named schedules are not supported; write the five fields"
            ));
        }
        for bad in ['#', '?'] {
            if t.contains(bad) {
                return Err(format!("`{t}`: `{bad}` is not supported"));
            }
        }
        // `L` and `W` only as Quartz uses them (`L`, `15W`, `5L`), so that
        // `jul` and `wed` in any case still read as names.
        for tok in t.split(|c: char| c.is_whitespace() || ",-/".contains(c)) {
            let digits_then = |c: char| {
                tok.len() > 1
                    && tok.ends_with(c)
                    && tok[..tok.len() - 1].bytes().all(|b| b.is_ascii_digit())
            };
            for bad in ['L', 'W'] {
                if tok == bad.to_string() || digits_then(bad) {
                    return Err(format!("`{t}`: `{bad}` is not supported"));
                }
            }
        }
        let f: Vec<&str> = t.split_whitespace().collect();
        if f.len() != 5 {
            return Err(format!(
                "`{t}` has {} field(s); cron here is exactly five: minute hour \
                 day-of-month month day-of-week",
                f.len()
            ));
        }
        let (minutes, _) = field(f[0], 0, 59, None).map_err(|e| format!("minute: {e}"))?;
        let (hours, _) = field(f[1], 0, 23, None).map_err(|e| format!("hour: {e}"))?;
        let (doms, dom_star) =
            field(f[2], 1, 31, None).map_err(|e| format!("day-of-month: {e}"))?;
        let (months, _) =
            field(f[3], 1, 12, Some((&MONTHS, 1))).map_err(|e| format!("month: {e}"))?;
        let (mut dows, dow_star) =
            field(f[4], 0, 7, Some((&DAYS, 0))).map_err(|e| format!("day-of-week: {e}"))?;
        if dows & (1 << 7) != 0 {
            dows = (dows | 1) & 0x7f;
        }
        Ok(Cron {
            minutes,
            hours: hours as u32,
            doms: doms as u32,
            months: months as u16,
            dows: dows as u8,
            dom_star,
            dow_star,
            text: t.to_string(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    fn day_matches(&self, days: i64, month: u32, dom: u32) -> bool {
        if self.months & (1 << month) == 0 {
            return false;
        }
        // 1970-01-01 was a Thursday.
        let dow = (days + 4).rem_euclid(7) as u32;
        let dom_ok = self.doms & (1 << dom) != 0;
        let dow_ok = self.dows & (1 << dow) != 0;
        // Vixie cron: if either day field starts with `*`, both must match;
        // only when both are restricted does either suffice. For a plain
        // `*` the set is full, so "both" is the other field alone, as
        // before; for `*/2` the step still applies.
        if self.dom_star || self.dow_star {
            dom_ok && dow_ok
        } else {
            dom_ok || dow_ok
        }
    }

    /// The first scheduled instant strictly after `t` (seconds since the
    /// epoch), evaluated in `tz`. `None` if nothing matches within about
    /// eight years, which is what `0 0 31 2 *` (never) comes to.
    pub fn next_after(&self, t: i64, tz: Tz) -> Option<i64> {
        let off = tz.offset_secs();
        // Work in local wall-clock seconds; the offset is fixed, so local
        // minus the offset is the instant.
        let local = t + off;
        let start = local.div_euclid(60) * 60 + 60;
        let mut day = start.div_euclid(86_400);
        let (mut h0, mut m0) = {
            let (.., h, m, _) = civil_from_secs(start);
            (h, m)
        };
        for _ in 0..(366 * 8) {
            let (_, month, dom, ..) = civil_from_secs(day * 86_400);
            if self.day_matches(day, month, dom) {
                for h in h0..24 {
                    if self.hours & (1 << h) == 0 {
                        continue;
                    }
                    let from = if h == h0 { m0 } else { 0 };
                    for m in from..60 {
                        if self.minutes & (1 << m) != 0 {
                            return Some(
                                day * 86_400 + i64::from(h) * 3600 + i64::from(m) * 60 - off,
                            );
                        }
                    }
                }
            }
            day += 1;
            h0 = 0;
            m0 = 0;
        }
        None
    }
}

/// UTC or a fixed offset from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tz {
    Utc,
    /// Seconds east of UTC.
    Fixed(i32),
}

impl Tz {
    pub fn parse(s: &str) -> Result<Tz, String> {
        let t = s.trim();
        if t.eq_ignore_ascii_case("utc") || t == "Z" || t == "+00:00" || t == "-00:00" {
            return Ok(Tz::Utc);
        }
        let sign = match t.as_bytes().first() {
            Some(b'+') => 1,
            Some(b'-') => -1,
            _ => {
                return Err(format!(
                    "timezone `{t}`: only `UTC` and fixed offsets like `+02:00` are \
                     supported. IANA names would need the tz database, a dependency \
                     that goes stale; a fixed offset does not follow daylight saving \
                     (ADR 0026 §7)"
                ));
            }
        };
        let (h, m) = t[1..]
            .split_once(':')
            .ok_or_else(|| format!("timezone `{t}`: write the offset as `+HH:MM`"))?;
        // Digits only: `i32::parse` takes a sign of its own, and read
        // `+-05:00` as -05:00 and `+05:-30` as +04:30.
        let digits = |f: &str, what: &str| -> Result<i32, String> {
            if f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("timezone `{t}`: bad {what}"));
            }
            f.parse().map_err(|_| format!("timezone `{t}`: bad {what}"))
        };
        let (h, m) = (digits(h, "hours")?, digits(m, "minutes")?);
        if h > 14 || m > 59 || h.to_string().len() > 2 {
            return Err(format!("timezone `{t}`: offset out of range"));
        }
        Ok(Tz::Fixed(sign * (h * 3600 + m * 60)))
    }

    pub fn offset_secs(self) -> i64 {
        match self {
            Tz::Utc => 0,
            Tz::Fixed(s) => i64::from(s),
        }
    }

    /// `YYYY-MM-DD` and `HH:MM` of an instant, on this zone's wall clock.
    pub fn date_time(self, t: i64) -> (String, String) {
        let (y, mo, d, h, mi, _) = civil_from_secs(t + self.offset_secs());
        (format!("{y:04}-{mo:02}-{d:02}"), format!("{h:02}:{mi:02}"))
    }
}

impl std::fmt::Display for Tz {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tz::Utc => write!(f, "UTC"),
            Tz::Fixed(s) => {
                let sign = if *s < 0 { '-' } else { '+' };
                let a = s.abs();
                write!(f, "{sign}{:02}:{:02}", a / 3600, a % 3600 / 60)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stamp::{parse_iso, secs_to_iso};

    fn next(c: &str, after: &str, tz: &str) -> String {
        let c = Cron::parse(c).unwrap();
        secs_to_iso(
            c.next_after(parse_iso(after).unwrap(), Tz::parse(tz).unwrap())
                .unwrap(),
        )
    }

    #[test]
    fn a_stepped_star_day_field_ands_with_the_other_as_vixie_cron_does() {
        // Vixie cron treats a day field as `*` for the OR-vs-AND rule when
        // its first character is `*`, so `*/2` in day-of-month ANDs with a
        // weekday: odd-numbered Mondays only. Read as a restriction instead,
        // it ORed, and fired every Monday and every odd day.
        // 2026-09-27 is a Sunday; Monday the 28th is even, the 5th is odd.
        assert_eq!(
            next("0 9 */2 * mon", "2026-09-27T12:00:00Z", "UTC"),
            "2026-10-05T09:00:00Z",
            "not Monday the 28th (even), not Tuesday the 29th (odd)"
        );
        assert_eq!(
            next("0 9 */2 * mon", "2026-10-05T12:00:00Z", "UTC"),
            "2026-10-19T09:00:00Z"
        );
        // The same in the other field: `*/2` weekdays AND the 13th.
        assert_eq!(
            next("0 9 13 * */2", "2026-09-27T12:00:00Z", "UTC"),
            "2026-10-13T09:00:00Z",
            "Tuesday the 13th; Tuesday is 2, which */2 includes"
        );
        // Two restricted fields still OR: Friday the 2nd comes first.
        assert_eq!(
            next("0 9 13 * fri", "2026-09-27T12:00:00Z", "UTC"),
            "2026-10-02T09:00:00Z"
        );
    }

    #[test]
    fn next_fire_times_match_a_calendar() {
        // 2026-09-27 is a Sunday (checked against `cal 9 2026`).
        assert_eq!(
            next("0 9 * * 1-5", "2026-09-27T12:00:00Z", "UTC"),
            "2026-09-28T09:00:00Z"
        );
        assert_eq!(
            next("*/15 * * * *", "2026-09-27T12:07:30Z", "UTC"),
            "2026-09-27T12:15:00Z"
        );
        // Strictly after: a fire at exactly `t` is not the next one.
        assert_eq!(
            next("0 12 * * *", "2026-09-27T12:00:00Z", "UTC"),
            "2026-09-28T12:00:00Z"
        );
        assert_eq!(
            next("0 0 29 2 *", "2026-03-01T00:00:00Z", "UTC"),
            "2028-02-29T00:00:00Z"
        );
        assert_eq!(
            next("30 8 * jan,jul sun", "2026-09-27T12:00:00Z", "UTC"),
            "2027-01-03T08:30:00Z"
        );
        // 7 is Sunday as well as 0.
        assert_eq!(
            next("0 0 * * 7", "2026-09-27T12:00:00Z", "UTC"),
            "2026-10-04T00:00:00Z"
        );
    }

    #[test]
    fn day_of_month_and_day_of_week_restricted_together_match_either() {
        // Vixie semantics: the 1st OR any Monday. Taking AND would fire only
        // on a Monday the 1st, a few times a year, and look like a bug in
        // the scheduler rather than in the reading of the rule.
        assert_eq!(
            next("0 9 1 * mon", "2026-09-27T12:00:00Z", "UTC"),
            "2026-09-28T09:00:00Z"
        );
        assert_eq!(
            next("0 9 1 * mon", "2026-09-28T12:00:00Z", "UTC"),
            "2026-10-01T09:00:00Z"
        );
    }

    #[test]
    fn a_fixed_offset_shifts_the_wall_clock_not_the_rule() {
        // 09:00 at +02:00 is 07:00Z; and a date near midnight lands on the
        // local day, not the UTC one.
        assert_eq!(
            next("0 9 * * *", "2026-09-27T12:00:00Z", "+02:00"),
            "2026-09-28T07:00:00Z"
        );
        assert_eq!(
            next("0 1 * * mon", "2026-09-27T12:00:00Z", "+05:30"),
            "2026-09-27T19:30:00Z"
        );
        assert_eq!(Tz::parse("-05:30").unwrap().to_string(), "-05:30");
    }

    #[test]
    fn what_is_not_supported_is_refused_by_name() {
        // The risk: a schedule silently read as something else fires at
        // times nobody asked for, unattended.
        let e = Tz::parse("Europe/Amsterdam").unwrap_err();
        assert!(e.contains("fixed offsets") && e.contains("daylight"), "{e}");
        assert!(Cron::parse("@daily").unwrap_err().contains("five fields"));
        assert!(Cron::parse("0 0 L * *").unwrap_err().contains("`L`"));
        assert!(Cron::parse("0 0 * *").unwrap_err().contains("exactly five"));
        assert!(Cron::parse("60 * * * *").is_err());
        assert!(Cron::parse("*/0 * * * *").is_err());
        assert!(Cron::parse("5-1 * * * *").is_err());
    }

    #[test]
    fn an_offset_with_a_second_sign_is_refused() {
        // `i32::parse` accepts a leading sign, so `+-05:00` read as -05:00
        // and `+05:-30` as +04:30: an offset nobody wrote, fired unattended.
        for bad in [
            "+-05:00", "++05:00", "+05:-30", "+05:+30", "- 5:00", "+05: 30",
        ] {
            assert!(Tz::parse(bad).is_err(), "`{bad}` was accepted");
        }
        assert_eq!(Tz::parse("+05:30").unwrap(), Tz::Fixed(5 * 3600 + 30 * 60));
        assert_eq!(Tz::parse("-5:00").unwrap(), Tz::Fixed(-5 * 3600));
    }

    #[test]
    fn a_schedule_that_never_fires_says_none_rather_than_looping() {
        let c = Cron::parse("0 0 31 2 *").unwrap();
        assert_eq!(c.next_after(0, Tz::Utc), None);
    }
}
