//! The progress report: a single screen tracking the manuscript against the
//! author's goals. This module is pure — it turns the project's goals plus a
//! few measured numbers into the report's text lines, so the formulas are
//! testable against the worked example in `docs/progress-reports.md`.
//!
//! The daily-momentum lines (today's delta, per-part deltas, moved-to-unused)
//! are color-coded (green gains / red losses), so [`lines`] returns styled rows
//! ([`Vec<Line>`](Line)), letting the UI render the colored deltas directly
//! rather than re-parsing strings for `+`/`-`.

use crate::theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use time::Date;

use crate::date;
use crate::project::Manifest;

/// Today's writing momentum, fed into the report from the daily counters.
pub struct Momentum {
    pub added: usize,
    pub removed: usize,
    pub moved_to_unused: usize,
}

/// One part's line in the "part deltas" block: its display name, current word
/// total, an optional word-count goal (#146), and today's added / removed.
pub struct PartDelta {
    pub name: String,
    pub words: usize,
    pub goal: Option<usize>,
    pub added: usize,
    pub removed: usize,
}

/// A multi-book series summary for the report header (#331): total words across
/// every book and the book count. `None` for a standalone novel.
pub struct SeriesSummary {
    pub words: usize,
    pub books: usize,
}

/// Words by revision status for the report's status breakdown: how much of the
/// manuscript has reached each stage of the draft → revised → final cycle.
/// Cumulative — `revised` counts every body chapter at Revised *or* Final (a
/// final chapter was revised too), so `final_words` ≤ `revised` by construction.
/// Both are measured over body chapters, against the same body-word total the
/// story line reports. A stage with zero words is omitted from the report.
pub struct StatusWords {
    /// Words in chapters at Revised or Final (cumulative through the cycle).
    pub revised: usize,
    /// Words in chapters at Final.
    pub final_words: usize,
}

