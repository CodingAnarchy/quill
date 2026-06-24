//! Switching the active book of a multi-book series (#329): the editor, chapter
//! list, and daily tally follow the active book, pending edits are flushed on
//! the way out, and a standalone novel has no switcher.

use super::*;
use crate::project::Project;

/// A two-book series on disk, opened on book one, with the deterministic test
/// pins the shared helper applies (Vim / smart typography / workflows off).
fn series_app(name: &str) -> (App, TempProject) {
    let mut dir = std::env::temp_dir();
    dir.push(format!("quill-series-{}-{}", std::process::id(), name));
    let _ = fs::remove_dir_all(&dir);
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

    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);
    app.vim = false;
    app.smart_typography = false;
    app.mode = Mode::Insert;
    app.workflows = Vec::new();
    app.triggers = Default::default();
    (app, TempProject::new(dir))
}

fn chapter_titles(app: &App) -> Vec<String> {
    app.project.manifest.chapters.iter().map(|c| c.title.clone()).collect()
}

#[test]
fn switching_book_reloads_the_editor_and_chapter_list() {
    let (mut app, _dir) = series_app("reload");
    assert_eq!(app.project.active_book(), 0);
    assert!(app.buffer.text().contains("alpha one"));
    assert_eq!(chapter_titles(&app), ["Dawn", "Noon"]);

    app.switch_book(1);

    assert_eq!(app.project.active_book(), 1);
    assert!(app.buffer.text().contains("beta one"), "editor opened book two's chapter");
    assert_eq!(chapter_titles(&app), ["Dusk"], "chapter list follows the active book");
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
    assert!(app.buffer.text().starts_with('X'), "edit survived the switch: {:?}", app.buffer.text());
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
    assert_eq!(app.added, 0, "switching to a book must not inflate today's words");
    assert_eq!(app.removed, 0);
}

#[test]
fn a_standalone_novel_has_no_book_switcher() {
    let (mut app, _dir) = two_chapter_app("single-book");
    app.open_book_switcher();
    assert!(!matches!(app.mode, Mode::Palette(_)), "no switcher for a one-book project");
    assert!(app.message.as_deref().unwrap_or("").contains("one book"));

    // And a direct switch is a guarded no-op.
    app.switch_book(1);
    assert_eq!(app.project.active_book(), 0);
}

#[test]
fn the_book_switcher_lists_every_book_with_the_active_one_marked() {
    let (mut app, _dir) = series_app("switcher");
    app.open_book_switcher();
    let Mode::Palette(p) = &app.mode else { panic!("expected the book palette") };
    assert_eq!(p.items.len(), 2);
    assert!(p.items[0].label.contains("Book One") && p.items[0].label.contains("current"));
    assert!(p.items[1].label.contains("Book Two") && !p.items[1].label.contains("current"));
}

// --- Tree navigator (#330) --------------------------------------------------

/// The visible tree row labels, indent-prefixed, with the `· N w` count suffix
/// stripped — structural assertions read the shape, the counts test reads counts.
fn tree_labels(app: &App) -> Vec<String> {
    let Mode::Tree(t) = &app.mode else { panic!("not in the tree navigator") };
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
        ["▾ The Saga", "  ▾ Book One", "    Dawn", "    Noon", "  ▸ Book Two"]
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
    assert!(labels.iter().any(|l| l.contains("▾ Book Two")), "{labels:?}");
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
    assert!(matches!(rows[t.selected].node, crate::app::tree::TreeNode::Book(0)));
    assert!(!tree_labels(&app).iter().any(|l| l.trim() == "Dawn"), "book one collapsed");
}

#[test]
fn a_standalone_novel_is_a_one_book_tree() {
    let (mut app, _dir) = two_chapter_app("tree-single");
    app.open_tree();
    let labels = tree_labels(&app);
    assert!(!labels.iter().any(|l| l.contains("Series")), "no series root for one book");
    assert_eq!(labels[0], "▾ Book"); // the book title, expanded
    assert!(labels.iter().any(|l| l.trim() == "One") && labels.iter().any(|l| l.trim() == "Two"));
}

#[test]
fn the_tree_groups_chapters_under_part_headers() {
    let (mut app, _dir) = two_chapter_app("tree-parts");
    app.project.manifest.parts =
        Some(vec![crate::project::Part { name: Some("Act I".into()), chapters: [1, 2], goal: None }]);
    app.open_tree();
    let Mode::Tree(t) = &app.mode else { panic!() };
    let rows = app.tree_rows(&t.expanded);
    let act = rows.iter().position(|r| r.label.starts_with("Act I")).expect("part header present");
    // Chapters sit under (and indented past) the part header.
    assert!(rows[act + 1].label.starts_with("One"));
    assert!(rows[act + 1].indent > rows[act].indent, "chapter indented under the part");
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
    assert!(out.contains("alpha one") && out.contains("alpha two"), "active book prose");
    assert!(out.contains("beta one"), "inactive book prose");
}

#[test]
fn exporting_just_the_active_book_is_scoped_to_it() {
    use crate::mode::{ExportFormat, ExportScope};
    let (mut app, dir) = series_app("export-book");
    app.run_export(ExportFormat::Markdown, ExportScope::ActiveBook);
    // Named for the active book; carries its prose, not the other book's.
    let out = fs::read_to_string(dir.join("export/book-one.md")).unwrap();
    assert!(out.contains("alpha one") && !out.contains("beta one"), "{out}");
}

#[test]
fn the_export_menu_offers_series_and_book_scopes() {
    let (mut app, _dir) = series_app("export-menu");
    app.open_export_palette();
    let Mode::Palette(p) = &app.mode else { panic!("export palette") };
    assert!(p.items.iter().any(|i| i.label.starts_with("Series → ")));
    assert!(p.items.iter().any(|i| i.label.starts_with("This book → ")));
}

#[test]
fn a_standalone_export_menu_is_unchanged() {
    let (mut app, _dir) = two_chapter_app("export-single");
    app.open_export_palette();
    let Mode::Palette(p) = &app.mode else { panic!("export palette") };
    assert_eq!(p.items.len(), 4);
    assert_eq!(p.items[0].label, "Manuscript (Markdown)");
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
    assert!(text.contains("series:") && text.contains("6 words across 2 books"), "{text}");
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
        app.tree_rows(&t.expanded).into_iter().find(|r| r.label.contains(needle)).expect("row").label
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
    let mut dir = std::env::temp_dir();
    dir.push(format!("quill-tree-perf-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let mut manifest = String::from("title = \"Series\"\n\n");
    for b in 0..books {
        fs::create_dir_all(dir.join(format!("b{b}"))).unwrap();
        manifest.push_str(&format!("[[books]]\ntitle = \"Book {b}\"\n\n"));
        for i in 0..per_book {
            let file = format!("b{b}/ch{i}.md");
            manifest.push_str(&format!("[[books.chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"));
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
    println!("tree rows for {books}×{per_book} ({} chapters) = {dt:?}", books * per_book);
    assert!(rows.len() >= books * per_book);
    assert!(dt.as_millis() < 16, "tree build took {dt:?}");

    fs::remove_dir_all(&dir).ok();
}
