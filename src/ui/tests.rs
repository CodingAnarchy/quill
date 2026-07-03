//! Tests for the `src/ui/mod.rs` module, split out to keep the module under the
//! ~1,000-line convention (#404). Pure move — no behavior change.

use super::render_to_string as render;
use super::*;
use crate::buffer::{Buffer, Cursor};
use crate::manuscript::Manuscript;
use crate::project::Project;
use crate::syntax::Heading;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use ratatui::{backend::TestBackend, Terminal};
use std::fmt::Write;
use std::fs;
use std::time::Instant;

fn buffer_with(text: &str) -> Buffer {
    let mut b = Buffer::empty();
    for c in text.chars() {
        b.insert_char(c);
    }
    b.refresh_syntax();
    b
}

/// An App over an in-memory scratch project, for render tests.
fn app_with(text: &str) -> App {
    App::new(buffer_with(text), Project::scratch())
}

/// Performance guard (run with `cargo test --release -- --ignored
/// --nocapture`). A full frame must render well within one 16ms frame, even
/// for a large chapter shown alongside a many-chapter outline pane — the
/// realistic worst case for per-frame work (the status bar's live word count
/// over the whole buffer, plus a nav row per chapter). If a future change
/// makes rendering re-read files or do per-chapter work proportional to the
/// novel, this catches it.
///
/// Focus mode and typewriter scrolling (#301) are both on here, over a corpus
/// with no blank lines — the pathological case where the focused "paragraph"
/// spans the whole buffer, so `focus_paragraph`'s line walk does its most
/// work — plus a spread of inline annotations (#302) so their per-row/char
/// scan is timed. Confirms none of these adds cost that breaks the frame budget.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn render_stays_within_a_frame() {
    let titles: Vec<String> = (0..150).map(|i| format!("Chapter {i}")).collect();
    let title_refs: Vec<&str> = titles.iter().map(String::as_str).collect();
    let project = Project::scratch_with(&title_refs);

    let mut text = String::new();
    for _ in 0..7500 {
        text.push_str("The night was dark and the road was very long indeed.\n");
    }
    let words = text.split_whitespace().count();
    let mut buffer = Buffer::from_text(&text);
    buffer.refresh_syntax();
    let mut app = App::new(buffer, project);
    app.outline_visible = true;
    app.focus_mode = true;
    app.typewriter = true;
    // The first-line paragraph indent (#722) classifies each visible logical line
    // (a bounded highlight lookup per row); on here so the render guard covers it.
    app.paragraph_indent = true;
    // A spread of inline annotations (#302) so the per-row/char annotation
    // scan is exercised under the frame budget too.
    app.annotations = (0..50)
        .map(|i| crate::annotation::Live {
            chapter: "7B".into(),
            start: i * 53,
            end: i * 53 + 5,
            note: "note".into(),
        })
        .collect();

    let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
    terminal.draw(|f| draw(f, &app)).unwrap(); // warm
    let t = Instant::now();
    for _ in 0..30 {
        terminal.draw(|f| draw(f, &app)).unwrap();
    }
    let dt = t.elapsed() / 30;

    println!("render {words}w + 150-chapter outline = {dt:?}");
    assert!(dt.as_millis() < 16, "render took {dt:?}");

    // And again with a modal overlay open (#349): the same editor plus the
    // O(screen) backdrop dim and the shadowed box. Confirms the depth pass
    // doesn't push a full frame past budget.
    app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)); // open the run-action palette
    assert!(matches!(app.mode, Mode::Palette(_)));
    terminal.draw(|f| draw(f, &app)).unwrap(); // warm
    let t = Instant::now();
    for _ in 0..30 {
        terminal.draw(|f| draw(f, &app)).unwrap();
    }
    let modal_dt = t.elapsed() / 30;
    println!("render with a modal overlay = {modal_dt:?}");
    assert!(modal_dt.as_millis() < 16, "modal render took {modal_dt:?}");
}

#[test]
fn a_split_collapses_only_once_a_pane_would_be_too_small() {
    let wide = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 30,
    };
    let thin = Rect {
        x: 0,
        y: 0,
        width: 50,
        height: 30,
    };
    let short = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 8,
    };
    assert!(!too_cramped_to_split(SplitDir::SideBySide, wide));
    assert!(
        too_cramped_to_split(SplitDir::SideBySide, thin),
        "two columns need room"
    );
    assert!(!too_cramped_to_split(SplitDir::Stacked, wide));
    assert!(
        too_cramped_to_split(SplitDir::Stacked, short),
        "two rows need room"
    );
}

