//! Two-way sync (#962) and its hand-resolved finalization (#963): fetch `origin`
//! and reconcile its chapter edits with the local working copy at chapter
//! granularity, so the same project can be written on two machines without a manual
//! merge. Chapters are atomic files, so reconciliation is a chapter-level three-way
//! (base / local / remote) over the shared [`merge`](super::merge) helpers, never a
//! per-line merge. A clean reconciliation lands a two-parent merge commit; a
//! conflicting one changes nothing and returns the per-chapter [`ChapterConflict`]s
//! for the resolution UI to resolve and [`finalize_sync`] to land.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::super::local::signature;
use super::merge::{adopt, blob_text, collect_blobs, index_from_tree, stage_blob};
use super::{classify, credentials, current_branch, DIRTY_PULL, NO_REMOTE, PHONE_DRAFT_PREFIX};
use crate::vcs::local::is_dirty;

/// One chapter changed on both machines since the shared ancestor — a per-chapter
/// conflict two-way [`sync`] detects but never resolves on its own (#962). Chapters
/// are atomic files, so a conflict is per-chapter, not per-line: the same chapter
/// was edited differently on each side. All three versions are carried so the
/// resolution UI (#963) can show base-vs-local-vs-remote and let the author choose,
/// and so sync itself never has to pick a side or clobber one.
#[derive(Debug, Clone)]
pub struct ChapterConflict {
    /// The chapter's path relative to the repo root (its file name).
    pub path: String,
    /// The version currently on this machine (left on disk, untouched by sync).
    pub local: String,
    /// The version fetched from the remote (preserved here, not written to disk).
    pub remote: String,
    /// The shared-ancestor version both sides diverged from (empty when the chapter
    /// was added independently on both sides). The resolution overlay (#963) shows
    /// only local ↔ remote today, so outside the tests (which pin the three-way
    /// detection) nothing reads it yet — kept for a base-aware view.
    #[cfg_attr(not(test), allow(dead_code))]
    pub base: String,
}

/// What a two-way [`sync`] did (#962). A sync fetches the remote and reconciles it
/// with local work at chapter granularity: chapters only one side touched merge
/// cleanly, chapters both sides changed differently become [conflicts](ChapterConflict).
#[derive(Debug)]
pub struct SyncOutcome {
    /// The author-facing summary of what happened.
    pub summary: String,
    /// Paths of chapters that merged cleanly from the remote onto this machine
    /// (remote-only edits adopted). Empty on a pure fast-forward, which merges the
    /// whole tree at once rather than chapter-by-chapter. The summary already folds
    /// the count in, so the list itself is read only by the tests (which pin which
    /// chapters merged) — annotated so the bin build stays warning-clean.
    #[cfg_attr(not(test), allow(dead_code))]
    pub merged: Vec<String>,
    /// Chapters changed on both sides — surfaced for resolution (#963), never
    /// silently clobbered. Empty on a clean sync.
    pub conflicts: Vec<ChapterConflict>,
    /// Whether the working tree on disk changed, so the editor should reload the
    /// active chapter (mirrors [`pull`](super::pull)'s reload signal).
    pub reload: bool,
}

/// Two-way sync with the remote (#962): fetch `origin`, then reconcile its chapter
/// edits with the local working copy so the same project can be written on two
/// machines (laptop → remote → desktop) without a manual merge.
///
/// Chapters are atomic files, so reconciliation is a **chapter-level three-way**
/// (base / local / remote), never a per-line merge:
///
/// * **Up to date** — nothing to do.
/// * **Fast-forward** — local has no commits the remote lacks; the branch and tree
///   advance to the fetched commit, exactly like [`pull`](super::pull).
/// * **Diverged** — both sides committed. For each chapter, sync compares its blob
///   in the merge base, local `HEAD`, and the fetched commit: a chapter only the
///   remote changed is adopted; a chapter only local changed is kept; a chapter
///   both changed *identically* needs nothing; a chapter both changed *differently*
///   is a [conflict](ChapterConflict). When there are no conflicts the merged tree
///   is written as a two-parent merge commit (both histories preserved) and checked
///   out. When any chapter conflicts, **nothing is committed or written** — the repo
///   is left exactly as it was and every conflict is returned with all three
///   versions, so neither side is ever clobbered and #963 can resolve them.
///
/// Like [`pull`](super::pull), sync refuses up front when the working tree is dirty
/// (#639), so an uncommitted edit can never be overwritten by the merge checkout.
pub fn sync(root: &Path) -> Result<SyncOutcome, String> {
    sync_fetched(root).map(|(outcome, _)| outcome)
}

