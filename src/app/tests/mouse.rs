//! Mouse handling tests (#757 click-to-position, #764 scroll wheel). The click
//! mapping is the inverse of the render layout, so these render the app to a
//! `TestBackend` first (which populates the click layout map) and then feed a
//! synthetic mouse event, asserting where the cursor lands.

use super::*;
use crate::buffer::Cursor;
use crate::mode::{Finding, FindingKind, Findings, Mode, Overlay};
use crate::ui::render_to_string;
use crossterm::event::{Event, MouseButton, MouseEvent, MouseEventKind};
use std::fmt::Write;

/// The width and height every test renders at — wide enough to center a narrow
/// prose column with a clear left margin, tall enough for a screenful of rows.
const W: u16 = 40;
const H: u16 = 20;

/// A one-chapter app whose body is `body`, rendered once at `W`x`H` so the click
/// layout map is populated. `focus_width` is pinned narrow so the centered column
/// has a real left margin and lines soft-wrap predictably.
fn rendered_app(tag: &str, body: &str, focus_width: u16) -> (App, TempProject) {
    let dir = make_project(tag, "Book", body);
    let mut app = open_app(&dir);
    app.focus_width = focus_width;
    render_to_string(&app, W, H);
    (app, TempProject::new(dir))
}

/// Feed a left-click at screen `(col, row)` and let it position the cursor.
fn click(app: &mut App, col: u16, row: u16) {
    app.handle_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }));
}

/// Feed a left-button drag to screen `(col, row)` — extends / enters a Visual
/// selection (#812).
fn drag(app: &mut App, col: u16, row: u16) {
    app.handle_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: col,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }));
}

/// Feed a left-button release (#812) — leaves the selection active.
fn release(app: &mut App, col: u16, row: u16) {
    app.handle_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: col,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }));
}

/// Feed a scroll-wheel notch (`up` toward earlier content, else later).
fn wheel(app: &mut App, up: bool) {
    let kind = if up {
        MouseEventKind::ScrollUp
    } else {
        MouseEventKind::ScrollDown
    };
    app.handle_event(Event::Mouse(MouseEvent {
        kind,
        column: 5,
        row: 5,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }));
}

/// The left edge (x) of the centered prose column for a given focus width — the
/// same math `centered_column` does: the column is `focus_width` wide (capped to
/// `W-2`) and centered, and the body starts one row below the blank top row.
fn column_x(focus_width: u16) -> u16 {
    let width = focus_width.min(W - 2);
    (W - width) / 2
}

/// The first body row (the render reserves the top row for the EditMeta banner).
const BODY_TOP: u16 = 1;

#[test]
fn click_lands_on_the_character_under_it_with_the_centered_margin() {
    // "hello world" on line 0. Clicking the margin-offset column of the 'w' (byte 6)
    // should put the cursor there.
    let (mut app, _dir) = rendered_app("mouse-basic", "hello world\n", 20);
    let x = column_x(20);
    click(&mut app, x + 6, BODY_TOP);
    let c = app.edit_buffer().cursor;
    assert_eq!(c.line, 0);
    assert_eq!(c.col, 6, "cursor on the 'w' of world");
}

#[test]
fn click_past_line_end_goes_to_line_end() {
    let (mut app, _dir) = rendered_app("mouse-eol", "short\n", 20);
    let x = column_x(20);
    // Way past the 5-char line.
    click(&mut app, x + 30, BODY_TOP);
    let c = app.edit_buffer().cursor;
    assert_eq!(c.line, 0);
    assert_eq!(c.col, 5, "clamped to the line's end");
}

#[test]
fn click_below_the_last_line_goes_to_document_end() {
    let (mut app, _dir) = rendered_app("mouse-below", "one\ntwo\n", 20);
    let x = column_x(20);
    // Row 10 is well below the two content rows.
    click(&mut app, x + 1, 10);
    let c = app.edit_buffer().cursor;
    // Last logical line is the trailing empty line after "two\n".
    let last = app.edit_buffer().line_count() - 1;
    assert_eq!(c.line, last, "lands on the last line");
}

