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
