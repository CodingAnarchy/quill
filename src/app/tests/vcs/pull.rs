//! A pull that lands on a stale context (#870): it must not reload after the author
//! navigated away or over unsaved edits, but the clean common path still reloads the
//! active chapter. Pure move from `vcs.rs`.

use super::super::*;

// --- A pull that lands on a stale context (#870) -----------------------------

/// A pull that lands after the author navigated to another chapter must not
/// reload — the just-pulled disk state is for the chapter the pull started on, not
/// wherever the author moved to. The now-active chapter's buffer is left intact.
#[test]
fn a_pull_landing_after_navigation_does_not_reload_the_new_chapter() {
    let (mut app, _dir) = two_chapter_app("vc-pull-nav");
    assert_eq!(app.project.active_index(), 0, "starts on chapter one");

    // Spawn the pull while on chapter one (it stamps one.md as its origin), then
    // navigate to chapter two and type an unsaved edit before it lands.
    app.spawn_landing_pull_for_test();
    app.next_chapter();
    assert_eq!(app.project.active_index(), 1);
    type_str(&mut app, "unsaved words on chapter two");
    assert!(app.buffer.dirty, "the new chapter has unsaved edits");

    app.settle_vc_for_test();

    assert!(
        app.buffer.text().contains("unsaved words on chapter two"),
        "the pull left the navigated-to chapter's edits intact: {:?}",
        app.buffer.text()
    );
    assert!(
        app.buffer.dirty,
        "and the buffer stays dirty (not reloaded)"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("navigated away"),
        "the author is told the open chapter was left untouched: {:?}",
        app.message
    );
}

/// A pull that lands while the origin chapter itself has unsaved edits (the author
/// typed during a network-slow pull) must not clobber them from disk.
#[test]
fn a_pull_landing_over_unsaved_edits_keeps_them() {
    let (mut app, _dir) = two_chapter_app("vc-pull-dirty");
    app.spawn_landing_pull_for_test(); // origin: the active chapter
    type_str(&mut app, "typed while the pull was in flight");
    assert!(app.buffer.dirty);

    app.settle_vc_for_test();

    assert!(
        app.buffer
            .text()
            .contains("typed while the pull was in flight"),
        "unsaved edits survived the pull: {:?}",
        app.buffer.text()
    );
    assert!(
        app.buffer.dirty,
        "the buffer wasn't reloaded out from under them"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("unsaved edits"),
        "the author is told their edits were kept: {:?}",
        app.message
    );
}

/// The clean, common path is unchanged: a pull that lands with the author still on
/// the origin chapter and no unsaved edits reloads the buffer from disk.
#[test]
fn a_clean_pull_still_reloads_the_active_chapter() {
    let (mut app, dir) = two_chapter_app("vc-pull-clean");
    // The pulled state on disk for the active chapter.
    fs::write(dir.join("one.md"), "pulled-in content\n").unwrap();

    app.spawn_landing_pull_for_test(); // still on the origin, buffer clean
    app.settle_vc_for_test();

    assert!(
        app.buffer.text().contains("pulled-in content"),
        "a clean pull reloaded the active chapter from disk: {:?}",
        app.buffer.text()
    );
}
