//! Version control (#54), the single seam between Quill and Git. Everything that
//! touches `git2` (libgit2) lives here, so the backend stays swappable (the spike
//! #129 chose git2 over the CLI and gitoxide; see `docs/git-integration-spike.md`).
//!
//! Every book gets version history without setup (#1296): the editor calls
//! [`start_auto_history`] on open and takes [`snapshot`]s as the author works,
//! in a repository Quill marks as its own. A book the author already manages
//! with git, or one inside a larger repository, is left alone. Connecting a
//! remote stays a deliberate, separate step.
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
    diff_file_to_worktree, diff_tree_to_worktree, exclude_editor_state, finalize_branch_merge,
    init, is_repo, list_branches, list_tags, log, merge_branch, restore, restore_all,
    set_tag_message, show_file_at, status, CommitMeta, MergeOutcome, TagMeta,
};
pub use local::{compare_prose, file_log, Change, FileVersion};
pub use local::{is_auto_history, snapshot, snapshot_message, start_auto_history, AutoHistory};
pub use remote::{
    clone, finalize_sync, folder_name, pull, push, remote_url, set_remote, set_token, sync,
    sync_or_divert, ChapterConflict, DeviceSync, RemoteError, RemoteErrorKind, PHONE_DRAFT_PREFIX,
};

// `diff_file` is the commit-to-commit diff primitive the UI doesn't wire yet —
// reached by name only from the tests, so re-export it there.
#[cfg(any(test, feature = "test-support"))]
pub use local::diff_file;

#[cfg(test)]
mod tests;
