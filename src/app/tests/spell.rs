//! Spell-check flagging wired into the editor (#357): the idle pass populates the
//! flag set for the active buffer, skipping the TODO marker.

use super::*;

#[test]
fn idle_flags_misspellings_and_skips_the_marker() {
    let (mut app, _dir) = two_chapter_app("spell-idle");
    // "quikc" is misspelled; "xxx" is the TODO marker; the rest are real words.
    app.buffer = Buffer::from_text("The quikc xxx fox");
    app.on_idle();

    assert_eq!(app.spell_flags.len(), 1, "only the misspelling: {:?}", app.spell_flags);
    let (s, e) = app.spell_flags[0];
    assert_eq!(&app.buffer.text()[s..e], "quikc");
}

#[test]
fn real_prose_produces_no_flags() {
    let (mut app, _dir) = two_chapter_app("spell-clean");
    app.buffer = Buffer::from_text("The quick brown fox jumps over the lazy dog.");
    app.on_idle();
    assert!(app.spell_flags.is_empty(), "{:?}", app.spell_flags);
}
