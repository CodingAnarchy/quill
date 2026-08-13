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
fn manuscript_board_moves_a_scene_down_into_the_next_chapter() {
    // Moving chapter one's last scene *down* crosses into chapter two (#974): its
    // prose leaves chapter one and lands at the top of chapter two, and the board
    // reopens on the moved scene, now in chapter two.
    let (mut app, dir) = multi_scene_app(
        "corkboard-ms-cross-down",
        "# Chapter One\n\n## Alpha\n\nA.\n",
        "# Chapter Two\n\n## Beta\n\nB.\n",
    );
    app.open_manuscript_corkboard();
    // "Alpha" is the last (only) scene of chapter one; move it down into chapter two.
    app.on_key(key(KeyCode::Char('J')));

    // The board stays open, manuscript-wide, with Alpha now under chapter two,
    // before Beta, and the moved card still selected.
    let Mode::Corkboard(board) = &app.mode else {
        panic!("manuscript corkboard stays open after a cross-chapter move");
    };
    assert!(board.manuscript);
    assert_eq!(card_titles(&app), vec!["Alpha", "Beta"]);
    assert_eq!(
        board.cards[0].chapter,
        Some(1),
        "Alpha now belongs to chapter two"
    );
    assert_eq!(board.cards[1].chapter, Some(1));
    assert_eq!(board.cards[board.selected].title, "Alpha");

    // Chapter one lost the prose; chapter two gained it at the top.
    let one = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(
        !one.contains("## Alpha"),
        "the scene left chapter one: {one:?}"
    );
    assert!(!one.contains("A.\n") || !one.contains("Alpha"));
    let two = fs::read_to_string(dir.join("two.md")).unwrap();
    assert!(
        two.contains("## Alpha\n\nA."),
        "chapter two gained the prose: {two:?}"
    );
    assert!(two.find("## Alpha").unwrap() < two.find("## Beta").unwrap());
}

#[test]
fn manuscript_board_moves_a_scene_up_into_the_previous_chapter() {
    // Moving chapter two's first scene *up* crosses into chapter one (#974): it
    // lands after chapter one's last scene, staying adjacent across the seam.
    let (mut app, dir) = multi_scene_app(
        "corkboard-ms-cross-up",
        "# Chapter One\n\n## Alpha\n\nA.\n",
        "# Chapter Two\n\n## Beta\n\nB.\n",
    );
    app.open_manuscript_corkboard();
    // Select "Beta" (chapter two's first scene) and move it up into chapter one.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('K')));

    assert_eq!(card_titles(&app), vec!["Alpha", "Beta"]);
    let Mode::Corkboard(board) = &app.mode else {
        panic!("board stays open");
    };
    assert_eq!(
        board.cards[1].chapter,
        Some(0),
        "Beta now belongs to chapter one"
    );
    assert_eq!(board.cards[board.selected].title, "Beta");

    // Chapter one gained Beta after Alpha; chapter two lost it.
    let one = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(one.find("## Alpha").unwrap() < one.find("## Beta").unwrap());
    assert!(
        one.contains("## Beta\n\nB."),
        "chapter one gained the prose: {one:?}"
    );
    let two = fs::read_to_string(dir.join("two.md")).unwrap();
    assert!(
        !two.contains("## Beta"),
        "the scene left chapter two: {two:?}"
    );
}

