//! The Swift-facing API of Quill's editing core (#1230), exported with UniFFI.
//!
//! The surface is deliberately coarse: Swift opens a [`QuillProject`] and asks
//! it for whole answers (the chapter list, a chapter's prose), so every rule
//! about how a project is laid out on disk (front matter, cached word counts,
//! series books, short story forms) stays in `quill-core` and the phone can
//! never disagree with the desktop about it.

mod error;
mod git;
mod lookup;
mod project;
mod session;

use std::env;
use std::path::Path;

pub use error::QuillError;
pub use git::{clone_destination, clone_project, set_git_token, SyncReport};
pub use lookup::{has_claude_api_key, look_up_words, set_claude_api_key, WordCandidate};
pub use project::{BookInfo, ChapterInfo, ProjectForm, ProjectInfo, QuillProject};
pub use session::{ChapterSession, StyleKind, StyleSpan};

uniffi::setup_scaffolding!();

/// The core's version, shown in the app's settings so a bug report names it.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Point the core's global state at the app's own container (#1230). On iOS
/// `$HOME` is the sandbox root, which the app can't write, so Quill's config
/// and data directories (the XDG fallbacks the desktop uses) are aimed at
/// `support_dir` (the app's Application Support folder) instead. Call once at
/// launch, before opening a project.
#[uniffi::export]
pub fn configure(support_dir: &str) {
    let base = Path::new(support_dir);
    // SAFETY-free on this platform's terms: `set_var` is only unsound when
    // another thread reads the environment at the same time, and the app calls
    // this at launch, before anything else touches the core.
    env::set_var("XDG_CONFIG_HOME", base.join("config"));
    env::set_var("XDG_DATA_HOME", base.join("data"));
}
