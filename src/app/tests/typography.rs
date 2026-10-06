//! Smart typography (#296): typing prose substitutes curly quotes, em-dashes,
//! and ellipses when the setting is on, inserts verbatim when off, and the
//! substituted characters persist to disk.

use super::*;

/// A two-chapter app with smart typography forced on and an empty active buffer
/// to type fresh prose into. (The shared `two_chapter_app` pins it off.)
fn smart_app(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.flags.smart_typography = true;
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
fn smart_typography_still_fires_inside_a_fenced_block() {
    // Fenced blocks hold "log line"-style prose (transmissions, letters), which
    // still wants curly quotes and em-dashes — typography is not suppressed (#522).
    let (mut app, _dir) = smart_app("typo-fence");
    type_str(&mut app, "```\n\"Wait--no\"\n```");
    assert!(
        app.buffer.text().contains("\u{201C}Wait\u{2014}no\u{201D}"),
        "typography applies inside the fence: {:?}",
        app.buffer.text()
    );
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
    app.flags.smart_typography = true;
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

// --- Epigraph metadata edits get the same pass as prose (#519) -------------

/// The #519 reproduction string and its smart-typeset form.
const RAW_EPIGRAPH: &str = "He said \"wait\" -- now...";
const SMART_EPIGRAPH: &str = "He said \u{201C}wait\u{201D} \u{2014} now\u{2026}";

#[test]
fn chapter_epigraph_fields_get_smart_typography() {
    let (mut app, _dir) = smart_app("typo-chapter-epigraph");
    let at = app.project.active_index();
    app.apply_meta(MetaField::ChapterEpigraph { at }, RAW_EPIGRAPH);
    app.apply_meta(MetaField::ChapterEpigraphBy { at }, "A -- B...");

    let ep = app.chapter_epigraph(at).expect("epigraph set");
    assert_eq!(ep.text, SMART_EPIGRAPH);
    assert_eq!(ep.attribution.as_deref(), Some("A \u{2014} B\u{2026}"));
}

#[test]
fn book_epigraph_fields_get_smart_typography() {
    let (mut app, _dir) = smart_app("typo-book-epigraph");
    app.apply_meta(MetaField::BookEpigraph, RAW_EPIGRAPH);
    app.apply_meta(MetaField::BookEpigraphBy, "A -- B...");

    let m = &app.project.manifest;
    assert_eq!(m.epigraph.as_deref(), Some(SMART_EPIGRAPH));
    assert_eq!(m.epigraph_by.as_deref(), Some("A \u{2014} B\u{2026}"));
}

#[test]
fn epigraph_fields_left_verbatim_when_smart_typography_off() {
    let (mut app, _dir) = two_chapter_app("typo-epigraph-off"); // pins it off
    let at = app.project.active_index();
    app.apply_meta(MetaField::ChapterEpigraph { at }, RAW_EPIGRAPH);
    app.apply_meta(MetaField::BookEpigraph, RAW_EPIGRAPH);

    assert_eq!(app.chapter_epigraph(at).unwrap().text, RAW_EPIGRAPH);
    assert_eq!(app.project.manifest.epigraph.as_deref(), Some(RAW_EPIGRAPH));
}

#[test]
fn non_epigraph_meta_fields_stay_verbatim_even_when_smart_on() {
    // A summary holds raw notes, not display prose — no substitution (#519).
    let (mut app, _dir) = smart_app("typo-summary-verbatim");
    let at = app.project.active_index();
    app.apply_meta(MetaField::Summary { at }, RAW_EPIGRAPH);
    assert_eq!(app.project.chapter_summary(at), Some(RAW_EPIGRAPH));
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
