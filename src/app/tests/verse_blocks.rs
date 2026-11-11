//! Verse inside prose (#1337): a fenced ```verse block is reported and marked
//! against its own targets, and lines the author picks can be checked on the
//! spot.

use super::*;
use crate::mode::{Action, ExportFormat};

/// The plain text of each line of the open overlay.
fn overlay_text(app: &App) -> Vec<String> {
    app.overlay_lines()
        .expect("an overlay is open")
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

/// Chapter one is prose with a pentameter couplet fenced inside it, one line
/// off the meter; chapter two is prose with a block naming no target.
const ONE: &str = "She sang it slowly.\n\n```verse iambic pentameter\nShall I compare thee to a summer's day?\nThe cat sat on the mat\n```\n\nThen she stopped.\n";
const TWO: &str = "He hummed.\n\n```verse\nA dog, happy, lay by the lamp at night\n```\n";

fn song_app(name: &str) -> (App, TempProject) {
    let dir = two_chapter_dir(name);
    fs::write(dir.join("one.md"), ONE).unwrap();
    fs::write(dir.join("two.md"), TWO).unwrap();
    let app = open_test_app(&dir);
    (app, dir)
}

#[test]
fn the_report_checks_a_fenced_block_and_not_the_prose_around_it() {
    let (mut app, _dir) = song_app("verse-block-report");
    app.run_action(Action::VerseReport, false);
    let text = overlay_text(&app).join("\n");
    assert!(
        text.contains("One \u{b7} verse from line 4 \u{b7} iambic pentameter"),
        "{text}"
    );
    assert!(text.contains("Meter: 1 of 2 lines fit"), "{text}");
    // Numbered as lines of the chapter: the couplet is lines 4 and 5.
    assert!(
        text.lines()
            .any(|l| l.trim_start().starts_with("5 \u{2717}") && l.contains("The cat")),
        "{text}"
    );
    assert!(
        !text.contains("She sang"),
        "the prose isn't scanned: {text}"
    );
    assert!(!text.contains("Then she stopped"), "{text}");
    // The second chapter's block names no target, and the chapter has none.
    assert!(text.contains("Two \u{b7} verse from line 4"), "{text}");
    assert!(text.contains("No target to check against"), "{text}");
}

#[test]
fn a_block_with_no_target_takes_the_chapters_and_a_bad_one_says_why() {
    let dir = two_chapter_dir("verse-block-inherit");
    fs::write(
        dir.join("one.md"),
        format!("---\nmeter: trochaic tetrameter\n---\n{TWO}"),
    )
    .unwrap();
    fs::write(dir.join("two.md"), "```verse iambik\nA line\n```\n").unwrap();
    let mut app = open_test_app(&dir);
    app.run_action(Action::VerseReport, false);
    let text = overlay_text(&app).join("\n");
    assert!(
        text.contains("One \u{b7} verse from line 4 \u{b7} trochaic tetrameter"),
        "{text}"
    );
    assert!(
        text.contains("Checked against the chapter's targets."),
        "{text}"
    );
    assert!(
        text.contains("Couldn't read \"iambik\" as a meter or rhyme scheme"),
        "{text}"
    );
}

#[test]
fn the_report_says_how_to_start_when_theres_nothing_to_check() {
    let (mut app, _dir) = two_chapter_app("verse-block-nothing");
    app.run_action(Action::VerseReport, false);
    assert!(app.overlay.is_none());
    let message = app.message.clone().unwrap_or_default();
    assert!(message.contains("```verse"), "{message}");
}

#[test]
fn the_gutter_marks_a_fenced_block_and_stepping_finds_its_faults() {
    let (mut app, _dir) = song_app("verse-block-marks");
    app.on_key(key(KeyCode::F(3)));
    let starts: Vec<usize> = app.scansion.lines.iter().map(|l| l.line_start).collect();
    let at = |s: &str| ONE.find(s).expect("the line");
    assert_eq!(starts, [at("Shall"), at("The cat")]);
    assert_eq!(app.scansion.fault_lines(), [at("The cat")]);

    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.run_action(Action::NextScansionFault, false);
    assert_eq!(app.buffer.cursor_byte(), at("The cat"));
    // Chapter two's block has nothing to check against, so there's nowhere
    // further to go.
    app.run_action(Action::NextScansionFault, false);
    assert_eq!(app.project.active_index(), 0);
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("end of the manuscript"));
}

#[test]
fn selected_lines_are_checked_against_the_targets_typed() {
    let dir = two_chapter_dir("verse-check-selection");
    fs::write(
        dir.join("one.md"),
        "A tavern, late.\n\nShall I compare thee to a summer's day?\nThe cat sat on the mat\n\nShe laughed.\n",
    )
    .unwrap();
    let mut app = open_test_app(&dir);
    let text = app.buffer.text();
    let first = text.find("Shall").expect("line one");
    let last = text.find("The cat").expect("line two") + 3;
    app.mode = Mode::Visual {
        anchor: first,
        linewise: false,
    };
    app.buffer.cursor = app.buffer.cursor_at_byte(last);
    app.on_key(ctrl('k'));
    app.run_action(Action::CheckVerse, false);
    type_str(&mut app, "iambic pentameter aa");
    app.on_key(key(KeyCode::Enter));

    let report = overlay_text(&app).join("\n");
    assert!(
        report.contains("One \u{b7} lines 3\u{2013}4 \u{b7} iambic pentameter \u{b7} rhymed AA"),
        "{report}"
    );
    assert!(report.contains("Meter: 1 of 2 lines fit"), "{report}");
    assert!(
        report.contains("Rhyme: 1 of 2 line endings fit"),
        "{report}"
    );
    assert!(!report.contains("tavern"), "{report}");
    assert_eq!(app.buffer.text(), text, "checking never changes the text");
}

