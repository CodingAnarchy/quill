//! Outline/nav pane, epigraph, chapter-badge, continuous-view, and corkboard
//! render tests. Pure move (#792).

use super::*;
use crate::manuscript::Manuscript;
use crate::syntax::Heading;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fs;

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

    let texts: Vec<String> = heading_outline_lines(&headings, None, None, skip, "")
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("At the ford"));
    assert!(!texts[0].contains("The Crossing"));

    // A non-matching title keeps every heading.
    assert_eq!(redundant_title_heading(&headings, "Different"), None);
    assert_eq!(
        heading_outline_lines(&headings, None, None, None, "").len(),
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
fn header_blocks_are_cached_across_frames_and_invalidate_on_change() {
    // The display-only header blocks are built once and reused per frame (#869):
    // an unchanged frame reuses the same allocation, and the render output is
    // byte-identical. Changing an input (the epigraph, the width) rebuilds them.
    let mut app = app_with("# Chapter\n\nBody.\n");
    app.buffer
        .front_matter_mut()
        .set_epigraph_text("First quote.");
    app.buffer.front_matter_mut().set_epigraph_by("A");

    let first = render(&app, 80, 20);
    let id1 = app.header_cache.borrow().identity();

    // A second frame with nothing changed reuses the cached blocks and renders
    // identically — no rebuild, no drift.
    let second = render(&app, 80, 20);
    let id2 = app.header_cache.borrow().identity();
    assert_eq!(first, second, "unchanged frame renders identically");
    assert_eq!(id1, id2, "unchanged frame reuses the cached header blocks");

    // Editing the epigraph invalidates the cache (different blocks, and the new
    // quote shows on screen).
    app.buffer
        .front_matter_mut()
        .set_epigraph_text("Second quote.");
    let changed = render(&app, 80, 20);
    let id3 = app.header_cache.borrow().identity();
    assert_ne!(id2, id3, "an edited epigraph rebuilds the header blocks");
    assert!(changed.contains("Second quote."), "screen:\n{changed}");
    assert!(!changed.contains("First quote."), "screen:\n{changed}");

    // A resize also invalidates the cache (the epigraph wraps to the new width).
    render(&app, 60, 20);
    let id4 = app.header_cache.borrow().identity();
    assert_ne!(id3, id4, "a resize rebuilds the header blocks");
}

#[test]
fn header_and_scene_break_inserts_render_identically_across_frames() {
    // A chapter with both an epigraph header (#518) and a scene break (#908): the
    // merge of the cached header blocks with the scene-break rows must produce the
    // exact same frame each draw, and it must not disturb the cached header `Rc`
    // (which is reused by refcount, never deep-cloned per frame in `merge_inserts`).
    let mut app = app_with("# Chapter\n\nBefore the break.\n\n---\n\nAfter the break.\n");
    app.buffer.front_matter_mut().set_epigraph_text("A quote.");
    app.buffer.front_matter_mut().set_epigraph_by("Author");

    let first = render(&app, 80, 24);
    let id1 = app.header_cache.borrow().identity();
    let second = render(&app, 80, 24);
    let id2 = app.header_cache.borrow().identity();

    assert!(first.contains("A quote."), "epigraph shows:\n{first}");
    assert!(first.contains("— Author"), "attribution shows:\n{first}");
    assert!(first.contains("Before the break."), "body:\n{first}");
    assert!(first.contains("After the break."), "body:\n{first}");
    assert!(
        first.contains("* * *"),
        "the scene-break divider renders:\n{first}"
    );
    assert_eq!(
        first, second,
        "the header + scene-break render is byte-identical across frames"
    );
    assert_eq!(
        id1, id2,
        "the cached header blocks are reused across frames even with a scene break"
    );
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
