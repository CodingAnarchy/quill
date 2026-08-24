//! The writing-history view (#586): the read side of the per-day history log
//! ([`crate::history`]). Quill already records each day's net words; this turns
//! that `Vec<Snapshot>` into a screen — a multi-day sparkline of daily net
//! words, the current and longest writing streak, the totals, and a GitHub-style
//! month calendar heatmap (#835) — rendered through the same full-screen report
//! overlay the progress report uses.
//!
//! Pure: it takes the loaded snapshots, the recorded sessions, and `today` and
//! returns styled rows, so the streak, total, and calendar formulas are testable
//! without a project on disk (deterministic injected dates). Number formatting
//! and the green-gain / red-loss delta spans are reused from [`crate::report`] so
//! the figures read the same as the progress report.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use time::Date;

use crate::date;
use crate::history::{SessionRecord, Snapshot};
use crate::report::{colored_net_span, thousands};
use crate::theme;

/// The eight block glyphs the sparkline draws net-words bars from, lightest to
/// fullest. A zero or negative day renders as the lowest mark so a flat or
/// red day still shows on the row.
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// The five heatmap intensity glyphs (#835), lightest (no output) to fullest.
/// Index 0 is a written-nothing day within the month; 1..=4 scale a positive
/// day's net output against the month's busiest day, GitHub-contribution style.
const HEAT: [char; 5] = ['·', '░', '▒', '▓', '█'];

/// Build the writing-history overlay's rows from the recorded `snaps` (oldest
/// first, as [`crate::history::load`] returns them) and `sessions` (the per-burst
/// records, #835), as of `today`. With no history yet, returns a single hint line
/// so the view is never blank.
pub fn lines(snaps: &[Snapshot], sessions: &[SessionRecord], today: Date) -> Vec<Line<'static>> {
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

    // --- Calendar heatmap (#835) -------------------------------------------
    out.push(Line::raw(""));
    out.extend(calendar_lines(snaps, sessions, today));

    out
}

/// A day's net output in the calendar: added − removed for the snapshot on that
/// date, keyed by ISO string. Only positive days shade the heatmap.
fn net_by_date(snaps: &[Snapshot]) -> std::collections::HashMap<String, i64> {
    snaps.iter().map(|s| (s.date.clone(), net(s))).collect()
}

/// The calendar days from `first` to `last` inclusive, in order.
fn month_days(first: Date, last: Date) -> impl Iterator<Item = Date> {
    let mut cursor = Some(first);
    std::iter::from_fn(move || {
        let day = cursor.filter(|&d| d <= last)?;
        cursor = Some(date::add_days(day, 1));
        Some(day)
    })
}

/// The month calendar heatmap ending on `today`'s month (#835): a Sunday-first
/// grid where each day cell is a shaded block scaled to the month's busiest day,
/// plus a per-session breakdown of the month's most productive day when its
/// output is spread across recorded sessions.
///
/// Pure over the injected `today`, so the grid is deterministic in tests.
fn calendar_lines(
    snaps: &[Snapshot],
    sessions: &[SessionRecord],
    today: Date,
) -> Vec<Line<'static>> {
    let theme = theme::current();
    let dim = Style::default().fg(theme.dim);
    let first = date::month_start(today);
    let last = date::month_end(today);
    let nets = net_by_date(snaps);

    // The month's busiest day scales the intensity; a month with no positive day
    // still renders (every cell at the lowest mark).
    let peak = month_days(first, last)
        .filter_map(|d| nets.get(&date::iso(d)).copied())
        .filter(|&n| n > 0)
        .max()
        .unwrap_or(0)
        .max(1);

    let mut out = vec![
        Line::styled(format!("calendar — {}", date::month_title(first)), dim),
        Line::styled("Su Mo Tu We Th Fr Sa", dim),
    ];

    // Lay the month out week by week, padding the first row to the 1st's column.
    let mut row: Vec<Span<'static>> = Vec::new();
    let lead = date::weekday_sunday0(first);
    for _ in 0..lead {
        row.push(Span::raw("   ")); // a blank cell before the 1st
    }
    let mut cursor = first;
    while cursor <= last {
        let net = nets.get(&date::iso(cursor)).copied().unwrap_or(0);
        row.push(heat_cell(net, peak, cursor == today));
        // A completed week (Saturday) flushes a row; otherwise keep filling.
        if date::weekday_sunday0(cursor) == 6 {
            out.push(Line::from(std::mem::take(&mut row)));
        }
        cursor = date::add_days(cursor, 1);
    }
    if !row.is_empty() {
        out.push(Line::from(row));
    }

    // The month's most productive day, broken into its recorded sessions — the
    // per-session view the coalesced snapshot can't show (#835).
    out.extend(busiest_day_breakdown(&nets, sessions, first, last));
    out
}