#[test]
fn cross_chapter_move_carries_the_scenes_metadata() {
    // The moved scene's synopsis + status travel with it into the destination's
    // `scenes:` front matter, and the source's block for it is dropped (#974).
    let (mut app, dir) = multi_scene_app(
        "corkboard-ms-cross-meta",
        "# Chapter One\n\n## Alpha\n\nA.\n",
        "# Chapter Two\n\n## Beta\n\nB.\n",
    );
    app.open_manuscript_corkboard();
    // Annotate Alpha (chapter one): a synopsis and a non-default status.
    app.on_key(key(KeyCode::Char('s'))); // Stub -> Draft
    app.on_key(key(KeyCode::Char('e')));
    type_str(&mut app, "the opening");
    app.on_key(key(KeyCode::Enter));
    // Re-select Alpha and move it down into chapter two.
    let alpha = card_titles(&app).iter().position(|t| t == "Alpha").unwrap();
    for _ in 0..alpha {
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Char('J')));

    // On the board, the moved Alpha card carries its metadata.
    let Mode::Corkboard(board) = &app.mode else {
        panic!("board stays open");
    };
    let moved = board.cards.iter().find(|c| c.title == "Alpha").unwrap();
    assert_eq!(moved.synopsis, "the opening");
    assert_eq!(moved.status, crate::frontmatter::SceneStatus::Draft);

    // On disk: chapter two carries Alpha's metadata; chapter one no longer does.
    let reopened = open_test_app(&dir);
    let two_meta = reopened.project.chapter_front_matter(1).unwrap().scenes();
    let alpha_meta = two_meta
        .iter()
        .find(|m| m.synopsis == "the opening")
        .unwrap();
    assert_eq!(alpha_meta.status, crate::frontmatter::SceneStatus::Draft);
    let one_meta = reopened
        .project
        .chapter_front_matter(0)
        .map(|f| f.scenes())
        .unwrap_or_default();
    assert!(
        !one_meta.iter().any(|m| m.synopsis == "the opening"),
        "chapter one no longer carries the moved scene's metadata: {one_meta:?}"
    );
}

#[test]
fn cross_chapter_move_is_atomic_on_a_write_failure() {
    // A write failure mid-move must leave *both* files byte-for-byte intact — no
    // partial state where one chapter changed and the other didn't (#262 / #974).
    let (mut app, dir) = multi_scene_app(
        "corkboard-ms-cross-atomic",
        "# Chapter One\n\n## Alpha\n\nA.\n",
        "# Chapter Two\n\n## Beta\n\nB.\n",
    );
    let one_before = fs::read_to_string(dir.join("one.md")).unwrap();
    let two_before = fs::read_to_string(dir.join("two.md")).unwrap();
    app.open_manuscript_corkboard();

    // Arm a failure on the second staged write of the move (the destination), after
    // the source's temp is already written but before either is renamed.
    crate::atomic::fail_point::arm_pre_rename_failure_on_nth(2);
    app.on_key(key(KeyCode::Char('J'))); // move Alpha down — this must fail

    // A message reports the failure, and neither file was rewritten.
    assert!(
        app.message.as_deref().unwrap_or("").contains("Move failed"),
        "the failure is reported: {:?}",
        app.message
    );
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), one_before);
    assert_eq!(fs::read_to_string(dir.join("two.md")).unwrap(), two_before);
    // No stray temp files were orphaned by the aborted move.
    let strays: Vec<_> = fs::read_dir(&*dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(strays.is_empty(), "temp file left behind: {strays:?}");
}

