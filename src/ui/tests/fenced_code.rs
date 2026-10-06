//! Fenced-code preview tests (#723): the code colour, fence concealment/reveal,
//! verbatim whitespace, display-only bytes, cursor row math below a block, the
//! reference-pane form, and the set-apart background. Pure move (#792).

use super::*;
use crate::buffer::Cursor;
use ratatui::style::Color;
use ratatui::{backend::TestBackend, Terminal};
use std::fmt::Write;

#[test]
fn a_fenced_code_block_renders_in_the_code_colour() {
    // A chapter with a fenced block; its content should be set apart by colour.
    let app = app_with("Intro line.\n\n```\nZENITH log\n```\n");
    let code = app.theme.code;
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    terminal.draw(|f| draw(f, &app)).unwrap();
    let buf = terminal.backend().buffer().clone();

    let fg_on_row = |needle: &str| -> Option<Color> {
        let row = (0..12u16).find(|&y| {
            let line: String = (0..40).map(|x| buf[(x, y)].symbol()).collect();
            line.contains(needle)
        })?;
        (0..40)
            .map(|x| &buf[(x, row)])
            .find(|c| c.symbol().chars().next().is_some_and(char::is_alphabetic))
            .map(|c| c.style().fg)?
    };

    // The fenced content is in the code colour; ordinary prose is not.
    assert_eq!(
        fg_on_row("ZENITH log"),
        Some(code),
        "code line in code colour"
    );
    assert_ne!(fg_on_row("Intro line."), Some(code), "prose is not");
}

#[test]
fn fenced_code_fences_hide_outside_the_block_and_reveal_inside() {
    // Cursor left at document end (outside the block): the ``` fence rows hide.
    let mut app = app_with("Intro line.\n\n```\nZENITH log\n```\n\nAfter.\n");
    let outside = render(&app, 40, 14);
    assert!(
        !outside.contains("```"),
        "fence markers are hidden in preview:\n{outside}"
    );
    assert!(
        outside.contains("ZENITH log"),
        "the block's content still shows:\n{outside}"
    );

    // Put the cursor inside the block (the content line): the raw fences appear.
    app.edit_buffer_mut().cursor = Cursor { line: 3, col: 2 };
    let inside = render(&app, 40, 14);
    assert!(
        inside.contains("```"),
        "the raw fences reveal for editing when the cursor is inside:\n{inside}"
    );
    assert!(
        inside.contains("ZENITH log"),
        "content still shows:\n{inside}"
    );
}

#[test]
fn fenced_code_preview_preserves_whitespace_and_does_not_reflow() {
    // A code line longer than the wrap column: in the preview it must stay on one
    // row (verbatim, no soft-wrap), unlike prose which would reflow.
    let long = "X".repeat(60);
    let app = app_with(&format!("Intro.\n\n```\n{long}\n```\n\nAfter.\n"));
    // Narrow column so ordinary prose would certainly wrap onto several rows.
    let screen = render(&app, 30, 16);
    // The long run of X's occupies exactly one rendered row — it is *not* broken
    // across rows the way soft-wrapped prose would be (verbatim, no reflow). The
    // row is clipped to the column, so we count rows that are all X's.
    let x_rows = screen
        .lines()
        .filter(|l| !l.trim().is_empty() && l.trim().chars().all(|c| c == 'X'))
        .count();
    assert_eq!(
        x_rows, 1,
        "the code line stays on one row (no reflow):\n{screen}"
    );
    // Leading whitespace inside the block is preserved verbatim.
    let app2 = app_with("Intro.\n\n```\n    indented code\n```\n\nAfter.\n");
    let screen2 = render(&app2, 40, 16);
    assert!(
        screen2.lines().any(|l| l.contains("    indented code")),
        "the block's leading whitespace is preserved:\n{screen2}"
    );
}

