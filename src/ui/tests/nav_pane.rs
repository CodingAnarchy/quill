//! Outline / nav pane scrolling: with more chapters than the pane has rows, the
//! row that matters (the Nav selection, the focused outline heading, or the
//! active chapter with its nested outline) stays on screen, band headers and body
//! numbering survive being scrolled into the middle, and the rolled-up totals line
//! stays pinned at the foot.

use super::*;
use crate::frontmatter::Role;
use crate::mode::Mode;
use std::fmt::Write;

/// Screen size for these tests: 90 wide gives a 28-column pane (27 inside its
/// right border); 20 tall leaves 19 pane rows above the status bar.
const W: u16 = 90;
const H: u16 = 20;
const PANE_ROWS: usize = 19;

/// `n` chapters titled `Ch000`, `Ch001`, … (zero-padded so no title is a
/// substring of another), the active buffer holding `body`.
fn many_chapters(n: usize, body: &str) -> App {
    let titles: Vec<String> = (0..n).map(|i| format!("Ch{i:03}")).collect();
    let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
    let mut app = app_with_chapters(&refs, body);
    app.outline_visible = true;
    app
}

/// The pane's rows as plain text (the columns left of its border).
fn pane_rows(app: &App) -> Vec<String> {
    render(app, W, H)
        .lines()
        .take(PANE_ROWS)
        .map(|l| l.chars().take(27).collect())
        .collect()
}

/// The pane row holding `needle`, if it's on screen.
fn row_of(rows: &[String], needle: &str) -> Option<usize> {
    rows.iter().position(|r| r.contains(needle))
}

fn nav(selected: usize, filter: &str) -> Mode {
    Mode::Nav {
        selected,
        filter: filter.into(),
        typing: false,
    }
}

#[test]
fn the_active_chapter_and_its_outline_stay_visible_past_the_bottom() {
    let mut app = many_chapters(60, "## Ford\n\nThe river.\n\n## Bridge\n\nThe city.\n");
    app.project.set_active(45);
    let rows = pane_rows(&app);
    let pane = rows.join("\n");
    let active = row_of(&rows, "▸ Ch045").unwrap_or_else(|| panic!("active row:\n{pane}"));
    // The nested heading outline follows its chapter row.
    assert_eq!(row_of(&rows, "Ford"), Some(active + 1), "pane:\n{pane}");
    assert_eq!(row_of(&rows, "Bridge"), Some(active + 2), "pane:\n{pane}");
    // The totals line is pinned to the last row, under a blank separator.
    assert!(rows[PANE_ROWS - 1].contains("words"), "pane:\n{pane}");
    assert!(rows[PANE_ROWS - 2].trim().is_empty(), "pane:\n{pane}");
    // The book header and early chapters scrolled off the top.
    assert_eq!(row_of(&rows, "Ch000"), None, "pane:\n{pane}");
}

#[test]
fn the_nav_selection_scrolls_down_and_back_up_a_row_at_a_time() {
    let mut app = many_chapters(60, "body\n");
    // The list area is the pane less the blank + totals footer.
    let list_rows = PANE_ROWS - 2;
    let mut last_offset = 0;
    for sel in 0..60 {
        app.mode = nav(sel, "");
        let rows = pane_rows(&app);
        let pane = rows.join("\n");
        let at = row_of(&rows, &format!("Ch{sel:03}"))
            .unwrap_or_else(|| panic!("selection {sel} is visible:\n{pane}"));
        assert!(at < list_rows, "selection sits above the footer:\n{pane}");
        assert!(rows[PANE_ROWS - 1].contains("words"), "totals:\n{pane}");
        // Walking down, the view only moves once the selection reaches the
        // bottom edge — then by exactly one row per step.
        let offset = app.nav_scroll.get();
        assert!(offset == last_offset || offset == last_offset + 1);
        last_offset = offset;
    }
    assert!(last_offset > 0, "the list scrolled");
    // Back up one: the view holds still (the last chapter is still on screen).
    app.mode = nav(58, "");
    assert!(row_of(&pane_rows(&app), "Ch059").is_some());
    // And walking all the way back up keeps every selection in view, ending at
    // the top with the book header back on screen.
    for sel in (0..59).rev() {
        app.mode = nav(sel, "");
        let rows = pane_rows(&app);
        assert!(
            row_of(&rows, &format!("Ch{sel:03}")).is_some(),
            "selection {sel} is visible:\n{}",
            rows.join("\n")
        );
    }
    assert_eq!(app.nav_scroll.get(), 0);
    assert!(row_of(&pane_rows(&app), "untitled").is_some());
}