/// [`sync`], also returning the remote commit it fetched, which the phone's
/// draft diversion (#1233) resets to after parking its own work on a draft.
pub(super) fn sync_fetched(root: &Path) -> Result<(SyncOutcome, git2::Oid), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;

    // Same guard as pull: the merge/fast-forward checkout force-writes the tree, so
    // refuse before touching the network if there are uncommitted edits to lose.
    if is_dirty(&repo)? {
        return Err(DIRTY_PULL.to_string());
    }

    let branch = current_branch(&repo)?;
    let mut remote = repo
        .find_remote("origin")
        .map_err(|_| NO_REMOTE.to_string())?;

    let mut cbs = git2::RemoteCallbacks::new();
    cbs.credentials(credentials);
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(cbs);
    remote
        .fetch(&[&branch], Some(&mut opts), None)
        .map_err(|e| classify(&e).message())?;

    let fetch_head = repo
        .find_reference("FETCH_HEAD")
        .map_err(|e| e.to_string())?;
    let fetched = repo
        .reference_to_annotated_commit(&fetch_head)
        .map_err(|e| e.to_string())?;
    let (analysis, _) = repo
        .merge_analysis(&[&fetched])
        .map_err(|e| e.to_string())?;

    let mut outcome = if analysis.is_up_to_date() {
        SyncOutcome {
            summary: "Already in sync with the remote.".to_string(),
            merged: Vec::new(),
            conflicts: Vec::new(),
            reload: false,
        }
    } else if analysis.is_fast_forward() {
        fast_forward(&repo, &branch, &fetched)?;
        SyncOutcome {
            summary: "Pulled the remote's changes — you were behind.".to_string(),
            merged: Vec::new(),
            conflicts: Vec::new(),
            reload: true,
        }
    } else {
        sync_diverged(&repo, &branch, &fetched)?
    };
    // After FETCH_HEAD has been read: this second fetch replaces it.
    let adopted = adopt_phone_drafts(&repo, &mut remote);
    if !adopted.is_empty() {
        let names: Vec<String> = adopted
            .iter()
            .map(|d| format!("\u{201c}{d}\u{201d}"))
            .collect();
        let _ = write!(
            outcome.summary,
            " The phone parked edits that conflicted on {} {} — merge from Drafts.",
            if adopted.len() == 1 {
                "draft"
            } else {
                "drafts"
            },
            names.join(", ")
        );
    }
    Ok((outcome, fetched.id()))
}

/// Bring the drafts the phone parked its conflicting work on (#1233) into this
/// copy's drafts list: fetch `origin`'s `phone/*` branches and create a local
/// draft for each one not already here, so the desktop can merge it with the
/// conflict resolver (#963). Best-effort, since a failure here mustn't fail
/// the sync that already landed. An existing local draft is never moved.
/// Returns the drafts it created.
fn adopt_phone_drafts(repo: &git2::Repository, remote: &mut git2::Remote) -> Vec<String> {
    let mut cbs = git2::RemoteCallbacks::new();
    cbs.credentials(credentials);
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(cbs);
    let spec =
        format!("+refs/heads/{PHONE_DRAFT_PREFIX}*:refs/remotes/origin/{PHONE_DRAFT_PREFIX}*");
    let mut adopted = Vec::new();
    if remote.fetch(&[&spec], Some(&mut opts), None).is_err() {
        return adopted;
    }
    let Ok(refs) = repo.references_glob(&format!("refs/remotes/origin/{PHONE_DRAFT_PREFIX}*"))
    else {
        return adopted;
    };
    for reference in refs.flatten() {
        let (Some(full), Ok(commit)) = (reference.name(), reference.peel_to_commit()) else {
            continue;
        };
        let name = full.trim_start_matches("refs/remotes/origin/");
        if repo.find_branch(name, git2::BranchType::Local).is_err()
            && repo.branch(name, &commit, false).is_ok()
        {
            adopted.push(name.to_string());
        }
    }
    adopted
}

