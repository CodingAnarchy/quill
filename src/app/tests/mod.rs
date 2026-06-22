//! Unit tests for the `app` layer, split by area into submodules. Shared helpers
//! that several areas use live here; area-specific helpers live with their tests.

use super::*;
use crate::project::Project;
use std::fs;
use std::path::PathBuf;

mod actions;
mod ai;
mod chapters;
mod editing;
mod panes;
mod vcs;
mod vim;

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
fn two_chapter_app(name: &str) -> (App, PathBuf) {
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
    app.mode = Mode::Insert;
    (app, dir)
}