#[test]
fn long_paragraph_soft_wraps_within_the_focus_column() {
    // One long logical line (a single paragraph, no internal newlines).
    let long = "word ".repeat(60);
    let app = app_with(&format!("{long}\n"));
    let screen = render(&app, 100, 20);

    // It appears on several rows (wrapped), not truncated to one... ("word word"
    // matches body rows, not the status bar's "60 words" count).
    let content_rows: Vec<&str> = screen.lines().filter(|l| l.contains("word word")).collect();
    assert!(content_rows.len() > 1, "the long line wrapped:\n{screen}");
    // ...and no rendered row is wider than the centered focus column (+ pad).
    for row in content_rows {
        assert!(
            row.trim_end().chars().count() <= 90,
            "row stays within the column: {row:?}"
        );
    }
}

#[test]
fn vim_mode_is_shown_in_the_status_bar() {
    let mut app = app_with("first chapter\n");
    // Off: no mode chip.
    assert!(
        !render(&app, 80, 8).contains("NORMAL"),
        "no chip when Vim is off"
    );
    // On, resting in Normal.
    app.vim = true;
    app.mode = Mode::Normal;
    assert!(render(&app, 80, 8).contains("NORMAL"), "Normal shown");
    // Entering Insert updates the indicator.
    app.mode = Mode::Insert;
    let screen = render(&app, 80, 8);
    assert!(
        screen.contains("INSERT") && !screen.contains("NORMAL"),
        "Insert shown:\n{screen}"
    );
    // Visual modes show their own labels.
    app.mode = Mode::Visual {
        anchor: 0,
        linewise: false,
    };
    assert!(render(&app, 80, 8).contains("VISUAL"), "Visual shown");
    app.mode = Mode::Visual {
        anchor: 0,
        linewise: true,
    };
    assert!(
        render(&app, 80, 8).contains("VISUAL LINE"),
        "Visual Line shown"
    );
}

#[test]
fn visual_selection_draws_a_highlight() {
    let mut app = app_with("hello world\n");
    app.vim = true;
    // Select the first few graphemes (anchor at 0, cursor at col 2).
    app.mode = Mode::Visual {
        anchor: 0,
        linewise: false,
    };
    app.buffer.cursor = Cursor { line: 0, col: 2 };
    let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
    terminal.draw(|f| draw(f, &app)).unwrap();
    let buf = terminal.backend().buffer();
    let highlighted =
        (0..40).any(|x| (0..6).any(|y| buf[(x, y)].style().bg == Some(Color::Yellow)));
    assert!(highlighted, "the selection carries a highlight background");
}

#[test]
fn markers_are_hidden_when_cursor_is_outside_the_span() {
    // After typing, the cursor sits past the span, so markers conceal.
    let app = app_with("*italic* word");
    let screen = render(&app, 80, 8);
    assert!(screen.contains("italic word"), "screen:\n{screen}");
    assert!(!screen.contains('*'), "markers should be hidden:\n{screen}");
}

#[test]
fn typewriter_pins_the_cursor_line_to_the_centre() {
    // A document taller than the page, cursor resting on the last line.
    let mut text = String::new();
    for i in 0..40 {
        if i > 0 {
            text.push('\n');
        }
        write!(text, "Line{i:02}").unwrap();
    }
    let mut app = app_with(&text); // typed in, so the cursor rests at the end
    let h = 21u16;
    let row_of_last = |app: &App| {
        render(app, 40, h)
            .lines()
            .position(|l| l.contains("Line39"))
            .expect("the last line is on screen")
    };

    // Off: the cursor's line has drifted to the bottom of the page.
    app.typewriter = false;
    let off = row_of_last(&app);
    // On: it's lifted back to the vertical centre, text scrolling under it.
    app.typewriter = true;
    let on = row_of_last(&app);

    assert!(
        on < off,
        "typewriter lifts the last line off the bottom: on={on} off={off}"
    );
    assert!(
        (8..=12).contains(&on),
        "pinned near the centre of a 20-row page: {on}"
    );
}

