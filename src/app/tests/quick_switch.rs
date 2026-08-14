//! Tests for the fuzzy chapter/scene quick-switcher (#976): the index lists every
//! chapter and scene heading, typing narrows it, Enter jumps to the right target
//! (reusing the go-to + jumplist path), and the per-keystroke filter is independent
//! of project size (a `cargo perf` guard).

use super::*;
use crate::app::quick_switch::scene_headings;
use crate::mode::{Mode, QuickSwitch};
use crate::project::Project;
use crate::syntax::Syntax;
use std::fs;

/// A two-chapter project whose chapters each carry scene headings, opened and
/// pinned like the other fixtures. Chapter one (`one.md`) starts active.
fn scenes_app(name: &str, one: &str, two: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"Chapter One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Chapter Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), one).unwrap();
    fs::write(dir.join("two.md"), two).unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

/// The visible (filtered) labels of the open switcher, in order.
fn visible_titles(app: &App) -> Vec<String> {
    let Mode::QuickSwitch(s) = &app.mode else {
        panic!("quick-switcher should be open");
    };
    s.matches().iter().map(|(_, t)| t.title.clone()).collect()
}

#[test]
fn ctrl_d_opens_the_switcher_listing_chapters_and_scene_headings() {
    let (mut app, _dir) = scenes_app(
        "qs-open",
        "# Chapter One\n\n## The Arrival\n\nRain.\n\n## The Meeting\n\nShe waited.\n",
        "# Chapter Two\n\n## The Departure\n\nSmoke.\n",
    );
    app.on_key(ctrl('d'));

    // Every chapter, each followed by its scene headings, in project order.
    assert_eq!(
        visible_titles(&app),
        vec![
            "Chapter One",
            "The Arrival",
            "The Meeting",
            "Chapter Two",
            "The Departure",
        ]
    );
    // The `#` chapter-title headings are not listed as scenes (only level ≥ 2).
    let Mode::QuickSwitch(s) = &app.mode else {
        panic!("open");
    };
    assert_eq!(s.targets.len(), 5, "2 chapters + 3 scene headings");
}

#[test]
fn typing_filters_chapters_and_scene_headings_by_substring() {
    let (mut app, _dir) = scenes_app(
        "qs-filter",
        "# Chapter One\n\n## The Arrival\n\nRain.\n\n## The Meeting\n\nShe waited.\n",
        "# Chapter Two\n\n## The Departure\n\nSmoke.\n",
    );
    app.on_key(ctrl('d'));

    // A scene-heading substring narrows to matching scenes across chapters.
    type_str(&mut app, "the ");
    assert_eq!(
        visible_titles(&app),
        vec!["The Arrival", "The Meeting", "The Departure"]
    );

    // Backspacing widens again; a chapter-title substring matches the chapter row.
    for _ in 0..4 {
        app.on_key(key(KeyCode::Backspace));
    }
    // "two" matches the "Chapter Two" row and its scene (whose haystack carries
    // the chapter title), so the chapter and its scenes surface together.
    type_str(&mut app, "two");
    assert_eq!(visible_titles(&app), vec!["Chapter Two", "The Departure"]);
}

#[test]
fn scene_haystack_includes_the_chapter_so_a_chapter_query_surfaces_its_scenes() {
    // Filtering by a chapter title also surfaces that chapter's scenes, since a
    // scene's match text carries its chapter (#976).
    let (mut app, _dir) = scenes_app(
        "qs-haystack",
        "# Chapter One\n\n## The Arrival\n\nRain.\n",
        "# Chapter Two\n\n## The Departure\n\nSmoke.\n",
    );
    app.on_key(ctrl('d'));
    type_str(&mut app, "one");
    assert_eq!(visible_titles(&app), vec!["Chapter One", "The Arrival"]);
}

#[test]
fn enter_jumps_to_a_scene_in_another_chapter_and_records_the_jump() {
    let two_body = "# Chapter Two\n\n## The Departure\n\nSmoke.\n";
    let (mut app, _dir) = scenes_app(
        "qs-jump-scene",
        "# Chapter One\n\n## The Arrival\n\nRain.\n",
        two_body,
    );
    assert_eq!(app.project.active_index(), 0);
    let before = app.vim_state.jumplist.len();

    app.on_key(ctrl('d'));
    // Filter to the chapter-two scene, then jump.
    type_str(&mut app, "departure");
    app.on_key(key(KeyCode::Enter));

    // The switcher closed, chapter two is active, and the cursor sits on its
    // "## The Departure" heading.
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.project.active_index(), 1);
    let expected = two_body.find("## The Departure").unwrap();
    assert_eq!(app.edit_buffer().cursor_byte(), expected);
    // The jump was recorded so `` `` `` can return here.
    assert_eq!(app.vim_state.jumplist.len(), before + 1);
}

