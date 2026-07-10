//! Branch-as-draft tests (#965): creating and listing a draft, switching (with the
//! working copy following and the dirty-tree guard), a clean draft merge that folds
//! chapters in, a conflicting merge that surfaces the conflict without clobbering and
//! finalizes on a hand resolution, and the log showing commits from every draft.

use std::collections::BTreeMap;
use std::fs;

use super::temp_dir;
use crate::vcs::*;

#[test]
fn a_draft_is_created_listed_and_becomes_current_on_switch() {
    // Create a draft off the current state, list it beside the mainline, then switch
    // to it — the current branch follows.
    let dir = temp_dir("draft-create");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nMainline.\n").unwrap();
    commit(&dir, "first").unwrap();
    let main = current_branch(&dir).unwrap();

    create_branch(&dir, "alt-ending").unwrap();
    // Creating doesn't switch: still on the mainline.
    assert_eq!(current_branch(&dir).unwrap(), main);

    let drafts = list_branches(&dir).unwrap();
    assert!(drafts.contains(&main), "the mainline is listed: {drafts:?}");
    assert!(
        drafts.contains(&"alt-ending".to_string()),
        "the new draft is listed: {drafts:?}"
    );

    // A duplicate name is refused rather than silently moving the existing draft.
    assert!(create_branch(&dir, "alt-ending").is_err());

    checkout_branch(&dir, "alt-ending").unwrap();
    assert_eq!(current_branch(&dir).unwrap(), "alt-ending");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn switching_drafts_updates_the_working_copy() {
    // Each draft has its own version of a chapter; switching rewrites the working
    // copy to the target draft's version.
    let dir = temp_dir("draft-switch");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nMainline text.\n").unwrap();
    commit(&dir, "first").unwrap();
    let main = current_branch(&dir).unwrap();

    create_branch(&dir, "alt").unwrap();
    checkout_branch(&dir, "alt").unwrap();
    fs::write(dir.join("one.md"), "# One\n\nAlt-draft text.\n").unwrap();
    commit(&dir, "alt-edit").unwrap();

    // Back to the mainline: the working copy is the mainline version again.
    checkout_branch(&dir, &main).unwrap();
    assert!(fs::read_to_string(dir.join("one.md"))
        .unwrap()
        .contains("Mainline text"));

    // Forward to the alt draft: the alt version is on disk.
    checkout_branch(&dir, "alt").unwrap();
    assert!(fs::read_to_string(dir.join("one.md"))
        .unwrap()
        .contains("Alt-draft text"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn switching_drafts_refuses_a_dirty_tree() {
    // An uncommitted edit would be clobbered by the checkout — refuse, preserving it.
    let dir = temp_dir("draft-dirty");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nCommitted.\n").unwrap();
    commit(&dir, "first").unwrap();
    create_branch(&dir, "alt").unwrap();

    fs::write(dir.join("one.md"), "# One\n\nUnsaved edit.\n").unwrap();
    assert!(
        checkout_branch(&dir, "alt").is_err(),
        "a dirty tree is refused"
    );
    // The edit is untouched.
    assert!(fs::read_to_string(dir.join("one.md"))
        .unwrap()
        .contains("Unsaved edit"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn merging_a_clean_draft_folds_its_chapters_in() {
    // A draft that added a new chapter (and left the shared one alone) merges cleanly
    // back into the mainline: its chapter lands and a two-parent commit is recorded.
    let dir = temp_dir("draft-merge-clean");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nShared.\n").unwrap();
    commit(&dir, "first").unwrap();
    let main = current_branch(&dir).unwrap();

    create_branch(&dir, "side").unwrap();
    checkout_branch(&dir, "side").unwrap();
    fs::write(dir.join("two.md"), "# Two\n\nAdded on the side draft.\n").unwrap();
    commit(&dir, "side-adds-two").unwrap();

    // Merge the side draft back into the mainline (which must be checked out first).
    checkout_branch(&dir, &main).unwrap();
    let outcome = merge_branch(&dir, &main, "side").unwrap();
    match outcome {
        MergeOutcome::Clean { merged } => {
            assert!(
                merged.iter().any(|p| p == "two.md"),
                "two.md folded in: {merged:?}"
            );
        }
        MergeOutcome::Conflicts { .. } => panic!("disjoint drafts don't conflict"),
    }
    // The side draft's chapter is now on the mainline's working copy.
    assert!(dir.join("two.md").exists());
    // Both histories are reachable through the two-parent merge commit.
    let summaries: Vec<String> = log(&dir, 20)
        .unwrap()
        .into_iter()
        .map(|c| c.summary)
        .collect();
    assert!(
        summaries.iter().any(|s| s == "side-adds-two"),
        "{summaries:?}"
    );
    assert!(
        summaries.iter().any(|s| s.contains("Merge draft")),
        "{summaries:?}"
    );
    // No half-merge left behind.
    assert_eq!(commit(&dir, "should-be-clean").unwrap(), None);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn merging_a_conflicting_draft_surfaces_the_conflict_without_clobbering() {
    // Both drafts changed the same chapter differently: the merge surfaces a
    // per-chapter conflict, preserves both versions, and leaves the repo untouched —
    // then finalize lands the author's resolution as a merge commit.
    let dir = temp_dir("draft-merge-conflict");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nShared draft.\n").unwrap();
    commit(&dir, "first").unwrap();
    let main = current_branch(&dir).unwrap();

    create_branch(&dir, "alt").unwrap();
    checkout_branch(&dir, "alt").unwrap();
    fs::write(dir.join("one.md"), "# One\n\nAlt rewrite.\n").unwrap();
    commit(&dir, "alt-edit").unwrap();

    checkout_branch(&dir, &main).unwrap();
    let mainline = "# One\n\nMainline rewrite.\n";
    fs::write(dir.join("one.md"), mainline).unwrap();
    commit(&dir, "main-edit").unwrap();

    let conflicts = match merge_branch(&dir, &main, "alt").unwrap() {
        MergeOutcome::Conflicts { conflicts } => conflicts,
        MergeOutcome::Clean { .. } => panic!("the same chapter was edited on both drafts"),
    };
    assert_eq!(conflicts.len(), 1, "one chapter conflicts");
    let c = &conflicts[0];
    assert_eq!(c.path, "one.md");
    assert!(
        c.local.contains("Mainline rewrite"),
        "target kept: {:?}",
        c.local
    );
    assert!(
        c.remote.contains("Alt rewrite"),
        "source kept: {:?}",
        c.remote
    );
    assert!(
        c.base.contains("Shared draft"),
        "ancestor carried: {:?}",
        c.base
    );

    // Nothing was written or committed — the mainline is exactly as it was.
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), mainline);
    assert_eq!(commit(&dir, "should-be-clean").unwrap(), None);

    // Resolve by hand (blend both) and finalize: the merge lands as a two-parent commit.
    let mut resolutions = BTreeMap::new();
    resolutions.insert(
        "one.md".to_string(),
        "# One\n\nBlended: mainline + alt.\n".to_string(),
    );
    let n = finalize_branch_merge(&dir, &main, "alt", &resolutions).unwrap();
    assert_eq!(n, 1);
    assert!(fs::read_to_string(dir.join("one.md"))
        .unwrap()
        .contains("Blended"));
    let summaries: Vec<String> = log(&dir, 20)
        .unwrap()
        .into_iter()
        .map(|c| c.summary)
        .collect();
    assert!(
        summaries.iter().any(|s| s.contains("Merge draft")),
        "{summaries:?}"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_log_shows_commits_from_every_draft() {
    // A commit made only on a side draft still appears in the log (#964 graph reflects
    // branches, #965), even while the mainline is checked out.
    let dir = temp_dir("draft-log");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nMainline.\n").unwrap();
    commit(&dir, "first").unwrap();
    let main = current_branch(&dir).unwrap();

    create_branch(&dir, "side").unwrap();
    checkout_branch(&dir, "side").unwrap();
    fs::write(dir.join("one.md"), "# One\n\nSide draft only.\n").unwrap();
    commit(&dir, "side-only-commit").unwrap();

    // Back on the mainline, the side draft's commit is still in the log.
    checkout_branch(&dir, &main).unwrap();
    let summaries: Vec<String> = log(&dir, 20)
        .unwrap()
        .into_iter()
        .map(|c| c.summary)
        .collect();
    assert!(
        summaries.iter().any(|s| s == "side-only-commit"),
        "the side draft's commit shows in the graph: {summaries:?}"
    );

    fs::remove_dir_all(&dir).ok();
}