#[test]
fn focus_mode_dims_the_other_paragraphs() {
    // Two paragraphs; the cursor (typed to the end) rests in the second.
    let mut app = app_with("alpha alpha\n\nbravo bravo");
    let dim = app.theme.dim;

    let first_letter_fg = |app: &App, needle: &str| -> Option<Color> {
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let row = (0..12u16)
            .find(|&y| {
                (0..40).any(|x| buf[(x, y)].symbol() == "a" || buf[(x, y)].symbol() == "b") && {
                    let line: String = (0..40).map(|x| buf[(x, y)].symbol()).collect();
                    line.contains(needle)
                }
            })
            .expect("paragraph on screen");
        (0..40)
            .map(|x| &buf[(x, row)])
            .find(|c| c.symbol().chars().next().is_some_and(char::is_alphabetic))
            .unwrap()
            .style()
            .fg
    };

    // Off (default): both paragraphs render in the normal prose colour.
    assert_ne!(
        first_letter_fg(&app, "alpha"),
        Some(dim),
        "no dimming when focus mode is off"
    );

    // On: the other paragraph dims; the one under the cursor stays normal.
    app.focus_mode = true;
    assert_eq!(
        first_letter_fg(&app, "alpha"),
        Some(dim),
        "the other paragraph dims"
    );
    assert_ne!(
        first_letter_fg(&app, "bravo"),
        Some(dim),
        "the focused paragraph stays normal"
    );
}

/// The screen column of the first non-space character on the first rendered row
/// whose text contains `needle` — how far that row is indented on screen.
fn indent_of_row(app: &App, needle: &str, w: u16, h: u16) -> Option<usize> {
    render(app, w, h)
        .lines()
        .find(|l| l.contains(needle))
        .map(|l| l.len() - l.trim_start().len())
}

#[test]
fn paragraph_indent_indents_only_the_first_line_of_a_prose_paragraph() {
    // A long single-paragraph logical line that soft-wraps to several rows.
    let long = "word ".repeat(40);
    let mut app = app_with(&format!("{long}\n"));
    let w = 60;
    let h = 20;

    // Off (default): the paragraph's rows all start at the same column.
    app.paragraph_indent = false;
    let base = indent_of_row(&app, "word word", w, h).expect("paragraph on screen");

    // On: the first row shifts right by the four-space indent…
    app.paragraph_indent = true;
    let first = indent_of_row(&app, "word word", w, h).expect("first row on screen");
    assert_eq!(first, base + 4, "first line indented by four columns");

    // …while the soft-wrapped continuation rows keep the un-indented column.
    let screen = render(&app, w, h);
    let rows: Vec<&str> = screen.lines().filter(|l| l.contains("word word")).collect();
    assert!(rows.len() > 1, "the paragraph wrapped:\n{screen}");
    for cont in &rows[1..] {
        let indent = cont.len() - cont.trim_start().len();
        assert_eq!(indent, base, "continuation row is not indented: {cont:?}");
    }
}

#[test]
fn paragraph_indent_leaves_the_buffer_and_file_bytes_unchanged() {
    // The indent is display-only: rendering it never mutates the prose.
    let text = "First paragraph.\n\nSecond paragraph.\n";
    let mut app = app_with(text);
    let before = app.edit_buffer().text();
    app.paragraph_indent = true;
    let _ = render(&app, 60, 12);
    assert_eq!(
        app.edit_buffer().text(),
        before,
        "rendering the indent left the buffer byte-for-byte unchanged"
    );
    // And the rendered indent is whitespace only — no stray characters injected.
    assert!(
        render(&app, 60, 12).contains("    First paragraph."),
        "the first paragraph shows the four-space indent"
    );
}

#[test]
fn paragraph_indent_skips_headings_blank_lines_and_the_first_para_after_a_heading() {
    // A heading, its opening paragraph, then a following paragraph.
    let mut app = app_with("# A Heading\n\nOpening line.\n\nFollowing line.\n");
    let w = 60;
    let h = 16;

    let base_heading = indent_of_row(&app, "A Heading", w, h).expect("heading on screen");
    let base_opening = indent_of_row(&app, "Opening line.", w, h).expect("opening on screen");

    app.paragraph_indent = true;
    // The heading itself is never indented.
    assert_eq!(
        indent_of_row(&app, "A Heading", w, h),
        Some(base_heading),
        "heading not indented"
    );
    // The first paragraph right after the heading is not indented (manuscript rule).
    assert_eq!(
        indent_of_row(&app, "Opening line.", w, h),
        Some(base_opening),
        "first paragraph after a heading not indented"
    );
    // A later paragraph is indented.
    let base_following = {
        app.paragraph_indent = false;
        let b = indent_of_row(&app, "Following line.", w, h).expect("following on screen");
        app.paragraph_indent = true;
        b
    };
    assert_eq!(
        indent_of_row(&app, "Following line.", w, h),
        Some(base_following + 4),
        "a later paragraph is indented"
    );
}

