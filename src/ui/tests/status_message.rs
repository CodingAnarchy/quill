//! A transient message too long for the status line gets rows of its own above
//! it, word-wrapped at full brightness, instead of being squeezed in after the
//! tags with one space between and cut off at the screen's edge.

use super::*;

const AUTH: &str = "Authentication failed — the remote rejected your credentials. Set up an \
                    SSH key, a credential helper, or a QUILL_GIT_TOKEN, then try again.";

fn rows(app: &App, w: u16, h: u16) -> Vec<String> {
    render(app, w, h).lines().map(str::to_string).collect()
}

#[test]
fn a_long_message_reads_on_its_own_rows_above_the_bar() {
    let mut app = app_with("one two three\n");
    app.message = Some(AUTH.to_string());
    let screen = rows(&app, 90, 10);
    let bar = screen.last().expect("a status line");
    assert!(!bar.contains("Authentication"), "not on the bar: {bar:?}");
    assert!(bar.contains("3 words"), "the bar keeps its count: {bar:?}");

    // The rows just above the bar hold the whole message, word-wrapped.
    let above: Vec<&str> = screen[screen.len() - 3..screen.len() - 1]
        .iter()
        .map(|r| r.trim())
        .collect();
    assert!(above[0].starts_with("Authentication failed"), "{screen:#?}");
    assert_eq!(above.join(" "), AUTH, "{screen:#?}");
}

#[test]
fn a_short_message_stays_beside_the_tags() {
    let mut app = app_with("one two three\n");
    app.message = Some("Saved.".to_string());
    let screen = rows(&app, 90, 10);
    let bar = screen.last().expect("a status line");
    assert!(bar.trim_end().ends_with("Saved."), "{bar:?}");
    assert!(
        !screen[screen.len() - 2].contains("Saved."),
        "no extra row: {screen:#?}"
    );
}

#[test]
fn a_message_that_only_just_fits_moves_off_the_bar() {
    // One space between the last tag and the message read as one more tag.
    let mut app = app_with("one two three\n");
    let bar = rows(&app, 60, 8).pop().expect("bar");
    let tags = bar.find("   ").expect("a gap before the count");
    let tags_w = bar[..tags].chars().count();
    // Wide enough to sit on the bar with a single space, as it used to.
    let message = "x".repeat(60 - tags_w - 3);
    app.message = Some(message.clone());
    let screen = rows(&app, 60, 8);
    assert!(
        !screen.last().expect("bar").contains(&message),
        "{screen:#?}"
    );
    assert!(screen[screen.len() - 2].contains(&message), "{screen:#?}");
}

#[test]
fn a_very_long_message_stops_at_three_rows() {
    let mut app = app_with("one two three\n");
    app.message = Some(AUTH.repeat(4));
    let screen = rows(&app, 60, 12);
    let message_rows = screen[..screen.len() - 1]
        .iter()
        .filter(|r| r.contains("Authentication") || r.contains("credential"))
        .count();
    assert!(message_rows <= 3, "{screen:#?}");
    assert!(
        screen[screen.len() - 2].trim_end().ends_with('…'),
        "{screen:#?}"
    );
}

#[test]
fn the_cursor_line_stays_on_screen_above_the_message() {
    // The editor gives up the message's rows, so the line being written isn't
    // hidden under it.
    let text = (1..=30)
        .map(|n| format!("Line number {n}."))
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut app = app_with(&text);
    app.buffer.cursor.line = app.buffer.line_count() - 1;
    app.message = Some(AUTH.to_string());
    let screen = render(&app, 90, 12);
    assert!(screen.contains("Line number 30."), "{screen}");
}
