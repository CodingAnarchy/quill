//! Switching the active book of a multi-book series (#329): the editor, chapter
//! list, and daily tally follow the active book, pending edits are flushed on
//! the way out, and a standalone novel has no switcher.

use super::*;
use crate::project::Project;

/// A two-book series on disk, opened on book one, with the deterministic test
/// pins the shared helper applies (Vim / smart typography / workflows off).
fn series_app(name: &str) -> (App, TempProject) {
    let dir = crate::testutil::scratch_dir("books", name);
    for sub in ["b1", "b2"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(dir.join("b1/one.md"), "alpha one\n").unwrap();
    fs::write(dir.join("b1/two.md"), "alpha two\n").unwrap();
    fs::write(dir.join("b2/one.md"), "beta one\n").unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"The Saga\"\n\n\
         [[books]]\ntitle = \"Book One\"\n\n\
         [[books.chapters]]\ntitle = \"Dawn\"\nfile = \"b1/one.md\"\n\n\
         [[books.chapters]]\ntitle = \"Noon\"\nfile = \"b1/two.md\"\n\n\
         [[books]]\ntitle = \"Book Two\"\n\n\
         [[books.chapters]]\ntitle = \"Dusk\"\nfile = \"b2/one.md\"\n",
    )
    .unwrap();

    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

fn chapter_titles(app: &App) -> Vec<String> {
    app.project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.clone())
        .collect()
}

#[test]
fn switching_book_reloads_the_editor_and_chapter_list() {
    let (mut app, _dir) = series_app("reload");
    assert_eq!(app.project.active_book(), 0);
    assert!(app.buffer.text().contains("alpha one"));
    assert_eq!(chapter_titles(&app), ["Dawn", "Noon"]);

    app.switch_book(1);

    assert_eq!(app.project.active_book(), 1);
    assert!(
        app.buffer.text().contains("beta one"),
        "editor opened book two's chapter"
    );
    assert_eq!(
        chapter_titles(&app),
        ["Dusk"],
        "chapter list follows the active book"
    );
    assert_eq!(app.project.book_title(1), Some("Book Two"));
}

#[test]
fn switching_book_flushes_pending_edits() {
    let (mut app, _dir) = series_app("flush");
    // Edit book one without an explicit save, then switch away and back.
    app.on_key(key(KeyCode::Char('X')));
    assert!(app.buffer.dirty);
    app.switch_book(1);
    app.switch_book(0);

    // The edit to book one was saved on the way out and reloads.
    assert!(
        app.buffer.text().starts_with('X'),
        "edit survived the switch: {:?}",
        app.buffer.text()
    );
    assert!(app.buffer.text().contains("alpha one"));
}

#[test]
fn switching_book_does_not_count_the_new_book_as_todays_writing() {
    let (mut app, _dir) = series_app("tally");
    app.tally_words(); // baseline book one
    app.switch_book(1);
    app.tally_words(); // book two is now active

    // Re-baselining on switch means book two's existing prose isn't "written
    // today" — only real edits move the counters.
    assert_eq!(
        app.added, 0,
        "switching to a book must not inflate today's words"
    );
    assert_eq!(app.removed, 0);
}

#[test]
fn a_standalone_novel_has_no_book_switcher() {
    let (mut app, _dir) = two_chapter_app("single-book");
    app.open_book_switcher();
    assert!(
        !matches!(app.mode, Mode::Palette(_)),
        "no switcher for a one-book project"
    );
    assert!(app.message.as_deref().unwrap_or("").contains("one book"));

    // And a direct switch is a guarded no-op.
    app.switch_book(1);
    assert_eq!(app.project.active_book(), 0);
}

#[test]
fn the_book_switcher_lists_every_book_with_the_active_one_marked() {
    let (mut app, _dir) = series_app("switcher");
    app.open_book_switcher();
    let Mode::Palette(p) = &app.mode else {
        panic!("expected the book palette")
    };
    assert_eq!(p.items.len(), 2);
    assert!(p.items[0].label.contains("Book One") && p.items[0].label.contains("current"));
    assert!(p.items[1].label.contains("Book Two") && !p.items[1].label.contains("current"));
}