#[test]
fn paragraph_indent_skips_fenced_code_blocks() {
    // Prose, then a fenced code block; the code content must not be indented.
    let mut app = app_with("Prose line.\n\n```\ncode line\n```\n");
    let w = 60;
    let h = 16;
    let base_code = indent_of_row(&app, "code line", w, h).expect("code on screen");
    app.paragraph_indent = true;
    assert_eq!(
        indent_of_row(&app, "code line", w, h),
        Some(base_code),
        "fenced code content is not indented"
    );
}

#[test]
fn paragraph_indent_keeps_the_cursor_on_the_indented_line() {
    use ratatui::{backend::TestBackend, Terminal};
    // Cursor at the very start of an indented paragraph's first line.
    let mut app = app_with("Alpha beta gamma.\n");
    app.edit_buffer_mut().cursor = Cursor { line: 0, col: 0 };

    let cursor_x = |app: &App| -> u16 {
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal.get_cursor_position().unwrap().x
    };

    let off = cursor_x(&app);
    app.paragraph_indent = true;
    let on = cursor_x(&app);
    assert_eq!(on, off + 4, "the cursor shifts right with the indent");

    // The character under the cursor is still the paragraph's first letter, not a
    // space — the indent hasn't displaced which byte the cursor addresses.
    let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
    terminal.draw(|f| draw(f, &app)).unwrap();
    let pos = terminal.get_cursor_position().unwrap();
    let buf = terminal.backend().buffer().clone();
    assert_eq!(
        buf[(pos.x, pos.y)].symbol(),
        "A",
        "cursor lands on the first character of the prose"
    );
}

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
            super::editor::render_reference_pane(f, area, &app, &buffer);
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

