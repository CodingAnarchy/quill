//! Git-palette init and the working-tree status: uncommitted counts, the idle
//! debounce (#779), and the interval-fallback backoff (#869). Pure move from `vcs.rs`.
use super::*;

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

#[test]
fn the_idle_fallback_backs_off_when_the_tree_is_unchanged() {
    // On an untouched editor the fallback walk keeps finding the same count, so the
    // interval stretches (doubles) rather than re-walking the whole tree on a fixed
    // short clock (#869). A real change (or a save) resets it to the base.
    let (mut app, dir) = two_chapter_app("vc-backoff");
    git_menu(&mut app, 0); // Initialize
    commit_via_menu(&mut app, "base"); // clean working tree
    assert_eq!(app.vc_changes, 0, "clean after the initial commit");

    let base = app.vc_status_idle_interval_for_test();

    // Two unchanged fallback walks: each lands the same count, so the interval grows.
    app.expire_vc_status_interval_for_test();
    app.settle_vc_status();
    let after_one = app.vc_status_idle_interval_for_test();
    assert!(
        after_one > base,
        "an unchanged walk backs off ({base:?} -> {after_one:?})"
    );

    app.expire_vc_status_interval_for_test();
    app.settle_vc_status();
    assert!(
        app.vc_status_idle_interval_for_test() > after_one,
        "successive unchanged walks keep backing off"
    );

    // A change surfaces via the (stretched) fallback and snaps the interval back.
    fs::write(dir.join("one.md"), "# One\n\nEdited outside Quill.\n").unwrap();
    app.expire_vc_status_interval_for_test();
    app.settle_vc_status();
    assert!(app.vc_changes > 0, "the change is still picked up");
    assert_eq!(
        app.vc_status_idle_interval_for_test(),
        base,
        "a real change resets the backoff to the base interval"
    );
}
