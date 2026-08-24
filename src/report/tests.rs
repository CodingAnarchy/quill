//! Tests for the progress report's formulas and rendering. They drive [`lines`]
//! end to end against the worked example in `docs/progress-reports.md`, then
//! cover each block on its own: the daily-goal line, the colored word delta and
//! per-part deltas, front/back-matter words, the revision-status breakdown,
//! digit grouping, and the deadline projection.

use super::*;
use crate::project::{Goals, Manifest};
use ratatui::style::Color;

fn manifest() -> Manifest {
    Manifest {
        format_version: None,
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
            deadline: None,
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

/// No revision progress — the default for tests not exercising the status block.
fn no_status() -> StatusWords {
    StatusWords {
        revised: 0,
        final_words: 0,
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
        &no_status(),
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
    let report = text(&lines(
        &m,
        163_204,
        0,
        56,
        0,
        &no_status(),
        &momentum(),
        &[],
        today,
        None,
    ));
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
    let report = text(&lines(
        &m,
        163_204,
        0,
        56,
        0,
        &no_status(),
        &momentum(),
        &[],
        today,
        None,
    ));
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
        &no_status(),
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
        &no_status(),
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
        &no_status(),
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

    let gain = lines(
        &manifest(),
        1000,
        0,
        2,
        0,
        &no_status(),
        &momentum(),
        &[],
        today,
        None,
    );
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
    let rows = lines(
        &manifest(),
        1000,
        0,
        2,
        0,
        &no_status(),
        &loss,
        &[],
        today,
        None,
    );
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
    let rows = lines(
        &m,
        163_204,
        0,
        56,
        12,
        &no_status(),
        &momentum(),
        &parts,
        today,
        None,
    );
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
        &no_status(),
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
        &no_status(),
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
fn revision_status_block_shows_revised_and_final_against_the_body_total() {
    let today = date::parse("2026-06-16").unwrap();
    // 100k body words: 70k has reached at least Revised (cumulative), 40k of
    // which is Final — so final ≤ revised, both measured against the 100k total.
    let status = StatusWords {
        revised: 70_000,
        final_words: 40_000,
    };
    let rows = lines(
        &manifest(),
        100_000,
        0,
        56,
        0,
        &status,
        &momentum(),
        &[],
        today,
        None,
    );
    let report = text(&rows);
    assert!(
        report.contains("revised:  70,000 / 100,000  words ( 70.00 % )"),
        "{report}"
    );
    assert!(
        report.contains("final:    40,000 / 100,000  words ( 40.00 % )"),
        "{report}"
    );
}

#[test]
fn revision_status_lines_are_omitted_when_empty() {
    let today = date::parse("2026-06-16").unwrap();
    // Nothing past draft → no revised line, no final line, and the report is
    // otherwise unchanged (only the story/momentum blocks show).
    let rows = lines(
        &manifest(),
        100_000,
        0,
        56,
        0,
        &no_status(),
        &momentum(),
        &[],
        today,
        None,
    );
    let report = text(&rows);
    assert!(!report.contains("revised:"), "{report}");
    assert!(!report.contains("final:"), "{report}");
}

#[test]
fn revision_status_shows_only_the_stage_with_words() {
    let today = date::parse("2026-06-16").unwrap();
    // Some revised, none final yet → the revised line appears alone.
    let status = StatusWords {
        revised: 25_000,
        final_words: 0,
    };
    let rows = lines(
        &manifest(),
        100_000,
        0,
        56,
        0,
        &status,
        &momentum(),
        &[],
        today,
        None,
    );
    let report = text(&rows);
    assert!(
        report.contains("revised:  25,000 / 100,000  words ( 25.00 % )"),
        "{report}"
    );
    assert!(!report.contains("final:"), "{report}");
}

#[test]
fn thousands_groups_digits() {
    assert_eq!(thousands(0), "0");
    assert_eq!(thousands(652), "652");
    assert_eq!(thousands(163_204), "163,204");
    assert_eq!(thousands(-1_486), "-1,486");
}

// --- Deadline-aware projection (#814) ----------------------------------
//
// All hermetic (#531): the report date and the deadline are injected, never
// wall-clock. The rate the deadline math reads is the report's own
// `words / days_since_start` — with start 2026-01-11 and today 2026-03-12
// (60 days), 60,000 body words is exactly 1,000 words/day, so the projected
// finish and the ahead/behind verdict are deterministic.

/// A manifest with a word/chapter goal, a fixed start date, and `deadline`.
fn manifest_with_deadline(deadline: Option<&str>) -> Manifest {
    let mut m = manifest();
    m.goals.as_mut().unwrap().deadline = deadline.map(str::to_string);
    m
}

#[test]
fn deadline_behind_shows_projected_finish_and_required_pace() {
    // 60k words at 1,000/day → finish 10 Jun 2026, past the 1 May deadline.
    let today = date::parse("2026-03-12").unwrap();
    let rows = lines(
        &manifest_with_deadline(Some("2026-05-01")),
        60_000,
        0,
        24,
        0,
        &no_status(),
        &momentum(),
        &[],
        today,
        None,
    );
    let report = text(&rows);
    assert!(report.contains("deadline:          1 May 2026"), "{report}");
    assert!(
        report.contains("projected finish:  10 Jun 2026 ( behind )"),
        "{report}"
    );
    // 90,000 to go over 50 days = 1,800 words/day required.
    assert!(
        report.contains("need for deadline: 1,800 words / day ( 90,000 to go )"),
        "{report}"
    );
}

#[test]
fn deadline_ahead_shows_on_track() {
    // Same 1,000/day pace, but a far-off deadline the finish beats.
    let today = date::parse("2026-03-12").unwrap();
    let rows = lines(
        &manifest_with_deadline(Some("2026-12-31")),
        60_000,
        0,
        24,
        0,
        &no_status(),
        &momentum(),
        &[],
        today,
        None,
    );
    let report = text(&rows);
    assert!(
        report.contains("projected finish:  10 Jun 2026 ( on track \u{2713} )"),
        "{report}"
    );
    // 90,000 over 294 days rounds up to 307 words/day.
    assert!(
        report.contains("need for deadline: 307 words / day ( 90,000 to go )"),
        "{report}"
    );
}

#[test]
fn deadline_already_past_reports_no_days_remaining() {
    let today = date::parse("2026-03-12").unwrap();
    let rows = lines(
        &manifest_with_deadline(Some("2026-03-01")),
        60_000,
        0,
        24,
        0,
        &no_status(),
        &momentum(),
        &[],
        today,
        None,
    );
    let report = text(&rows);
    // Past due: still shows the projected finish (behind), but no required pace.
    assert!(
        report.contains("projected finish:  10 Jun 2026 ( behind )"),
        "{report}"
    );
    assert!(
        report.contains("need for deadline: deadline reached — no days remaining"),
        "{report}"
    );
}

#[test]
fn no_deadline_omits_the_deadline_lines() {
    let today = date::parse("2026-03-12").unwrap();
    let rows = lines(
        &manifest_with_deadline(None),
        60_000,
        0,
        24,
        0,
        &no_status(),
        &momentum(),
        &[],
        today,
        None,
    );
    let report = text(&rows);
    // The existing projection is untouched; none of the deadline lines appear.
    assert!(!report.contains("deadline:"), "{report}");
    assert!(!report.contains("projected finish:"), "{report}");
    assert!(!report.contains("need for deadline:"), "{report}");
}

#[test]
fn deadline_omitted_once_the_word_goal_is_met() {
    // Goal already reached → nothing to project against the deadline.
    let today = date::parse("2026-03-12").unwrap();
    let rows = lines(
        &manifest_with_deadline(Some("2026-05-01")),
        150_000,
        0,
        24,
        0,
        &no_status(),
        &momentum(),
        &[],
        today,
        None,
    );
    let report = text(&rows);
    assert!(!report.contains("deadline:"), "{report}");
    assert!(!report.contains("need for deadline:"), "{report}");
}