#[test]
fn a_long_palette_scrolls_to_keep_the_selection_visible() {
    // The Ctrl-K action palette has ~50 entries; on a short terminal it overflows.
    let mut app = app_with("text\n");
    app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    assert!(matches!(app.mode, Mode::Palette(_)), "palette open");
    // Drive the selection well past a short viewport.
    for _ in 0..30 {
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    let screen = render(&app, 100, 16);

    let Mode::Palette(p) = &app.mode else {
        unreachable!()
    };
    // `selected` indexes the visible rows (headers + items); resolve its label.
    let rows = p.rows();
    let selected = match &rows[p.selected] {
        crate::mode::PaletteRow::Header { group, .. } => group.to_string(),
        crate::mode::PaletteRow::Item(i) => p.items[*i].label.clone(),
    };
    // The selected row scrolled into view; the first item ("Save") scrolled off.
    assert!(
        screen.contains(&selected),
        "selected row '{selected}' is visible:\n{screen}"
    );
    assert!(
        !screen.contains("Save"),
        "the first item scrolled off the top:\n{screen}"
    );
}

#[test]
fn the_model_manager_lists_the_catalog_with_characteristics() {
    let mut app = app_with("text\n");
    app.mode = Mode::ModelManager { selected: 0 };
    let screen = render(&app, 100, 24);
    assert!(screen.contains("Local AI models"), "{screen}");
    // Models appear as the Qwen3 dense family (#789) with their context window.
    assert!(
        screen.contains("Qwen3-8B"),
        "the catalog is listed:\n{screen}"
    );
    // Qwen3's native window is 40k across the family (#791) — no 1M tier.
    assert!(
        screen.contains("40k ctx"),
        "the native context window shows:\n{screen}"
    );
    // The mixed-quant catalog surfaces the quant (Q8_0 for the smallest, #789).
    assert!(screen.contains("Q8_0"), "the quant is shown:\n{screen}");
    assert!(
        screen.contains("Largest"),
        "descriptions are shown:\n{screen}"
    );
}

#[test]
fn an_open_modal_dims_the_editor_and_casts_a_shadow() {
    let mut app = app_with("# Heading one\n\nThe night was dark and long.\n");

    let render = |app: &App| {
        let mut t = Terminal::new(TestBackend::new(80, 20)).unwrap();
        t.draw(|f| draw(f, app)).unwrap();
        t.backend().buffer().clone()
    };

    // No modal open: nothing behind is dimmed.
    let plain = render(&app);
    let plain_dim = (0..80u16)
        .flat_map(|x| (0..20u16).map(move |y| (x, y)))
        .filter(|&(x, y)| plain[(x, y)].modifier.contains(Modifier::DIM))
        .count();
    assert_eq!(plain_dim, 0, "nothing is dimmed without a modal");

    // Palette open: the editor behind dims, and the box casts a shadow.
    app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    assert!(
        matches!(app.mode, Mode::Palette(_)),
        "Ctrl-K opened the palette"
    );
    let buf = render(&app);
    let mut dimmed = 0usize;
    let mut shadow = false;
    for y in 0..20u16 {
        for x in 0..80u16 {
            if buf[(x, y)].modifier.contains(Modifier::DIM) {
                dimmed += 1;
            }
            if buf[(x, y)].symbol() == "▓" {
                shadow = true;
            }
        }
    }
    assert!(dimmed > 0, "the editor behind the modal is dimmed");
    assert!(shadow, "the modal casts a drop shadow");
}

#[test]
fn an_annotated_span_is_marked_in_the_editor() {
    let mut app = app_with("first chapter\n");
    let anno_bg = app.theme.annotation;
    // Anchor a note to "first" (bytes 0..5).
    app.annotations = vec![crate::annotation::Live {
        chapter: "one.md".into(),
        start: 0,
        end: 5,
        note: "check".into(),
    }];

    let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
    terminal.draw(|f| draw(f, &app)).unwrap();
    let buf = terminal.backend().buffer().clone();

    let marked = (0..40).any(|x| (0..8).any(|y| buf[(x, y)].style().bg == Some(anno_bg)));
    assert!(
        marked,
        "the annotated span carries the annotation background"
    );
}

#[test]
fn markers_reappear_when_cursor_enters_the_span() {
    let mut app = app_with("*italic* word");
    app.buffer.cursor = Cursor { line: 0, col: 3 }; // inside "italic"
    let screen = render(&app, 80, 8);
    assert!(screen.contains("*italic*"), "screen:\n{screen}");
}

#[test]
fn current_section_is_the_last_heading_at_or_before_the_cursor() {
    let hs = vec![
        Heading {
            level: 1,
            title: "A".into(),
            start: 0,
        },
        Heading {
            level: 2,
            title: "B".into(),
            start: 50,
        },
    ];
    assert_eq!(current_section(&hs, 0), Some(0));
    assert_eq!(current_section(&hs, 49), Some(0));
    assert_eq!(current_section(&hs, 50), Some(1));
    assert_eq!(current_section(&hs, 999), Some(1));

    // Cursor above the first heading, and the no-headings case.
    let later = vec![Heading {
        level: 1,
        title: "A".into(),
        start: 10,
    }];
    assert_eq!(current_section(&later, 5), None);
    assert_eq!(current_section(&[], 0), None);
}

/// An App over an in-memory project with several chapters (no files needed
/// for rendering, which only reads chapter titles + the active buffer).
fn app_with_chapters(titles: &[&str], active_text: &str) -> App {
    App::new(buffer_with(active_text), Project::scratch_with(titles))
}

#[test]
fn pane_nests_active_chapter_headings() {
    let mut app = app_with("# Chapter\n\nbody\n\n## Scene\n\nmore\n");
    app.outline_visible = true;
    let screen = render(&app, 90, 16);
    assert!(screen.contains("CHAPTERS"), "screen:\n{screen}");
    // The scratch project's chapter, with the in-chapter headings nested.
    assert!(screen.contains("Chapter 1"), "screen:\n{screen}");
    assert!(screen.contains("Scene"), "screen:\n{screen}");
}

#[test]
fn pane_lists_chapter_even_without_headings() {
    let mut app = app_with("just prose, no headings\n");
    app.outline_visible = true;
    let screen = render(&app, 90, 12);
    assert!(screen.contains("CHAPTERS"), "screen:\n{screen}");
    assert!(screen.contains("Chapter 1"), "screen:\n{screen}");
}

#[test]
fn outline_skips_a_leading_h1_that_matches_the_chapter_title() {
    let headings = vec![
        Heading {
            level: 1,
            title: "The Crossing".into(),
            start: 0,
        },
        Heading {
            level: 2,
            title: "At the ford".into(),
            start: 30,
        },
    ];

    // The chapter is titled "The Crossing", so its opening H1 is redundant.
    let skip = redundant_title_heading(&headings, "The Crossing");
    assert_eq!(skip, Some(0));

    let texts: Vec<String> = heading_outline_lines(&headings, None, None, skip)
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("At the ford"));
    assert!(!texts[0].contains("The Crossing"));

    // A non-matching title keeps every heading.
    assert_eq!(redundant_title_heading(&headings, "Different"), None);
    assert_eq!(heading_outline_lines(&headings, None, None, None).len(), 2);
}