#[test]
fn bands_numbers_and_the_outline_render_when_scrolled_into_the_middle() {
    let mut app = many_chapters(54, "## Ford\n\nThe river.\n");
    let chapters = &mut app.project.manifest.chapters;
    chapters[0].role = Some(Role::Dedication);
    chapters[1].role = Some(Role::Prologue);
    chapters[52].role = Some(Role::Epilogue);
    chapters[53].role = Some(Role::Afterword);
    // Cached counts through the setter, which keeps the running total in step.
    app.project.set_chapter_words(0, 7);
    app.project.set_chapter_words(53, 5);
    app.project.set_chapter_words(10, 100);
    app.project.set_active(51); // the last body chapter: body number 50
                                // Browse to the last section, scrolling the list to its end: the active
                                // chapter's rows sit in the middle of the window, back matter below them.
    app.mode = nav(53, "");
    let rows = pane_rows(&app);
    let pane = rows.join("\n");

    let active = row_of(&rows, "▸ 50. Ch051").unwrap_or_else(|| panic!("active:\n{pane}"));
    assert_eq!(row_of(&rows, "Ford"), Some(active + 1), "pane:\n{pane}");
    assert!(row_of(&rows, "49. Ch050").is_some_and(|r| r + 1 == active));
    // Below the outline: a gap, the back-matter band header, a blank, its rows.
    assert!(rows[active + 2].trim().is_empty(), "gap:\n{pane}");
    assert!(rows[active + 3].contains("BACK MATTER"), "pane:\n{pane}");
    assert!(rows[active + 5].contains("Ch052 "), "pane:\n{pane}");
    assert!(rows[active + 5].contains("· Epilogue"), "pane:\n{pane}");
    assert!(rows[active + 6].contains("Ch053 "), "pane:\n{pane}");
    // The front matter is scrolled away; the totals keep sections apart.
    assert_eq!(row_of(&rows, "FRONT MATTER"), None, "pane:\n{pane}");
    let body = 100 + app.buffer.word_count();
    assert!(
        rows[PANE_ROWS - 1].contains(&format!("{body} words · 12 in sections")),
        "totals:\n{pane}"
    );
}

#[test]
fn the_focused_outline_keeps_its_selected_heading_visible() {
    let mut body = String::new();
    for i in 0..40 {
        let _ = write!(body, "## Scene {i:02}\n\nText.\n\n");
    }
    let mut app = many_chapters(3, &body);
    app.mode = Mode::Outline {
        selected: 35,
        filter: String::new(),
        typing: false,
    };
    let rows = pane_rows(&app);
    assert!(
        row_of(&rows, "Scene 35").is_some(),
        "selected heading:\n{}",
        rows.join("\n")
    );

    // Filtered (#822): the matching heading and the filter line both show.
    app.mode = Mode::Outline {
        selected: 35,
        filter: "35".into(),
        typing: true,
    };
    let rows = pane_rows(&app);
    let pane = rows.join("\n");
    assert!(row_of(&rows, "Scene 35").is_some(), "pane:\n{pane}");
    assert_eq!(row_of(&rows, "Scene 34"), None, "pane:\n{pane}");
    assert!(row_of(&rows, "filter: 35_").is_some(), "pane:\n{pane}");
}

#[test]
fn a_nav_filter_scrolls_to_its_selection_among_the_matches() {
    let mut app = many_chapters(200, "body\n");
    // "ch1" matches Ch100–Ch199 plus Ch010–Ch019: more than fit on screen.
    app.mode = nav(190, "ch1");
    let rows = pane_rows(&app);
    let pane = rows.join("\n");
    assert!(row_of(&rows, "Ch190").is_some(), "selection:\n{pane}");
    assert_eq!(row_of(&rows, "Ch020"), None, "non-matches hidden:\n{pane}");
    assert!(rows[PANE_ROWS - 1].contains("words"), "totals:\n{pane}");
}

#[test]
fn a_short_list_keeps_the_totals_right_under_it() {
    let app = many_chapters(3, "alpha beta\n");
    let rows = pane_rows(&app);
    let pane = rows.join("\n");
    let last = row_of(&rows, "Ch002").unwrap_or_else(|| panic!("pane:\n{pane}"));
    assert!(rows[last + 1].trim().is_empty(), "pane:\n{pane}");
    assert!(rows[last + 2].contains("2 words"), "pane:\n{pane}");
}

#[test]
fn the_active_chapters_whole_outline_comes_into_view_when_it_fits() {
    // The cursor sits on the chapter's own H1 (left out of the outline as a
    // repeat of its title), so there's no current heading row to chase — the
    // nested scenes must still scroll into view under the chapter row.
    let mut app = many_chapters(
        60,
        "# Ch045\n\n## Ford\n\nThe river.\n\n## Bridge\n\nThe city.\n",
    );
    app.project.set_active(45);
    let rows = pane_rows(&app);
    let pane = rows.join("\n");
    let active = row_of(&rows, "▸ Ch045").unwrap_or_else(|| panic!("active row:\n{pane}"));
    assert_eq!(row_of(&rows, "Ford"), Some(active + 1), "pane:\n{pane}");
    assert_eq!(row_of(&rows, "Bridge"), Some(active + 2), "pane:\n{pane}");
}