/// Build the report's rows for a project with `words` body words, `section_words`
/// in front/back matter (counted toward neither the goal nor the projection,
/// #182), `chapters` body chapters, `todo` TODO markers, and today's `momentum`,
/// as of `today`. Rows are styled (the word delta is colored). Returns a short
/// hint when no goals are set.
// The report's measured inputs read clearer flat than boxed.
#[allow(clippy::too_many_arguments)]
// `series` is taken by value: the caller (app/reports.rs) builds it fresh and hands
// it off, and passing `&Option` would ripple a `&` into that out-of-crate call site.
#[allow(clippy::needless_pass_by_value)]
pub fn lines(
    manifest: &Manifest,
    words: usize,
    section_words: usize,
    chapters: usize,
    todo: usize,
    status: &StatusWords,
    momentum: &Momentum,
    parts: &[PartDelta],
    today: Date,
    series: Option<SeriesSummary>,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    // The whole-series total leads the report; the body below is the active book.
    if let Some(s) = &series {
        out.push(Line::raw(format!(
            "series:   {} words across {} books",
            thousands(s.words as i64),
            s.books,
        )));
        out.push(Line::raw(""));
    }

    let Some(goals) = manifest
        .goals
        .as_ref()
        .filter(|g| g.words > 0 || g.chapters > 0)
    else {
        out.push(Line::raw("No writing goals set."));
        out.push(Line::raw(
            "Set them in-app: Ctrl-K → \"goals:set\" (target words, chapters, daily, deadline).",
        ));
        return out;
    };
    let wpp = goals.words_per_page.max(1);
    let pages = (words / wpp) as i64;

    // --- Story: completion against the goals -------------------------------
    out.push(Line::raw(format!(
        "story :   {} / {}  words ( {} )",
        thousands(words as i64),
        thousands(goals.words as i64),
        percent(words, goals.words),
    )));
    out.push(Line::raw(format!(
        "          {} / {}  pages",
        thousands(pages),
        thousands((goals.words / wpp) as i64),
    )));
    out.push(Line::raw(format!(
        "          {} / {} chapters ( {} )",
        chapters,
        goals.chapters,
        percent(chapters, goals.chapters),
    )));
    // Front/back-matter words are shown but kept out of the goal and projection.
    if section_words > 0 {
        out.push(Line::raw(format!(
            "          (+{} words in front/back matter, not counted)",
            thousands(section_words as i64),
        )));
    }

    // --- Revision status: how much of the manuscript has been revised / finalized.
    // Cumulative through the draft → revised → final cycle, against the same body-
    // word total the story line reports. Each line appears only when it has words,
    // so a project that never touches statuses sees this block not at all.
    out.extend(status_lines(status, words));

    // --- Projection: extrapolate current chapter pace to the whole book ----
    // f = chapter completion; projected words Wp = W / f. Computed once.
    let projection = Projection::of(words, chapters, goals, wpp);
    if let Some(p) = projection {
        out.push(Line::raw(""));
        out.push(Line::raw(format!(
            "          expansion ratio: {:.2}",
            p.ratio
        )));
        out.push(Line::raw(format!(
            "          words at this ratio: {}",
            thousands(p.words)
        )));
        out.push(Line::raw(format!(
            "          pages at this ratio: {}",
            thousands(p.pages)
        )));
        out.push(Line::raw(""));
        out.push(Line::raw(format!(
            "          {} / {} words at this ratio ( {} )",
            thousands(words as i64),
            thousands(p.words),
            percent(chapters, goals.chapters), // == W / Wp by construction
        )));
        out.push(Line::raw(format!(
            "          {} / {} pages at this ratio",
            thousands(pages),
            thousands(p.pages),
        )));
    }

    // --- Markers & today's momentum ---------------------------------------
    out.push(Line::raw(""));
    out.push(Line::raw(format!("xxx:               {todo}")));
    out.push(Line::raw(format!(
        "moved to unused:   {}",
        thousands(momentum.moved_to_unused as i64),
    )));
    out.push(Line::raw(""));
    out.push(delta_line(momentum));
    // Today's progress toward the daily target (#277), when one is set. Uses the
    // same net (added − removed) as the delta line above, so the two agree.
    if let Some(daily) = goals.daily.filter(|&d| d > 0) {
        out.push(daily_goal_line(momentum, daily));
    }
    out.extend(part_lines(parts, manifest.part_label()));

    // --- Rate & projected completion --------------------------------------
    if let Some(start) = manifest.start_date.as_deref().and_then(date::parse) {
        let days = date::days_between(start, today).max(0);
        let rate = words as i64 / days.max(1); // floor(W / days)
        out.push(Line::raw(""));
        out.push(Line::raw(format!("days since start:  {}", thousands(days))));
        out.push(Line::raw(format!(
            "average rate:      {} words / day",
            thousands(rate)
        )));
        out.push(Line::raw(to_go(
            "to go:            ",
            goals.words as i64 - words as i64,
            rate,
            today,
        )));
        if let Some(p) = projection {
            out.push(Line::raw(to_go(
                "to go (expansion):",
                p.words - words as i64,
                rate,
                today,
            )));
        }
        // With a deadline set (#814), read the current pace against the calendar:
        // where it lands the finish, the words/day the deadline needs, and whether
        // pace is ahead or behind. No deadline → these lines are simply omitted.
        if let Some(dl) = DeadlineProjection::of(goals, words, rate, today) {
            out.push(Line::raw(""));
            out.extend(dl.lines());
        }
    }

    // A discoverable affordance for changing the goals these numbers track (#550).
    out.push(Line::raw(""));
    out.push(Line::styled(
        "Change goals: Ctrl-K → \"goals:set\".",
        Style::default().fg(theme::current().dim),
    ));

    out
}

/// Today's progress toward the daily word-count target (#277): the net words
/// today (colored like the delta line) against the goal, with how many remain or
/// that it's met. `daily` is guaranteed non-zero by the caller.
fn daily_goal_line(momentum: &Momentum, daily: usize) -> Line<'static> {
    let net = momentum.added as i64 - momentum.removed as i64;
    let status = if net >= daily as i64 {
        "met \u{2713}".to_string()
    } else {
        let pct = (net.max(0) as f64 / daily as f64 * 100.0) as i64;
        format!("{pct}% \u{b7} {} to go", thousands(daily as i64 - net))
    };
    Line::from(vec![
        Span::raw("daily goal:         "),
        colored_net_span(net),
        Span::raw(format!(
            " / {} words  ( {status} )",
            thousands(daily as i64)
        )),
    ])
}