// A *hard* read failure on the destination chapter must ABORT the move — never
// compose the destination's new content onto an empty body, which would durably
// wipe its real prose while reporting success and capture that empty state in the
// undo (#565/#871). We isolate the *read* hazard (distinct from the write-side
// pre-flight) with a chmod-000 destination file: it is a *regular* file whose path
// a rename could still land on (so the swap pre-flight passes), but whose contents
// `read_to_string` cannot read — exactly the "unreadable but writable" case where
// the old swallowing read would blank the real prose. Unix-only (POSIX perms).
#[cfg(unix)]
#[test]
fn cross_chapter_move_aborts_when_the_destination_cannot_be_read() {
    use std::os::unix::fs::PermissionsExt;

    let dir = app_scratch_dir("corkboard-ms-cross-dest-read-fail");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "# Chapter One\n\n## Alpha\n\nA.\n").unwrap();
    // The destination carries real prose that must survive an aborted move.
    let two_body = "# Chapter Two\n\n## Beta\n\nBeta's real prose here.\n";
    fs::write(dir.join("two.md"), two_body).unwrap();
    let dir = TempProject::new(dir);
    let one_before = fs::read_to_string(dir.join("one.md")).unwrap();

    let mut app = open_test_app(&dir);
    app.open_manuscript_corkboard();
    // Make the destination unreadable *after* open (open cached its metadata). A
    // regular file with no read permission: readable as a path, unreadable as bytes.
    fs::set_permissions(dir.join("two.md"), fs::Permissions::from_mode(0o000)).unwrap();
    // Under root the read bit is bypassed, so the file stays readable and there's
    // nothing to assert — skip rather than false-pass. Checked *before* the move,
    // since a buggy blanking move replaces the file with a readable empty one.
    if fs::read_to_string(dir.join("two.md")).is_ok() {
        fs::set_permissions(dir.join("two.md"), fs::Permissions::from_mode(0o644)).unwrap();
        return;
    }

    // Alpha is chapter one's only scene; move it down into chapter two, whose file
    // can't be read. The move must abort rather than compose onto an empty body.
    app.on_key(key(KeyCode::Char('J')));

    // Restore perms so the file can be read back and the temp dir cleaned up.
    fs::set_permissions(dir.join("two.md"), fs::Permissions::from_mode(0o644)).unwrap();

    // A message reports the failure to read the chapter.
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Move failed: could not read chapter"),
        "the read failure aborts the move: {:?}",
        app.message
    );
    // The destination's real prose is byte-for-byte intact — NOT blanked.
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        two_body,
        "the destination's prose must survive an unreadable-destination move"
    );
    // The source chapter is unchanged too (its scene was NOT removed).
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), one_before);
    // Nothing was captured for undo — there is nothing to revert.
    assert!(app.cork_undo.is_none(), "an aborted move records no undo");
    // No stray temp files were orphaned by the aborted move.
    let strays: Vec<_> = fs::read_dir(&*dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
        .collect();
    assert!(strays.is_empty(), "temp file left behind: {strays:?}");
}

#[test]
fn cross_chapter_move_is_undoable_from_the_board() {
    // The board's `u` restores the prior arrangement of a cross-chapter move: both
    // files return to their exact pre-move contents (#974).
    let (mut app, dir) = multi_scene_app(
        "corkboard-ms-cross-undo",
        "# Chapter One\n\n## Alpha\n\nA.\n",
        "# Chapter Two\n\n## Beta\n\nB.\n",
    );
    let one_before = fs::read_to_string(dir.join("one.md")).unwrap();
    let two_before = fs::read_to_string(dir.join("two.md")).unwrap();
    app.open_manuscript_corkboard();
    app.on_key(key(KeyCode::Char('J'))); // move Alpha down into chapter two
                                         // The move happened.
    assert!(!fs::read_to_string(dir.join("one.md"))
        .unwrap()
        .contains("## Alpha"));

    app.on_key(key(KeyCode::Char('u'))); // undo it

    // Both files are back to exactly their pre-move contents, and the board reopens
    // on the restored scene in chapter one.
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), one_before);
    assert_eq!(fs::read_to_string(dir.join("two.md")).unwrap(), two_before);
    let Mode::Corkboard(board) = &app.mode else {
        panic!("board reopens after undo");
    };
    let alpha = board.cards.iter().find(|c| c.title == "Alpha").unwrap();
    assert_eq!(alpha.chapter, Some(0), "Alpha is back in chapter one");
}

#[test]
fn cross_chapter_move_is_a_noop_at_the_manuscript_ends() {
    // The very first scene can't move up (no chapter before it) and the very last
    // can't move down (none after) — a no-op that leaves both files untouched (#974).
    let (mut app, dir) = multi_scene_app(
        "corkboard-ms-cross-ends",
        "# Chapter One\n\n## Alpha\n\nA.\n",
        "# Chapter Two\n\n## Beta\n\nB.\n",
    );
    let one_before = fs::read_to_string(dir.join("one.md")).unwrap();
    let two_before = fs::read_to_string(dir.join("two.md")).unwrap();
    app.open_manuscript_corkboard();
    app.on_key(key(KeyCode::Char('K'))); // Alpha up — nothing above chapter one
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Char('J'))); // Beta down — nothing below chapter two

    assert_eq!(card_titles(&app), vec!["Alpha", "Beta"]);
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), one_before);
    assert_eq!(fs::read_to_string(dir.join("two.md")).unwrap(), two_before);
}