// --- Tree navigator (#330) --------------------------------------------------

/// The visible tree row labels, indent-prefixed, with the `· N w` count suffix
/// stripped — structural assertions read the shape, the counts test reads counts.
fn tree_labels(app: &App) -> Vec<String> {
    let Mode::Tree(t) = &app.mode else {
        panic!("not in the tree navigator")
    };
    app.tree_rows(&t.expanded)
        .iter()
        .map(|r| {
            let label = r.label.split("  ·  ").next().unwrap_or(&r.label);
            format!("{}{}", "  ".repeat(r.indent as usize), label)
        })
        .collect()
}

#[test]
fn the_tree_shows_the_series_with_inactive_books_collapsed() {
    let (mut app, _dir) = series_app("tree-structure");
    app.open_tree();
    // Series root, both books, the active book's chapters; book two collapsed.
    assert_eq!(
        tree_labels(&app),
        [
            "▾ The Saga",
            "  ▾ Book One",
            "    Dawn",
            "    Noon",
            "  ▸ Book Two"
        ]
    );
}

#[test]
fn expanding_a_book_reveals_its_chapters() {
    let (mut app, _dir) = series_app("tree-expand");
    app.open_tree(); // selection on the active chapter (Dawn)
    app.on_key(key(KeyCode::Down)); // Noon
    app.on_key(key(KeyCode::Down)); // Book Two
    app.on_key(key(KeyCode::Right)); // expand it
    let labels = tree_labels(&app);
    assert!(
        labels.iter().any(|l| l.contains("▾ Book Two")),
        "{labels:?}"
    );
    assert!(labels.iter().any(|l| l.trim() == "Dusk"), "{labels:?}");
}

#[test]
fn entering_a_chapter_in_another_book_switches_and_opens_it() {
    let (mut app, _dir) = series_app("tree-jump");
    app.open_tree();
    app.on_key(key(KeyCode::Down)); // Noon
    app.on_key(key(KeyCode::Down)); // Book Two
    app.on_key(key(KeyCode::Right)); // expand
    app.on_key(key(KeyCode::Down)); // Dusk
    app.on_key(key(KeyCode::Enter)); // jump

    assert_eq!(app.project.active_book(), 1, "jumped to book two");
    assert!(app.buffer.text().contains("beta one"), "opened the chapter");
    assert!(!matches!(app.mode, Mode::Tree(_)), "tree closed on jump");
}

#[test]
fn collapsing_moves_the_selection_to_the_book() {
    let (mut app, _dir) = series_app("tree-collapse");
    app.open_tree(); // on Dawn (book one expanded)
    app.on_key(key(KeyCode::Left)); // collapse book one
    let Mode::Tree(t) = &app.mode else { panic!() };
    let rows = app.tree_rows(&t.expanded);
    assert!(matches!(
        rows[t.selected].node,
        crate::app::tree::TreeNode::Book(0)
    ));
    assert!(
        !tree_labels(&app).iter().any(|l| l.trim() == "Dawn"),
        "book one collapsed"
    );
}

#[test]
fn a_standalone_novel_is_a_one_book_tree() {
    let (mut app, _dir) = two_chapter_app("tree-single");
    app.open_tree();
    let labels = tree_labels(&app);
    assert!(
        !labels.iter().any(|l| l.contains("Series")),
        "no series root for one book"
    );
    assert_eq!(labels[0], "▾ Book"); // the book title, expanded
    assert!(labels.iter().any(|l| l.trim() == "One") && labels.iter().any(|l| l.trim() == "Two"));
}

