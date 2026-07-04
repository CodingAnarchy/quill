//! Date helpers for the progress report — the author's local "today", ISO and
//! human formatting, and day arithmetic. Thin wrappers over the `time` crate
//! (a dependency for the local day boundary anyway), so there's no hand-rolled
//! calendar math. Dates are `time::Date`.

use time::macros::format_description;
use time::{Date, Duration, Month, OffsetDateTime, Weekday};

/// The author's local date (falling back to UTC if the local offset can't be
/// determined), so the daily counters reset at the local midnight.
pub fn today() -> Date {
    OffsetDateTime::now_local()
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .date()
}

/// Parse an ISO `YYYY-MM-DD` date (the manifest / snapshot form).
pub fn parse(s: &str) -> Option<Date> {
    Date::parse(s.trim(), format_description!("[year]-[month]-[day]")).ok()
}

/// ISO `YYYY-MM-DD` — the manifest / snapshot form.
pub fn iso(date: Date) -> String {
    date.format(format_description!("[year]-[month]-[day]"))
        .unwrap_or_default()
}

/// The current UTC instant as an EPUB `dcterms:modified` timestamp
/// (`YYYY-MM-DDThh:mm:ssZ`, the OPF-required form) — the one place export needs a
/// full datetime rather than a `Date`.
pub fn utc_timestamp() -> String {
    let t = OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        t.year(),
        t.month() as u8,
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// A filename-safe local timestamp `YYYY-MM-DD-HHMMSS` — the stamp an offline
/// project archive (#594) carries so successive backups sort and never collide.
/// Local time (falling back to UTC) so the name matches the author's clock.
pub fn file_stamp() -> String {
    let t = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    format!(
        "{:04}-{:02}-{:02}-{:02}{:02}{:02}",
        t.year(),
        t.month() as u8,
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// Human form `D Mon YYYY`, e.g. `27 Jun 2026` — for the report's dates.
pub fn human(date: Date) -> String {
    date.format(format_description!(
        "[day padding:none] [month repr:short] [year]"
    ))
    .unwrap_or_default()
}

/// Whole days from `start` to `end` (negative if `end` is earlier).
pub fn days_between(start: Date, end: Date) -> i64 {
    (end - start).whole_days()
}

/// The date `n` days after `date`.
pub const fn add_days(date: Date, n: i64) -> Date {
    date.saturating_add(Duration::days(n))
}

/// The first day of `date`'s calendar month (the 1st).
pub fn month_start(date: Date) -> Date {
    Date::from_calendar_date(date.year(), date.month(), 1).unwrap_or(date)
}

/// The last day of `date`'s calendar month (28–31 depending on the month/year).
pub fn month_end(date: Date) -> Date {
    let last = days_in_month(date.year(), date.month());
    Date::from_calendar_date(date.year(), date.month(), last).unwrap_or(date)
}

/// The number of days in the given month, leap years handled.
const fn days_in_month(year: i32, month: Month) -> u8 {
    match month {
        Month::January
        | Month::March
        | Month::May
        | Month::July
        | Month::August
        | Month::October
        | Month::December => 31,
        Month::April | Month::June | Month::September | Month::November => 30,
        Month::February => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
    }
}

const fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The weekday index with Sunday = 0 … Saturday = 6 — the column a day lands in
/// on a Sunday-first calendar grid.
pub const fn weekday_sunday0(date: Date) -> usize {
    match date.weekday() {
        Weekday::Sunday => 0,
        Weekday::Monday => 1,
        Weekday::Tuesday => 2,
        Weekday::Wednesday => 3,
        Weekday::Thursday => 4,
        Weekday::Friday => 5,
        Weekday::Saturday => 6,
    }
}

/// The month's human name and year, e.g. `June 2026` — the calendar heading.
pub fn month_title(date: Date) -> String {
    date.format(format_description!("[month repr:long] [year]"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats() {
        let d = parse("2026-01-11").unwrap();
        assert_eq!(iso(d), "2026-01-11");
        assert_eq!(
            parse("  2026-06-16 ").map(iso).as_deref(),
            Some("2026-06-16")
        );
        assert_eq!(parse("not-a-date"), None);
    }

    #[test]
    fn day_math_matches_the_worked_example() {
        // The progress-report spec: 2026-01-11 → 2026-06-16 is 156 days elapsed,
        // and +11 days from the report date is 27 Jun 2026.
        let start = parse("2026-01-11").unwrap();
        let report = parse("2026-06-16").unwrap();
        assert_eq!(days_between(start, report), 156);
        assert_eq!(human(add_days(report, 11)), "27 Jun 2026");
    }

    #[test]
    fn month_bounds_and_title() {
        let mid = parse("2026-06-17").unwrap();
        assert_eq!(iso(month_start(mid)), "2026-06-01");
        assert_eq!(iso(month_end(mid)), "2026-06-30"); // June has 30 days
        assert_eq!(month_title(mid), "June 2026");
        // February in a leap year has 29 days; a common year, 28.
        assert_eq!(iso(month_end(parse("2024-02-10").unwrap())), "2024-02-29");
        assert_eq!(iso(month_end(parse("2026-02-10").unwrap())), "2026-02-28");
        // Century rule: 2100 is not a leap year, 2000 is.
        assert_eq!(iso(month_end(parse("2100-02-10").unwrap())), "2100-02-28");
        assert_eq!(iso(month_end(parse("2000-02-10").unwrap())), "2000-02-29");
    }

    #[test]
    fn weekday_column_is_sunday_first() {
        // 2026-06-01 is a Monday, 2026-06-07 a Sunday.
        assert_eq!(weekday_sunday0(parse("2026-06-01").unwrap()), 1);
        assert_eq!(weekday_sunday0(parse("2026-06-07").unwrap()), 0);
        assert_eq!(weekday_sunday0(parse("2026-06-13").unwrap()), 6); // Saturday
    }
}
