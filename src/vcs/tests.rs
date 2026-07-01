//! Tests for the version-control seam (#659). They drive the re-exported public
//! surface (`super::*`) rather than either half directly, so a function moving
//! between [`super::local`] and [`super::remote`] never touches this file. All are
//! hermetic (#531): they build repositories under a scratch dir, or use a bare
//! repo as a stand-in remote, so nothing reaches the network.

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

    // After committing, the same pull fast-forwards exactly as before.
    commit(&b, "keep-my-edit").unwrap();
    // (b and the remote have diverged now, so it stops for merge — the point
    // is only that the dirty-tree refusal is gone once the tree is clean.)
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
fn push_without_a_remote_explains() {
    let dir = temp_dir("remote-none");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n").unwrap();
    commit(&dir, "first").unwrap();
    let err = push(&dir).unwrap_err();
    assert!(err.contains("No remote"), "{err}");
    fs::remove_dir_all(&dir).ok();
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
