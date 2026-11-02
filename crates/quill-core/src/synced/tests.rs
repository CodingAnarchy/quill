use std::fs;

use super::*;
use crate::testutil::scratch_dir;

fn book(name: &str) -> PathBuf {
    let root = scratch_dir("synced", name);
    fs::write(
        root.join("book.toml"),
        "title = \"Tide\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .expect("manifest");
    fs::write(root.join("one.md"), "Text.\n").expect("chapter");
    root
}

#[test]
fn a_folder_under_mobile_documents_is_synced() {
    let icloud = scratch_dir("synced", "icloud")
        .join("Library")
        .join("Mobile Documents")
        .join("com~apple~CloudDocs")
        .join("Tide");
    fs::create_dir_all(&icloud).expect("make");
    assert!(is_synced(&icloud));
    assert_eq!(state_dir(&icloud), icloud.join(".quill.nosync"));

    let local = scratch_dir("synced", "local");
    assert!(!is_synced(&local));
    assert_eq!(state_dir(&local), local.join(".quill"));
}

#[test]
fn a_book_gets_one_id_written_first_in_its_manifest() {
    let root = book("id");
    assert_eq!(project_id(&root), None);
    let id = ensure_project_id(&root).expect("id");
    assert_eq!(id.len(), 36, "a UUID: {id}");
    assert_eq!(ensure_project_id(&root).expect("again"), id, "kept");
    let text = fs::read_to_string(root.join("book.toml")).expect("read");
    assert!(text.starts_with(&format!("id = \"{id}\"\n")), "{text}");
    let project = crate::project::Project::open(&root).expect("still opens");
    assert_eq!(project.manifest.id.as_deref(), Some(id.as_str()));
    assert_ne!(new_id(), new_id(), "ids differ");
}

#[test]
fn a_synced_books_history_lives_in_this_devices_data_dir() {
    let root = book("history");
    let data = scratch_dir("synced", "history-data");
    config::set_data_dir(Some(data.clone()));
    assert_eq!(history_dir(&root), None, "not synced");
    mark_synced_for_test(&root);
    assert_eq!(history_dir(&root), None, "no id yet");
    let id = ensure_project_id(&root).expect("id");
    assert_eq!(
        history_dir(&root),
        Some(data.join("histories").join(format!("{id}.git")))
    );
    config::set_data_dir(None);
}

#[test]
fn conflict_copies_are_found_beside_their_chapters() {
    let root = book("conflicts");
    fs::create_dir_all(root.join("part")).expect("dir");
    fs::write(root.join("one 2.md"), "Their version.\n").expect("copy");
    fs::write(root.join("one 3.md"), "Another.\n").expect("copy");
    fs::write(root.join("one notes.md"), "Not a copy.\n").expect("other");
    fs::write(root.join("part").join("two 2.md"), "Copy.\n").expect("copy");
    let files = vec!["one.md".to_string(), "part/two.md".to_string()];
    assert_eq!(
        conflict_copies(&root, &files),
        vec![
            ("one.md".to_string(), "one 2.md".to_string()),
            ("one.md".to_string(), "one 3.md".to_string()),
            ("part/two.md".to_string(), "part/two 2.md".to_string()),
        ]
    );
}

#[test]
fn another_devices_version_is_kept_under_the_next_free_name() {
    let root = book("keep");
    let chapter = root.join("one.md");
    assert_eq!(
        keep_copy(&chapter, "Theirs.\n").expect("keep"),
        root.join("one 2.md")
    );
    assert_eq!(
        keep_copy(&chapter, "Again.\n").expect("keep"),
        root.join("one 3.md")
    );
    assert_eq!(
        fs::read_to_string(root.join("one 2.md")).expect("read"),
        "Theirs.\n"
    );
    let found = conflict_copies(&root, &["one.md".to_string()]);
    assert_eq!(found.len(), 2, "the copies read as conflict copies");
}
