//! Tests for the opt-in Vim modal-editing foundation (#201): Normal / Insert
//! modes, entry/exit keys, basic motion, and that the modeless default and the
//! shared `Ctrl-` shortcuts are unaffected.

use super::*;

fn vim_app(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.vim = true;
    app.mode = Mode::Normal;
    (app, dir)
}

#[test]
fn normal_mode_keys_do_not_insert_text() {
    let (mut app, _dir) = vim_app("vim-normal-no-insert");
    let original = app.buffer.text();
    // An unmapped letter is a no-op, never typed input.
    app.on_key(key(KeyCode::Char('z')));
    assert_eq!(
        app.buffer.text(),
        original,
        "letters are commands, not input"
    );
    assert!(matches!(app.mode, Mode::Normal));
}

#[test]
fn i_enters_insert_then_esc_returns_to_normal() {
    let (mut app, _dir) = vim_app("vim-i-esc");
    app.on_key(key(KeyCode::Char('i')));
    assert!(matches!(app.mode, Mode::Insert), "i enters Insert");
    type_str(&mut app, "Hi");
    assert!(
        app.buffer.text().starts_with("Hifirst chapter"),
        "{}",
        app.buffer.text()
    );
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Normal), "Esc returns to Normal");
}

#[test]
fn a_appends_after_the_cursor() {
    let (mut app, _dir) = vim_app("vim-append");
    // Cursor starts at col 0; `a` enters Insert one grapheme to the right.
    app.on_key(key(KeyCode::Char('a')));
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(
        app.buffer.cursor.col, 1,
        "append moves past the first grapheme"
    );
    type_str(&mut app, "X");
    assert!(
        app.buffer.text().starts_with("fXirst chapter"),
        "{}",
        app.buffer.text()
    );
}

#[test]
fn capital_a_appends_at_line_end() {
    let (mut app, _dir) = vim_app("vim-A");
    app.on_key(key(KeyCode::Char('A')));
    assert!(matches!(app.mode, Mode::Insert));
    type_str(&mut app, "!");
    assert!(
        app.buffer.text().starts_with("first chapter!"),
        "{}",
        app.buffer.text()
    );
}

#[test]
fn o_opens_a_line_below_and_capital_o_above() {
    let (mut app, _dir) = vim_app("vim-open");
    app.on_key(key(KeyCode::Char('o')));
    assert!(matches!(app.mode, Mode::Insert), "o enters Insert");
    type_str(&mut app, "new");
    assert_eq!(app.buffer.text(), "first chapter\nnew\n", "o opens below");

    // Back to Normal, then O opens above the current (now second) line.
    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Char('O')));
    type_str(&mut app, "top");
    assert_eq!(
        app.buffer.text(),
        "first chapter\ntop\nnew\n",
        "{}",
        app.buffer.text()
    );
}

#[test]
fn hjkl_moves_the_cursor_in_normal_mode() {
    let (mut app, _dir) = vim_app("vim-motion");
    assert_eq!(app.buffer.cursor.col, 0);
    app.on_key(key(KeyCode::Char('l')));
    assert_eq!(app.buffer.cursor.col, 1, "l moves right");
    app.on_key(key(KeyCode::Char('h')));
    assert_eq!(app.buffer.cursor.col, 0, "h moves left");
}

#[test]
fn ctrl_shortcuts_still_work_in_normal_mode() {
    let (mut app, _dir) = vim_app("vim-ctrl");
    // Ctrl-g focuses the chapter browser, the same as in Insert mode.
    app.on_key(ctrl('g'));
    assert!(
        matches!(app.mode, Mode::Nav { .. }),
        "Ctrl-g reaches the browser from Normal"
    );
}

#[test]
fn returning_from_the_browser_lands_in_normal_when_vim_is_on() {
    let (mut app, _dir) = vim_app("vim-return");
    app.on_key(ctrl('g')); // into the browser
    app.on_key(key(KeyCode::Enter)); // open the selected chapter
    assert!(
        matches!(app.mode, Mode::Normal),
        "Vim returns to Normal, not Insert"
    );
}

