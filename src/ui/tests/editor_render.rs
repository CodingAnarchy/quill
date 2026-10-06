//! Editor render tests: pane splitting, soft-wrap, the Vim status chip, visual
//! selection highlight, emphasis-marker concealment, typewriter/focus scrolling,
//! the first-line paragraph indent (#722), annotation marking, and the
//! current-section lookup. Pure move (#792).

use super::*;
use crate::buffer::Cursor;
use crate::project::Goals;
use crate::syntax::Heading;
use crate::ui::nav_pane::current_section;
use ratatui::style::Color;
use ratatui::{backend::TestBackend, Terminal};
use std::fmt::Write;

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
    app.flags.vim = true;
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
    app.flags.vim = true;
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
fn visual_mode_status_bar_shows_the_selected_count() {
    // Selection tallies only the selected bytes, not the whole document (#824).
    let mut app = app_with("one two three four\n");
    app.flags.vim = true;
    // Non-Visual: the status bar shows the document count, no "selected" tally.
    let doc = render(&app, 80, 8);
    assert!(
        doc.contains("4 words"),
        "document count when not selecting:\n{doc}"
    );
    assert!(
        !doc.contains("selected"),
        "no selection tally at rest:\n{doc}"
    );

    // Visual, anchor at 0, cursor over the last char of "two" (col 6): the
    // selection spans "one two" inclusively — 2 words, 7 chars.
    app.mode = Mode::Visual {
        anchor: 0,
        linewise: false,
    };
    app.buffer.cursor = Cursor { line: 0, col: 6 };
    let screen = render(&app, 80, 8);
    assert!(
        screen.contains("selected 2 words · 7 chars"),
        "selected count is scoped to the selection:\n{screen}"
    );
    // The whole-document count is not shown while a selection is live.
    assert!(
        !screen.contains("4 words"),
        "document count is replaced by the selection count:\n{screen}"
    );
}

