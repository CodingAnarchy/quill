//! The remote/auth half of version control (#659): pointing the project at a
//! hosted `origin`, pushing and pulling, resolving credentials, and translating
//! libgit2's network/auth errors into author-facing guidance. The offline,
//! working-tree side (including the [`is_dirty`](super::local::is_dirty) guard
//! `pull` relies on) lives in [`super::local`].
//!
//! Remote failures are triaged into a small taxonomy (#967) rather than a single
//! generic "operation failed" string: [`RemoteError`] carries a [category](RemoteErrorKind)
//! — offline/network, auth, a rejected (non-fast-forward) push, or anything else —
//! and each category renders an actionable message *plus* a recovery hint. A bare
//! "pull first" leaves the author stuck when the real cause was a dropped network
//! or a stale token; the category lets the sync/push/pull flow say which of those
//! it was, and what to do next. The push/pull/sync entry points still hand callers
//! a `String` (what the UI shows), but it's built from a category via
//! [`RemoteError::message`], not lifted from a raw libgit2 line.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use git2::{ErrorClass, ErrorCode};

use super::local::{is_dirty, signature};

/// Point the project at a hosted remote (#134). Stores it as `origin`, replacing
/// any existing one, so reconfiguring is just calling this again. The URL can be
/// HTTPS (`https://…`), SSH (`git@…`), or a local path — auth is resolved at
/// push/pull time, not here.
pub fn set_remote(root: &Path, url: &str) -> Result<(), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    // remote_set_url creates or updates; fall back to add for a brand-new remote.
    if repo.find_remote("origin").is_ok() {
        repo.remote_set_url("origin", url)
            .map_err(|e| format!("set remote: {e}"))?;
    } else {
        repo.remote("origin", url)
            .map_err(|e| format!("add remote: {e}"))?;
    }
    Ok(())
}

/// The configured `origin` URL, if any — for showing the author where their work
/// backs up to.
pub fn remote_url(root: &Path) -> Option<String> {
    let repo = git2::Repository::open(root).ok()?;
    let remote = repo.find_remote("origin").ok()?;
    remote.url().map(str::to_string)
}

/// Push the current branch's commits to `origin` (#134). Never forces: if the
/// remote has commits the local branch doesn't, libgit2 rejects the
/// non-fast-forward push and we surface that ("pull first") rather than
/// clobbering remote history. Returns the branch name pushed.
pub fn push(root: &Path) -> Result<String, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let branch = current_branch(&repo)?;
    let mut remote = repo
        .find_remote("origin")
        .map_err(|_| NO_REMOTE.to_string())?;

    // A rejected ref (e.g. non-fast-forward) is reported through this callback,
    // not always as an error from push() — capture it so we never silently
    // "succeed" at a push the remote refused.
    let rejected = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
    let captured = rejected.clone();
    let mut cbs = git2::RemoteCallbacks::new();
    cbs.credentials(credentials);
    cbs.push_update_reference(move |_refname, status| {
        if let Some(reason) = status {
            *captured.borrow_mut() = Some(reason.to_string());
        }
        Ok(())
    });
    let mut opts = git2::PushOptions::new();
    opts.remote_callbacks(cbs);

    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    remote
        .push(&[&refspec], Some(&mut opts))
        .map_err(|e| classify(&e).message())?;

    // A rejection that came back via the callback rather than as an error.
    if rejected.borrow_mut().take().is_some() {
        return Err(RemoteError::rejected().message());
    }
    Ok(branch)
}

