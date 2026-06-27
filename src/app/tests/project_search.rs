//! Manuscript-wide find & replace (#295): searching across every chapter,
//! jumping to a match in its chapter, and replacing across the whole project
//! (front matter left untouched).

use super::*;
use crossterm::event::KeyCode;

#[test]
fn project_search_finds_matches_across_every_chapter() {
    let (mut app, _dir) = two_chapter_app("psearch-find");
    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter));

    let ps = app.project_search();
    assert_eq!(ps.matches.len(), 2, "one match per chapter");
    assert_eq!(ps.matches[0].chapter, 0);
    assert_eq!(ps.matches[1].chapter, 1);
    assert_eq!(ps.chapter_span(), 2);
}

#[test]
fn no_matches_reports_and_closes() {
    let (mut app, _dir) = two_chapter_app("psearch-none");
    app.open_project_find();
    type_str(&mut app, "zzzznotfound");
    app.on_key(key(KeyCode::Enter));

    assert!(!matches!(app.mode, Mode::ProjectSearch(_)), "closed");
    assert!(
        app.message.as_deref().unwrap_or("").contains("No matches"),
        "{:?}",
        app.message
    );
}

#[test]
fn jumping_to_a_match_opens_its_chapter() {
    let (mut app, _dir) = two_chapter_app("psearch-jump");
    assert_eq!(app.project.active_index(), 0);

    app.open_project_find();
    type_str(&mut app, "second"); // only in chapter two
    app.on_key(key(KeyCode::Enter)); // Results
    app.on_key(key(KeyCode::Enter)); // jump to the selected match

    assert_eq!(
        app.project.active_index(),
        1,
        "switched to the match's chapter"
    );
    assert!(
        app.buffer.text().contains("second chapter"),
        "opened the right chapter"
    );
}

#[test]
fn replace_all_rewrites_every_chapter_on_disk() {
    let (mut app, dir) = two_chapter_app("psearch-replace-all");
    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter)); // Results
    app.on_key(key(KeyCode::Char('r'))); // → Replacement
    type_str(&mut app, "section");
    app.on_key(key(KeyCode::Enter)); // → Step
    app.on_key(key(KeyCode::Char('a'))); // → Confirm
    app.on_key(key(KeyCode::Char('y'))); // replace all

    assert!(
        app.message.as_deref().unwrap_or("").contains("2 chapters"),
        "{:?}",
        app.message
    );
    // The active chapter (one.md) is flushed; the other is written directly.
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "first section\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        "second section\n"
    );
}

#[test]
fn replace_all_needs_confirmation() {
    let (mut app, dir) = two_chapter_app("psearch-confirm");
    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('r')));
    type_str(&mut app, "section");
    app.on_key(key(KeyCode::Enter)); // Step
    app.on_key(key(KeyCode::Char('a'))); // Confirm prompt
    app.on_key(key(KeyCode::Esc)); // decline — backs out to stepping

    assert!(
        matches!(app.mode, Mode::ProjectSearch(_)),
        "still open, nothing replaced"
    );
    // Files untouched (the non-active one is the unambiguous on-disk check).
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        "second chapter\n"
    );
}

#[test]
fn replacing_one_match_leaves_the_others() {
    let (mut app, dir) = two_chapter_app("psearch-one");
    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter)); // Results
    app.on_key(key(KeyCode::Char('r'))); // → Replacement
    type_str(&mut app, "section");
    app.on_key(key(KeyCode::Enter)); // → Step, selected = match 0 (chapter one, active)
    app.on_key(key(KeyCode::Char('y'))); // replace just that one
    app.on_key(key(KeyCode::Enter)); // done

    // Only chapter two's match remains untouched on disk.
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        "second chapter\n"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("Replaced 1"),
        "{:?}",
        app.message
    );
}

#[test]
fn replace_refreshes_cached_word_counts() {
    let (mut app, _dir) = two_chapter_app("psearch-recount");
    // two.md (non-active) is "second chapter" — two words.
    assert_eq!(app.project.chapter_words(1), 2);

    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('r')));
    type_str(&mut app, "epic tale");
    app.on_key(key(KeyCode::Enter)); // Step
    app.on_key(key(KeyCode::Char('a'))); // Confirm
    app.on_key(key(KeyCode::Char('y'))); // replace all

    // "second epic tale" — three words; the non-active chapter's cache refreshed.
    assert_eq!(app.project.chapter_words(1), 3);
}

#[test]
fn front_matter_is_not_searched_or_replaced() {
    let (mut app, dir) = two_chapter_app("psearch-frontmatter");
    // Chapter two's front matter mentions the query term; its body mentions it once.
    fs::write(
        dir.join("two.md"),
        "---\npov: chapter\n---\nsecond chapter\n",
    )
    .unwrap();

    app.open_project_find();
    type_str(&mut app, "chapter");
    app.on_key(key(KeyCode::Enter));
    // The front-matter "chapter" is not a match — one body match per chapter.
    assert_eq!(app.project_search().matches.len(), 2);

    app.on_key(key(KeyCode::Char('r')));
    type_str(&mut app, "scene");
    app.on_key(key(KeyCode::Enter)); // Step
    app.on_key(key(KeyCode::Char('a'))); // Confirm
    app.on_key(key(KeyCode::Char('y'))); // replace all

    let two = fs::read_to_string(dir.join("two.md")).unwrap();
    assert!(
        two.contains("pov: chapter"),
        "front matter preserved: {two}"
    );
    assert!(two.contains("second scene"), "body replaced: {two}");
}