#[test]
fn enter_on_a_chapter_row_jumps_to_the_chapter_top() {
    let (mut app, _dir) = scenes_app(
        "qs-jump-chapter",
        "# Chapter One\n\n## The Arrival\n\nRain.\n",
        "# Chapter Two\n\n## The Departure\n\nSmoke.\n",
    );
    app.on_key(ctrl('d'));
    // Unfiltered order: [Chapter One, The Arrival, Chapter Two, The Departure].
    // Move down twice to the "Chapter Two" row and jump.
    for _ in 0..2 {
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.project.active_index(), 1);
    assert_eq!(app.edit_buffer().cursor_byte(), 0, "chapter top");
}

#[test]
fn esc_closes_without_jumping() {
    let (mut app, _dir) = scenes_app(
        "qs-esc",
        "# Chapter One\n\n## The Arrival\n\nRain.\n",
        "# Chapter Two\n\n## The Departure\n\nSmoke.\n",
    );
    app.on_key(ctrl('d'));
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.project.active_index(), 0, "no jump on cancel");
}

#[test]
fn enter_with_no_match_closes_without_moving() {
    let (mut app, _dir) = scenes_app(
        "qs-nomatch",
        "# Chapter One\n\n## The Arrival\n\nRain.\n",
        "# Chapter Two\n\n## The Departure\n\nSmoke.\n",
    );
    app.on_key(ctrl('d'));
    type_str(&mut app, "zzzznope");
    assert!(visible_titles(&app).is_empty());
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.project.active_index(), 0);
}

#[test]
fn selection_resets_to_the_top_as_the_query_narrows() {
    let (mut app, _dir) = scenes_app(
        "qs-sel-reset",
        "# Chapter One\n\n## Alpha\n\nA.\n\n## Beta\n\nB.\n",
        "# Chapter Two\n\n## Gamma\n\nG.\n",
    );
    app.on_key(ctrl('d'));
    // Move the selection down, then type — the new filter re-anchors at row 0.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    type_str(&mut app, "gamma");
    let Mode::QuickSwitch(s) = &app.mode else {
        panic!("open");
    };
    assert_eq!(s.selected, 0);
    assert_eq!(s.current().map(|t| t.title.as_str()), Some("Gamma"));
}

#[test]
fn the_lightweight_heading_scan_matches_the_full_parse() {
    // Bug J: the switcher's off-buffer chapters extract headings without a full
    // tree-sitter parse when the body is plain ATX markdown. The fast path must be
    // byte-for-byte identical to `Syntax::new(..).headings()`, and any body it can't
    // safely handle (setext underlines, `---` scene breaks) must fall back to the
    // parse — so the switcher's targets never drift. Cover a spread of shapes.
    let bodies = [
        // Plain ATX (the fast path).
        "# Chapter\n\n## Scene one\n\nRain.\n\n## Scene two\n\nSnow.\n",
        "## a\n### b\n#### c\n",
        // Leading spaces (0–3 allowed), trailing hashes, indent > 3 (not a heading).
        "   ## Indented\n\n#### Deep ####\n\n    #### too indented\n",
        // `#` without a following space is not a heading; 7 hashes isn't either.
        "##NoSpace\n\n####### Seven\n\n#\n",
        // Headings inside a fenced code block must be ignored.
        "```\n## in fence\n```\n\n## real\n",
        "~~~rust\n## tilde fence\n~~~\n\n## after\n",
        // Setext + scene-break bodies force the fallback — must still match the parse.
        "Setext Title\n---\n\n## After\n",
        "Intro\n===\n\n## Scene\n",
        "## Scene\n\nBody.\n\n---\n\n## Next\n",
        // Empty and heading-free bodies.
        "",
        "Just prose with no headings at all.\n",
    ];
    for body in bodies {
        let fast = scene_headings(body);
        let parsed = Syntax::new(body).headings().to_vec();
        assert_eq!(
            fast, parsed,
            "the heading scan must match the full parse for body {body:?}"
        );
    }
}

