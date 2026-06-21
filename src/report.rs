//! The progress report: a single screen tracking the manuscript against the
//! author's goals. This module is pure — it turns the project's goals plus a
//! few measured numbers into the report's text lines, so the formulas are
//! testable against the worked example in [`docs/progress-reports.md`].
//!
//! The daily-momentum lines (today's delta, per-act deltas, moved-to-unused)
//! need the snapshot log and land with #59/#60; this renders the static lines.
//! Those deltas are color-coded (green gains / red losses), so #59 should change
//! [`lines`] to return styled rows (`Vec<Line>` or a small typed enum the UI
//! colors) and add delta rows — not re-parse these strings for `+`/`-`.

use ratatui::style::{Color, Style};
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

/// Build the report's rows for a project with `words` total words, `chapters`
/// chapters, `todo` TODO markers, and today's `momentum`, as of `today`. Rows
/// are styled (the word delta is colored). Returns a short hint when no goals
/// are set.
pub fn lines(
    manifest: &Manifest,
    words: usize,
    chapters: usize,
    todo: usize,
    momentum: &Momentum,
    parts: &[PartDelta],
    today: Date,
) -> Vec<Line<'static>> {
    let Some(goals) = &manifest.goals else {
        return vec![
            Line::raw("No writing goals set."),
            Line::raw("Add a [goals] block to book.toml (words, chapters)."),
        ];
    };

    let mut out = Vec::new();
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

    // --- Projection: extrapolate current chapter pace to the whole book ----
    // f = chapter completion; projected words Wp = W / f. Computed once.
    let projection = Projection::of(words, chapters, goals, wpp);
    if let Some(p) = projection {
        out.push(Line::raw(""));
        out.push(Line::raw(format!("          expansion ratio: {:.2}", p.ratio)));
        out.push(Line::raw(format!("          words at this ratio: {}", thousands(p.words))));
        out.push(Line::raw(format!("          pages at this ratio: {}", thousands(p.pages))));
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
    out.extend(part_lines(parts, manifest.part_label()));

    // --- Rate & projected completion --------------------------------------
    if let Some(start) = manifest.start_date.as_deref().and_then(date::parse) {
        let days = date::days_between(start, today).max(0);
        let rate = words as i64 / days.max(1); // floor(W / days)
        out.push(Line::raw(""));
        out.push(Line::raw(format!("days since start:  {}", thousands(days))));
        out.push(Line::raw(format!("average rate:      {} words / day", thousands(rate))));
        out.push(Line::raw(to_go("to go:            ", goals.words as i64 - words as i64, rate, today)));
        if let Some(p) = projection {
            out.push(Line::raw(to_go("to go (expansion):", p.words - words as i64, rate, today)));
        }
    }

    out
}

/// The "today's word delta" row, with the net colored green (gain) or red (loss)
/// and the additions/deletions breakdown dimmed.
fn delta_line(momentum: &Momentum) -> Line<'static> {
    let mut spans = vec![Span::raw("today's word delta: ")];
    spans.extend(signed_delta_spans(momentum.added, momentum.removed));
    Line::from(spans)
}

