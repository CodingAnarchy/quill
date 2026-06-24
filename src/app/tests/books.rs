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
