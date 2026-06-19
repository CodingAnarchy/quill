//! A tiny civil-date type — just enough for the progress report's "days since
//! start", writing rate, and projected completion dates. No dependency: dates
//! convert to/from a day number (Howard Hinnant's `days_from_civil`), which
//! makes differences and offsets plain integer math.
//!
//! `today()` uses the system clock in **UTC**. The local day boundary (which
//! matters for resetting daily counters) is the daily-momentum ticket's
//! concern (#59); a one-day skew here only nudges "days since start".

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A calendar date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Date {
    pub year: i64,
    pub month: u32,
    pub day: u32,
}

impl Date {
    /// Parse an ISO `YYYY-MM-DD` date (the manifest's `start_date` form).
    pub fn parse(s: &str) -> Option<Date> {
        let mut parts = s.trim().split('-');
        let year = parts.next()?.parse().ok()?;
        let month = parts.next()?.parse().ok()?;
        let day = parts.next()?.parse().ok()?;
        if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        Some(Date { year, month, day })
    }

    /// Today's date, from the system clock (UTC — see the module note).
    pub fn today() -> Date {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Date::from_days(secs.div_euclid(86_400))
    }

    /// Days from the civil epoch (1970-01-01 = 0); negative before it.
    fn to_days(self) -> i64 {
        let (y, m, d) = (self.year, self.month as i64, self.day as i64);
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// The inverse of [`to_days`].
    fn from_days(z: i64) -> Date {
        let z = z + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
        Date {
            year: if month <= 2 { y + 1 } else { y },
            month,
            day,
        }
    }

    /// Whole days from `start` to `self` (negative if `self` is earlier).
    pub fn days_since(self, start: Date) -> i64 {
        self.to_days() - start.to_days()
    }

    /// The date `n` days after `self`.
    pub fn add_days(self, n: i64) -> Date {
        Date::from_days(self.to_days() + n)
    }
}

impl fmt::Display for Date {
    /// `DD Mon YYYY`, e.g. `27 Jun 2026`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let month = MONTHS.get((self.month as usize).wrapping_sub(1)).unwrap_or(&"???");
        write!(f, "{} {month} {}", self.day, self.year)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(year: i64, month: u32, day: u32) -> Date {
        Date { year, month, day }
    }

    #[test]
    fn parses_iso_dates() {
        assert_eq!(Date::parse("2026-01-11"), Some(ymd(2026, 1, 11)));
        assert_eq!(Date::parse("  2026-06-16 "), Some(ymd(2026, 6, 16)));
        assert_eq!(Date::parse("2026-13-01"), None); // bad month
        assert_eq!(Date::parse("not-a-date"), None);
        assert_eq!(Date::parse("2026-06"), None);
    }

    #[test]
    fn days_since_matches_the_worked_example() {
        // The progress-report spec: 2026-01-11 → 2026-06-16 is 156 days elapsed.
        assert_eq!(ymd(2026, 6, 16).days_since(ymd(2026, 1, 11)), 156);
    }

    #[test]
    fn add_days_crosses_months_and_formats() {
        let eta = ymd(2026, 6, 16).add_days(11);
        assert_eq!(eta, ymd(2026, 6, 27));
        assert_eq!(eta.to_string(), "27 Jun 2026");
    }

    #[test]
    fn round_trips_through_the_day_number_across_a_leap_year() {
        for date in [ymd(2024, 2, 29), ymd(2000, 1, 1), ymd(2026, 12, 31), ymd(1970, 1, 1)] {
            assert_eq!(Date::from_days(date.to_days()), date);
        }
    }
}
