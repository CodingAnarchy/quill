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
            "Set them in-app: Ctrl-K → \"goals:set\" (target words, chapters, daily).",
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
mod tests {
    use super::*;
    use crate::project::{Goals, Manifest};
    use ratatui::style::Color;

    fn manifest() -> Manifest {
        Manifest {
            title: "Red State Earth".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            epigraph_by: None,
            isbn: None,
            publisher: None,
            pub_date: None,
            language: None,
            rights: None,
            start_date: Some("2026-01-11".into()),
            todo_marker: None,
            part_label: None,
            run_on_threshold: None,
            style: None,
            print: None,
            analysis: None,
            goals: Some(Goals {
                words: 150_000,
                chapters: 60,
                words_per_page: 250,
                daily: None,
            }),
            parts: None,
            chapters: Vec::new(),
        }
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

    fn momentum() -> Momentum {
        Momentum {
            added: 3143,
            removed: 283,
            moved_to_unused: 0,
        }
    }

    #[test]
    fn reproduces_the_worked_example() {
        // The spec's sample, as of report date 2026-06-16 (156 days in).
        let today = date::parse("2026-06-16").unwrap();
        let rows = lines(
            &manifest(),
            163_204,
            0,
            56,
            12,
            &momentum(),
            &[],
            today,
            None,
        );
        let report = text(&rows);

        assert!(
            report.contains("story :   163,204 / 150,000  words ( 108.80 % )"),
            "{report}"
        );
        assert!(report.contains("652 / 600  pages"), "{report}");
        assert!(report.contains("56 / 60 chapters ( 93.33 % )"), "{report}");
        assert!(report.contains("expansion ratio: 1.17"), "{report}");
        assert!(report.contains("words at this ratio: 174,861"), "{report}");
        assert!(report.contains("pages at this ratio: 699"), "{report}");
        assert!(
            report.contains("163,204 / 174,861 words at this ratio ( 93.33 % )"),
            "{report}"
        );
        assert!(report.contains("xxx:               12"), "{report}");
        assert!(report.contains("moved to unused:   0"), "{report}");
        assert!(
            report.contains("today's word delta: +2,860   (+3,143 -283)"),
            "{report}"
        );
        assert!(report.contains("days since start:  156"), "{report}");
        assert!(
            report.contains("average rate:      1,046 words / day"),
            "{report}"
        );
        assert!(
            report.contains("to go:             0 words ; 0 days ( 16 Jun 2026 )"),
            "{report}"
        );
        assert!(
            report.contains("to go (expansion): 11,657 words ; 11 days ( 27 Jun 2026 )"),
            "{report}"
        );
    }

    #[test]
    fn daily_goal_shows_met_when_today_clears_it() {
        let today = date::parse("2026-06-16").unwrap();
        let mut m = manifest();
        m.goals.as_mut().unwrap().daily = Some(1_000); // net today is +2,860
        let report = text(&lines(&m, 163_204, 0, 56, 0, &momentum(), &[], today, None));
        assert!(
            report.contains("daily goal:         +2,860 / 1,000 words  ( met \u{2713} )"),
            "{report}"
        );
    }

    #[test]
    fn daily_goal_shows_words_remaining() {
        let today = date::parse("2026-06-16").unwrap();
        let mut m = manifest();
        m.goals.as_mut().unwrap().daily = Some(5_000); // net +2,860 of 5,000
        let report = text(&lines(&m, 163_204, 0, 56, 0, &momentum(), &[], today, None));
        assert!(
            report.contains("daily goal:         +2,860 / 5,000 words  ( 57% \u{b7} 2,140 to go )"),
            "{report}"
        );
    }

    #[test]
    fn no_daily_goal_shows_no_daily_line() {
        let today = date::parse("2026-06-16").unwrap();
        let report = text(&lines(
            &manifest(),
            163_204,
            0,
            56,
            0,
            &momentum(),
            &[],
            today,
            None,
        ));
        assert!(
            !report.contains("daily goal:"),
            "no daily line without a target: {report}"
        );
    }

    #[test]
    fn front_back_matter_words_are_shown_but_not_counted() {
        let today = date::parse("2026-06-16").unwrap();
        // 140k body words against a 150k goal, plus 8k in front/back matter.
        let rows = lines(
            &manifest(),
            140_000,
            8_000,
            56,
            0,
            &momentum(),
            &[],
            today,
            None,
        );
        let report = text(&rows);
        // The goal measures the body words, not body + sections.
        assert!(
            report.contains("story :   140,000 / 150,000  words ( 93.33 % )"),
            "{report}"
        );
        assert!(
            !report.contains("148,000"),
            "section words don't inflate the goal: {report}"
        );
        // The section total is shown, clearly excluded.
        assert!(
            report.contains("(+8,000 words in front/back matter, not counted)"),
            "{report}"
        );
    }

    #[test]
    fn no_section_words_means_no_extra_line() {
        let today = date::parse("2026-06-16").unwrap();
        let rows = lines(
            &manifest(),
            140_000,
            0,
            56,
            0,
            &momentum(),
            &[],
            today,
            None,
        );
        assert!(
            !text(&rows).contains("front/back matter"),
            "all-body report is unchanged"
        );
    }

    #[test]
    fn word_delta_is_colored_by_sign() {
        let today = date::parse("2026-06-16").unwrap();

        let gain = lines(&manifest(), 1000, 0, 2, 0, &momentum(), &[], today, None);
        let row = gain
            .iter()
            .find(|l| text(std::slice::from_ref(l)).contains("today's word delta"))
            .unwrap();
        assert_eq!(row.spans[1].content, "+2,860");
        assert_eq!(row.spans[1].style.fg, Some(Color::Green));

        let loss = Momentum {
            added: 10,
            removed: 300,
            moved_to_unused: 0,
        };
        let rows = lines(&manifest(), 1000, 0, 2, 0, &loss, &[], today, None);
        let row = rows
            .iter()
            .find(|l| text(std::slice::from_ref(l)).contains("today's word delta"))
            .unwrap();
        assert_eq!(row.spans[1].content, "-290");
        assert_eq!(row.spans[1].style.fg, Some(Color::Red));
    }

    #[test]
    fn renders_a_part_deltas_block() {
        let today = date::parse("2026-06-16").unwrap();
        let mut m = manifest();
        m.part_label = Some("Act".into()); // header should lowercase to "act deltas:"
        let parts = vec![
            PartDelta {
                name: "The Gathering".into(),
                words: 12_450,
                goal: Some(15_000),
                added: 1_486,
                removed: 159,
            },
            PartDelta {
                name: "Part 2".into(),
                words: 8_200,
                goal: None,
                added: 12,
                removed: 16,
            },
        ];
        let rows = lines(&m, 163_204, 0, 56, 12, &momentum(), &parts, today, None);
        let report = text(&rows);

        assert!(report.contains("act deltas:"), "{report}");
        assert!(report.contains("The Gathering"), "{report}");
        assert!(report.contains("12,450 words"), "{report}");
        // The part with a goal shows its progress; the one without doesn't (#146).
        assert!(
            report.contains("12,450 words / 15,000 ( 83.00 % )"),
            "{report}"
        );
        assert!(
            report.contains("8,200 words   "),
            "no-goal part shows just the count: {report}"
        );

        // The first part is a net gain (green +1,327), the second a loss (red -4).
        let gain = rows
            .iter()
            .find(|l| text(std::slice::from_ref(l)).contains("The Gathering"))
            .unwrap();
        let net = gain
            .spans
            .iter()
            .find(|s| s.content.contains("+1,327"))
            .unwrap();
        assert_eq!(net.style.fg, Some(Color::Green));
        let loss = rows
            .iter()
            .find(|l| text(std::slice::from_ref(l)).contains("Part 2"))
            .unwrap();
        let net = loss
            .spans
            .iter()
            .find(|s| s.content.trim() == "-4")
            .unwrap();
        assert_eq!(net.style.fg, Some(Color::Red));
    }

    #[test]
    fn no_parts_omits_the_block() {
        let today = date::parse("2026-06-16").unwrap();
        let rows = lines(
            &manifest(),
            163_204,
            0,
            56,
            12,
            &momentum(),
            &[],
            today,
            None,
        );
        assert!(
            !text(&rows).contains("deltas:") || text(&rows).contains("word delta:"),
            "{}",
            text(&rows)
        );
        assert!(!text(&rows).contains("part deltas:"), "{}", text(&rows));
    }

    #[test]
    fn no_goals_shows_a_hint() {
        let mut m = manifest();
        m.goals = None;
        let rows = lines(
            &m,
            1000,
            0,
            2,
            0,
            &momentum(),
            &[],
            date::parse("2026-06-16").unwrap(),
            None,
        );
        assert!(
            text(&rows).contains("No writing goals set."),
            "{}",
            text(&rows)
        );
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(652), "652");
        assert_eq!(thousands(163_204), "163,204");
        assert_eq!(thousands(-1_486), "-1,486");
    }
}
