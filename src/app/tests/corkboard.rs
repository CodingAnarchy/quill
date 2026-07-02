//! The corkboard (#589 Phase 1 / #636 Phase 2 / #698): an index-card overview
//! built from the active chapter's scene headings. Each card is a scene (a heading
//! plus everything up to the next same-or-shallower heading, so a `##` owns its
//! `###` sub-beats) + its first line of prose; Enter jumps the editor cursor to
//! that scene. The `#` chapter title is not a reorderable card, and reorder /
//! synopsis / status persist through the `scenes:` front matter.

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
    // A card per scene heading, in document order — the `#` chapter title is not a
    // reorderable scene, so it isn't carded (#698).
    let titles: Vec<&str> = board.cards.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, vec!["The Arrival", "The Meeting"]);

    // Each scene card carries its first line of prose as the fallback synopsis.
    assert_eq!(board.cards[0].prose_line, "Rain hammered the platform.");
    assert_eq!(board.cards[1].prose_line, "She was already waiting.");
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
fn nested_sub_beats_fold_into_their_scene_and_the_title_is_not_a_card() {
    // A `#` chapter title plus two `##` scenes, each with `###` sub-beats. The
    // title is not a card, and each `##` is one card that owns its sub-beats — the
    // `###` headings do not become their own reorderable scenes (#698).
    let (mut app, _dir) = scene_app(
        "corkboard-nested-cards",
        "# Chapter One\n\n\
         ## The Arrival\n\nRain fell.\n\n\
         ### Off the train\n\nMara stepped down.\n\n\
         ## The Meeting\n\nShe waited.\n\n\
         ### The handoff\n\nThe case changed hands.\n",
    );
    app.open_corkboard();

    // Only the two `##` scenes card up; the `#` title and the `###` sub-beats do
    // not.
    assert_eq!(card_titles(&app), vec!["The Arrival", "The Meeting"]);
}

#[test]
fn reordering_a_scene_carries_its_nested_sub_beats_and_metadata() {
    // Reordering a `##` scene up must move its `###` sub-beat prose with it as one
    // block — never splitting the scene at its first sub-beat — and its metadata
    // must travel too (#698). This is the data-integrity regression: the old
    // flat-heading reorder moved only up to the first `###`.
    let (mut app, dir) = scene_app(
        "corkboard-nested-reorder",
        "# Chapter One\n\n\
         ## The Arrival\n\nRain fell.\n\n\
         ### Off the train\n\nMara stepped down.\n\n\
         ## The Meeting\n\nShe waited.\n\n\
         ### The handoff\n\nThe case changed hands.\n",
    );
    app.open_corkboard();
    // Annotate each scene so we can prove the metadata moved with it.
    app.on_key(key(KeyCode::Char('e')));
    for c in "arrival note".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('e')));
    for c in "meeting note".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter));

    // Selection is on "The Meeting"; move it up above "The Arrival".
    app.on_key(key(KeyCode::Char('K')));

    // The board reordered, and each card still carries its own synopsis.
    assert_eq!(card_titles(&app), vec!["The Meeting", "The Arrival"]);
    let Mode::Corkboard(board) = &app.mode else {
        panic!("corkboard stays open after a reorder");
    };
    assert_eq!(board.selected, 0);
    assert_eq!(board.cards[0].synopsis, "meeting note");
    assert_eq!(board.cards[1].synopsis, "arrival note");

    // The whole "The Meeting" block — heading, prose, and its `### The handoff`
    // sub-beat — moved above "The Arrival" as a unit; no prose was left behind.
    let saved = fs::read_to_string(dir.join("one.md")).unwrap();
    let meeting = saved.find("## The Meeting").unwrap();
    let arrival = saved.find("## The Arrival").unwrap();
    let handoff = saved.find("### The handoff").unwrap();
    let off_train = saved.find("### Off the train").unwrap();
    // The Meeting scene (heading through its sub-beat) sits entirely before The
    // Arrival scene.
    assert!(
        meeting < handoff,
        "The Meeting's sub-beat follows its heading"
    );
    assert!(
        handoff < arrival,
        "The whole Meeting block precedes The Arrival"
    );
    assert!(
        arrival < off_train,
        "The Arrival still owns its own sub-beat"
    );
    // The prose stayed intact within each block — no scene was split.
    assert!(saved
        .contains("## The Meeting\n\nShe waited.\n\n### The handoff\n\nThe case changed hands.\n"));
    assert!(
        saved.contains("## The Arrival\n\nRain fell.\n\n### Off the train\n\nMara stepped down.\n")
    );
    // The `#` chapter title stayed at the top, above both moved scenes.
    let title = saved.find("# Chapter One").unwrap();
    assert!(title < meeting, "the chapter title is untouched at the top");
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