#[test]
fn word_motion_and_count_move_the_cursor() {
    // Active chapter is "first chapter\n".
    let (mut app, _dir) = vim_app("vim-word");
    app.on_key(key(KeyCode::Char('w'))); // to "chapter"
    assert_eq!(app.buffer.cursor.col, 6, "w jumps to the next word");
    app.on_key(key(KeyCode::Char('0'))); // back to line start
    assert_eq!(app.buffer.cursor.col, 0, "0 goes to the line start");
    app.on_key(key(KeyCode::Char('$'))); // to line end
    assert_eq!(
        app.buffer.cursor.col,
        app.buffer.line_len(),
        "$ goes to the line end"
    );
}

#[test]
fn a_count_prefix_repeats_a_motion() {
    let (mut app, _dir) = vim_app("vim-count");
    // "3l" moves right three graphemes.
    type_str(&mut app, "3l");
    assert_eq!(app.buffer.cursor.col, 3, "3l moves three to the right");
    // The count is consumed — a lone "l" now moves one.
    app.on_key(key(KeyCode::Char('l')));
    assert_eq!(app.buffer.cursor.col, 4, "count resets after the motion");
}

#[test]
fn gg_and_capital_g_jump_between_lines() {
    let (mut app, _dir) = vim_app("vim-gg");
    // Give the chapter a few lines via Insert, then return to Normal.
    app.on_key(key(KeyCode::Char('A')));
    type_str(&mut app, "\nsecond\nthird");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(
        app.buffer.cursor.line, 2,
        "cursor on the last line after typing"
    );
    type_str(&mut app, "gg");
    assert_eq!(app.buffer.cursor.line, 0, "gg jumps to the first line");
    app.on_key(key(KeyCode::Char('G')));
    assert_eq!(app.buffer.cursor.line, 2, "G jumps to the last line");
    type_str(&mut app, "2G");
    assert_eq!(
        app.buffer.cursor.line, 1,
        "a count before G jumps to that line"
    );
}

#[test]
fn f_finds_a_char_and_semicolon_repeats() {
    let (mut app, _dir) = vim_app("vim-find");
    // "first chapter": 't' at col 4 (first) and col 10 (chapter).
    type_str(&mut app, "ft");
    assert_eq!(app.buffer.cursor.col, 4, "ft jumps to the first 't'");
    app.on_key(key(KeyCode::Char(';')));
    assert_eq!(app.buffer.cursor.col, 10, "; repeats the find");
    app.on_key(key(KeyCode::Char(',')));
    assert_eq!(app.buffer.cursor.col, 4, ", repeats the find backwards");
}

#[test]
fn dw_deletes_a_word_and_stays_in_normal() {
    let (mut app, _dir) = vim_app("vim-dw");
    type_str(&mut app, "dw"); // delete "first " (word + trailing space)
    assert_eq!(app.buffer.text(), "chapter\n", "{}", app.buffer.text());
    assert!(matches!(app.mode, Mode::Normal), "d stays in Normal");
}

#[test]
fn cw_changes_to_word_end_and_enters_insert() {
    let (mut app, _dir) = vim_app("vim-cw");
    // cw acts like ce: changes "first" (not the trailing space), then Insert.
    type_str(&mut app, "cw");
    assert!(matches!(app.mode, Mode::Insert), "c enters Insert");
    type_str(&mut app, "last");
    assert_eq!(app.buffer.text(), "last chapter\n", "{}", app.buffer.text());
}

#[test]
fn dd_deletes_the_line_with_a_count() {
    let (mut app, _dir) = vim_app("vim-dd");
    // Build three lines, return to Normal at the top.
    app.on_key(key(KeyCode::Char('A')));
    type_str(&mut app, "\nsecond\nthird");
    app.on_key(key(KeyCode::Esc));
    type_str(&mut app, "gg"); // to line 0
    type_str(&mut app, "2dd"); // delete the first two lines
    assert_eq!(app.buffer.text(), "third\n", "{}", app.buffer.text());
}

#[test]
fn x_deletes_the_char_under_the_cursor() {
    let (mut app, _dir) = vim_app("vim-x");
    type_str(&mut app, "x");
    assert_eq!(app.buffer.text(), "irst chapter\n", "{}", app.buffer.text());
    type_str(&mut app, "2x");
    assert_eq!(
        app.buffer.text(),
        "st chapter\n",
        "2x deletes two: {}",
        app.buffer.text()
    );
}

