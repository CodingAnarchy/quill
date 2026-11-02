//! Version history for a book in a synced folder (#1298): each device keeps
//! its own history of the book, outside the shared folder.

use std::fs;

use super::temp_dir;
use crate::config;
use crate::synced::{mark_synced_for_test, project_id};
use crate::vcs::{is_repo, log, snapshot, start_auto_history, status, AutoHistory};

/// A one-chapter book in a "synced" scratch folder.
fn shared_book(name: &str) -> std::path::PathBuf {
    let root = temp_dir(name);
    mark_synced_for_test(&root);
    fs::write(
        root.join("book.toml"),
        "title = \"Tide\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .expect("manifest");
    fs::write(root.join("one.md"), "The sea came in.\n").expect("chapter");
    root
}

#[test]
fn a_synced_books_history_is_kept_out_of_the_shared_folder() {
    let root = shared_book("synced-out");
    let device = temp_dir("synced-out-device");
    config::set_data_dir(Some(device.clone()));

    assert_eq!(
        start_auto_history(&root).expect("start"),
        AutoHistory::Started
    );
    assert!(!root.join(".git").exists(), "no .git in the shared folder");
    let id = project_id(&root).expect("the book has an id");
    assert!(device.join("histories").join(format!("{id}.git")).is_dir());
    assert!(is_repo(&root));
    assert_eq!(log(&root, 10).expect("log").len(), 1);

    // The editor's state on this device stays out of the history too.
    fs::create_dir_all(root.join(".quill.nosync")).expect("state");
    fs::write(root.join(".quill.nosync").join("lock"), "pid").expect("lock");
    assert_eq!(
        status(&root).expect("status").count(),
        0,
        "the state isn't a change"
    );
    config::set_data_dir(None);
}

#[test]
fn each_device_keeps_its_own_history_including_the_others_edits() {
    let root = shared_book("synced-two");
    let mac = temp_dir("synced-two-mac");
    let phone = temp_dir("synced-two-phone");

    config::set_data_dir(Some(mac.clone()));
    start_auto_history(&root).expect("the Mac starts its history");

    // The phone opens the same folder: the id is shared, its history is its own.
    config::set_data_dir(Some(phone));
    assert_eq!(
        start_auto_history(&root).expect("the phone starts its history"),
        AutoHistory::Started
    );
    fs::write(root.join("one.md"), "The sea came in, then out.\n").expect("the phone's edit");
    snapshot(&root).expect("the phone's snapshot");
    assert_eq!(log(&root, 10).expect("phone log").len(), 2);

    // Back on the Mac, the phone's edit arrived through the sync, and the next
    // snapshot records it in the Mac's history too.
    config::set_data_dir(Some(mac));
    assert_eq!(start_auto_history(&root).expect("again"), AutoHistory::On);
    assert!(snapshot(&root).expect("the Mac's snapshot").is_some());
    assert_eq!(log(&root, 10).expect("mac log").len(), 2);
    config::set_data_dir(None);
}
