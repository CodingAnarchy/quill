//! The annotation margin: inline notes drawn beside the prose column, level with
//! their anchored text, stacking rather than overprinting, and yielding when the
//! terminal has no room or the margin is switched off.

use super::*;
use crate::annotation::Live;
use crate::mode::{Action, SettingBool};

const PROSE: &str = "The night was dark and long.\n\nShe reached the gate.\n";

/// An app over `PROSE` with a note on each `(start, end, note)`, margin on.
fn annotated(notes: &[(usize, usize, &str)]) -> App {
    let mut app = app_with(PROSE);
    app.flags.annotation_margin = true;
    app.annotations = notes
        .iter()
        .map(|&(start, end, note)| Live {
            chapter: String::new(),
            start,
            end,
            note: note.into(),
            replies: Vec::new(),
            resolved: false,
            reviewer: None,
        })
        .collect();
    app
}

/// The screen row holding `needle`, and the column it starts at.
fn find(screen: &str, needle: &str) -> Option<(usize, usize)> {
    screen
        .lines()
        .enumerate()
        .find_map(|(y, l)| l.find(needle).map(|x| (y, l[..x].chars().count())))
}

#[test]
fn a_note_sits_beside_its_anchored_line() {
    // "reached" is on the third prose line (byte 34).
    let app = annotated(&[(34, 41, "timing vs ch 3")]);
    let screen = render(&app, 120, 12);
    let (note_y, note_x) = find(&screen, "timing vs ch 3").expect(&screen);
    let (prose_y, prose_x) = find(&screen, "She reached").expect(&screen);
    assert_eq!(note_y, prose_y, "level with its anchor:\n{screen}");
    assert!(
        note_x > prose_x + 72,
        "right of the prose column:\n{screen}"
    );
}

#[test]
fn notes_on_the_same_line_stack_instead_of_overprinting() {
    let app = annotated(&[(4, 9, "first note"), (14, 18, "second note")]);
    let screen = render(&app, 120, 12);
    let (a, _) = find(&screen, "first note").expect(&screen);
    let (b, _) = find(&screen, "second note").expect(&screen);
    assert!(
        b > a + 1,
        "the second slides below the first, past a gap:\n{screen}"
    );
}

#[test]
fn a_long_note_is_clipped_with_an_ellipsis() {
    let long = "word ".repeat(60);
    let app = annotated(&[(4, 9, long.trim())]);
    let screen = render(&app, 120, 16);
    let rows = screen.lines().filter(|l| l.contains("word word")).count();
    assert_eq!(rows, 4, "capped at four rows:\n{screen}");
    assert!(screen.contains('…'), "the cut is marked:\n{screen}");
}

#[test]
fn the_column_shifts_left_to_make_room_when_the_gutter_is_narrow() {
    let plain = render(&app_with(PROSE), 100, 12);
    let app = annotated(&[(4, 9, "check")]);
    let screen = render(&app, 100, 12);
    let (_, centered) = find(&plain, "The night").unwrap();
    let (_, shifted) = find(&screen, "The night").unwrap();
    assert!(shifted < centered, "prose moved left:\n{screen}");
    assert!(
        screen.contains("check"),
        "the note fits beside it:\n{screen}"
    );
}

#[test]
fn no_margin_when_there_is_no_room_or_it_is_off() {
    let mut app = annotated(&[(4, 9, "check")]);
    let narrow = render(&app, 80, 12);
    assert!(!narrow.contains("check"), "no room at 80 cols:\n{narrow}");
    assert_eq!(
        find(&narrow, "The night"),
        find(&render(&app_with(PROSE), 80, 12), "The night"),
        "and the prose stays centered"
    );

    app.run_action(Action::Toggle(SettingBool::AnnotationMargin), false);
    assert!(!app.flags.annotation_margin);
    let off = render(&app, 120, 12);
    assert!(!off.contains("check"), "toggled off:\n{off}");
}
