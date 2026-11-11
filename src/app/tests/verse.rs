//! Verse analysis in the editor (#1324): a chapter's target meter is set from
//! the chapter browser, persists in its front matter, and drives the verse
//! report.

use super::*;
use crate::mode::Action;

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
