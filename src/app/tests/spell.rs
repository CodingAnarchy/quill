//! Spell-check flagging wired into the editor (#357): the idle pass populates the
//! flag set for the active buffer, skipping the TODO marker.

use super::*;

#[test]
fn idle_flags_misspellings_and_skips_the_marker() {
    let (mut app, _dir) = two_chapter_app("spell-idle");
    // "quikc" is misspelled; "xxx" is the TODO marker; the rest are real words.
    app.buffer = Buffer::from_text("The quikc xxx fox");
    app.on_idle();

    assert_eq!(
        app.spell_flags.len(),
        1,
        "only the misspelling: {:?}",
        app.spell_flags
    );
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
    assert!(
        app.message.as_deref().unwrap_or("").contains("Added"),
        "{:?}",
        app.message
    );
    let file = fs::read_to_string(dir.join("dictionary.txt")).unwrap();
    assert!(file.contains("Vashti"), "persisted: {file}");
}

#[test]
fn accepted_words_survive_a_reload_and_others_still_flag() {
    // The acceptance criterion end to end (#935): accept a word, reopen the
    // project (a fresh App on the same dir), and it's no longer flagged — while a
    // different invented word still is, and real prose stays clean.
    let (mut app, dir) = two_chapter_app("spell-persist");
    app.buffer = Buffer::from_text("Vashti walked on");
    app.on_idle();
    let (s, _) = app.spell_flags[0];
    app.buffer.cursor = app.buffer.cursor_at_byte(s);
    app.accept_word_at_cursor();
    assert!(app.spell_flags.is_empty(), "accepted word clears now");

    // Reopen: the persisted word loads from disk, so it isn't re-flagged.
    let mut reopened = open_test_app(&dir);
    reopened.buffer = Buffer::from_text("Vashti met Aerendil");
    reopened.on_idle();
    assert_eq!(
        reopened.spell_flags.len(),
        1,
        "the accepted 'Vashti' stays clear; the unaccepted 'Aerendil' is flagged: {:?}",
        reopened.spell_flags
    );
    let (s, e) = reopened.spell_flags[0];
    assert_eq!(&reopened.buffer.text()[s..e], "Aerendil");

    reopened.buffer = Buffer::from_text("the quick brown fox");
    reopened.on_idle();
    assert!(
        reopened.spell_flags.is_empty(),
        "bundled behavior unchanged"
    );
}

#[test]
fn viewing_the_dictionary_opens_it_in_a_pane() {
    let (mut app, _dir) = two_chapter_app("spell-view");
    app.view_dictionary();
    let path = app.project.dictionary_path();
    let opened = match app.split_view() {
        Some(SplitView {
            content: SplitContent::Editor(other),
            ..
        }) => other.path.as_deref() == Some(path.as_path()),
        _ => false,
    };
    assert!(opened, "the dictionary file opens in an editor pane");
}

#[test]
fn editing_the_dictionary_pane_reloads_the_checker_on_close() {
    // Editing the dictionary in its pane and closing it takes effect without a
    // reopen (#935): a word added in the pane clears the flag right away.
    let (mut app, _dir) = two_chapter_app("spell-edit-pane");
    app.buffer = Buffer::from_text("Aerendil sailed");
    app.on_idle();
    assert_eq!(app.spell_flags.len(), 1, "the name is flagged first");

    // Open the dictionary pane, focus it, add the word, save, then close.
    app.view_dictionary();
    app.on_key(ctrl('w')); // focus the dictionary pane
    app.on_key(key(KeyCode::Char('w')));
    type_str(&mut app, "Aerendil\n");
    app.on_key(ctrl('s')); // write dictionary.txt
                           // Close the dictionary pane while it's focused: Vim's Ctrl-w c closes the
                           // focused pane, which returns to the manuscript and reloads the checker (the
                           // real path: edit the list, then close it to get back to writing).
    app.on_key(ctrl('w')); // Ctrl-w c closes the focused pane → reload + refresh
    app.on_key(key(KeyCode::Char('c')));
    assert!(app.split_view().is_none(), "the pane closed");

    // The edit reloaded into the checker without a project reopen, so a fresh
    // spell pass over prose using the word no longer flags it.
    assert!(
        app.project.is_accepted("Aerendil"),
        "the edit reloaded into the checker"
    );
    app.buffer = Buffer::from_text("Aerendil sailed");
    app.on_idle();
    assert!(
        app.spell_flags.is_empty(),
        "the newly accepted word is no longer flagged: {:?}",
        app.spell_flags
    );
}

#[test]
fn accepting_with_no_flag_at_the_cursor_says_so() {
    let (mut app, _dir) = two_chapter_app("spell-noflag");
    app.buffer = Buffer::from_text("all real words here");
    app.on_idle();
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.accept_word_at_cursor();
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No flagged word"),
        "{:?}",
        app.message
    );
}

