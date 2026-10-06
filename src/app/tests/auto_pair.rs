//! Auto-pair quotes and brackets (#1007): with the opt-in setting on, typing an
//! opener inserts its matching closer and drops the cursor between the pair, and
//! typing a closer that's already at the cursor skips past it instead of
//! duplicating. Off by default, behavior is unchanged. Composes with smart
//! typography (#296) so a `"` becomes a correctly-oriented curly-quote pair.

use super::*;

/// A two-chapter app with an empty active buffer and auto-pair forced on (smart
/// typography left off, so these test the plain ASCII pairing). The shared
/// `two_chapter_app` pins both off.
fn pair_app(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.flags.auto_pair = true;
    app.buffer = Buffer::from_text("");
    (app, dir)
}

#[test]
fn typing_an_opener_inserts_its_closer_and_lands_between() {
    let (mut app, _dir) = pair_app("pair-open-quote");
    type_str(&mut app, "\"");
    assert_eq!(app.buffer.text(), "\"\"", "the closer was auto-inserted");
    // The cursor sits between the pair (col 1), so the next char lands inside.
    assert_eq!(app.buffer.cursor.col, 1);
    type_str(&mut app, "hi");
    assert_eq!(app.buffer.text(), "\"hi\"", "typing lands inside the pair");
}

#[test]
fn brackets_and_parens_pair_too() {
    let (mut app, _dir) = pair_app("pair-brackets");
    type_str(&mut app, "(");
    assert_eq!(app.buffer.text(), "()");
    assert_eq!(app.buffer.cursor.col, 1);
    type_str(&mut app, "[");
    assert_eq!(
        app.buffer.text(),
        "([])",
        "nested pair, cursor between each"
    );
    assert_eq!(app.buffer.cursor.col, 2);
}

#[test]
fn overtyping_a_closer_skips_past_it_instead_of_duplicating() {
    let (mut app, _dir) = pair_app("pair-skip");
    type_str(&mut app, "(");
    assert_eq!(app.buffer.text(), "()");
    // Type the matching `)` while it's already at the cursor: skip over it, don't
    // insert a duplicate.
    type_str(&mut app, ")");
    assert_eq!(app.buffer.text(), "()", "no duplicate closer");
    assert_eq!(app.buffer.cursor.col, 2, "cursor moved past the closer");
    // Now past the pair, a fresh closer with nothing to skip inserts literally.
    type_str(&mut app, ")");
    assert_eq!(app.buffer.text(), "())");
}

#[test]
fn overtyping_a_quote_closer_skips_past_it() {
    let (mut app, _dir) = pair_app("pair-skip-quote");
    type_str(&mut app, "\"hi");
    assert_eq!(app.buffer.text(), "\"hi\"");
    assert_eq!(app.buffer.cursor.col, 3, "cursor before the auto-closer");
    type_str(&mut app, "\"");
    assert_eq!(
        app.buffer.text(),
        "\"hi\"",
        "over-typing the closing quote skips, no duplicate"
    );
    assert_eq!(app.buffer.cursor.col, 4);
}

#[test]
fn a_closer_with_nothing_to_skip_inserts_literally() {
    // A `)` typed where the cursor isn't sitting on a `)` inserts as itself — the
    // skip only fires when the exact closer is already at the cursor.
    let (mut app, _dir) = pair_app("pair-lonely-closer");
    type_str(&mut app, "a)");
    assert_eq!(app.buffer.text(), "a)");
}

#[test]
fn off_by_default_leaves_typing_unchanged() {
    // `two_chapter_app` pins auto_pair off; typing an opener inserts just it.
    let (mut app, _dir) = two_chapter_app("pair-off");
    app.buffer = Buffer::from_text("");
    type_str(&mut app, "(\"[");
    assert_eq!(
        app.buffer.text(),
        "(\"[",
        "no auto-closers when the setting is off"
    );
    assert_eq!(app.buffer.cursor.col, 3);
}

// --- Composition with smart typography (#296) ------------------------------

/// Auto-pair *and* smart typography both on, empty buffer — the interaction case.
fn smart_pair_app(name: &str) -> (App, TempProject) {
    let (mut app, dir) = pair_app(name);
    app.flags.smart_typography = true;
    (app, dir)
}

#[test]
fn smart_typography_and_auto_pair_make_an_oriented_curly_pair() {
    let (mut app, _dir) = smart_pair_app("pair-smart-curly");
    type_str(&mut app, "\"");
    // A correctly-oriented open/close curly pair, cursor between — no double
    // closer and no mis-orientation.
    assert_eq!(
        app.buffer.text(),
        "\u{201C}\u{201D}",
        "open “ + close ” curly pair"
    );
    assert_eq!(app.buffer.cursor.col, 1, "cursor sits inside the pair");
    type_str(&mut app, "Wait");
    assert_eq!(app.buffer.text(), "\u{201C}Wait\u{201D}");
}

#[test]
fn overtyping_the_curly_closer_skips_past_it() {
    let (mut app, _dir) = smart_pair_app("pair-smart-skip");
    type_str(&mut app, "\"Wait");
    assert_eq!(app.buffer.text(), "\u{201C}Wait\u{201D}");
    assert_eq!(app.buffer.cursor.col, 5, "cursor before the closing ”");
    // Typing `"` to close, after a word, would smart-orient to a closing ” — which
    // is already at the cursor, so it skips instead of inserting a duplicate.
    type_str(&mut app, "\"");
    assert_eq!(
        app.buffer.text(),
        "\u{201C}Wait\u{201D}",
        "no doubled closing curly quote"
    );
    assert_eq!(app.buffer.cursor.col, 6, "cursor moved past the closer");
}

#[test]
fn a_full_line_of_dialogue_composes_correctly() {
    // The realistic drafting case: opener pairs, over-typed closer skips, and the
    // apostrophe (a closing single quote) is left verbatim (not an opener).
    let (mut app, _dir) = smart_pair_app("pair-smart-dialogue");
    type_str(&mut app, "\"Don't.\"");
    assert_eq!(
        app.buffer.text(),
        "\u{201C}Don\u{2019}t.\u{201D}",
        "one clean oriented quote span, contraction curled, no stray closer"
    );
}