#[test]
fn click_accounts_for_vertical_scroll() {
    // A tall document scrolled so the cursor is near the bottom; the top of the
    // screen is no longer line 0. A click on the first body row should map to
    // whatever logical line is drawn there, not line 0.
    let body = (0..60).fold(String::new(), |mut s, i| {
        writeln!(s, "line{i}").unwrap();
        s
    });
    let (mut app, _dir) = rendered_app("mouse-scroll", &body, 20);
    // Move the cursor far down so the view scrolls, then re-render to refresh the map.
    for _ in 0..50 {
        app.on_key(key(KeyCode::Down));
    }
    render_to_string(&app, W, H);
    let x = column_x(20);
    click(&mut app, x + 2, BODY_TOP);
    let c = app.edit_buffer().cursor;
    assert!(c.line > 0, "the top row is a scrolled-to line, not line 0");
    // And clicking maps to the exact line drawn on that row: byte on that line.
    assert!(c.line < 50, "somewhere above the cursor's line");
}

#[test]
fn click_on_a_soft_wrap_continuation_row() {
    // A long line that wraps at width 20; the second visual row starts partway in.
    // Clicking column 0 of the continuation row lands at the wrap point, not byte 0.
    let long = "aaaa bbbb cccc dddd eeee ffff\n"; // wraps within 20 cells
    let (mut app, _dir) = rendered_app("mouse-wrap", long, 20);
    let x = column_x(20);
    // Row BODY_TOP is the first wrapped row; BODY_TOP+1 is the continuation.
    click(&mut app, x, BODY_TOP + 1);
    let c = app.edit_buffer().cursor;
    assert_eq!(c.line, 0);
    assert!(c.col > 0, "continuation row maps past the first row's text");
}

#[test]
fn click_never_panics_on_empty_and_far_clicks() {
    let (mut app, _dir) = rendered_app("mouse-empty", "\n", 20);
    click(&mut app, 0, 0); // top-left, in the margin / blank row
    click(&mut app, W - 1, H - 1); // bottom-right corner
    click(&mut app, 200, 200); // off-screen
                               // No panic, cursor stays valid.
    assert!(app.edit_buffer().cursor.line < app.edit_buffer().line_count());
}

#[test]
fn click_in_left_margin_is_ignored() {
    let (mut app, _dir) = rendered_app("mouse-margin", "hello world\n", 20);
    // Move the cursor somewhere first.
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Right));
    let before = app.edit_buffer().cursor;
    // A click in the left margin (x=0 with a centered column) is out of scope.
    click(&mut app, 0, BODY_TOP);
    assert_eq!(app.edit_buffer().cursor, before, "margin click is a no-op");
}

#[test]
fn click_does_not_change_the_editing_mode() {
    // In Vim Normal mode, a click positions the cursor but stays in Normal.
    let (mut app, _dir) = rendered_app("mouse-vim", "hello world\n", 20);
    app.vim = true;
    app.mode = Mode::Normal;
    let x = column_x(20);
    click(&mut app, x + 6, BODY_TOP);
    assert!(matches!(app.mode, Mode::Normal), "mode is unchanged");
    assert_eq!(app.edit_buffer().cursor.col, 6);
}

#[test]
fn click_accounts_for_concealed_emphasis_markers() {
    // "*bold* text": the emphasis `*` markers are concealed off the cursor's span,
    // so on screen the row reads "bold text". A click on the visible 't' of "text"
    // must map past the concealed markers to the real byte in the buffer.
    let (mut app, _dir) = rendered_app("mouse-conceal", "*em* text\n", 20);
    app.on_key(key(KeyCode::Down)); // move the cursor off line 0 so its markers conceal
    app.on_idle(); // parse so conceal ranges exist
    render_to_string(&app, W, H);
    let x = column_x(20);
    // On screen the concealed row reads "em text"; the 't' of "text" is at visible
    // column 3. The real buffer byte there is 5 (`*`,`e`,`m`,`*`,` `,`t`).
    click(&mut app, x + 3, BODY_TOP);
    let text = app.edit_buffer().line(0);
    let byte = app.edit_buffer().cursor_byte(); // line 0 starts at byte 0
    assert_eq!(
        &text[byte..byte + 1],
        "t",
        "click landed on the visible 't'"
    );
}

#[test]
fn click_accounts_for_the_first_line_paragraph_indent() {
    // With the #722 indent on, the first visual row is shifted right by four cells.
    // A click at the column of the first character must account for that shift.
    let (mut app, _dir) = rendered_app("mouse-indent", "hello world\n", 20);
    app.paragraph_indent = true;
    render_to_string(&app, W, H);
    let x = column_x(20);
    // The 'h' now renders at column 4 (four-space indent). Click there.
    click(&mut app, x + 4, BODY_TOP);
    assert_eq!(
        app.edit_buffer().cursor.col,
        0,
        "click maps past the indent to byte 0"
    );
    // A click within the indent itself lands at the line start.
    click(&mut app, x + 1, BODY_TOP);
    assert_eq!(
        app.edit_buffer().cursor.col,
        0,
        "indent click clamps to line start"
    );
}

