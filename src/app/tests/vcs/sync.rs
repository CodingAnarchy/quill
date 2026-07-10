//! Remote configuration and two-way sync (#962/#963): the remote/push/pull/sync
//! palette gating, per-chapter conflict stashing, the resolution flow (keep local /
//! take remote / merge / cancel), and dead-worker cause reporting. Pure move.
use super::super::*;
use super::*;

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

    // Set a remote through the palette.
    git_menu_label(&mut app, "Set remote");
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

    // Now the menu offers Push, Pull, and Sync, and "Change remote" instead of
    // "Set remote".
    app.on_key(ctrl('v'));
    let labels = palette_labels(&app);
    assert!(
        labels.contains(&"Push") && labels.contains(&"Pull") && labels.contains(&"Sync"),
        "{labels:?}"
    );
    assert!(
        labels.contains(&"Change remote") && !labels.contains(&"Set remote"),
        "{labels:?}"
    );
}

// --- Two-way sync (#962) -----------------------------------------------------

#[test]
fn sync_is_offered_only_once_a_remote_is_configured() {
    let (mut app, _dir) = two_chapter_app("vc-sync-gated");
    git_menu(&mut app, 0); // Initialize
    app.on_key(ctrl('v'));
    assert!(
        !palette_labels(&app).contains(&"Sync"),
        "no Sync before a remote is set"
    );
    app.on_key(key(KeyCode::Esc));

    git_menu_label(&mut app, "Set remote");
    type_str(&mut app, "https://example.com/book.git");
    app.on_key(key(KeyCode::Enter));

    app.on_key(ctrl('v'));
    assert!(
        palette_labels(&app).contains(&"Sync"),
        "Sync appears once a remote is configured"
    );
}

#[test]
fn a_landing_sync_stashes_per_chapter_conflicts_for_resolution() {
    // A two-way sync that detects same-chapter conflicts (#962) surfaces them for the
    // resolution UI (#963) via `sync_conflicts`, never clobbering a side. Driven with
    // a stand-in worker so no real remote is needed; the conflict detection itself is
    // covered end-to-end by the vcs tests.
    let (mut app, _dir) = two_chapter_app("vc-sync-conflicts");
    git_menu(&mut app, 0); // Initialize
    assert!(app.sync_conflicts().is_empty(), "none before a sync");

    app.spawn_sync_conflicts_for_test(&["one.md", "two.md"]);
    app.settle_vc_for_test();

    let paths: Vec<&str> = app
        .sync_conflicts()
        .iter()
        .map(|c| c.path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec!["one.md", "two.md"],
        "both conflicts are stashed"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("both machines"),
        "the author is told chapters conflict: {:?}",
        app.message
    );
}

#[test]
fn a_clean_sync_clears_any_prior_conflicts() {
    // A subsequent clean sync (or pull) leaves no conflicts, clearing a prior set so
    // the resolution UI never shows stale entries.
    let (mut app, _dir) = two_chapter_app("vc-sync-clears");
    git_menu(&mut app, 0); // Initialize
    app.spawn_sync_conflicts_for_test(&["one.md"]);
    app.settle_vc_for_test();
    assert!(!app.sync_conflicts().is_empty(), "a conflict was stashed");

    // A clean pull carries no conflicts → the prior set is cleared.
    app.spawn_landing_pull_for_test();
    app.settle_vc_for_test();
    assert!(
        app.sync_conflicts().is_empty(),
        "a clean sync/pull clears the prior conflicts"
    );
}

/// Build a real bare remote plus a clone `b` opened as an app, driven into a
/// same-chapter conflict against `a`'s pushed edit (#963). Returns the app on
/// `b`, its `TempProject`, and the two extra scratch dirs to clean up. `b`'s
/// `one.md` holds "B's rewrite"; the remote holds "A's rewrite" — a real conflict
/// a live sync will detect.
fn conflicted_sync_app(name: &str) -> (App, TempProject, PathBuf, PathBuf) {
    use crate::vcs::{commit, push, set_remote};

    let manifest = "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n";

    let remote = app_scratch_dir(&format!("{name}-bare"));
    git2::Repository::init_bare(&remote).unwrap();
    let url = remote.to_str().unwrap();

    // Clone a: seed the shared first commit, push it.
    let a = app_scratch_dir(&format!("{name}-a"));
    fs::write(Project::manifest_path(&a), manifest).unwrap();
    fs::write(a.join("one.md"), "shared draft\n").unwrap();
    crate::vcs::init(&a).unwrap();
    commit(&a, "first").unwrap();
    set_remote(&a, url).unwrap();
    push(&a).unwrap();

    // Clone b (the app): clone the remote, then edit one.md differently and commit —
    // both sides now diverge on the same chapter.
    let b = app_scratch_dir(&format!("{name}-b"));
    fs::remove_dir_all(&b).ok();
    git2::Repository::clone(url, &b).unwrap();
    fs::write(b.join("one.md"), "B's rewrite\n").unwrap();
    commit(&b, "b-edit").unwrap();

    // a rewrites the same chapter and pushes → the remote now conflicts with b.
    fs::write(a.join("one.md"), "A's rewrite\n").unwrap();
    commit(&a, "a-edit").unwrap();
    push(&a).unwrap();

    let app = open_test_app(&b);
    (app, TempProject::new(b), a, remote)
}