/// One calendar cell: a two-wide shaded block scaled to `peak`, green for a
/// positive day, red for a net loss, dim for a written-nothing day. `is_today`
/// marks the cell with a trailing caret so the author finds their place.
fn heat_cell(net: i64, peak: i64, is_today: bool) -> Span<'static> {
    let theme = theme::current();
    let (glyph, color) = if net > 0 {
        // Map (0, peak] onto intensity levels 1..=4.
        let idx = ((net as f64 / peak as f64) * 4.0).ceil() as usize;
        (HEAT[idx.clamp(1, 4)], theme.added)
    } else if net < 0 {
        (HEAT[1], theme.removed) // a losing day still shows, in red
    } else {
        (HEAT[0], theme.dim) // wrote nothing (or no record)
    };
    let marker = if is_today { '<' } else { ' ' };
    Span::styled(
        format!("{glyph}{glyph}{marker}"),
        Style::default().fg(color),
    )
}

/// The per-session breakdown of the month's most productive day (#835): its date
/// and each recorded session's net output, so a day written in bursts shows its
/// bursts. Empty when the month has no positive day or that day has no recorded
/// sessions (an older day predating session records).
fn busiest_day_breakdown(
    nets: &std::collections::HashMap<String, i64>,
    sessions: &[SessionRecord],
    first: Date,
    last: Date,
) -> Vec<Line<'static>> {
    // The busiest day *within this month* — the one whose snapshot net is highest.
    let Some((iso, _)) = month_days(first, last)
        .map(date::iso)
        .filter_map(|d| nets.get(&d).map(|&n| (d, n)))
        .filter(|(_, n)| *n > 0)
        .max_by_key(|(_, n)| *n)
    else {
        return Vec::new();
    };
    let day_sessions: Vec<&SessionRecord> = sessions.iter().filter(|s| s.date == iso).collect();
    if day_sessions.is_empty() {
        return Vec::new();
    }

    let dim = Style::default().fg(theme::current().dim);
    let mut out = vec![
        Line::raw(""),
        Line::styled(
            format!(
                "sessions on {} ({}):",
                human(&iso),
                pluralize(day_sessions.len())
            ),
            dim,
        ),
    ];
    for (i, s) in day_sessions.iter().enumerate() {
        out.push(Line::from(vec![
            Span::styled(format!("  {}. ", i + 1), dim),
            colored_net_span(s.net()),
            Span::styled(
                format!(
                    "  (+{} -{})",
                    thousands(s.added as i64),
                    thousands(s.removed as i64)
                ),
                dim,
            ),
        ]));
    }
    out
}

