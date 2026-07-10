//! Branch-as-draft (#965): a named branch is a "draft" — an alternate version
//! (alt ending, POV split, an experiment) tracked as a first-class line beside
//! the mainline. Create one from the current state, switch between them (the
//! working copy follows), list them, and merge one draft back into another. A
//! merge reuses the chapter-level three-way the two-way sync (#962) does, so a
//! clean merge lands a two-parent commit and a conflicting one surfaces the same
//! per-chapter [`ChapterConflict`]s the resolution UI (#963) already resolves.

use std::collections::BTreeMap;

use super::super::remote::merge::{adopt, blob_text, collect_blobs, index_from_tree, stage_blob};
use super::super::remote::ChapterConflict;
use super::{is_dirty, signature};

/// The name of the draft (branch) currently checked out (#965), e.g. `main` or
/// `alt-ending`. Errors on a detached or unborn HEAD (no draft to name — nothing
/// is committed yet). The mirror of [`super::super::remote`]'s `current_branch`,
/// exposed here so the drafts overlay can show which draft is active.
pub fn current_branch(root: &std::path::Path) -> Result<String, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let head = repo
        .head()
        .map_err(|_| "No commits yet — commit before working with drafts.".to_string())?;
    head.shorthand()
        .map(str::to_string)
        .ok_or_else(|| "Not on a draft (detached HEAD).".to_string())
}

/// Every draft (local branch) in the project (#965), sorted by name. Empty when
/// nothing is committed yet (no branches exist). The list the drafts overlay
/// renders, one row per draft, with the current one marked.
pub fn list_branches(root: &std::path::Path) -> Result<Vec<String>, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let branches = repo
        .branches(Some(git2::BranchType::Local))
        .map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    for branch in branches {
        let (branch, _) = branch.map_err(|e| e.to_string())?;
        if let Some(name) = branch
            .name()
            .map_err(|e| e.to_string())?
            .map(str::to_string)
        {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

/// Create a new draft `name` from the current state (#965) — a branch at the
/// current `HEAD`. Does *not* switch to it (the caller decides), so an author can
/// spin off an alt version and keep writing the current one, or create-and-switch
/// in one gesture. Fails when nothing is committed yet (no commit to branch from)
/// or a draft by that name already exists, so an existing draft is never silently
/// moved out from under its own history.
pub fn create_branch(root: &std::path::Path, name: &str) -> Result<(), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let head = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|_| "No commits yet — commit before creating a draft.".to_string())?;
    if repo.find_branch(name, git2::BranchType::Local).is_ok() {
        return Err(format!("A draft named '{name}' already exists."));
    }
    repo.branch(name, &head, false)
        .map_err(|e| format!("create draft: {e}"))?;
    Ok(())
}

/// Switch to the draft `name` (#965): point `HEAD` at that branch and bring the
/// working tree up to its version, so the editor now shows that draft's chapters.
/// Refuses up front when the working tree is dirty (#639) — the force checkout
/// would clobber uncommitted edits — so the guard mirrors [`super::super::remote::pull`]:
/// the caller commits (or restores) first. A no-op switch to the current draft is
/// rejected with a clear message rather than a churny re-checkout. The caller
/// reloads the active chapter afterwards, since the files on disk have changed.
pub fn checkout_branch(root: &std::path::Path, name: &str) -> Result<(), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    if current_branch(root).ok().as_deref() == Some(name) {
        return Err(format!("Already on draft '{name}'."));
    }
    if is_dirty(&repo)? {
        return Err(SWITCH_DIRTY.to_string());
    }
    let branch = repo
        .find_branch(name, git2::BranchType::Local)
        .map_err(|_| format!("No draft named '{name}'."))?;
    let refname = branch
        .get()
        .name()
        .ok_or_else(|| "draft has no ref name".to_string())?
        .to_string();
    let object = repo
        .revparse_single(&refname)
        .map_err(|e| format!("resolve draft: {e}"))?;
    // Force overwrites any working-tree edits (already known clean by the guard) and
    // remove_untracked drops files the target draft doesn't have, so the tree is
    // exactly that draft's snapshot, not a superset of it.
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force().remove_untracked(true);
    repo.checkout_tree(&object, Some(&mut checkout))
        .map_err(|e| format!("switch draft: {e}"))?;
    repo.set_head(&refname)
        .map_err(|e| format!("switch draft: {e}"))?;
    Ok(())
}

/// The result of merging one draft into another (#965): a clean merge that landed a
/// commit, or a set of per-chapter conflicts that stopped it. On [`Conflicts`] the
/// repo is left exactly as it was — nothing committed, both drafts intact — so the
/// resolution UI (#963) can resolve them and [`finalize_branch_merge`] finish the job.
///
/// [`Conflicts`]: MergeOutcome::Conflicts
pub enum MergeOutcome {
    /// The merge landed cleanly. `merged` lists the chapters adopted from the source
    /// draft (empty when only the target draft had changed since they split).
    Clean { merged: Vec<String> },
    /// The merge stopped on chapters changed differently on both drafts. Each carries
    /// all three versions, for the resolution UI (#963) — nothing was written.
    Conflicts { conflicts: Vec<ChapterConflict> },
}