#[test]
fn ci_quote_changes_inside_quotes() {
    let (mut app, _dir) = vim_app("vim-ciq");
    // Replace the chapter with a quoted line.
    type_str(&mut app, "cc");
    type_str(&mut app, "say \"hi\" now");
    app.on_key(key(KeyCode::Esc));
    type_str(&mut app, "0"); // line start
    type_str(&mut app, "ci\""); // change inside the quotes
    assert!(matches!(app.mode, Mode::Insert));
    type_str(&mut app, "bye");
    assert_eq!(
        app.buffer.text(),
        "say \"bye\" now\n",
        "{}",
        app.buffer.text()
    );
}

// --- Paragraph and sentence motions / objects (#821) -------------------

#[test]
fn brace_motions_jump_between_paragraphs() {
    let (mut app, _dir) = vim_app("vim-para-jump");
    app.buffer = Buffer::from_text("one\ntwo\n\nthree\n\nfour\n");
    // lines:                        0    1    2    3      4    5
    type_str(&mut app, "gg"); // line 0
    type_str(&mut app, "}"); // to the first blank line
    assert_eq!(app.buffer.cursor.line, 2, "}} jumps to the blank boundary");
    type_str(&mut app, "}"); // to the next blank line
    assert_eq!(app.buffer.cursor.line, 4);
    type_str(&mut app, "{"); // back to the previous blank line
    assert_eq!(app.buffer.cursor.line, 2, "{{ jumps back");
}

#[test]
fn dap_deletes_a_paragraph_with_its_trailing_blank() {
    let (mut app, _dir) = vim_app("vim-dap");
    app.buffer = Buffer::from_text("one\ntwo\n\nkeep this\n");
    type_str(&mut app, "gg"); // in the first paragraph
    type_str(&mut app, "dap"); // delete it and the blank line after
    assert_eq!(app.buffer.text(), "keep this\n", "{}", app.buffer.text());
    assert!(matches!(app.mode, Mode::Normal));
}

#[test]
fn dip_deletes_the_paragraph_but_keeps_the_blank_line() {
    let (mut app, _dir) = vim_app("vim-dip");
    app.buffer = Buffer::from_text("one\ntwo\n\nkeep this\n");
    type_str(&mut app, "gg");
    type_str(&mut app, "dip"); // just the paragraph, leaving the blank line
    assert_eq!(app.buffer.text(), "\nkeep this\n", "{}", app.buffer.text());
}

#[test]
fn yip_yanks_the_paragraph_and_p_puts_it() {
    let (mut app, _dir) = vim_app("vim-yip");
    app.buffer = Buffer::from_text("alpha\nbeta\n\ntail\n");
    type_str(&mut app, "gg");
    type_str(&mut app, "yip"); // yank "alpha\nbeta\n"
                               // Put it below the cursor; the yank was charwise (line-aligned bytes).
    type_str(&mut app, "P"); // put before the cursor
    assert_eq!(
        app.buffer.text(),
        "alpha\nbeta\nalpha\nbeta\n\ntail\n",
        "{}",
        app.buffer.text()
    );
}

#[test]
fn cis_changes_a_sentence_in_the_middle() {
    let (mut app, _dir) = vim_app("vim-cis");
    app.buffer = Buffer::from_text("One fish. Two fish. Red fish.\n");
    type_str(&mut app, "0"); // line start
    type_str(&mut app, "fT"); // onto the 'T' of "Two", inside the middle sentence
    type_str(&mut app, "cis"); // change the middle sentence
    assert!(matches!(app.mode, Mode::Insert));
    type_str(&mut app, "Blue fish.");
    assert_eq!(
        app.buffer.text(),
        "One fish. Blue fish. Red fish.\n",
        "{}",
        app.buffer.text()
    );
}

#[test]
fn das_deletes_a_sentence_with_its_trailing_space() {
    let (mut app, _dir) = vim_app("vim-das");
    app.buffer = Buffer::from_text("One fish. Two fish. Red fish.\n");
    type_str(&mut app, "0");
    type_str(&mut app, "das"); // delete the first sentence and its trailing space
    assert_eq!(
        app.buffer.text(),
        "Two fish. Red fish.\n",
        "{}",
        app.buffer.text()
    );
}

