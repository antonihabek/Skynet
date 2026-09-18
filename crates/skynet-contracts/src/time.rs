//! Wall time on the wire: RFC 3339 in UTC, and the day a timestamp falls on (§35.1).
//!
//! Here rather than in the daemon because three kinds of program write a date into a document
//! another one reads — the supervisor's audit log and API, the CLI's results documents, and an
//! edition's own corpus runner — and a calendar is exactly the fact that must exist once. Two
//! implementations of a date conversion disagree exactly once: on a leap day, years after either
//! was written.
//!
//! Nothing here reads a clock. A caller passes the reading, so the arithmetic is the same on a
//! robot, a workstation and a `no_std` target, and a test can name the instant it means.

use alloc::format;
use alloc::string::String;

/// Wall time as RFC 3339, UTC, from a Unix millisecond reading (§35.1).
///
/// Written out rather than pulled in, because the workspace has no dependencies (see the root
/// `Cargo.toml`) and a date is arithmetic. The civil-from-days conversion is Howard Hinnant's,
/// which is the one every implementation of this uses; it is exact for every day this machine will
/// see and it is short enough to check by reading.
pub fn rfc3339(unix_ms: u64) -> String {
    let seconds = unix_ms / 1000;
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);

    // Shift the epoch to 0000-03-01 so that a leap day lands at the end of the cycle and every
    // month has a fixed length from the start of the era.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// The day number an RFC 3339 timestamp falls on, counting from the Unix epoch.
///
/// The inverse of [`rfc3339`]'s calendar half, and beside it for that reason: one calendar in one
/// file.
///
/// Only the `YYYY-MM-DD` prefix is read. Everything after it is a time of day, and §15 measures a
/// bench result's age in days, so an hour would be precision the answer does not have.
///
/// `None` for anything that is not a date: a caller asking how old a result is must be able to tell
/// *"three hundred days"* from *"this record does not say"*, and a zero for the second would be a
/// year-old bench pass reading as fresh.
pub fn day_number(timestamp: &str) -> Option<i64> {
    let date = timestamp.get(..10)?;
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // Howard Hinnant's days-from-civil, the exact inverse of the civil-from-days above.
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

#[cfg(test)]
mod tests {
    use super::{day_number, rfc3339};

    /// A leap day, a century that is not a leap year (1900) and one that is (2000) are the three
    /// cases every hand-rolled civil calendar gets wrong, so they are the three that are here.
    #[test]
    fn wall_time_renders_as_rfc3339() {
        for (unix_ms, expected) in [
            (0u64, "1970-01-01T00:00:00Z"),
            (1_000, "1970-01-01T00:00:01Z"),
            (951_782_400_000, "2000-02-29T00:00:00Z"),
            (951_868_800_000, "2000-03-01T00:00:00Z"),
            (1_709_164_800_000, "2024-02-29T00:00:00Z"),
            (1_756_555_200_000, "2025-08-30T12:00:00Z"),
            (1_788_091_200_000, "2026-08-30T12:00:00Z"),
            (2_082_758_399_000, "2035-12-31T23:59:59Z"),
        ] {
            assert_eq!(rfc3339(unix_ms), expected, "at {unix_ms}");
        }
    }

    /// The two halves of the calendar are each other's inverse, checked across two leap rules and
    /// a century boundary rather than on one convenient date.
    #[test]
    fn the_two_directions_agree() {
        for day in [0_i64, 1, 59, 60, 11_016, 20_000, 25_000, 30_000, 40_000] {
            let stamp = rfc3339(day as u64 * 86_400_000);
            assert_eq!(day_number(&stamp), Some(day), "{stamp}");
        }
    }

    #[test]
    fn known_days_are_the_known_numbers() {
        assert_eq!(day_number("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(day_number("2000-02-29T12:00:00Z"), Some(11_016));
        assert_eq!(day_number("2026-09-16"), Some(20_712));
    }

    /// A date this cannot read is `None` and never a zero, because a caller measuring an age would
    /// read a zero as *today* — which turns an unreadable date into the freshest evidence there is.
    #[test]
    fn nothing_that_is_not_a_date_becomes_one() {
        for bad in [
            "",
            "yesterday",
            "2026-13-01",
            "2026-09-99",
            "2026/09/16",
            "2026-09",
        ] {
            assert_eq!(day_number(bad), None, "{bad}");
        }
    }
}
