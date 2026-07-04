//! Tests for the version-control palette (init / commit / history / diff /
//! restore / remote).

use super::*;

/// Open the Git palette (Ctrl-v), move to item `index`, and run it.
fn git_menu(app: &mut App, index: usize) {
    app.on_key(ctrl('v'));
    for _ in 0..index {
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Enter));
}

/// Run the Commit action, then confirm the pre-commit review (#704) — Enter on the
/// review overlay proceeds to the message prompt, type `message`, and submit. The
/// full author-facing commit flow, reused by the tests that just want a snapshot.
fn commit_via_menu(app: &mut App, message: &str) {
    git_menu(app, 0); // Commit → the pre-commit review overlay
    app.on_key(key(KeyCode::Enter)); // confirm → the message prompt
    type_str(app, message);
    app.on_key(key(KeyCode::Enter));
}

fn palette_labels(app: &App) -> Vec<&str> {
    match &app.mode {
        Mode::Palette(p) => p.items.iter().map(|it| it.label.as_str()).collect(),
        _ => panic!("expected a palette to be open"),
    }
}

fn overlay_text(app: &App) -> String {
    app.overlay_lines()
        .expect("an overlay should be open")
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn git_palette_initializes_version_control() {
    let (mut app, dir) = two_chapter_app("vcs-init");
    assert!(!app.under_vc, "fresh project isn't under version control");
    git_menu(&mut app, 0); // the only item before init is "Initialize"
    assert!(app.under_vc, "the project is now under version control");
    assert!(dir.join(".git").exists() && dir.join(".gitignore").exists());
    assert!(
        app.message.as_deref().unwrap_or("").contains("Initialized"),
        "{:?}",
        app.message
    );
}

#[test]
fn vc_status_reflects_uncommitted_changes() {
    let (mut app, _dir) = two_chapter_app("vc-status");
    git_menu(&mut app, 0); // Initialize → the project's files are uncommitted
    assert!(app.vc_changes > 0, "a fresh repo has uncommitted files");

    commit_via_menu(&mut app, "snapshot");
    assert_eq!(app.vc_changes, 0, "clean working tree after a commit");

    // Edit a chapter and save → the tree is dirty again. The save marks the
    // status stale (#779); the idle path recomputes it off-thread, so settle the
    // walk before asserting.
    app.on_key(key(KeyCode::Char('X')));
    app.on_key(ctrl('s'));
    app.settle_vc_status();
    assert!(app.vc_changes > 0, "dirty after a saved edit");
}

#[test]
fn idle_typing_does_not_rewalk_git_status_but_a_save_does() {
    let (mut app, _dir) = two_chapter_app("vc-idle-debounce");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "base"); // clean working tree
    assert_eq!(app.vc_changes, 0, "clean after the initial commit");

    // Pure typing then idle: typing must not mark the status stale (that's the
    // #779 invariant). We assert the dirty flag specifically rather than "settling",
    // because the wall-clock interval fallback may legitimately spawn a walk on an
    // idle tick — under heavy parallel test load the thread can be preempted past
    // the interval — and that walk is harmless here: an unsaved edit changes the
    // buffer, not the on-disk tree, so git status stays 0 either way.
    type_str(&mut app, "unsaved edits");
    app.on_idle();
    assert!(
        !app.vc_status_marked_dirty(),
        "pure typing doesn't mark the status stale"
    );
    app.settle_vc_status(); // drain any interval-triggered walk so the count is settled
    assert_eq!(
        app.vc_changes, 0,
        "an unsaved edit doesn't change git status"
    );

    // Saving marks the status stale; the idle path then recomputes it off-thread
    // and the count reflects the now-dirty tree.
    app.on_key(ctrl('s'));
    assert!(
        app.vc_status_settling(),
        "a save marks the status stale for the next idle tick"
    );
    app.settle_vc_status();
    assert!(
        app.vc_changes > 0,
        "the status refreshes after a save — the saved edit is uncommitted"
    );
}

#[test]
fn the_interval_fallback_picks_up_an_external_edit() {
    let (mut app, dir) = two_chapter_app("vc-external-edit");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "base"); // clean working tree
    assert_eq!(app.vc_changes, 0, "clean after the initial commit");

    // A change made outside Quill (no save marks it dirty). Force the interval
    // fallback due, then let the idle path notice it — external edits still
    // surface even when nothing in-app flagged the status (#779).
    fs::write(dir.join("one.md"), "# One\n\nEdited outside Quill.\n").unwrap();
    app.expire_vc_status_interval_for_test();
    app.settle_vc_status();
    assert!(
        app.vc_changes > 0,
        "the interval fallback re-walks the tree and sees the external edit"
    );
}