/// The "today's word delta" row, with the net colored green (gain) or red (loss)
/// and the additions/deletions breakdown dimmed.
fn delta_line(momentum: &Momentum) -> Line<'static> {
    let mut spans = vec![Span::raw("today's word delta: ")];
    spans.extend(signed_delta_spans(momentum.added, momentum.removed));
    Line::from(spans)
}

/// A net word count as a signed, colored span: green with a `+` for a gain, red
/// for a loss. Shared by the delta line, the per-part rows, the daily goal, and
/// the writing-history view (#586).
pub fn colored_net_span(net: i64) -> Span<'static> {
    let theme = theme::current();
    let color = if net >= 0 { theme.added } else { theme.removed };
    let signed = if net >= 0 {
        format!("+{}", thousands(net))
    } else {
        thousands(net)
    };
    Span::styled(signed, Style::default().fg(color))
}

/// The colored net-delta span (green gain / red loss) followed by the dimmed
/// `(+added -removed)` breakdown. Shared by the global delta line and each
/// per-part row.
fn signed_delta_spans(added: usize, removed: usize) -> Vec<Span<'static>> {
    let net = added as i64 - removed as i64;
    vec![
        colored_net_span(net),
        Span::styled(
            format!(
                "   (+{} -{})",
                thousands(added as i64),
                thousands(removed as i64)
            ),
            Style::default().fg(theme::current().dim),
        ),
    ]
}

/// The "part deltas" block: a header (`"{label} deltas:"`) and one row per part
/// showing its name, current word count, and today's colored delta. Empty when
/// there are no parts (the block is then omitted entirely).
fn part_lines(parts: &[PartDelta], label: &str) -> Vec<Line<'static>> {
    if parts.is_empty() {
        return Vec::new();
    }
    // Align names and word counts into columns.
    let name_w = parts
        .iter()
        .map(|p| p.name.chars().count())
        .max()
        .unwrap_or(0);
    let words_w = parts
        .iter()
        .map(|p| thousands(p.words as i64).len())
        .max()
        .unwrap_or(0);

    let mut out = vec![Line::raw(format!("{} deltas:", label.to_lowercase()))];
    for p in parts {
        // When a part has a goal (#146), show its words against it; otherwise
        // just the count.
        let target = p.goal.map_or_else(String::new, |g| {
            format!(" / {} ( {} )", thousands(g as i64), percent(p.words, g))
        });
        let mut spans = vec![Span::raw(format!(
            "  {name:<name_w$}  {words:>words_w$} words{target}   ",
            name = p.name,
            words = thousands(p.words as i64),
        ))];
        spans.extend(signed_delta_spans(p.added, p.removed));
        out.push(Line::from(spans));
    }
    out
}

/// The revision-status block: cumulative words at Revised and at Final against
/// the body-word `total`, one row each (only when it has words), led by a blank
/// separator. Empty — no rows, no separator — when nothing has advanced past
/// draft, so a project that never sets statuses shows no status block at all.
fn status_lines(status: &StatusWords, total: usize) -> Vec<Line<'static>> {
    // Label column padded to align the counts under the story line's numbers.
    let row = |label: &str, words: usize| {
        Line::raw(format!(
            "{label:<8}  {} / {}  words ( {} )",
            thousands(words as i64),
            thousands(total as i64),
            percent(words, total),
        ))
    };
    let mut out = Vec::new();
    if status.revised > 0 {
        out.push(row("revised:", status.revised));
    }
    if status.final_words > 0 {
        out.push(row("final:", status.final_words));
    }
    if !out.is_empty() {
        out.insert(0, Line::raw(""));
    }
    out
}

/// The projected final manuscript, extrapolating the current chapter pace to
/// all the goal's chapters. `None` when there's nothing to project from yet.
#[derive(Clone, Copy)]
struct Projection {
    /// Projected total words (rounded).
    words: i64,
    /// Projected total pages.
    pages: i64,
    /// Projected length over the goal (the "expansion ratio").
    ratio: f64,
}

impl Projection {
    fn of(
        words: usize,
        chapters: usize,
        goals: &crate::project::Goals,
        wpp: usize,
    ) -> Option<Self> {
        if chapters == 0 || goals.chapters == 0 {
            return None;
        }
        let wp = words as f64 * goals.chapters as f64 / chapters as f64;
        Some(Self {
            words: wp.round() as i64,
            pages: (wp / wpp as f64).floor() as i64,
            ratio: if goals.words > 0 {
                wp / goals.words as f64
            } else {
                0.0
            },
        })
    }
}