#[test]
fn the_below_target_cue_shows_in_the_status_bar_and_hides() {
    // A daily target set, and today's net under it, so the ambient cue (#1012)
    // shows the shortfall in the editor status bar.
    let mut app = app_with("one two three\n");
    app.project.manifest.goals = Some(Goals {
        daily: Some(100),
        ..Goals::default()
    });
    app.set_day_tally_for_test(20, 0); // today's net is 20, 80 short of the 100 target
    assert!(
        render(&app, 80, 8).contains("80 to daily goal"),
        "the cue shows the shortfall:\n{}",
        render(&app, 80, 8)
    );
    // Turned off (config or the palette toggle): no cue.
    app.flags.daily_nudge = false;
    assert!(
        !render(&app, 80, 8).contains("to daily goal"),
        "an opted-out cue never shows:\n{}",
        render(&app, 80, 8)
    );
    // Meeting the target removes it (nudge back on, net now over the target).
    app.flags.daily_nudge = true;
    app.set_day_tally_for_test(120, 0);
    assert!(
        !render(&app, 80, 8).contains("to daily goal"),
        "a met target shows no cue:\n{}",
        render(&app, 80, 8)
    );
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
    app.flags.typewriter = false;
    let off = row_of_last(&app);
    // On: it's lifted back to the vertical centre, text scrolling under it.
    app.flags.typewriter = true;
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
    app.flags.focus_mode = true;
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

#[test]
fn paragraph_indent_indents_only_the_first_line_of_a_prose_paragraph() {
    // A long single-paragraph logical line that soft-wraps to several rows.
    let long = "word ".repeat(40);
    let mut app = app_with(&format!("{long}\n"));
    let w = 60;
    let h = 20;

    // Off (default): the paragraph's rows all start at the same column.
    app.flags.paragraph_indent = false;
    let base = indent_of_row(&app, "word word", w, h).expect("paragraph on screen");

    // On: the first row shifts right by the four-space indent…
    app.flags.paragraph_indent = true;
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
    app.flags.paragraph_indent = true;
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

    app.flags.paragraph_indent = true;
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
        app.flags.paragraph_indent = false;
        let b = indent_of_row(&app, "Following line.", w, h).expect("following on screen");
        app.flags.paragraph_indent = true;
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
    app.flags.paragraph_indent = true;
    assert_eq!(
        indent_of_row(&app, "code line", w, h),
        Some(base_code),
        "fenced code content is not indented"
    );
}

#[test]
fn a_scene_break_renders_as_a_centered_spaced_divider() {
    // A thematic break between two paragraphs renders as a centered `* * *` divider
    // (not the raw `---`), with blank breathing room above and below it (#908).
    let app = app_with("Scene one ends.\n\n---\n\nScene two opens.\n");
    let screen = render(&app, 60, 16);
    let rows: Vec<&str> = screen.lines().collect();

    // The raw markers are gone; the centered glyph is shown instead.
    assert!(
        !screen.contains("---"),
        "the raw scene break is hidden:\n{screen}"
    );
    let glyph = rows
        .iter()
        .position(|l| l.contains("* * *"))
        .expect("the divider glyph renders");
    // Centered: the glyph sits well right of the left margin (indented by padding).
    let indent = rows[glyph].len() - rows[glyph].trim_start().len();
    assert!(indent > 10, "the divider is centered: {:?}", rows[glyph]);

    // Breathing room: the rows just above and below the divider are blank, and the
    // paragraphs themselves are further out — more than a plain paragraph gap.
    assert!(
        rows[glyph - 1].trim().is_empty() && rows[glyph + 1].trim().is_empty(),
        "the divider has blank rows immediately around it:\n{screen}"
    );
    let one = rows.iter().position(|l| l.contains("Scene one")).unwrap();
    let two = rows.iter().position(|l| l.contains("Scene two")).unwrap();
    assert!(
        glyph - one >= 3 && two - glyph >= 3,
        "extra spacing separates the divider from the prose:\n{screen}"
    );
}

#[test]
fn a_scene_break_is_display_only_and_reveals_on_cursor() {
    // Rendering the divider never mutates the buffer bytes, and placing the cursor
    // on the break line reveals the raw markers for editing (#908).
    let text = "Scene one ends.\n\n---\n\nScene two opens.\n";
    let mut app = app_with(text);
    let before = app.edit_buffer().text();

    // Cursor away from the break: the divider shows, bytes untouched.
    let away = render(&app, 60, 16);
    assert!(away.contains("* * *"), "the divider renders:\n{away}");
    assert!(!away.contains("---"), "the raw markers are hidden:\n{away}");
    assert_eq!(
        app.edit_buffer().text(),
        before,
        "the preview left the buffer byte-for-byte unchanged"
    );

    // Cursor on the break line (index 2): the raw `---` reveals, divider gone.
    app.edit_buffer_mut().cursor = Cursor { line: 2, col: 0 };
    let revealed = render(&app, 60, 16);
    assert!(
        revealed.contains("---"),
        "the raw markers reveal under the cursor:\n{revealed}"
    );
    assert!(
        !revealed.contains("* * *"),
        "the divider is replaced by the raw text:\n{revealed}"
    );
    assert_eq!(
        app.edit_buffer().text(),
        before,
        "revealing the raw markers left the bytes unchanged"
    );
}

#[test]
fn paragraph_indent_skips_the_paragraph_after_a_scene_break() {
    // The first paragraph resuming after a scene break is flush-left (#908), like the
    // first paragraph after a heading — while a later paragraph still indents.
    let mut app = app_with("First para.\n\n---\n\nResuming para.\n\nLater para.\n");
    let w = 60;
    let h = 18;
    let base_resuming = indent_of_row(&app, "Resuming para.", w, h).expect("resuming on screen");
    let base_later = indent_of_row(&app, "Later para.", w, h).expect("later on screen");

    app.flags.paragraph_indent = true;
    assert_eq!(
        indent_of_row(&app, "Resuming para.", w, h),
        Some(base_resuming),
        "the paragraph after a scene break is not indented"
    );
    assert_eq!(
        indent_of_row(&app, "Later para.", w, h),
        Some(base_later + 4),
        "a later paragraph still indents"
    );
}

#[test]
fn paragraph_indent_skips_the_paragraph_after_a_code_block() {
    // The paragraph resuming after a fenced code block is flush-left (#908).
    let mut app = app_with("Intro.\n\n```\ncode\n```\n\nAfter code.\n");
    let w = 60;
    let h = 18;
    let base_after = indent_of_row(&app, "After code.", w, h).expect("after-code on screen");
    app.flags.paragraph_indent = true;
    assert_eq!(
        indent_of_row(&app, "After code.", w, h),
        Some(base_after),
        "the paragraph after a code block is not indented"
    );
}

#[test]
fn paragraph_indent_keeps_the_cursor_on_the_indented_line() {
    // Cursor at the very start of an indented paragraph's first line.
    let mut app = app_with("Alpha beta gamma.\n");
    app.edit_buffer_mut().cursor = Cursor { line: 0, col: 0 };

    let cursor_x = |app: &App| -> u16 {
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal.get_cursor_position().unwrap().x
    };

    let off = cursor_x(&app);
    app.flags.paragraph_indent = true;
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
fn an_annotated_span_is_marked_in_the_editor() {
    let mut app = app_with("first chapter\n");
    let anno_bg = app.theme.annotation;
    // Anchor a note to "first" (bytes 0..5).
    app.annotations = vec![crate::annotation::Live {
        chapter: "one.md".into(),
        start: 0,
        end: 5,
        note: "check".into(),
        replies: Vec::new(),
        resolved: false,
        reviewer: None,
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
fn active_style_flag_hits_are_underlined_and_clear_when_off() {
    // The sweep's cached hits (#1001) underline in place like a misspelling, and
    // clearing the sweep removes the mark on the next frame.
    let mut app = app_with("She walked slowly.\n");
    // Underline the adverb "slowly" (bytes 11..17).
    app.style_flag_category = Some(crate::analysis::FlagCategory::Adverbs);
    app.style_flags = vec![(11, 17)];

    let underlined = |app: &App| -> bool {
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..40).any(|x| {
            (0..8).any(|y| {
                buf[(x, y)]
                    .style()
                    .add_modifier
                    .contains(ratatui::style::Modifier::UNDERLINED)
            })
        })
    };

    assert!(underlined(&app), "the active flag hit is underlined");

    // Toggling the sweep off clears the cache, so nothing underlines.
    app.style_flag_category = None;
    app.style_flags.clear();
    assert!(
        !underlined(&app),
        "clearing the sweep removes the underline"
    );
}

#[test]
fn proximity_repeats_are_dim_marked_in_view_and_clear_when_off() {
    // A word echoed within the window (#1002) dim-marks in the visible viewport
    // while the toggle is on; toggling off clears the cache so nothing dims, and the
    // prose is never modified.
    let before = "She turned, then he turned.\n";
    let mut app = app_with(before);
    app.toggle_proximity_repeats(); // on: computes the marks for the visible window
    assert!(
        !app.proximity_repeats.is_empty(),
        "the close repeat is detected in view"
    );

    let dimmed = |app: &App| -> bool {
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..40).any(|x| {
            (0..8).any(|y| {
                buf[(x, y)]
                    .style()
                    .add_modifier
                    .contains(ratatui::style::Modifier::DIM)
            })
        })
    };

    assert!(
        dimmed(&app),
        "the near-proximity repeat is dim-marked in view"
    );

    // Toggling off clears the cache, so nothing dims on the next frame.
    app.toggle_proximity_repeats();
    assert!(
        app.proximity_repeats.is_empty(),
        "toggling off clears the cache"
    );
    assert!(!dimmed(&app), "clearing the marking removes the dim marker");

    // The marking only observes — the prose is untouched by either toggle.
    assert_eq!(app.edit_buffer().text(), before, "prose is never modified");
}

#[test]
fn distant_proximity_repeat_is_not_marked() {
    // The same word repeated far apart (past the window) is ordinary recurrence, not
    // a near-echo, so nothing is cached to mark.
    let mut filler = String::new();
    for i in 0..40 {
        write!(filler, "filler{i} ").unwrap();
    }
    let mut app = app_with(&format!("She turned away. {filler} He turned back.\n"));
    app.toggle_proximity_repeats();
    assert!(
        app.proximity_repeats.is_empty(),
        "a distant repeat is not a near-echo: {:?}",
        app.proximity_repeats
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

#[test]
fn a_wheel_scroll_does_not_overshoot_past_the_end() {
    // #5: when the top visible logical line is a wrapped paragraph whose first rows
    // are scrolled above the viewport (so `header_rows(top) == 0` but `to_skip > 0`),
    // the read-ahead scroll clamp must count the top line's *remaining* visible
    // height, not its whole body. A short intro line owns the chapter-title header
    // (line 0), so the long paragraph below it has no header insert of its own.
    let long = "lorem ".repeat(200); // wraps to far more rows than the page height
    let app = app_with(&format!("Intro\n\n{long}")); // typed in — cursor rests at the end

    // The cursor sits on the final line and the view already shows the last
    // screenful, so there is genuinely no room to scroll further: the applied
    // offset must clamp to zero rather than advancing into blank space past the end.
    assert_eq!(
        app.buffer.cursor.line,
        app.buffer.line_count() - 1,
        "the cursor rests on the last logical line"
    );
    app.scroll_offset.set(30); // a wheel-down while already at the bottom
    render(&app, 60, 14);
    assert_eq!(
        app.scroll_offset.get(),
        0,
        "a wheel-down at the true bottom stays put — no overshoot into blank rows"
    );
}
