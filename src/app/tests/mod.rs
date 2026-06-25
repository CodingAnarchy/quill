//! Unit tests for the `app` layer, split by area into submodules. Shared helpers
//! that several areas use live here; area-specific helpers live with their tests.

use super::*;
use crate::project::Project;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// Owns a test's temp project directory and removes it on `Drop` (#317), so the
/// tree is cleaned on both the success and the panic (failing-assertion) paths —
/// no per-test `fs::remove_dir_all` line to remember, and no orphaned dirs left
/// behind by a failing run. Derefs to `Path`, so `&dir`, `dir.join(..)`, and the
/// like keep working as they did with the bare `PathBuf`.
pub struct TempProject {
    path: PathBuf,
}

impl TempProject {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Deref for TempProject {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for TempProject {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

mod actions;
mod ai;
mod books;
mod chapters;
mod configure;
mod editing;
mod panes;
mod project_search;
mod projects;
mod recovery;
mod spell;
mod typography;
mod vcs;
mod vim;
mod workflows;

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Type a string into the active prompt (or the buffer, in insert mode).
fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

/// A two-chapter project on disk with distinct content per chapter.
fn two_chapter_app(name: &str) -> (App, TempProject) {
    let mut dir = std::env::temp_dir();
    dir.push(format!("quill-app-{}-{}", std::process::id(), name));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    fs::write(dir.join("two.md"), "second chapter\n").unwrap();

    let project = Project::open(&dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);
    // App::new reads the developer's real config; pin the modeless default so the
    // tests don't depend on whether the dev has Vim mode (#27) enabled. The Vim
    // tests opt back in explicitly via `vim_app`.
    app.vim = false;
    // Smart typography (#296) is on by default; pin it off so tests that type
    // quotes/dashes/dots assert the literal ASCII (the typography tests opt in).
    app.smart_typography = false;
    app.mode = Mode::Insert;
    // Likewise drop any workflows / triggers from the real config, so a developer
    // with an on-save trigger doesn't perturb the save-based tests (#228). The
    // workflow tests set these explicitly.
    app.workflows = Vec::new();
    app.triggers = Default::default();
    (app, TempProject::new(dir))
}
