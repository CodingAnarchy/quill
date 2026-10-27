//! The short story form in the editor (#1239): no chapter list, no chapter
//! structure commands, the title edit keeping the story's chapter title in step,
//! and the length category beside the word count.

use super::*;
use crate::mode::{Action, ListFilter, MetaField, Mode};
use crate::project::{Project, ProjectTemplate};
use crate::ui::render_to_string;

/// A short story created from the template with `body` as its prose.
pub(super) fn story_app(name: &str, body: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    let project = Project::create_with_template(&dir, ProjectTemplate::short_story()).unwrap();
    fs::write(project.active_path(), body).unwrap();
    drop(project);
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn chapter_structure_actions_are_hidden_in_a_short_story() {
    let (app, _dir) = story_app("story-actions", "It rang at low tide.\n");
    for action in [
        Action::SplitChapter,
        Action::MergeChapter,
        Action::ManuscriptCorkboard,
        Action::ManageBooks,
        Action::SeriesTree,
    ] {
        assert!(
            app.action_status(action).is_some(),
            "{} is not offered in a short story",
            action.name()
        );
    }
    assert!(app.action_status(Action::Save).is_none());
}

#[test]
fn the_continuous_view_does_not_open_for_a_short_story() {
    let (mut app, _dir) = story_app("story-continuous", "It rang.\n");
    app.on_key(ctrl('l'));
    assert!(app.manuscript.is_none());
    assert!(app.message.as_deref().unwrap_or("").contains("short story"));
}

#[test]
fn ctrl_g_focuses_the_scene_outline_rather_than_a_chapter_browser() {
    let (mut app, _dir) = story_app(
        "story-ctrl-g",
        "## Shore\n\nWaves.\n\n## Bell\n\nIt rang.\n",
    );
    app.on_key(ctrl('g'));
    assert!(
        matches!(app.mode, Mode::Outline { .. }),
        "Ctrl-G opens the outline, not Nav"
    );
}

#[test]
fn chapter_editing_keys_are_refused_in_the_browser() {
    let (mut app, _dir) = story_app("story-nav-keys", "It rang.\n");
    // The browser is still reachable (by mouse, say); its structure keys aren't.
    app.mode = Mode::Nav {
        selected: 0,
        filter: ListFilter::default(),
    };
    app.on_key(key(KeyCode::Char('a')));
    assert!(
        matches!(app.mode, Mode::Nav { .. }),
        "no new-chapter prompt"
    );
    assert_eq!(app.project.manifest.chapters.len(), 1);
    assert!(app.message.as_deref().unwrap_or("").contains("no chapters"));
}

#[test]
fn renaming_the_story_renames_its_chapter() {
    let (mut app, _dir) = story_app("story-rename", "It rang.\n");
    app.apply_meta(MetaField::BookTitle, "The Drowned Bell");
    assert_eq!(app.project.manifest.title, "The Drowned Bell");
    assert_eq!(app.project.active_title(), Some("The Drowned Bell"));
}

#[test]
fn the_status_bar_names_the_length_category() {
    let (app, _dir) = story_app("story-status", "It rang at low tide.\n");
    let screen = render_to_string(&app, 100, 10);
    assert!(screen.contains("5 words · flash fiction"), "{screen}");
}

#[test]
fn the_outline_pane_shows_scenes_not_a_chapter_list() {
    let (mut app, _dir) = story_app("story-pane", "## Shore\n\nWaves.\n");
    app.outline_visible = true;
    let screen = render_to_string(&app, 80, 12);
    assert!(screen.contains("SCENES"), "{screen}");
    assert!(!screen.contains("CHAPTERS"), "{screen}");
    assert!(screen.contains("Shore"), "{screen}");
}

// --- #1240: scenes split by scene breaks, headings optional ---------------

/// A three-scene story separated by breaks, with no headings at all.
const UNTITLED: &str =
    "The bell rang at low tide.\n\n* * *\n\nNobody came.\n\n* * *\n\nThe sea took it back.\n";

/// The open corkboard's card titles.
fn card_titles(app: &App) -> Vec<String> {
    let Mode::Corkboard(board) = &app.mode else {
        panic!("the corkboard should be open");
    };
    board.cards.iter().map(|c| c.title.clone()).collect()
}

#[test]
fn an_untitled_storys_corkboard_has_a_card_per_scene() {
    let (mut app, _dir) = story_app("story-cards", UNTITLED);
    app.open_corkboard();
    assert_eq!(
        card_titles(&app),
        vec![
            "The bell rang at low tide.",
            "Nobody came.",
            "The sea took it back."
        ]
    );
}

#[test]
fn a_story_with_no_breaks_is_one_scene_on_the_corkboard() {
    let (mut app, _dir) = story_app("story-one-card", "It rang.\n\nNobody came.\n");
    app.open_corkboard();
    assert_eq!(card_titles(&app), vec!["It rang."]);
}

#[test]
fn moving_a_scene_on_the_corkboard_keeps_one_break_between_scenes() {
    let (mut app, dir) = story_app("story-reorder", UNTITLED);
    app.open_corkboard();
    app.on_key(key(KeyCode::Char('J'))); // first scene down
    let saved = fs::read_to_string(dir.join("story.md")).unwrap();
    assert_eq!(
        saved,
        "Nobody came.\n\n* * *\n\nThe bell rang at low tide.\n\n* * *\n\nThe sea took it back.\n"
    );
    // Moving the last scene up never leaves a break at either end.
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('K')));
    let saved = fs::read_to_string(dir.join("story.md")).unwrap();
    assert!(!saved.starts_with("* * *") && !saved.trim_end().ends_with("* * *"));
    assert_eq!(saved.matches("* * *").count(), 2, "{saved}");
}