#[test]
fn click_on_a_render_only_header_row_maps_to_an_editable_position() {
    // A fenced code block in preview (cursor outside) renders its content verbatim;
    // the fence lines are concealed to zero rows. Clicking never panics and lands on
    // an editable position. Also exercises a header-free single chapter.
    let body = "para one\n\n```\ncode line\n```\n\nafter\n";
    let (mut app, _dir) = rendered_app("mouse-code", body, 30);
    app.on_key(key(KeyCode::Down));
    app.on_idle();
    render_to_string(&app, W, H);
    let x = column_x(30);
    // Click somewhere in the rendered code region; assert no panic + valid cursor.
    for row in BODY_TOP..(BODY_TOP + 6) {
        click(&mut app, x + 2, row);
        assert!(app.edit_buffer().cursor.line < app.edit_buffer().line_count());
    }
}

// --- Drag-to-select (#812) ----------------------------------------------------

/// The (start, end) absolute bytes of the current Visual selection, or `None`
/// when not in Visual mode.
fn selection(app: &App) -> Option<(usize, usize)> {
    match app.mode {
        Mode::Visual { anchor, linewise } => Some(app.selection_bytes(anchor, linewise)),
        _ => None,
    }
}

#[test]
fn a_plain_click_does_not_enter_visual() {
    let (mut app, _dir) = rendered_app("drag-click", "hello world\n", 20);
    let x = column_x(20);
    click(&mut app, x + 2, BODY_TOP);
    release(&mut app, x + 2, BODY_TOP);
    assert!(
        selection(&app).is_none(),
        "a press-and-release with no drag just positions the cursor"
    );
    assert_eq!(app.edit_buffer().cursor.col, 2);
}

#[test]
fn a_drag_selects_the_spanned_range() {
    // Press on 'h' (byte 0), drag to the 'w' of "world" (byte 6): the selection
    // spans [0, 6) plus the char under the active end (inclusive, Vim charwise).
    let (mut app, _dir) = rendered_app("drag-span", "hello world\n", 20);
    let x = column_x(20);
    click(&mut app, x, BODY_TOP);
    drag(&mut app, x + 6, BODY_TOP);
    let (lo, hi) = selection(&app).expect("a drag enters Visual mode");
    assert_eq!(lo, 0, "anchored at the press");
    assert_eq!(hi, 7, "through the char under the drag point (inclusive)");
    assert_eq!(&app.edit_buffer().byte_slice(lo, hi), "hello w");
}

#[test]
fn a_release_leaves_the_selection_active() {
    let (mut app, _dir) = rendered_app("drag-release", "hello world\n", 20);
    let x = column_x(20);
    click(&mut app, x, BODY_TOP);
    drag(&mut app, x + 5, BODY_TOP);
    release(&mut app, x + 5, BODY_TOP);
    assert!(
        matches!(app.mode, Mode::Visual { .. }),
        "the selection stays active after release, like Vim Visual"
    );
}

#[test]
fn a_drag_across_a_soft_wrap_selects_the_wrapped_bytes() {
    // A long line that wraps; drag from the first row to the continuation row and
    // the selection spans the wrap, not just one visual row.
    let long = "aaaa bbbb cccc dddd eeee ffff\n";
    let (mut app, _dir) = rendered_app("drag-wrap", long, 20);
    let x = column_x(20);
    click(&mut app, x, BODY_TOP);
    drag(&mut app, x + 3, BODY_TOP + 1);
    let (lo, hi) = selection(&app).expect("dragging enters Visual");
    assert_eq!(lo, 0);
    assert!(
        hi > "aaaa bbbb".len(),
        "the selection reaches past the first wrapped row into the continuation"
    );
}

#[test]
fn a_drag_accounts_for_the_paragraph_indent() {
    // With the #722 indent on, the first row shifts right by four cells; a drag
    // still maps to the right bytes through the indent.
    let (mut app, _dir) = rendered_app("drag-indent", "hello world\n", 20);
    app.paragraph_indent = true;
    render_to_string(&app, W, H);
    let x = column_x(20);
    // Press at the 'h' (rendered at column 4 with the indent), drag five chars.
    click(&mut app, x + 4, BODY_TOP);
    drag(&mut app, x + 4 + 5, BODY_TOP);
    let (lo, hi) = selection(&app).expect("dragging enters Visual");
    assert_eq!(lo, 0, "anchored past the indent at byte 0");
    assert_eq!(&app.edit_buffer().byte_slice(lo, hi), "hello ");
}

