//! Converting between a novel and a short story: a one-chapter novel becomes a
//! short story (#1243), and a story grows into a novel, a chapter per scene
//! (#1242) — each confirmed first and preceded by a restore point.

use super::short_story::story_app;
use super::*;
use crate::config;
use crate::mode::{Action, Mode};
use crate::project::{Project, ProjectTemplate};

// --- #1243: convert a one-chapter novel into a short story ---------------

/// A novel project from a hand-written manifest plus `files` (name, contents).
fn novel_app(name: &str, manifest: &str, files: &[(&str, &str)]) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    for (file, body) in files {
        fs::write(dir.join(file), body).unwrap();
    }
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

const ONE_CHAPTER: &str =
    "title = \"Tide\"\n\n[[chapters]]\ntitle = \"Chapter 1\"\nfile = \"chapter-1.md\"\n";

#[test]
fn a_one_chapter_novel_converts_to_a_short_story_with_a_backup() {
    let data = app_scratch_dir("story-convert-data");
    config::set_data_dir(Some(data.clone()));
    let (mut app, dir) = novel_app(
        "story-convert",
        ONE_CHAPTER,
        &[("chapter-1.md", "The bell rang.\n")],
    );
    app.run_action(Action::ConvertToShortStory, false);
    assert!(matches!(app.mode, Mode::Prompt(_)), "a confirm comes first");
    app.on_key(key(KeyCode::Char('y')));

    assert!(app.project.manifest.is_short_story());
    assert_eq!(app.project.active_title(), Some("Tide"));
    assert_eq!(
        fs::read_to_string(dir.join("chapter-1.md")).unwrap(),
        "The bell rang.\n",
        "the prose is unchanged"
    );
    let reopened = Project::open(&dir).unwrap();
    assert!(reopened.manifest.is_short_story());
    // A restore point was archived before anything changed.
    let backups: Vec<_> = fs::read_dir(data.join("backups"))
        .unwrap()
        .flat_map(|d| fs::read_dir(d.unwrap().path()).unwrap())
        .map(|f| f.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        backups.iter().any(|f| f.starts_with("pre-short-story-")),
        "{backups:?}"
    );
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("Restore point"));
    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn declining_the_conversion_leaves_the_novel_alone() {
    let (mut app, _dir) = novel_app(
        "story-convert-no",
        ONE_CHAPTER,
        &[("chapter-1.md", "The bell rang.\n")],
    );
    app.run_action(Action::ConvertToShortStory, false);
    app.on_key(key(KeyCode::Char('n')));
    assert!(!app.project.manifest.is_short_story());
}

#[test]
fn a_novel_with_several_chapters_or_parts_cannot_convert_and_says_why() {
    let (mut app, _dir) = two_chapter_app("story-convert-two");
    app.run_action(Action::ConvertToShortStory, false);
    assert!(!matches!(app.mode, Mode::Prompt(_)));
    assert!(!app.project.manifest.is_short_story());
    let msg = app.message.clone().unwrap_or_default();
    assert!(msg.contains("has 2"), "{msg}");

    let (mut app, _dir) = novel_app(
        "story-convert-parts",
        "title = \"Tide\"\n\n[[parts]]\nname = \"One\"\nchapters = [1, 1]\n\n\
         [[chapters]]\ntitle = \"C\"\nfile = \"c.md\"\n",
        &[("c.md", "x\n")],
    );
    app.run_action(Action::ConvertToShortStory, false);
    assert!(!app.project.manifest.is_short_story());
    assert!(app.message.as_deref().unwrap_or("").contains("parts"));
}

#[test]
fn front_matter_sections_stay_around_the_converted_story() {
    let data = app_scratch_dir("story-convert-sections-data");
    config::set_data_dir(Some(data.clone()));
    let (mut app, _dir) = novel_app(
        "story-convert-sections",
        "title = \"Tide\"\n\n[[chapters]]\ntitle = \"For M\"\nfile = \"ded.md\"\n\n\
         [[chapters]]\ntitle = \"Chapter 1\"\nfile = \"c.md\"\n",
        &[
            ("ded.md", "---\nrole: dedication\n---\nFor M.\n"),
            ("c.md", "The bell rang.\n"),
        ],
    );
    app.run_action(Action::ConvertToShortStory, false);
    app.on_key(key(KeyCode::Char('y')));
    assert!(app.project.manifest.is_short_story());
    assert_eq!(app.project.manifest.chapters[0].title, "For M", "kept");
    assert_eq!(app.project.manifest.chapters[1].title, "Tide");
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("front and back matter"));
    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn a_versioned_project_records_a_milestone_before_converting() {
    let (mut app, dir) = novel_app(
        "story-convert-vcs",
        ONE_CHAPTER,
        &[("chapter-1.md", "The bell rang.\n")],
    );
    crate::vcs::init(&dir).unwrap();
    app.run_action(Action::ConvertToShortStory, false);
    app.on_key(key(KeyCode::Char('y')));
    assert!(app.project.manifest.is_short_story());
    let tags = crate::vcs::list_tags(&dir).unwrap();
    assert!(tags.iter().any(|t| t.name.starts_with("pre-short-story-")));
}

