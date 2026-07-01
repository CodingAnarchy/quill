//! The read-only corkboard (#589, Phase 1): an index-card overview built from the
//! active chapter's `##`/`###` scene headings. Each card is a heading + its first
//! line of prose; Enter jumps the editor cursor to that scene. Phase 2 (per-scene
//! metadata + reorder) is a documented follow-up on #589.

use super::*;
use crate::mode::Mode;
use crate::project::Project;

/// A one-chapter project whose body has scene headings, opened and pinned like the
/// other fixtures.
fn scene_app(name: &str, body: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), body).unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn corkboard_lists_the_active_chapters_scenes_as_cards() {
    let (mut app, _dir) = scene_app(
        "corkboard-cards",
        "# Chapter One\n\n\
         ## The Arrival\n\nRain hammered the platform.\n\n\
         ## The Meeting\n\nShe was already waiting.\n",
    );
    app.open_corkboard();

    let Mode::Corkboard(board) = &app.mode else {
        panic!("corkboard should be open");
    };
    // A card per heading, in document order — including the chapter's `#` title.
    let titles: Vec<&str> = board.cards.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, vec!["Chapter One", "The Arrival", "The Meeting"]);

    // Each scene card carries its first line of prose as the fallback synopsis.
    assert_eq!(board.cards[1].prose_line, "Rain hammered the platform.");
    assert_eq!(board.cards[2].prose_line, "She was already waiting.");
    // The `#` title has no prose before the first `##`, so its fallback is empty.
    assert_eq!(board.cards[0].prose_line, "");
    // A chapter with no scene metadata still lists cards, each at the default
    // (blank synopsis, stub status) (#636).
    assert!(board.cards.iter().all(|c| c.synopsis.is_empty()));
    assert!(board
        .cards
        .iter()
        .all(|c| c.status == crate::frontmatter::SceneStatus::Stub));
}

#[test]
fn enter_jumps_the_cursor_to_the_selected_scene() {
    let body = "# Chapter One\n\n\
                ## The Arrival\n\nRain hammered the platform.\n\n\
                ## The Meeting\n\nShe was already waiting.\n";
    let (mut app, _dir) = scene_app("corkboard-jump", body);
    app.open_corkboard();

    // Select the third card ("The Meeting") and jump to it.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    // The corkboard closed back to the editor.
    assert!(matches!(app.mode, Mode::Insert));
    // The cursor now sits at the "## The Meeting" heading's byte offset.
    let expected = body.find("## The Meeting").unwrap();
    assert_eq!(app.edit_buffer().cursor_byte(), expected);
}

#[test]
fn corkboard_reports_when_the_chapter_has_no_scenes() {
    let (mut app, _dir) = scene_app("corkboard-empty", "Just prose, no headings at all.\n");
    app.open_corkboard();

    // No headings → no corkboard; a message explains why.
    assert!(matches!(app.mode, Mode::Insert));
    assert!(app.message.as_deref().unwrap_or("").contains("No scenes"));
}

#[test]
fn esc_closes_the_corkboard_without_moving_the_cursor() {
    let body = "## Scene A\n\nBody.\n";
    let (mut app, _dir) = scene_app("corkboard-esc", body);
    let before = app.edit_buffer().cursor_byte();
    app.open_corkboard();
    app.on_key(key(KeyCode::Esc));

    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.edit_buffer().cursor_byte(), before);
}

/// The card titles of the currently-open corkboard, in order.
fn card_titles(app: &App) -> Vec<String> {
    let Mode::Corkboard(board) = &app.mode else {
        panic!("corkboard should be open");
    };
    board.cards.iter().map(|c| c.title.clone()).collect()
}

