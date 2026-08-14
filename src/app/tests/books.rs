//! The series tree navigator (#330): the series/book/chapter tree, expanding and
//! collapsing books, jumping into another book's chapter, part headers, and the
//! rolled-up word counts — plus the row-building perf guard.

use super::*;
use crate::project::Project;

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
#[ignore = "release-only perf guard; run via cargo perf"]
fn tree_rows_are_fast_for_a_series() {
    use std::collections::HashSet;
    use std::fmt::Write;
    use std::time::Instant;

    use crate::testutil::perf_budget_ms;

    let (books, per_book) = (4, 200);
    let dir = crate::testutil::scratch_dir("tree", "perf");
    let mut manifest = String::from("title = \"Series\"\n\n");
    for b in 0..books {
        fs::create_dir_all(dir.join(format!("b{b}"))).unwrap();
        write!(manifest, "[[books]]\ntitle = \"Book {b}\"\n\n").unwrap();
        for i in 0..per_book {
            let file = format!("b{b}/ch{i}.md");
            write!(
                manifest,
                "[[books.chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"
            )
            .unwrap();
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
    assert!(
        dt.as_millis() < perf_budget_ms(16),
        "tree build took {dt:?}"
    );

    fs::remove_dir_all(&dir).ok();
}
