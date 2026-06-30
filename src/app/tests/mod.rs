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
    pub(crate) fn new(path: PathBuf) -> Self {
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
mod ai_cost_routing;
mod ai_generation;
mod ai_images;
mod ai_prose;
mod ai_qa_trackers;
mod analysis;
mod annotations;
mod assistant;
mod autosave;
mod book_details;
mod books;
mod chapters_continuous;
mod chapters_counts_and_perf;
mod chapters_editing;
mod chapters_filter;
mod chapters_sections;
mod configure;
mod editing;
mod export_styling;
mod goals;
mod imports;
mod models;
mod panes;
mod project_search;
mod projects;
mod recovery;
mod snippets;
mod spell;
mod sprint;
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

/// A continuity result with one event and the given findings — shared by the
/// continuity-tracker tests and the triage tests (#318 split).
fn cont_with(findings: Vec<continuity::Finding>) -> Continuity {
    Continuity {
        events: vec![continuity::Event {
            description: "alpha event".into(),
            chapter: "One".into(),
            story_order: 1,
        }],
        findings,
    }
}

/// A unique-per-process scratch directory for an app-test project (#428).
pub(crate) fn app_scratch_dir(name: &str) -> PathBuf {
    crate::testutil::scratch_dir("app", name)
}

/// Open the project at `dir`, build the editor, and pin every config-derived
/// setting (#428). The single open→build→pin path every app fixture builds on:
/// `App::new` reads the developer's real config, so without these pins a dev with
/// Vim mode, smart typography, or an on-save trigger would make tests flaky or
/// config-dependent. Tests that want those opt back in explicitly.
pub(crate) fn open_test_app(dir: &Path) -> App {
    let project = Project::open(dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);
    app.vim = false;
    app.smart_typography = false;
    app.mode = Mode::Insert;
    app.workflows = Vec::new();
    app.triggers = Default::default();
    app
}

/// A two-chapter project on disk with distinct content per chapter.
fn two_chapter_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    fs::write(dir.join("two.md"), "second chapter\n").unwrap();

    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

/// A three-chapter project (`c0/c1/c2.md` = "alpha"/"beta"/"gamma") with the same
/// test pins as [`two_chapter_app`] — for the continuous-view tests, where the
/// window holds several chapters at known byte offsets.
fn three_chapter_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    let mut manifest = String::from("title = \"Book\"\n\n");
    for (i, body) in ["alpha\n", "beta\n", "gamma\n"].iter().enumerate() {
        manifest.push_str(&format!(
            "[[chapters]]\ntitle = \"C{i}\"\nfile = \"c{i}.md\"\n\n"
        ));
        fs::write(dir.join(format!("c{i}.md")), body).unwrap();
    }
    fs::write(Project::manifest_path(&dir), manifest).unwrap();

    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

/// Point the config dir at a fresh temp dir for this test thread (#528), so
/// `recent::record`/`list` and config reads touch scratch state, never the
/// developer's real `~/.config/quill`. Shared by the project-management and
/// import test modules (#500).
pub(super) fn isolate(tag: &str) {
    let dir = crate::testutil::scratch_dir("xdg", tag);
    crate::config::set_config_dir(Some(dir));
}

/// A one-chapter project on disk with the given title and body.
pub(super) fn make_project(tag: &str, title: &str, body: &str) -> PathBuf {
    let dir = crate::testutil::scratch_dir("proj270", tag);
    fs::write(
        Project::manifest_path(&dir),
        format!("title = \"{title}\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n"),
    )
    .unwrap();
    fs::write(dir.join("one.md"), body).unwrap();
    dir
}

pub(super) fn open_app(dir: &Path) -> App {
    open_test_app(dir)
}

/// A directory holding several project subdirs (and any plain ones the caller
/// adds), for exercising the navigator and as a parent for new projects.
pub(super) fn books_dir(tag: &str) -> PathBuf {
    crate::testutil::scratch_dir("navbooks", tag)
}
