//! The corkboard (#589 Phase 1 / #636 Phase 2 / #698): an index-card overview
//! built from the active chapter's scene headings. Each card is a scene (a heading
//! plus everything up to the next same-or-shallower heading, so a `##` owns its
//! `###` sub-beats) + its first line of prose; Enter jumps the editor cursor to
//! that scene. The `#` chapter title is not a reorderable card, and reorder /
//! synopsis / status persist through the `scenes:` front matter.

use super::*;
use crate::mode::Mode;
use crate::project::Project;
use std::time::{Duration, Instant};

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

/// #717: the corkboard snapshots each `Card.start` when it opens, and autosave
/// (#409) fires on the idle tick regardless of mode (with an idle reparse). Neither
/// may shift the byte offset the snapshot jumps to. The buffer is dirty and
/// idle-elapsed while the board is up, so both a `tick_autosave` and an `on_idle`
/// reparse run before Enter; the jump must still land on the selected scene.
#[test]
fn autosave_and_reparse_cant_shift_a_focused_corkboard_jump() {
    let body = "# Chapter One\n\n\
                ## The Arrival\n\nRain hammered the platform.\n\n\
                ## The Meeting\n\nShe was already waiting.\n";
    let (mut app, _dir) = scene_app("corkboard-autosave-jump", body);
    app.autosave_interval_secs = 1; // autosave on
    app.open_corkboard();

    // Select the second card ("The Meeting").
    app.on_key(key(KeyCode::Down));

    // The buffer is dirty and has gone idle while the board is up: the event loop
    // would run both the idle-timer autosave and an idle reparse here.
    app.buffer.dirty = true;
    app.last_activity = Instant::now().checked_sub(Duration::from_secs(2)).unwrap();
    app.tick_autosave();
    app.on_idle();

    // Enter still lands on the correct "## The Meeting" offset — the save and
    // reparse left the in-memory offsets the snapshot jumps by untouched.
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(
        app.edit_buffer().cursor_byte(),
        body.find("## The Meeting").unwrap()
    );
    assert!(!app.buffer.dirty, "autosave saved the dirty buffer");
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

// --- Manuscript-wide corkboard (#701) --------------------------------------

/// A two-chapter project whose chapters each have scene headings, opened and
/// pinned like the other corkboard fixtures. The active chapter starts as the
/// first (`one.md`).
fn multi_scene_app(name: &str, one: &str, two: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), one).unwrap();
    fs::write(dir.join("two.md"), two).unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn manuscript_board_lists_scenes_from_every_chapter_grouped_by_chapter() {
    let (mut app, _dir) = multi_scene_app(
        "corkboard-ms-list",
        "# Chapter One\n\n## The Arrival\n\nRain.\n\n## The Meeting\n\nShe waited.\n",
        "# Chapter Two\n\n## The Departure\n\nSmoke.\n",
    );
    app.open_manuscript_corkboard();

    let Mode::Corkboard(board) = &app.mode else {
        panic!("manuscript corkboard should be open");
    };
    assert!(board.manuscript, "the board is the manuscript-wide scope");
    // Every chapter's scenes, in chapter then document order.
    let titles: Vec<&str> = board.cards.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, vec!["The Arrival", "The Meeting", "The Departure"]);
    // Cards are grouped by chapter: the first two belong to chapter 0, the last to
    // chapter 1, each tagged with its chapter title for the group header.
    assert_eq!(board.cards[0].chapter, Some(0));
    assert_eq!(board.cards[1].chapter, Some(0));
    assert_eq!(board.cards[2].chapter, Some(1));
    assert_eq!(board.cards[0].chapter_title, "One");
    assert_eq!(board.cards[2].chapter_title, "Two");
}

#[test]
fn manuscript_board_jump_opens_the_right_chapter_at_the_right_heading() {
    let two_body = "# Chapter Two\n\n## The Departure\n\nSmoke.\n";
    let (mut app, _dir) = multi_scene_app(
        "corkboard-ms-jump",
        "# Chapter One\n\n## The Arrival\n\nRain.\n",
        two_body,
    );
    app.open_manuscript_corkboard();
    // Chapter 0 starts active; the third card ("The Departure") is in chapter 1.
    assert_eq!(app.project.active_index(), 0);

    // Select the chapter-two scene and jump to it.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));

    // The jump switched to chapter 1 and closed the board.
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.project.active_index(), 1);
    // The cursor sits on the "## The Departure" heading of chapter two's body.
    let expected = two_body.find("## The Departure").unwrap();
    assert_eq!(app.edit_buffer().cursor_byte(), expected);
}

