//! The local, offline half of version control (#659): everything that reads or
//! writes the working tree and this repository's own history — init/status/log,
//! reading a file out of a past commit, the diff helpers, restore, and commit.
//! Nothing here touches the network; the remote/auth side lives in
//! [`super::remote`].
//!
//! Three cohesive concerns sit under this half: [`history`] reads this repo's own
//! history (the log and its graph, milestones, file/tree diff and restore),
//! [`graph`] lays out the commit-graph lanes (#964) the log stamps on, and
//! [`drafts`] is branch-as-draft (#965) with its chapter-level three-way merge.
//! This module keeps init/status/commit and the identity/dirty-tree helpers the
//! remote half also leans on, and re-exports the pieces so `super::local::…` (and
//! the flat `crate::vcs::…` surface) see one facade.

use std::fs;
use std::path::{Path, PathBuf};

mod drafts;
mod graph;
mod history;

pub use drafts::{
    checkout_branch, create_branch, current_branch, finalize_branch_merge, list_branches,
    merge_branch, MergeOutcome,
};
pub use history::{
    create_tag, diff, diff_file_to_worktree, diff_tree_to_worktree, list_tags, log, restore,
    restore_all, set_tag_message, show_file_at, CommitMeta, TagMeta,
};

// The commit-to-commit diff primitive the UI doesn't wire yet — reached by name
// only from the tests, so re-exported (and forwarded by `super`) only under test.
#[cfg(test)]
pub use history::diff_file;

/// Editor state and scratch that should never be committed — Quill's own
/// metadata and OS cruft. Everything else (chapter `.md` files, `book.toml`, the
/// `bible/` and `submission/` artifacts) is tracked. The durable findings store
/// (`bible/findings.toml`) is deliberately *tracked* (#1051): committing it at each
/// milestone is what lets the findings diff read the store as it stood at a past
/// point in history and report which findings a revision resolved / introduced.
const GITIGNORE: &str = "\
# Quill editor state — not part of the manuscript
/.quill/
/unused/
/bible/.state.toml

# OS / editor cruft
.DS_Store
*.swp
*.tmp
";

/// Whether `root` is already the top of a Git repository.
pub fn is_repo(root: &Path) -> bool {
    root.join(".git").exists()
}

/// Initialize a Git repository at `root` and write a default `.gitignore` (only
/// if one isn't already present, so a hand-tuned file is never clobbered).
/// Idempotent: re-initializing an existing repository is safe.
pub fn init(root: &Path) -> Result<(), String> {
    git2::Repository::init(root).map_err(|e| format!("git init: {e}"))?;
    let gitignore = root.join(".gitignore");
    if !gitignore.exists() {
        fs::write(&gitignore, GITIGNORE).map_err(|e| format!("write .gitignore: {e}"))?;
    }
    Ok(())
}

/// The working-tree status: which tracked-or-untracked (non-ignored) files differ
/// from the last commit. The basis for the uncommitted-changes indicator (#132),
/// and the per-file change list the diff/restore (#135) and AI incremental
/// scoping (#90) reuse.
pub struct Status {
    pub changed: Vec<PathBuf>,
}

impl Status {
    pub const fn count(&self) -> usize {
        self.changed.len()
    }
}

/// Compute the working-tree status for `root` — the non-ignored paths that differ
/// from `HEAD` (added, modified, deleted, or untracked). Stat-based via libgit2,
/// so it's cheap enough to refresh on idle, not per keystroke.
pub fn status(root: &Path) -> Result<Status, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let changed = changed_paths(&repo)?;
    Ok(Status { changed })
}

/// The non-ignored paths that differ from `HEAD` (added, modified, deleted, or
/// untracked). Shared by [`status`] and the dirty-tree guard in
/// [`super::remote::pull`].
fn changed_paths(repo: &git2::Repository) -> Result<Vec<PathBuf>, String> {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    let statuses = repo.statuses(Some(&mut opts)).map_err(|e| e.to_string())?;
    Ok(statuses
        .iter()
        .filter(|e| !e.status().is_ignored())
        .filter_map(|e| e.path().map(PathBuf::from))
        .collect())
}

/// Whether the working tree has uncommitted changes (added, modified, deleted, or
/// untracked non-ignored files). The guard [`super::remote::pull`] uses to avoid
/// clobbering edits.
pub(super) fn is_dirty(repo: &git2::Repository) -> Result<bool, String> {
    Ok(!changed_paths(repo)?.is_empty())
}

