//! Remote push/pull tests (#134/#703): configuring `origin`, pushing and
//! fast-forwarding, a conflict-free divergence that merges, and a same-line
//! divergence that reports the conflict without losing work. A bare repo stands in
//! for the hosted remote, so nothing reaches the network.

use std::fs;

use super::temp_dir;
use crate::vcs::*;

#[test]
fn set_and_read_remote() {
    let dir = temp_dir("remote-set");
    init(&dir).unwrap();
    assert!(remote_url(&dir).is_none(), "no remote on a fresh repo");
    set_remote(&dir, "https://example.com/a.git").unwrap();
    assert_eq!(
        remote_url(&dir).as_deref(),
        Some("https://example.com/a.git")
    );
    // Reconfiguring just replaces the URL.
    set_remote(&dir, "https://example.com/b.git").unwrap();
    assert_eq!(
        remote_url(&dir).as_deref(),
        Some("https://example.com/b.git")
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn push_and_pull_through_a_local_remote() {
    // A bare repo stands in for the hosted remote — this exercises the full
    // push/pull/divergence plumbing with no network or auth.
    let remote = temp_dir("remote-bare");
    git2::Repository::init_bare(&remote).unwrap();
    let url = remote.to_str().unwrap();

    let a = temp_dir("remote-a");
    init(&a).unwrap();
    fs::write(a.join("one.md"), "# One\n\nDraft.\n").unwrap();
    commit(&a, "first").unwrap();
    set_remote(&a, url).unwrap();
    assert!(
        !push(&a).unwrap().is_empty(),
        "first push reports the branch"
    );

    // b is a second working copy (clone) — it has "first".
    let b = temp_dir("remote-b");
    fs::remove_dir_all(&b).ok();
    git2::Repository::clone(url, &b).unwrap();
    assert!(b.join("one.md").exists());

    // a commits again and pushes; b fast-forwards on pull.
    fs::write(a.join("one.md"), "# One\n\nRevised draft.\n").unwrap();
    commit(&a, "second").unwrap();
    push(&a).unwrap();
    let msg = pull(&b).unwrap();
    assert!(msg.contains("Pulled"), "{msg}");
    assert!(fs::read_to_string(b.join("one.md"))
        .unwrap()
        .contains("Revised"));
    // Pulling again with nothing new is a no-op, not an error.
    assert!(pull(&b).unwrap().contains("up to date"));

    // Divergence: b moves ahead and pushes; a (behind, with its own new
    // commit) is refused rather than overwriting b's work.
    fs::write(b.join("one.md"), "# One\n\nB's edit.\n").unwrap();
    commit(&b, "b-change").unwrap();
    push(&b).unwrap();
    fs::write(a.join("one.md"), "# One\n\nA's edit.\n").unwrap();
    commit(&a, "a-change").unwrap();
    let err = push(&a).unwrap_err();
    assert!(err.to_lowercase().contains("pull first"), "{err}");

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn pull_refuses_a_dirty_tree_and_preserves_edits() {
    // #639: a save-but-don't-commit-then-pull must not silently discard the
    // uncommitted edit. Set up a real fast-forward, then dirty the tree and
    // confirm the pull is refused with the edit left intact on disk.
    let remote = temp_dir("dirty-bare");
    git2::Repository::init_bare(&remote).unwrap();
    let url = remote.to_str().unwrap();

    let a = temp_dir("dirty-a");
    init(&a).unwrap();
    fs::write(a.join("one.md"), "# One\n\nDraft.\n").unwrap();
    commit(&a, "first").unwrap();
    set_remote(&a, url).unwrap();
    push(&a).unwrap();

    // b clones, then a pushes a second commit so b has something to fast-forward.
    let b = temp_dir("dirty-b");
    fs::remove_dir_all(&b).ok();
    git2::Repository::clone(url, &b).unwrap();
    fs::write(a.join("one.md"), "# One\n\nRevised draft.\n").unwrap();
    commit(&a, "second").unwrap();
    push(&a).unwrap();

    // The author saved an edit in b but hasn't committed. A pull would
    // fast-forward and force-checkout, clobbering this — so it must refuse.
    let uncommitted = "# One\n\nB's unsaved-to-git work.\n";
    fs::write(b.join("one.md"), uncommitted).unwrap();
    let err = pull(&b).unwrap_err();
    assert!(
        err.to_lowercase().contains("uncommitted"),
        "refusal should name the uncommitted changes: {err}"
    );
    // The author's words are still on disk, untouched.
    assert_eq!(fs::read_to_string(b.join("one.md")).unwrap(), uncommitted);

    // After committing, the dirty-tree refusal is gone. b and the remote have
    // diverged on the same line now, so the pull reports a merge conflict (#703)
    // rather than the uncommitted-changes refusal — the point here is only that the
    // clean tree gets past the dirty guard.
    commit(&b, "keep-my-edit").unwrap();
    let outcome = pull(&b);
    assert!(
        outcome.is_err()
            && !outcome
                .as_ref()
                .unwrap_err()
                .to_lowercase()
                .contains("uncommitted"),
        "a clean tree gets past the dirty guard: {outcome:?}"
    );

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn pull_fast_forwards_a_clean_tree() {
    // The unchanged happy path: a clean working tree fast-forwards and the
    // remote's changes land on disk (the dirty guard must not block this).
    let remote = temp_dir("clean-bare");
    git2::Repository::init_bare(&remote).unwrap();
    let url = remote.to_str().unwrap();

    let a = temp_dir("clean-a");
    init(&a).unwrap();
    fs::write(a.join("one.md"), "# One\n\nDraft.\n").unwrap();
    commit(&a, "first").unwrap();
    set_remote(&a, url).unwrap();
    push(&a).unwrap();

    let b = temp_dir("clean-b");
    fs::remove_dir_all(&b).ok();
    git2::Repository::clone(url, &b).unwrap();

    fs::write(a.join("one.md"), "# One\n\nRevised draft.\n").unwrap();
    commit(&a, "second").unwrap();
    push(&a).unwrap();

    let msg = pull(&b).unwrap();
    assert!(msg.contains("Pulled"), "{msg}");
    assert!(fs::read_to_string(b.join("one.md"))
        .unwrap()
        .contains("Revised"));

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn pull_merges_a_conflict_free_divergence() {
    // #703: two machines commit different files, then one pushes. The other pulls a
    // diverged branch that touches no shared line — the pull merges instead of
    // dead-ending, and both authors' commits survive.
    let remote = temp_dir("merge-bare");
    git2::Repository::init_bare(&remote).unwrap();
    let url = remote.to_str().unwrap();

    let a = temp_dir("merge-a");
    init(&a).unwrap();
    fs::write(a.join("one.md"), "# One\n\nShared.\n").unwrap();
    commit(&a, "first").unwrap();
    set_remote(&a, url).unwrap();
    push(&a).unwrap();

    let b = temp_dir("merge-b");
    fs::remove_dir_all(&b).ok();
    git2::Repository::clone(url, &b).unwrap();

    // a and b now diverge on *different* files, so there's no textual conflict.
    fs::write(a.join("two.md"), "# Two\n\nA's new chapter.\n").unwrap();
    commit(&a, "a-adds-two").unwrap();
    push(&a).unwrap();
    fs::write(b.join("three.md"), "# Three\n\nB's new chapter.\n").unwrap();
    commit(&b, "b-adds-three").unwrap();

    let msg = pull(&b).unwrap();
    assert!(msg.to_lowercase().contains("merged"), "{msg}");
    // Both sides' files landed on disk after the merge.
    assert!(b.join("two.md").exists(), "a's chapter merged into b");
    assert!(b.join("three.md").exists(), "b's own chapter is preserved");

    // Both commits are reachable, plus the merge commit itself.
    let summaries: Vec<String> = log(&b, 20)
        .unwrap()
        .into_iter()
        .map(|c| c.summary)
        .collect();
    assert!(summaries.iter().any(|s| s == "a-adds-two"), "{summaries:?}");
    assert!(
        summaries.iter().any(|s| s == "b-adds-three"),
        "{summaries:?}"
    );
    assert!(
        summaries.iter().any(|s| s.contains("Merge remote")),
        "a merge commit ties the two histories: {summaries:?}"
    );

    // The merge is now pushable (b is ahead of the remote, not behind).
    push(&b).unwrap();

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn pull_reports_conflicts_without_losing_work() {
    // #703: both machines edit the *same* line, then one pushes. The other's pull
    // can't merge automatically — it must name the conflicted file and leave the
    // author's own committed work exactly as it was, not a half-merged tree.
    let remote = temp_dir("conflict-bare");
    git2::Repository::init_bare(&remote).unwrap();
    let url = remote.to_str().unwrap();

    let a = temp_dir("conflict-a");
    init(&a).unwrap();
    fs::write(a.join("one.md"), "# One\n\nOriginal line.\n").unwrap();
    commit(&a, "first").unwrap();
    set_remote(&a, url).unwrap();
    push(&a).unwrap();

    let b = temp_dir("conflict-b");
    fs::remove_dir_all(&b).ok();
    git2::Repository::clone(url, &b).unwrap();

    // Same file, same line, edited differently on each side.
    fs::write(a.join("one.md"), "# One\n\nA's rewrite.\n").unwrap();
    commit(&a, "a-edit").unwrap();
    push(&a).unwrap();
    let b_content = "# One\n\nB's rewrite.\n";
    fs::write(b.join("one.md"), b_content).unwrap();
    commit(&b, "b-edit").unwrap();

    let err = pull(&b).unwrap_err();
    assert!(
        err.contains("one.md"),
        "the conflict names the file to reconcile: {err}"
    );
    // b's committed work is untouched — no half-merged tree, no conflict markers.
    assert_eq!(fs::read_to_string(b.join("one.md")).unwrap(), b_content);
    // The repo isn't left mid-merge: a fresh commit sees a clean tree.
    assert_eq!(commit(&b, "should-be-clean").unwrap(), None);

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
    fs::remove_dir_all(&remote).ok();
}

#[test]
fn push_without_a_remote_explains() {
    let dir = temp_dir("remote-none");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n").unwrap();
    commit(&dir, "first").unwrap();
    let err = push(&dir).unwrap_err();
    assert!(err.contains("No remote"), "{err}");
    fs::remove_dir_all(&dir).ok();
}
