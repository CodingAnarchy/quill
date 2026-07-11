//! Branch-as-draft tests (#965): creating and listing a draft, switching (with the
//! working copy following and the dirty-tree guard), a clean draft merge that folds
//! chapters in, a conflicting merge that surfaces the conflict without clobbering and
//! finalizes on a hand resolution, and the log showing commits from every draft. The
//! shared chapter-level three-way is exercised over the delete/add arms too:
//! delete-vs-edit conflicts, delete-vs-unchanged applies the deletion either
//! direction, both-delete converges, add/add conflicts, a nested-path reconcile
//! (the flat-TreeBuilder regression), plus the self-merge, dirty-tree, and
//! under-supplied-resolutions guards.

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

/// The commit summaries reachable from the current draft, for asserting a two-parent
/// merge landed (both drafts' history present) or that none did.
fn summaries(dir: &std::path::Path) -> Vec<String> {
    log(dir, 20)
        .unwrap()
        .into_iter()
        .map(|c| c.summary)
        .collect()
}

/// Seed a repo with `one.md` and `two.md` committed on the mainline, then branch an
/// `alt` draft off it (not switched). Both drafts share both chapters as their base.
/// Returns the scratch dir and the mainline branch name.
fn two_chapter_drafts(name: &str) -> (std::path::PathBuf, String) {
    let dir = temp_dir(name);
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nShared.\n").unwrap();
    fs::write(dir.join("two.md"), "# Two\n\nShared second chapter.\n").unwrap();
    commit(&dir, "first").unwrap();
    let main = current_branch(&dir).unwrap();
    create_branch(&dir, "alt").unwrap();
    (dir, main)
}