/// The colored net-delta span (green gain / red loss) followed by the dimmed
/// `(+added -removed)` breakdown. Shared by the global delta line and each
/// per-part row.
fn signed_delta_spans(added: usize, removed: usize) -> Vec<Span<'static>> {
    let net = added as i64 - removed as i64;
    let color = if net >= 0 { Color::Green } else { Color::Red };
    let signed = if net >= 0 {
        format!("+{}", thousands(net))
    } else {
        thousands(net)
    };
    vec![
        Span::styled(signed, Style::default().fg(color)),
        Span::styled(
            format!("   (+{} -{})", thousands(added as i64), thousands(removed as i64)),
            Style::default().fg(Color::DarkGray),
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
    let name_w = parts.iter().map(|p| p.name.chars().count()).max().unwrap_or(0);
    let words_w = parts
        .iter()
        .map(|p| thousands(p.words as i64).len())
        .max()
        .unwrap_or(0);

    let mut out = vec![Line::raw(format!("{} deltas:", label.to_lowercase()))];
    for p in parts {
        // When a part has a goal (#146), show its words against it; otherwise
        // just the count.
        let target = match p.goal {
            Some(g) => format!(" / {} ( {} )", thousands(g as i64), percent(p.words, g)),
            None => String::new(),
        };
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
    fn of(words: usize, chapters: usize, goals: &crate::project::Goals, wpp: usize) -> Option<Projection> {
        if chapters == 0 || goals.chapters == 0 {
            return None;
        }
        let wp = words as f64 * goals.chapters as f64 / chapters as f64;
        Some(Projection {
            words: wp.round() as i64,
            pages: (wp / wpp as f64).floor() as i64,
            ratio: if goals.words > 0 { wp / goals.words as f64 } else { 0.0 },
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
fn thousands(n: i64) -> String {
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

    fn manifest() -> Manifest {
        Manifest {
            title: "Red State Earth".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            epigraph_by: None,
            start_date: Some("2026-01-11".into()),
            todo_marker: None,
            part_label: None,
            run_on_threshold: None,
            goals: Some(Goals {
                words: 150_000,
                chapters: 60,
                words_per_page: 250,
            }),
            parts: None,
            chapters: Vec::new(),
        }
    }

    /// The plain text of styled rows, for content assertions.
    fn text(rows: &[Line]) -> String {
        rows.iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
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
        let rows = lines(&manifest(), 163_204, 56, 12, &momentum(), &[], today);
        let report = text(&rows);

        assert!(report.contains("story :   163,204 / 150,000  words ( 108.80 % )"), "{report}");
        assert!(report.contains("652 / 600  pages"), "{report}");
        assert!(report.contains("56 / 60 chapters ( 93.33 % )"), "{report}");
        assert!(report.contains("expansion ratio: 1.17"), "{report}");
        assert!(report.contains("words at this ratio: 174,861"), "{report}");
        assert!(report.contains("pages at this ratio: 699"), "{report}");
        assert!(report.contains("163,204 / 174,861 words at this ratio ( 93.33 % )"), "{report}");
        assert!(report.contains("xxx:               12"), "{report}");
        assert!(report.contains("moved to unused:   0"), "{report}");
        assert!(report.contains("today's word delta: +2,860   (+3,143 -283)"), "{report}");
        assert!(report.contains("days since start:  156"), "{report}");
        assert!(report.contains("average rate:      1,046 words / day"), "{report}");
        assert!(report.contains("to go:             0 words ; 0 days ( 16 Jun 2026 )"), "{report}");
        assert!(report.contains("to go (expansion): 11,657 words ; 11 days ( 27 Jun 2026 )"), "{report}");
    }

    #[test]
    fn word_delta_is_colored_by_sign() {
        let today = date::parse("2026-06-16").unwrap();

        let gain = lines(&manifest(), 1000, 2, 0, &momentum(), &[], today);
        let row = gain.iter().find(|l| text(std::slice::from_ref(l)).contains("today's word delta")).unwrap();
        assert_eq!(row.spans[1].content, "+2,860");
        assert_eq!(row.spans[1].style.fg, Some(Color::Green));

        let loss = Momentum { added: 10, removed: 300, moved_to_unused: 0 };
        let rows = lines(&manifest(), 1000, 2, 0, &loss, &[], today);
        let row = rows.iter().find(|l| text(std::slice::from_ref(l)).contains("today's word delta")).unwrap();
        assert_eq!(row.spans[1].content, "-290");
        assert_eq!(row.spans[1].style.fg, Some(Color::Red));
    }

    #[test]
    fn renders_a_part_deltas_block() {
        let today = date::parse("2026-06-16").unwrap();
        let mut m = manifest();
        m.part_label = Some("Act".into()); // header should lowercase to "act deltas:"
        let parts = vec![
            PartDelta { name: "The Gathering".into(), words: 12_450, goal: Some(15_000), added: 1_486, removed: 159 },
            PartDelta { name: "Part 2".into(), words: 8_200, goal: None, added: 12, removed: 16 },
        ];
        let rows = lines(&m, 163_204, 56, 12, &momentum(), &parts, today);
        let report = text(&rows);

        assert!(report.contains("act deltas:"), "{report}");
        assert!(report.contains("The Gathering"), "{report}");
        assert!(report.contains("12,450 words"), "{report}");
        // The part with a goal shows its progress; the one without doesn't (#146).
        assert!(report.contains("12,450 words / 15,000 ( 83.00 % )"), "{report}");
        assert!(report.contains("8,200 words   "), "no-goal part shows just the count: {report}");

        // The first part is a net gain (green +1,327), the second a loss (red -4).
        let gain = rows.iter().find(|l| text(std::slice::from_ref(l)).contains("The Gathering")).unwrap();
        let net = gain.spans.iter().find(|s| s.content.contains("+1,327")).unwrap();
        assert_eq!(net.style.fg, Some(Color::Green));
        let loss = rows.iter().find(|l| text(std::slice::from_ref(l)).contains("Part 2")).unwrap();
        let net = loss.spans.iter().find(|s| s.content.trim() == "-4").unwrap();
        assert_eq!(net.style.fg, Some(Color::Red));
    }

    #[test]
    fn no_parts_omits_the_block() {
        let today = date::parse("2026-06-16").unwrap();
        let rows = lines(&manifest(), 163_204, 56, 12, &momentum(), &[], today);
        assert!(!text(&rows).contains("deltas:") || text(&rows).contains("word delta:"), "{}", text(&rows));
        assert!(!text(&rows).contains("part deltas:"), "{}", text(&rows));
    }

    #[test]
    fn no_goals_shows_a_hint() {
        let mut m = manifest();
        m.goals = None;
        let rows = lines(&m, 1000, 2, 0, &momentum(), &[], date::parse("2026-06-16").unwrap());
        assert!(text(&rows).contains("No writing goals set."), "{}", text(&rows));
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(652), "652");
        assert_eq!(thousands(163_204), "163,204");
        assert_eq!(thousands(-1_486), "-1,486");
    }
}