#[test]
fn a_sync_conflict_opens_the_resolution_flow_with_a_safety_snapshot() {
    // A real two-way sync that hits a same-chapter conflict (#963) opens the
    // resolution overlay, tags a safety milestone (neither version lost), and
    // leaves the local file exactly as it was until the author resolves.
    let (mut app, b, a, remote) = conflicted_sync_app("vc-resolve-open");
    let before = fs::read_to_string(b.join("one.md")).unwrap();

    git_menu_label(&mut app, "Sync");
    app.settle_vc_for_test();

    assert!(
        matches!(app.mode, Mode::ConflictReview(_)),
        "the resolution overlay opened"
    );
    assert_eq!(app.sync_conflicts().len(), 1, "one chapter conflicts");
    // A safety milestone was tagged before anything was applied.
    let tags = crate::vcs::list_tags(&b).unwrap();
    assert!(
        tags.iter().any(|t| t.name.starts_with("pre-sync-")),
        "a safety snapshot exists: {:?}",
        tags.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
    // Nothing was clobbered before the author chose — the local text is untouched.
    assert_eq!(fs::read_to_string(b.join("one.md")).unwrap(), before);

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn keeping_local_completes_the_sync_with_the_local_text() {
    let (mut app, b, a, remote) = conflicted_sync_app("vc-resolve-local");
    git_menu_label(&mut app, "Sync");
    app.settle_vc_for_test();

    app.on_key(key(KeyCode::Char('l'))); // keep local

    assert!(
        matches!(app.mode, Mode::Insert),
        "the flow completed back to the editor"
    );
    assert!(app.sync_conflicts().is_empty(), "conflicts cleared");
    assert_eq!(
        fs::read_to_string(b.join("one.md")).unwrap(),
        "B's rewrite\n"
    );
    // A real merge landed: a fresh commit sees a clean tree.
    assert_eq!(crate::vcs::commit(&b, "clean").unwrap(), None);

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn taking_remote_completes_the_sync_with_the_remote_text() {
    let (mut app, b, a, remote) = conflicted_sync_app("vc-resolve-remote");
    git_menu_label(&mut app, "Sync");
    app.settle_vc_for_test();

    app.on_key(key(KeyCode::Char('r'))); // take remote

    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(
        fs::read_to_string(b.join("one.md")).unwrap(),
        "A's rewrite\n"
    );
    assert_eq!(crate::vcs::commit(&b, "clean").unwrap(), None);

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn merging_lands_the_hand_edited_text() {
    let (mut app, b, a, remote) = conflicted_sync_app("vc-resolve-merge");
    git_menu_label(&mut app, "Sync");
    app.settle_vc_for_test();

    app.on_key(key(KeyCode::Char('m'))); // open the merge editor
    assert!(
        matches!(&app.mode, Mode::ConflictReview(r) if r.editing.is_some()),
        "the merge editor opened, seeded with both versions"
    );
    // The seed carries both versions, so neither is lost going in.
    let seeded = app.edit_buffer().text();
    assert!(
        seeded.contains("B's rewrite") && seeded.contains("A's rewrite"),
        "{seeded}"
    );

    // Replace it with a blended version and accept (Ctrl-S).
    *app.edit_buffer_mut() = Buffer::from_text("Both rewrites, blended\n");
    app.on_key(ctrl('s'));

    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(
        fs::read_to_string(b.join("one.md")).unwrap(),
        "Both rewrites, blended\n"
    );
    assert_eq!(crate::vcs::commit(&b, "clean").unwrap(), None);

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn cancelling_resolution_keeps_both_versions() {
    let (mut app, b, a, remote) = conflicted_sync_app("vc-resolve-cancel");
    git_menu_label(&mut app, "Sync");
    app.settle_vc_for_test();

    app.on_key(key(KeyCode::Esc)); // abandon the flow

    assert!(matches!(app.mode, Mode::Insert));
    // Local is untouched…
    assert_eq!(
        fs::read_to_string(b.join("one.md")).unwrap(),
        "B's rewrite\n"
    );
    // …and the safety snapshot (with both committed sides reachable) survives.
    let tags = crate::vcs::list_tags(&b).unwrap();
    assert!(tags.iter().any(|t| t.name.starts_with("pre-sync-")));

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn a_dead_vc_worker_surfaces_its_cause_not_a_generic_message() {
    // A push/pull worker that dies used to report a bare "Remote operation
    // failed." — the real reason (auth, network, rejected) was swallowed (#838).
    // Drive the death path with a failing worker and assert the cause reaches the
    // author-facing message.
    let (mut app, _dir) = two_chapter_app("vc-worker-died");
    git_menu(&mut app, 0); // Initialize
    app.spawn_failing_vc_for_test("remote authentication failed");
    app.settle_vc_for_test();
    let msg = app.message.as_deref().unwrap_or("");
    assert!(
        msg.contains("remote authentication failed"),
        "the message should name the underlying cause, got {msg:?}"
    );
    assert_ne!(
        msg, "Remote operation failed.",
        "a dead worker must not fall back to the generic message when a cause is known"
    );
}
