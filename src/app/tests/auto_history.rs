//! Automatic version history in the editor (#1296): a book with no history
//! gets one on open, snapshots follow a pause after a save and the close, a
//! history the author manages is left alone, and the setting turns it off.

use std::time::{Duration, Instant};

use super::*;
use crate::config::HistoryConfig;
use crate::vcs;

/// A two-chapter book opened with automatic history started and settled.
fn started(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.history = HistoryConfig::default();
    app.start_auto_history();
    app.settle_auto_history_for_test();
    (app, dir)
}

/// Make a snapshot owed, as long ago as the configured pause, then tick.
fn pause_after_a_save(app: &mut App) {
    app.mark_snapshot_owed();
    app.snapshot_owed_since = Instant::now().checked_sub(Duration::from_secs(10 * 60));
    app.tick_auto_history();
    app.settle_auto_history_for_test();
}

#[test]
fn opening_a_book_without_history_starts_one_and_says_so_once() {
    let (app, dir) = started("autohist-start");
    assert!(
        vcs::is_auto_history(&dir),
        "the book has a history Quill started"
    );
    assert!(
        app.under_vc,
        "the Git menu treats the book as under version control"
    );
    assert!(app.auto_snapshots);
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("version history")),
        "a short note the first time: {:?}",
        app.message
    );

    // Opening it again finds the history on, and says nothing.
    let mut again = open_test_app(&dir);
    again.history = HistoryConfig::default();
    again.start_auto_history();
    again.settle_auto_history_for_test();
    assert!(again.auto_snapshots);
    assert!(
        !again
            .message
            .as_deref()
            .is_some_and(|m| m.contains("version history")),
        "no note the second time: {:?}",
        again.message
    );
}

#[test]
fn a_pause_after_a_save_takes_a_snapshot_off_the_ui_thread() {
    let (mut app, dir) = started("autohist-pause");
    type_str(&mut app, "More ");
    app.flush_active().expect("save");
    app.settle_saves();

    // Not yet: the pause hasn't been long enough.
    app.mark_snapshot_owed();
    app.tick_auto_history();
    assert!(
        !app.snapshot_task.is_busy(),
        "no snapshot right after a save"
    );

    pause_after_a_save(&mut app);
    let commits = vcs::log(&dir, 10).expect("log");
    assert_eq!(commits.len(), 2, "the start, then the snapshot");
    assert!(commits[0].summary.starts_with("Snapshot, "));
}

#[test]
fn closing_the_book_takes_a_snapshot() {
    let (mut app, dir) = started("autohist-close");
    type_str(&mut app, "Last words ");
    app.flush_active().expect("save");
    app.settle_saves();
    app.snapshot_on_close();
    assert_eq!(vcs::log(&dir, 10).expect("log").len(), 2);
    assert_eq!(vcs::status(&dir).expect("status").count(), 0, "all of it");
}

#[test]
fn a_history_the_author_manages_gets_no_snapshots() {
    let (mut app, dir) = two_chapter_app("autohist-hand");
    vcs::init(&dir).expect("the author's own repository");
    vcs::commit(&dir, "My first commit").expect("commit");
    app.history = HistoryConfig::default();
    app.start_auto_history();
    app.settle_auto_history_for_test();
    assert!(!app.auto_snapshots);

    type_str(&mut app, "Edit ");
    app.flush_active().expect("save");
    app.settle_saves();
    pause_after_a_save(&mut app);
    app.snapshot_on_close();
    let log: Vec<String> = vcs::log(&dir, 10)
        .expect("log")
        .into_iter()
        .map(|c| c.summary)
        .collect();
    assert_eq!(log, ["My first commit"], "only the author's commit");
}

#[test]
fn turning_automatic_history_off_leaves_the_book_alone() {
    let (mut app, dir) = two_chapter_app("autohist-off");
    app.history = HistoryConfig {
        automatic: false,
        ..HistoryConfig::default()
    };
    app.start_auto_history();
    app.settle_auto_history_for_test();
    assert!(!dir.join(".git").exists(), "no history started");
    app.snapshot_on_close();
    assert!(!dir.join(".git").exists());
}

#[test]
fn opening_another_book_snapshots_this_one_and_starts_history_for_the_next() {
    isolate("autohist-switch");
    let (mut app, first) = started("autohist-switch-a");
    type_str(&mut app, "Parting words ");
    let second = make_project("autohist-switch-b", "Second", "The second book.\n");

    app.switch_to(second.clone());
    app.settle_auto_history_for_test();
    assert_eq!(
        vcs::log(&first, 10).expect("log").len(),
        2,
        "the first book was snapshotted"
    );
    assert!(
        vcs::is_auto_history(&second),
        "the second book has a history too"
    );
}