#[test]
fn pane_lists_all_chapters_and_marks_the_active_one() {
    let mut app = app_with_chapters(&["One", "Two", "Three"], "body\n");
    app.outline_visible = true;
    let screen = render(&app, 90, 14);
    assert!(screen.contains("One"), "screen:\n{screen}");
    assert!(screen.contains("Two"), "screen:\n{screen}");
    assert!(screen.contains("Three"), "screen:\n{screen}");
    // The active chapter (index 0) carries the active marker.
    assert!(screen.contains("▸ One"), "screen:\n{screen}");
}

#[test]
fn outline_shows_chapter_goal_progress() {
    let mut app = app_with_chapters(&["One", "Two"], "alpha beta gamma\n"); // 3 words, active
    app.outline_visible = true;

    app.project.manifest.chapters[0].goal = Some(10);
    let screen = render(&app, 90, 12);
    assert!(
        screen.contains("3 / 10"),
        "progress toward the goal:\n{screen}"
    );

    // A reached goal is marked.
    app.project.manifest.chapters[0].goal = Some(2);
    let screen = render(&app, 90, 12);
    assert!(screen.contains("3 / 2 ✓"), "reached goal marked:\n{screen}");
}

#[test]
fn nav_pane_shows_the_project_header() {
    let mut app = app_with_chapters(&["One", "Two"], "body\n");
    app.project.manifest.title = "Book of Dust".into();
    app.project.manifest.subtitle = Some("La Belle Sauvage".into());
    app.project.manifest.author = Some("Philip Pullman".into());
    app.project.manifest.series = Some("The Book of Dust".into());
    app.outline_visible = true;

    let screen = render(&app, 90, 16);
    assert!(screen.contains("Book of Dust"), "screen:\n{screen}");
    assert!(screen.contains("La Belle Sauvage"), "screen:\n{screen}");
    assert!(screen.contains("by Philip Pullman"), "screen:\n{screen}");
    assert!(screen.contains("CHAPTERS"), "screen:\n{screen}");
}

#[test]
fn epigraphs_render_above_the_body() {
    // Book epigraph (manifest) + chapter epigraph (active buffer front matter)
    // both render above chapter 0's prose, with attributions.
    let mut app = app_with("# Chapter\n\nThe prose begins here.\n");
    app.project.manifest.epigraph = Some("In the beginning.".into());
    app.project.manifest.epigraph_by = Some("Genesis".into());
    let fm = app.buffer.front_matter_mut();
    fm.set_epigraph_text("A chapter quote.");
    fm.set_epigraph_by("Someone");

    let screen = render(&app, 80, 20);
    assert!(
        screen.contains("In the beginning."),
        "book epigraph:\n{screen}"
    );
    assert!(screen.contains("— Genesis"), "book attribution:\n{screen}");
    assert!(
        screen.contains("A chapter quote."),
        "chapter epigraph:\n{screen}"
    );
    assert!(
        screen.contains("— Someone"),
        "chapter attribution:\n{screen}"
    );
    assert!(screen.contains("The prose begins here."), "body:\n{screen}");

    // The epigraph is display-only: it never entered the editable buffer.
    assert!(!app.buffer.line(0).contains("In the beginning"));
    assert!(!app.buffer.line(0).contains("A chapter quote"));
}

#[test]
fn a_multiline_epigraph_renders_one_line_each() {
    let mut app = app_with("# Chapter\n\nBody.\n");
    let fm = app.buffer.front_matter_mut();
    fm.set_epigraph_text("Roses are red\nViolets are blue");
    fm.set_epigraph_by("Anon");

    let screen = render(&app, 80, 20);
    // Each verse line renders on its own row, attribution beneath.
    let red = screen.lines().position(|l| l.contains("Roses are red"));
    let blue = screen.lines().position(|l| l.contains("Violets are blue"));
    let by = screen.lines().position(|l| l.contains("— Anon"));
    assert!(
        red.is_some() && blue.is_some() && by.is_some(),
        "screen:\n{screen}"
    );
    assert!(red < blue && blue < by, "lines out of order:\n{screen}");

    // The attribution is right-aligned (more leading space than the inset quote),
    // matching the export's `text-align: right` (#518).
    let quote_indent = screen
        .lines()
        .find(|l| l.contains("Roses are red"))
        .map(|l| l.len() - l.trim_start().len())
        .unwrap();
    let attr_indent = screen
        .lines()
        .find(|l| l.contains("— Anon"))
        .map(|l| l.len() - l.trim_start().len())
        .unwrap();
    assert!(
        attr_indent > quote_indent,
        "attribution right-aligned past the inset quote:\n{screen}"
    );
}