#[test]
fn fenced_code_preview_is_display_only() {
    // Rendering the concealed preview never mutates the buffer bytes.
    let text = "Intro.\n\n```\ncode line\n```\n\nAfter.\n";
    let app = app_with(text);
    let before = app.edit_buffer().text();
    let _ = render(&app, 40, 16); // cursor outside: preview
    let mut app2 = app_with(text);
    app2.edit_buffer_mut().cursor = Cursor { line: 3, col: 1 }; // cursor inside: revealed
    let _ = render(&app2, 40, 16);
    assert_eq!(
        app.edit_buffer().text(),
        before,
        "preview left bytes unchanged"
    );
    assert_eq!(
        app2.edit_buffer().text(),
        before,
        "revealed render left bytes unchanged"
    );
}

#[test]
fn cursor_lands_correctly_below_a_block_with_hidden_fences() {
    // With the two fence rows hidden, the on-screen cursor for a line *below* the
    // block must still land on that line's first character — the removed rows are
    // accounted for in the scroll/row math.
    let mut app = app_with("Intro.\n\n```\ncode\n```\n\nTail line.\n");
    // Cursor at the start of "Tail line." (line index 6).
    app.edit_buffer_mut().cursor = Cursor { line: 6, col: 0 };
    let mut terminal = Terminal::new(TestBackend::new(40, 16)).unwrap();
    terminal.draw(|f| draw(f, &app)).unwrap();
    let pos = terminal.get_cursor_position().unwrap();
    let buf = terminal.backend().buffer().clone();
    assert_eq!(
        buf[(pos.x, pos.y)].symbol(),
        "T",
        "the cursor lands on the first char below the fence-hidden block"
    );
}

#[test]
fn a_reference_pane_shows_the_concealed_preview_form() {
    // A reference pane has no cursor, so it always shows the concealed form — the
    // fences hidden — regardless of where the (other pane's) edit cursor sits.
    let buffer = buffer_with("Intro.\n\n```\nBEACON log\n```\n\nAfter.\n");
    let app = App::new(buffer_with("other chapter\n"), Project::scratch());
    let mut terminal = Terminal::new(TestBackend::new(40, 16)).unwrap();
    terminal
        .draw(|f| {
            let area = f.area();
            super::editor::render_reference_pane(
                f,
                area,
                &app,
                &buffer,
                app.project.active_index(),
            );
        })
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let screen = (0..16).fold(String::new(), |mut screen, y| {
        let l: String = (0..40).map(|x| buf[(x, y)].symbol()).collect();
        writeln!(screen, "{l}").unwrap();
        screen
    });
    assert!(
        !screen.contains("```"),
        "the reference pane hides the fences:\n{screen}"
    );
    assert!(
        screen.contains("BEACON log"),
        "the reference pane shows the block content:\n{screen}"
    );
}

#[test]
fn fenced_code_rows_carry_the_set_apart_background() {
    // Every code row (content and, when revealed, the fences) gets the code_bg
    // background tint, so the block reads as one set-apart box.
    let mut app = app_with("Intro.\n\n```\nMARKER\n```\n\nAfter.\n");
    let code_bg = app.theme.code_bg;
    let bg_on_row = |app: &App, needle: &str| -> Option<Color> {
        let mut terminal = Terminal::new(TestBackend::new(40, 14)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let row = (0..14u16).find(|&y| {
            let line: String = (0..40).map(|x| buf[(x, y)].symbol()).collect();
            line.contains(needle)
        })?;
        (0..40)
            .filter_map(|x| buf[(x, row)].style().bg)
            .find(|&bg| bg == code_bg)
    };
    assert_eq!(
        bg_on_row(&app, "MARKER"),
        Some(code_bg),
        "code content sits on the set-apart background"
    );
    // Prose does not.
    assert_ne!(
        bg_on_row(&app, "Intro."),
        Some(code_bg),
        "prose is not tinted"
    );
    // When revealed, the fence rows carry the tint too.
    app.edit_buffer_mut().cursor = Cursor { line: 3, col: 1 };
    assert_eq!(
        bg_on_row(&app, "```"),
        Some(code_bg),
        "the revealed fence row is part of the box"
    );
}
