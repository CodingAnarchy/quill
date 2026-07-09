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
//! working-tree side (init/status/log/show/diff/restore/commit) and [`remote`] is
//! the network/auth side (set_remote/remote_url/push/pull and credential
//! resolution). Both are re-exported here so `crate::vcs::…` callers see one flat
//! surface and never care which half a function lives in.

mod local;
mod remote;

pub use local::{
    commit, create_tag, diff, diff_file_to_worktree, diff_tree_to_worktree, init, is_repo,
    list_tags, log, restore, restore_all, set_tag_message, show_file_at, status, CommitMeta,
    TagMeta,
};
pub use remote::{pull, push, remote_url, set_remote};

// `diff_file` is the commit-to-commit diff primitive the UI doesn't wire yet —
// reached by name only from the tests, so re-export it there.
#[cfg(test)]
pub use local::diff_file;

#[cfg(test)]
mod tests;