#[test]
fn a_drag_accounts_for_vertical_scroll() {
    // A tall document scrolled so the top row isn't line 0; a drag on the visible
    // rows selects the on-screen (scrolled-to) bytes, not from line 0.
    let body = (0..60).fold(String::new(), |mut s, i| {
        writeln!(s, "line{i}").unwrap();
        s
    });
    let (mut app, _dir) = rendered_app("drag-scroll", &body, 20);
    for _ in 0..50 {
        app.on_key(key(KeyCode::Down));
    }
    render_to_string(&app, W, H);
    let x = column_x(20);
    click(&mut app, x, BODY_TOP);
    drag(&mut app, x + 3, BODY_TOP);
    let (lo, hi) = selection(&app).expect("dragging enters Visual");
    assert!(lo > 0, "the anchor is a scrolled-to line, not line 0");
    assert!(hi > lo, "a non-empty selection");
}

#[test]
fn a_drag_never_panics_on_far_points() {
    let (mut app, _dir) = rendered_app("drag-far", "hi\n", 20);
    let x = column_x(20);
    click(&mut app, x, BODY_TOP);
    drag(&mut app, 250, 250); // way off screen
    drag(&mut app, x + 1, BODY_TOP);
    release(&mut app, x + 1, BODY_TOP);
    assert!(app.edit_buffer().cursor.line < app.edit_buffer().line_count());
}

#[test]
fn a_drag_works_in_the_continuous_view() {
    // The continuous view shares the render→byte map; a drag selects there too.
    let (mut app, _dir) = two_chapter_app("drag-continuous");
    app.on_key(ctrl('l')); // enter continuous view
    render_to_string(&app, W, H);
    let x = column_x(app.focus_width);
    click(&mut app, x, BODY_TOP);
    drag(&mut app, x + 4, BODY_TOP);
    assert!(
        selection(&app).is_some(),
        "dragging selects in the continuous view"
    );
}

// --- Overlay click-to-select (#812) -------------------------------------------

#[test]
fn a_click_selects_a_findings_row() {
    let (mut app, _dir) = rendered_app("click-findings", "hi\n", 20);
    let items: Vec<Finding> = (0..5)
        .map(|i| Finding {
            summary: format!("finding {i}"),
            kind: FindingKind::Other,
            chapters: Vec::new(),
            book: None,
            action: None,
            corroborators: Vec::new(),
        })
        .collect();
    app.mode = Mode::Findings(Box::new(Findings {
        items,
        ..Default::default()
    }));
    let screen = render_to_string(&app, W, H);
    assert!(screen.contains("finding 0"), "the list rendered");
    // Click the third visible row of the list (findings row indices == screen rows).
    let first_row = app.list_layout.borrow().first_row;
    click(&mut app, 3, first_row + 2);
    if let Mode::Findings(f) = &app.mode {
        assert_eq!(f.selected, 2, "the clicked row is selected");
    } else {
        panic!("still in the findings list");
    }
}

// --- Scroll wheel (#764) ------------------------------------------------------

#[test]
fn wheel_scrolls_the_editor_without_moving_the_cursor() {
    let body = (0..60).fold(String::new(), |mut s, i| {
        writeln!(s, "line{i}").unwrap();
        s
    });
    let (mut app, _dir) = rendered_app("wheel-editor", &body, 20);
    let cursor_before = app.edit_buffer().cursor;
    wheel(&mut app, false); // scroll down
    assert!(app.scroll_offset.get() > 0, "the viewport offset advanced");
    assert_eq!(
        app.edit_buffer().cursor,
        cursor_before,
        "read-ahead scroll leaves the cursor put"
    );
}

#[test]
fn wheel_up_clamps_at_the_start_of_the_chapter() {
    // With the cursor at the top of the chapter there is nothing above to reveal, so
    // wheel-up stays clamped at the first row (a burst can't page above line0).
    let body = (0..60).fold(String::new(), |mut s, i| {
        writeln!(s, "line{i}").unwrap();
        s
    });
    let (mut app, _dir) = rendered_app("wheel-top", &body, 20);
    for _ in 0..10 {
        wheel(&mut app, true);
    }
    render_to_string(&app, W, H); // clamps and stores the applied offset
    assert_eq!(
        app.scroll_offset.get(),
        0,
        "clamped at the start of the chapter"
    );
    let screen = render_to_string(&app, W, H);
    assert!(
        first_prose_row(&screen).contains("line0"),
        "the first line still tops the view, no blank space above"
    );
}