/// Advance the branch ref and working tree to the fetched commit — the shared
/// fast-forward step of [`pull`](super::pull) and [`sync`]. The caller has confirmed
/// the tree is clean, so the force checkout is safe.
fn fast_forward(
    repo: &git2::Repository,
    branch: &str,
    fetched: &git2::AnnotatedCommit,
) -> Result<(), String> {
    let refname = format!("refs/heads/{branch}");
    match repo.find_reference(&refname) {
        Ok(mut r) => {
            r.set_target(fetched.id(), "sync: fast-forward")
                .map_err(|e| e.to_string())?;
        }
        Err(_) => {
            repo.reference(&refname, fetched.id(), true, "sync: initial")
                .map_err(|e| e.to_string())?;
        }
    }
    repo.set_head(&refname).map_err(|e| e.to_string())?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Reconcile a diverged branch chapter-by-chapter (#962). See [`sync`] for the
/// three-way model. On a clean reconciliation this records a two-parent merge commit
/// and checks it out; on any conflict it changes nothing and returns the conflicts.
fn sync_diverged(
    repo: &git2::Repository,
    branch: &str,
    fetched: &git2::AnnotatedCommit,
) -> Result<SyncOutcome, String> {
    let local = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| e.to_string())?;
    let theirs = repo.find_commit(fetched.id()).map_err(|e| e.to_string())?;

    // The merge base is the shared ancestor both sides diverged from — the "base"
    // leg of every chapter's three-way. Absent (unrelated histories) is treated as
    // an empty ancestor, so every touched chapter reads as added on both sides.
    let base_tree = match repo.merge_base(local.id(), theirs.id()) {
        Ok(base) => Some(
            repo.find_commit(base)
                .and_then(|c| c.tree())
                .map_err(|e| e.to_string())?,
        ),
        Err(_) => None,
    };
    let local_tree = local.tree().map_err(|e| e.to_string())?;
    let their_tree = theirs.tree().map_err(|e| e.to_string())?;

    // Blob id per path in each of the three trees. `None` means the path is absent
    // in that tree (added on one side, or deleted). Only blobs (chapter files) are
    // reconciled here; trees (subdirectories) are compared by their own entry id, so
    // a changed subtree still reads as a conflict/adoption rather than being missed.
    let mut base = BTreeMap::new();
    if let Some(t) = &base_tree {
        collect_blobs(t, &mut base);
    }
    let mut ours = BTreeMap::new();
    collect_blobs(&local_tree, &mut ours);
    let mut theirs_blobs = BTreeMap::new();
    collect_blobs(&their_tree, &mut theirs_blobs);

    let mut paths: Vec<String> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs_blobs.keys())
        .cloned()
        .collect();
    paths.sort();
    paths.dedup();

    let mut merged = Vec::new();
    let mut conflicts = Vec::new();
    // Start the merged tree from local; clean remote-only edits are layered on. An
    // index (not a flat TreeBuilder) so nested chapter paths merge correctly.
    let mut index = index_from_tree(&local_tree)?;

    for path in paths {
        let b = base.get(&path).copied();
        let o = ours.get(&path).copied();
        let t = theirs_blobs.get(&path).copied();

        if o == t {
            continue; // both sides agree (including both absent) — nothing to do
        }
        if o == b {
            // Only the remote changed this chapter — adopt its version.
            adopt(&mut index, &path, t)?;
            merged.push(path);
        } else if t == b {
            // Only local changed it — keep ours (already in the index base).
        } else {
            // Both changed it, differently — a per-chapter conflict (#962).
            conflicts.push(ChapterConflict {
                path: path.clone(),
                local: blob_text(repo, o),
                remote: blob_text(repo, t),
                base: blob_text(repo, b),
            });
        }
    }

    if !conflicts.is_empty() {
        // Never write a half-merged tree: leave the repo exactly as it was and hand
        // the conflicts back for resolution (#963). Both versions are preserved.
        let merged_note = if merged.is_empty() {
            String::new()
        } else {
            format!(
                " {} other chapter(s) are ready to merge once resolved.",
                merged.len()
            )
        };
        return Ok(SyncOutcome {
            summary: format!(
                "{} chapter(s) changed on both machines — resolve them, then sync again.{merged_note}",
                conflicts.len()
            ),
            merged,
            conflicts,
            reload: false,
        });
    }

    // Clean reconciliation: commit the merged tree as a two-parent merge so both
    // histories stay reachable, then bring the working tree up to it. `write_tree_to`
    // persists the merged tree into the repo's odb without touching the on-disk index.
    let tree_id = index.write_tree_to(repo).map_err(|e| e.to_string())?;
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;
    let sig = signature(repo)?;
    let message = format!("Sync remote changes into {branch}");
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        &message,
        &tree,
        &[&local, &theirs],
    )
    .map_err(|e| format!("sync commit: {e}"))?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| e.to_string())?;

    let summary = if merged.is_empty() {
        "Synced with the remote — your local edits lead.".to_string()
    } else {
        format!(
            "Synced with the remote — merged {} chapter(s) cleanly.",
            merged.len()
        )
    };
    Ok(SyncOutcome {
        summary,
        merged,
        conflicts: Vec::new(),
        reload: true,
    })
}