// --- A focused reference pane is not the active chapter (#165 / #1167 follow-up)

/// A two-chapter scene project with one bible entry, opened with that bible entry
/// focused in a reference pane (#165) — the state where `edit_buffer()` is the
/// bible, not the active chapter. The active chapter stays chapter one, whose scene
/// is "The Arrival"; the bible's own `##` sections are "Physical" / "Voice", so a
/// board that mistook the pane for the chapter would card *those* up.
fn bible_focused_scene_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("one.md"),
        "# Chapter One\n\n## The Arrival\n\nRain.\n",
    )
    .unwrap();
    fs::write(
        dir.join("two.md"),
        "# Chapter Two\n\n## The Departure\n\nSmoke.\n",
    )
    .unwrap();
    fs::create_dir_all(dir.join("bible")).unwrap();
    fs::write(
        dir.join("bible").join("mara.md"),
        "# Mara\n\n## Physical\n\ntall\n\n## Voice\n\nlow\n",
    )
    .unwrap();
    let mut app = open_test_app(&dir);
    app.on_key(ctrl('g')); // chapter browser (reveals the reference-file switch)
    app.on_key(key(KeyCode::Char('f'))); // reference files
    app.on_key(key(KeyCode::Enter)); // open the (only) bible entry in a pane
    app.on_key(ctrl('w')); // Ctrl-w window prefix …
    app.on_key(key(KeyCode::Char('w'))); // … focus the bible pane
    (app, TempProject::new(dir))
}

#[test]
fn per_chapter_corkboard_refuses_a_focused_reference_pane() {
    let (mut app, _dir) = bible_focused_scene_app("corkboard-ref-per-chapter");
    assert!(
        !app.focus_shows_active_chapter(),
        "the bible reference pane is focused, not the active chapter"
    );
    app.open_corkboard();
    // The board must not open on the focused bible's `##` sections; a message points
    // the author back to a chapter (the board edits scenes into the live buffer).
    assert!(
        !matches!(app.mode, Mode::Corkboard(_)),
        "the per-chapter board must not open on a reference pane"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Focus a chapter"),
        "message: {:?}",
        app.message
    );
}

#[test]
fn manuscript_board_reads_the_active_chapter_from_disk_not_a_focused_reference_pane() {
    let (mut app, _dir) = bible_focused_scene_app("corkboard-ref-ms-read");
    app.open_manuscript_corkboard();
    let Mode::Corkboard(board) = &app.mode else {
        panic!("the manuscript board opens even with a reference pane focused");
    };
    // The active chapter (chapter one) is carded from its file on disk, not the
    // focused bible — its scene "The Arrival", never the bible's "Physical"/"Voice".
    let titles: Vec<&str> = board.cards.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, vec!["The Arrival", "The Departure"]);
    assert!(
        !titles.iter().any(|t| *t == "Physical" || *t == "Voice"),
        "the focused bible's sections must not be carded as chapter scenes: {titles:?}"
    );
    assert_eq!(board.cards[0].chapter, Some(0));
}

#[test]
fn manuscript_board_edit_wont_mutate_a_focused_reference_pane() {
    let (mut app, dir) = bible_focused_scene_app("corkboard-ref-ms-edit");
    let mara_path = dir.join("bible").join("mara.md");
    let mara_before = fs::read_to_string(&mara_path).unwrap();
    app.open_manuscript_corkboard();
    // The first card is the active chapter's scene ("The Arrival"). Cycling its
    // status must be refused, not written as `scenes:` metadata into the focused
    // bible file (which `edit_buffer()`/`self.buffer` currently holds).
    assert_eq!(card_titles(&app)[0], "The Arrival");
    app.on_key(key(KeyCode::Char('s')));
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Focus a chapter"),
        "the edit is refused: {:?}",
        app.message
    );
    assert_eq!(
        fs::read_to_string(&mara_path).unwrap(),
        mara_before,
        "the bible file must be untouched — no scene metadata written into it"
    );
    assert!(
        !mara_before.contains("scenes:"),
        "sanity: the bible had no scene metadata to begin with"
    );
}