#[test]
fn active_chapter_scenes_come_from_disk_when_a_reference_pane_is_focused() {
    // With a story-bible entry opened in a pane and focused (#165), the focused
    // `edit_buffer()` is the bible, not the active chapter — so the switcher must
    // read the active chapter's scene headings from disk, not lift the bible's
    // headings into the chapter's jump-targets.
    let dir = app_scratch_dir("qs-ref-pane-focus");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"Chapter One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Chapter Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("one.md"),
        "# Chapter One\n\n## The Arrival\n\nRain.\n",
    )
    .unwrap();
    fs::write(
        dir.join("two.md"),
        "# Chapter Two\n\n## The Departure\n\nSmoke.\n",
    )
    .unwrap();
    // A bible entry whose own headings must NOT leak into a chapter's targets.
    fs::create_dir_all(dir.join("bible")).unwrap();
    fs::write(
        dir.join("bible").join("mara.md"),
        "# Mara\n\n## Physical\n\ntall\n\n## Voice\n\nlow\n",
    )
    .unwrap();
    let dir = TempProject::new(dir);
    let mut app = open_test_app(&dir);

    // Open the bible entry in a pane and focus it (Ctrl-w w), the state where
    // `edit_buffer()` is the bible while chapter one stays active.
    app.on_key(ctrl('g')); // chapter browser
    app.on_key(key(KeyCode::Char('f'))); // reference files
    app.on_key(key(KeyCode::Enter)); // open the (only) bible entry in a pane
    app.on_key(ctrl('w'));
    app.on_key(key(KeyCode::Char('w'))); // focus the bible pane
    assert!(
        !app.focus_shows_active_chapter(),
        "the focused pane is the bible, not the active chapter"
    );
    assert_eq!(app.project.active_index(), 0, "chapter one stays active");

    app.on_key(ctrl('d')); // open the quick-switcher

    // The active chapter's scene target is its own heading (read from disk), and
    // the focused bible's headings never appear as scene targets.
    let titles = visible_titles(&app);
    assert!(
        titles.contains(&"The Arrival".to_string()),
        "the active chapter's own scene is listed: {titles:?}"
    );
    assert!(
        !titles.iter().any(|t| t == "Physical" || t == "Voice"),
        "the focused bible's headings must not appear as scene targets: {titles:?}"
    );
    assert_eq!(
        visible_titles(&app),
        vec!["Chapter One", "The Arrival", "Chapter Two", "The Departure",],
        "targets are the chapters and their own scenes only"
    );
}