#[test]
fn capital_d_deletes_to_end_of_line_and_u_undoes() {
    let (mut app, _dir) = vim_app("vim-D");
    type_str(&mut app, "fc"); // jump to the 'c' of "chapter"
    type_str(&mut app, "D"); // delete to end of line
    assert_eq!(app.buffer.text(), "first \n", "{}", app.buffer.text());
    app.on_idle(); // seal the edit into an undo group
    type_str(&mut app, "u"); // undo
    assert_eq!(app.buffer.text(), "first chapter\n", "u restores the line");
}

#[test]
fn an_operator_composes_with_the_arrow_keys() {
    let (mut app, _dir) = vim_app("vim-d-arrow");
    app.on_key(key(KeyCode::Char('A')));
    type_str(&mut app, "\nsecond\nthird");
    app.on_key(key(KeyCode::Esc));
    type_str(&mut app, "gg"); // to line 0
                              // d<Down> deletes this line and the next, like dj.
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.buffer.text(), "third\n", "{}", app.buffer.text());
}

#[test]
fn yy_then_p_duplicates_a_line() {
    let (mut app, _dir) = vim_app("vim-yy-p");
    type_str(&mut app, "yy"); // yank "first chapter"
    type_str(&mut app, "p"); // put it on a new line below
    assert_eq!(
        app.buffer.text(),
        "first chapter\nfirst chapter\n",
        "{}",
        app.buffer.text()
    );
    assert!(matches!(app.mode, Mode::Normal), "put stays in Normal");
}

#[test]
fn dd_then_p_moves_a_line() {
    let (mut app, _dir) = vim_app("vim-dd-p");
    app.on_key(key(KeyCode::Char('A')));
    type_str(&mut app, "\nsecond");
    app.on_key(key(KeyCode::Esc));
    type_str(&mut app, "gg"); // line 0 ("first chapter")
    type_str(&mut app, "dd"); // delete it (now in unnamed register)
    type_str(&mut app, "p"); // put below the current line ("second")
    assert_eq!(
        app.buffer.text(),
        "second\nfirst chapter\n",
        "{}",
        app.buffer.text()
    );
}

#[test]
fn yw_then_capital_p_puts_inline_before_the_cursor() {
    let (mut app, _dir) = vim_app("vim-yw-P");
    type_str(&mut app, "yw"); // yank "first " (charwise)
    type_str(&mut app, "P"); // put before the cursor, inline
    assert_eq!(
        app.buffer.text(),
        "first first chapter\n",
        "{}",
        app.buffer.text()
    );
}

#[test]
fn named_registers_hold_separate_text() {
    let (mut app, _dir) = vim_app("vim-named-reg");
    // Two lines; yank each into its own register.
    app.on_key(key(KeyCode::Char('A')));
    type_str(&mut app, "\nsecond");
    app.on_key(key(KeyCode::Esc));
    type_str(&mut app, "gg");
    type_str(&mut app, "\"ayy"); // line 0 → register a
    type_str(&mut app, "j");
    type_str(&mut app, "\"byy"); // line 1 → register b
                                 // Put register a below the current (last) line.
    type_str(&mut app, "\"ap");
    assert_eq!(
        app.buffer.text(),
        "first chapter\nsecond\nfirst chapter\n",
        "register a held line 0: {}",
        app.buffer.text()
    );
}

#[test]
fn count_repeats_a_put() {
    let (mut app, _dir) = vim_app("vim-3p");
    type_str(&mut app, "yy");
    type_str(&mut app, "3p");
    assert_eq!(
        app.buffer.text(),
        "first chapter\nfirst chapter\nfirst chapter\nfirst chapter\n",
        "{}",
        app.buffer.text()
    );
}

#[test]
fn visual_char_selection_deletes_the_range() {
    let (mut app, _dir) = vim_app("vim-v-d");
    // v selects from col 0; ll extends to col 2; d deletes "fir" (inclusive).
    type_str(&mut app, "vlld");
    assert_eq!(app.buffer.text(), "st chapter\n", "{}", app.buffer.text());
    assert!(
        matches!(app.mode, Mode::Normal),
        "delete leaves Visual for Normal"
    );
}