#[test]
fn a_scene_synopsis_follows_its_scene_through_a_move_and_a_reopen() {
    let (mut app, dir) = story_app("story-synopsis", UNTITLED);
    app.open_corkboard();
    app.apply_scene_synopsis(None, 0, false, "The bell rings");
    app.on_key(key(KeyCode::Char('J'))); // the first scene moves to second
    drop(app);
    let mut app = open_test_app(&dir);
    app.open_corkboard();
    let Mode::Corkboard(board) = &app.mode else {
        panic!("the corkboard should be open");
    };
    assert_eq!(board.cards[1].title, "The bell rang at low tide.");
    assert_eq!(board.cards[1].synopsis, "The bell rings");
    assert_eq!(board.cards[0].synopsis, "");
}

#[test]
fn typing_a_new_break_adds_a_blank_scene_without_shifting_notes() {
    let (mut app, _dir) = story_app("story-insert", UNTITLED);
    app.open_corkboard();
    app.apply_scene_synopsis(None, 1, false, "Waiting");
    app.mode = Mode::Insert;
    // Split the first scene by typing a break into it.
    let at = "The bell rang at low tide.\n".len();
    app.buffer
        .replace_byte_range(at, at, "\n* * *\n\nA gull watched.\n");
    app.buffer.refresh_syntax();
    app.open_corkboard();
    let Mode::Corkboard(board) = &app.mode else {
        panic!("the corkboard should be open");
    };
    let notes: Vec<(&str, &str)> = board
        .cards
        .iter()
        .map(|c| (c.title.as_str(), c.synopsis.as_str()))
        .collect();
    assert_eq!(
        notes,
        vec![
            ("The bell rang at low tide.", ""),
            ("A gull watched.", ""),
            ("Nobody came.", "Waiting"),
            ("The sea took it back.", ""),
        ]
    );
}

#[test]
fn the_outline_and_quick_switch_list_a_storys_scenes() {
    let (mut app, _dir) = story_app("story-outline", UNTITLED);
    let titles: Vec<String> = app
        .outline_entries()
        .iter()
        .map(|h| h.title.clone())
        .collect();
    assert_eq!(titles.len(), 3);
    assert_eq!(titles[1], "Nobody came.");

    // Ctrl-G focuses the scene outline; Enter jumps to the selected scene.
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    let expected = UNTITLED.find("Nobody came.").unwrap();
    assert_eq!(app.buffer.cursor_byte(), expected);

    app.open_quick_switch();
    let Mode::QuickSwitch(s) = &app.mode else {
        panic!("quick switch should be open");
    };
    let scenes: Vec<String> = s
        .matches()
        .iter()
        .filter(|(_, t)| t.is_scene)
        .map(|(_, t)| t.title.clone())
        .collect();
    assert_eq!(scenes, titles);
}

#[test]
fn the_focused_outline_moves_story_scenes() {
    let (mut app, dir) = story_app("story-outline-move", UNTITLED);
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('J')));
    let saved = fs::read_to_string(dir.join("story.md")).unwrap();
    assert!(saved.starts_with("Nobody came."), "{saved}");
    app.on_key(key(KeyCode::Char('>')));
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("no heading levels"));
}