#[test]
fn the_tree_groups_chapters_under_part_headers() {
    let (mut app, _dir) = two_chapter_app("tree-parts");
    app.project.manifest.parts = Some(vec![crate::project::Part {
        name: Some("Act I".into()),
        chapters: [1, 2],
        goal: None,
    }]);
    app.open_tree();
    let Mode::Tree(t) = &app.mode else { panic!() };
    let rows = app.tree_rows(&t.expanded);
    let act = rows
        .iter()
        .position(|r| r.label.starts_with("Act I"))
        .expect("part header present");
    // Chapters sit under (and indented past) the part header.
    assert!(rows[act + 1].label.starts_with("One"));
    assert!(
        rows[act + 1].indent > rows[act].indent,
        "chapter indented under the part"
    );
}

// --- Series bible: book-scoped passes (#339) --------------------------------

#[test]
fn book_digests_are_scoped_to_one_book() {
    let (app, _dir) = series_app("book-digests");
    let b0: Vec<String> = app.book_digests(0).into_keys().collect();
    let b1: Vec<String> = app.book_digests(1).into_keys().collect();
    assert_eq!(b0, ["b1/one.md", "b1/two.md"], "book one's files only");
    assert_eq!(b1, ["b2/one.md"], "book two's files only");
}

#[test]
fn folding_a_book_into_the_bible_keeps_the_other_books_state() {
    use crate::ai::incremental::State;
    let (mut app, dir) = series_app("fold-keeps");
    // Book one's character pass already recorded what it was built from.
    State::record(&dir, "characters", "2026-06-24", app.book_digests(0)).unwrap();
    // Folding book two in is a series pass: it merges, not replaces.
    app.pending_pass = Some(("characters".into(), app.book_digests(1), true));
    app.record_pass_state();

    let recorded = State::load(&dir).built_from("characters").unwrap().clone();
    assert!(
        recorded.contains_key("b1/one.md") && recorded.contains_key("b2/one.md"),
        "book one wasn't forgotten when book two folded in: {recorded:?}"
    );
}

// --- Series bible refresh driver (#340) -------------------------------------

#[test]
fn the_series_bible_refresh_is_gated_off_for_one_book() {
    use crate::mode::Action;
    let (app, _dir) = two_chapter_app("bible-gate");
    assert!(
        app.action_status(Action::RefreshSeriesBible).is_some(),
        "single-book: unavailable"
    );
}

#[test]
fn starting_the_refresh_with_one_book_is_a_no_op() {
    let (mut app, _dir) = two_chapter_app("bible-noop");
    app.start_refresh_series_bible();
    assert!(!app.has_series_bible_run());
    assert!(app.message.as_deref().unwrap_or("").contains("one book"));
}

#[test]
fn a_pass_error_stops_the_refresh() {
    use crate::mode::AiAction;
    let (mut app, _dir) = series_app("bible-error");
    app.seed_series_bible_run(vec![(AiAction::Characters, 0), (AiAction::Characters, 1)]);
    app.advance_run(Some("boom".into()));
    assert!(!app.has_series_bible_run(), "the run stopped");
    assert!(
        app.message.as_deref().unwrap_or("").contains("boom"),
        "{:?}",
        app.message
    );
}

#[test]
fn cancelling_stops_the_refresh_after_the_current_pass() {
    use crate::mode::AiAction;
    let (mut app, _dir) = series_app("bible-cancel");
    app.seed_series_bible_run(vec![(AiAction::Characters, 0), (AiAction::Characters, 1)]);
    app.cancel_run();
    assert!(
        app.has_series_bible_run(),
        "still running until the pass finishes"
    );
    // The in-flight pass completes; the cancel takes effect, later passes skipped.
    app.advance_run(None);
    assert!(!app.has_series_bible_run());
    assert!(
        app.message.as_deref().unwrap_or("").contains("cancelled"),
        "{:?}",
        app.message
    );
}

