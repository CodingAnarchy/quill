//! Regression tests for background-VCS concurrency/state bugs (bug F + bug H).
//!
//! Bug F (poll-mode clobber): a background two-way sync (#962) lands via
//! [`poll_vc`](App::poll_vc) on *every* tick. When it lands with conflicts it must
//! never overwrite an active prompt/overlay/modal — the resolution overlay is
//! deferred until the author is back in a plain editing mode. And the SYNC path's
//! [`open_conflict_resolution`](App::open_conflict_resolution) must clear any stale
//! `pending_draft_merge`, so a draft-merge left mid-resolution can never misroute the
//! shared finalize step to the wrong refs.
//!
//! Bug H (read-only bypass): the background git ops that force-checkout the tree via
//! libgit2 — sync, pull, draft switch, draft merge — must refuse when the project is
//! open read-only (#871), mirroring the save-path write guards, so a secondary
//! read-only instance can't clobber the owning instance's chapter files.
//!
//! All hermetic (#531) — local scratch repos, no network.

use super::*;

/// A two-chapter project already under version control with a first commit, so the
/// drafts/sync flows have something to work with. Not read-only.
fn committed_two_chapter(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    fs::write(dir.join("two.md"), "second chapter\n").unwrap();
    crate::vcs::init(&dir).unwrap();
    crate::vcs::commit(&dir, "first").unwrap();

    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

/// This host's name, matching the lock module's derivation, so a seeded lockfile
/// reads as same-host (mirrors the onboarding / hardening tests).
#[cfg(unix)]
fn host() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| "unknown-host".to_string())
}

/// Build a project under version control with a remote set, then seed a *live*
/// other-instance lock (pid 1, always alive on Unix) before opening it, so it opens
/// read-only (#871) — the shape a genuine second Quill's lock drives. A unit test
/// can't fork a second editor, so this stands in for one. Unix-only, matching the
/// lock's platform coverage.
#[cfg(unix)]
fn read_only_remote_app(name: &str) -> (App, TempProject, PathBuf) {
    let remote = app_scratch_dir(&format!("{name}-bare"));
    git2::Repository::init_bare(&remote).unwrap();
    let url = remote.to_str().unwrap();

    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "mainline chapter\n").unwrap();
    crate::vcs::init(&dir).unwrap();
    crate::vcs::commit(&dir, "first").unwrap();
    crate::vcs::create_branch(&dir, "alt").unwrap();
    crate::vcs::set_remote(&dir, url).unwrap();

    // Seed the live lock so the project opens read-only.
    std::fs::create_dir_all(dir.join(".quill")).unwrap();
    std::fs::write(
        dir.join(".quill").join("lock"),
        format!("1\n{}\n0\n", host()),
    )
    .unwrap();

    let app = open_test_app(&dir);
    assert!(
        app.project.is_read_only(),
        "the seeded live lock opens the project read-only"
    );
    (app, TempProject::new(dir), remote)
}

// ---------------------------------------------------------------------------
// Bug F(1): a background sync conflict does not clobber a non-editing mode.
// ---------------------------------------------------------------------------

#[test]
fn a_landing_sync_conflict_does_not_clobber_an_open_prompt() {
    // A background sync lands with conflicts while the author is mid-prompt (naming a
    // milestone). poll_vc must NOT overwrite the prompt with the resolution overlay —
    // the prompt and its pending state survive; the overlay is deferred.
    let (mut app, _dir) = committed_two_chapter("vc-conc-defer-prompt");

    // Open a milestone-name prompt (a non-editing mode).
    app.on_key(ctrl('v'));
    // Run the "Milestone" palette item.
    let idx = match &app.mode {
        Mode::Palette(p) => p
            .items
            .iter()
            .position(|it| it.label == "Milestone")
            .expect("a Milestone item exists once under VC"),
        _ => panic!("expected the Git palette"),
    };
    for _ in 0..idx {
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "the milestone-name prompt is open"
    );

    // A background sync lands with conflicts while the prompt is up.
    app.spawn_sync_conflicts_for_test(&["one.md"]);
    app.settle_vc_for_test();

    // The prompt was NOT clobbered — the resolution overlay was deferred.
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "the open prompt survived a landing sync conflict: {:?}",
        std::mem::discriminant(&app.mode)
    );
    assert!(
        !app.sync_conflicts().is_empty(),
        "the conflict is stashed for when the author returns"
    );

    // Dismiss the prompt → back to a plain editing mode. The next poll_vc tick now
    // surfaces the deferred conflict resolution.
    app.on_key(key(KeyCode::Esc));
    assert!(
        matches!(app.mode, Mode::Insert | Mode::Normal),
        "the prompt closed back to the editor"
    );
    app.poll_vc();
    assert!(
        matches!(app.mode, Mode::ConflictReview(_)),
        "the deferred conflict resolution opened once back in the editor"
    );
}