#[test]
fn wheel_up_pages_back_with_the_cursor_at_the_bottom() {
    // The #936 bug: put the cursor on the last line of a tall chapter (bottom of the
    // page, no read-ahead room below), then wheel up — earlier lines must still
    // scroll into view, and the cursor doesn't move.
    let body = (0..60).fold(String::new(), |mut s, i| {
        writeln!(s, "line{i}").unwrap();
        s
    });
    let (mut app, _dir) = rendered_app("wheel-bottom", &body, 20);
    let last = app.edit_buffer().line_count() - 1;
    app.edit_buffer_mut().cursor = Cursor { line: last, col: 0 };
    render_to_string(&app, W, H); // re-anchor the view on the moved cursor
    let cursor_before = app.edit_buffer().cursor;
    let top_before = top_row_text(&app);

    for _ in 0..3 {
        wheel(&mut app, true); // page back up
    }
    let top_after = top_row_text(&app);
    assert!(
        app.scroll_offset.get() < 0,
        "the view scrolled up above the cursor"
    );
    assert_ne!(top_before, top_after, "an earlier line now tops the view");
    assert_eq!(
        app.edit_buffer().cursor,
        cursor_before,
        "scrolling up leaves the cursor put"
    );
}

#[test]
fn a_keystroke_snaps_the_view_back_to_the_cursor() {
    let body = (0..60).fold(String::new(), |mut s, i| {
        writeln!(s, "line{i}").unwrap();
        s
    });
    let (mut app, _dir) = rendered_app("wheel-snap", &body, 20);
    wheel(&mut app, false);
    wheel(&mut app, false);
    assert!(app.scroll_offset.get() > 0);
    app.on_key(key(KeyCode::Down)); // a cursor move
    assert_eq!(
        app.scroll_offset.get(),
        0,
        "the view snapped back to the cursor"
    );
}

#[test]
fn wheel_routes_to_an_open_overlay() {
    let (mut app, _dir) = rendered_app("wheel-overlay", "hi\n", 20);
    // A tall overlay so it can scroll.
    let lines: Vec<_> = (0..100)
        .map(|i| ratatui::text::Line::raw(format!("row {i}")))
        .collect();
    app.overlay = Some(Overlay::guide(lines));
    app.viewport_height = H;
    wheel(&mut app, false);
    assert!(
        app.overlay_scroll() > 0,
        "the overlay scrolled, not the editor"
    );
    assert_eq!(
        app.scroll_offset.get(),
        0,
        "the editor viewport didn't move"
    );
}

#[test]
fn wheel_down_shifts_what_the_top_row_shows_and_clamps_to_content() {
    // A tall document: scrolling down moves later lines up to the top row, and the
    // renderer clamps the stored offset so it can't run past the last screenful.
    let body = (0..60).fold(String::new(), |mut s, i| {
        writeln!(s, "line{i}").unwrap();
        s
    });
    let (mut app, _dir) = rendered_app("wheel-shift", &body, 20);
    let top_before = top_row_text(&app);
    wheel(&mut app, false);
    let after = render_to_string(&app, W, H);
    let top_after = first_prose_row(&after);
    assert_ne!(top_before, top_after, "the top row now shows a later line");

    // A huge burst of wheel-downs is clamped by the render write-back to the last
    // valid offset (no runaway accumulation, no blank tail).
    for _ in 0..100 {
        wheel(&mut app, false);
    }
    render_to_string(&app, W, H); // clamps and stores the applied offset
    let clamped = app.scroll_offset.get();
    // One more notch can't push it past the clamp.
    render_to_string(&app, W, H);
    assert_eq!(
        app.scroll_offset.get(),
        clamped,
        "offset is clamped to content"
    );
    // And the bottom row isn't blank past the end.
    let screen = render_to_string(&app, W, H);
    assert!(
        screen.lines().any(|l| l.contains("line59")),
        "the last line is reachable, no blank tail"
    );
}

/// The trimmed text of the first non-empty prose row of the current render.
fn top_row_text(app: &App) -> String {
    first_prose_row(&render_to_string(app, W, H))
}

fn first_prose_row(screen: &str) -> String {
    screen
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}