/// Stage the whole project and commit it with `message`. Returns the new commit's
/// short id, or `None` when there's nothing to commit (a clean working tree).
///
/// Staging respects `.gitignore` and captures additions, modifications, and
/// deletions (e.g. a chapter moved to `unused/`). Identity comes from the git
/// config when set, else a generic default — so an author who has never used git
/// can still commit without configuring anything.
pub fn commit(root: &Path, message: &str) -> Result<Option<String>, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;

    let mut index = repo.index().map_err(|e| e.to_string())?;
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .map_err(|e| e.to_string())?;
    index
        .update_all(["*"].iter(), None)
        .map_err(|e| e.to_string())?; // stage deletions of tracked files

    finalize_commit(&repo, &mut index, message)
}

/// Write the staged `index`, build its tree, and commit it against the current
/// `HEAD` — the finalize sequence [`commit`] and [`commit_findings`] share once
/// each has staged its own paths. Returns the new commit's short id, or `None`
/// when the staged tree is byte-identical to `HEAD`'s (the empty-commit guard).
///
/// Everything from `write_tree` on lives here: identity, parent resolution, the
/// empty-commit check, the commit itself, and short-oid formatting. Only the
/// staging differs between the two callers, so only that stays with them.
fn finalize_commit(
    repo: &git2::Repository,
    index: &mut git2::Index,
    message: &str,
) -> Result<Option<String>, String> {
    let sig = signature(repo)?;
    index.write().map_err(|e| e.to_string())?;
    let tree_id = index.write_tree().map_err(|e| e.to_string())?;
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;

    // Parent = current HEAD, unless this is the first (unborn HEAD) commit.
    let parent = match repo.head() {
        Ok(head) => Some(head.peel_to_commit().map_err(|e| e.to_string())?),
        Err(_) => None,
    };
    // Nothing changed since the last commit → don't create an empty commit.
    if let Some(p) = &parent {
        if p.tree_id() == tree_id {
            return Ok(None);
        }
    }

    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .map_err(|e| format!("commit: {e}"))?;
    Ok(Some(oid.to_string().chars().take(7).collect()))
}

/// Auto-commit *only* the tracked findings store after an analysis pass rewrote it
/// (#1060), leaving any uncommitted manuscript prose untouched in the working tree.
///
/// Since #1051 `bible/findings.toml` is tracked so the findings diff can read it at
/// a past commit, but a pass rewrites it every run — which made [`is_dirty`] count
/// that write as an uncommitted change and blocked Sync/Pull/switch-draft/merge-draft
/// ("commit first") even though no prose changed. This stages that one path and
/// commits it on its own, so the working tree stays clean for those guards *and*
/// each pass's findings land in history for the diff.
///
/// Scoped to a single path on purpose: it stages `findings_rel` alone, never the
/// author's in-progress prose, so an analysis pass can't sweep unsaved edits into a
/// commit. The path is the caller's single source of truth for the tracked store
/// (the app layer's `store_rel_path()`), passed in so a rename can't desync this
/// auto-committer from the diff reader. Returns the new commit's short id, or `None`
/// when the store didn't change (nothing to commit) or the project isn't yet a
/// repository — both no-ops, so the caller can fire it unconditionally after every
/// pass write.
pub fn commit_findings(
    root: &Path,
    findings_rel: &str,
    message: &str,
) -> Result<Option<String>, String> {
    if !is_repo(root) {
        return Ok(None);
    }
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;

    // Stage only the findings path — the rest of the index still mirrors HEAD, so
    // the tree we build is HEAD's tree with just this file updated. Uncommitted
    // prose edits, never staged here, stay in the working tree.
    let mut index = repo.index().map_err(|e| e.to_string())?;
    let rel = Path::new(findings_rel);
    if root.join(rel).exists() {
        index.add_path(rel).map_err(|e| e.to_string())?;
    } else {
        // The store was removed (e.g. the last finding resolved and the file
        // cleaned up); stage that deletion so the commit reflects it.
        let _ = index.remove_path(rel);
    }

    finalize_commit(&repo, &mut index, message)
}

/// The identity to record on a commit — the author's git config when set, else a
/// generic Quill default, so someone who has never configured git can still commit
/// (and merge). Shared by [`commit`] and the merge commit [`super::remote::pull`]
/// creates when a diverged branch merges cleanly.
pub(super) fn signature(repo: &git2::Repository) -> Result<git2::Signature<'static>, String> {
    repo.signature()
        .or_else(|_| git2::Signature::now("Quill Author", "author@quill.local"))
        .map_err(|e| format!("signature: {e}"))
}
