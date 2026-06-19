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

use crate::date::Date;
use crate::project::Manifest;

/// Build the report's content lines for a project with `words` total words and
/// `chapters` chapters, `todo` TODO markers, as of `today`. Returns a short
/// hint when no goals are set.
pub fn lines(manifest: &Manifest, words: usize, chapters: usize, todo: usize, today: Date) -> Vec<String> {
    let Some(goals) = &manifest.goals else {
        return vec![
            "No writing goals set.".to_string(),
            "Add a [goals] block to book.toml (words, chapters).".to_string(),
        ];
    };

    let mut out = Vec::new();
    let wpp = goals.words_per_page.max(1);
    let pages = (words / wpp) as i64;

    // --- Story: completion against the goals -------------------------------
    out.push(format!(
        "story :   {} / {}  words ( {} )",
        thousands(words as i64),
        thousands(goals.words as i64),
        percent(words, goals.words),
    ));
    out.push(format!(
        "          {} / {}  pages",
        thousands(pages),
        thousands((goals.words / wpp) as i64),
    ));
    out.push(format!(
        "          {} / {} chapters ( {} )",
        chapters,
        goals.chapters,
        percent(chapters, goals.chapters),
    ));

    // --- Projection: extrapolate current chapter pace to the whole book ----
    // f = chapter completion; projected words Wp = W / f. Computed once.
    let projection = Projection::of(words, chapters, goals, wpp);
    if let Some(p) = projection {
        out.push(String::new());
        out.push(format!("          expansion ratio: {:.2}", p.ratio));
        out.push(format!("          words at this ratio: {}", thousands(p.words)));
        out.push(format!("          pages at this ratio: {}", thousands(p.pages)));
        out.push(String::new());
        out.push(format!(
            "          {} / {} words at this ratio ( {} )",
            thousands(words as i64),
            thousands(p.words),
            percent(chapters, goals.chapters), // == W / Wp by construction
        ));
        out.push(format!("          {} / {} pages at this ratio", thousands(pages), thousands(p.pages)));
    }

    // --- Markers ----------------------------------------------------------
    out.push(String::new());
    out.push(format!("xxx:               {todo}"));

    // --- Rate & projected completion --------------------------------------
    if let Some(start) = manifest.start_date.as_deref().and_then(Date::parse) {
        let days = today.days_since(start).max(0);
        let rate = words as i64 / days.max(1); // floor(W / days)
        out.push(String::new());
        out.push(format!("days since start:  {}", thousands(days)));
        out.push(format!("average rate:      {} words / day", thousands(rate)));
        out.push(to_go("to go:            ", goals.words as i64 - words as i64, rate, today));
        if let Some(p) = projection {
            out.push(to_go("to go (expansion):", p.words - words as i64, rate, today));
        }
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
        today.add_days(days),
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
            goals: Some(Goals {
                words: 150_000,
                chapters: 60,
                words_per_page: 250,
            }),
            chapters: Vec::new(),
        }
    }

    #[test]
    fn reproduces_the_worked_example() {
        // The spec's sample, as of report date 2026-06-16 (156 days in).
        let today = Date::parse("2026-06-16").unwrap();
        let report = lines(&manifest(), 163_204, 56, 12, today).join("\n");

        assert!(report.contains("story :   163,204 / 150,000  words ( 108.80 % )"), "{report}");
        assert!(report.contains("652 / 600  pages"), "{report}");
        assert!(report.contains("56 / 60 chapters ( 93.33 % )"), "{report}");
        assert!(report.contains("expansion ratio: 1.17"), "{report}");
        assert!(report.contains("words at this ratio: 174,861"), "{report}");
        assert!(report.contains("pages at this ratio: 699"), "{report}");
        assert!(report.contains("163,204 / 174,861 words at this ratio ( 93.33 % )"), "{report}");
        assert!(report.contains("xxx:               12"), "{report}");
        assert!(report.contains("days since start:  156"), "{report}");
        assert!(report.contains("average rate:      1,046 words / day"), "{report}");
        assert!(report.contains("to go:             0 words ; 0 days ( 16 Jun 2026 )"), "{report}");
        assert!(report.contains("to go (expansion): 11,657 words ; 11 days ( 27 Jun 2026 )"), "{report}");
    }

    #[test]
    fn no_goals_shows_a_hint() {
        let mut m = manifest();
        m.goals = None;
        let report = lines(&m, 1000, 2, 0, Date::parse("2026-06-16").unwrap()).join("\n");
        assert!(report.contains("No writing goals set."), "{report}");
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(652), "652");
        assert_eq!(thousands(163_204), "163,204");
        assert_eq!(thousands(-1_486), "-1,486");
    }
}
