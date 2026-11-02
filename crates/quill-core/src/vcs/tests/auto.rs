//! Automatic version history (#1296): starting a history for a book that has
//! none, leaving a hand-managed or nested repository alone, and snapshots.

use std::fs;

use time::macros::datetime;

use super::temp_dir;
use crate::vcs::{
    init, is_auto_history, log, snapshot, snapshot_message, start_auto_history, status, AutoHistory,
};

/// A one-chapter book at `name` under a scratch dir.
fn book(name: &str) -> std::path::PathBuf {
    let root = temp_dir(name);
    fs::write(
        root.join("book.toml"),
        "title = \"Tide\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .expect("write the manifest");
    fs::write(root.join("one.md"), "# One\n\nThe sea came in.\n").expect("write the chapter");
    root
}

#[test]
fn a_book_without_history_gets_one_with_the_book_committed() {
    let root = book("auto-start");
    assert_eq!(
        start_auto_history(&root).expect("start"),
        AutoHistory::Started
    );

    assert!(is_auto_history(&root), "the history is marked as Quill's");
    let commits = log(&root, 10).expect("log");
    assert_eq!(commits.len(), 1, "one commit: the book as it stood");
    assert_eq!(commits[0].summary, "Start of version history");
    assert_eq!(
        status(&root).expect("status").count(),
        0,
        "everything was committed"
    );
    assert!(
        root.join(".gitignore").exists(),
        "Quill's .gitignore keeps the editor state out"
    );
}

#[test]
fn starting_again_finds_the_history_already_on() {
    let root = book("auto-again");
    start_auto_history(&root).expect("start");
    assert_eq!(start_auto_history(&root).expect("again"), AutoHistory::On);
    assert_eq!(
        log(&root, 10).expect("log").len(),
        1,
        "no second first commit"
    );
}

#[test]
fn a_repository_the_author_made_is_left_alone() {
    let root = book("auto-hand");
    init(&root).expect("the author's own init");
    assert_eq!(
        start_auto_history(&root).expect("start"),
        AutoHistory::HandManaged
    );
    assert!(!is_auto_history(&root), "not marked as Quill's");
    assert!(log(&root, 10).expect("log").is_empty(), "no commit added");
    assert!(!AutoHistory::HandManaged.snapshots(false));
    assert!(
        AutoHistory::HandManaged.snapshots(true),
        "unless the author turns snapshots on for it"
    );
}

#[test]
fn a_book_inside_a_larger_repository_gets_no_history_of_its_own() {
    let shelf = temp_dir("auto-shelf");
    init(&shelf).expect("the shelf's repository");
    let root = shelf.join("Tide");
    fs::create_dir_all(&root).expect("make the book folder");
    fs::write(root.join("one.md"), "The sea came in.\n").expect("write the chapter");

    assert_eq!(
        start_auto_history(&root).expect("start"),
        AutoHistory::Nested
    );
    assert!(!root.join(".git").exists(), "no nested repository");
    assert!(!AutoHistory::Nested.snapshots(true));
}

#[test]
fn a_snapshot_commits_changes_and_skips_when_nothing_changed() {
    let root = book("auto-snapshot");
    start_auto_history(&root).expect("start");

    assert_eq!(
        snapshot(&root).expect("clean snapshot"),
        None,
        "nothing to commit"
    );

    fs::write(root.join("one.md"), "# One\n\nThe sea came in, and out.\n").expect("edit");
    assert!(snapshot(&root).expect("snapshot").is_some());
    let commits = log(&root, 10).expect("log");
    assert_eq!(commits.len(), 2);
    assert!(
        commits[0].summary.starts_with("Snapshot, "),
        "a plain dated message: {}",
        commits[0].summary
    );
}

#[test]
fn the_editor_state_never_lands_in_a_snapshot() {
    let root = book("auto-state");
    fs::create_dir_all(root.join(".quill")).expect("make the state folder");
    fs::write(root.join(".quill").join("lock"), "pid").expect("write a lock");
    start_auto_history(&root).expect("start");
    let repo = git2::Repository::open(&root).expect("open");
    let tree = repo
        .head()
        .and_then(|head| head.peel_to_tree())
        .expect("the first commit's tree");
    assert!(
        tree.get_name("one.md").is_some(),
        "the chapter was committed"
    );
    assert!(tree.get_name(".quill").is_none(), "the lock wasn't");
}

#[test]
fn the_snapshot_message_reads_as_a_date_and_time() {
    assert_eq!(
        snapshot_message(datetime!(2026-11-02 14:05 UTC)),
        "Snapshot, 2 Nov 2026 at 14:05"
    );
}
