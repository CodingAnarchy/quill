//! Tests for collections (#1244): linking existing short stories by relative
//! path, writing a new story inside, reordering, crediting, unlinking (never
//! deleting), a missing story and relocating it, and the cached story details
//! following edits made in the story itself.

use std::time::Instant;

use super::*;
use crate::project::collection::relative_path;

/// Find the "Collection" template by its form.
fn collection_template() -> &'static ProjectTemplate {
    TEMPLATES
        .iter()
        .find(|t| t.form == Form::Collection)
        .expect("a collection template is offered")
}

/// A writing folder holding a collection and two sibling short stories.
struct Shelf {
    root: PathBuf,
}

impl Shelf {
    fn new(name: &str) -> Self {
        Self {
            root: temp_dir(name),
        }
    }

    /// A short story at `<root>/<folder>` titled `title` with `body`.
    fn story(&self, folder: &str, title: &str, body: &str) -> PathBuf {
        let dir = self.root.join(folder);
        let mut story =
            Project::create_with_template(&dir, ProjectTemplate::short_story()).unwrap();
        story.manifest.title = title.into();
        story.manifest.sync_story_title();
        story.save_manifest().unwrap();
        fs::write(story.active_path(), body).unwrap();
        dir
    }

    fn collection(&self) -> Project {
        Project::create_with_template(self.root.join("collected"), collection_template()).unwrap()
    }
}

