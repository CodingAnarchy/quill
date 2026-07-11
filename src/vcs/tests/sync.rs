//! Two-way sync tests (#962) and its hand-resolved finalization (#963): disjoint
//! chapter edits that merge cleanly, a same-chapter conflict that's detected without
//! clobbering, the fast-forward and dirty-tree guards, nested-path reconciliation,
//! the delete/add reconciliation arms (delete-vs-edit conflicts, delete-vs-unchanged
//! applies the deletion either direction, both-delete converges, add/add conflicts),
//! and the finalize paths (keep local / take remote / hand-merge / mixed / stale
//! remote). A bare repo stands in for the remote, so nothing reaches the network.

use std::collections::BTreeMap;
use std::fs;

use super::temp_dir;
use crate::vcs::*;

/// Set up a bare stand-in remote plus two working clones `a` and `b`, both holding
/// the same seeded `one.md` first commit. The shared scaffold for the sync tests —
/// no network, everything under the scratch dir.
fn two_clones(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let remote = temp_dir(&format!("{name}-bare"));
    git2::Repository::init_bare(&remote).unwrap();
    let url = remote.to_str().unwrap();

    let a = temp_dir(&format!("{name}-a"));
    init(&a).unwrap();
    fs::write(a.join("one.md"), "# One\n\nShared draft.\n").unwrap();
    commit(&a, "first").unwrap();
    set_remote(&a, url).unwrap();
    push(&a).unwrap();

    let b = temp_dir(&format!("{name}-b"));
    fs::remove_dir_all(&b).ok();
    git2::Repository::clone(url, &b).unwrap();

    (remote, a, b)
}

/// Drive two clones into a same-chapter conflict and run [`sync`] on `b`, which
/// detects it without writing anything. Returns the scratch dirs plus the detected
/// conflict, so each resolution-path test starts from the exact post-sync state the
/// resolution UI acts on.
fn conflicted_clones(
    name: &str,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    ChapterConflict,
) {
    let (remote, a, b) = two_clones(name);

    fs::write(a.join("one.md"), "# One\n\nA's rewrite.\n").unwrap();
    commit(&a, "a-edit").unwrap();
    push(&a).unwrap();
    fs::write(b.join("one.md"), "# One\n\nB's rewrite.\n").unwrap();
    commit(&b, "b-edit").unwrap();

    let out = sync(&b).unwrap();
    assert_eq!(out.conflicts.len(), 1, "one chapter conflicts");
    let conflict = out.conflicts.into_iter().next().unwrap();
    (remote, a, b, conflict)
}

/// The merge commit b now sits on: both parents' summaries plus its own message,
/// so a test can assert the two-parent merge landed and both histories are reachable.
fn merge_summaries(dir: &std::path::Path) -> Vec<String> {
    log(dir, 20)
        .unwrap()
        .into_iter()
        .map(|c| c.summary)
        .collect()
}