#[test]
fn a_landing_sync_conflict_opens_immediately_when_in_the_editor() {
    // The common path is unchanged: a sync conflict that lands while the author is in
    // a plain editing mode opens the resolution overlay straight away.
    let (mut app, _dir) = committed_two_chapter("vc-conc-open-immediate");
    assert!(matches!(app.mode, Mode::Insert), "starts in the editor");

    app.spawn_sync_conflicts_for_test(&["one.md"]);
    app.settle_vc_for_test();

    assert!(
        matches!(app.mode, Mode::ConflictReview(_)),
        "a sync conflict landing in the editor opens the overlay at once"
    );
}

// ---------------------------------------------------------------------------
// Bug F(2): open_conflict_resolution (sync path) clears a stale draft merge.
// ---------------------------------------------------------------------------

#[test]
fn opening_sync_resolution_clears_a_stale_pending_draft_merge() {
    // If a draft merge was left mid-resolution (pending_draft_merge set) and a
    // background SYNC conflict now opens its own resolution flow, the sync path must
    // clear the stale draft-merge target — otherwise finalize_conflict_resolution
    // would take it FIRST and misroute the sync resolutions to finalize_draft_merge
    // against the wrong refs.
    let (mut app, _dir) = committed_two_chapter("vc-conc-clear-draft");

    // Simulate a draft merge left mid-resolution.
    app.pending_draft_merge = Some(("main".into(), "alt".into()));
    // A sync landed with conflicts (stashed for resolution).
    app.spawn_sync_conflicts_for_test(&["one.md"]);
    app.settle_vc_for_test();

    // The sync resolution opened AND the stale draft-merge target was cleared, so a
    // subsequent finalize routes to the sync path (finalize_sync) not the draft path.
    assert!(
        matches!(app.mode, Mode::ConflictReview(_)),
        "the sync resolution overlay opened"
    );
    assert!(
        app.pending_draft_merge.is_none(),
        "the stale draft-merge target was cleared by the sync path"
    );
}

#[test]
fn a_real_sync_resolution_finalizes_against_the_remote_despite_a_stale_draft_merge() {
    // End-to-end: a real two-way sync conflict, with a stale draft-merge planted, must
    // finalize as a SYNC merge (clean tree, chosen text) — proving the resolution
    // routed to finalize_sync, not finalize_draft_merge against bogus refs.
    let (mut app, b, a, remote) = conflicted_sync_app_local("vc-conc-real-sync");

    // Plant a stale draft-merge target (bug F), as if a draft merge had been abandoned.
    app.pending_draft_merge = Some(("main".into(), "alt".into()));

    git_menu_label(&mut app, "Sync");
    app.settle_vc_for_test();
    assert!(
        matches!(app.mode, Mode::ConflictReview(_)),
        "the sync resolution overlay opened"
    );
    assert!(
        app.pending_draft_merge.is_none(),
        "the stale draft-merge target was cleared"
    );

    // Keep local → the sync finalizes against the remote.
    app.on_key(key(KeyCode::Char('l')));
    assert!(
        matches!(app.mode, Mode::Insert),
        "the flow completed back to the editor"
    );
    assert!(app.sync_conflicts().is_empty(), "conflicts cleared");
    assert_eq!(
        fs::read_to_string(b.join("one.md")).unwrap(),
        "B's rewrite\n",
        "the local text was kept via the sync path"
    );
    // A real sync merge landed: a fresh commit sees a clean tree.
    assert_eq!(crate::vcs::commit(&b, "clean").unwrap(), None);

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&remote).ok();
}

/// Build a real bare remote plus a clone `b` opened as an app, driven into a
/// same-chapter conflict against `a`'s pushed edit (#963). `b`'s `one.md` holds "B's
/// rewrite"; the remote holds "A's rewrite" — a real conflict a live sync detects.
/// (A local copy of the vcs-test helper so this module is self-contained.)
fn conflicted_sync_app_local(name: &str) -> (App, TempProject, PathBuf, PathBuf) {
    use crate::vcs::{commit, push, set_remote};

    let manifest = "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n";

    let remote = app_scratch_dir(&format!("{name}-bare"));
    git2::Repository::init_bare(&remote).unwrap();
    let url = remote.to_str().unwrap();

    let a = app_scratch_dir(&format!("{name}-a"));
    fs::write(Project::manifest_path(&a), manifest).unwrap();
    fs::write(a.join("one.md"), "shared draft\n").unwrap();
    crate::vcs::init(&a).unwrap();
    commit(&a, "first").unwrap();
    set_remote(&a, url).unwrap();
    push(&a).unwrap();

    let b = app_scratch_dir(&format!("{name}-b"));
    fs::remove_dir_all(&b).ok();
    git2::Repository::clone(url, &b).unwrap();
    fs::write(b.join("one.md"), "B's rewrite\n").unwrap();
    commit(&b, "b-edit").unwrap();

    fs::write(a.join("one.md"), "A's rewrite\n").unwrap();
    commit(&a, "a-edit").unwrap();
    push(&a).unwrap();

    let app = open_test_app(&b);
    (app, TempProject::new(b), a, remote)
}