impl Drop for Shelf {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

#[test]
fn a_new_collection_has_an_introduction_and_no_stories() {
    let shelf = Shelf::new("coll-create");
    let c = shelf.collection();
    assert!(c.is_collection());
    assert_eq!(c.manifest.chapters[0].title, "Introduction");
    assert_eq!(
        c.manifest.chapters[0].role,
        Some(crate::frontmatter::Role::Introduction)
    );
    assert!(c.manifest.stories.is_empty());
    let text = fs::read_to_string(Project::manifest_path(&c.root)).unwrap();
    assert!(text.contains("form = \"collection\""), "{text}");
}

#[test]
fn linking_a_story_stores_a_relative_path_and_its_details() {
    let shelf = Shelf::new("coll-link");
    let bell = shelf.story("bell", "The Drowned Bell", "It rang at low tide.\n");
    let mut c = shelf.collection();
    let i = c.add_story(&bell).unwrap();
    assert_eq!(c.manifest.stories[i].path, "../bell");
    let info = c.manifest.stories[i].info.clone().unwrap();
    assert_eq!((info.title.as_str(), info.words), ("The Drowned Bell", 5));

    // It survives a reopen, and linking it twice is refused.
    let mut c = Project::open(&c.root).unwrap();
    assert_eq!(c.manifest.stories.len(), 1);
    assert_eq!(c.collection_words(), 5);
    assert!(c.add_story(&bell).is_err());
}

#[test]
fn only_short_stories_can_be_linked() {
    let shelf = Shelf::new("coll-novel");
    let novel = shelf.root.join("novel");
    drop(Project::create(&novel).unwrap());
    let mut c = shelf.collection();
    let err = c.add_story(&novel).unwrap_err().to_string();
    assert!(err.contains("isn't a short story"), "{err}");
    assert!(c.add_story(&shelf.root.join("nowhere")).is_err());
    assert!(c.manifest.stories.is_empty());
}

#[test]
fn a_new_story_is_written_inside_the_collection_and_linked() {
    let shelf = Shelf::new("coll-new");
    let mut c = shelf.collection();
    c.manifest.author = Some("Mara Quill".into());
    let dir = c.create_story("Salt Wind").unwrap();
    assert_eq!(dir, c.root.join("stories").join("Salt Wind"));
    let story = Project::open(&dir).unwrap();
    assert!(story.manifest.is_short_story());
    assert_eq!(story.manifest.title, "Salt Wind");
    assert_eq!(story.manifest.author.as_deref(), Some("Mara Quill"));
    assert_eq!(c.manifest.stories[0].path, "stories/Salt Wind");
}

#[test]
fn stories_reorder_credit_and_unlink_without_touching_the_story() {
    let shelf = Shelf::new("coll-edit");
    let a = shelf.story("a", "A", "one\n");
    let b = shelf.story("b", "B", "two\n");
    let mut c = shelf.collection();
    c.add_story(&a).unwrap();
    c.add_story(&b).unwrap();
    assert_eq!(c.move_story(1, true).unwrap(), Some(0));
    assert_eq!(c.move_story(0, true).unwrap(), None, "already first");
    c.set_story_credit(0, "First published in Lightspeed, 2027")
        .unwrap();
    let c2 = Project::open(&c.root).unwrap();
    let titles: Vec<&str> = c2
        .manifest
        .stories
        .iter()
        .map(|s| s.info.as_ref().unwrap().title.as_str())
        .collect();
    assert_eq!(titles, vec!["B", "A"]);
    assert_eq!(
        c2.manifest.stories[0].credit.as_deref(),
        Some("First published in Lightspeed, 2027")
    );
    // The credit lives in the collection; the story's own manifest is untouched.
    let story_toml = fs::read_to_string(b.join(MANIFEST_NAME)).unwrap();
    assert!(!story_toml.contains("Lightspeed"));

    c.remove_story(0).unwrap();
    assert_eq!(c.manifest.stories.len(), 1);
    assert!(
        b.join(MANIFEST_NAME).exists(),
        "unlinking never deletes the story"
    );
}

#[test]
fn a_missing_story_is_reported_and_can_be_relocated() {
    let shelf = Shelf::new("coll-missing");
    let a = shelf.story("a", "A", "one two\n");
    let mut c = shelf.collection();
    c.add_story(&a).unwrap();
    let moved = shelf.root.join("archive-a");
    fs::rename(&a, &moved).unwrap();

    let mut c = Project::open(&c.root).unwrap();
    assert!(c.manifest.stories[0].info.is_none(), "missing is visible");
    c.relocate_story(0, &moved).unwrap();
    assert_eq!(c.manifest.stories[0].path, "../archive-a");
    assert_eq!(c.manifest.stories[0].info.as_ref().unwrap().words, 2);
}

#[test]
fn an_edit_in_the_story_shows_in_the_collection() {
    let shelf = Shelf::new("coll-follow");
    let a = shelf.story("a", "A", "one\n");
    let mut c = shelf.collection();
    c.add_story(&a).unwrap();
    fs::write(a.join("story.md"), "one two three\n").unwrap();
    c.refresh_stories();
    assert_eq!(c.collection_words(), 3);
}

#[test]
fn relative_paths_walk_up_and_down() {
    let shelf = Shelf::new("coll-rel");
    let from = shelf.root.join("x").join("collection");
    let to = shelf.root.join("y").join("story");
    fs::create_dir_all(&from).unwrap();
    fs::create_dir_all(&to).unwrap();
    assert_eq!(relative_path(&from, &to), "../../y/story");
    assert_eq!(relative_path(&from, &from.join("stories")), "stories");
}

/// Opening a collection reads each linked story once (manifest + story file) —
/// a ~50-story collection opens near-instantly. Timing only means anything in a
/// release build, so it's `#[ignore]`d with the other perf guards.
#[test]
#[ignore = "perf: run with `cargo perf`"]
fn open_is_fast_for_a_large_collection() {
    let shelf = Shelf::new("coll-perf");
    let body = "The bell rang at low tide and nobody came. ".repeat(600);
    let mut c = shelf.collection();
    for i in 0..50 {
        let dir = shelf.story(&format!("s{i}"), &format!("Story {i}"), &body);
        c.add_story(&dir).unwrap();
    }
    let root = c.root.clone();
    drop(c);
    let start = Instant::now();
    let c = Project::open(&root).unwrap();
    let elapsed = start.elapsed();
    assert_eq!(c.manifest.stories.len(), 50);
    println!("open a 50-story collection: {elapsed:?}");
    assert!(elapsed.as_millis() < 50, "took {elapsed:?}");
}
