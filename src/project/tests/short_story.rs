//! Tests for the short story form (#1239): creating one from the template, the
//! `form` key's round trip (and its absence for a novel), the story title staying
//! the chapter title, and an older Quill's extra chapter reading as a novel.

use super::*;

/// A short story created from the template, plus its directory.
fn short_story(name: &str) -> (Project, PathBuf) {
    let dir = temp_dir(name);
    let project = Project::create_with_template(&dir, ProjectTemplate::short_story()).unwrap();
    (project, dir)
}

#[test]
fn the_short_story_template_scaffolds_one_story_chapter() {
    let (project, dir) = short_story("short-create");
    assert!(project.manifest.is_short_story());
    assert_eq!(project.manifest.chapters.len(), 1);
    assert_eq!(project.manifest.chapters[0].file, "story.md");
    // The chapter is titled after the project, never "Chapter 1".
    assert_eq!(
        project.active_title(),
        Some(project.manifest.title.as_str())
    );
    assert!(project.manifest.parts.is_none(), "a story has no parts");
    assert!(project.active_path().exists());
    let text = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(text.contains("form = \"short-story\""), "{text}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_short_story_reopens_as_a_short_story() {
    let (_, dir) = short_story("short-reopen");
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.form, Form::ShortStory);
    assert_eq!(reopened.manifest.story_chapter(), Some(0));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_novel_never_writes_a_form_key() {
    let dir = temp_dir("short-novel-no-key");
    let project = Project::create(&dir).unwrap();
    project.save_manifest().unwrap();
    let text = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(
        !text.contains("form ="),
        "a novel's manifest is unchanged: {text}"
    );
    assert!(!project.manifest.is_short_story());
    assert_eq!(project.manifest.story_chapter(), None);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_hand_edited_story_chapter_title_heals_to_the_story_title_on_open() {
    let dir = temp_dir("short-heal-title");
    fs::write(
        Project::manifest_path(&dir),
        "form = \"short-story\"\ntitle = \"The Drowned Bell\"\n\n\
         [[chapters]]\ntitle = \"Chapter 1\"\nfile = \"story.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("story.md"), "It rang at low tide.\n").unwrap();
    let project = Project::open(&dir).unwrap();
    assert_eq!(project.active_title(), Some("The Drowned Bell"));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_short_story_with_a_second_body_chapter_reads_as_a_novel() {
    // An older Quill keeps `form` but can add a chapter (#1239): the project must
    // then show the chapters it really has instead of hiding one.
    let dir = temp_dir("short-two-chapters");
    fs::write(
        Project::manifest_path(&dir),
        "form = \"short-story\"\ntitle = \"Tide\"\n\n\
         [[chapters]]\ntitle = \"Tide\"\nfile = \"story.md\"\n\n\
         [[chapters]]\ntitle = \"More\"\nfile = \"more.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("story.md"), "one\n").unwrap();
    fs::write(dir.join("more.md"), "two\n").unwrap();
    let project = Project::open(&dir).unwrap();
    assert_eq!(project.manifest.form, Form::ShortStory, "the key survives");
    assert!(!project.manifest.is_short_story());
    assert_eq!(
        project.manifest.chapters[1].title, "More",
        "nothing renamed"
    );
    fs::remove_dir_all(&dir).ok();
}