/// Open the Git palette and run the item with `label`.
fn git_menu_label(app: &mut App, label: &str) {
    app.on_key(ctrl('v'));
    let index = match &app.mode {
        Mode::Palette(p) => p
            .items
            .iter()
            .position(|it| it.label == label)
            .unwrap_or_else(|| panic!("no Git-palette item labelled {label:?}")),
        _ => panic!("expected the Git palette to open"),
    };
    for _ in 0..index {
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Enter));
}

// ---------------------------------------------------------------------------
// Bug F(3): drafts / start entry points refuse while a VCS op is in flight.
// ---------------------------------------------------------------------------

#[test]
fn opening_drafts_refuses_while_a_background_sync_runs() {
    // With vc_task busy (a sync in flight), opening Drafts must no-op with a message
    // rather than start an overlapping VCS flow that a landing sync could clobber.
    let (mut app, _dir) = committed_two_chapter("vc-conc-drafts-busy");

    // Start a background sync (leaves vc_task busy).
    app.spawn_sync_conflicts_for_test(&["one.md"]);
    assert!(app.vc_task.is_busy(), "the sync is in flight");

    app.open_drafts();
    assert!(
        !matches!(app.mode, Mode::Drafts(_)),
        "drafts refused to open while a sync is in flight"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("still running"),
        "the author is told why: {:?}",
        app.message
    );

    // Once the sync settles, drafts open normally.
    app.settle_vc_for_test();
    // Settling opened the conflict resolution (in-editor) — dismiss it, then Drafts work.
    if matches!(app.mode, Mode::ConflictReview(_)) {
        app.on_key(key(KeyCode::Esc));
    }
    app.open_drafts();
    assert!(
        matches!(app.mode, Mode::Drafts(_)),
        "drafts open once the sync has settled"
    );
}

// ---------------------------------------------------------------------------
// Bug H: read-only instance refuses tree-rewriting VCS ops.
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn a_read_only_instance_refuses_to_sync() {
    let (mut app, _dir, remote) = read_only_remote_app("vc-ro-sync");
    git_menu_label(&mut app, "Sync");
    assert!(
        !app.vc_task.is_busy(),
        "no background sync was spawned in read-only"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("Read-only"),
        "the author is told the project is read-only: {:?}",
        app.message
    );
    fs::remove_dir_all(&remote).ok();
}

#[cfg(unix)]
#[test]
fn a_read_only_instance_refuses_to_pull() {
    let (mut app, _dir, remote) = read_only_remote_app("vc-ro-pull");
    git_menu_label(&mut app, "Pull");
    assert!(
        !app.vc_task.is_busy(),
        "no background pull was spawned in read-only"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("Read-only"),
        "the author is told the project is read-only: {:?}",
        app.message
    );
    fs::remove_dir_all(&remote).ok();
}

#[cfg(unix)]
#[test]
fn a_read_only_instance_refuses_to_switch_drafts() {
    let (mut app, dir, remote) = read_only_remote_app("vc-ro-switch");
    let before = fs::read_to_string(dir.join("one.md")).unwrap();

    // Open the drafts overlay directly (open_drafts is a read op; the checkout is
    // what's guarded) and select the other draft, then switch (Enter).
    app.open_drafts();
    if let Mode::Drafts(menu) = &mut app.mode {
        menu.selected = menu.drafts.iter().position(|d| d == "alt").unwrap_or(0);
    }
    app.on_key(key(KeyCode::Enter)); // switch_selected_draft

    assert!(
        app.message.as_deref().unwrap_or("").contains("Read-only"),
        "the author is told the project is read-only: {:?}",
        app.message
    );
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "the tree was not force-checked-out"
    );
    fs::remove_dir_all(&remote).ok();
}

#[cfg(unix)]
#[test]
fn a_read_only_instance_refuses_to_merge_drafts() {
    let (mut app, dir, remote) = read_only_remote_app("vc-ro-merge");
    // Give the "alt" draft a divergent commit so a merge would actually do work.
    // (Done on disk before the read-only app acts; the app itself must refuse.)
    let before = fs::read_to_string(dir.join("one.md")).unwrap();

    app.open_drafts();
    if let Mode::Drafts(menu) = &mut app.mode {
        menu.selected = menu.drafts.iter().position(|d| d == "alt").unwrap_or(0);
    }
    app.on_key(key(KeyCode::Char('m'))); // merge_selected_draft

    assert!(
        app.message.as_deref().unwrap_or("").contains("Read-only"),
        "the author is told the project is read-only: {:?}",
        app.message
    );
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "the tree was not force-checked-out"
    );
    fs::remove_dir_all(&remote).ok();
}
