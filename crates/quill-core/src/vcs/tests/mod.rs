//! Tests for the version-control seam (#659). They drive the re-exported public
//! surface (`crate::vcs::*`) rather than either half directly, so a function moving
//! between [`super::super::local`] and [`super::super::remote`] never touches these
//! files. All are hermetic (#531): they build repositories under a scratch dir, or
//! use a bare repo as a stand-in remote, so nothing reaches the network.
//!
//! Split to mirror the code modules: [`history`] covers init/commit/log/milestones/
//! diff/restore, [`clone`] cloning onto a second device (#1231), [`remote`] the
//! push/pull plumbing, [`sync`] the two-way sync (#962)
//! and its hand-resolved finalization (#963), and [`drafts`] branch-as-draft (#965).

mod clone;
mod divert;
mod drafts;
mod history;
mod remote;
mod sync;

/// A fresh scratch directory for a hermetic vcs test, namespaced under `vcs`.
/// Shared by every submodule so each test builds its repositories in isolation.
pub(super) fn temp_dir(name: &str) -> std::path::PathBuf {
    crate::testutil::scratch_dir("vcs", name)
}
