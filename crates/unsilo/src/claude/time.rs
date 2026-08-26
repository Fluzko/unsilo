//! Timestamps as Claude writes them.
//!
//! Transcripts carry ISO-8601 UTC strings (`2026-08-24T20:07:16.145Z`) while the
//! desktop index stores epoch milliseconds. Everything is normalised to epoch
//! millis so filters compare one unit.
//!
//! The shape is fixed and narrow, so this parses it directly rather than pulling
//! in a date library: anything that does not match exactly returns `None` instead
//! of being coerced into a wrong instant.

/// Days from the Unix epoch to a proleptic Gregorian date.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[must_use]
pub fn iso_to_epoch_ms(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.len() < 20 || bytes.get(4) != Some(&b'-') || bytes.get(10) != Some(&b'T') {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> { s.get(range)?.parse().ok() };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let millis = match bytes.get(19) {
        Some(b'.') => {
            let frac: String = s.get(20..)?.chars().take_while(char::is_ascii_digit).collect();
            let mut value: i64 = frac.get(0..3).unwrap_or(&frac).parse().ok()?;
            // ".5" means 500ms, not 5ms.
            for _ in frac.len()..3 {
                value *= 10;
            }
            value
        }
        _ => 0,
    };
    let secs = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    Some(secs * 1000 + millis)
}

#[must_use]
pub fn days_between(earlier_ms: i64, later_ms: i64) -> i64 {
    (later_ms - earlier_ms) / 86_400_000
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn the_unix_epoch_is_zero() {
        assert_eq!(iso_to_epoch_ms("1970-01-01T00:00:00.000Z"), Some(0));
    }

    #[test]
    fn a_real_transcript_timestamp_round_trips() {
        // Cross-checked against Date.parse in the browser.
        assert_eq!(iso_to_epoch_ms("2026-08-24T20:07:16.145Z"), Some(1_787_602_036_145));
    }

    #[test]
    fn milliseconds_are_optional() {
        assert_eq!(iso_to_epoch_ms("2026-08-24T20:07:16Z"), Some(1_787_602_036_000));
    }

    #[test]
    fn a_short_fraction_is_left_aligned_not_right() {
        assert_eq!(iso_to_epoch_ms("1970-01-01T00:00:00.5Z"), Some(500));
        assert_eq!(iso_to_epoch_ms("1970-01-01T00:00:00.05Z"), Some(50));
    }

    #[test]
    fn extra_precision_is_truncated_not_rejected() {
        assert_eq!(iso_to_epoch_ms("1970-01-01T00:00:00.123456Z"), Some(123));
    }

    #[test]
    fn leap_years_and_centuries_land_correctly() {
        assert_eq!(iso_to_epoch_ms("2000-02-29T00:00:00.000Z"), Some(951_782_400_000));
        assert_eq!(iso_to_epoch_ms("2024-12-31T23:59:59.999Z"), Some(1_735_689_599_999));
    }

    #[test]
    fn garbage_is_rejected_rather_than_coerced() {
        for s in
            ["", "nope", "2026-13-01T00:00:00Z", "2026-08-24 20:07:16Z", "2026-08-24T99:00:00Z"]
        {
            assert_eq!(iso_to_epoch_ms(s), None, "{s} should not parse");
        }
    }

    #[test]
    fn day_arithmetic_is_whole_days() {
        let a = iso_to_epoch_ms("2026-01-01T00:00:00Z").unwrap();
        let b = iso_to_epoch_ms("2026-02-01T00:00:00Z").unwrap();
        assert_eq!(days_between(a, b), 31);
    }
}
