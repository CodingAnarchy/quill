//! Tests for the "Book details…" run-action (#571): the chained-prompt flow sets
//! title, subtitle, author, and series and persists each to the manifest (round-
//! tripping), the rest of the manifest stays intact, and the long-standing
//! chapter-browser single-letter keys (`t`/`b`/`w`/`e`) still set the same fields
//! and read back identically.

use super::*;
use crate::mode::{Action, BookField, Mode, Pending, PromptState};
use crate::project::Project;
use std::fs;

/// Drive one book-details prompt: clear whatever was prefilled, type `text`, and
/// press Enter (a blank `text` leaves the input empty — keeping the title or
/// clearing an optional field).
fn enter(app: &mut App, text: &str) {
    while matches!(&app.mode, Mode::Prompt(p) if !p.input.is_empty()) {
        app.on_key(key(KeyCode::Backspace));
    }
    type_str(app, text);
    app.on_key(key(KeyCode::Enter));
}

#[test]
fn the_book_details_action_is_registered_and_named() {
    assert!(
        Action::ALL.iter().any(|a| a.name() == "book:details"),
        "book:details is in the registry"
    );
    assert!(matches!(
        Action::from_name("book:details"),
        Some(Action::BookDetails)
    ));
    assert_eq!(Action::BookDetails.group(), "Project");
}

#[test]
fn walking_the_flow_persists_every_field_to_the_manifest() {
    let (mut app, dir) = two_chapter_app("book-details-walk");
    app.run_action(Action::BookDetails, false);
    enter(&mut app, "Book of Dust"); // title
    enter(&mut app, "La Belle Sauvage"); // subtitle
    enter(&mut app, "Philip Pullman"); // author
    enter(&mut app, "His Dark Materials"); // series

    // The in-memory manifest carries every value.
    assert_eq!(app.project.manifest.title, "Book of Dust");
    assert_eq!(
        app.project.manifest.subtitle.as_deref(),
        Some("La Belle Sauvage")
    );
    assert_eq!(
        app.project.manifest.author.as_deref(),
        Some("Philip Pullman")
    );
    assert_eq!(
        app.project.manifest.series.as_deref(),
        Some("His Dark Materials")
    );

    // And the manifest on disk re-opens identically.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.title, "Book of Dust");
    assert_eq!(
        reopened.manifest.subtitle.as_deref(),
        Some("La Belle Sauvage")
    );
    assert_eq!(reopened.manifest.author.as_deref(), Some("Philip Pullman"));
    assert_eq!(
        reopened.manifest.series.as_deref(),
        Some("His Dark Materials")
    );

    // The flow finishes back in the editor with a confirmation.
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.message.as_deref(), Some("Book details updated."));
}

#[test]
fn the_action_sets_the_same_fields_as_the_single_letter_keys() {
    // Set via the run-action…
    let (mut app, dir) = two_chapter_app("book-details-vs-keys-action");
    app.run_action(Action::BookDetails, false);
    enter(&mut app, "Title A");
    enter(&mut app, "Sub A");
    enter(&mut app, "Author A");
    enter(&mut app, "Series A");
    let from_action = Project::open(&dir).unwrap().manifest;

    // …and the same values via the chapter-browser single-letter keys. Each
    // prompt returns to the browser on Enter, so it stays open between fields.
    let (mut keyed, kdir) = two_chapter_app("book-details-vs-keys-keys");
    keyed.on_key(ctrl('g'));
    keyed.on_key(key(KeyCode::Char('t')));
    enter(&mut keyed, "Title A");
    keyed.on_key(key(KeyCode::Char('b')));
    enter(&mut keyed, "Sub A");
    keyed.on_key(key(KeyCode::Char('w')));
    enter(&mut keyed, "Author A");
    keyed.on_key(key(KeyCode::Char('e')));
    enter(&mut keyed, "Series A");
    let from_keys = Project::open(&kdir).unwrap().manifest;

    assert_eq!(from_action.title, from_keys.title);
    assert_eq!(from_action.subtitle, from_keys.subtitle);
    assert_eq!(from_action.author, from_keys.author);
    assert_eq!(from_action.series, from_keys.series);
}