#[test]
fn manuscript_board_reorders_within_a_nonactive_chapter_and_persists() {
    // Reorder two scenes in chapter *two* while chapter one is active — the edit
    // must open chapter two, move the block, and persist to its file (#701).
    let (mut app, dir) = multi_scene_app(
        "corkboard-ms-reorder",
        "# Chapter One\n\n## Alpha\n\nA.\n",
        "# Chapter Two\n\n## First\n\nBeta.\n\n## Second\n\nGamma.\n",
    );
    app.open_manuscript_corkboard();
    // Cards: [Alpha(ch0), First(ch1), Second(ch1)]. Select "Second" and move up.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('K')));

    // The board stays open, still manuscript-wide, with "Second" now before
    // "First" within chapter two, and the moved card still selected.
    let Mode::Corkboard(board) = &app.mode else {
        panic!("manuscript corkboard stays open after a reorder");
    };
    assert!(board.manuscript);
    let titles: Vec<&str> = board.cards.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, vec!["Alpha", "Second", "First"]);
    assert_eq!(board.cards[board.selected].title, "Second");

    // Chapter two's file reflects the new order; chapter one is untouched.
    let two = fs::read_to_string(dir.join("two.md")).unwrap();
    assert!(two.find("## Second").unwrap() < two.find("## First").unwrap());
    assert!(two.contains("## Second\n\nGamma.\n"));
    assert!(two.contains("## First\n\nBeta.\n"));
    let one = fs::read_to_string(dir.join("one.md")).unwrap();
    assert_eq!(one, "# Chapter One\n\n## Alpha\n\nA.\n");
}

#[test]
fn manuscript_board_synopsis_and_status_target_the_cards_own_chapter() {
    // Cycle status and set a synopsis on a scene in the non-active chapter two;
    // both must round-trip through *that* chapter's front matter (#701).
    let (mut app, dir) = multi_scene_app(
        "corkboard-ms-meta",
        "# Chapter One\n\n## Alpha\n\nA.\n",
        "# Chapter Two\n\n## Beta\n\nB.\n",
    );
    app.open_manuscript_corkboard();
    // Select "Beta" (chapter two), cycle its status, and set a synopsis.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('s')));
    app.on_key(key(KeyCode::Char('e')));
    type_str(&mut app, "the exit");
    app.on_key(key(KeyCode::Enter));

    let Mode::Corkboard(board) = &app.mode else {
        panic!("manuscript corkboard reopens after a synopsis edit");
    };
    let beta = board.cards.iter().find(|c| c.title == "Beta").unwrap();
    assert_eq!(beta.synopsis, "the exit");
    assert_eq!(beta.status, crate::frontmatter::SceneStatus::Draft);

    // The metadata landed in chapter two's front matter, not chapter one's.
    let two = fs::read_to_string(dir.join("two.md")).unwrap();
    assert!(two.starts_with("---\n") && two.contains("the exit"));
    let one = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(!one.contains("scenes:"), "chapter one is untouched");

    // Reopen from disk: the synopsis and status survived on chapter two.
    let reopened = open_test_app(&dir);
    let cards = reopened.project.chapter_front_matter(1).unwrap().scenes();
    assert_eq!(cards[0].synopsis, "the exit");
    assert_eq!(cards[0].status, crate::frontmatter::SceneStatus::Draft);
}

#[test]
fn manuscript_board_is_reachable_from_the_continuous_view() {
    // The per-chapter board is refused in the continuous view; the manuscript-wide
    // board leaves it first and opens (#701).
    let (mut app, _dir) = multi_scene_app(
        "corkboard-ms-continuous",
        "# Chapter One\n\n## Alpha\n\nA.\n",
        "# Chapter Two\n\n## Beta\n\nB.\n",
    );
    app.enter_continuous();
    assert!(app.manuscript.is_some(), "continuous view is active");

    app.open_manuscript_corkboard();
    assert!(app.manuscript.is_none(), "it left the continuous view");
    let Mode::Corkboard(board) = &app.mode else {
        panic!("manuscript corkboard opens from the continuous view");
    };
    assert!(board.manuscript);
    assert_eq!(board.cards.len(), 2);
}

#[test]
fn manuscript_board_refuses_a_cross_chapter_reorder() {
    // Moving the first chapter's last scene *down* would cross into chapter two —
    // deferred, so it's a no-op with an explanatory message (#701).
    let (mut app, dir) = multi_scene_app(
        "corkboard-ms-cross",
        "# Chapter One\n\n## Alpha\n\nA.\n",
        "# Chapter Two\n\n## Beta\n\nB.\n",
    );
    app.open_manuscript_corkboard();
    // "Alpha" is the last (only) scene of chapter one; move it down.
    app.on_key(key(KeyCode::Char('J')));

    // Order unchanged, a message explains why, and neither file was rewritten.
    assert_eq!(card_titles(&app), vec!["Alpha", "Beta"]);
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("Cross-chapter"));
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "# Chapter One\n\n## Alpha\n\nA.\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        "# Chapter Two\n\n## Beta\n\nB.\n"
    );
}
