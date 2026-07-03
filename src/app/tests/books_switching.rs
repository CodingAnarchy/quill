//! Switching the active book of a multi-book series (#329): the editor, chapter
//! list, and daily tally follow the active book, pending edits are flushed on the
//! way out, and a standalone novel has no switcher. Plus in-app series management
//! — convert / add / remove / reorder (#648).

use super::*;

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

// --- Series management: create / add / remove / reorder (#648) ---------------

/// Converting a standalone novel to a series in-app (#648): the convert prompt
/// writes the series and chains to the add-a-book prompt, and the second book
/// then appears in the switcher.
#[test]
fn convert_and_add_a_book_in_app() {
    let (mut app, _dir) = two_chapter_app("convert");
    assert_eq!(app.project.book_count(), 1);

    // Convert: blank uses the current book title as the series name; this chains
    // straight into the add-a-book prompt.
    app.start_convert_to_series();
    app.convert_to_series("The Saga");
    assert_eq!(app.project.book_count(), 1);
    assert_eq!(app.project.series_title(), Some("The Saga"));
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::AddBook)),
        "convert chains into the add-a-book prompt"
    );

    // Add the second book: it becomes active and shows in the switcher.
    app.add_book("Book Two");
    assert_eq!(app.project.book_count(), 2);
    assert_eq!(app.project.active_book(), 1);
    assert!(
        app.buffer.text().is_empty(),
        "opened the new book's starter chapter"
    );

    app.open_book_switcher();
    let Mode::Palette(p) = &app.mode else {
        panic!("expected the book palette")
    };
    assert_eq!(p.items.len(), 2);
    assert!(p.items[1].label.contains("Book Two") && p.items[1].label.contains("current"));
}

/// The manage-books palette (#648) lists each book with remove / move / add
/// entries; removing a book (confirmed) drops it from the switcher, and its
/// chapters are retired to `unused/`.
#[test]
fn manage_books_removes_a_book() {
    let (mut app, dir) = series_app("manage-remove");
    app.open_manage_books();
    assert!(
        matches!(app.mode, Mode::Palette(_)),
        "manage-books opens a palette"
    );

    // Remove the inactive second book through the confirm flow.
    app.confirm_remove_book(1);
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ConfirmRemoveBook { index: 1 })),
        "remove opens a confirm"
    );
    app.on_key(key(KeyCode::Char('y')));
    assert_eq!(app.project.book_count(), 1);
    assert!(dir.join("unused").join("one.md").exists());

    // The switcher now reports a single book.
    app.open_book_switcher();
    assert!(
        !matches!(app.mode, Mode::Palette(_)),
        "one book left — no switcher"
    );
}

/// Reordering books in-app (#648) swaps their series order and re-opens the
/// manage palette so the new order is visible; the switcher reflects it.
#[test]
fn manage_books_reorders() {
    let (mut app, _dir) = series_app("manage-reorder");
    // Move Book One (index 0) down.
    app.move_book(0, false);
    assert_eq!(app.project.book_title(0), Some("Book Two"));
    assert_eq!(app.project.book_title(1), Some("Book One"));
    // The palette re-opened with the new order.
    let Mode::Palette(p) = &app.mode else {
        panic!("manage palette re-opens after a move")
    };
    assert!(p.items[0].label.contains("Book Two"));
}
