//! The writing-history view (#586): the read side of the per-day history log
//! ([`crate::history`]). Quill already records each day's net words; this turns
//! that `Vec<Snapshot>` into a screen — a multi-day sparkline of daily net
//! words, the current and longest writing streak, and the totals — rendered
//! through the same full-screen report overlay the progress report uses.
//!
//! Pure: it takes the loaded snapshots and `today` and returns styled rows, so
//! the streak and total formulas are testable without a project on disk. Number
//! formatting and the green-gain / red-loss delta spans are reused from
//! [`crate::report`] so the figures read the same as the progress report.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use time::Date;

use crate::date;
use crate::history::Snapshot;
use crate::report::{colored_net_span, thousands};
use crate::theme;

/// The eight block glyphs the sparkline draws net-words bars from, lightest to
/// fullest. A zero or negative day renders as the lowest mark so a flat or
/// red day still shows on the row.
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Build the writing-history overlay's rows from the recorded `snaps` (oldest
/// first, as [`crate::history::load`] returns them), as of `today`. With no
/// history yet, returns a single hint line so the view is never blank.
pub fn lines(snaps: &[Snapshot], today: Date) -> Vec<Line<'static>> {
    if snaps.is_empty() {
        return vec![Line::raw(
            "No writing history yet — it builds up a day at a time as you write.",
        )];
    }

    let mut out = Vec::new();
    let nets: Vec<i64> = snaps.iter().map(net).collect();

    // --- Streaks & best day ------------------------------------------------
    let (current, longest) = streaks(snaps, today);
    out.push(Line::raw(format!(
        "current streak:  {} days",
        thousands(current)
    )));
    out.push(Line::raw(format!(
        "longest streak:  {} days",
        thousands(longest)
    )));

    if let Some((i, best)) = nets
        .iter()
        .enumerate()
        .max_by_key(|(_, &n)| n)
        .filter(|(_, &n)| n > 0)
    {
        out.push(Line::from(vec![
            Span::raw("best day:        "),
            colored_net_span(*best),
            Span::raw(format!(" on {}", human(&snaps[i].date))),
        ]));
    }

    // --- Totals ------------------------------------------------------------
    let added: i64 = snaps.iter().map(|s| s.added as i64).sum();
    let removed: i64 = snaps.iter().map(|s| s.removed as i64).sum();
    out.push(Line::raw(""));
    out.push(Line::raw(format!(
        "days recorded:   {}",
        thousands(snaps.len() as i64)
    )));
    out.push(Line::from(vec![
        Span::raw("net words:       "),
        colored_net_span(added - removed),
        Span::styled(
            format!("   (+{} -{})", thousands(added), thousands(removed)),
            Style::default().fg(theme::current().dim),
        ),
    ]));

    // --- Sparkline ---------------------------------------------------------
    out.push(Line::raw(""));
    out.push(Line::raw("daily net words:"));
    out.push(sparkline_line(&nets));
    // The window the sparkline spans, so a bar maps back to a date.
    out.push(Line::styled(
        format!(
            "{} → {}",
            human(&snaps[0].date),
            human(&snaps[snaps.len() - 1].date)
        ),
        Style::default().fg(theme::current().dim),
    ));

    out
}

/// Today's net words for a snapshot: added − removed.
const fn net(s: &Snapshot) -> i64 {
    s.added as i64 - s.removed as i64
}

/// The current and longest writing streaks (in days). A day counts toward a
/// streak when it has a positive net word count; the *current* streak is the
/// run of such days ending today (or yesterday — a streak isn't broken until a
/// day passes without writing). The *longest* is the longest such run anywhere
/// in the history. Gaps in the dates break a run even if the surrounding days
/// are positive.
fn streaks(snaps: &[Snapshot], today: Date) -> (i64, i64) {
    // The writing days, parsed and sorted; a malformed date drops out.
    let mut days: Vec<Date> = snaps
        .iter()
        .filter(|s| net(s) > 0)
        .filter_map(|s| date::parse(&s.date))
        .collect();
    days.sort();
    if days.is_empty() {
        return (0, 0);
    }

    // Longest: the longest run of consecutive calendar days.
    let mut longest = 1i64;
    let mut run = 1i64;
    for pair in days.windows(2) {
        if date::days_between(pair[0], pair[1]) == 1 {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 1;
        }
    }

    // Current: only counts if the last writing day is today or yesterday;
    // otherwise the streak has lapsed. Walk back over consecutive days.
    let last = *days.last().unwrap();
    let gap = date::days_between(last, today);
    let current = if gap > 1 {
        0
    } else {
        let mut count = 1i64;
        for pair in days.windows(2).rev() {
            if date::days_between(pair[0], pair[1]) == 1 {
                count += 1;
            } else {
                break;
            }
        }
        count
    };

    (current, longest)
}