#[test]
fn visual_line_selection_deletes_whole_lines() {
    let (mut app, _dir) = vim_app("vim-vline-d");
    app.on_key(key(KeyCode::Char('A')));
    type_str(&mut app, "\nsecond");
    app.on_key(key(KeyCode::Esc));
    type_str(&mut app, "gg"); // line 0
    type_str(&mut app, "Vd"); // linewise select + delete the line
    assert_eq!(app.buffer.text(), "second\n", "{}", app.buffer.text());
}

#[test]
fn visual_change_enters_insert() {
    let (mut app, _dir) = vim_app("vim-v-c");
    type_str(&mut app, "vllc");
    assert!(matches!(app.mode, Mode::Insert), "change enters Insert");
    type_str(&mut app, "X");
    assert_eq!(app.buffer.text(), "Xst chapter\n", "{}", app.buffer.text());
}

#[test]
fn visual_yank_then_put() {
    let (mut app, _dir) = vim_app("vim-v-y");
    type_str(&mut app, "vlly"); // yank "fir"; cursor returns to the start
    type_str(&mut app, "P"); // put it before the cursor, inline
    assert_eq!(
        app.buffer.text(),
        "firfirst chapter\n",
        "{}",
        app.buffer.text()
    );
}

#[test]
fn esc_cancels_visual_and_o_swaps_ends() {
    let (mut app, _dir) = vim_app("vim-v-esc");
    app.on_key(key(KeyCode::Char('v')));
    assert!(matches!(app.mode, Mode::Visual { .. }), "v enters Visual");
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Normal), "Esc cancels Visual");
    // `o` swaps the ends without changing which text is selected.
    type_str(&mut app, "vllod");
    assert_eq!(
        app.buffer.text(),
        "st chapter\n",
        "swap leaves the same selection: {}",
        app.buffer.text()
    );
}

#[test]
fn slash_search_jumps_and_n_repeats() {
    let (mut app, _dir) = vim_app("vim-search");
    // "first chapter": search for "r" → jumps to the first 'r' (col 3).
    type_str(&mut app, "/r");
    app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(app.mode, Mode::Normal),
        "Enter finalizes back to Normal"
    );
    assert_eq!(app.buffer.cursor.col, 2, "jumped to the first match");
    app.on_key(key(KeyCode::Char('n'))); // next 'r' (in "chapter", col 12)
    assert_eq!(app.buffer.cursor.col, 12, "n repeats forward");
    app.on_key(key(KeyCode::Char('N'))); // back to the first
    assert_eq!(app.buffer.cursor.col, 2, "N reverses");
}

#[test]
fn search_wraps_with_a_notice() {
    let (mut app, _dir) = vim_app("vim-search-wrap");
    type_str(&mut app, "/r");
    app.on_key(key(KeyCode::Enter)); // col 3
    app.on_key(key(KeyCode::Char('n'))); // col 12 (last 'r')
    app.on_key(key(KeyCode::Char('n'))); // wraps to col 2
    assert_eq!(app.buffer.cursor.col, 2, "wrapped to the first match");
    assert!(
        app.message.as_deref().unwrap_or("").contains("BOTTOM"),
        "{:?}",
        app.message
    );
}

#[test]
fn question_searches_backward_and_esc_restores_the_cursor() {
    let (mut app, _dir) = vim_app("vim-search-back");
    type_str(&mut app, "$"); // to end of line
    let start = app.buffer.cursor.col;
    // `?t` searches backward → nearest 't' before the cursor (col 10 in "chapter").
    type_str(&mut app, "?t");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.buffer.cursor.col, 10, "? jumped backward");
    // A fresh search, abandoned with Esc, restores the origin.
    type_str(&mut app, "/f");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(
        app.buffer.cursor.col, 10,
        "Esc restores the pre-search cursor"
    );
    let _ = start;
}

