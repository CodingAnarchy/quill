//! Date helpers for the progress report — the author's local "today", ISO and
//! human formatting, and day arithmetic. Thin wrappers over the `time` crate
//! (a dependency for the local day boundary anyway), so there's no hand-rolled
//! calendar math. Dates are `time::Date`.

use time::macros::format_description;
use time::{Date, Duration, OffsetDateTime};

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
pub fn add_days(date: Date, n: i64) -> Date {
    date.saturating_add(Duration::days(n))
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
}
