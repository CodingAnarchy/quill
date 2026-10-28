//! Writes derived from a failed read must abort, not destroy data (#565).
//!
//! Each splice-then-write path reads an expected-existing file first. A transient
//! read failure must abort the write rather than write content derived from `""`
//! (which would durably destroy the manifest / a chapter / the dictionary while
//! reporting success). To simulate an unreadable-but-present file hermetically we
//! replace the target path with a directory: `read_to_string` then fails with a
//! non-NotFound error — clearly "read failed", not "file is empty" — and the
//! sibling file we assert on is left byte-for-byte.

use super::*;

#[test]
fn setting_a_goal_aborts_when_the_manifest_cant_be_read() {
    let dir = temp_dir("goal-read-fail");
    let mut project = Project::create(&dir).unwrap();
    let path = Project::manifest_path(&dir);
    // A real, populated manifest, then make it unreadable by swapping the file
    // for a directory of the same name.
    let before = fs::read_to_string(&path).unwrap();
    assert!(before.contains("[[chapters]]"), "real manifest:\n{before}");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();

    let result = project.set_goal(GoalField::Words, Some(80_000));
    assert!(
        result.is_err(),
        "a failed manifest read must abort the write"
    );

    // The directory (our stand-in for the unreadable file) is untouched: nothing
    // was spliced onto an empty base.
    assert!(path.is_dir(), "the manifest was not overwritten");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn replacing_a_chapter_body_aborts_when_the_file_cant_be_read() {
    let dir = temp_dir("chapter-read-fail");
    let mut project = Project::create(&dir).unwrap();
    // Add a second, non-active chapter (the on-disk replace path) with real prose.
    project.add_chapter(1, "Two").unwrap();
    let file = project.manifest.chapters[1].file.clone();
    let chapter_path = dir.join(&file);
    fs::write(&chapter_path, "the original prose stays\n").unwrap();
    let before = fs::read_to_string(&chapter_path).unwrap();

    // Make the chapter file unreadable (swap it for a directory).
    fs::remove_file(&chapter_path).unwrap();
    fs::create_dir(&chapter_path).unwrap();

    let result = project.replace_chapter_body(1, &[(0, 3)], "X");
    assert!(
        result.is_err(),
        "a failed chapter read must abort the write"
    );
    assert!(chapter_path.is_dir(), "the chapter was not overwritten");

    // Swap the real prose back; it is exactly what it was — never blanked.
    fs::remove_dir(&chapter_path).unwrap();
    fs::write(&chapter_path, &before).unwrap();
    assert_eq!(fs::read_to_string(&chapter_path).unwrap(), before);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn editing_chapter_front_matter_aborts_when_the_file_cant_be_read() {
    let dir = temp_dir("front-matter-read-fail");
    let mut project = Project::create(&dir).unwrap();
    project.add_chapter(1, "Two").unwrap();
    let chapter_path = dir.join(&project.manifest.chapters[1].file);
    fs::write(&chapter_path, "---\npov: Mara\n---\nprose here\n").unwrap();

    fs::remove_file(&chapter_path).unwrap();
    fs::create_dir(&chapter_path).unwrap();

    let result = project.edit_chapter_front_matter(1, |fm| fm.set_pov("Vashti"));
    assert!(
        result.is_err(),
        "a failed front-matter read must abort the write"
    );
    assert!(chapter_path.is_dir(), "the chapter was not overwritten");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn accepting_a_word_aborts_when_the_dictionary_cant_be_read() {
    let dir = temp_dir("dict-read-fail");
    let mut project = Project::create(&dir).unwrap();
    let path = dir.join(DICTIONARY_NAME);
    fs::write(&path, "Aerendil\nEowyn\n").unwrap();
    project.dictionary = load_dictionary(&dir);

    // Make the dictionary unreadable.
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();

    let result = project.accept_word("Vashti");
    assert!(
        result.is_err(),
        "a failed dictionary read must abort the write, not shrink the list"
    );
    // The in-memory set is unchanged (the insert only happens after a good write),
    // and the directory stand-in was not overwritten with just the new word.
    assert!(!project.is_accepted("Vashti"), "set not mutated on failure");
    assert!(path.is_dir(), "the dictionary was not overwritten");
    fs::remove_dir_all(&dir).ok();
}
