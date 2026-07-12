//! The user's-guide searchable section-jump (#1108): the 400+-line F1 guide is a
//! long linear scroll, so `/` over it opens a filterable list of section headers
//! that jumps straight to a topic. These tests prove the guide's sections parse
//! out of the rendered lines, that a keyword filter narrows them, and that
//! selecting one scrolls the overlay to that section — so "how do I export?" is a
//! filter-and-Enter, not a scroll through the whole guide.

use super::*;
use crate::mode::GuideNav;

/// Open the guide overlay through the real F1 path, so the test drives the same
/// overlay the author sees.
fn guide_app(tag: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(tag);
    app.on_key(key(KeyCode::F(1)));
    (app, dir)
}

/// The guide's ALL-CAPS section banners parse into a jump list, in document order,
/// each carrying the row it starts on. The EXPORT section — the ticket's motivating
/// "how do I export EPUB?" — is among them, and every section's recorded line
/// really holds its header.
#[test]
fn guide_sections_parse_from_the_rendered_lines() {
    let (app, _dir) = guide_app("guide-sections-parse");
    let overlay = app.overlay.as_ref().expect("F1 opened the guide");
    let sections = crate::app::reports::guide_sections(&overlay.lines);
    assert!(
        sections.len() >= 8,
        "the guide has several sections, got {}",
        sections.len()
    );
    // The recorded line for each section is its header row.
    for section in &sections {
        let row: String = overlay.lines[section.line]
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(row.trim(), section.label, "line points at the header");
    }
    // Sections are in document order (their lines strictly increase).
    for pair in sections.windows(2) {
        assert!(pair[0].line < pair[1].line, "sections in document order");
    }
    assert!(
        sections.iter().any(|s| s.label == "EXPORT"),
        "the EXPORT section is jumpable"
    );
}

/// A mid-paragraph all-caps word (a keybinding token, an emphatic phrase) isn't a
/// banner: only a lone all-caps line with a blank line above it counts, so the jump
/// list is the real sections — not every shout in the prose.
#[test]
fn only_banner_lines_become_sections() {
    let lines = ["WRITING", "", "Type CTRL-Z to undo.", "", "EXPORT", "Body."]
        .iter()
        .map(|l| ratatui::text::Line::raw((*l).to_string()))
        .collect::<Vec<_>>();
    let sections = crate::app::reports::guide_sections(&lines);
    let labels: Vec<&str> = sections.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(
        labels,
        ["WRITING", "EXPORT"],
        "CTRL-Z mid-prose isn't a section"
    );
}

/// `/` over the guide opens the section-jump; a keyword filter narrows the visible
/// sections to the matching one, and Enter scrolls the guide to that section's line
/// — the core "find a topic without scrolling" acceptance.
#[test]
fn filtering_and_jumping_scrolls_to_the_section() {
    let (mut app, _dir) = guide_app("guide-jump-export");
    // `/` opens the jump over the guide (the scroll starts at the top).
    assert_eq!(app.overlay_scroll(), 0);
    app.on_key(key(KeyCode::Char('/')));
    assert!(
        app.overlay.as_ref().and_then(|o| o.nav.as_ref()).is_some(),
        "'/' opened the section jump"
    );
    // Type "export": the filter narrows to the EXPORT section.
    for c in "export".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let nav = app.overlay.as_ref().and_then(|o| o.nav.as_ref()).unwrap();
    let matches = nav.matches();
    assert_eq!(matches.len(), 1, "the filter narrows to one section");
    assert_eq!(matches[0].1.label, "EXPORT");
    let target = matches[0].1.line;
    // Enter jumps: the jump closes and the guide scrolls to the EXPORT header.
    app.on_key(key(KeyCode::Enter));
    assert!(
        app.overlay.as_ref().and_then(|o| o.nav.as_ref()).is_none(),
        "Enter closed the jump"
    );
    assert_eq!(
        app.overlay_scroll() as usize,
        target,
        "the guide scrolled to the EXPORT section"
    );
    // The row now at the top of the scroll is the EXPORT header.
    let lines = app.overlay_lines().unwrap();
    let row: String = lines[target]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(row.trim(), "EXPORT");
}

/// Esc closes the jump without moving: the guide stays where it was, so opening the
/// jump and backing out is free.
#[test]
fn esc_closes_the_jump_keeping_the_scroll() {
    let (mut app, _dir) = guide_app("guide-jump-esc");
    app.on_key(key(KeyCode::Char('/')));
    for c in "chapters".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Esc));
    assert!(
        app.overlay.as_ref().and_then(|o| o.nav.as_ref()).is_none(),
        "Esc closed the jump"
    );
    assert_eq!(app.overlay_scroll(), 0, "the guide didn't move");
    assert!(app.overlay.is_some(), "the guide overlay is still open");
}

/// `GuideNav` filters case-insensitively on the header and reports the highlighted
/// section's target line — the shared logic the renderer and jump both read.
#[test]
fn guide_nav_matches_and_selected_line() {
    let nav = GuideNav {
        sections: vec![
            crate::mode::GuideSection {
                label: "WRITING".into(),
                line: 9,
            },
            crate::mode::GuideSection {
                label: "EXPORT".into(),
                line: 387,
            },
        ],
        filter: "expo".into(),
        selected: 0,
    };
    let matches = nav.matches();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].1.label, "EXPORT");
    assert_eq!(nav.selected_line(), Some(387));
    // An empty filter shows every section; a non-matching one shows none.
    let all = GuideNav {
        filter: String::new(),
        ..nav.clone()
    };
    assert_eq!(all.matches().len(), 2);
    let none = GuideNav {
        filter: "zzz".into(),
        ..nav
    };
    assert!(none.matches().is_empty());
    assert_eq!(none.selected_line(), None);
}
