//! Verse analysis in the editor (#1324): a chapter's target meter is set from
//! the chapter browser, persists in its front matter, and drives the verse
//! report.

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

#[test]
fn a_target_meter_is_set_from_the_browser_and_survives_reopen() {
    let (mut app, dir) = two_chapter_app("verse-set");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // "Two", not the active chapter
    app.on_key(key(KeyCode::Char('V')));
    type_str(&mut app, "Iambic  Pentameter");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(
        app.project.chapter_meter(1).map(Meter::key).as_deref(),
        Some("iambic pentameter")
    );
    assert!(fs::read_to_string(dir.join("two.md"))
        .unwrap()
        .contains("meter: iambic pentameter"));
    let reopened = Project::open(&dir).unwrap();
    assert!(reopened.chapter_meter(1).is_some());

    // Blank clears it.
    app.on_key(key(KeyCode::Char('V')));
    for _ in 0.."iambic pentameter".len() {
        app.on_key(key(KeyCode::Backspace));
    }
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.project.chapter_meter(1), None);
    assert!(!fs::read_to_string(dir.join("two.md"))
        .unwrap()
        .contains("meter:"));
}

#[test]
fn an_unknown_meter_is_rejected_with_examples() {
    let (mut app, _dir) = two_chapter_app("verse-bad");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('V')));
    type_str(&mut app, "loose iambs");
    app.on_key(key(KeyCode::Enter));
    let message = app.message.clone().unwrap_or_default();
    assert!(
        message.contains("Unknown meter") && message.contains("blank verse"),
        "{message}"
    );
    assert_eq!(app.project.chapter_meter(0), None);
}

#[test]
fn the_verse_report_scans_only_chapters_with_a_target() {
    let (mut app, _dir) = two_chapter_app("verse-report");
    app.run_action(Action::VerseReport, false);
    assert!(app.overlay_lines().is_none(), "nothing to report yet");
    assert!(app
        .message
        .as_deref()
        .is_some_and(|m| m.contains("No chapter has a target meter or rhyme scheme")));

    // The active chapter is read live, unsaved edits and all.
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('V')));
    type_str(&mut app, "blank verse");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Esc));
    app.buffer =
        Buffer::from_text("Shall I compare thee to a summer's day?\nThe cat sat on the mat\n");
    app.run_action(Action::VerseReport, false);
    let text = overlay_text(&app);
    assert!(
        text.iter().any(|l| l == "One \u{b7} blank verse"),
        "{text:#?}"
    );
    assert!(
        text.iter().any(|l| l.contains("1 of 2 lines fit (50%)")),
        "{text:#?}"
    );
    assert!(text
        .iter()
        .any(|l| l.contains("\u{2713} 10/10") && l.contains("summer's day")));
    assert!(
        text.iter().any(|l| l.contains("4 syllables short")),
        "{text:#?}"
    );
    assert!(
        !text.iter().any(|l| l.starts_with("Two")),
        "Two has no target"
    );
}

#[test]
fn a_rhyme_scheme_is_set_and_reported_beside_the_meter() {
    let (mut app, _dir) = two_chapter_app("verse-rhyme");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('H')));
    type_str(&mut app, "abab");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.project
            .chapter_rhyme(0)
            .map(RhymeScheme::key)
            .as_deref(),
        Some("ABAB")
    );
    app.on_key(key(KeyCode::Char('V')));
    type_str(&mut app, "iambic trimeter");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Esc));
    app.buffer = Buffer::from_text(
        "The wind was on the bay\nThe gulls were in the sky\nThe boats came home by day\nAnd no one wondered when\n",
    );
    app.run_action(Action::VerseReport, false);
    let text = overlay_text(&app);
    assert!(
        text.iter()
            .any(|l| l == "One \u{b7} iambic trimeter \u{b7} rhymed ABAB"),
        "{text:#?}"
    );
    assert!(
        text.iter()
            .any(|l| l.contains("Rhyme: 3 of 4 line endings fit (75%)")),
        "{text:#?}"
    );
    assert!(
        text.iter()
            .any(|l| l.contains("\"when\" doesn't rhyme with line 2 (no rhyme)")),
        "{text:#?}"
    );
    assert!(text.iter().any(|l| l.contains("\u{2717} C/B")), "{text:#?}");
}