#[test]
fn the_single_letter_keys_still_work_and_read_back() {
    let (mut app, dir) = two_chapter_app("book-details-keys-still-work");
    app.on_key(ctrl('g'));

    app.on_key(key(KeyCode::Char('t'))); // title (prefilled "Book")
    type_str(&mut app, " of Dust");
    app.on_key(key(KeyCode::Enter));

    app.on_key(key(KeyCode::Char('w'))); // author
    type_str(&mut app, "A. Writer");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.project.manifest.title, "Book of Dust");
    assert_eq!(app.project.manifest.author.as_deref(), Some("A. Writer"));

    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.title, "Book of Dust");
    assert_eq!(reopened.manifest.author.as_deref(), Some("A. Writer"));
}

#[test]
fn a_blank_title_keeps_the_current_one_but_blank_optionals_clear() {
    let (mut app, _dir) = two_chapter_app("book-details-blank");
    app.project.manifest.subtitle = Some("placeholder".into());
    app.project.manifest.author = Some("placeholder".into());

    app.run_action(Action::BookDetails, false);
    enter(&mut app, ""); // title — required, blank keeps "Book"
    enter(&mut app, ""); // subtitle — optional, blank clears it
    enter(&mut app, ""); // author — optional, blank clears it
    enter(&mut app, ""); // series — stays unset

    assert_eq!(app.project.manifest.title, "Book", "title kept on blank");
    assert_eq!(app.project.manifest.subtitle, None, "subtitle cleared");
    assert_eq!(app.project.manifest.author, None, "author cleared");
    assert_eq!(app.project.manifest.series, None);
}

#[test]
fn the_flow_prefills_each_field_with_its_current_value() {
    let (mut app, _dir) = two_chapter_app("book-details-prefill");
    app.project.manifest.subtitle = Some("Existing sub".into());

    app.run_action(Action::BookDetails, false);
    // First prompt is the title, prefilled with the current title.
    assert!(matches!(
        &app.mode,
        Mode::Prompt(PromptState { pending: Pending::SetBookField(BookField::Title), input, .. })
            if input == "Book"
    ));
    app.on_key(key(KeyCode::Enter)); // keep title, chain to subtitle

    // The subtitle prompt is prefilled with the existing subtitle.
    assert!(matches!(
        &app.mode,
        Mode::Prompt(PromptState { pending: Pending::SetBookField(BookField::Subtitle), input, .. })
            if input == "Existing sub"
    ));
}

#[test]
fn setting_book_details_leaves_the_rest_of_the_manifest_intact() {
    let dir = app_scratch_dir("book-details-preserve");
    let manifest = "title = \"Book\"\nauthor = \"Ada\"\n\n\
         [goals]\nwords = 70000\nchapters = 30\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n";
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    fs::write(dir.join("one.md"), "x\n").unwrap();
    let mut app = open_test_app(&dir);
    let _dir = TempProject::new(dir.clone());

    app.run_action(Action::BookDetails, false);
    enter(&mut app, "New Title");
    enter(&mut app, ""); // no subtitle
    enter(&mut app, "Ada Lovelace"); // change the author
    enter(&mut app, ""); // no series

    // The goals table and the chapter list survive (the manifest is rewritten
    // wholesale from the in-memory struct, not corrupted).
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.title, "New Title");
    assert_eq!(reopened.manifest.author.as_deref(), Some("Ada Lovelace"));
    let goals = reopened.goals().expect("goals survive");
    assert_eq!(goals.words, 70_000);
    assert_eq!(goals.chapters, 30);
    assert_eq!(reopened.manifest.chapters.len(), 1);
}