#[test]
fn a_stuck_series_bible_pass_times_out() {
    use crate::ai;
    use crate::mode::AiAction;
    let (mut app, _dir) = series_app("bible-timeout");
    // A spawned-but-unpolled task keeps `ai_task` busy (a request in flight); the
    // run is backdated past the per-step timeout, so the shared guard (#344) drops it.
    app.ai_task.spawn(|| {
        ai::Outcome::Error(ai::AiError::Network {
            status: None,
            message: "stuck".into(),
        })
    });
    app.seed_series_bible_run(vec![(AiAction::Characters, 0)]);
    app.expire_series_bible_step();
    app.tick_run_timeout();
    assert!(!app.has_series_bible_run(), "the stuck run was stopped");
    assert!(!app.ai_task.is_busy(), "the stuck request was dropped");
    assert!(
        app.message.as_deref().unwrap_or("").contains("timed out"),
        "{:?}",
        app.message
    );
}

// --- Cross-book findings (#341) ---------------------------------------------

#[test]
fn series_findings_are_attributed_to_their_book_and_accumulate_silently() {
    use crate::ai::continuity::{Continuity, Finding};
    use crate::mode::AiAction;
    let (mut app, _dir) = series_app("bible-cross");
    let mk = |chapter: &str| Continuity {
        events: vec![],
        findings: vec![Finding {
            kind: "object".into(),
            detail: "the locket changes color".into(),
            chapters: vec![chapter.into()],
        }],
    };
    // Book one's continuity pass is in flight: its finding is tagged to book 0,
    // accumulated into triage, and must NOT pop a findings overlay mid-refresh.
    app.seed_series_bible_run(vec![(AiAction::Continuity, 0)]);
    app.apply_continuity(Ok(mk("Dawn")));
    assert!(
        !matches!(app.mode, Mode::Findings(_)),
        "no overlay pops mid-refresh"
    );

    // Book two's pass next: an identical-looking finding is a *distinct* one,
    // because it belongs to a different book (titles can repeat across books).
    app.seed_series_bible_run(vec![(AiAction::Continuity, 1)]);
    app.apply_continuity(Ok(mk("Dusk")));
    assert_eq!(app.triage.len(), 2, "both books' findings accumulated");
    assert_eq!(app.triage[0].book, Some(0));
    assert_eq!(app.triage[1].book, Some(1));

    // Re-running book two's pass doesn't double-count its own finding.
    app.apply_continuity(Ok(mk("Dusk")));
    assert_eq!(app.triage.len(), 2, "deduped within a book");
}

#[test]
fn jumping_to_a_cross_book_finding_switches_books() {
    use crate::mode::Finding;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let (mut app, _dir) = series_app("bible-jump");
    assert_eq!(app.project.active_book(), 0, "opens on book one");
    // A finding in book two, surfaced in the (session-wide) triage list.
    app.triage.push(Finding {
        summary: "object: the locket".into(),
        chapters: vec!["Dusk".into()],
        book: Some(1),
    });
    app.open_triage();
    assert!(matches!(app.mode, Mode::Findings(_)));
    app.on_key_findings(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.project.active_book(),
        1,
        "Enter switched to the finding's book"
    );
    assert!(
        app.buffer.text().contains("beta one"),
        "opened book two's chapter"
    );
}

// --- Series export (#332) ---------------------------------------------------

#[test]
fn exporting_the_series_compiles_every_book_in_order() {
    use crate::mode::{ExportFormat, ExportScope};
    let (mut app, dir) = series_app("export-series");
    app.run_export(ExportFormat::Markdown, ExportScope::Series);

    // The file is named for the series; it carries both books and their prose.
    let out = fs::read_to_string(dir.join("export/the-saga.md")).unwrap();
    assert!(out.starts_with("The Saga"), "{out}");
    let one = out.find("# Book One").expect("book one");
    let two = out.find("# Book Two").expect("book two");
    assert!(one < two, "books in reading order");
    // Prose from the active book and the disk-read inactive book both appear.
    assert!(
        out.contains("alpha one") && out.contains("alpha two"),
        "active book prose"
    );
    assert!(out.contains("beta one"), "inactive book prose");
}

