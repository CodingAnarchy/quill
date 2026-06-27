//! Project switching and directory browsing: the project/dir browsers and the
//! new-project flow. Extracted from `mod.rs` (#314).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};

use crate::buffer::Buffer;
use crate::config::ProjectsConfig;
use crate::mode::{
    Command, DirBrowser, DirEntry, Mode, Palette, PaletteItem, Pending, ProjectBrowser,
};
use crate::project::Project;
use crate::recent;

use super::App;

impl App {
    /// Open the book switcher for a multi-book series (#329): list the books with
    /// the active one marked, selecting one makes it active. A no-op (with a
    /// status message) for a standalone novel.
    pub(crate) fn open_book_switcher(&mut self) {
        let count = self.project.book_count();
        if count <= 1 {
            self.message = Some("This project has only one book".into());
            return;
        }
        let active = self.project.active_book();
        let items = (0..count)
            .map(|i| {
                let title = self
                    .project
                    .book_title(i)
                    .unwrap_or("(untitled)")
                    .to_string();
                let label = if i == active {
                    format!("{title}  ·  current")
                } else {
                    title
                };
                PaletteItem {
                    label,
                    desc: format!("book {} of {count}", i + 1),
                    command: Command::SwitchBook(i),
                }
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Switch book",
            items,
            selected: active,
        }));
    }

    /// Make book `index` the active one (#329): flush the outgoing book's edits,
    /// swap the project to the target book, reload the editor on its first
    /// chapter, and re-baseline the daily tally for the new book's structure.
    pub(crate) fn switch_book(&mut self, index: usize) {
        if index == self.project.active_book() || index >= self.project.book_count() {
            return;
        }
        // The continuous view windows one book; drop to single-chapter, saving it.
        self.leave_continuous();
        // Persist the chapter we're leaving before swapping books — never drop work.
        if self.flush_active().is_err() {
            return;
        }
        self.project.set_active_book(index);
        match Buffer::from_path(self.project.active_path()) {
            Ok(buffer) => {
                self.buffer = buffer;
                // The new book has its own parts and chapters: re-baseline so its
                // existing words aren't counted as today's writing and the per-part
                // counters match its structure (#329).
                self.reset_tally_for_active_book();
                let title = self.project.book_title(index).unwrap_or("");
                self.message = Some(format!("Book: {title}"));
            }
            Err(e) => self.message = Some(format!("Open failed: {e}")),
        }
    }

    /// Open the project browser over the recently-opened projects (newest first).
    /// Empty until at least one project has been opened.
    pub(super) fn open_project_browser(&mut self) {
        // Recent projects first (newest), then every other project under the
        // configured projects-home — a complete library view (#272). Omitted
        // silently when no home is set. Always opens even when empty, so `b`
        // (browse) stays reachable for a first-time author (#271).
        let mut projects = recent::list();
        if let Some(home) = ProjectsConfig::load().home_dir() {
            let known: HashSet<PathBuf> = projects.iter().map(|p| p.path.clone()).collect();
            projects.extend(
                recent::scan(&home)
                    .into_iter()
                    .filter(|p| !known.contains(&p.path)),
            );
        }
        self.mode = Mode::ProjectBrowser(Box::new(ProjectBrowser {
            projects,
            selected: 0,
        }));
    }

    /// Navigate the project browser (#270): arrows move, Enter switches to the
    /// selected project, `b` browses the filesystem (#271), Esc returns to editing.
    pub(super) fn on_key_project_browser(&mut self, key: KeyEvent) {
        let Mode::ProjectBrowser(state) = &mut self.mode else {
            return;
        };
        if let Some(s) = super::assist::list_move(state.selected, state.projects.len(), key.code) {
            state.selected = s;
            return;
        }
        match key.code {
            KeyCode::Enter => {
                if let Some(p) = state.projects.get(state.selected) {
                    let target = p.path.clone();
                    self.request_switch(target);
                }
            }
            KeyCode::Char('b') => self.open_dir_browser(),
            KeyCode::Char('n') => {
                let base = self.new_project_base();
                self.prompt_new_project(base);
            }
            KeyCode::Esc => self.mode = self.editing_mode(),
            _ => {}
        }
    }

    /// The default location for a new project: the configured projects-home
    /// (#272), else beside the current project.
    pub(super) fn new_project_base(&self) -> PathBuf {
        ProjectsConfig::load()
            .home_dir()
            .filter(|p| p.is_dir())
            .or_else(|| {
                self.project
                    .root
                    .canonicalize()
                    .ok()
                    .and_then(|r| r.parent().map(Path::to_path_buf))
            })
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Prompt for a new project's path, prefilled with `base` and a trailing
    /// separator so the author just types a name (and can edit the location).
    pub(super) fn prompt_new_project(&mut self, base: PathBuf) {
        let prefill = format!("{}/", base.display());
        self.open_prompt(Pending::NewProject, "New project — path: ", &prefill, None);
    }

    /// Create a new project at `target` and open it in place. Refuses to clobber
    /// an existing project, and surfaces a clear message on any failure rather
    /// than crashing (#264/#273).
    pub(super) fn create_project_at(&mut self, target: PathBuf) {
        // Offer the setup wizard only when the project was actually created and
        // opened (#282), and the wizard isn't disabled.
        if self.create_and_open(&target) && ProjectsConfig::load().wizard_enabled() {
            self.start_wizard();
        }
    }

    /// Create a new project at `target` and switch to it in place. Returns whether
    /// the switch landed (false if a project already exists there, creation fails,
    /// or a save aborts the switch), with a clear message set on failure. Shared
    /// by "new empty project" (#273) and "new project from notes" (#286).
    pub(crate) fn create_and_open(&mut self, target: &Path) -> bool {
        if Project::manifest_path(target).exists() {
            self.message = Some(format!("A project already exists at {}", target.display()));
            self.mode = self.editing_mode();
            return false;
        }
        match Project::create(target) {
            // switch_to saves the current project, opens the new one, and records
            // it in recents — the same path a normal switch takes.
            Ok(_) => {
                self.switch_to(target.to_path_buf());
                self.project.root == *target
            }
            Err(e) => {
                self.message = Some(format!("Couldn't create project: {e}"));
                self.mode = self.editing_mode();
                false
            }
        }
    }

    /// Open the directory navigator to find a project that isn't in the recent
    /// list. Starts beside the current project (its parent directory), where
    /// sibling books most likely live.
    fn open_dir_browser(&mut self) {
        let start = self.browse_start_dir();
        self.browse_into(start);
    }

    /// A sensible directory to start browsing from: the parent of the current
    /// project (sibling projects), falling back to the working directory.
    fn browse_start_dir(&self) -> PathBuf {
        let root = self
            .project
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.project.root.clone());
        root.parent()
            .map(Path::to_path_buf)
            .filter(|p| p.is_dir())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Show `dir` in the directory navigator. An unreadable directory (e.g.
    /// permission denied) surfaces a clear message and leaves the current view
    /// in place, rather than failing silently or crashing (#264/#271).
    pub(super) fn browse_into(&mut self, dir: PathBuf) {
        match Self::dir_entries(&dir) {
            Ok(entries) => {
                self.mode = Mode::DirBrowser(Box::new(DirBrowser {
                    cwd: dir,
                    entries,
                    selected: 0,
                }))
            }
            Err(e) => self.message = Some(format!("Can't open {}: {e}", dir.display())),
        }
    }

    /// The navigable rows for `dir`: a `..` parent entry (unless at the root),
    /// then its subdirectories sorted by name (hidden ones skipped), each flagged
    /// as a project if it holds a `book.toml`.
    fn dir_entries(dir: &Path) -> std::io::Result<Vec<DirEntry>> {
        let mut entries = Vec::new();
        if let Some(parent) = dir.parent() {
            entries.push(DirEntry {
                label: "..".into(),
                path: parent.to_path_buf(),
                is_project: false,
                is_parent: true,
            });
        }
        let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir)?
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .filter(|p| {
                !p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'))
            })
            .collect();
        subdirs.sort();
        for path in subdirs {
            let label = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let is_project = Project::manifest_path(&path).exists();
            entries.push(DirEntry {
                label,
                path,
                is_project,
                is_parent: false,
            });
        }
        Ok(entries)
    }

    /// Navigate the directory browser (#271): arrows move; Enter opens a project
    /// or descends into a folder (`..` goes up); Left/Backspace go up; Esc exits.
    pub(super) fn on_key_dir_browser(&mut self, key: KeyEvent) {
        let Mode::DirBrowser(state) = &mut self.mode else {
            return;
        };
        if let Some(s) = super::assist::list_move(state.selected, state.entries.len(), key.code) {
            state.selected = s;
            return;
        }
        match key.code {
            KeyCode::Enter => {
                let Some(entry) = state.entries.get(state.selected) else {
                    return;
                };
                let path = entry.path.clone();
                if entry.is_project {
                    self.request_switch(path);
                } else {
                    self.browse_into(path); // `..` or a plain subdirectory
                }
            }
            KeyCode::Left | KeyCode::Backspace => {
                if let Some(parent) = state.cwd.parent().map(Path::to_path_buf) {
                    self.browse_into(parent);
                }
            }
            KeyCode::Char('n') => {
                let base = state.cwd.clone(); // create the new project under the folder being browsed
                self.prompt_new_project(base);
            }
            // New project FROM the selected folder's notes (#286): the highlighted
            // directory is the notes source. Skip the `..` row.
            KeyCode::Char('N') => {
                if let Some(entry) = state.entries.get(state.selected).filter(|e| !e.is_parent) {
                    let folder = entry.path.clone();
                    self.begin_new_from_notes(&folder.to_string_lossy());
                }
            }
            KeyCode::Esc => self.mode = self.editing_mode(),
            _ => {}
        }
    }

    /// Switch to `target`, guarding unsaved work first: if the buffer is dirty,
    /// confirm a save-and-switch; if `target` is already open, say so; otherwise
    /// switch straight away.
    pub(super) fn request_switch(&mut self, target: PathBuf) {
        let target = target.canonicalize().unwrap_or(target);
        let current = self
            .project
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.project.root.clone());
        if target == current {
            self.message = Some("That project is already open.".into());
            self.mode = self.editing_mode();
            return;
        }
        if self.edit_dirty() {
            self.pending_switch = Some(target);
            self.open_prompt(
                Pending::ConfirmSwitch,
                "Unsaved changes — save and switch project? (y/N)",
                "",
                None,
            );
        } else {
            self.switch_to(target);
        }
    }

    /// Save and finalize the current project, then load `target` in place — the
    /// running editor adopts the new project wholesale, with no state carried
    /// over. A save failure aborts the switch (the current project stays put);
    /// an open failure leaves the current project open with a message.
    pub(super) fn switch_to(&mut self, target: PathBuf) {
        if self.finalize_current_project().is_err() {
            return; // save failed — message set; don't risk the unsaved work
        }
        self.open_in_place(target);
    }

    /// Save and finalize the current project before adopting another in its place:
    /// flush the continuous window back to its chapters and the active chapter to
    /// disk, then drop the recovery sidecar. Returns `Err` (with a message set) if
    /// the save fails, so the caller aborts rather than risk unsaved work. Shared
    /// by switching projects (#270) and renaming the open one (#424).
    fn finalize_current_project(&mut self) -> Result<(), ()> {
        self.leave_continuous();
        self.flush_active()?;
        self.clear_recovery();
        Ok(())
    }

    /// Adopt the project at `target` wholesale — a fresh [`App`] with no state
    /// carried over — assuming the current project's work is already saved and
    /// finalized. Shared by switching projects (#270) and renaming the open one
    /// (#424), which both save first and then repoint here.
    fn open_in_place(&mut self, target: PathBuf) {
        let opened = Project::open(&target).and_then(|project| {
            let buffer = Buffer::from_path(project.active_path())?;
            Ok((project, buffer))
        });
        match opened {
            Ok((project, buffer)) => {
                let root = project.root.clone();
                let title = project.manifest.title.clone();
                // A fresh App resets every per-project field (buffer, manuscript,
                // splits, tally baselines, caches, VC status) in one move.
                *self = App::new(buffer, project);
                self.load_history();
                recent::record(&root, &title);
                self.message = Some(format!("Opened \u{201c}{title}\u{201d}"));
                self.offer_recovery(); // honor the incoming project's unsaved work, if any
            }
            Err(e) => {
                self.message = Some(format!("Couldn't open project: {e}"));
                self.mode = self.editing_mode();
            }
        }
    }

    /// Prompt for the open project's new folder name (#424), prefilled with the
    /// book title so matching the folder to the title is a single Enter. Renaming
    /// moves only the directory, not the title (they're edited separately).
    pub(crate) fn prompt_rename_project(&mut self) {
        let prefill = self.project.manifest.title.clone();
        self.open_prompt(
            Pending::RenameProject,
            "Rename project folder (matches the book title): ",
            &prefill,
            None,
        );
    }

    /// Rename the open project's directory to `new_name`, in place: save the
    /// current work, move the folder, and reopen the project at the new path so
    /// every save from here lands there (#424). Refuses to clobber an existing
    /// folder, and leaves the project untouched on any failure (no half-move).
    pub(crate) fn rename_project(&mut self, new_name: &str) {
        let name = crate::project::sanitize_dir_name(new_name);
        if name.is_empty() {
            self.message = Some("Enter a folder name.".into());
            self.mode = self.editing_mode();
            return;
        }
        // Resolve against the real location so the parent is correct even when the
        // project was opened by a relative path.
        let old_root = self
            .project
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.project.root.clone());
        let Some(new_root) = old_root.parent().map(|p| p.join(&name)) else {
            self.message = Some("Can't rename the filesystem root.".into());
            self.mode = self.editing_mode();
            return;
        };
        if new_root == old_root {
            self.message = Some(format!("Project is already named \u{201c}{name}\u{201d}."));
            self.mode = self.editing_mode();
            return;
        }
        if new_root.exists() {
            self.message = Some(format!(
                "A folder named \u{201c}{name}\u{201d} already exists here."
            ));
            self.mode = self.editing_mode();
            return;
        }
        // Save and finalize the current project at the OLD location before moving,
        // so nothing writes back to a path that's about to disappear.
        if self.finalize_current_project().is_err() {
            return; // save failed — message set; don't risk the move
        }
        if let Err(e) = std::fs::rename(&old_root, &new_root) {
            // The project is still intact at the old location; just report.
            self.message = Some(format!("Couldn't rename project: {e}"));
            self.mode = self.editing_mode();
            return;
        }
        // Reopen at the new path (the old recents entry self-prunes — its folder is
        // gone). open_in_place sets an "Opened …" message; replace it with the
        // rename confirmation.
        self.open_in_place(new_root.clone());
        if self.project.root == new_root {
            self.message = Some(format!("Renamed project folder to \u{201c}{name}\u{201d}."));
        }
    }
}
