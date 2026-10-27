//! A collection's story list in the editor (#1244): Ctrl-G lists the linked
//! stories with their words and totals, opening one edits the story's own
//! project, and adding, writing, crediting, and unlinking go through prompts.

use std::io::{Cursor, Read};

use super::*;
use crate::mode::{Command, ExportFormat, ExportScope, Mode, StoryCommand};
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

// --- #1245: a collection exports as one volume ---------------------------

/// [`collection_app`] plus a second story with a scene break, a credit line on
/// the first story, and an introduction.
fn full_collection_app(name: &str) -> (App, TempProject) {
    let (app, root) = collection_app(name);
    drop(app);
    let tide = root.join("tide");
    let mut story = Project::create_with_template(&tide, ProjectTemplate::short_story()).unwrap();
    story.manifest.title = "Tide".into();
    story.manifest.sync_story_title();
    story.save_manifest().unwrap();
    fs::write(story.active_path(), "The sea rose.\n\n* * *\n\nIt fell.\n").unwrap();
    drop(story);
    let mut collected = Project::open(root.join("collected")).unwrap();
    collected.add_story(&tide).unwrap();
    collected
        .set_story_credit(0, "First published in Lightspeed, 2027")
        .unwrap();
    drop(collected);
    fs::write(
        root.join("collected/introduction.md"),
        "---\nrole: introduction\n---\nThese stories are about the sea.\n",
    )
    .unwrap();
    (open_test_app(&root.join("collected")), root)
}

/// The text of the one export file with extension `ext`.
fn exported(root: &Path, ext: &str) -> Vec<u8> {
    let dir = root.join("collected/export");
    let path = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == ext))
        .unwrap_or_else(|| panic!("no .{ext} in {}", dir.display()));
    fs::read(path).unwrap()
}

#[test]
fn a_collection_exports_its_pages_and_stories_in_order() {
    let (mut app, root) = full_collection_app("coll-export-md");
    app.run_export(ExportFormat::Markdown, ExportScope::ActiveBook);
    let md = String::from_utf8(exported(&root, "md")).unwrap();
    let intro = md.find("These stories are about the sea.").unwrap();
    let bell = md.find("## The Drowned Bell").unwrap();
    let credit = md.find("First published in Lightspeed, 2027").unwrap();
    let bell_text = md.find("It rang at low tide.").unwrap();
    let tide = md.find("## Tide").unwrap();
    assert!(
        intro < bell && bell < credit && credit < bell_text && bell_text < tide,
        "{md}"
    );
    assert!(md.contains("* * *"), "scene breaks pass through: {md}");
}

#[test]
fn an_edit_to_a_story_shows_in_the_next_export() {
    let (mut app, root) = full_collection_app("coll-export-fresh");
    app.run_export(ExportFormat::Markdown, ExportScope::ActiveBook);
    fs::write(root.join("tide/story.md"), "The sea rose higher.\n").unwrap();
    app.run_export(ExportFormat::Markdown, ExportScope::ActiveBook);
    let md = String::from_utf8(exported(&root, "md")).unwrap();
    assert!(md.contains("The sea rose higher."), "{md}");
}

#[test]
fn the_word_and_epub_exports_list_every_story() {
    let (mut app, root) = full_collection_app("coll-export-docx");
    app.run_export(ExportFormat::Docx, ExportScope::ActiveBook);
    let docx = exported(&root, "docx");
    let mut zip = zip::ZipArchive::new(Cursor::new(docx)).unwrap();
    let mut xml = String::new();
    zip.by_name("word/document.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    assert!(xml.contains("TOC \\o"), "a table of contents");
    assert!(xml.contains("The Drowned Bell") && xml.contains("Tide"));

    app.run_export(ExportFormat::Epub, ExportScope::ActiveBook);
    let epub = exported(&root, "epub");
    let mut zip = zip::ZipArchive::new(Cursor::new(epub)).unwrap();
    let nav_name = zip
        .file_names()
        .find(|n| n.contains("nav"))
        .unwrap()
        .to_string();
    let mut nav = String::new();
    zip.by_name(&nav_name)
        .unwrap()
        .read_to_string(&mut nav)
        .unwrap();
    assert!(
        nav.contains("The Drowned Bell") && nav.contains("Tide"),
        "{nav}"
    );
}

#[test]
fn a_missing_story_is_left_out_and_named_in_the_message() {
    let (mut app, root) = full_collection_app("coll-export-missing");
    fs::rename(root.join("tide"), root.join("gone")).unwrap();
    app.run_export(ExportFormat::Markdown, ExportScope::ActiveBook);
    let md = String::from_utf8(exported(&root, "md")).unwrap();
    assert!(!md.contains("The sea rose."));
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("1 missing story"));
}