/// Pull `origin`'s changes for the current branch (#134), resolving a diverged
/// branch in-app (#703). Non-destructive throughout — it never resets, force-pushes,
/// or discards a commit:
///
/// * **Up to date / behind** — a fast-forward moves the branch to the fetched commit.
/// * **Diverged, no conflicts** — both sides committed. The fetched tree is merged
///   into local and, when the merge is clean, committed as an ordinary two-parent
///   merge commit, so both authors' commits stay reachable.
/// * **Diverged, with conflicts** — the same files were edited on both machines in a
///   way git can't reconcile. Rather than write half-merged files or leave the repo
///   mid-merge, the attempt is rolled back and the conflicted paths are reported so
///   the author knows exactly what to reconcile.
///
/// A fast-forward (and the merge checkout) finishes with a force checkout of the
/// resulting tree, which would clobber uncommitted working-tree edits. Rather than
/// trust the caller to keep the tree clean, `pull` refuses up front when the tree is
/// dirty (#639) — so a save-but-don't-commit-then-pull can never silently discard the
/// author's words. Returns a human message.
pub fn pull(root: &Path) -> Result<String, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;

    // Guard against silent data loss: the fast-forward below force-checks-out the
    // fetched tree, overwriting any uncommitted edits. Refuse before touching the
    // network so the author commits (or restores) those changes first.
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

    if analysis.is_up_to_date() {
        return Ok("Already up to date with the remote.".to_string());
    }
    if !analysis.is_fast_forward() {
        // Local and remote both moved (each has commits the other lacks). Merge the
        // fetched commit into local rather than dead-ending (#703).
        return merge_diverged(&repo, &branch, &fetched);
    }

    // Fast-forward: move the branch ref to the fetched commit and update the tree.
    let refname = format!("refs/heads/{branch}");
    match repo.find_reference(&refname) {
        Ok(mut r) => {
            r.set_target(fetched.id(), "pull: fast-forward")
                .map_err(|e| e.to_string())?;
        }
        Err(_) => {
            // Unborn branch (no local commits yet) — create it at the fetched commit.
            repo.reference(&refname, fetched.id(), true, "pull: initial")
                .map_err(|e| e.to_string())?;
        }
    }
    repo.set_head(&refname).map_err(|e| e.to_string())?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| e.to_string())?;
    Ok("Pulled the latest changes from the remote.".to_string())
}

/// Merge the fetched commit into the local branch when the two have diverged (#703).
/// The tree is already known clean (the caller's #639 dirty guard runs first), so a
/// clean merge can safely land on disk. On a clean merge this writes a two-parent
/// merge commit (both authors' history preserved) and checks it out; on conflicts it
/// rolls the attempt back and reports the conflicted paths without changing a commit
/// or leaving a half-written tree.
fn merge_diverged(
    repo: &git2::Repository,
    branch: &str,
    fetched: &git2::AnnotatedCommit,
) -> Result<String, String> {
    let local = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| e.to_string())?;
    let their_commit = repo.find_commit(fetched.id()).map_err(|e| e.to_string())?;

    // Merge the two trees in memory first, so a conflict never touches the working
    // tree or the index — nothing to unwind on the conflict path.
    let mut merged = repo
        .merge_commits(&local, &their_commit, None)
        .map_err(|e| format!("merge: {e}"))?;

    if merged.has_conflicts() {
        let files = conflicted_paths(&merged);
        return Err(format!(
            "Local and remote have diverged and the same text was edited on both — \
             Quill can't merge {files} automatically. Restore or reconcile {them} \
             (the History view shows each side), then pull again.",
            files = if files.is_empty() {
                "these changes".to_string()
            } else {
                files.clone()
            },
            them = if files.is_empty() { "them" } else { "it" },
        ));
    }

    // Clean merge: write the merged tree and record a two-parent merge commit so both
    // sides' commits stay in history.
    let tree_id = merged
        .write_tree_to(repo)
        .map_err(|e| format!("merge tree: {e}"))?;
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;
    let sig = signature(repo)?;
    let message = format!("Merge remote changes into {branch}");
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        &message,
        &tree,
        &[&local, &their_commit],
    )
    .map_err(|e| format!("merge commit: {e}"))?;

    // Bring the working tree up to the merged result.
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| e.to_string())?;
    Ok("Merged the remote's changes with yours — push to share the merge.".to_string())
}