/// The commit summaries shown in the open history view, newest first.
fn history_summaries(app: &App) -> Vec<String> {
    match &app.mode {
        Mode::History(h) => h.commits.iter().map(|c| c.summary.clone()).collect(),
        _ => panic!("expected the history view to be open"),
    }
}

#[test]
fn git_palette_shows_commit_history() {
    let (mut app, _dir) = two_chapter_app("vc-log");
    git_menu(&mut app, 0); // Initialize
    git_menu(&mut app, 1); // History — before any commit: a message, no view
    assert!(!matches!(app.mode, Mode::History(_)));
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("No commits yet"));

    commit_via_menu(&mut app, "first draft");

    git_menu(&mut app, 1); // History — now a navigable view
    assert!(
        history_summaries(&app).contains(&"first draft".to_string()),
        "history lists the commit"
    );
    app.on_key(key(KeyCode::Esc)); // close it
    assert!(!matches!(app.mode, Mode::History(_)));
}

#[test]
fn history_diffs_the_chapter_against_a_chosen_commit() {
    let (mut app, _dir) = two_chapter_app("vc-hist-diff");
    git_menu(&mut app, 0); // Initialize

    // Commit an original version of the active chapter (one.md).
    type_str(&mut app, "The night was dark.");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "original");

    // Edit and save a newer version (the cursor sits after the committed text).
    type_str(&mut app, " Then it was long.");
    app.on_key(ctrl('s'));

    // Open history, select the "original" commit, and diff.
    git_menu(&mut app, 1); // History (newest first: [original])
    app.on_key(key(KeyCode::Char('d')));
    let diff = overlay_text(&app);
    assert!(
        diff.contains("+") && diff.contains("Then it was long"),
        "the diff shows what changed since that commit: {diff}"
    );
}

#[test]
fn history_restores_the_chapter_from_a_chosen_commit() {
    let (mut app, _dir) = two_chapter_app("vc-hist-restore");
    git_menu(&mut app, 0); // Initialize

    // First committed version of the active chapter.
    type_str(&mut app, "The first draft.");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "draft one");

    // A second, different committed version.
    type_str(&mut app, " Now revised.");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "draft two");
    assert!(
        app.buffer.text().contains("Now revised"),
        "the revision is live"
    );

    // Open history (newest first: [draft two, draft one]); pick the older commit.
    git_menu(&mut app, 1); // History
    app.on_key(key(KeyCode::Down)); // move to "draft one"
    app.on_key(key(KeyCode::Char('r'))); // restore from it

    // The older version is in the editable buffer (review-and-save), not written.
    assert!(!matches!(app.mode, Mode::History(_)), "the view closed");
    assert!(
        app.buffer.text().contains("The first draft") && !app.buffer.text().contains("Now revised"),
        "the older version loaded into the buffer: {:?}",
        app.buffer.text()
    );
    assert!(
        app.edit_dirty(),
        "the recovered version is dirty, awaiting save"
    );
    // Unsaved: the file on disk still holds the newer, committed version.
    assert!(
        crate::vcs::show_file_at(&app.project.root, "HEAD", "one.md")
            .unwrap()
            .unwrap()
            .contains("Now revised")
    );
}

#[test]
fn git_palette_commits_with_a_message() {
    let (mut app, _dir) = two_chapter_app("vcs-commit");
    git_menu(&mut app, 0); // Initialize

    // Commit first opens the pre-commit review (#704): the changed-file list + diff.
    git_menu(&mut app, 0); // Commit
    assert!(app.overlay.is_some(), "the pre-commit review overlay opens");
    let review = overlay_text(&app);
    assert!(
        review.contains("one.md") && review.contains("will be committed"),
        "the review lists the changed files: {review}"
    );

    // Confirm the review → the message prompt.
    app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "a commit-message prompt opens after the review"
    );
    type_str(&mut app, "first draft");
    app.on_key(key(KeyCode::Enter));
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .starts_with("Committed"),
        "{:?}",
        app.message
    );
    // Everything was captured → the tree is now clean.
    assert_eq!(crate::vcs::commit(&app.project.root, "x").unwrap(), None);
}