#[test]
fn an_unknown_rhyme_scheme_is_rejected_with_examples() {
    let (mut app, _dir) = two_chapter_app("verse-rhyme-bad");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('H')));
    type_str(&mut app, "A-B-A-B");
    app.on_key(key(KeyCode::Enter));
    let message = app.message.clone().unwrap_or_default();
    assert!(
        message.contains("Unknown rhyme scheme") && message.contains("terza rima"),
        "{message}"
    );
    assert_eq!(app.project.chapter_rhyme(0), None);
}

/// Put the cursor at the start of `word` in the active buffer.
fn cursor_on(app: &mut App, word: &str) {
    let at = app
        .buffer
        .text()
        .find(word)
        .expect("the word is in the buffer");
    app.buffer.cursor = app.buffer.cursor_at_byte(at + 1);
}

#[test]
fn the_word_at_the_cursor_gets_the_authors_pronunciation() {
    let (mut app, dir) = two_chapter_app("verse-pronounce");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('V')));
    type_str(&mut app, "iambic pentameter");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Esc));
    app.buffer = Buffer::from_text("And every fair from fair sometime declines,\n");
    app.run_action(Action::VerseReport, false);
    assert!(overlay_text(&app).iter().any(|l| l.contains("(sometime)")));
    app.overlay = None;

    // The prompt opens on the word, prefilled with the dictionary's stress.
    cursor_on(&mut app, "sometime");
    app.run_action(Action::SetPronunciation, false);
    match &app.mode {
        Mode::Prompt(p) => {
            assert_eq!(p.input, "12", "the dictionary's SOME-time");
            assert!(p.label.contains("'sometime'"), "{}", p.label);
        }
        _ => panic!("the pronunciation prompt is open"),
    }
    for _ in 0..2 {
        app.on_key(key(KeyCode::Backspace));
    }
    type_str(&mut app, "01");
    app.on_key(key(KeyCode::Enter));
    let file = fs::read_to_string(dir.join("pronunciations.txt")).unwrap();
    assert!(file.starts_with("# How words sound"), "{file}");
    assert!(file.ends_with("sometime = 01\n"), "{file}");

    // The report now hears some-TIME, and the line fits.
    app.run_action(Action::VerseReport, false);
    let text = overlay_text(&app);
    assert!(
        text.iter().any(|l| l.contains("1 of 1 lines fit")),
        "{text:#?}"
    );
    app.overlay = None;

    // Reopened, the project still hears it that way.
    let reopened = Project::open(&dir).unwrap();
    assert!(reopened.lexicon().get("Sometime").is_some());

    // Blank hands the word back to the dictionary.
    cursor_on(&mut app, "sometime");
    app.run_action(Action::SetPronunciation, false);
    for _ in 0..2 {
        app.on_key(key(KeyCode::Backspace));
    }
    app.on_key(key(KeyCode::Enter));
    assert!(app.project.lexicon().is_empty());
    assert!(!fs::read_to_string(dir.join("pronunciations.txt"))
        .unwrap()
        .contains("sometime ="));
}

#[test]
fn enter_closes_the_pronunciation_prompt_and_typing_goes_on() {
    // The prompt used to stay open after Enter with its input taken and its
    // caret left past the end, so the next letter typed crashed Quill.
    let (mut app, _dir) = two_chapter_app("verse-pronounce-closes");
    app.buffer = Buffer::from_text("And every fair from fair sometime declines,\n");
    cursor_on(&mut app, "sometime");
    app.run_action(Action::SetPronunciation, false);
    app.on_key(key(KeyCode::Enter));
    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "Enter closes the prompt"
    );
    app.on_key(key(KeyCode::Char('x')));
    assert!(app.buffer.text().contains('x'), "the key reaches the text");
}

