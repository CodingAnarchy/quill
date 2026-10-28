use std::fs;
use std::sync::Arc;

use quill_core::project::{Project, ProjectTemplate};
use quill_core::testutil::scratch_dir;

use crate::{ProjectForm, QuillProject};

use super::*;

/// A short story with three scenes: a headed one, then two split by breaks,
/// and a synopsis and status stored for the second.
fn story(name: &str) -> (std::path::PathBuf, Arc<QuillProject>) {
    let dir = scratch_dir("ffi-story", name);
    let p = Project::create_with_template(&dir, ProjectTemplate::short_story()).unwrap();
    let file = p.chapter_path(p.manifest.story_chapter().unwrap());
    drop(p);
    fs::write(
        &file,
        "---\nscenes:\n  - synopsis: \"\"\n    anchor: \"Dawn\"\n  - synopsis: The boat arrives\n    status: draft\n    anchor: \"🙂 The boat came in.\"\n---\n\
         ## Dawn\n\nLight on the water.\n\n* * *\n\n🙂 The boat came in.\n\n* * *\n\nNight fell.\n",
    )
    .unwrap();
    let project = QuillProject::open(&dir.to_string_lossy()).unwrap();
    (dir, project)
}

#[test]
fn a_story_opens_into_its_one_chapter() {
    let (dir, project) = story("opens");
    let info = project.info().unwrap();
    assert_eq!(info.form, ProjectForm::ShortStory);
    let index = info.story_chapter.expect("a story chapter");
    assert!(project.chapters().unwrap().len() as u32 > index);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn scenes_come_from_the_core_with_their_notes() {
    let (dir, project) = story("scenes");
    let index = project.info().unwrap().story_chapter.unwrap();
    let session = Arc::clone(&project).edit_chapter(index).unwrap();
    let scenes = session.scenes();
    let titles: Vec<&str> = scenes.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["Dawn", "🙂 The boat came in.", "Night fell."]);
    assert_eq!(scenes[1].synopsis.as_deref(), Some("The boat arrives"));
    assert_eq!(scenes[1].status, "Draft");
    assert_eq!(scenes[0].synopsis, None);

    // Each start points at the scene's first text, in UTF-16.
    let text: Vec<u16> = session.text().encode_utf16().collect();
    let at = |s: &SceneInfo| String::from_utf16_lossy(&text[s.start as usize..][..4]);
    assert_eq!(at(&scenes[0]), "## D");
    assert_eq!(at(&scenes[2]), "Nigh");

    // Typing a new break mid-story adds a scene without moving the synopsis.
    let night = scenes[2].start;
    session.apply_edit(night, night, "Later.\n\n* * *\n\n");
    let scenes = session.scenes();
    assert_eq!(scenes.len(), 4);
    assert_eq!(scenes[1].synopsis.as_deref(), Some("The boat arrives"));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_length_category_matches_the_desktop() {
    assert_eq!(story_length(999), "flash fiction");
    assert_eq!(story_length(4_000), "short story");
    assert_eq!(story_length(9_000), "novelette");
}