/// Complete a two-way [`sync`] the author has resolved by hand (#963). A [`sync`]
/// that found per-chapter conflicts left the repo exactly as it was — no merge
/// commit, both versions preserved. Once the author has chosen a final text for
/// every conflicting chapter (keep local / take remote / a hand-merged blend),
/// this lands them: it re-derives the same three-way merge, layers the clean
/// remote-only edits and the author's `resolutions` (path → chosen text) onto the
/// local tree, and records the two-parent merge commit so both histories stay
/// reachable — the exact finalization the conflict-free path does, with the
/// conflicting chapters filled in by the author instead of aborted.
///
/// It re-fetches `origin` so it works against the live remote head rather than a
/// stale `FETCH_HEAD`, and (like [`sync`]) refuses a dirty tree up front so the
/// merge checkout can't overwrite an uncommitted edit. `resolutions` must cover
/// every still-conflicting chapter; a chapter not in the map keeps its local
/// version. Returns the author-facing summary.
/// The blobs of the `origin/<branch>` tracking ref, keyed by path — the remote as the
/// last fetch left it. [`finalize_sync`] reads this before re-fetching to recover the
/// per-path "theirs" the detecting [`sync`] saw, so it can tell whether a resolved
/// chapter's remote moved again in the meantime. `None` when the ref is missing or
/// unreadable (there was no prior fetch), so the caller can fall back rather than
/// treat "no baseline" as "everything changed".
fn tracking_blobs(repo: &git2::Repository, branch: &str) -> Option<BTreeMap<String, git2::Oid>> {
    let refname = format!("refs/remotes/origin/{branch}");
    let tree = repo
        .find_reference(&refname)
        .ok()?
        .peel_to_commit()
        .ok()?
        .tree()
        .ok()?;
    let mut blobs = BTreeMap::new();
    collect_blobs(&tree, &mut blobs);
    Some(blobs)
}