#[test]
fn a_long_epigraph_line_wraps_within_the_column() {
    // A single epigraph line longer than the focus column (#518): it must soft-wrap
    // onto multiple rows, none wider than the column, rather than overflow/clip.
    let mut app = app_with("# Chapter\n\nBody.\n");
    let long = "telemetry ".repeat(12); // ~120 cells, well past the 72 column
    app.buffer.front_matter_mut().set_epigraph_text(&long);

    let screen = render(&app, 100, 24);
    let rows: Vec<&str> = screen.lines().filter(|l| l.contains("telemetry")).collect();
    assert!(rows.len() > 1, "the long epigraph wrapped:\n{screen}");
    for row in rows {
        assert!(
            row.trim_end().chars().count() <= 90,
            "epigraph row stays within the column: {row:?}"
        );
    }
}

#[test]
fn editing_an_epigraph_shows_a_banner() {
    let mut app = app_with("Body.\n");
    app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)); // → Nav
    app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)); // → epigraph editor
    assert!(matches!(app.mode, Mode::EditMeta(_)));

    let screen = render(&app, 80, 10);
    assert!(
        screen.contains("editing chapter epigraph"),
        "screen:\n{screen}"
    );
    assert!(screen.contains("Ctrl-S save"), "screen:\n{screen}");
}

#[test]
fn pane_shows_a_chapter_status_badge() {
    let mut app = app_with_chapters(&["One", "Two"], "body\n");
    app.project.manifest.chapters[0].status = Some(crate::frontmatter::Status::Final);
    app.outline_visible = true;
    let screen = render(&app, 90, 14);
    let row = screen.lines().find(|l| l.contains("One")).unwrap();
    assert!(row.contains('F'), "status badge missing in row: {row:?}");
}

#[test]
fn nav_status_bar_shows_the_selected_chapters_metadata() {
    let mut app = app_with_chapters(&["One", "Two"], "body\n");
    app.project.manifest.chapters[1].summary = Some("A grand opening".into());
    app.project.manifest.chapters[1].pov = Some("Mara".into());
    app.project.manifest.chapters[1].status = Some(crate::frontmatter::Status::Revised);
    app.outline_visible = true;
    app.mode = Mode::Nav {
        selected: 1,
        filter: String::new(),
        typing: false,
    };
    let screen = render(&app, 90, 14);
    assert!(screen.contains("A grand opening"), "screen:\n{screen}");
    assert!(screen.contains("POV: Mara"), "screen:\n{screen}");
    assert!(screen.contains("Revised"), "screen:\n{screen}");
}