/// The paths libgit2 reports as conflicting in a merged index, joined for display.
/// Reads the "our" side's path (present for edit/edit conflicts, the common case for
/// two authors touching the same chapter).
fn conflicted_paths(index: &git2::Index) -> String {
    let Ok(conflicts) = index.conflicts() else {
        return String::new();
    };
    let mut names: Vec<String> = conflicts
        .filter_map(std::result::Result::ok)
        .filter_map(|c| c.our.or(c.their).or(c.ancestor))
        .filter_map(|entry| String::from_utf8(entry.path).ok())
        .collect();
    names.sort();
    names.dedup();
    names.join(", ")
}

/// One chapter changed on both machines since the shared ancestor — a per-chapter
/// conflict two-way [`sync`] detects but never resolves on its own (#962). Chapters
/// are atomic files, so a conflict is per-chapter, not per-line: the same chapter
/// was edited differently on each side. All three versions are carried so the
/// resolution UI (#963) can show base-vs-local-vs-remote and let the author choose,
/// and so sync itself never has to pick a side or clobber one.
///
/// The three version strings are the data the resolution UI (#963) renders and acts
/// on; this ticket only detects and exposes them, so today they're read by name only
/// from the tests — annotated so the not-yet-wired bin build stays warning-clean.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct ChapterConflict {
    /// The chapter's path relative to the repo root (its file name).
    pub path: String,
    /// The version currently on this machine (left on disk, untouched by sync).
    pub local: String,
    /// The version fetched from the remote (preserved here, not written to disk).
    pub remote: String,
    /// The shared-ancestor version both sides diverged from (empty when the chapter
    /// was added independently on both sides).
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
    /// the count in, so today the list itself is read only by the tests (and the
    /// #963 UI to come) — annotated so the bin build stays warning-clean.
    #[cfg_attr(not(test), allow(dead_code))]
    pub merged: Vec<String>,
    /// Chapters changed on both sides — surfaced for resolution (#963), never
    /// silently clobbered. Empty on a clean sync.
    pub conflicts: Vec<ChapterConflict>,
    /// Whether the working tree on disk changed, so the editor should reload the
    /// active chapter (mirrors [`pull`]'s reload signal).
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
///   advance to the fetched commit, exactly like [`pull`].
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
/// Like [`pull`], sync refuses up front when the working tree is dirty (#639), so an
/// uncommitted edit can never be overwritten by the merge checkout.
pub fn sync(root: &Path) -> Result<SyncOutcome, String> {
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

    if analysis.is_up_to_date() {
        return Ok(SyncOutcome {
            summary: "Already in sync with the remote.".to_string(),
            merged: Vec::new(),
            conflicts: Vec::new(),
            reload: false,
        });
    }
    if analysis.is_fast_forward() {
        fast_forward(&repo, &branch, &fetched)?;
        return Ok(SyncOutcome {
            summary: "Pulled the remote's changes — you were behind.".to_string(),
            merged: Vec::new(),
            conflicts: Vec::new(),
            reload: true,
        });
    }

    sync_diverged(&repo, &branch, &fetched)
}

/// Advance the branch ref and working tree to the fetched commit — the shared
/// fast-forward step of [`pull`] and [`sync`]. The caller has confirmed the tree is
/// clean, so the force checkout is safe.
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
pub fn finalize_sync(
    root: &Path,
    resolutions: &BTreeMap<String, String>,
) -> Result<String, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
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
            // A resolved conflict: write the author's final text verbatim.
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

/// Write the remote's version of `path` into the merged tree index (or stage its
/// removal when the remote deleted the chapter). The chapter is adopted wholesale —
/// chapters are atomic files, so there's no sub-file merge to do. Shared with the
/// branch merge (#965), which reconciles two local branches with the same
/// chapter-level three-way.
///
/// The merged tree is assembled through a [`git2::Index`] rather than a flat
/// `TreeBuilder`: libgit2's single-level `TreeBuilder` rejects any entry name
/// containing `/`, so nested chapter paths (`bible/setting.md`, `submission/…`)
/// abort the whole merge. An index seeded with `read_tree` handles the full path
/// natively and writes the nested subtrees for us.
pub(super) fn adopt(
    index: &mut git2::Index,
    path: &str,
    theirs: Option<git2::Oid>,
) -> Result<(), String> {
    match theirs {
        Some(oid) => stage_blob(index, path, oid),
        None => index
            .remove_path(Path::new(path))
            .map_err(|e| e.to_string()),
    }
}