#[test]
fn exporting_just_the_active_book_is_scoped_to_it() {
    use crate::mode::{ExportFormat, ExportScope};
    let (mut app, dir) = series_app("export-book");
    app.run_export(ExportFormat::Markdown, ExportScope::ActiveBook);
    // Named for the active book; carries its prose, not the other book's.
    let out = fs::read_to_string(dir.join("export/book-one.md")).unwrap();
    assert!(
        out.contains("alpha one") && !out.contains("beta one"),
        "{out}"
    );
}

#[test]
fn the_export_menu_offers_series_and_book_scopes() {
    let (mut app, _dir) = series_app("export-menu");
    app.open_export_palette();
    let Mode::Palette(p) = &app.mode else {
        panic!("export palette")
    };
    assert!(p.items.iter().any(|i| i.label.starts_with("Series → ")));
    assert!(p.items.iter().any(|i| i.label.starts_with("This book → ")));
}

#[test]
fn a_standalone_export_menu_is_unchanged() {
    let (mut app, _dir) = two_chapter_app("export-single");
    app.open_export_palette();
    let Mode::Palette(p) = &app.mode else {
        panic!("export palette")
    };
    assert_eq!(p.items.len(), 5);
    assert_eq!(p.items[0].label, "Manuscript (Markdown)");
    assert_eq!(p.items[4].label, "EPUB (.epub)");
}

fn report_text(app: &App) -> String {
    app.report_lines()
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_progress_report_shows_a_series_total() {
    let (app, _dir) = series_app("report-series");
    let text = report_text(&app);
    assert!(
        text.contains("series:") && text.contains("6 words across 2 books"),
        "{text}"
    );
}

#[test]
fn a_standalone_report_has_no_series_line() {
    let (app, _dir) = two_chapter_app("report-single");
    assert!(!report_text(&app).contains("series:"));
}

#[test]
fn the_tree_rolls_up_word_counts() {
    // series_app: book one = "alpha one"(2) + "alpha two"(2) = 4; book two = "beta one"(2).
    let (mut app, _dir) = series_app("tree-counts");
    app.open_tree();
    let label = |needle: &str| {
        let Mode::Tree(t) = &app.mode else { panic!() };
        app.tree_rows(&t.expanded)
            .into_iter()
            .find(|r| r.label.contains(needle))
            .expect("row")
            .label
    };
    assert!(label("The Saga").contains("6 w"), "series total = 4 + 2"); // {}
    assert!(label("Book One").contains("4 w"), "book one total");
    assert!(label("Dawn").contains("2 w"), "chapter count");
}

/// Building the tree rows for a series-sized project reads only cached metadata
/// (never files), so even with every book expanded it stays well within a frame.
#[test]
#[ignore]
fn tree_rows_are_fast_for_a_series() {
    use std::collections::HashSet;
    use std::time::Instant;

    let (books, per_book) = (4, 200);
    let dir = crate::testutil::scratch_dir("tree", "perf");
    let mut manifest = String::from("title = \"Series\"\n\n");
    for b in 0..books {
        fs::create_dir_all(dir.join(format!("b{b}"))).unwrap();
        manifest.push_str(&format!("[[books]]\ntitle = \"Book {b}\"\n\n"));
        for i in 0..per_book {
            let file = format!("b{b}/ch{i}.md");
            manifest.push_str(&format!(
                "[[books.chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"
            ));
            fs::write(dir.join(&file), "prose\n").unwrap();
        }
    }
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let app = App::new(buffer, project);
    let all: HashSet<usize> = (0..books).collect(); // every book expanded

    let t = Instant::now();
    let rows = app.tree_rows(&all);
    let dt = t.elapsed();
    println!(
        "tree rows for {books}×{per_book} ({} chapters) = {dt:?}",
        books * per_book
    );
    assert!(rows.len() >= books * per_book);
    assert!(dt.as_millis() < 16, "tree build took {dt:?}");

    fs::remove_dir_all(&dir).ok();
}