/// Merge the draft `from` into the draft `into` (#965) — fold an alt version back
/// into the mainline (or vice-versa). `into` is checked out afterwards, so the
/// author lands on the draft that received the merge. Reuses the two-way sync's
/// chapter-level three-way (#962): base = the drafts' shared ancestor, "ours" =
/// `into`, "theirs" = `from`. A chapter only `from` changed is adopted; one only
/// `into` changed is kept; one both changed differently is a
/// [conflict](ChapterConflict). A clean merge records a two-parent commit (both
/// drafts' history reachable) and updates the working tree; a conflicting one
/// changes nothing and returns the conflicts for [`finalize_branch_merge`] to land
/// once resolved.
///
/// Refuses a dirty working tree up front (#639), like [`checkout_branch`], so the
/// merge checkout can't overwrite an uncommitted edit. `into` must be the current
/// draft (the working tree is already its snapshot) — the caller switches to it
/// first — so a clean merge lands on disk without an extra checkout dance.
pub fn merge_branch(
    root: &std::path::Path,
    into: &str,
    from: &str,
) -> Result<MergeOutcome, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    if into == from {
        return Err("A draft can't merge into itself.".to_string());
    }
    if is_dirty(&repo)? {
        return Err(SWITCH_DIRTY.to_string());
    }
    if current_branch(root).ok().as_deref() != Some(into) {
        return Err(format!("Switch to draft '{into}' before merging into it."));
    }

    let (ours, theirs, base_tree) = merge_bases(&repo, into, from)?;
    let ours_tree = ours.tree().map_err(|e| e.to_string())?;
    let theirs_tree = theirs.tree().map_err(|e| e.to_string())?;

    // Reconcile chapter-by-chapter, filling no conflicts (this pass only *detects*
    // them). An empty resolution map means a both-sides-changed chapter is reported,
    // never written.
    let recon = reconcile_trees(
        &repo,
        base_tree.as_ref(),
        &ours_tree,
        &theirs_tree,
        &BTreeMap::new(),
    )?;
    if !recon.conflicts.is_empty() {
        // Never write a half-merged tree — hand the conflicts back for resolution.
        return Ok(MergeOutcome::Conflicts {
            conflicts: recon.conflicts,
        });
    }

    land_merge(&repo, into, from, &ours, &theirs, recon.tree)?;
    Ok(MergeOutcome::Clean {
        merged: recon.merged,
    })
}

/// Complete a draft merge the author resolved by hand (#965) — the branch-merge
/// counterpart to [`super::super::remote::finalize_sync`]. A [`merge_branch`] that hit
/// conflicts left the repo untouched; once the author has chosen a final text for
/// each conflicting chapter (`resolutions`: path → chosen text), this re-derives the
/// same three-way, layers the clean source-only edits and the author's resolutions
/// onto the target draft's tree, and records the two-parent merge commit. `into`
/// must still be the current draft. Refuses a dirty tree up front. Returns the count
/// of chapters landed for the caller's summary.
pub fn finalize_branch_merge(
    root: &std::path::Path,
    into: &str,
    from: &str,
    resolutions: &BTreeMap<String, String>,
) -> Result<usize, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    if is_dirty(&repo)? {
        return Err(SWITCH_DIRTY.to_string());
    }
    if current_branch(root).ok().as_deref() != Some(into) {
        return Err(format!("Switch to draft '{into}' before merging into it."));
    }

    let (ours, theirs, base_tree) = merge_bases(&repo, into, from)?;
    let ours_tree = ours.tree().map_err(|e| e.to_string())?;
    let theirs_tree = theirs.tree().map_err(|e| e.to_string())?;

    let recon = reconcile_trees(
        &repo,
        base_tree.as_ref(),
        &ours_tree,
        &theirs_tree,
        resolutions,
    )?;
    // Any conflict left unresolved means the caller under-supplied resolutions; the
    // three-way filled the resolved ones already, so a leftover is a real gap.
    if !recon.conflicts.is_empty() {
        return Err(format!(
            "{} chapter(s) still need resolving before the merge can complete.",
            recon.conflicts.len()
        ));
    }
    land_merge(&repo, into, from, &ours, &theirs, recon.tree)?;
    Ok(resolutions.len())
}

/// Resolve the two draft tips and their shared-ancestor tree for a merge (#965).
/// Returns `(into_commit, from_commit, base_tree)`; the base is `None` for unrelated
/// histories (treated as an empty ancestor, so every touched chapter reads as added
/// on both drafts).
fn merge_bases<'r>(
    repo: &'r git2::Repository,
    into: &str,
    from: &str,
) -> Result<(git2::Commit<'r>, git2::Commit<'r>, Option<git2::Tree<'r>>), String> {
    let ours = branch_commit(repo, into)?;
    let theirs = branch_commit(repo, from)?;
    let base_tree = match repo.merge_base(ours.id(), theirs.id()) {
        Ok(base) => Some(
            repo.find_commit(base)
                .and_then(|c| c.tree())
                .map_err(|e| e.to_string())?,
        ),
        Err(_) => None,
    };
    Ok((ours, theirs, base_tree))
}

