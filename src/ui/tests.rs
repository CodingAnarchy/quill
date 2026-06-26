//! Tests for the `src/ui/mod.rs` module, split out to keep the module under the
//! ~1,000-line convention (#404). Pure move — no behavior change.

use super::*;
use super::render_to_string as render;
use crate::buffer::{Buffer, Cursor};
use crate::manuscript::Manuscript;
use crate::project::Project;
use crate::syntax::Heading;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use ratatui::{backend::TestBackend, Terminal};
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
#[ignore]
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
    // A spread of inline annotations (#302) so the per-row/char annotation
    // scan is exercised under the frame budget too.
    app.annotations = (0..50)
        .map(|i| crate::annotation::Live { chapter: "7B".into(), start: i * 53, end: i * 53 + 5, note: "note".into() })
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
    let wide = Rect { x: 0, y: 0, width: 100, height: 30 };
    let thin = Rect { x: 0, y: 0, width: 50, height: 30 };
    let short = Rect { x: 0, y: 0, width: 100, height: 8 };
    assert!(!too_cramped_to_split(SplitDir::SideBySide, wide));
    assert!(too_cramped_to_split(SplitDir::SideBySide, thin), "two columns need room");
    assert!(!too_cramped_to_split(SplitDir::Stacked, wide));
    assert!(too_cramped_to_split(SplitDir::Stacked, short), "two rows need room");
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
        assert!(row.trim_end().chars().count() <= 90, "row stays within the column: {row:?}");
    }
}

#[test]
fn vim_mode_is_shown_in_the_status_bar() {
    let mut app = app_with("first chapter\n");
    // Off: no mode chip.
    assert!(!render(&app, 80, 8).contains("NORMAL"), "no chip when Vim is off");
    // On, resting in Normal.
    app.vim = true;
    app.mode = Mode::Normal;
    assert!(render(&app, 80, 8).contains("NORMAL"), "Normal shown");
    // Entering Insert updates the indicator.
    app.mode = Mode::Insert;
    let screen = render(&app, 80, 8);
    assert!(screen.contains("INSERT") && !screen.contains("NORMAL"), "Insert shown:\n{screen}");
    // Visual modes show their own labels.
    app.mode = Mode::Visual { anchor: 0, linewise: false };
    assert!(render(&app, 80, 8).contains("VISUAL"), "Visual shown");
    app.mode = Mode::Visual { anchor: 0, linewise: true };
    assert!(render(&app, 80, 8).contains("VISUAL LINE"), "Visual Line shown");
}

#[test]
fn visual_selection_draws_a_highlight() {
    let mut app = app_with("hello world\n");
    app.vim = true;
    // Select the first few graphemes (anchor at 0, cursor at col 2).
    app.mode = Mode::Visual { anchor: 0, linewise: false };
    app.buffer.cursor = Cursor { line: 0, col: 2 };
    let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
    terminal.draw(|f| draw(f, &app)).unwrap();
    let buf = terminal.backend().buffer();
    let highlighted = (0..40).any(|x| (0..6).any(|y| buf[(x, y)].style().bg == Some(Color::Yellow)));
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
        text.push_str(&format!("Line{i:02}"));
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

    assert!(on < off, "typewriter lifts the last line off the bottom: on={on} off={off}");
    assert!((8..=12).contains(&on), "pinned near the centre of a 20-row page: {on}");
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
            .find(|&y| (0..40).any(|x| buf[(x, y)].symbol() == "a" || buf[(x, y)].symbol() == "b") && {
                let line: String = (0..40).map(|x| buf[(x, y)].symbol()).collect();
                line.contains(needle)
            })
            .expect("paragraph on screen");
        (0..40)
            .map(|x| &buf[(x, row)])
            .find(|c| c.symbol().chars().next().is_some_and(|ch| ch.is_alphabetic()))
            .unwrap()
            .style()
            .fg
    };

    // Off (default): both paragraphs render in the normal prose colour.
    assert_ne!(first_letter_fg(&app, "alpha"), Some(dim), "no dimming when focus mode is off");

    // On: the other paragraph dims; the one under the cursor stays normal.
    app.focus_mode = true;
    assert_eq!(first_letter_fg(&app, "alpha"), Some(dim), "the other paragraph dims");
    assert_ne!(first_letter_fg(&app, "bravo"), Some(dim), "the focused paragraph stays normal");
}