#[test]
fn a_bad_pronunciation_is_refused_with_a_reason() {
    let (mut app, dir) = two_chapter_app("verse-pronounce-bad");
    app.buffer = Buffer::from_text("Hermione walked\n");
    cursor_on(&mut app, "Hermione");
    app.run_action(Action::SetPronunciation, false);
    type_str(&mut app, "x");
    app.on_key(key(KeyCode::Enter));
    let message = app.message.clone().unwrap_or_default();
    assert!(message.contains("isn't a stress digit"), "{message}");
    assert!(!dir.join("pronunciations.txt").exists());
}

#[test]
fn editing_the_pronunciations_pane_reloads_them_on_close() {
    let (mut app, _dir) = two_chapter_app("verse-pronounce-pane");
    app.run_action(Action::EditPronunciations, false);
    assert!(app.split_view().is_some(), "the pane opened");
    app.on_key(ctrl('w'));
    app.on_key(key(KeyCode::Char('w')));
    type_str(&mut app, "Quessaly = 102\nbroken\n");
    app.on_key(ctrl('s'));
    app.on_key(ctrl('w'));
    app.on_key(key(KeyCode::Char('c')));
    assert!(app.split_view().is_none(), "the pane closed");
    assert!(
        app.project.lexicon().get("quessaly").is_some(),
        "reloaded on close"
    );
    let message = app.message.clone().unwrap_or_default();
    assert!(
        message.contains("line 2"),
        "the bad line is reported: {message}"
    );
}

#[test]
fn a_verse_chapter_keeps_its_lines_on_export() {
    let (mut app, dir) = two_chapter_app("verse-export");
    fs::write(dir.join("two.md"), "The tide came in\nand went away\n").unwrap();
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('L')));
    assert!(app.project.chapter_is_verse(1));
    assert!(fs::read_to_string(dir.join("two.md"))
        .unwrap()
        .contains("verse: true"));
    app.on_key(key(KeyCode::Esc));
    app.run_action(Action::Export(ExportFormat::Markdown), false);
    let out = fs::read_to_string(dir.join("export/book.md")).unwrap();
    assert!(out.contains("The tide came in\\\nand went away"), "{out}");

    // Toggled off, the lines join again; a chapter with a meter stays verse.
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('L')));
    assert!(!app.project.chapter_is_verse(1));
    app.on_key(key(KeyCode::Char('V')));
    type_str(&mut app, "blank verse");
    app.on_key(key(KeyCode::Enter));
    assert!(app.project.chapter_is_verse(1), "a meter makes it verse");
    app.on_key(key(KeyCode::Char('L')));
    assert!(app.project.chapter_is_verse(1));
    assert!(
        app.message.as_deref().unwrap_or("").contains("still verse"),
        "{:?}",
        app.message
    );
}

/// A two-chapter project in pentameter, each chapter with one line off it.
fn pentameter_app(name: &str) -> (App, TempProject) {
    let dir = two_chapter_dir(name);
    fs::write(
        dir.join("one.md"),
        "---\nmeter: iambic pentameter\n---\nShall I compare thee to a summer's day?\nThe cat sat on the mat\nThou art more lovely and more temperate:\n",
    )
    .unwrap();
    fs::write(
        dir.join("two.md"),
        "---\nmeter: iambic pentameter\n---\nRough winds do shake the darling buds of May,\nA dog, happy, lay by the lamp at night\n",
    )
    .unwrap();
    let app = open_test_app(&dir);
    (app, dir)
}

#[test]
fn the_scansion_marks_toggle_and_are_remembered_for_the_project() {
    let (mut app, dir) = pentameter_app("verse-marks-toggle");
    assert!(!app.scansion_on, "off by default");
    app.on_key(key(KeyCode::F(3)));
    assert!(app.scansion_on);
    assert_eq!(
        app.scansion.lines.len(),
        3,
        "each verse line has a gutter row"
    );
    assert_eq!(app.scansion.fault_lines().len(), 1);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("3 lines, 1 off"),
        "{:?}",
        app.message
    );
    assert!(open_test_app(&dir).scansion_on, "remembered on reopen");

    app.run_action(Action::ToggleScansion, false);
    assert!(!app.scansion_on);
    assert!(app.scansion.lines.is_empty());
    assert!(!open_test_app(&dir).scansion_on, "and off again");
}