#[test]
fn a_one_line_selection_checks_just_that_line() {
    let dir = two_chapter_dir("verse-check-one-line");
    fs::write(
        dir.join("one.md"),
        "Shall I compare thee to a summer's day?\nThe cat sat on the mat\n",
    )
    .unwrap();
    let mut app = open_test_app(&dir);
    let at = app.buffer.text().find("The cat").expect("line two");
    app.mode = Mode::Visual {
        anchor: at,
        linewise: true,
    };
    app.buffer.cursor = app.buffer.cursor_at_byte(at + 2);
    app.on_key(ctrl('k'));
    app.run_action(Action::CheckVerse, false);
    type_str(&mut app, "iambic pentameter");
    app.on_key(key(KeyCode::Enter));
    let report = overlay_text(&app).join("\n");
    assert!(
        report.contains("One \u{b7} line 2 \u{b7} iambic pentameter"),
        "{report}"
    );
    assert!(report.contains("Meter: 0 of 1 lines fit"), "{report}");
    // The prompt closed under the report, so keys after it reach the editor.
    assert!(!matches!(app.mode, Mode::Prompt(_)));
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Char('i')));
    app.on_key(key(KeyCode::Char('x')));
    assert!(app.overlay.is_none());
}

#[test]
fn with_no_selection_the_stanza_at_the_cursor_is_checked() {
    let dir = two_chapter_dir("verse-check-stanza");
    fs::write(
        dir.join("one.md"),
        "Prose here.\n\nShall I compare thee to a summer's day?\nThe cat sat on the mat\n\nMore prose.\n",
    )
    .unwrap();
    let mut app = open_test_app(&dir);
    let at = app.buffer.text().find("The cat").expect("the line");
    app.buffer.cursor = app.buffer.cursor_at_byte(at);
    app.run_action(Action::CheckVerse, false);
    // A misspelled meter is refused, and says what would do.
    type_str(&mut app, "iambik");
    app.on_key(key(KeyCode::Enter));
    assert!(app.overlay.is_none());
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("Name a meter, a rhyme scheme, or both"));

    // Asked again, it offers what was last used: nothing yet, so the
    // chapter's targets, which are none.
    app.run_action(Action::CheckVerse, false);
    type_str(&mut app, "iambic pentameter");
    app.on_key(key(KeyCode::Enter));
    let report = overlay_text(&app).join("\n");
    assert!(report.contains("One \u{b7} lines 3\u{2013}4"), "{report}");
    assert!(report.contains("Meter: 1 of 2 lines fit"), "{report}");

    // On a blank line there's no stanza to check.
    app.overlay = None;
    let blank = app.buffer.text().find("\n\nMore").expect("the gap") + 1;
    app.buffer.cursor = app.buffer.cursor_at_byte(blank);
    app.run_action(Action::CheckVerse, false);
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("put the cursor in a stanza"));
}

#[test]
fn a_fenced_block_exports_as_verse() {
    let (mut app, dir) = song_app("verse-block-export");
    app.run_action(Action::Export(ExportFormat::Markdown), false);
    let md = fs::read_to_string(dir.join("export/book.md")).unwrap();
    assert!(!md.contains("```"), "{md}");
    assert!(
        md.contains("Shall I compare thee to a summer's day?\\\nThe cat sat on the mat"),
        "{md}"
    );
}

/// The idle scansion pass over a long prose chapter carrying a few fenced songs
/// (#1337): only the songs are scanned, so a ~13k-word chapter costs about what
/// its verse does, far inside the long poem's 50ms budget.
#[test]
#[ignore = "timing guard: run with `cargo perf` (release)"]
fn scansion_refresh_is_fast_for_prose_with_songs() {
    let prose = "The road ran on past the mill and the river, and nobody on it said a word about the weather or the war. ".repeat(6);
    let song = "```verse iambic pentameter abab\nShall I compare thee to a summer's day?\nThou art more lovely and more temperate:\nRough winds do shake the darling buds of May,\nAnd summer's lease hath all too short a date;\n```\n\n";
    let mut body = String::new();
    for i in 0..600 {
        body.push_str(&prose);
        body.push_str("\n\n");
        if i % 100 == 0 {
            body.push_str(song);
        }
    }
    let dir = two_chapter_dir("verse-block-perf");
    fs::write(dir.join("one.md"), body).unwrap();
    let mut app = open_test_app(&dir);
    crate::verse::pronounce::warm();
    app.scansion_on = true;
    app.refresh_scansion();
    assert_eq!(app.scansion.lines.len(), 24, "six songs of four lines");
    let runs = 5;
    let start = std::time::Instant::now();
    for _ in 0..runs {
        app.invalidate_revision_highlights();
        app.refresh_scansion();
    }
    let each = start.elapsed() / runs;
    eprintln!("scansion refresh, prose with six songs: {each:?}");
    assert!(each.as_millis() < 10, "took {each:?}");
}