/// The commit a local draft points at, or a clear error naming the missing draft.
fn branch_commit<'r>(repo: &'r git2::Repository, name: &str) -> Result<git2::Commit<'r>, String> {
    repo.find_branch(name, git2::BranchType::Local)
        .map_err(|_| format!("No draft named '{name}'."))?
        .get()
        .peel_to_commit()
        .map_err(|e| format!("resolve draft '{name}': {e}"))
}

/// The outcome of a chapter-level three-way over two trees (#965): the merged tree
/// (written to the odb) plus the chapters adopted from "theirs" and any unresolved
/// conflicts. The shared body of [`merge_branch`] (detect) and
/// [`finalize_branch_merge`] (apply resolutions), so both walk the trees identically.
struct Reconciled {
    tree: git2::Oid,
    merged: Vec<String>,
    conflicts: Vec<ChapterConflict>,
}

/// Reconcile `ours` and `theirs` against their `base` chapter-by-chapter (#965) — the
/// same three-way the two-way sync (#962) runs, over two local drafts. Starts the
/// merged tree from `ours` (so an ours-only edit and a keep-target resolution are
/// already in place), adopts a theirs-only edit, writes any `resolutions` verbatim,
/// and records a [`ChapterConflict`] for a both-changed chapter not in `resolutions`.
/// The tree is always written (cheap, unreferenced until a commit points at it); the
/// caller ignores it when conflicts remain.
fn reconcile_trees(
    repo: &git2::Repository,
    base: Option<&git2::Tree>,
    ours: &git2::Tree,
    theirs: &git2::Tree,
    resolutions: &BTreeMap<String, String>,
) -> Result<Reconciled, String> {
    let mut base_blobs = BTreeMap::new();
    if let Some(t) = base {
        collect_blobs(t, &mut base_blobs);
    }
    let mut our_blobs = BTreeMap::new();
    collect_blobs(ours, &mut our_blobs);
    let mut their_blobs = BTreeMap::new();
    collect_blobs(theirs, &mut their_blobs);

    let mut paths: Vec<String> = base_blobs
        .keys()
        .chain(our_blobs.keys())
        .chain(their_blobs.keys())
        .cloned()
        .collect();
    paths.sort();
    paths.dedup();

    let mut merged = Vec::new();
    let mut conflicts = Vec::new();
    // An index (not a flat TreeBuilder) so nested chapter paths merge correctly.
    let mut index = index_from_tree(ours)?;

    for path in paths {
        let b = base_blobs.get(&path).copied();
        let o = our_blobs.get(&path).copied();
        let t = their_blobs.get(&path).copied();

        if let Some(chosen) = resolutions.get(&path) {
            // A resolved conflict: write the author's final text verbatim.
            let oid = repo
                .blob(chosen.as_bytes())
                .map_err(|e| format!("write resolution: {e}"))?;
            stage_blob(&mut index, &path, oid)?;
            continue;
        }
        if o == t {
            continue; // both drafts agree (including both absent) — nothing to do
        }
        if o == b {
            // Only the source draft changed this chapter — adopt its version.
            adopt(&mut index, &path, t)?;
            merged.push(path);
        } else if t == b {
            // Only the target draft changed it — keep ours (already in the index).
        } else {
            // Both changed it, differently — a per-chapter conflict (#965).
            conflicts.push(ChapterConflict {
                path: path.clone(),
                local: blob_text(repo, o),
                remote: blob_text(repo, t),
                base: blob_text(repo, b),
            });
        }
    }

    let tree = index.write_tree_to(repo).map_err(|e| e.to_string())?;
    Ok(Reconciled {
        tree,
        merged,
        conflicts,
    })
}

/// Record the two-parent merge commit for a clean (or resolved) draft merge and
/// bring the working tree up to it (#965). `into` is the current draft, so the
/// commit lands on its ref and the force checkout is safe (the caller confirmed the
/// tree clean). Both drafts' commits stay reachable through the two parents.
fn land_merge(
    repo: &git2::Repository,
    into: &str,
    from: &str,
    ours: &git2::Commit,
    theirs: &git2::Commit,
    tree: git2::Oid,
) -> Result<(), String> {
    let tree = repo.find_tree(tree).map_err(|e| e.to_string())?;
    let sig = signature(repo)?;
    let message = format!("Merge draft '{from}' into '{into}'");
    let refname = format!("refs/heads/{into}");
    repo.commit(Some(&refname), &sig, &sig, &message, &tree, &[ours, theirs])
        .map_err(|e| format!("merge commit: {e}"))?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Shown when switching drafts or merging is refused because the working tree has
/// uncommitted edits the checkout would overwrite (#965). Commit (or restore) first.
const SWITCH_DIRTY: &str =
    "You have uncommitted changes — commit them before switching or merging drafts.";