#[test]
fn the_marks_clear_while_typing_and_return_on_idle() {
    let (mut app, _dir) = pentameter_app("verse-marks-idle");
    app.on_key(key(KeyCode::F(3)));
    assert!(!app.scansion.faults.is_empty());
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.on_key(key(KeyCode::Char('O')));
    assert!(
        app.scansion.lines.is_empty() && app.scansion.faults.is_empty(),
        "an edit drops marks that point at old offsets"
    );
    app.on_idle();
    assert_eq!(app.scansion.lines.len(), 3, "back on the next idle pass");
}

#[test]
fn a_chapter_without_a_target_shows_nothing() {
    let (mut app, _dir) = two_chapter_app("verse-marks-prose");
    app.on_key(key(KeyCode::F(3)));
    assert!(app.scansion_on && app.scansion.lines.is_empty());
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("Nothing here has a target"));
}

#[test]
fn stepping_finds_the_lines_off_the_meter_across_chapters() {
    let (mut app, _dir) = pentameter_app("verse-marks-step");
    app.on_key(key(KeyCode::F(3)));
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.run_action(Action::NextScansionFault, false);
    let line = |app: &App| app.buffer.line_of_byte(app.buffer.cursor_byte());
    assert_eq!(
        (app.project.active_index(), line(&app)),
        (0, 1),
        "The cat …"
    );
    app.run_action(Action::NextScansionFault, false);
    assert_eq!(
        app.project.active_index(),
        1,
        "crossed into the next chapter"
    );
    assert_eq!(line(&app), 1, "A dog, happy …");
    assert_eq!(app.scansion.lines.len(), 2, "the marks follow the chapter");
    app.run_action(Action::NextScansionFault, false);
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("end of the manuscript"));
    app.run_action(Action::PrevScansionFault, false);
    assert_eq!(
        (app.project.active_index(), line(&app)),
        (0, 1),
        "back again"
    );
}

#[test]
fn the_continuous_view_marks_every_chapter_in_it() {
    let (mut app, _dir) = pentameter_app("verse-marks-continuous");
    app.toggle_continuous();
    app.on_key(key(KeyCode::F(3)));
    assert_eq!(app.scansion.lines.len(), 5, "both chapters' lines");
    assert_eq!(app.scansion.fault_lines().len(), 2);
}

/// The idle scansion pass (#1326) over a book-length verse chapter: 1,500 lines
/// of pentameter, scanned and rhyme-checked. It runs on the idle tick, never per
/// keystroke, and once per edit burst (it's cached by revision), but it still
/// runs on the UI thread, so it must stay well under a pause in typing.
#[test]
#[ignore = "timing guard: run with `cargo perf` (release)"]
fn scansion_refresh_is_fast_for_a_long_poem() {
    let stanza = "Shall I compare thee to a summer's day?\nThou art more lovely and more temperate:\nRough winds do shake the darling buds of May,\nAnd summer's lease hath all too short a date;\n\n";
    let dir = two_chapter_dir("verse-marks-perf");
    fs::write(
        dir.join("one.md"),
        format!(
            "---\nmeter: iambic pentameter\nrhyme: ABAB\n---\n{}",
            stanza.repeat(375)
        ),
    )
    .unwrap();
    let mut app = open_test_app(&dir);
    crate::verse::pronounce::warm();
    app.scansion_on = true;
    // Warm the caches the first pass fills, then time a fresh pass.
    app.refresh_scansion();
    assert_eq!(app.scansion.lines.len(), 1500);
    let runs = 5;
    let start = std::time::Instant::now();
    for _ in 0..runs {
        app.invalidate_revision_highlights();
        app.refresh_scansion();
    }
    let each = start.elapsed() / runs;
    eprintln!("scansion refresh, 1,500 lines: {each:?}");
    assert!(each.as_millis() < 50, "took {each:?}");
}
