use std::fs;
use std::path::Path;

use quill_core::project::{Project, ProjectTemplate};
use quill_core::testutil::scratch_dir;

use super::*;

/// A two-chapter novel on disk: the first chapter carries front matter (a
/// status and a synopsis), which the phone must hide from the prose.
fn novel(dir: &Path) {
    fs::write(
        dir.join("book.toml"),
        "title = \"The Lighthouse\"\nauthor = \"A. Writer\"\n\n\
         [[chapters]]\ntitle = \"Arrival\"\nfile = \"arrival.md\"\n\n\
         [[chapters]]\ntitle = \"Storm\"\nfile = \"storm.md\"\n",
    )
    .unwrap();
    fs::write(
        dir.join("arrival.md"),
        "---\nstatus: revised\nsummary: She lands at dusk.\n---\nThe boat came in at dusk.\n",
    )
    .unwrap();
    fs::write(dir.join("storm.md"), "Wind. Then rain, and more wind.\n").unwrap();
}

fn open(dir: &Path) -> Arc<QuillProject> {
    QuillProject::open(&dir.to_string_lossy()).unwrap()
}

#[test]
fn lists_chapters_with_the_desktops_cached_counts() {
    let dir = scratch_dir("ffi", "lists_chapters");
    novel(&dir);
    let project = open(&dir);

    let info = project.info().unwrap();
    assert_eq!(info.title, "The Lighthouse");
    assert_eq!(info.author.as_deref(), Some("A. Writer"));
    assert_eq!(info.form, ProjectForm::Novel);
    assert_eq!(info.books.len(), 1);
    assert_eq!(info.length_category, None);

    let chapters = project.chapters().unwrap();
    let titles: Vec<&str> = chapters.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, ["Arrival", "Storm"]);
    // The same per-chapter cache the desktop's outline pane reads.
    assert_eq!(chapters[0].words, 6);
    assert_eq!(chapters[1].words, 6);
    assert_eq!(info.words, 12);
    assert_eq!(chapters[0].status.as_deref(), Some("Revised"));
    assert_eq!(chapters[0].summary.as_deref(), Some("She lands at dusk."));
    assert_eq!(chapters[0].matter, Matter::Body);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn chapter_text_hides_front_matter() {
    let dir = scratch_dir("ffi", "chapter_text");
    novel(&dir);
    let project = open(&dir);
    assert_eq!(
        project.chapter_text(0).unwrap(),
        "The boat came in at dusk.\n"
    );
    assert!(matches!(
        project.chapter_text(9),
        Err(QuillError::NotFound { .. })
    ));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_project_is_not_found() {
    let dir = scratch_dir("ffi", "missing");
    let err = QuillProject::open(&dir.join("nope").to_string_lossy())
        .err()
        .unwrap();
    assert!(matches!(err, QuillError::NotFound { .. }), "{err:?}");
    assert!(err.to_string().contains("book.toml"), "{err}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_short_story_names_its_length_category() {
    let dir = scratch_dir("ffi", "short_story");
    let story = Project::create_with_template(&dir, ProjectTemplate::short_story()).unwrap();
    let file = story.chapter_path(story.manifest.story_chapter().unwrap());
    drop(story);
    fs::write(file, "word ".repeat(1_200)).unwrap();

    let info = open(&dir).info().unwrap();
    assert_eq!(info.form, ProjectForm::ShortStory);
    assert_eq!(info.length_category.as_deref(), Some("short story"));
    fs::remove_dir_all(&dir).ok();
}
