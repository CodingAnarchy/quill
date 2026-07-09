//! Tests for the version-control seam (#659). They drive the re-exported public
//! surface (`super::*`) rather than either half directly, so a function moving
//! between [`super::local`] and [`super::remote`] never touches this file. All are
//! hermetic (#531): they build repositories under a scratch dir, or use a bare
//! repo as a stand-in remote, so nothing reaches the network.

use std::collections::BTreeMap;
use std::fs;

use super::*;

fn temp_dir(name: &str) -> std::path::PathBuf {
    crate::testutil::scratch_dir("vcs", name)
}

#[test]
fn init_creates_a_repo_and_gitignore() {
    let dir = temp_dir("init");
    assert!(!is_repo(&dir));
    init(&dir).unwrap();
    assert!(is_repo(&dir), "a .git directory should exist");
    let ignore = fs::read_to_string(dir.join(".gitignore")).unwrap();
    assert!(ignore.contains("/.quill/") && ignore.contains("/unused/"));
    assert!(
        ignore.contains("/bible/.state.toml"),
        "the AI scoping cache is ignored (#90)"
    );
    assert!(
        ignore.contains("/bible/findings.toml"),
        "the durable triage store is editor state, not manuscript (#591)"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn commit_captures_changes_and_reports_nothing_when_clean() {
    let dir = temp_dir("commit");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nThe night was dark.\n").unwrap();

    let id = commit(&dir, "first").unwrap();
    assert!(id.is_some(), "the first commit should be created");
    // Nothing changed since → clean, no commit.
    assert_eq!(commit(&dir, "again").unwrap(), None);
    // A change → a new commit.
    fs::write(dir.join("one.md"), "# One\n\nThe night was long.\n").unwrap();
    assert!(commit(&dir, "edit").unwrap().is_some());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn log_lists_commits_newest_first() {
    let dir = temp_dir("log");
    init(&dir).unwrap();
    fs::write(dir.join("a.md"), "1\n").unwrap();
    commit(&dir, "first").unwrap();
    fs::write(dir.join("a.md"), "2\n").unwrap();
    commit(&dir, "second").unwrap();

    let entries = log(&dir, 10).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].summary, "second"); // newest first
    assert_eq!(entries[1].summary, "first");
    assert_eq!(entries[0].when.len(), 10, "YYYY-MM-DD"); // "{}", entries[0].when
    assert_eq!(entries[0].id.len(), 7, "short id");
    assert_eq!(entries[0].rev.len(), 40, "full object id");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn show_file_at_reads_a_past_version() {
    let dir = temp_dir("show-at");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "The night was dark.\n").unwrap();
    commit(&dir, "first").unwrap();
    fs::write(dir.join("one.md"), "The night was long.\n").unwrap();
    commit(&dir, "second").unwrap();

    let entries = log(&dir, 10).unwrap(); // [second, first]
                                          // The older commit still holds the original line…
    let older = show_file_at(&dir, &entries[1].rev, "one.md").unwrap();
    assert_eq!(older.as_deref(), Some("The night was dark.\n"));
    // …and the newer one the edit. A short id resolves the same commit.
    let newer = show_file_at(&dir, &entries[0].id, "one.md").unwrap();
    assert_eq!(newer.as_deref(), Some("The night was long.\n"));
    // A file absent from that commit reads as None, not an error.
    assert_eq!(
        show_file_at(&dir, &entries[0].rev, "gone.md").unwrap(),
        None
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn diff_file_compares_two_commits() {
    let dir = temp_dir("diff-at");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "The night was dark.\n").unwrap();
    commit(&dir, "first").unwrap();
    fs::write(dir.join("one.md"), "The night was long.\n").unwrap();
    commit(&dir, "second").unwrap();

    let entries = log(&dir, 10).unwrap(); // [second, first]
    let patch = diff_file(&dir, &entries[1].rev, &entries[0].rev, "one.md").unwrap();
    assert!(patch.contains("-The night was dark."), "{patch}");
    assert!(patch.contains("+The night was long."), "{patch}");
    // Same commit on both sides → nothing changed.
    assert!(diff_file(&dir, &entries[0].rev, &entries[0].rev, "one.md")
        .unwrap()
        .is_empty());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn milestone_is_created_listed_and_restored_and_diffed() {
    // #833: create a named milestone, list it, whole-project restore to it, and
    // whole-tree diff against it. Hermetic over a scratch repo.
    let dir = temp_dir("milestone");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nFirst draft.\n").unwrap();
    fs::write(dir.join("two.md"), "# Two\n\nAlso here.\n").unwrap();
    commit(&dir, "first draft done").unwrap();

    // A milestone can be named and lists back with its annotation.
    create_tag(&dir, "first-draft", "First complete draft").unwrap();
    let tags = list_tags(&dir).unwrap();
    let tag = tags
        .iter()
        .find(|t| t.name == "first-draft")
        .expect("the milestone should list back");
    assert_eq!(tag.message, "First complete draft");
    assert_eq!(tag.rev.len(), 40, "the full commit id it points at");

    // Move the manuscript on: edit one chapter, delete another, add a third.
    fs::write(dir.join("one.md"), "# One\n\nHeavily revised.\n").unwrap();
    fs::remove_file(dir.join("two.md")).unwrap();
    fs::write(dir.join("three.md"), "# Three\n\nBrand new.\n").unwrap();
    commit(&dir, "revision pass").unwrap();

    // The whole-tree diff against the milestone spans every changed file, not just
    // one chapter.
    let patch = diff_tree_to_worktree(&dir, &tag.rev).unwrap();
    assert!(patch.contains("-First draft.") && patch.contains("+Heavily revised."));
    assert!(
        patch.contains("Brand new."),
        "the added chapter shows: {patch}"
    );
    assert!(
        patch.contains("two.md"),
        "the deleted chapter shows: {patch}"
    );

    // Whole-project restore to the milestone rewinds the entire working tree.
    restore_all(&dir, &tag.rev).unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "# One\n\nFirst draft.\n",
        "the edited chapter is rewound"
    );
    assert!(dir.join("two.md").exists(), "the deleted chapter is back");
    assert!(
        !dir.join("three.md").exists(),
        "the later-added chapter is removed"
    );
    // The working tree now matches the milestone. HEAD still points at the later
    // commit (restore rewinds the tree, not history — non-destructive), so the
    // reverted state can be committed on top, recording the revert as a new commit
    // rather than rewriting history.
    assert!(
        commit(&dir, "back to first draft").unwrap().is_some(),
        "the reverted tree differs from HEAD, so it commits"
    );
    // …and immediately after, there's nothing left to commit.
    assert_eq!(commit(&dir, "clean").unwrap(), None);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn restore_all_rewinds_the_whole_tree_to_a_past_commit() {
    // #833: restore_all works against any revision, not only a tag — a plain past
    // commit rewinds the whole tree the same way. Rows are resolved by summary, not
    // position, since same-second commits order nondeterministically.
    let dir = temp_dir("restore-all-commit");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "original\n").unwrap();
    commit(&dir, "base").unwrap();
    fs::write(dir.join("one.md"), "changed\n").unwrap();
    commit(&dir, "edit").unwrap();

    let base = log(&dir, 10)
        .unwrap()
        .into_iter()
        .find(|c| c.summary == "base")
        .expect("the base commit is in the log");
    restore_all(&dir, &base.rev).unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "original\n"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn create_tag_refuses_a_duplicate_name() {
    // #833: a milestone name is claimed once — re-using it errors rather than
    // silently moving the earlier milestone.
    let dir = temp_dir("tag-dup");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "x\n").unwrap();
    commit(&dir, "first").unwrap();
    create_tag(&dir, "draft", "").unwrap();
    assert!(
        create_tag(&dir, "draft", "").is_err(),
        "a second milestone by the same name is refused"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn set_tag_message_edits_a_note_in_place() {
    // #966: a milestone's note can be edited later without moving the milestone —
    // the tag keeps its name and the revision it marks, only the annotation changes.
    let dir = temp_dir("tag-note-edit");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "x\n").unwrap();
    commit(&dir, "first").unwrap();
    create_tag(&dir, "beta", "sent to beta readers").unwrap();

    let before = list_tags(&dir)
        .unwrap()
        .into_iter()
        .find(|t| t.name == "beta")
        .expect("the milestone should list back");
    assert_eq!(before.message, "sent to beta readers");

    set_tag_message(&dir, "beta", "beta round two").unwrap();
    let after = list_tags(&dir)
        .unwrap()
        .into_iter()
        .find(|t| t.name == "beta")
        .expect("the milestone should still list back");
    assert_eq!(after.message, "beta round two", "the note is updated");
    assert_eq!(
        after.rev, before.rev,
        "the milestone still marks the same commit"
    );

    // Editing a milestone that doesn't exist is an error, not a silent create.
    assert!(
        set_tag_message(&dir, "no-such-milestone", "x").is_err(),
        "editing a nonexistent milestone's note is refused"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn create_tag_needs_a_commit() {
    // #833: there must be something to tag — a milestone marks a committed state.
    let dir = temp_dir("tag-unborn");
    init(&dir).unwrap();
    assert!(
        create_tag(&dir, "draft", "").is_err(),
        "no commit yet → no milestone"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn log_is_empty_before_any_commit() {
    let dir = temp_dir("log-empty");
    init(&dir).unwrap();
    assert!(log(&dir, 10).unwrap().is_empty());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn commit_ignores_editor_state_and_unused() {
    let dir = temp_dir("commit-ignore");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n").unwrap();
    commit(&dir, "first").unwrap();
    // Editor state under ignored paths must not count as a change.
    fs::create_dir_all(dir.join(".quill")).unwrap();
    fs::write(dir.join(".quill/history.jsonl"), "{}\n").unwrap();
    fs::create_dir_all(dir.join("unused")).unwrap();
    fs::write(dir.join("unused/old.md"), "# Old\n").unwrap();
    assert_eq!(
        commit(&dir, "noise").unwrap(),
        None,
        "ignored paths aren't committed"
    );
    fs::remove_dir_all(&dir).ok();
}

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

// --- Two-way sync (#962) -----------------------------------------------------

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

// --- Resolving a two-way sync conflict (#963) --------------------------------

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
fn init_is_idempotent_and_preserves_an_existing_gitignore() {
    let dir = temp_dir("idempotent");
    fs::write(dir.join(".gitignore"), "# hand-tuned\n/custom/\n").unwrap();
    init(&dir).unwrap();
    init(&dir).unwrap(); // second time is harmless
    assert!(is_repo(&dir));
    // The author's existing .gitignore is left untouched.
    assert_eq!(
        fs::read_to_string(dir.join(".gitignore")).unwrap(),
        "# hand-tuned\n/custom/\n"
    );
    fs::remove_dir_all(&dir).ok();
}
