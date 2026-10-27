//! A collection's story list in the editor (#1244): Ctrl-G lists the linked
//! stories with their words and totals, opening one edits the story's own
//! project, and adding, writing, crediting, and unlinking go through prompts.

use super::*;
use crate::mode::{Command, Mode, StoryCommand};
use crate::project::{Form, Project, ProjectTemplate, TEMPLATES};

/// A collection at `<root>/collected` with one linked story at `<root>/bell`.
fn collection_app(name: &str) -> (App, TempProject) {
    let root = app_scratch_dir(name);
    let bell = root.join("bell");
    let mut story = Project::create_with_template(&bell, ProjectTemplate::short_story()).unwrap();
    story.manifest.title = "The Drowned Bell".into();
    story.manifest.sync_story_title();
    story.save_manifest().unwrap();
    fs::write(story.active_path(), "It rang at low tide.\n").unwrap();
    drop(story);
    let template = TEMPLATES
        .iter()
        .find(|t| t.form == Form::Collection)
        .unwrap();
    let mut c = Project::create_with_template(root.join("collected"), template).unwrap();
    c.add_story(&bell).unwrap();
    drop(c);
    let app = open_test_app(&root.join("collected"));
    (app, TempProject::new(root))
}

/// The open palette's item labels.
fn labels(app: &App) -> Vec<String> {
    let Mode::Palette(p) = &app.mode else {
        panic!("the story list should be open");
    };
    p.items.iter().map(|i| i.label.clone()).collect()
}

#[test]
fn ctrl_g_lists_the_collections_stories_with_their_words() {
    let (mut app, _root) = collection_app("coll-list");
    app.on_key(ctrl('g'));
    let rows = labels(&app);
    assert_eq!(rows[0], "1. The Drowned Bell");
    let Mode::Palette(p) = &app.mode else {
        unreachable!()
    };
    assert!(
        p.items[0].desc.starts_with("5 words"),
        "{}",
        p.items[0].desc
    );
    assert!(rows.iter().any(|r| r == "Add an existing story…"));
    assert!(rows.iter().any(|r| r == "Write a new story…"));
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("1 story · 5 words"));
}

#[test]
fn opening_a_story_edits_the_story_project_itself() {
    let (mut app, root) = collection_app("coll-open");
    app.run_story_command(StoryCommand::Open(0));
    let bell = fs::canonicalize(root.join("bell")).unwrap();
    assert_eq!(fs::canonicalize(&app.project.root).unwrap(), bell);
    assert!(app.project.manifest.is_short_story());
}

#[test]
fn a_credit_line_is_kept_in_the_collection() {
    let (mut app, root) = collection_app("coll-credit");
    app.run_story_command(StoryCommand::Credit(0));
    for c in "Lightspeed, 2027".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.project.manifest.stories[0].credit.as_deref(),
        Some("Lightspeed, 2027")
    );
    assert!(!fs::read_to_string(root.join("bell/book.toml"))
        .unwrap()
        .contains("Lightspeed"));
}

#[test]
fn removing_a_story_is_confirmed_and_keeps_the_story() {
    let (mut app, root) = collection_app("coll-remove");
    app.run_story_command(StoryCommand::Remove(0));
    app.on_key(key(KeyCode::Char('y')));
    assert!(app.project.manifest.stories.is_empty());
    assert!(root.join("bell/book.toml").exists());
}

#[test]
fn writing_a_new_story_creates_and_opens_it() {
    let (mut app, root) = collection_app("coll-new");
    app.run_story_command(StoryCommand::New);
    for c in "Salt Wind".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter));
    assert!(app.project.manifest.is_short_story());
    assert_eq!(app.project.manifest.title, "Salt Wind");
    let collected = Project::open(root.join("collected")).unwrap();
    assert_eq!(collected.manifest.stories.len(), 2);
}

#[test]
fn a_missing_story_offers_to_be_located() {
    let (mut app, root) = collection_app("coll-locate");
    fs::rename(root.join("bell"), root.join("moved-bell")).unwrap();
    app.project.refresh_stories();
    app.on_key(ctrl('g'));
    let Mode::Palette(p) = &app.mode else {
        panic!("the story list should be open");
    };
    assert!(p.items[0].label.contains("(missing)"));
    assert!(matches!(
        p.items[0].command,
        Command::Story(StoryCommand::Locate(0))
    ));
    app.locate_story(0, root.join("moved-bell").to_str().unwrap());
    assert_eq!(app.project.manifest.stories[0].path, "../moved-bell");
}

#[test]
fn the_stories_action_is_only_for_collections() {
    let (app, _dir) = two_chapter_app("coll-novel-action");
    assert!(app
        .action_status(crate::mode::Action::CollectionStories)
        .is_some());
}