#[test]
fn star_searches_the_word_under_the_cursor() {
    let (mut app, _dir) = vim_app("vim-star");
    // Make the word repeat so * has somewhere to jump.
    type_str(&mut app, "A"); // append at end → Insert
    type_str(&mut app, " first");
    app.on_key(key(KeyCode::Esc));
    type_str(&mut app, "gg0"); // back to the start ("first chapter first")
    type_str(&mut app, "*"); // search "first" → next occurrence
    assert_eq!(
        app.buffer.cursor.col,
        14,
        "* jumped to the next 'first': {}",
        app.buffer.text()
    );
}

#[test]
fn entering_continuous_view_leaves_visual() {
    let (mut app, _dir) = vim_app("vim-v-continuous");
    app.on_key(key(KeyCode::Char('v')));
    assert!(matches!(app.mode, Mode::Visual { .. }));
    app.on_key(ctrl('l')); // toggle the continuous view
    assert!(app.manuscript.is_some(), "entered the continuous view");
    assert!(
        matches!(app.mode, Mode::Normal),
        "Visual's stale anchor is dropped"
    );
}

#[test]
fn a_remapped_key_runs_the_action_and_defaults_still_work() {
    let (mut app, _dir) = vim_app("vim-remap");
    app.vim_keys.insert('s', 'l'); // make 's' do what 'l' (right) does
    app.on_key(key(KeyCode::Char('s')));
    assert_eq!(app.buffer.cursor.col, 1, "remapped 's' moved right");
    app.on_key(key(KeyCode::Char('l'))); // the default key is unchanged
    assert_eq!(app.buffer.cursor.col, 2, "unmapped 'l' keeps its default");
}

#[test]
fn a_remapped_operator_composes_with_motions() {
    let (mut app, _dir) = vim_app("vim-remap-op");
    app.vim_keys.insert('x', 'd'); // 'x' now starts the delete operator
    type_str(&mut app, "xw"); // x→d, then w: delete the word
    assert_eq!(app.buffer.text(), "chapter\n", "{}", app.buffer.text());
}

#[test]
fn keymap_builds_valid_entries_and_reports_bad_ones() {
    use std::collections::HashMap;
    let cfg = HashMap::from([
        ("down".to_string(), "n".to_string()),       // valid: 'n' → 'j'
        ("frobnicate".to_string(), "z".to_string()), // unknown action
        ("left".to_string(), "hh".to_string()),      // not a single key
    ]);
    let (map, errors) = super::super::build_keymap(&cfg);
    assert_eq!(map.get(&'n'), Some(&'j'), "down maps the 'n' key to 'j'");
    assert_eq!(map.len(), 1, "only the valid entry is kept");
    assert_eq!(
        errors.len(),
        2,
        "the unknown action and multi-char value are reported: {errors:?}"
    );
}

#[test]
fn keymap_reports_a_key_bound_twice() {
    use std::collections::HashMap;
    let cfg = HashMap::from([
        ("down".to_string(), "j".to_string()),
        ("delete".to_string(), "j".to_string()), // both want the 'j' key
    ]);
    let (_, errors) = super::super::build_keymap(&cfg);
    assert_eq!(errors.len(), 1, "the collision is reported: {errors:?}");
    assert!(errors[0].contains("more than one"), "{errors:?}");
}

#[test]
fn a_remapped_motion_extends_a_visual_selection() {
    let (mut app, _dir) = vim_app("vim-remap-visual");
    app.vim_keys.insert('s', 'l'); // 's' moves right
    type_str(&mut app, "vssd"); // visual, extend right twice via 's', delete
    assert_eq!(
        app.buffer.text(),
        "st chapter\n",
        "remapped motion drove the selection: {}",
        app.buffer.text()
    );
}

#[test]
fn modeless_default_is_unchanged_when_vim_is_off() {
    let (mut app, _dir) = two_chapter_app("vim-off");
    app.vim = false;
    app.mode = Mode::Insert;
    // Letters type straight into the buffer, and Esc is inert.
    type_str(&mut app, "Z");
    assert!(
        app.buffer.text().starts_with("Zfirst chapter"),
        "{}",
        app.buffer.text()
    );
    app.on_key(key(KeyCode::Esc));
    assert!(
        matches!(app.mode, Mode::Insert),
        "Esc does nothing without Vim mode"
    );
}