#[test]
fn continuous_view_shows_chapters_flowing_with_a_divider() {
    // A two-chapter project on disk (the manuscript window reads files).
    let dir = crate::testutil::scratch_dir("ui", "cont");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "Alpha prose here.\n").unwrap();
    fs::write(dir.join("two.md"), "Beta prose here.\n").unwrap();

    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);
    app.manuscript = Some(Manuscript::open(&app.project).unwrap());

    let screen = render(&app, 60, 12);
    // Both chapters' prose appears in one continuous flow...
    assert!(screen.contains("Alpha prose here."), "screen:\n{screen}");
    assert!(screen.contains("Beta prose here."), "screen:\n{screen}");
    // ...separated by a centered divider carrying the next chapter's title.
    assert!(screen.contains("—— Two ——"), "screen:\n{screen}");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn continuous_view_shows_part_headers_at_boundaries() {
    // A three-chapter project split into two parts: chapter 1 is "Part One",
    // chapters 2-3 are "Part Two". The continuous window loads all three and
    // crosses the boundary (#412).
    let dir = crate::testutil::scratch_dir("ui", "parts");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[parts]]\nname = \"Part One\"\nchapters = [1, 1]\n\n\
         [[parts]]\nname = \"Part Two\"\nchapters = [2, 3]\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n\n\
         [[chapters]]\ntitle = \"Three\"\nfile = \"three.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "Alpha prose here.\n").unwrap();
    fs::write(dir.join("two.md"), "Beta prose here.\n").unwrap();
    fs::write(dir.join("three.md"), "Gamma prose here.\n").unwrap();

    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);
    app.manuscript = Some(Manuscript::open(&app.project).unwrap());

    let screen = render(&app, 60, 16);
    // The part name is drawn (uppercased) where each part begins...
    assert!(screen.contains("PART ONE"), "first part header:\n{screen}");
    assert!(screen.contains("PART TWO"), "second part header:\n{screen}");
    // ...above the chapter divider for the chapter that starts the part.
    let part_two = screen.find("PART TWO").unwrap();
    let divider_two = screen.find("—— Two ——").unwrap();
    assert!(
        part_two < divider_two,
        "the part header sits above its divider:\n{screen}"
    );
    // It's display-only: the part name never entered the editable buffer.
    let flow = app.manuscript.as_ref().unwrap().buffer();
    assert!(
        !flow
            .line_range_text(0, flow.line_count())
            .to_uppercase()
            .contains("PART"),
        "the structure header leaked into the buffer",
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn manuscript_corkboard_renders_chapter_group_headers() {
    // The manuscript-wide board (#701) prefixes each chapter's first card with a
    // chapter-title header, so its cards read grouped by chapter.
    use crate::frontmatter::SceneStatus;
    use crate::mode::{Card, Corkboard};

    let card = |title: &str, chapter: usize, chapter_title: &str| Card {
        title: title.into(),
        level: 2,
        prose_line: String::new(),
        synopsis: String::new(),
        status: SceneStatus::Stub,
        start: 0,
        chapter: Some(chapter),
        chapter_title: chapter_title.into(),
    };
    let mut app = app_with("body\n");
    app.mode = Mode::Corkboard(Box::new(Corkboard {
        cards: vec![
            card("The Arrival", 0, "One"),
            card("The Meeting", 0, "One"),
            card("The Departure", 1, "Two"),
        ],
        selected: 0,
        manuscript: true,
    }));

    let screen = render(&app, 80, 20);
    assert!(
        screen.contains("manuscript"),
        "board titled its scope:\n{screen}"
    );
    // Each chapter's title heads its group, above that chapter's scene cards.
    let one = screen.find("One").unwrap();
    let arrival = screen.find("The Arrival").unwrap();
    let two = screen.find("Two").unwrap();
    let departure = screen.find("The Departure").unwrap();
    assert!(
        one < arrival,
        "chapter one's header sits above its cards:\n{screen}"
    );
    assert!(
        arrival < two,
        "chapter two's header follows chapter one's cards:\n{screen}"
    );
    assert!(
        two < departure,
        "chapter two's header sits above its card:\n{screen}"
    );
}

#[test]
fn import_preview_lists_the_chapters_count_and_level() {
    // The import preview (#702) shows the split before writing: the chapter
    // count, the guessed heading level, the book title, and the chapter titles.
    use crate::mode::ImportPreview;

    let mut app = app_with("body\n");
    app.mode = Mode::ImportPreview(Box::new(ImportPreview {
        source: std::path::PathBuf::from("/tmp/book.md"),
        dest: std::path::PathBuf::from("/tmp/proj"),
        markdown: Some(String::new()),
        title: Some("The Long Road".into()),
        author: Some("A. Writer".into()),
        chapters: vec![
            ("One".into(), "a\n".into()),
            ("Two".into(), "b\n".into()),
            ("Three".into(), "c\n".into()),
        ],
        level: Some(2),
        levels: vec![1, 2, 3],
        warning: String::new(),
        scroll: 0,
    }));

    let screen = render(&app, 80, 24);
    assert!(
        screen.contains("3 chapters will be created"),
        "the chapter count is shown before writing:\n{screen}"
    );
    assert!(
        screen.contains("heading level 2"),
        "the guessed split level is shown:\n{screen}"
    );
    assert!(
        screen.contains("The Long Road") && screen.contains("A. Writer"),
        "the mined title/author are shown:\n{screen}"
    );
    for title in ["One", "Two", "Three"] {
        assert!(
            screen.contains(title),
            "chapter {title} is listed:\n{screen}"
        );
    }
    assert!(
        screen.contains("enter import") && screen.contains("esc cancel"),
        "the confirm/cancel affordance is shown:\n{screen}"
    );
}