pub fn finalize_sync(
    root: &Path,
    resolutions: &BTreeMap<String, String>,
) -> Result<String, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    if is_dirty(&repo)? {
        return Err(DIRTY_PULL.to_string());
    }

    let branch = current_branch(&repo)?;

    // Capture the remote exactly as it stood when the conflict was detected. The
    // detecting [`sync`]'s fetch left that state in the `origin` tracking ref, and we
    // read it *before* the re-fetch below overwrites it. This is the per-path "theirs"
    // baseline the resolution was made against; a resolved chapter whose remote blob
    // has moved since means a third push landed on the very chapter being resolved
    // (see the guard in the reconcile loop). `None` when the tracking ref is
    // unreadable (never after a conflict-detecting sync), in which case we can't tell
    // and fall back to the prior behavior rather than refuse spuriously.
    let prior_theirs = tracking_blobs(&repo, &branch);

    let mut remote = repo
        .find_remote("origin")
        .map_err(|_| NO_REMOTE.to_string())?;
    let mut cbs = git2::RemoteCallbacks::new();
    cbs.credentials(credentials);
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(cbs);
    remote
        .fetch(&[&branch], Some(&mut opts), None)
        .map_err(|e| classify(&e).message())?;

    let fetch_head = repo
        .find_reference("FETCH_HEAD")
        .map_err(|e| e.to_string())?;
    let fetched = repo
        .reference_to_annotated_commit(&fetch_head)
        .map_err(|e| e.to_string())?;

    let local = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| e.to_string())?;
    let theirs = repo.find_commit(fetched.id()).map_err(|e| e.to_string())?;

    let base_tree = match repo.merge_base(local.id(), theirs.id()) {
        Ok(base) => Some(
            repo.find_commit(base)
                .and_then(|c| c.tree())
                .map_err(|e| e.to_string())?,
        ),
        Err(_) => None,
    };
    let local_tree = local.tree().map_err(|e| e.to_string())?;
    let their_tree = theirs.tree().map_err(|e| e.to_string())?;

    let mut base = BTreeMap::new();
    if let Some(t) = &base_tree {
        collect_blobs(t, &mut base);
    }
    let mut ours = BTreeMap::new();
    collect_blobs(&local_tree, &mut ours);
    let mut theirs_blobs = BTreeMap::new();
    collect_blobs(&their_tree, &mut theirs_blobs);

    let mut paths: Vec<String> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs_blobs.keys())
        .cloned()
        .collect();
    paths.sort();
    paths.dedup();

    // Rebuild the merged tree exactly as sync_diverged would, but this time the
    // both-sides-changed chapters aren't dead-ends: they take the author's chosen
    // text. Start from local so a keep-local resolution (and every local-only edit)
    // is already in place. An index (not a flat TreeBuilder) so nested chapter paths
    // merge correctly.
    let mut index = index_from_tree(&local_tree)?;
    let mut fresh_conflicts = Vec::new();
    for path in paths {
        let b = base.get(&path).copied();
        let o = ours.get(&path).copied();
        let t = theirs_blobs.get(&path).copied();

        if let Some(chosen) = resolutions.get(&path) {
            // A resolved conflict — but only if the remote for this chapter hasn't
            // moved since detection. If a third push landed on the very chapter the
            // author resolved (its re-fetched blob differs from the baseline the
            // resolution was made against), writing the resolution here would silently
            // drop that newer remote edit: the merge commit's second parent is the
            // fresh `theirs`, so git would treat the path as resolved and it would
            // never resurface. Refuse instead, the same way an unresolved fresh
            // conflict does below. When we have no baseline (`None`) we can't compare,
            // so fall back to applying the resolution as before.
            let remote_moved = prior_theirs
                .as_ref()
                .is_some_and(|prior| prior.get(&path).copied() != t);
            if remote_moved {
                fresh_conflicts.push(path);
                continue;
            }
            // Write the author's final text verbatim.
            let oid = repo
                .blob(chosen.as_bytes())
                .map_err(|e| format!("write resolution: {e}"))?;
            stage_blob(&mut index, &path, oid)?;
            continue;
        }
        if o == t {
            continue;
        }
        if o == b {
            // Only the remote changed it — adopt the remote version.
            adopt(&mut index, &path, t)?;
        } else if t == b {
            // Only local changed it — already in the index from local_tree.
        } else {
            // Both changed it, differently, yet the author never resolved it — the
            // remote must have moved again between detection and finalization (a
            // third push landed). Committing here would silently drop that new remote
            // edit, so refuse (mirrors finalize_branch_merge's guard).
            fresh_conflicts.push(path);
        }
    }

    if !fresh_conflicts.is_empty() {
        return Err(format!(
            "The remote changed again while you were resolving — {} chapter(s) now \
             conflict that weren't covered ({}). Run Sync again to pick them up.",
            fresh_conflicts.len(),
            fresh_conflicts.join(", "),
        ));
    }

    let tree_id = index.write_tree_to(&repo).map_err(|e| e.to_string())?;
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;
    let sig = signature(&repo)?;
    let message = format!("Sync remote changes into {branch}");
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        &message,
        &tree,
        &[&local, &theirs],
    )
    .map_err(|e| format!("sync commit: {e}"))?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| e.to_string())?;

    Ok(format!(
        "Resolved {} conflicting chapter(s) and completed the sync.",
        resolutions.len()
    ))
}