/// Stage `oid` at `path` in the merged tree `index` as a regular-file blob. Builds
/// the [`git2::IndexEntry`] by hand (rather than `add_path`, which would read the
/// working directory) so the merge stays purely in-memory; the timestamp/ownership
/// fields are zeroed since only the path, mode, and blob id matter for `write_tree`.
pub(super) fn stage_blob(
    index: &mut git2::Index,
    path: &str,
    oid: git2::Oid,
) -> Result<(), String> {
    let entry = git2::IndexEntry {
        ctime: git2::IndexTime::new(0, 0),
        mtime: git2::IndexTime::new(0, 0),
        dev: 0,
        ino: 0,
        mode: git2::FileMode::Blob.into(),
        uid: 0,
        gid: 0,
        file_size: 0,
        id: oid,
        flags: 0,
        flags_extended: 0,
        path: path.as_bytes().to_vec(),
    };
    index.add(&entry).map_err(|e| e.to_string())
}

/// Seed a fresh in-memory index with the blobs of `tree`, the starting point for a
/// three-way merge. Kept off the repository's on-disk index so the merge never
/// touches the working index or the checkout; the caller layers adoptions/removals
/// on top and calls [`git2::Index::write_tree_to`] to persist the merged tree.
pub(super) fn index_from_tree(tree: &git2::Tree) -> Result<git2::Index, String> {
    let mut index = git2::Index::new().map_err(|e| e.to_string())?;
    index.read_tree(tree).map_err(|e| e.to_string())?;
    Ok(index)
}

/// Collect every blob (file) in `tree`, keyed by its full path, as `path -> blob id`.
/// Recurses into subtrees so nested paths are compared too; non-blob entries (the
/// subtree objects themselves) are skipped, since only files carry chapter text.
/// Shared with the branch merge (#965).
pub(super) fn collect_blobs(tree: &git2::Tree, out: &mut BTreeMap<String, git2::Oid>) {
    let _ = tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if entry.kind() == Some(git2::ObjectType::Blob) {
            if let Some(name) = entry.name() {
                out.insert(format!("{dir}{name}"), entry.id());
            }
        }
        git2::TreeWalkResult::Ok
    });
}

/// The text of a blob by id, lossily decoded as UTF-8 (chapters are Markdown).
/// Empty string when the blob is absent (the chapter didn't exist on that side) or
/// can't be read — a missing ancestor reads as "added", the caller's intent. Shared
/// with the branch merge (#965).
pub(super) fn blob_text(repo: &git2::Repository, oid: Option<git2::Oid>) -> String {
    oid.and_then(|oid| repo.find_blob(oid).ok())
        .map(|blob| String::from_utf8_lossy(blob.content()).into_owned())
        .unwrap_or_default()
}

const NO_REMOTE: &str = "No remote configured — set one first.";

/// Shown when a pull is refused because the working tree has uncommitted edits a
/// fast-forward checkout would overwrite (#639). Commit (or restore) them first.
const DIRTY_PULL: &str = "You have uncommitted changes — commit them before pulling.";

/// How a remote operation failed, at the granularity the author can act on (#967).
/// libgit2 reports dozens of distinct error codes, but for triage they collapse to
/// four buckets — each with its own recovery move — so the sync/push/pull flow can
/// say *which kind* of failure it was rather than a single generic line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteErrorKind {
    /// The remote couldn't be reached at all — no network, DNS failure, connection
    /// refused/timed out, host down. The work is fine; the link isn't.
    Offline,
    /// The remote was reached but rejected the credentials — no/expired token, an
    /// SSH key the ssh-agent doesn't hold, a 401/403. Nothing to pull or push until
    /// auth is fixed.
    Auth,
    /// The push was refused as a non-fast-forward: the remote has commits the local
    /// branch doesn't, so pushing would drop them. Pull/sync first, then push.
    Rejected,
    /// Anything else — a real failure worth surfacing verbatim, but not one of the
    /// three actionable categories above.
    Other,
}