/// A sparkline row of the daily net words, scaled to the largest positive day.
/// Non-positive days draw at the lowest mark (so a flat or losing day is still
/// visible on the row) and are colored red; positive days green.
fn sparkline_line(nets: &[i64]) -> Line<'static> {
    let theme = theme::current();
    let peak = nets.iter().copied().max().unwrap_or(0).max(1);
    let spans = nets
        .iter()
        .map(|&n| {
            let glyph = if n <= 0 {
                BARS[0]
            } else {
                // Map (0, peak] onto bar indices 0..=7.
                let idx = ((n as f64 / peak as f64) * (BARS.len() - 1) as f64).round() as usize;
                BARS[idx.min(BARS.len() - 1)]
            };
            let color = if n > 0 { theme.added } else { theme.removed };
            Span::styled(glyph.to_string(), Style::default().fg(color))
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

/// An ISO date string in the report's human `D Mon YYYY` form, falling back to
/// the raw string if it doesn't parse.
fn human(iso: &str) -> String {
    date::parse(iso).map_or_else(|| iso.to_string(), date::human)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn snap(date: &str, added: usize, removed: usize) -> Snapshot {
        Snapshot {
            date: date.into(),
            words: 1000,
            added,
            removed,
            todo: 0,
            moved_to_unused: 0,
            ..Snapshot::default()
        }
    }

    fn today() -> Date {
        date::parse("2026-06-17").unwrap()
    }

    /// The plain text of styled rows, for content assertions.
    fn text(rows: &[Line]) -> String {
        rows.iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn empty_history_shows_a_hint() {
        let rows = lines(&[], today());
        assert!(text(&rows).contains("No writing history yet"));
    }

    #[test]
    fn consecutive_days_ending_today_make_the_current_streak() {
        // Three days in a row, the last being today.
        let snaps = [
            snap("2026-06-15", 300, 10),
            snap("2026-06-16", 200, 0),
            snap("2026-06-17", 150, 0),
        ];
        let (current, longest) = streaks(&snaps, today());
        assert_eq!(current, 3);
        assert_eq!(longest, 3);
    }

    #[test]
    fn current_streak_counts_when_last_day_was_yesterday() {
        // No record for today yet, but yesterday + the day before are a run.
        let snaps = [snap("2026-06-15", 100, 0), snap("2026-06-16", 100, 0)];
        let (current, longest) = streaks(&snaps, today());
        assert_eq!(current, 2, "a streak isn't broken until a whole day passes");
        assert_eq!(longest, 2);
    }

    #[test]
    fn a_gap_resets_the_current_streak_but_not_the_longest() {
        // A four-day run, then a gap, then two recent days ending today.
        let snaps = [
            snap("2026-06-08", 100, 0),
            snap("2026-06-09", 100, 0),
            snap("2026-06-10", 100, 0),
            snap("2026-06-11", 100, 0),
            // gap: 12th–15th not written
            snap("2026-06-16", 100, 0),
            snap("2026-06-17", 100, 0),
        ];
        let (current, longest) = streaks(&snaps, today());
        assert_eq!(current, 2, "only the recent run counts now");
        assert_eq!(longest, 4, "the earlier four-day run is the longest");
    }

    #[test]
    fn a_lapsed_streak_is_zero() {
        // The last writing day was several days ago — the streak has lapsed.
        let snaps = [snap("2026-06-10", 100, 0), snap("2026-06-11", 100, 0)];
        let (current, longest) = streaks(&snaps, today());
        assert_eq!(current, 0);
        assert_eq!(longest, 2);
    }

    #[test]
    fn zero_and_losing_days_dont_extend_a_streak() {
        // A net-zero day and a net-loss day both break the run.
        let snaps = [
            snap("2026-06-15", 100, 0),
            snap("2026-06-16", 50, 50), // net 0
            snap("2026-06-17", 100, 0),
        ];
        let (current, longest) = streaks(&snaps, today());
        assert_eq!(current, 1, "only today's positive day counts");
        assert_eq!(longest, 1);
    }

    #[test]
    fn totals_and_best_day_are_summed_from_the_history() {
        let snaps = [
            snap("2026-06-15", 300, 50),
            snap("2026-06-16", 800, 100), // the best day, net +700
            snap("2026-06-17", 100, 0),
        ];
        let report = text(&lines(&snaps, today()));
        // Totals: added 1,200 / removed 150, net +1,050.
        assert!(report.contains("net words:       +1,050"), "{report}");
        assert!(report.contains("(+1,200 -150)"), "{report}");
        assert!(report.contains("days recorded:   3"), "{report}");
        // Best day is the largest net, shown with its date.
        assert!(
            report.contains("best day:        +700 on 16 Jun 2026"),
            "{report}"
        );
        assert!(report.contains("current streak:  3 days"), "{report}");
        assert!(report.contains("longest streak:  3 days"), "{report}");
    }

    #[test]
    fn sparkline_has_one_glyph_per_day_colored_by_sign() {
        let snaps = [
            snap("2026-06-15", 1000, 0), // big gain → tall bar
            snap("2026-06-16", 10, 200), // net loss → red, low bar
            snap("2026-06-17", 100, 0),  // small gain
        ];
        let rows = lines(&snaps, today());
        let spark = rows
            .iter()
            .find(|l| {
                !l.spans.is_empty()
                    && l.spans
                        .iter()
                        .all(|s| BARS.contains(&s.content.chars().next().unwrap_or(' ')))
            })
            .expect("a sparkline row");
        assert_eq!(spark.spans.len(), 3, "one glyph per recorded day");
        // The biggest day is the fullest bar; the losing day is red.
        assert_eq!(spark.spans[0].content, "█");
        assert_eq!(spark.spans[0].style.fg, Some(Color::Green));
        assert_eq!(spark.spans[1].style.fg, Some(Color::Red));
    }

    #[test]
    fn builds_from_a_loaded_history_log() {
        // Hermetic (#531): persist a few days to a scratch project's log, then
        // load and render exactly the way the overlay does — end to end through
        // `history::load`, no project on disk needed beyond the scratch dir.
        use crate::history;
        let dir = crate::testutil::scratch_dir("histview", "load");
        history::persist(&dir, &snap("2026-06-16", 400, 0)).unwrap();
        history::persist(&dir, &snap("2026-06-17", 250, 50)).unwrap();

        let report = text(&lines(&history::load(&dir), today()));
        assert!(report.contains("days recorded:   2"), "{report}");
        assert!(report.contains("current streak:  2 days"), "{report}");
        // Totals: added 650 / removed 50, net +600.
        assert!(report.contains("net words:       +600"), "{report}");
        assert!(report.contains("(+650 -50)"), "{report}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