#[test]
fn pre_commit_review_can_be_cancelled_without_committing() {
    let (mut app, _dir) = two_chapter_app("vcs-review-cancel");
    git_menu(&mut app, 0); // Initialize
    let dirty_before = app.vc_changes;
    assert!(dirty_before > 0, "the fresh repo has uncommitted files");

    // Open the review, then cancel with Esc — nothing is committed.
    git_menu(&mut app, 0); // Commit
    assert!(app.overlay.is_some(), "the review overlay opens");
    app.on_key(key(KeyCode::Esc));
    assert!(app.overlay.is_none(), "the review closed");
    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "cancelling doesn't open the message prompt"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("cancelled"),
        "{:?}",
        app.message
    );
    app.refresh_vc_status();
    assert_eq!(
        app.vc_changes, dirty_before,
        "nothing was committed — the tree is still dirty"
    );
}

#[test]
fn commit_on_a_clean_tree_reports_nothing_to_commit() {
    let (mut app, _dir) = two_chapter_app("vcs-clean");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "everything"); // capture the initial files
    assert_eq!(app.vc_changes, 0, "clean after the first commit");

    // A second Commit on a clean tree: no review overlay, just a message.
    git_menu(&mut app, 0); // Commit
    assert!(app.overlay.is_none(), "no review overlay on a clean tree");
    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "and no message prompt either"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Nothing to commit"),
        "{:?}",
        app.message
    );
}

#[test]
fn pre_commit_review_shows_the_diff_of_what_will_be_committed() {
    let (mut app, _dir) = two_chapter_app("vcs-review-diff");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "base"); // capture the starting files

    // Edit and save the active chapter, then open the review.
    type_str(&mut app, "The night was long.");
    app.on_key(ctrl('s'));
    git_menu(&mut app, 0); // Commit → the pre-commit review
    let review = overlay_text(&app);
    assert!(
        review.contains("1 file will be committed") && review.contains("one.md"),
        "the review lists the single changed file: {review}"
    );
    assert!(
        review.contains("+The night was long."),
        "the review shows the added line the commit would capture: {review}"
    );

    // Confirming commits it; the tree goes clean.
    app.on_key(key(KeyCode::Enter));
    type_str(&mut app, "add a line");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.vc_changes, 0, "committed → clean working tree");
}

#[test]
fn git_palette_shows_changes_diff() {
    let (mut app, _dir) = two_chapter_app("vc-diff");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "base");

    // Clean tree → a message, no overlay.
    git_menu(&mut app, 2); // Changes
    assert!(app.overlay.is_none());
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("No uncommitted changes"));

    // Edit + save → the diff overlay shows the changed file.
    app.on_key(key(KeyCode::Char('Q')));
    app.on_key(ctrl('s'));
    git_menu(&mut app, 2); // Changes
    assert!(
        overlay_text(&app).contains("one.md"),
        "diff names the changed file"
    );
}

#[test]
fn git_palette_restores_a_chapter() {
    let (mut app, _dir) = two_chapter_app("vc-restore");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "base");

    // Edit + save, then restore.
    app.on_key(key(KeyCode::Char('Z')));
    app.on_key(ctrl('s'));
    assert!(app.buffer.text().contains('Z'), "the edit landed");
    git_menu(&mut app, 3); // Restore chapter
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "restore asks for confirmation"
    );
    app.on_key(key(KeyCode::Char('y')));
    assert!(
        !app.buffer.text().contains('Z'),
        "the edit was reverted: {:?}",
        app.buffer.text()
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("Restored"),
        "{:?}",
        app.message
    );
}

#[test]
fn git_palette_configures_a_remote_then_offers_sync() {
    let (mut app, _dir) = two_chapter_app("vc-remote");
    git_menu(&mut app, 0); // Initialize

    // Before a remote: the menu offers "Set remote" but not Push/Pull.
    app.on_key(ctrl('v'));
    let labels = palette_labels(&app);
    assert!(labels.contains(&"Set remote"), "{labels:?}");
    assert!(
        !labels.contains(&"Push") && !labels.contains(&"Pull"),
        "{labels:?}"
    );
    app.on_key(key(KeyCode::Esc));

    // Set a remote through the palette (Commit, History, Changes, Restore, Set remote = idx 4).
    git_menu(&mut app, 4);
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "set-remote prompts for a URL"
    );
    type_str(&mut app, "https://example.com/book.git");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        crate::vcs::remote_url(&app.project.root).as_deref(),
        Some("https://example.com/book.git"),
    );

    // Now the menu offers Push and Pull, and "Change remote" instead of "Set remote".
    app.on_key(ctrl('v'));
    let labels = palette_labels(&app);
    assert!(
        labels.contains(&"Push") && labels.contains(&"Pull"),
        "{labels:?}"
    );
    assert!(
        labels.contains(&"Change remote") && !labels.contains(&"Set remote"),
        "{labels:?}"
    );
}

