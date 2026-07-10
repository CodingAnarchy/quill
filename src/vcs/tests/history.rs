//! Local-history tests (#659): init and its idempotence, commit's stage/skip-clean
//! behavior, the log and its branch/merge graph (#964), reading a file out of a past
//! commit (#587), commit-to-commit diff (#587), and milestones with whole-project
//! restore/diff (#833/#966).

use std::fs;

use super::temp_dir;
use crate::vcs::*;

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
fn log_carries_parents_and_the_changed_paths_per_commit() {
    // #964: each commit reports its parent ids (topology) and the paths it changed
    // versus its first parent (the chapters touched), so the history view can draw a
    // graph and list what each commit changed.
    let dir = temp_dir("log-topology");
    init(&dir).unwrap();
    fs::write(dir.join("a.md"), "one\n").unwrap();
    fs::write(dir.join("b.md"), "one\n").unwrap();
    commit(&dir, "root").unwrap();
    fs::write(dir.join("a.md"), "two\n").unwrap(); // touch only a.md
    commit(&dir, "edit-a").unwrap();

    let entries = log(&dir, 10).unwrap(); // [edit-a, root]
    let edit = &entries[0];
    let root = &entries[1];
    // The root has no parent; the second commit's parent is the root.
    assert!(root.parents.is_empty(), "the root commit has no parent");
    assert_eq!(edit.parents, vec![root.rev.clone()], "parent is the root");
    // The second commit changed only a.md; the root reports its whole tree.
    assert_eq!(edit.changed, vec!["a.md".to_string()], "only a.md changed");
    assert!(
        root.changed.contains(&"a.md".to_string()) && root.changed.contains(&"b.md".to_string()),
        "the root reports its whole tree: {:?}",
        root.changed
    );
    // Every commit carries a non-empty graph column (its node), padded uniformly.
    assert!(
        entries.iter().all(|c| c.graph.contains('●')),
        "each row is a node"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn log_graph_reflects_a_branch_and_merge_topology() {
    // #964: build a real side branch off the mainline and merge it back, then check
    // the log's graph shows the branch topology — the merge commit lists two parents,
    // and while the branch is unresolved the graph draws more than one lane. Hermetic:
    // the branch + merge are built directly with git2 over a scratch repo.
    let dir = temp_dir("log-merge-graph");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "base\n").unwrap();
    commit(&dir, "base").unwrap();

    let repo = git2::Repository::open(&dir).unwrap();
    let sig = git2::Signature::now("T", "t@t").unwrap();
    let base = repo.head().unwrap().peel_to_commit().unwrap();

    // A side branch off base: add two.md, one commit.
    let branch = repo.branch("side", &base, false).unwrap();
    repo.set_head(branch.get().name().unwrap()).unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    fs::write(dir.join("two.md"), "side\n").unwrap();
    let side = commit_on_head(&repo, &sig, "side-work");

    // Back on the mainline, move it forward: edit one.md.
    let main = repo.find_branch("master", git2::BranchType::Local).unwrap();
    repo.set_head(main.get().name().unwrap()).unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    fs::write(dir.join("one.md"), "mainline edit\n").unwrap();
    let mainline = commit_on_head(&repo, &sig, "mainline-work");

    // Merge the side branch into the mainline — a two-parent commit.
    let main_c = repo.find_commit(mainline).unwrap();
    let side_c = repo.find_commit(side).unwrap();
    let tree = main_c.tree().unwrap(); // no textual overlap; keep the mainline tree
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "merge side",
        &tree,
        &[&main_c, &side_c],
    )
    .unwrap();

    let entries = log(&dir, 20).unwrap();
    let merge = entries
        .iter()
        .find(|c| c.summary == "merge side")
        .expect("the merge commit is in the log");
    // The merge names both histories as parents — the topological fact the graph draws.
    assert_eq!(merge.parents.len(), 2, "the merge has two parents");
    assert!(merge.parents.contains(&mainline.to_string()));
    assert!(merge.parents.contains(&side.to_string()));
    // Somewhere in the log the graph fans out to more than one lane (the branch),
    // and every row is padded to the same width so the columns align.
    let widths: Vec<usize> = entries.iter().map(|c| c.graph.chars().count()).collect();
    assert!(
        widths.iter().all(|w| *w == widths[0]),
        "graph columns are aligned to one width: {widths:?}"
    );
    let max_lanes = entries
        .iter()
        .map(|c| c.graph.matches('│').count() + c.graph.matches('●').count())
        .max()
        .unwrap_or(0);
    assert!(
        max_lanes >= 2,
        "the branch draws a second lane in the graph: {:?}",
        entries.iter().map(|c| &c.graph).collect::<Vec<_>>()
    );
    fs::remove_dir_all(&dir).ok();
}

/// Commit the current index/worktree on HEAD with `message`, returning the new oid.
/// A small test helper for building branch topology directly with git2.
fn commit_on_head(repo: &git2::Repository, sig: &git2::Signature, message: &str) -> git2::Oid {
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), sig, sig, message, &tree, &[&parent])
        .unwrap()
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