/// "1 session" / "N sessions" for the breakdown heading.
fn pluralize(n: usize) -> String {
    if n == 1 {
        "1 session".to_string()
    } else {
        format!("{n} sessions")
    }
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
    // Binds the last writing day and proves `days` non-empty for the loops below;
    // an empty history has no streak.
    let Some(&last) = days.last() else {
        return (0, 0);
    };

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
    use crate::history;
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

    fn session(date: &str, added: usize, removed: usize) -> SessionRecord {
        SessionRecord {
            date: date.into(),
            added,
            removed,
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
        let rows = lines(&[], &[], today());
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
        let report = text(&lines(&snaps, &[], today()));
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
        let rows = lines(&snaps, &[], today());
        let spark = rows
            .iter()
            .find(|l| {
                // A sparkline row is one single-char BARS glyph per day; the
                // calendar's cells are two-char HEAT blocks, so keep it to
                // one-char spans to avoid matching a full-intensity week row.
                !l.spans.is_empty()
                    && l.spans.iter().all(|s| {
                        s.content.chars().count() == 1
                            && BARS.contains(&s.content.chars().next().unwrap_or(' '))
                    })
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
        let dir = crate::testutil::scratch_dir("histview", "load");
        history::persist(&dir, &snap("2026-06-16", 400, 0)).unwrap();
        history::persist(&dir, &snap("2026-06-17", 250, 50)).unwrap();

        let report = text(&lines(&history::load(&dir), &[], today()));
        assert!(report.contains("days recorded:   2"), "{report}");
        assert!(report.contains("current streak:  2 days"), "{report}");
        // Totals: added 650 / removed 50, net +600.
        assert!(report.contains("net words:       +600"), "{report}");
        assert!(report.contains("(+650 -50)"), "{report}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The calendar grid rows: the ones whose spans are all HEAT cells (skipping
    /// the heading and weekday labels), each cell being a padded two-glyph block.
    fn calendar_grid<'a>(rows: &'a [Line<'static>]) -> Vec<&'a Line<'static>> {
        rows.iter()
            .filter(|l| {
                !l.spans.is_empty()
                    && l.spans.iter().all(|s| {
                        let c = s.content.chars().next().unwrap_or('x');
                        HEAT.contains(&c) || c == ' '
                    })
            })
            .collect()
    }

    #[test]
    fn calendar_renders_the_month_grid_for_injected_dates() {
        // June 2026 starts on a Monday, so the first grid row has one blank lead
        // cell (Sunday) then the 1st. The month has 30 days across 5 week rows.
        let snaps = [snap("2026-06-16", 500, 0), snap("2026-06-17", 200, 0)];
        let rows = lines(&snaps, &[], today());
        let report = text(&rows);
        assert!(report.contains("calendar — June 2026"), "{report}");
        assert!(report.contains("Su Mo Tu We Th Fr Sa"), "{report}");

        let grid = calendar_grid(&rows);
        assert_eq!(grid.len(), 5, "June 2026 spans five week rows");
        // The first row: a blank lead cell (Sunday) then Mon 1 … Sat 6 — 7 cells.
        assert_eq!(grid[0].spans.len(), 7, "first week padded to the 1st");
        // The 1st is a Monday, so cell index 1 is June 1 (wrote nothing → dim dot).
        assert_eq!(grid[0].spans[0].content.trim(), ""); // Sunday blank lead
        assert_eq!(grid[0].spans[1].content.chars().next(), Some(HEAT[0]));
    }

    #[test]
    fn calendar_shades_by_daily_output_and_marks_today() {
        // The 16th is the busiest day (peak); the 17th (today) a smaller gain; a
        // net-loss day shows in red.
        let snaps = [
            snap("2026-06-15", 20, 300), // net loss
            snap("2026-06-16", 1000, 0), // the peak
            snap("2026-06-17", 100, 0),  // today, a small gain
        ];
        let rows = lines(&snaps, &[], today());
        // Only the calendar's own cells — two-glyph HEAT blocks — not the
        // sparkline's single-glyph BARS spans (which share the '█' glyph).
        let cells: Vec<&Span> = calendar_grid(&rows)
            .into_iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.content.chars().count() >= 2)
            .collect();

        // The peak day is the fullest green block; today carries the '<' marker;
        // the loss day is red. Locate them by color + glyph among the cells.
        let peak = cells
            .iter()
            .find(|s| s.content.starts_with('█') && s.style.fg == Some(Color::Green))
            .expect("a full-intensity green cell for the peak day");
        assert!(peak.content.starts_with("██"));

        let today_cell = cells
            .iter()
            .find(|s| s.content.ends_with('<'))
            .expect("today's cell is marked");
        assert_eq!(today_cell.style.fg, Some(Color::Green));

        let loss = cells
            .iter()
            .find(|s| {
                s.style.fg == Some(Color::Red)
                    && HEAT.contains(&s.content.chars().next().unwrap_or('x'))
            })
            .expect("a red cell for the net-loss day");
        assert!(loss.content.starts_with('░'));
    }

    #[test]
    fn calendar_breaks_the_busiest_day_into_sessions() {
        // The 16th is the busiest snapshot day; its recorded sessions break the
        // day's output into bursts (#835) — the coalesced snapshot can't show it.
        let snaps = [snap("2026-06-16", 900, 50), snap("2026-06-17", 100, 0)];
        let sessions = [
            session("2026-06-16", 400, 0),
            session("2026-06-16", 500, 50), // a second burst
            session("2026-06-17", 100, 0),  // a different day, not shown here
        ];
        let report = text(&lines(&snaps, &sessions, today()));
        assert!(
            report.contains("sessions on 16 Jun 2026 (2 sessions):"),
            "{report}"
        );
        assert!(report.contains("1. +400"), "{report}");
        assert!(report.contains("2. +450"), "{report}"); // net of the 500/50 burst
        assert!(report.contains("(+500 -50)"), "{report}");
        // The other day's session isn't in the breakdown.
        assert!(!report.contains("sessions on 17 Jun"), "{report}");
    }

    #[test]
    fn calendar_omits_the_breakdown_without_session_records() {
        // A month with output but no session records (older days) shows the grid
        // but no per-session breakdown — nothing to show.
        let snaps = [snap("2026-06-16", 500, 0)];
        let report = text(&lines(&snaps, &[], today()));
        assert!(report.contains("calendar — June 2026"), "{report}");
        assert!(!report.contains("sessions on"), "{report}");
    }
}
