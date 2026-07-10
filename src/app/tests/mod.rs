//! Unit tests for the `app` layer, split by area into submodules. Shared helpers
//! that several areas use live here; area-specific helpers live with their tests.

use super::*;
use crate::project::Project;
use crate::workflow::Triggers;
use std::fmt::Write;
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
mod ai_accuracy;
mod ai_characters;
mod ai_continuity;
mod ai_cost_routing;
mod ai_foreshadow;
mod ai_generation;
mod ai_images;
mod ai_plot;
mod ai_prose;
mod ai_qa;
mod ai_relationship;
mod ai_theme;
mod analysis;
mod annotations;
mod archive;
mod assistant;
mod auto_pair;
mod autosave;
mod backup;
mod backup_browser;
mod batch_update;
mod book_details;
mod books;
mod books_bible;
mod books_export;
mod books_switching;
mod chapters_continuous;
mod chapters_counts_and_perf;
mod chapters_editing;
mod chapters_filter;
mod chapters_sections;
mod clipboard;
mod configure;
mod cooccurrence;
mod corkboard;
mod drafts;
mod editing;
mod export_styling;
mod folds;
mod goals;
mod health;
mod imports;
mod join_reflow;
mod marks;
mod models;
mod mouse;
mod onboarding;
mod outline;
mod panes;
mod pass_queue;
mod project_search;
mod projects;
mod prompt_editing;
mod quick_switch;
mod recovery;
mod research_notebook;
mod resume;
mod review_import;
mod session_recap;
mod similar;
mod snippets;
mod spell;
mod sprint;
mod style_flags;
mod typography;
mod vcs;
mod vcs_concurrency;
mod vim;
mod vim_case;
mod vim_dot;
mod vim_ex;
mod vim_marks;
mod workflows;

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// A key with the Ctrl modifier held (e.g. Ctrl-Backspace).
fn ctrl_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
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
pub fn app_scratch_dir(name: &str) -> PathBuf {
    crate::testutil::scratch_dir("app", name)
}

/// Open the project at `dir`, build the editor, and pin every config-derived
/// setting (#428). The single open→build→pin path every app fixture builds on:
/// `App::new` reads the developer's real config, so without these pins a dev with
/// Vim mode, smart typography, or an on-save trigger would make tests flaky or
/// config-dependent. Tests that want those opt back in explicitly.
pub fn open_test_app(dir: &Path) -> App {
    let project = Project::open(dir).unwrap();
    let buffer = Buffer::from_path(project.active_path()).unwrap();
    let mut app = App::new(buffer, project);
    app.vim = false;
    app.smart_typography = false;
    app.auto_pair = false;
    app.mode = Mode::Insert;
    app.workflows = Vec::new();
    app.triggers = Triggers::default();
    // Swap in the in-memory clipboard so copy / paste tests never touch the real
    // OS clipboard or emit a terminal escape (#531, #811).
    app.use_fake_clipboard();
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
        write!(
            manifest,
            "[[chapters]]\ntitle = \"C{i}\"\nfile = \"c{i}.md\"\n\n"
        )
        .unwrap();
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

/// Pick the structure template named `name` from the create-time picker (#611),
/// asserting the picker is open first. The Enter selection routes through the
/// palette handler exactly as a keypress would. Shared by the new-project and
/// setup-wizard tests, which both flow through the picker.
pub(super) fn pick_template(app: &mut App, name: &str) {
    let index = match &app.mode {
        Mode::Palette(p) => p
            .items
            .iter()
            .position(|i| i.label == name)
            .unwrap_or_else(|| panic!("template {name:?} in the picker")),
        _ => panic!("the create flow should open the structure-template picker"),
    };
    if let Mode::Palette(p) = &mut app.mode {
        p.selected = index; // templates are flat/ungrouped, so item index == visible row
    }
    app.on_key(key(KeyCode::Enter));
}

/// A directory holding several project subdirs (and any plain ones the caller
/// adds), for exercising the navigator and as a parent for new projects.
pub(super) fn books_dir(tag: &str) -> PathBuf {
    crate::testutil::scratch_dir("navbooks", tag)
}

/// A two-book series on disk, opened on book one, with the deterministic test
/// pins the shared helper applies (Vim / smart typography / workflows off).
/// Shared by the book-switching, tree-navigator, series-bible, and series-export
/// test modules (#763 split).
pub(super) fn series_app(name: &str) -> (App, TempProject) {
    let dir = crate::testutil::scratch_dir("books", name);
    for sub in ["b1", "b2"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(dir.join("b1/one.md"), "alpha one\n").unwrap();
    fs::write(dir.join("b1/two.md"), "alpha two\n").unwrap();
    fs::write(dir.join("b2/one.md"), "beta one\n").unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"The Saga\"\n\n\
         [[books]]\ntitle = \"Book One\"\n\n\
         [[books.chapters]]\ntitle = \"Dawn\"\nfile = \"b1/one.md\"\n\n\
         [[books.chapters]]\ntitle = \"Noon\"\nfile = \"b1/two.md\"\n\n\
         [[books]]\ntitle = \"Book Two\"\n\n\
         [[books.chapters]]\ntitle = \"Dusk\"\nfile = \"b2/one.md\"\n",
    )
    .unwrap();

    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}
