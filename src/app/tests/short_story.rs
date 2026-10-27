//! The short story form in the editor (#1239): no chapter list, no chapter
//! structure commands, the title edit keeping the story's chapter title in step,
//! and the length category beside the word count.

use super::*;
use crate::mode::{Action, ListFilter, MetaField, Mode};
use crate::project::{Project, ProjectTemplate};
use crate::ui::render_to_string;

/// A short story created from the template with `body` as its prose.
fn story_app(name: &str, body: &str) -> (App, TempProject) {
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