/// Performance guard (`cargo perf`). The per-keystroke work is filtering the
/// pre-built index, which must not grow with the project: the index is captured
/// once when the switcher opens, so a keystroke only re-scans it in memory — no
/// file reads. Filtering a series-sized index (2000 chapters × several scenes)
/// stays well within a frame. See `docs/performance.md`.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn filtering_is_independent_of_project_size() {
    use crate::mode::QuickTarget;
    use std::time::Instant;

    use crate::testutil::perf_budget_ms;

    // Build a series-sized index directly (the same shape the switcher captures on
    // open), so the guard times the per-keystroke filter, not the one-time build.
    let n_chapters = 2000;
    let scenes_per = 4;
    let mut targets = Vec::new();
    for ch in 0..n_chapters {
        let title = format!("Chapter {ch}");
        targets.push(QuickTarget {
            chapter: ch,
            byte: 0,
            title: title.clone(),
            chapter_title: String::new(),
            haystack: title.to_lowercase(),
            is_scene: false,
        });
        for s in 0..scenes_per {
            let heading = format!("Scene {ch}-{s} the arrival at dawn");
            targets.push(QuickTarget {
                chapter: ch,
                byte: 1 + s,
                haystack: format!("{heading} {title}").to_lowercase(),
                title: heading,
                chapter_title: title.clone(),
                is_scene: true,
            });
        }
    }
    let total = targets.len();
    let mut switch = QuickSwitch::new(targets);
    switch.query = "arrival".to_string();

    // Warm, then time the filter over the whole index.
    let _ = switch.matches();
    let iters = 200;
    let t = Instant::now();
    let mut hits = 0;
    for _ in 0..iters {
        hits = switch.matches().len();
    }
    let per = t.elapsed() / iters;

    println!("quick-switch filter over {total} targets = {per:?}");
    assert!(hits > 0, "the query should match the seeded scenes");
    // A pure in-memory substring scan of a series-sized index is sub-millisecond;
    // a regression to per-keystroke file reads would blow far past this.
    assert!(
        per.as_millis() < perf_budget_ms(5),
        "quick-switch filter took {per:?}"
    );
}

/// Performance guard (`cargo perf`). Opening the switcher builds its index over the
/// whole project (#976 bug J): one heading extraction per chapter. That extraction
/// must stay cheap — a full tree-sitter parse (block *and* inline emphasis trees)
/// per chapter stalls the Ctrl-D keystroke on a series. The lightweight ATX scan
/// keeps the open near the cost of the file reads alone, so opening scales linearly
/// and stays well within an interactive budget even on a series-sized project. If a
/// change reintroduces a per-chapter full parse on open, this catches it. See
/// `docs/performance.md`.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn opening_the_switcher_is_fast_in_a_series_sized_project() {
    use std::time::Instant;

    use crate::testutil::perf_budget_ms;

    // A series' worth of chapters, each with a handful of ATX scene headings and a
    // realistic body — the switcher reads and scans every one on open.
    let n = 600;
    let dir = crate::testutil::scratch_dir("perf", "qs-open");
    let dir = TempProject::new(dir);
    let mut manifest = String::from("title = \"Series\"\n\n");
    for i in 0..n {
        let file = format!("ch{i}.md");
        write!(
            manifest,
            "[[chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"
        )
        .unwrap();
        let mut body = format!("# Chapter {i}\n\n");
        for s in 0..4 {
            write!(body, "## Scene {s} the arrival at dawn\n\n").unwrap();
            for _ in 0..10 {
                body.push_str("The night was dark and the road was very long indeed.\n");
            }
            body.push('\n');
        }
        fs::write(dir.join(&file), body).unwrap();
    }
    fs::write(Project::manifest_path(&dir), manifest).unwrap();

    let mut app = open_test_app(&dir);

    // One untimed open pays one-off allocation / CPU-ramp costs.
    app.open_quick_switch();
    app.mode = Mode::Insert;

    let opens = 20;
    let t = Instant::now();
    for _ in 0..opens {
        app.open_quick_switch();
        // The index is captured; close it so the next iteration rebuilds from disk.
        app.mode = Mode::Insert;
    }
    let per = t.elapsed() / opens;

    println!("quick-switch open over a {n}-chapter project = {per:?}");
    // Sanity: the index really was built for every chapter + its scenes.
    app.open_quick_switch();
    let Mode::QuickSwitch(s) = &app.mode else {
        panic!("open");
    };
    assert!(
        s.targets.len() >= n * 5,
        "every chapter and its scenes are indexed"
    );
    // Reading + lightweight-scanning 600 chapters stays comfortably interactive; a
    // regression to a full tree-sitter parse per chapter blows well past this.
    assert!(
        per.as_millis() < perf_budget_ms(60),
        "opening the switcher took {per:?} over {n} chapters"
    );
}
