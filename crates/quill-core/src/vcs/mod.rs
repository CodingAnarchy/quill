//! Version control (#54), the single seam between Quill and Git. Everything that
//! touches `git2` (libgit2) lives here, so the backend stays swappable (the spike
//! #129 chose git2 over the CLI and gitoxide; see `docs/git-integration-spike.md`).
//!
//! Version control is opt-in: a project is only a repository once the author runs
//! [`init`]. This first slice (#130) covers detecting and initializing a
//! repository with a sensible `.gitignore`; commit / status / log / diff land in
//! the later tickets through this same module.
//!
//! Two cohesive halves make up the module (#659): [`local`] is the offline,
//! working-tree side (init/status/log/show/diff/restore/commit and the drafts and
//! commit-graph layout) and [`remote`] is the network/auth side (set_remote/
//! remote_url/push/pull/sync, credential resolution, and the remote-error taxonomy).
//! Each half is a directory of concern submodules (#968–#972 pattern), but both are
//! re-exported here so `crate::vcs::…` callers see one flat surface and never care
//! which half — or which submodule — a function lives in.

mod local;
mod remote;

pub use local::{
    checkout_branch, commit, commit_findings, create_branch, create_tag, current_branch, diff,
    diff_file_to_worktree, diff_tree_to_worktree, finalize_branch_merge, init, is_repo,
    list_branches, list_tags, log, merge_branch, restore, restore_all, set_tag_message,
    show_file_at, status, CommitMeta, MergeOutcome, TagMeta,
};
pub use remote::{finalize_sync, pull, push, remote_url, set_remote, sync, ChapterConflict};

// `diff_file` is the commit-to-commit diff primitive the UI doesn't wire yet —
// reached by name only from the tests, so re-export it there.
#[cfg(any(test, feature = "test-support"))]
pub use local::diff_file;

#[cfg(test)]
mod tests;