// --- History view: type-to-filter (#822) -----------------------------------

/// The history view's `(selected, filter, typing)` state.
fn history_state(app: &App) -> (usize, String, bool) {
    match &app.mode {
        Mode::History(h) => (h.selected, h.filter.clone(), h.typing),
        _ => panic!("expected the history view to be open"),
    }
}

/// A repo with three commits (summaries alpha/beta/gamma), history open. Each
/// commit captures a distinct chapter body so a restore is observable. Git may
/// order same-second commits either way, so tests resolve rows by summary rather
/// than assuming a fixed position.
fn history_over_three(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    git_menu(&mut app, 0); // Initialize
                           // The base commit captures the seeded "first chapter\n".
    commit_via_menu(&mut app, "alpha groundwork");
    type_str(&mut app, " edit b");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "beta revision");
    type_str(&mut app, " edit c");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "gamma polish");
    git_menu(&mut app, 1); // History
    (app, dir)
}

/// The history row whose summary contains `needle`.
fn history_row_of(app: &App, needle: &str) -> usize {
    history_summaries(app)
        .iter()
        .position(|s| s.contains(needle))
        .expect("a commit with that summary")
}

#[test]
fn slash_filters_history_by_summary_and_moves_within_matches() {
    let (mut app, _dir) = history_over_three("vc-history-filter");
    let beta_row = history_row_of(&app, "beta");

    app.on_key(key(KeyCode::Char('/')));
    assert!(history_state(&app).2, "typing after /");
    // "beta" matches exactly one commit; the selection snaps onto its row.
    type_str(&mut app, "beta");
    assert_eq!(history_state(&app).0, beta_row);

    // A broader query still matching only "beta revision" (by its word).
    for _ in 0..4 {
        app.on_key(key(KeyCode::Backspace));
    }
    type_str(&mut app, "revision");
    assert_eq!(history_state(&app).0, beta_row);
    // No further match in either direction — Down/Up stay put on the one match.
    app.on_key(key(KeyCode::Down));
    assert_eq!(history_state(&app).0, beta_row);
    app.on_key(key(KeyCode::Up));
    assert_eq!(history_state(&app).0, beta_row);
}

#[test]
fn history_esc_clears_the_filter_then_closes() {
    let (mut app, _dir) = history_over_three("vc-history-filter-esc");

    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "gamma");
    app.on_key(key(KeyCode::Esc)); // leave input, keep the filter
    assert_eq!(history_state(&app).1, "gamma");
    assert!(!history_state(&app).2);

    app.on_key(key(KeyCode::Esc)); // clear filter; list restored
    assert_eq!(history_state(&app).1, "");
    assert!(matches!(app.mode, Mode::History(_)));

    app.on_key(key(KeyCode::Esc)); // close
    assert!(!matches!(app.mode, Mode::History(_)));
}

#[test]
fn history_restore_acts_on_the_filtered_match() {
    let (mut app, dir) = history_over_three("vc-history-filter-restore");
    let alpha_row = history_row_of(&app, "alpha");
    // Filter to the base commit and restore the chapter to it; the buffer must
    // load that commit's version (review-and-save), proving the action targets
    // the filtered selection, not the top row.
    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "alpha"); // the base commit: "first chapter\n"
    assert_eq!(history_state(&app).0, alpha_row);
    app.on_key(key(KeyCode::Enter)); // leave input, keep filter
    app.on_key(key(KeyCode::Char('r'))); // restore this chapter to the match

    assert!(!matches!(app.mode, Mode::History(_)), "the view closed");
    // The base commit's version loads into the buffer, without the later edits —
    // proving the restore targeted the filtered selection, not the top row.
    assert_eq!(app.buffer.text(), "first chapter\n");
    let _ = &dir;
}