#[test]
fn next_and_prev_jump_between_misspellings_and_wrap() {
    // "quikc" at byte 6, "wrnog" at byte 17; the rest are real words.
    let (mut app, _dir) = two_chapter_app("spell-nav");
    app.buffer = Buffer::from_text("alpha quikc beta wrnog gamma");
    app.on_idle();
    assert_eq!(app.spell_flags.len(), 2, "{:?}", app.spell_flags);

    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.navigate_spell_flag(1);
    assert_eq!(app.buffer.cursor_byte(), 6, "next → quikc");
    app.navigate_spell_flag(1);
    assert_eq!(app.buffer.cursor_byte(), 17, "next → wrnog");
    app.navigate_spell_flag(1);
    assert_eq!(app.buffer.cursor_byte(), 6, "next wraps → quikc");

    app.navigate_spell_flag(-1);
    assert_eq!(app.buffer.cursor_byte(), 17, "prev wraps → wrnog");
    app.navigate_spell_flag(-1);
    assert_eq!(app.buffer.cursor_byte(), 6, "prev → quikc");
}

#[test]
fn navigation_reports_when_none_remain() {
    let (mut app, _dir) = two_chapter_app("spell-nav-none");
    app.buffer = Buffer::from_text("alpha beta gamma");
    app.on_idle();
    app.navigate_spell_flag(1);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No spelling flags"),
        "{:?}",
        app.message
    );
}

#[test]
fn suggesting_offers_candidates_and_picking_one_replaces_the_word() {
    let (mut app, _dir) = two_chapter_app("spell-suggest");
    app.buffer = Buffer::from_text("The quikc brown fox");
    app.on_idle();
    assert_eq!(app.spell_flags.len(), 1, "{:?}", app.spell_flags);

    // Land on the flagged word and open the suggestions pick-list.
    let (s, e) = app.spell_flags[0];
    app.buffer.cursor = app.buffer.cursor_at_byte(s);
    app.suggest_word_at_cursor();

    // The list is a picker offering the correct spelling.
    let Mode::Palette(picker) = &app.mode else {
        panic!("suggestions open a picker");
    };
    assert!(!picker.items.is_empty(), "candidates offered");
    let quick = picker
        .items
        .iter()
        .find(|i| i.label == "quick")
        .expect("the correction 'quick' is among the candidates");
    let crate::mode::Command::ReplaceMisspelling {
        start,
        end,
        replacement,
        ..
    } = quick.command.clone()
    else {
        panic!("a suggestion carries a replace command");
    };
    assert_eq!(
        (start, end),
        (s, e),
        "the command targets the flagged range"
    );

    // Picking it replaces exactly the flagged word, and the flag clears.
    app.replace_misspelling(start, end, "quikc", &replacement);
    assert_eq!(app.buffer.text(), "The quick brown fox");
    app.on_idle();
    assert!(app.spell_flags.is_empty(), "the fix clears the flag");
}

#[test]
fn suggesting_with_no_flag_at_the_cursor_says_so() {
    let (mut app, _dir) = two_chapter_app("spell-suggest-noflag");
    app.buffer = Buffer::from_text("all real words here");
    app.on_idle();
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.suggest_word_at_cursor();
    assert!(
        matches!(app.mode, Mode::Insert | Mode::Normal),
        "no picker without a flag"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No flagged word"),
        "{:?}",
        app.message
    );
}

#[test]
fn replacing_a_stale_range_declines_rather_than_corrupting() {
    let (mut app, _dir) = two_chapter_app("spell-suggest-stale");
    app.buffer = Buffer::from_text("The quikc brown fox");
    app.on_idle();
    let (s, e) = app.spell_flags[0];
    // The buffer changed since the list opened: the range no longer holds "quikc".
    app.buffer = Buffer::from_text("Totally different text now");
    app.replace_misspelling(s, e, "quikc", "quick");
    assert_eq!(
        app.buffer.text(),
        "Totally different text now",
        "a stale replace leaves the text untouched"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("changed"),
        "{:?}",
        app.message
    );
}

#[test]
fn vim_z_equals_opens_the_suggestions_picker() {
    let (mut app, _dir) = two_chapter_app("spell-suggest-vim");
    app.vim = true;
    app.mode = Mode::Normal;
    app.buffer = Buffer::from_text("The quikc brown fox");
    app.on_idle();
    let (s, _) = app.spell_flags[0];
    app.buffer.cursor = app.buffer.cursor_at_byte(s);
    app.on_key(key(KeyCode::Char('z')));
    app.on_key(key(KeyCode::Char('=')));
    assert!(matches!(app.mode, Mode::Palette(_)), "z= opens the picker");
}