#[test]
fn merging_a_draft_that_deletes_a_chapter_the_target_edited_conflicts() {
    // delete-vs-edit on the draft-merge path: the source draft (alt) deletes two.md
    // while the target (mainline) edits it. Reconcile must surface a conflict rather
    // than drop either intent — the deleted side empty, the edited side preserved —
    // and leave the tree byte-unchanged with no merge commit.
    let (dir, main) = two_chapter_drafts("draft-del-vs-edit");

    checkout_branch(&dir, "alt").unwrap();
    fs::remove_file(dir.join("two.md")).unwrap();
    commit(&dir, "alt-deletes-two").unwrap();

    checkout_branch(&dir, &main).unwrap();
    let target = "# Two\n\nMainline revision.\n";
    fs::write(dir.join("two.md"), target).unwrap();
    commit(&dir, "main-edits-two").unwrap();

    let conflicts = match merge_branch(&dir, &main, "alt").unwrap() {
        MergeOutcome::Conflicts { conflicts } => conflicts,
        MergeOutcome::Clean { .. } => panic!("delete-vs-edit must conflict"),
    };
    assert_eq!(conflicts.len(), 1);
    let c = &conflicts[0];
    assert_eq!(c.path, "two.md");
    assert!(
        c.local.contains("Mainline revision"),
        "target edit kept: {:?}",
        c.local
    );
    assert_eq!(c.remote, "", "the deleted source side is empty");
    assert!(
        c.base.contains("Shared second chapter"),
        "ancestor carried: {:?}",
        c.base
    );

    // Nothing written or committed.
    assert_eq!(fs::read_to_string(dir.join("two.md")).unwrap(), target);
    assert_eq!(commit(&dir, "should-be-clean").unwrap(), None);
    assert!(
        !summaries(&dir).iter().any(|s| s.contains("Merge draft")),
        "no merge commit on a conflict"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn merging_a_draft_that_deletes_an_unchanged_chapter_applies_the_deletion() {
    // delete-vs-unchanged (source deletes, target unchanged): the alt draft deletes
    // two.md; the mainline never touched it (it edited one.md instead). The deletion
    // is a source-only change and is ADOPTED — two.md is gone from the merged tree,
    // not silently kept — and the merge is clean.
    let (dir, main) = two_chapter_drafts("draft-del-source");

    checkout_branch(&dir, "alt").unwrap();
    fs::remove_file(dir.join("two.md")).unwrap();
    commit(&dir, "alt-deletes-two").unwrap();

    checkout_branch(&dir, &main).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nMainline disjoint edit.\n").unwrap();
    commit(&dir, "main-edits-one").unwrap();

    match merge_branch(&dir, &main, "alt").unwrap() {
        MergeOutcome::Clean { .. } => {}
        MergeOutcome::Conflicts { .. } => panic!("an unchanged deletion is clean"),
    }
    assert!(
        !dir.join("two.md").exists(),
        "source deletion applied on disk"
    );
    assert!(fs::read_to_string(dir.join("one.md"))
        .unwrap()
        .contains("Mainline disjoint edit"));
    assert_eq!(commit(&dir, "should-be-clean").unwrap(), None);
    assert!(summaries(&dir).iter().any(|s| s.contains("Merge draft")));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn merging_a_draft_keeps_a_target_only_deletion() {
    // delete-vs-unchanged (target deletes, source unchanged): the mainline deletes
    // two.md; the alt draft never touched it (it edited one.md). The target's deletion
    // must be kept — the merged tree still has two.md gone, not resurrected by the
    // source's unchanged copy — while the source's disjoint edit is adopted.
    let (dir, main) = two_chapter_drafts("draft-del-target");

    checkout_branch(&dir, "alt").unwrap();
    fs::write(dir.join("one.md"), "# One\n\nAlt disjoint edit.\n").unwrap();
    commit(&dir, "alt-edits-one").unwrap();

    checkout_branch(&dir, &main).unwrap();
    fs::remove_file(dir.join("two.md")).unwrap();
    commit(&dir, "main-deletes-two").unwrap();

    match merge_branch(&dir, &main, "alt").unwrap() {
        MergeOutcome::Clean { .. } => {}
        MergeOutcome::Conflicts { .. } => panic!("a target-only deletion is clean"),
    }
    assert!(
        !dir.join("two.md").exists(),
        "target deletion preserved, not resurrected"
    );
    assert!(fs::read_to_string(dir.join("one.md"))
        .unwrap()
        .contains("Alt disjoint edit"));
    assert_eq!(commit(&dir, "should-be-clean").unwrap(), None);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn merging_drafts_that_both_delete_the_same_chapter_converges_cleanly() {
    // both-sides-delete on the draft-merge path: both drafts remove two.md. They agree
    // (o == t == None), so the merge is clean with the file gone and no conflict.
    let (dir, main) = two_chapter_drafts("draft-del-both");

    checkout_branch(&dir, "alt").unwrap();
    fs::remove_file(dir.join("two.md")).unwrap();
    commit(&dir, "alt-deletes-two").unwrap();
    // A disjoint edit so the drafts diverge (a real three-way, not an ancestor merge).
    fs::write(dir.join("one.md"), "# One\n\nAlt disjoint edit.\n").unwrap();
    commit(&dir, "alt-edits-one").unwrap();

    checkout_branch(&dir, &main).unwrap();
    fs::remove_file(dir.join("two.md")).unwrap();
    commit(&dir, "main-deletes-two").unwrap();

    match merge_branch(&dir, &main, "alt").unwrap() {
        MergeOutcome::Clean { .. } => {}
        MergeOutcome::Conflicts { .. } => panic!("agreeing deletions don't conflict"),
    }
    assert!(
        !dir.join("two.md").exists(),
        "the file is gone after both delete it"
    );
    assert!(fs::read_to_string(dir.join("one.md"))
        .unwrap()
        .contains("Alt disjoint edit"));
    assert_eq!(commit(&dir, "should-be-clean").unwrap(), None);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn merging_drafts_that_both_add_the_same_new_chapter_conflicts() {
    // add/add on the draft-merge path: neither the base nor the shared history has
    // three.md; both drafts add it with different content. With no ancestor it reads
    // as added on both sides — a conflict, never a silent pick — with an empty base.
    let (dir, main) = two_chapter_drafts("draft-add-add");

    checkout_branch(&dir, "alt").unwrap();
    fs::write(dir.join("three.md"), "# Three\n\nAlt's new chapter.\n").unwrap();
    commit(&dir, "alt-adds-three").unwrap();

    checkout_branch(&dir, &main).unwrap();
    let target = "# Three\n\nMainline's new chapter.\n";
    fs::write(dir.join("three.md"), target).unwrap();
    commit(&dir, "main-adds-three").unwrap();

    let conflicts = match merge_branch(&dir, &main, "alt").unwrap() {
        MergeOutcome::Conflicts { conflicts } => conflicts,
        MergeOutcome::Clean { .. } => panic!("add/add of the same path must conflict"),
    };
    assert_eq!(conflicts.len(), 1);
    let c = &conflicts[0];
    assert_eq!(c.path, "three.md");
    assert!(
        c.local.contains("Mainline's new"),
        "target add kept: {:?}",
        c.local
    );
    assert!(
        c.remote.contains("Alt's new"),
        "source add kept: {:?}",
        c.remote
    );
    assert_eq!(c.base, "", "no shared ancestor for an add/add");
    // Byte-unchanged, no merge commit.
    assert_eq!(fs::read_to_string(dir.join("three.md")).unwrap(), target);
    assert_eq!(commit(&dir, "should-be-clean").unwrap(), None);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn merging_a_draft_reconciles_a_nested_chapter_across_a_divergence() {
    // Draft-merge counterpart to sync_merges_a_nested_chapter_across_a_divergence: the
    // reconcile helpers are shared, so the flat-TreeBuilder regression must be guarded
    // here too. The source draft adds a new nested chapter AND edits a shared nested
    // one while the target makes a disjoint edit — the merge must land the nested
    // content, not abort on the "/" in the path.
    let dir = temp_dir("draft-nested");
    init(&dir).unwrap();
    fs::create_dir_all(dir.join("bible")).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nShared.\n").unwrap();
    fs::write(dir.join("bible/alice.md"), "# Alice\n\nOriginal note.\n").unwrap();
    commit(&dir, "first").unwrap();
    let main = current_branch(&dir).unwrap();
    create_branch(&dir, "alt").unwrap();

    checkout_branch(&dir, "alt").unwrap();
    fs::write(
        dir.join("bible/setting.md"),
        "# Setting\n\nThe fog-bound harbor.\n",
    )
    .unwrap();
    fs::write(
        dir.join("bible/alice.md"),
        "# Alice\n\nRevised on the alt draft.\n",
    )
    .unwrap();
    commit(&dir, "alt-nested-edits").unwrap();

    checkout_branch(&dir, &main).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nMainline disjoint edit.\n").unwrap();
    commit(&dir, "main-edits-one").unwrap();

    // Before the shared fix this failed with "invalid name for a tree entry".
    match merge_branch(&dir, &main, "alt").unwrap() {
        MergeOutcome::Clean { merged } => {
            assert!(
                merged.iter().any(|p| p == "bible/setting.md"),
                "new nested folded: {merged:?}"
            );
            assert!(
                merged.iter().any(|p| p == "bible/alice.md"),
                "edited nested folded: {merged:?}"
            );
        }
        MergeOutcome::Conflicts { .. } => panic!("disjoint nested/top-level edits don't conflict"),
    }
    // The added nested chapter landed…
    assert_eq!(
        fs::read_to_string(dir.join("bible/setting.md")).unwrap(),
        "# Setting\n\nThe fog-bound harbor.\n"
    );
    // …the edited nested chapter took the source's revision…
    assert!(fs::read_to_string(dir.join("bible/alice.md"))
        .unwrap()
        .contains("Revised on the alt draft"));
    // …and the target's disjoint edit is preserved.
    assert!(fs::read_to_string(dir.join("one.md"))
        .unwrap()
        .contains("Mainline disjoint edit"));
    assert_eq!(commit(&dir, "should-be-clean").unwrap(), None);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn merging_a_draft_into_itself_is_refused() {
    // Guard: merging a draft into itself is a no-op with no base to reconcile — it's
    // refused with a clear message rather than recording an empty self-merge.
    let dir = temp_dir("draft-self-merge");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nMainline.\n").unwrap();
    commit(&dir, "first").unwrap();
    let main = current_branch(&dir).unwrap();

    let err = merge_branch(&dir, &main, &main)
        .err()
        .expect("self-merge is refused");
    assert!(err.contains("itself"), "self-merge refused: {err}");

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn merging_a_draft_refuses_a_dirty_tree() {
    // Guard (#639): an uncommitted edit would be clobbered by the merge checkout —
    // merge_branch refuses up front and leaves the edit on disk.
    let dir = temp_dir("draft-merge-dirty");
    init(&dir).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nShared.\n").unwrap();
    commit(&dir, "first").unwrap();
    let main = current_branch(&dir).unwrap();

    create_branch(&dir, "side").unwrap();
    checkout_branch(&dir, "side").unwrap();
    fs::write(dir.join("two.md"), "# Two\n\nAdded on the side.\n").unwrap();
    commit(&dir, "side-adds-two").unwrap();
    checkout_branch(&dir, &main).unwrap();

    let dirty = "# One\n\nUncommitted edit.\n";
    fs::write(dir.join("one.md"), dirty).unwrap();
    let err = merge_branch(&dir, &main, "side")
        .err()
        .expect("a dirty tree is refused");
    assert!(err.to_lowercase().contains("uncommitted"), "{err}");
    // The edit is untouched and no merge landed.
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), dirty);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn finalize_branch_merge_refuses_under_supplied_resolutions() {
    // Guard: two chapters conflict but the author supplies a resolution for only one.
    // finalize_branch_merge must refuse — a leftover conflict is a real gap — rather
    // than land a merge that silently keeps the target's version of the unresolved one.
    let (dir, main) = two_chapter_drafts("draft-underresolved");

    checkout_branch(&dir, "alt").unwrap();
    fs::write(dir.join("one.md"), "# One\n\nAlt one.\n").unwrap();
    fs::write(dir.join("two.md"), "# Two\n\nAlt two.\n").unwrap();
    commit(&dir, "alt-edits-both").unwrap();

    checkout_branch(&dir, &main).unwrap();
    fs::write(dir.join("one.md"), "# One\n\nMainline one.\n").unwrap();
    fs::write(dir.join("two.md"), "# Two\n\nMainline two.\n").unwrap();
    commit(&dir, "main-edits-both").unwrap();

    let conflicts = match merge_branch(&dir, &main, "alt").unwrap() {
        MergeOutcome::Conflicts { conflicts } => conflicts,
        MergeOutcome::Clean { .. } => panic!("both chapters conflict"),
    };
    assert_eq!(conflicts.len(), 2, "both chapters conflict");

    // Supply only one resolution.
    let mut resolutions = BTreeMap::new();
    resolutions.insert("one.md".to_string(), "# One\n\nBlended one.\n".to_string());
    let err = finalize_branch_merge(&dir, &main, "alt", &resolutions).unwrap_err();
    assert!(
        err.contains("still need resolving"),
        "under-supplied refused: {err}"
    );

    // Nothing landed — the tree is still the target's, no merge commit.
    assert_eq!(commit(&dir, "should-be-clean").unwrap(), None);
    assert!(!summaries(&dir).iter().any(|s| s.contains("Merge draft")));

    fs::remove_dir_all(&dir).ok();
}