/// A categorized remote failure (#967): a [kind](RemoteErrorKind) plus the raw
/// libgit2 detail, from which [`message`](RemoteError::message) builds the
/// author-facing string (actionable message + recovery hint). Kept as a distinct
/// type — rather than folding straight to a `String` — so callers (and tests) can
/// branch on the category, and so the message/hint wording lives in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteError {
    /// The triage category the failure falls into.
    pub kind: RemoteErrorKind,
    /// The underlying libgit2 message in its original case, carried so an `Other`
    /// failure can still show the author what actually went wrong.
    detail: String,
}

impl RemoteError {
    /// A rejected (non-fast-forward) push — the branch has diverged from the remote.
    /// Named because [`push`] reaches this both from a push error and from the
    /// update-reference callback, and both want the identical guidance.
    const fn rejected() -> Self {
        Self {
            kind: RemoteErrorKind::Rejected,
            detail: String::new(),
        }
    }

    /// The author-facing string: an actionable statement of what went wrong followed
    /// by the concrete recovery move. This is what push/pull/sync return as their
    /// error, so the category shapes the message the editor shows.
    pub fn message(&self) -> String {
        match self.kind {
            RemoteErrorKind::Offline => {
                "Can't reach the remote — check your internet connection, then try again."
                    .to_string()
            }
            RemoteErrorKind::Auth => "Authentication failed — the remote rejected your \
                 credentials. Set up an SSH key, a credential helper, or a QUILL_GIT_TOKEN, \
                 then try again."
                .to_string(),
            RemoteErrorKind::Rejected => {
                "The remote has changes you don't — pull first, then push again.".to_string()
            }
            RemoteErrorKind::Other => {
                format!(
                    "Remote operation failed: {} Try again, or check the remote.",
                    {
                        let d = self.detail.trim();
                        if d.is_empty() {
                            "the remote operation failed"
                        } else {
                            d
                        }
                    }
                )
            }
        }
    }
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

/// The short name of the branch `HEAD` points at (e.g. `main`), or an error for a
/// detached or unborn HEAD (nothing to push/pull).
fn current_branch(repo: &git2::Repository) -> Result<String, String> {
    let head = repo
        .head()
        .map_err(|_| "No commits yet — commit before syncing.".to_string())?;
    head.shorthand()
        .map(str::to_string)
        .ok_or_else(|| "Not on a branch.".to_string())
}

/// git2 credential callback: reuse whatever the author already has, rather than
/// inventing key management. SSH URLs use the ssh-agent; HTTPS uses a token from
/// the environment if present, otherwise the system git credential helper (e.g.
/// the macOS keychain or `gh`). Documented in `docs/git-integration-spike.md`.
fn credentials(
    url: &str,
    username: Option<&str>,
    allowed: git2::CredentialType,
) -> Result<git2::Cred, git2::Error> {
    use git2::CredentialType as Type;
    if allowed.contains(Type::SSH_KEY) {
        return git2::Cred::ssh_key_from_agent(username.unwrap_or("git"));
    }
    if allowed.contains(Type::USER_PASS_PLAINTEXT) {
        if let Some(token) = token_from_env() {
            return git2::Cred::userpass_plaintext(username.unwrap_or("git"), &token);
        }
        let config = git2::Config::open_default()?;
        return git2::Cred::credential_helper(&config, url, username);
    }
    if allowed.contains(Type::USERNAME) {
        return git2::Cred::username(username.unwrap_or("git"));
    }
    Err(git2::Error::from_str(
        "no supported authentication method available",
    ))
}

/// A personal-access token from the environment, for HTTPS auth without a
/// configured credential helper. Checks Quill's own var first, then the
/// conventional GitHub ones.
fn token_from_env() -> Option<String> {
    ["QUILL_GIT_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
}

/// Triage a libgit2 error into the [`RemoteError`] taxonomy (#967). Classification
/// leans on libgit2's own `class`/`code` first — the authoritative signal — and
/// falls back to matching the message text, since many failures libgit2 reports
/// under a generic class (`Net`, `Http`) only distinguish offline-vs-auth-vs-rejected
/// in their wording. Order matters: auth and rejected are checked before the broad
/// network bucket so a "401 over HTTP" reads as auth, not merely "network".
fn classify(e: &git2::Error) -> RemoteError {
    let msg = e.message().to_lowercase();
    let kind = if is_auth(e, &msg) {
        RemoteErrorKind::Auth
    } else if is_rejected(&msg) {
        RemoteErrorKind::Rejected
    } else if is_offline(e, &msg) {
        RemoteErrorKind::Offline
    } else {
        RemoteErrorKind::Other
    };
    RemoteError {
        kind,
        detail: e.message().to_string(),
    }
}

/// The remote rejected the author's credentials. libgit2 flags this as
/// [`ErrorCode::Auth`]/[`ErrorCode::Certificate`] or through the SSH/callback error
/// classes; HTTP transports instead surface a 401/403 or the word "authentication"
/// in the message, so both signals are checked.
fn is_auth(e: &git2::Error, msg: &str) -> bool {
    matches!(e.code(), ErrorCode::Auth | ErrorCode::Certificate)
        || matches!(e.class(), ErrorClass::Ssh)
        || msg.contains("authentication")
        || msg.contains("authorization")
        || msg.contains("credentials")
        || msg.contains("401")
        || msg.contains("403")
}

/// The push was refused as a non-fast-forward — the local branch is behind the
/// remote. libgit2 reports this only in the message (there's no dedicated code for
/// it on the push path), so it's matched on the wording git uses.
fn is_rejected(msg: &str) -> bool {
    msg.contains("fast-forward")
        || msg.contains("fast forward")
        || msg.contains("rejected")
        || msg.contains("cannot push")
        || msg.contains("non-fast-forward")
        || msg.contains("not present locally")
        || msg.contains("tip of your current branch is behind")
}

/// The remote couldn't be reached. Covers the network/TLS error classes plus the
/// message shapes for DNS/connection failures and timeouts that libgit2 sometimes
/// reports under a generic class.
fn is_offline(e: &git2::Error, msg: &str) -> bool {
    matches!(
        e.class(),
        ErrorClass::Net | ErrorClass::Ssl | ErrorClass::Http
    ) || msg.contains("could not resolve")
        || msg.contains("couldn't resolve")
        || msg.contains("failed to resolve")
        || msg.contains("connection refused")
        || msg.contains("could not connect")
        || msg.contains("couldn't connect")
        || msg.contains("failed to connect")
        || msg.contains("timed out")
        || msg.contains("timeout")
        || msg.contains("network is unreachable")
        || msg.contains("no route to host")
        || msg.contains("name or service not known")
        || msg.contains("temporary failure in name resolution")
}

#[cfg(test)]
mod error_taxonomy_tests {
    //! Hermetic (#531) tests over the remote-error taxonomy (#967): forge a
    //! `git2::Error` with a chosen class/code/message and assert `classify` buckets it
    //! into the expected category, and that each category renders an actionable
    //! message. No repository, network, or auth is involved — the mapping is pure.

    use super::*;

    /// Build a libgit2 error with a specific class/code/message, as the transports
    /// produce on a real failure.
    fn err(code: ErrorCode, class: ErrorClass, message: &str) -> git2::Error {
        git2::Error::new(code, class, message)
    }

    #[test]
    fn offline_is_categorized_from_the_network_class() {
        // A dropped/absent network surfaces under libgit2's Net class…
        let e = err(
            ErrorCode::GenericError,
            ErrorClass::Net,
            "failed to connect to github.com: Network is unreachable",
        );
        let mapped = classify(&e);
        assert_eq!(mapped.kind, RemoteErrorKind::Offline);
        let msg = mapped.message();
        assert!(msg.contains("Can't reach the remote"), "{msg}");
        assert!(msg.contains("connection"), "recovery hint present: {msg}");
    }

    #[test]
    fn offline_is_categorized_from_the_message_when_the_class_is_generic() {
        // …and even when libgit2 reports it under a generic class, the DNS/connection
        // wording still reads as offline.
        for detail in [
            "could not resolve host: origin",
            "Connection refused",
            "operation timed out",
        ] {
            let e = err(ErrorCode::GenericError, ErrorClass::None, detail);
            assert_eq!(
                classify(&e).kind,
                RemoteErrorKind::Offline,
                "should read as offline: {detail}"
            );
        }
    }

    #[test]
    fn auth_is_categorized_from_the_code_and_from_http_status() {
        // libgit2's own auth code…
        let by_code = classify(&err(
            ErrorCode::Auth,
            ErrorClass::None,
            "too many redirects or authentication replays",
        ));
        assert_eq!(by_code.kind, RemoteErrorKind::Auth);
        // …a certificate rejection…
        let by_cert = classify(&err(
            ErrorCode::Certificate,
            ErrorClass::Ssl,
            "the SSL certificate is invalid",
        ));
        assert_eq!(by_cert.kind, RemoteErrorKind::Auth);
        // …and an HTTP 401 the transport reports only in the message.
        let by_status = classify(&err(
            ErrorCode::GenericError,
            ErrorClass::Http,
            "request failed with status code: 401",
        ));
        assert_eq!(by_status.kind, RemoteErrorKind::Auth);

        let msg = by_code.message();
        assert!(msg.contains("Authentication failed"), "{msg}");
        assert!(
            msg.contains("SSH key") || msg.contains("QUILL_GIT_TOKEN"),
            "recovery hint present: {msg}"
        );
        // The multi-line literal joins on single spaces (no double-space seam).
        assert!(!msg.contains("  "), "no doubled spaces: {msg:?}");
    }

    #[test]
    fn rejected_non_fast_forward_is_categorized() {
        // The classic "pull first" push refusal, in git's own wording.
        for detail in [
            "cannot push non-fast-forward",
            "failed to push some refs: Updates were rejected",
            "the tip of your current branch is behind its remote counterpart",
        ] {
            let mapped = classify(&err(ErrorCode::GenericError, ErrorClass::None, detail));
            assert_eq!(
                mapped.kind,
                RemoteErrorKind::Rejected,
                "should read as rejected: {detail}"
            );
        }
        let msg = RemoteError::rejected().message();
        assert!(msg.contains("pull first"), "{msg}");
    }

    #[test]
    fn auth_wins_over_the_network_class() {
        // A 401 arriving over an HTTP transport (Net/Http class) is auth, not merely
        // "offline" — the ordering in `classify` guarantees this.
        let e = err(
            ErrorCode::GenericError,
            ErrorClass::Http,
            "remote authentication required but no callback set: 401",
        );
        assert_eq!(classify(&e).kind, RemoteErrorKind::Auth);
    }

    #[test]
    fn unknown_failures_fall_through_to_other_and_show_the_detail() {
        // Anything outside the three actionable buckets stays verbatim so the author
        // sees what actually went wrong.
        let e = err(
            ErrorCode::GenericError,
            ErrorClass::None,
            "unexpected packfile object",
        );
        let mapped = classify(&e);
        assert_eq!(mapped.kind, RemoteErrorKind::Other);
        let msg = mapped.message();
        assert!(msg.contains("Remote operation failed"), "{msg}");
        assert!(
            msg.contains("unexpected packfile object"),
            "the raw detail is preserved: {msg}"
        );
    }

    #[test]
    fn every_category_renders_a_nonempty_actionable_message() {
        for kind in [
            RemoteErrorKind::Offline,
            RemoteErrorKind::Auth,
            RemoteErrorKind::Rejected,
            RemoteErrorKind::Other,
        ] {
            let e = RemoteError {
                kind,
                detail: "detail".to_string(),
            };
            assert!(!e.message().trim().is_empty(), "{kind:?} has a message");
            // Display mirrors message() so `{e}` at a call site is the same string.
            assert_eq!(e.to_string(), e.message());
        }
    }
}
