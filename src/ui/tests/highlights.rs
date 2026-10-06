//! Per-cell highlight styling across wrapped rows — the row-window narrowing of
//! the conceal / search / spell / proximity / annotation lists must style exactly
//! the cells the whole-list scans did, including ranges that start on an earlier
//! row and overlapping or nested annotations — plus the status bar's right-hand
//! alignment with multi-byte glyphs present.

use super::*;
use crate::annotation::Live;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer as Cells;
use ratatui::style::Modifier;
use ratatui::{backend::TestBackend, Terminal};

/// Render `app` and return the screen's cells.
fn cells(app: &App, w: u16, h: u16) -> Cells {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| draw(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

/// The screen position of the `nth` (0-based) on-screen occurrence of `word`,
/// scanning rows top to bottom. The test text is ASCII, so a row's char index is
/// its column.
fn find(buf: &Cells, word: &str, nth: usize) -> (u16, u16) {
    let area = buf.area;
    let mut seen = 0;
    for y in 0..area.height {
        let row: String = (0..area.width).map(|x| buf[(x, y)].symbol()).collect();
        let mut from = 0;
        while let Some(i) = row[from..].find(word) {
            if seen == nth {
                return ((from + i) as u16, y);
            }
            seen += 1;
            from += i + word.len();
        }
    }
    panic!("{word:?} #{nth} not on screen");
}

/// An annotation over bytes `start..end`.
fn note(start: usize, end: usize) -> Live {
    Live {
        chapter: "one.md".into(),
        start,
        end,
        note: "n".into(),
        replies: Vec::new(),
        resolved: false,
        reviewer: None,
    }
}

#[test]
fn highlights_style_the_right_cells_across_wrapped_rows() {
    let text = "Alpha *bravo* charlie delta echo foxtrot golf hotel india juliet \
                kilo lima mike november oscar papa quebec romeo papa sierra.\n";
    let at = |w: &str| text.find(w).unwrap();
    let mut app = app_with(text);
    app.buffer.cursor = crate::buffer::Cursor { line: 0, col: 0 };
    // Annotations: one over alpha..charlie, a long one charlie..oscar that wraps
    // across several rows (so later rows are tinted by a range that starts rows
    // earlier, after a shorter range has already ended), and one nested inside
    // the long one. Inserted out of order — the renderer must not rely on it.
    app.annotations = vec![
        note(at("charlie"), at("oscar") + 5),
        note(at("Alpha"), at("charlie") + 7),
        note(at("golf"), at("golf") + 4),
    ];
    app.spell_flags = vec![(at("kilo"), at("kilo") + 4)];
    app.proximity_repeats = vec![(at("hotel"), at("hotel") + 5)];
    // Find "papa": two matches, the first (after the cursor) is current.
    app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    for c in "papa".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let theme = app.theme;

    // Narrow enough that the paragraph wraps onto several rows.
    let buf = cells(&app, 30, 14);
    let style = |w: &str, nth: usize| buf[find(&buf, w, nth)].style();
    let row = |w: &str| find(&buf, w, 0).1;
    assert!(row("oscar") > row("charlie"), "the long note spans rows");

    // Emphasis: markers concealed (cursor outside the span), text italic.
    let screen: String = (0..14)
        .flat_map(|y| (0..30).map(move |x| (x, y)))
        .map(|p| buf[p].symbol().to_string())
        .collect();
    assert!(!screen.contains("*bravo*"), "markers concealed");
    assert!(style("bravo", 0).add_modifier.contains(Modifier::ITALIC));

    // Annotations: overlapping, long, and nested ranges all tint their cells.
    for w in [
        "Alpha", "bravo", "charlie", "golf", "juliet", "mike", "oscar",
    ] {
        assert_eq!(style(w, 0).bg, Some(theme.annotation), "{w} is annotated");
    }
    // The last cell of the long note is tinted; the space after it is not.
    let (ox, oy) = find(&buf, "oscar", 0);
    assert_eq!(buf[(ox + 4, oy)].style().bg, Some(theme.annotation));
    assert_ne!(buf[(ox + 5, oy)].style().bg, Some(theme.annotation));

    // Spell flag underlined, proximity hit dimmed, neighbors untouched.
    assert!(style("kilo", 0).add_modifier.contains(Modifier::UNDERLINED));
    assert!(!style("lima", 0).add_modifier.contains(Modifier::UNDERLINED));
    assert!(style("hotel", 0).add_modifier.contains(Modifier::DIM));
    assert!(!style("india", 0).add_modifier.contains(Modifier::DIM));

    // Search: the current match is bright, the other dim; non-matches plain.
    assert_eq!(style("papa", 0).bg, Some(theme.match_current));
    assert_eq!(style("papa", 1).bg, Some(theme.match_other));
    let (px, py) = find(&buf, "papa", 1);
    assert_eq!(buf[(px + 3, py)].style().bg, Some(theme.match_other));
    assert_ne!(buf[(px + 4, py)].style().bg, Some(theme.match_other));
    for bg in [theme.match_current, theme.match_other, theme.annotation] {
        assert_ne!(style("quebec", 0).bg, Some(bg), "quebec is unstyled");
    }
}

#[test]
fn status_bar_right_text_reaches_the_edge_with_multibyte_glyphs() {
    // The left side carries the save-state glyph (✓ / ●) and the right a `·`
    // separator — multi-byte in UTF-8 but one cell wide. Padding by byte length
    // left the right-hand count short of the edge by the extra bytes.
    let app = app_with("one two three\n");
    let (w, h) = (60, 6);
    let buf = cells(&app, w, h);
    let bar: Vec<String> = (0..w)
        .map(|x| buf[(x, h - 1)].symbol().to_string())
        .collect();
    let text = bar.concat();
    assert!(
        text.contains('✓') || text.contains('●'),
        "a multi-byte glyph is on the bar: {text:?}"
    );
    assert!(text.contains('·'), "a separator is on the bar: {text:?}");
    // The right-hand text is `… ln 2/2` followed by its two-space margin, so
    // the last non-blank cell is exactly two columns in from the edge.
    let last = bar.iter().rposition(|s| s.trim() != "").unwrap();
    assert_eq!(
        last,
        w as usize - 3,
        "right text ends at the edge: {text:?}"
    );
    assert!(text.trim_end().ends_with("ln 2/2"), "{text:?}");
}