// --- #1242: grow a short story into a novel, a chapter per scene ---------

/// A story with an untitled scene, a headed scene behind a break, and an
/// untitled last scene.
const GROWING: &str =
    "The bell rang.\n\n* * *\n\n## Shore\n\nWaves came in.\n\n* * *\n\nThe sea took it.\n";

/// Grow `app`'s story into a novel through the command and its confirm.
fn grow(app: &mut App) {
    app.run_action(Action::GrowIntoNovel, false);
    assert!(matches!(app.mode, Mode::Prompt(_)), "a confirm comes first");
    app.on_key(key(KeyCode::Char('y')));
}

#[test]
fn growing_a_story_makes_a_chapter_per_scene() {
    let data = app_scratch_dir("story-grow-data");
    config::set_data_dir(Some(data.clone()));
    let (mut app, dir) = story_app("story-grow", GROWING);
    grow(&mut app);

    assert!(!app.project.manifest.is_short_story());
    let titles: Vec<&str> = app
        .project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(titles, vec!["Chapter 1", "Shore", "Chapter 3"]);
    let bodies: Vec<String> = (0..3).map(|i| app.project.chapter_body(i)).collect();
    assert_eq!(
        bodies,
        vec![
            "The bell rang.\n".to_string(),
            "## Shore\n\nWaves came in.\n".to_string(),
            "The sea took it.\n".to_string(),
        ]
    );
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.form, crate::project::Form::Novel);
    assert_eq!(reopened.manifest.chapters.len(), 3);
    assert!(app.message.as_deref().unwrap_or("").contains("3 chapters"));
    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn scene_notes_become_chapter_summaries_and_statuses() {
    let data = app_scratch_dir("story-grow-notes-data");
    config::set_data_dir(Some(data.clone()));
    let (mut app, _dir) = story_app("story-grow-notes", GROWING);
    app.open_corkboard();
    app.apply_scene_synopsis(None, 1, false, "The shore at dawn"); // reopens on it
    app.on_key(key(KeyCode::Char('s'))); // stub → draft
    app.on_key(key(KeyCode::Char('s'))); // draft → done
    app.mode = Mode::Insert;
    grow(&mut app);
    assert_eq!(app.project.chapter_summary(1), Some("The shore at dawn"));
    assert_eq!(
        app.project.chapter_status(1),
        Some(crate::frontmatter::Status::Final)
    );
    assert_eq!(app.project.chapter_status(0), None);
    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn annotations_move_to_the_chapter_their_text_landed_in() {
    let data = app_scratch_dir("story-grow-notes-move-data");
    config::set_data_dir(Some(data.clone()));
    let dir = app_scratch_dir("story-grow-annotations");
    let mut project = Project::create_with_template(&dir, ProjectTemplate::short_story()).unwrap();
    fs::write(project.active_path(), GROWING).unwrap();
    let start = GROWING.find("Waves").unwrap();
    project
        .set_chapter_annotations(
            "story.md",
            &[crate::annotation::Live {
                chapter: "story.md".into(),
                start,
                end: start + "Waves".len(),
                note: "too calm?".into(),
                replies: Vec::new(),
                resolved: false,
                reviewer: None,
            }],
        )
        .unwrap();
    drop(project);
    let mut app = open_test_app(&dir);
    let _dir = TempProject::new(dir);
    grow(&mut app);

    let file = app.project.chapter_file(1).to_string();
    let notes = app.project.annotations_for(&file);
    assert_eq!(notes.len(), 1, "the note moved to the Shore chapter");
    let body = app.project.chapter_body(1);
    assert_eq!(&body[notes[0].start..notes[0].end], "Waves");
    assert!(app.project.annotations_for("story.md").is_empty());
    config::set_data_dir(None);
    fs::remove_dir_all(&data).ok();
}

#[test]
fn growing_is_offered_once_when_a_story_reaches_novella_length() {
    let long = "word ".repeat(17_500);
    let (mut app, dir) = story_app("story-novella", &long);
    app.on_quiet();
    let msg = app.message.clone().unwrap_or_default();
    assert!(msg.contains("novella length"), "{msg}");
    assert!(
        matches!(app.mode, Mode::Insert),
        "it never interrupts typing"
    );

    // Reopening the project doesn't offer it again.
    drop(app);
    let mut app = open_test_app(&dir);
    app.message = None;
    app.on_quiet();
    assert!(!app.message.as_deref().unwrap_or("").contains("novella"));
}

#[test]
fn growing_is_only_offered_for_a_short_story() {
    let (app, _dir) = two_chapter_app("story-grow-novel");
    assert!(app.action_status(Action::GrowIntoNovel).is_some());
}
