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

#[test]
fn accepting_a_flagged_word_clears_it_and_persists() {
    let (mut app, dir) = two_chapter_app("spell-accept");
    app.buffer = Buffer::from_text("Vashti walked on");
    app.on_idle();
    assert_eq!(app.spell_flags.len(), 1, "the invented name is flagged");

    // Put the cursor on the flagged word and accept it.
    let (s, _) = app.spell_flags[0];
    app.buffer.cursor = app.buffer.cursor_at_byte(s);
    app.accept_word_at_cursor();

    assert!(app.spell_flags.is_empty(), "flag cleared after accepting");
    assert!(app.message.as_deref().unwrap_or("").contains("Added"), "{:?}", app.message);
    let file = fs::read_to_string(dir.join("dictionary.txt")).unwrap();
    assert!(file.contains("Vashti"), "persisted: {file}");
}

#[test]
fn accepting_with_no_flag_at_the_cursor_says_so() {
    let (mut app, _dir) = two_chapter_app("spell-noflag");
    app.buffer = Buffer::from_text("all real words here");
    app.on_idle();
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.accept_word_at_cursor();
    assert!(app.message.as_deref().unwrap_or("").contains("No flagged word"), "{:?}", app.message);
}