#[test]
fn reorder_moves_a_scenes_heading_and_prose_together_and_persists() {
    // Two scenes; moving the second up must carry its heading *and* its prose
    // above the first, and the change must be written to disk (#636).
    let (mut app, dir) = scene_app(
        "corkboard-reorder",
        "## The Arrival\n\nRain hammered the platform.\n\n\
         ## The Meeting\n\nShe was already waiting.\n",
    );
    app.open_corkboard();
    // Select "The Meeting" and move it up (K).
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('K')));

    // The board rebuilt with the moved scene still selected, in the new order.
    assert_eq!(card_titles(&app), vec!["The Meeting", "The Arrival"]);
    let Mode::Corkboard(board) = &app.mode else {
        panic!("corkboard stays open after a reorder");
    };
    assert_eq!(board.selected, 0);

    // The on-disk body reflects the new order — heading and prose moved together.
    let saved = fs::read_to_string(dir.join("one.md")).unwrap();
    let meeting = saved.find("## The Meeting").unwrap();
    let arrival = saved.find("## The Arrival").unwrap();
    assert!(
        meeting < arrival,
        "The Meeting block moved above The Arrival"
    );
    assert!(saved.contains("## The Meeting\n\nShe was already waiting.\n"));
    assert!(saved.contains("## The Arrival\n\nRain hammered the platform.\n"));
    // Prose is not polluted with any metadata markup.
    assert!(!saved.contains("scenes:"));
}

#[test]
fn synopsis_and_status_round_trip_through_front_matter() {
    // Edit a scene's synopsis and cycle its status, then reopen the project from
    // disk and confirm both survived (#636).
    let (mut app, dir) = scene_app(
        "corkboard-meta",
        "## The Arrival\n\nRain hammered the platform.\n",
    );
    app.open_corkboard();
    // Cycle the status once: stub → draft.
    app.on_key(key(KeyCode::Char('s')));
    // Edit the synopsis via the prompt.
    app.on_key(key(KeyCode::Char('e')));
    for c in "Mara steps off the train".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter));

    let Mode::Corkboard(board) = &app.mode else {
        panic!("corkboard reopens after a synopsis edit");
    };
    assert_eq!(board.cards[0].synopsis, "Mara steps off the train");
    assert_eq!(
        board.cards[0].status,
        crate::frontmatter::SceneStatus::Draft
    );

    // The metadata lives in the front matter, out of the prose body.
    let saved = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(saved.starts_with("---\n"));
    assert!(saved.contains("scenes:"));
    assert!(!saved.contains("Rain hammered the platform.\nscenes"));

    // Reopen from disk: the synopsis and status are still there.
    let reopened = open_test_app(&dir);
    let cards = reopened.corkboard_cards();
    assert_eq!(cards[0].synopsis, "Mara steps off the train");
    assert_eq!(cards[0].status, crate::frontmatter::SceneStatus::Draft);
}

#[test]
fn reorder_keeps_each_scenes_metadata_attached() {
    // Give each scene a distinct synopsis, reorder, and confirm the metadata
    // travelled with its scene rather than staying at the old position (#636).
    let (mut app, _dir) = scene_app(
        "corkboard-reorder-meta",
        "## First\n\nAlpha.\n\n## Second\n\nBeta.\n",
    );
    app.open_corkboard();
    // Synopsis for the first scene.
    app.on_key(key(KeyCode::Char('e')));
    for c in "first synopsis".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter));
    // Synopsis for the second scene.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('e')));
    for c in "second synopsis".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter));

    // Selection is on "Second"; move it up.
    app.on_key(key(KeyCode::Char('K')));

    let Mode::Corkboard(board) = &app.mode else {
        panic!("corkboard stays open");
    };
    assert_eq!(
        board
            .cards
            .iter()
            .map(|c| c.title.as_str())
            .collect::<Vec<_>>(),
        vec!["Second", "First"]
    );
    // The synopses moved with their scenes — "Second" now leads, still carrying
    // "second synopsis", not the metadata that had been at position 0.
    assert_eq!(board.cards[0].synopsis, "second synopsis");
    assert_eq!(board.cards[1].synopsis, "first synopsis");
}

#[test]
fn an_existing_chapter_without_scene_metadata_opens_unchanged() {
    // A plain chapter (no front matter) still lists cards and its file is
    // untouched until the author actually edits scene metadata (#636).
    let body = "## Only Scene\n\nJust prose.\n";
    let (mut app, dir) = scene_app("corkboard-plain", body);
    app.open_corkboard();
    assert_eq!(card_titles(&app), vec!["Only Scene"]);
    // Nothing persisted yet — the file is byte-for-byte the original.
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), body);
}
