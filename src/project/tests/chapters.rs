//! Tests for the chapter-list edits that persist to the manifest — add, rename,
//! swap, remove — and for replacing the chapter set from an imported manuscript,
//! including the atomicity guarantee (#600) that a write failing partway through
//! an import leaves the original chapters and manifest byte-for-byte.

use super::*;

#[test]
fn add_rename_swap_remove_persist() {
    let dir = temp_dir("structure");
    let mut project = Project::create(&dir).unwrap(); // "Chapter 1"

    // Add a second chapter after the first; file is created with a fresh name.
    project.add_chapter(1, "Second").unwrap();
    assert_eq!(project.manifest.chapters.len(), 2);
    assert_ne!(
        project.manifest.chapters[0].file,
        project.manifest.chapters[1].file
    );
    assert!(project.chapter_path(1).exists());

    // Rename and reorder.
    project.rename_chapter(0, "First").unwrap();
    project.swap_chapters(0, 1).unwrap();
    let order: Vec<&str> = project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(order, vec!["Second", "First"]);

    // Reopen: every change persisted.
    let reopened = Project::open(&dir).unwrap();
    let order: Vec<String> = reopened
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.clone())
        .collect();
    assert_eq!(order, vec!["Second".to_string(), "First".to_string()]);

    // Remove the first; its file moves to unused/ rather than vanishing.
    let removed_file = reopened.manifest.chapters[0].file.clone();
    let mut reopened = reopened;
    reopened.remove_chapter(0).unwrap();
    assert_eq!(reopened.manifest.chapters.len(), 1);
    assert!(dir.join("unused").join(&removed_file).exists());
    assert!(!dir.join(&removed_file).exists());

    fs::remove_dir_all(&dir).ok();
}

/// Importing a manuscript replaces the starter chapter with the imported set:
/// one file per chapter in reading order, the manifest rebuilt, and the title /
/// author applied. The success path is what the atomicity fix must preserve.
#[test]
fn import_chapters_replaces_the_starter_set() {
    let dir = temp_dir("import-ok");
    let mut project = Project::create(&dir).unwrap(); // one starter chapter
    let starter = project.manifest.chapters[0].file.clone();

    let chapters = vec![
        ("One".to_string(), "first body\n".to_string()),
        ("Two".to_string(), "second body\n".to_string()),
        ("Three".to_string(), "third body\n".to_string()),
    ];
    project
        .import_chapters(Some("Imported"), Some("A. Author"), &chapters)
        .unwrap();

    assert_eq!(project.manifest.title, "Imported");
    assert_eq!(project.manifest.author.as_deref(), Some("A. Author"));
    let titles: Vec<&str> = project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(titles, vec!["One", "Two", "Three"]);
    assert_eq!(
        fs::read_to_string(project.chapter_path(1)).unwrap(),
        "second body\n"
    );
    // The starter chapter's file (chapter-1.md) is reused, not orphaned; no
    // leftover starter beyond the imported set, and no staging temp files.
    assert!(!dir.join("unused").join(&starter).exists());
    let strays: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(strays.is_empty(), "staging temp left behind: {strays:?}");

    // Persisted across a reopen.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.chapters.len(), 3);
    fs::remove_dir_all(&dir).ok();
}

/// The atomicity guarantee (#600): if a write partway through an import fails,
/// the original chapters and manifest survive byte-for-byte. We seed a project,
/// capture its on-disk state, then import a fresh manuscript with the third
/// staging write armed to fail before its rename — the originals must be intact.
#[test]
fn a_failed_import_write_preserves_the_originals() {
    let dir = temp_dir("import-fail");
    let mut project = Project::create(&dir).unwrap();
    // Give the project real content to lose: rename the starter and add two more.
    project.rename_chapter(0, "Original One").unwrap();
    project.add_chapter(1, "Original Two").unwrap();
    project.add_chapter(2, "Original Three").unwrap();
    for i in 0..3 {
        let path = project.chapter_path(i);
        fs::write(&path, format!("original body {}\n", i + 1)).unwrap();
    }
    project.save_manifest().unwrap();

    // Snapshot every chapter file and the manifest as written on disk.
    let manifest_path = Project::manifest_path(&dir);
    let manifest_before = fs::read_to_string(&manifest_path).unwrap();
    let files_before: Vec<(String, String)> = project
        .manifest
        .chapters
        .iter()
        .map(|c| {
            (
                c.file.clone(),
                fs::read_to_string(dir.join(&c.file)).unwrap(),
            )
        })
        .collect();

    // Import four new chapters, but fail the third staging write before its
    // rename — mid-operation, after some new files are staged.
    let incoming = vec![
        ("New One".to_string(), "doomed one\n".to_string()),
        ("New Two".to_string(), "doomed two\n".to_string()),
        ("New Three".to_string(), "doomed three\n".to_string()),
        ("New Four".to_string(), "doomed four\n".to_string()),
    ];
    crate::atomic::fail_point::arm_pre_rename_failure_on_nth(3);
    let result = project.import_chapters(Some("Doomed"), Some("Nobody"), &incoming);
    assert!(
        result.is_err(),
        "the interrupted import must report failure"
    );

    // The manifest on disk is untouched, and every original chapter file is
    // byte-for-byte intact — no originals deleted, no half-rebuilt manifest.
    assert_eq!(
        fs::read_to_string(&manifest_path).unwrap(),
        manifest_before,
        "the manifest must survive a failed import"
    );
    for (file, body) in &files_before {
        assert_eq!(
            fs::read_to_string(dir.join(file)).unwrap(),
            *body,
            "original chapter {file} must survive a failed import"
        );
    }
    // No staging temp files were left behind.
    let strays: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(strays.is_empty(), "staging temp left behind: {strays:?}");

    // And a reopen still sees the originals — the project remains usable.
    let reopened = Project::open(&dir).unwrap();
    let titles: Vec<&str> = reopened
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(
        titles,
        vec!["Original One", "Original Two", "Original Three"]
    );
    fs::remove_dir_all(&dir).ok();
}