#[test]
fn modeless_alt_equals_opens_the_suggestions_picker() {
    let (mut app, _dir) = two_chapter_app("spell-suggest-alt");
    app.buffer = Buffer::from_text("The quikc brown fox");
    app.on_idle();
    let (s, _) = app.spell_flags[0];
    app.buffer.cursor = app.buffer.cursor_at_byte(s);
    app.on_key(KeyEvent::new(KeyCode::Char('='), KeyModifiers::ALT));
    assert!(
        matches!(app.mode, Mode::Palette(_)),
        "Alt-= opens the picker"
    );
}

#[test]
fn navigation_skips_a_word_thats_since_been_fixed() {
    let (mut app, _dir) = two_chapter_app("spell-nav-fixed");
    app.buffer = Buffer::from_text("quikc here");
    app.on_idle();
    assert_eq!(app.spell_flags.len(), 1);
    // Fix the word without an idle pass; navigation refreshes and finds none.
    app.buffer = Buffer::from_text("quick here");
    app.navigate_spell_flag(1);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No spelling flags"),
        "{:?}",
        app.message
    );
}

#[test]
fn the_settled_pass_keeps_spell_flags_on_their_words_while_typing() {
    // Between idle passes the flags aren't recomputed, only carried through the
    // burst's edits, so an underline never lands on shifted text mid-typing.
    let (mut app, _dir) = two_chapter_app("spell-remap");
    app.buffer = Buffer::from_text("The quikc fox\n");
    app.on_idle();
    assert_eq!(app.spell_flags.len(), 1);

    // Type before the misspelling, one batch per key.
    for c in "Big ".chars() {
        app.on_key(key(KeyCode::Char(c)));
        app.on_input_settled();
    }
    assert_eq!(app.buffer.text(), "Big The quikc fox\n");
    let (s, e) = app.spell_flags[0];
    assert_eq!(
        &app.buffer.text()[s..e],
        "quikc",
        "the flag moved with its word"
    );

    // An undo swaps the rope without edit deltas: the settled pass recomputes.
    app.on_idle();
    app.on_key(ctrl('z'));
    app.on_input_settled();
    let (s, e) = app.spell_flags[0];
    assert_eq!(
        &app.buffer.text()[s..e],
        "quikc",
        "recomputed after the undo"
    );
}

#[test]
fn a_quiet_pass_after_typing_re_judges_the_remapped_flags() {
    // While typing the flags are only remapped, so a word typed mid-burst isn't
    // judged yet. The quiet pass that follows must re-judge it, even though the
    // remap kept the flags aligned with the current text.
    let (mut app, _dir) = two_chapter_app("spell-quiet-rejudge");
    app.buffer = Buffer::from_text("The fox\n");
    app.on_idle();
    assert!(app.spell_flags.is_empty());

    for c in "quikc ".chars() {
        app.on_key(key(KeyCode::Char(c)));
        app.on_input_settled();
    }
    assert!(app.spell_flags.is_empty(), "remapped, not yet re-judged");

    app.on_quiet();
    assert_eq!(app.spell_flags.len(), 1, "{:?}", app.spell_flags);
    let (s, e) = app.spell_flags[0];
    assert_eq!(&app.buffer.text()[s..e], "quikc");
}

#[test]
fn a_cursor_only_quiet_pass_keeps_the_judged_flags() {
    // Nothing the spell judgment reads changed, so the quiet pass leaves the flags
    // alone rather than re-scanning the window. (A marker value stands in for the
    // judged flags, so a rescan would visibly replace it.)
    let (mut app, _dir) = two_chapter_app("spell-quiet-skip");
    app.buffer = Buffer::from_text("The quikc fox\n");
    app.on_idle();
    assert_eq!(app.spell_flags.len(), 1);
    let marker = vec![(0, 3)];
    app.spell_flags.clone_from(&marker);

    app.on_key(key(KeyCode::Right));
    app.on_quiet();
    assert_eq!(app.spell_flags, marker, "no rescan for a cursor move");
}

#[test]
fn a_word_accepted_elsewhere_clears_on_the_next_quiet_pass() {
    // The dictionary can grow outside the accept-at-cursor action (e.g. the
    // add-these-names path, #303). The buffer is unchanged, but the next quiet pass
    // must still re-judge against the new dictionary.
    let (mut app, _dir) = two_chapter_app("spell-quiet-dictionary");
    app.buffer = Buffer::from_text("Vashti walked on");
    app.on_idle();
    assert_eq!(app.spell_flags.len(), 1, "the invented name is flagged");

    assert!(app.project.accept_word("Vashti").unwrap());
    app.on_quiet();
    assert!(app.spell_flags.is_empty(), "{:?}", app.spell_flags);
}