#[test]
fn the_model_manager_lists_the_catalog_with_characteristics() {
    let mut app = app_with("text\n");
    app.mode = Mode::ModelManager { selected: 0 };
    let screen = render(&app, 100, 24);
    assert!(screen.contains("Local AI models"), "{screen}");
    // Models appear with their context window and description.
    assert!(screen.contains("Qwen2.5-7B"), "the catalog is listed:\n{screen}");
    assert!(screen.contains("1M ctx"), "the extended-context model's window shows:\n{screen}");
    assert!(screen.contains("Long context"), "descriptions are shown:\n{screen}");
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
    assert!(matches!(app.mode, Mode::Palette(_)), "Ctrl-K opened the palette");
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
    app.annotations = vec![crate::annotation::Live { chapter: "one.md".into(), start: 0, end: 5, note: "check".into() }];

    let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
    terminal.draw(|f| draw(f, &app)).unwrap();
    let buf = terminal.backend().buffer().clone();

    let marked = (0..40).any(|x| (0..8).any(|y| buf[(x, y)].style().bg == Some(anno_bg)));
    assert!(marked, "the annotated span carries the annotation background");
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
        Heading { level: 1, title: "A".into(), start: 0 },
        Heading { level: 2, title: "B".into(), start: 50 },
    ];
    assert_eq!(current_section(&hs, 0), Some(0));
    assert_eq!(current_section(&hs, 49), Some(0));
    assert_eq!(current_section(&hs, 50), Some(1));
    assert_eq!(current_section(&hs, 999), Some(1));

    // Cursor above the first heading, and the no-headings case.
    let later = vec![Heading { level: 1, title: "A".into(), start: 10 }];
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
        Heading { level: 1, title: "The Crossing".into(), start: 0 },
        Heading { level: 2, title: "At the ford".into(), start: 30 },
    ];

    // The chapter is titled "The Crossing", so its opening H1 is redundant.
    let skip = redundant_title_heading(&headings, "The Crossing");
    assert_eq!(skip, Some(0));

    let texts: Vec<String> = heading_outline_lines(&headings, None, skip)
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("At the ford"));
    assert!(!texts[0].contains("The Crossing"));

    // A non-matching title keeps every heading.
    assert_eq!(redundant_title_heading(&headings, "Different"), None);
    assert_eq!(
        heading_outline_lines(&headings, None, None).len(),
        2
    );
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
    assert!(screen.contains("3 / 10"), "progress toward the goal:\n{screen}");

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
    assert!(screen.contains("In the beginning."), "book epigraph:\n{screen}");
    assert!(screen.contains("— Genesis"), "book attribution:\n{screen}");
    assert!(screen.contains("A chapter quote."), "chapter epigraph:\n{screen}");
    assert!(screen.contains("— Someone"), "chapter attribution:\n{screen}");
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
    assert!(red.is_some() && blue.is_some() && by.is_some(), "screen:\n{screen}");
    assert!(red < blue && blue < by, "lines out of order:\n{screen}");
}

#[test]
fn editing_an_epigraph_shows_a_banner() {
    let mut app = app_with("Body.\n");
    app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)); // → Nav
    app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)); // → epigraph editor
    assert!(matches!(app.mode, Mode::EditMeta(_)));

    let screen = render(&app, 80, 10);
    assert!(screen.contains("editing chapter epigraph"), "screen:\n{screen}");
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
    app.mode = Mode::Nav { selected: 1 };
    let screen = render(&app, 90, 14);
    assert!(screen.contains("A grand opening"), "screen:\n{screen}");
    assert!(screen.contains("POV: Mara"), "screen:\n{screen}");
    assert!(screen.contains("Revised"), "screen:\n{screen}");
}

#[test]
fn continuous_view_shows_chapters_flowing_with_a_divider() {

    // A two-chapter project on disk (the manuscript window reads files).
    let mut dir = std::env::temp_dir();
    dir.push(format!("quill-ui-cont-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
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
    let mut dir = std::env::temp_dir();
    dir.push(format!("quill-ui-parts-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
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
    assert!(part_two < divider_two, "the part header sits above its divider:\n{screen}");
    // It's display-only: the part name never entered the editable buffer.
    let flow = app.manuscript.as_ref().unwrap().buffer();
    assert!(
        !flow.line_range_text(0, flow.line_count()).to_uppercase().contains("PART"),
        "the structure header leaked into the buffer",
    );

    fs::remove_dir_all(&dir).ok();
}