#[test]
fn sync_merges_disjoint_chapter_edits_cleanly() {
    // Two machines edit *different* chapters concurrently, then one pushes. The
    // other syncs a diverged branch: every chapter was touched by only one side, so
    // the chapter-level three-way merges with no conflicts and both sides' work
    // lands on disk.
    let (remote, a, b) = two_clones("sync-disjoint");

    // a adds two.md and pushes; b edits its own one.md but hasn't pushed.
    fs::write(a.join("two.md"), "# Two\n\nA's chapter.\n").unwrap();
    commit(&a, "a-adds-two").unwrap();
    push(&a).unwrap();
    fs::write(b.join("one.md"), "# One\n\nB's revised draft.\n").unwrap();
    commit(&b, "b-edits-one").unwrap();

    let out = sync(&b).unwrap();
    assert!(out.conflicts.is_empty(), "disjoint edits don't conflict");
    assert!(out.reload, "the tree changed on disk");
    assert!(
        out.merged.iter().any(|p| p == "two.md"),
        "a's chapter merged in: {:?}",
        out.merged
    );
    // a's added chapter is now on disk in b…
    assert!(b.join("two.md").exists(), "a's chapter merged onto b");
    assert!(fs::read_to_string(b.join("two.md"))
        .unwrap()
        .contains("A's chapter"));
    // …and b's own concurrent edit is preserved untouched.
    assert!(fs::read_to_string(b.join("one.md"))
        .unwrap()
        .contains("B's revised draft"));

    // The reconciliation is a real two-parent merge commit — both histories reachable.
    let summaries: Vec<String> = log(&b, 20)
        .unwrap()
        .into_iter()
        .map(|c| c.summary)
        .collect();
    assert!(summaries.iter().any(|s| s == "a-adds-two"), "{summaries:?}");
    assert!(
        summaries.iter().any(|s| s == "b-edits-one"),
        "{summaries:?}"
    );
    assert!(
        summaries.iter().any(|s| s.contains("Sync remote")),
        "{summaries:?}"
    );
    // b is now ahead of the remote and can push the merge.
    push(&b).unwrap();

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn sync_detects_a_same_chapter_conflict_without_clobbering() {
    // Both machines edit the *same* chapter differently. Sync must detect a
    // per-chapter conflict, preserve both versions (local on disk, remote in the
    // returned conflict), and leave the repo untouched — never a half-merged tree.
    let (remote, a, b) = two_clones("sync-conflict");

    fs::write(
        a.join("one.md"),
        "# One\n\nA's rewrite of the shared chapter.\n",
    )
    .unwrap();
    commit(&a, "a-edit").unwrap();
    push(&a).unwrap();
    let b_content = "# One\n\nB's rewrite of the shared chapter.\n";
    fs::write(b.join("one.md"), b_content).unwrap();
    commit(&b, "b-edit").unwrap();

    let out = sync(&b).unwrap();
    assert_eq!(out.conflicts.len(), 1, "one chapter conflicts");
    assert!(!out.reload, "nothing was written on a conflict");
    let conflict = &out.conflicts[0];
    assert_eq!(conflict.path, "one.md");
    // Both versions are preserved for the resolution UI (#963), neither clobbered.
    assert!(
        conflict.local.contains("B's rewrite"),
        "local kept: {:?}",
        conflict.local
    );
    assert!(
        conflict.remote.contains("A's rewrite"),
        "remote kept: {:?}",
        conflict.remote
    );
    assert!(
        conflict.base.contains("Shared draft"),
        "the ancestor is carried: {:?}",
        conflict.base
    );
    assert!(out.summary.contains("both machines"), "{}", out.summary);

    // b's committed work is exactly as it was — no conflict markers, no half-merge.
    assert_eq!(fs::read_to_string(b.join("one.md")).unwrap(), b_content);
    // The repo isn't left mid-merge: a fresh commit sees a clean tree.
    assert_eq!(commit(&b, "should-be-clean").unwrap(), None);

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn sync_fast_forwards_when_only_the_remote_moved() {
    // b is strictly behind: a pushed a new commit and b has none of its own. Sync
    // fast-forwards, exactly like a pull, and reports no per-chapter merges.
    let (remote, a, b) = two_clones("sync-ff");

    fs::write(a.join("one.md"), "# One\n\nRevised on a.\n").unwrap();
    commit(&a, "second").unwrap();
    push(&a).unwrap();

    let out = sync(&b).unwrap();
    assert!(out.conflicts.is_empty() && out.merged.is_empty());
    assert!(out.reload, "the fast-forward changed the tree");
    assert!(fs::read_to_string(b.join("one.md"))
        .unwrap()
        .contains("Revised on a"));

    // A second sync with nothing new is a no-op, not an error.
    let again = sync(&b).unwrap();
    assert!(
        again.summary.to_lowercase().contains("in sync"),
        "{}",
        again.summary
    );
    assert!(!again.reload);

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn sync_refuses_a_dirty_tree_and_preserves_edits() {
    // #639 guard applies to sync too: an uncommitted edit must not be overwritten by
    // the merge checkout. Sync refuses before touching the network and leaves the
    // edit on disk.
    let (remote, a, b) = two_clones("sync-dirty");

    fs::write(a.join("one.md"), "# One\n\nRevised on a.\n").unwrap();
    commit(&a, "second").unwrap();
    push(&a).unwrap();

    let uncommitted = "# One\n\nUnsaved-to-git work on b.\n";
    fs::write(b.join("one.md"), uncommitted).unwrap();
    let err = sync(&b).unwrap_err();
    assert!(err.to_lowercase().contains("uncommitted"), "{err}");
    // The author's words are still on disk.
    assert_eq!(fs::read_to_string(b.join("one.md")).unwrap(), uncommitted);

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn finalize_sync_keeps_the_local_version() {
    // Keep-local: the author's chosen text is their local version. finalize_sync
    // writes it and lands the two-parent merge — nothing from the remote clobbers it.
    let (remote, a, b, conflict) = conflicted_clones("finalize-local");

    let mut resolutions = BTreeMap::new();
    resolutions.insert(conflict.path.clone(), conflict.local.clone());
    let summary = finalize_sync(&b, &resolutions).unwrap();
    assert!(summary.contains("completed the sync"), "{summary}");

    // The chosen (local) text is on disk and the tree is clean (a real merge landed).
    assert_eq!(
        fs::read_to_string(b.join("one.md")).unwrap(),
        conflict.local
    );
    assert_eq!(commit(&b, "should-be-clean").unwrap(), None);
    // Both parents are reachable through the two-parent merge.
    let summaries = merge_summaries(&b);
    assert!(summaries.iter().any(|s| s == "a-edit"), "{summaries:?}");
    assert!(summaries.iter().any(|s| s == "b-edit"), "{summaries:?}");
    assert!(
        summaries.iter().any(|s| s.contains("Sync remote")),
        "{summaries:?}"
    );

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn finalize_sync_takes_the_remote_version() {
    // Take-remote: the chosen text is the fetched remote version. It replaces local
    // on disk, and the merge commit still records both parents.
    let (remote, a, b, conflict) = conflicted_clones("finalize-remote");

    let mut resolutions = BTreeMap::new();
    resolutions.insert(conflict.path.clone(), conflict.remote.clone());
    finalize_sync(&b, &resolutions).unwrap();

    assert_eq!(
        fs::read_to_string(b.join("one.md")).unwrap(),
        conflict.remote
    );
    assert!(conflict.remote.contains("A's rewrite"), "sanity");
    assert_eq!(commit(&b, "should-be-clean").unwrap(), None);
    let summaries = merge_summaries(&b);
    assert!(summaries.iter().any(|s| s == "a-edit"), "{summaries:?}");
    assert!(summaries.iter().any(|s| s == "b-edit"), "{summaries:?}");

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn finalize_sync_lands_a_hand_merged_version() {
    // Merge: the author blends both into a new text. finalize_sync writes exactly
    // that and completes the sync, losing neither side's ideas.
    let (remote, a, b, conflict) = conflicted_clones("finalize-merge");

    let blended = "# One\n\nA's rewrite, plus B's rewrite.\n";
    let mut resolutions = BTreeMap::new();
    resolutions.insert(conflict.path, blended.to_string());
    finalize_sync(&b, &resolutions).unwrap();

    assert_eq!(fs::read_to_string(b.join("one.md")).unwrap(), blended);
    assert_eq!(commit(&b, "should-be-clean").unwrap(), None);
    // b is ahead of the remote and can push the completed merge.
    push(&b).unwrap();

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn finalize_sync_also_lands_the_clean_remote_only_edits() {
    // A conflict on one.md alongside a clean remote-only new chapter: resolving the
    // conflict lands both the resolution and the adopted remote edit in one merge.
    let (remote, a, b) = two_clones("finalize-mixed");

    // a adds a brand-new chapter (clean, remote-only) AND edits the shared one.
    fs::write(a.join("two.md"), "# Two\n\nA's new chapter.\n").unwrap();
    fs::write(a.join("one.md"), "# One\n\nA's rewrite.\n").unwrap();
    commit(&a, "a-adds-and-edits").unwrap();
    push(&a).unwrap();
    // b edits the shared one differently → a conflict on one.md only.
    fs::write(b.join("one.md"), "# One\n\nB's rewrite.\n").unwrap();
    commit(&b, "b-edit").unwrap();

    let out = sync(&b).unwrap();
    assert_eq!(out.conflicts.len(), 1);
    let conflict = out.conflicts.into_iter().next().unwrap();
    assert_eq!(conflict.path, "one.md");

    let mut resolutions = BTreeMap::new();
    resolutions.insert(conflict.path, conflict.local);
    finalize_sync(&b, &resolutions).unwrap();

    // The resolved chapter kept b's text…
    assert!(fs::read_to_string(b.join("one.md"))
        .unwrap()
        .contains("B's rewrite"));
    // …and the clean remote-only chapter was adopted in the same merge.
    assert!(
        b.join("two.md").exists(),
        "the clean remote edit landed too"
    );
    assert!(fs::read_to_string(b.join("two.md"))
        .unwrap()
        .contains("A's new chapter"));
    assert_eq!(commit(&b, "should-be-clean").unwrap(), None);

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn sync_merges_a_nested_chapter_across_a_divergence() {
    // Regression for the flat-TreeBuilder bug: real projects track nested files under
    // `bible/` and `submission/`. The three-way merge built the merged tree with a
    // single-level TreeBuilder, which rejects any entry name containing "/", so any
    // diverged sync touching a nested path aborted with a cryptic libgit2 error.
    // Here a adds `bible/setting.md` and edits an existing nested file while b makes a
    // disjoint edit — the reconcile must succeed and land the nested content on disk.
    let (remote, a, b) = two_clones("sync-nested");

    // Seed a nested chapter on both sides via the remote so it's in the merge base.
    fs::create_dir_all(a.join("bible")).unwrap();
    fs::write(a.join("bible/alice.md"), "# Alice\n\nOriginal note.\n").unwrap();
    commit(&a, "seed-nested").unwrap();
    push(&a).unwrap();
    // Bring the seeded nested file into b so both share it as the ancestor.
    let out = sync(&b).unwrap();
    assert!(out.conflicts.is_empty());
    assert!(b.join("bible/alice.md").exists(), "nested seed reached b");

    // a: add a brand-new nested chapter AND edit the shared nested one, then push.
    fs::write(
        a.join("bible/setting.md"),
        "# Setting\n\nThe fog-bound harbor.\n",
    )
    .unwrap();
    fs::write(
        a.join("bible/alice.md"),
        "# Alice\n\nRevised on a — sharper.\n",
    )
    .unwrap();
    commit(&a, "a-nested-edits").unwrap();
    push(&a).unwrap();

    // b: a disjoint edit to a top-level chapter (no overlap with a's nested changes).
    fs::write(b.join("one.md"), "# One\n\nB's disjoint edit.\n").unwrap();
    commit(&b, "b-edits-one").unwrap();

    // Before the fix this returned Err("invalid name for a tree entry - …").
    let out = sync(&b).unwrap();
    assert!(
        out.conflicts.is_empty(),
        "disjoint nested/top-level edits don't conflict: {:?}",
        out.conflicts
    );
    assert!(out.reload, "the merged tree was written to disk");

    // The new nested chapter landed with a's content…
    assert!(
        b.join("bible/setting.md").exists(),
        "the added nested chapter merged onto b"
    );
    assert_eq!(
        fs::read_to_string(b.join("bible/setting.md")).unwrap(),
        "# Setting\n\nThe fog-bound harbor.\n"
    );
    // …the edited nested chapter took a's revision (remote-only edit adopted)…
    assert!(fs::read_to_string(b.join("bible/alice.md"))
        .unwrap()
        .contains("Revised on a"));
    // …and b's own disjoint edit is preserved untouched.
    assert!(fs::read_to_string(b.join("one.md"))
        .unwrap()
        .contains("B's disjoint edit"));
    // A real, clean two-parent merge landed — a fresh commit sees nothing to do.
    assert_eq!(commit(&b, "should-be-clean").unwrap(), None);
    // And the completed merge pushes back cleanly.
    push(&b).unwrap();

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn finalize_sync_refuses_when_the_remote_changed_again_mid_resolution() {
    // Regression for the silent-drop bug: finalize_sync re-fetches the live remote and
    // rebuilds the three-way, but only applies the `resolutions` captured at detection
    // time. If a third push lands a fresh both-changed conflict on a *different*
    // chapter — one the author never resolved — the old code fell through every branch
    // and silently kept local, dropping the new remote edit under a two-parent merge.
    // It must instead refuse and tell the author to sync again.
    let (remote, a, b) = two_clones("finalize-stale");

    // Seed a second shared chapter so both sides can later diverge on it.
    fs::write(a.join("two.md"), "# Two\n\nShared second chapter.\n").unwrap();
    commit(&a, "seed-two").unwrap();
    push(&a).unwrap();
    let out = sync(&b).unwrap();
    assert!(out.conflicts.is_empty() && b.join("two.md").exists());

    // a and b diverge on one.md → the conflict the author will resolve.
    fs::write(a.join("one.md"), "# One\n\nA's rewrite.\n").unwrap();
    commit(&a, "a-edit-one").unwrap();
    push(&a).unwrap();
    fs::write(b.join("one.md"), "# One\n\nB's rewrite.\n").unwrap();
    commit(&b, "b-edit-one").unwrap();
    // b edits two.md as well (local side of a future conflict).
    fs::write(b.join("two.md"), "# Two\n\nB's take on two.\n").unwrap();
    commit(&b, "b-edit-two").unwrap();

    // b syncs and sees the one.md conflict; captures its resolution.
    let out = sync(&b).unwrap();
    let one_conflict = out
        .conflicts
        .into_iter()
        .find(|c| c.path == "one.md")
        .expect("one.md conflicts");

    // Now a THIRD push lands after detection: a edits two.md differently, so two.md is
    // now a genuine both-changed conflict that b never resolved.
    fs::write(a.join("two.md"), "# Two\n\nA's late take on two.\n").unwrap();
    commit(&a, "a-edit-two-late").unwrap();
    push(&a).unwrap();

    // The author finalizes with only the one.md resolution they knew about.
    let mut resolutions = BTreeMap::new();
    resolutions.insert(one_conflict.path, one_conflict.local);
    let err = finalize_sync(&b, &resolutions).unwrap_err();
    assert!(
        err.contains("changed again") && err.contains("two.md"),
        "refuses and names the newly-conflicting chapter: {err}"
    );

    // Nothing was committed: no merge landed, and a's late edit was NOT silently
    // dropped — b's local two.md is untouched and the tree still has b's own work.
    assert!(fs::read_to_string(b.join("two.md"))
        .unwrap()
        .contains("B's take on two"));
    let summaries = merge_summaries(&b);
    assert!(
        !summaries.iter().any(|s| s.contains("Sync remote")),
        "no partial merge commit landed: {summaries:?}"
    );

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

/// Seed a second shared chapter `two.md` on both clones (via the remote) so a delete
/// test has an ancestor blob to diverge from. Returns the scratch dirs; both `a` and
/// `b` hold `one.md` and `two.md` at the same commit afterwards.
fn two_shared_chapters(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let (remote, a, b) = two_clones(name);
    fs::write(a.join("two.md"), "# Two\n\nShared second chapter.\n").unwrap();
    commit(&a, "seed-two").unwrap();
    push(&a).unwrap();
    let out = sync(&b).unwrap();
    assert!(out.conflicts.is_empty() && b.join("two.md").exists());
    (remote, a, b)
}

#[test]
fn sync_conflicts_when_the_remote_deletes_a_chapter_the_local_edited() {
    // delete-vs-edit: the remote (a) deletes two.md while local (b) edits it. The
    // chapter-level three-way can't silently drop either side's intent — it's a
    // conflict, with the deleted side surfaced as empty and the edited side preserved.
    // On a conflict nothing is written and no commit lands (the tree stays byte-for-
    // byte as it was), so the author resolves it with the full picture.
    let (remote, a, b) = two_shared_chapters("sync-del-vs-edit");

    fs::remove_file(a.join("two.md")).unwrap();
    commit(&a, "a-deletes-two").unwrap();
    push(&a).unwrap();
    let b_content = "# Two\n\nB's revision of the second chapter.\n";
    fs::write(b.join("two.md"), b_content).unwrap();
    commit(&b, "b-edits-two").unwrap();

    let out = sync(&b).unwrap();
    assert_eq!(out.conflicts.len(), 1, "delete-vs-edit is a conflict");
    assert!(!out.reload, "nothing written on a conflict");
    let c = &out.conflicts[0];
    assert_eq!(c.path, "two.md");
    // The edited (local) side is preserved; the deleted (remote) side reads as empty.
    assert!(
        c.local.contains("B's revision"),
        "local edit kept: {:?}",
        c.local
    );
    assert_eq!(c.remote, "", "the deleted side is represented as empty");
    assert!(
        c.base.contains("Shared second chapter"),
        "ancestor carried: {:?}",
        c.base
    );

    // The tree is byte-unchanged and no merge commit landed.
    assert_eq!(fs::read_to_string(b.join("two.md")).unwrap(), b_content);
    assert_eq!(commit(&b, "should-be-clean").unwrap(), None);
    let summaries = merge_summaries(&b);
    assert!(
        !summaries.iter().any(|s| s.contains("Sync remote")),
        "no merge commit on a conflict: {summaries:?}"
    );

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn sync_applies_a_remote_deletion_of_an_unchanged_chapter() {
    // delete-vs-unchanged (remote deletes, local unchanged): the remote (a) deletes
    // two.md; local (b) never touched it. The deletion is a remote-only change, so it
    // is ADOPTED — the file is gone from the merged tree and off disk, not silently
    // kept — while b's disjoint edit to one.md is preserved.
    let (remote, a, b) = two_shared_chapters("sync-del-remote");

    fs::remove_file(a.join("two.md")).unwrap();
    commit(&a, "a-deletes-two").unwrap();
    push(&a).unwrap();
    // b makes a disjoint edit so the branch diverges (not a fast-forward).
    fs::write(b.join("one.md"), "# One\n\nB's disjoint edit.\n").unwrap();
    commit(&b, "b-edits-one").unwrap();

    let out = sync(&b).unwrap();
    assert!(
        out.conflicts.is_empty(),
        "an unchanged deletion is clean: {:?}",
        out.conflicts
    );
    assert!(out.reload, "the tree changed on disk");
    // The remote's deletion was applied: the file is gone from the merged tree and disk.
    assert!(
        !b.join("two.md").exists(),
        "remote deletion applied on disk"
    );
    // b's disjoint edit survives.
    assert!(fs::read_to_string(b.join("one.md"))
        .unwrap()
        .contains("B's disjoint edit"));
    // A real, clean two-parent merge landed.
    assert_eq!(commit(&b, "should-be-clean").unwrap(), None);
    let summaries = merge_summaries(&b);
    assert!(
        summaries.iter().any(|s| s.contains("Sync remote")),
        "{summaries:?}"
    );
    push(&b).unwrap();

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn sync_applies_a_local_deletion_of_a_chapter_the_remote_left_alone() {
    // delete-vs-unchanged (local deletes, remote unchanged): local (b) deletes two.md;
    // the remote (a) never touched it (it made a disjoint edit elsewhere). The local
    // deletion is kept — the merged tree has the file gone, not resurrected by the
    // remote's unchanged copy — while the remote's disjoint edit is adopted.
    let (remote, a, b) = two_shared_chapters("sync-del-local");

    // a makes a disjoint edit and pushes so the branch diverges.
    fs::write(a.join("one.md"), "# One\n\nA's disjoint edit.\n").unwrap();
    commit(&a, "a-edits-one").unwrap();
    push(&a).unwrap();
    // b deletes two.md.
    fs::remove_file(b.join("two.md")).unwrap();
    commit(&b, "b-deletes-two").unwrap();

    let out = sync(&b).unwrap();
    assert!(
        out.conflicts.is_empty(),
        "a local-only deletion is clean: {:?}",
        out.conflicts
    );
    // The local deletion is preserved (not resurrected by the remote's stale copy)…
    assert!(!b.join("two.md").exists(), "local deletion preserved");
    // …and the remote's disjoint edit is adopted.
    assert!(fs::read_to_string(b.join("one.md"))
        .unwrap()
        .contains("A's disjoint edit"));
    assert_eq!(commit(&b, "should-be-clean").unwrap(), None);

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn sync_converges_cleanly_when_both_sides_delete_the_same_chapter() {
    // both-sides-delete: a and b independently remove two.md. Since both agree the
    // chapter is gone, the three-way needs nothing (o == t == None) — a clean
    // convergence with the file absent and no conflict.
    let (remote, a, b) = two_shared_chapters("sync-del-both");

    fs::remove_file(a.join("two.md")).unwrap();
    commit(&a, "a-deletes-two").unwrap();
    push(&a).unwrap();
    fs::remove_file(b.join("two.md")).unwrap();
    commit(&b, "b-deletes-two").unwrap();
    // A disjoint edit on b so the branch diverges rather than fast-forwarding.
    fs::write(b.join("one.md"), "# One\n\nB's disjoint edit.\n").unwrap();
    commit(&b, "b-edits-one").unwrap();

    let out = sync(&b).unwrap();
    assert!(
        out.conflicts.is_empty(),
        "agreeing deletions don't conflict: {:?}",
        out.conflicts
    );
    assert!(
        !b.join("two.md").exists(),
        "the file is gone after both delete it"
    );
    assert_eq!(commit(&b, "should-be-clean").unwrap(), None);

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn sync_conflicts_when_both_sides_add_the_same_new_chapter_differently() {
    // add/add: base has no three.md; a and b each add three.md with different content.
    // With no shared ancestor the chapter reads as added on both sides, so it's a
    // conflict (never a silent pick), with each side's text carried and an empty base.
    let (remote, a, b) = two_clones("sync-add-add");

    fs::write(a.join("three.md"), "# Three\n\nA's brand-new chapter.\n").unwrap();
    commit(&a, "a-adds-three").unwrap();
    push(&a).unwrap();
    let b_content = "# Three\n\nB's brand-new chapter.\n";
    fs::write(b.join("three.md"), b_content).unwrap();
    commit(&b, "b-adds-three").unwrap();

    let out = sync(&b).unwrap();
    assert_eq!(
        out.conflicts.len(),
        1,
        "add/add of the same path is a conflict"
    );
    assert!(!out.reload, "nothing written on a conflict");
    let c = &out.conflicts[0];
    assert_eq!(c.path, "three.md");
    assert!(
        c.local.contains("B's brand-new"),
        "local add kept: {:?}",
        c.local
    );
    assert!(
        c.remote.contains("A's brand-new"),
        "remote add kept: {:?}",
        c.remote
    );
    assert_eq!(c.base, "", "no shared ancestor for an add/add");
    // Byte-unchanged, no merge commit.
    assert_eq!(fs::read_to_string(b.join("three.md")).unwrap(), b_content);
    assert_eq!(commit(&b, "should-be-clean").unwrap(), None);

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}