/// The deadline read (#814): the current word pace measured against a target
/// completion date. `None` when no deadline is set, when there's no word goal to
/// measure against (0), or the goal is already met — nothing left to project.
#[derive(Clone, Copy)]
struct DeadlineProjection {
    /// The target completion date the author set.
    deadline: Date,
    /// Words still to write to reach the word goal (> 0 by construction).
    remaining: i64,
    /// Projected finish date at the current pace, or `None` when the pace is 0
    /// (no measurable rate yet) so no date can be projected.
    finish: Option<Date>,
    /// Words/day needed from `today` to hit the deadline. `None` when the deadline
    /// is today or already past (no whole days left to spread the work over).
    required: Option<i64>,
}

impl DeadlineProjection {
    /// Read `goals`' deadline against the current `rate` (words/day) as of `today`,
    /// given `words` written toward the word goal. `None` when there's no deadline,
    /// no word goal, or the goal is already met (nothing to project).
    fn of(goals: &crate::project::Goals, words: usize, rate: i64, today: Date) -> Option<Self> {
        let deadline = goals.deadline_date()?;
        let remaining = goals.words as i64 - words as i64;
        if goals.words == 0 || remaining <= 0 {
            return None;
        }
        // Projected finish: today + ceil(remaining / rate) days. Only when there's
        // a positive measured pace — a zero rate can't reach any date.
        let finish = (rate > 0).then(|| {
            let days = (remaining as f64 / rate as f64).ceil() as i64;
            date::add_days(today, days)
        });
        // Required pace: ceil(remaining / days-until-deadline), when the deadline is
        // still in the future (at least one whole day to spread the work over).
        let days_left = date::days_between(today, deadline);
        let required = (days_left > 0).then(|| (remaining as f64 / days_left as f64).ceil() as i64);
        Some(Self {
            deadline,
            remaining,
            finish,
            required,
        })
    }

    /// Whether the current pace is on track to hit the deadline: the projected
    /// finish lands on or before it. A past-due deadline (no `required` pace) or an
    /// unmeasurable pace (no `finish`) is never "on track".
    fn on_track(&self) -> bool {
        matches!(self.finish, Some(f) if f <= self.deadline)
    }

    /// The report's deadline lines: the target date, the projected finish and
    /// whether pace is ahead/behind, and the words/day the deadline needs.
    fn lines(&self) -> Vec<Line<'static>> {
        let mut out = vec![Line::raw(format!(
            "deadline:          {}",
            date::human(self.deadline)
        ))];
        // Projected finish + an ahead/behind verdict against the deadline.
        match self.finish {
            Some(finish) => {
                let verdict = if self.on_track() {
                    "on track \u{2713}"
                } else {
                    "behind"
                };
                out.push(Line::raw(format!(
                    "projected finish:  {} ( {verdict} )",
                    date::human(finish)
                )));
            }
            None => out.push(Line::raw("projected finish:  — (no pace yet)".to_string())),
        }
        // Words/day the deadline requires from today (or that it's already past).
        match self.required {
            Some(req) => out.push(Line::raw(format!(
                "need for deadline: {} words / day ( {} to go )",
                thousands(req),
                thousands(self.remaining),
            ))),
            None => out.push(Line::raw(
                "need for deadline: deadline reached — no days remaining".to_string(),
            )),
        }
        out
    }
}

/// A "to go" line: words remaining, days at `rate`, and the projected date.
fn to_go(label: &str, remaining: i64, rate: i64, today: Date) -> String {
    let words = remaining.max(0);
    let days = if rate > 0 {
        (words as f64 / rate as f64).round() as i64
    } else {
        0
    };
    format!(
        "{label} {} words ; {} days ( {} )",
        thousands(words),
        thousands(days),
        date::human(date::add_days(today, days)),
    )
}

/// `numerator / denominator` as a 2-dp percentage string, e.g. `108.80 %`.
fn percent(numerator: usize, denominator: usize) -> String {
    let pct = if denominator > 0 {
        numerator as f64 / denominator as f64 * 100.0
    } else {
        0.0
    };
    format!("{pct:.2} %")
}

/// Group an integer with thousands separators, e.g. `163,204`.
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests;
