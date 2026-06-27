//! Smart typography (#296): typing prose substitutes curly quotes, em-dashes,
//! and ellipses when the setting is on, inserts verbatim when off, and the
//! substituted characters persist to disk.

use super::*;

/// A two-chapter app with smart typography forced on and an empty active buffer
/// to type fresh prose into. (The shared `two_chapter_app` pins it off.)
fn smart_app(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.smart_typography = true;
    app.buffer = Buffer::from_text("");
    (app, dir)
}

#[test]
fn quotes_orient_from_context() {
    let (mut app, _dir) = smart_app("typo-quotes");
    type_str(&mut app, "\"Wait,\" she said. \"Don't.\"");
    assert_eq!(
        app.buffer.text(),
        "\u{201C}Wait,\u{201D} she said. \u{201C}Don\u{2019}t.\u{201D}"
    );
}

#[test]
fn double_hyphen_and_triple_dot_substitute() {
    let (mut app, _dir) = smart_app("typo-dash");
    type_str(&mut app, "Wait--no...");
    assert_eq!(app.buffer.text(), "Wait\u{2014}no\u{2026}");
}

#[test]
fn a_lone_hyphen_and_dot_stay_literal() {
    let (mut app, _dir) = smart_app("typo-literal");
    type_str(&mut app, "well-being. end.");
    assert_eq!(app.buffer.text(), "well-being. end.");
}

#[test]
fn off_inserts_punctuation_verbatim() {
    let (mut app, _dir) = two_chapter_app("typo-off"); // pins smart_typography off
    app.buffer = Buffer::from_text("");
    type_str(&mut app, "\"a--b...\"");
    assert_eq!(app.buffer.text(), "\"a--b...\"");
}

#[test]
fn substitutions_persist_to_disk_and_reload() {
    let (mut app, dir) = two_chapter_app("typo-roundtrip");
    app.smart_typography = true;
    // Type into the active chapter (one.md), then save and read it back.
    type_str(&mut app, "He said \"hi--bye...\" ");
    app.on_key(ctrl('s'));

    let saved = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(
        saved.contains("He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}"),
        "curly punctuation persisted: {saved:?}"
    );

    // And it reloads byte-for-byte (the substituted glyphs survive a round-trip).
    let reloaded = Buffer::from_path(dir.join("one.md")).unwrap();
    assert!(reloaded
        .text()
        .contains("\u{201C}hi\u{2014}bye\u{2026}\u{201D}"));
}

#[test]
fn substitution_is_one_undo_with_the_typing_burst() {
    let (mut app, _dir) = smart_app("typo-undo");
    type_str(&mut app, "a--b");
    assert_eq!(app.buffer.text(), "a\u{2014}b");
    app.buffer.seal_undo_group();
    assert!(app.buffer.undo());
    assert_eq!(
        app.buffer.text(),
        "",
        "undo reverts the whole burst, em-dash included"
    );
}
