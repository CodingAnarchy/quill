//! The project browser and the filesystem directory navigator: listing the
//! recently-opened projects, the type-to-filter sub-mode (#612), browsing the
//! filesystem for a project (#271), and the browser's rename / delete of a
//! selected project (#612). Split out of `project_ops`; child module of it, so
//! it keeps access to `App`'s private fields. The switch it triggers lands in
//! `request_switch` (the create/open/switch lifecycle in the parent module).

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};

use crate::config::ProjectsConfig;
use crate::mode::{
    on_filter_key, DirBrowser, DirEntry, FilterInput, Mode, Pending, ProjectBrowser,
};
use crate::project::Project;
use crate::recent;

use super::super::App;
use super::set_manifest_title;

impl App {
    /// Open the project browser over the recently-opened projects (newest first).
    /// Empty until at least one project has been opened.
    pub(crate) fn open_project_browser(&mut self) {
        // Recent projects first (newest), then every other project under the
        // configured projects-home — a complete library view (#272). Omitted
        // silently when no home is set. Always opens even when empty, so `b`
        // (browse) stays reachable for a first-time author (#271). Shared with the
        // startup chooser (#526) so the two offer the same library.
        let projects = recent::openable(ProjectsConfig::load().home_dir().as_deref());
        self.mode = Mode::ProjectBrowser(Box::new(ProjectBrowser {
            projects,
            selected: 0,
            filter: String::new(),
            typing: false,
        }));
    }

    /// Navigate the project browser (#270): arrows move (skipping filtered-out
    /// rows), Enter switches to the selected project, `b` browses the filesystem
    /// (#271), `n` creates one, Esc returns to editing. `/` starts the
    /// type-to-filter query (#612); `r` renames and `d` deletes the selected
    /// project.
    pub(crate) fn on_key_project_browser(&mut self, key: KeyEvent) {
        let Mode::ProjectBrowser(state) = &self.mode else {
            return;
        };
        // In the filter-input sub-mode, keystrokes build the query (#612); the
        // browser's single-key commands are reachable only outside it.
        if state.typing {
            self.on_key_project_filter(key);
            return;
        }
        match key.code {
            // Start the type-to-filter query (#612).
            KeyCode::Char('/') => self.set_project_typing(true),
            KeyCode::Up | KeyCode::Char('k') => {
                let s = self.project_step(false);
                self.set_project_selected(s);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let s = self.project_step(true);
                self.set_project_selected(s);
            }
            KeyCode::Enter => {
                if let Some(target) = self.selected_project_path() {
                    self.request_switch(target);
                }
            }
            KeyCode::Char('b') => self.open_dir_browser(),
            KeyCode::Char('n') => {
                let base = self.new_project_base();
                self.prompt_new_project(base);
            }
            // Rename / delete the selected project from the library (#612).
            KeyCode::Char('r') => self.prompt_rename_browser_project(),
            KeyCode::Char('d') => self.prompt_delete_browser_project(),
            // Esc clears an active filter first (restoring the full list), then a
            // second Esc leaves the browser.
            KeyCode::Esc => {
                if self.project_has_filter() {
                    self.clear_project_filter();
                } else {
                    self.mode = self.editing_mode();
                }
            }
            _ => {}
        }
    }

    /// Keys for the project browser's filter-input sub-mode (#612): printable
    /// characters build the query (live), Backspace edits it, the arrows move
    /// within the matches, Enter opens the highlighted one, and Esc leaves input
    /// — keeping the filter so the browser's commands act on the narrowed list (a
    /// second Esc, in command mode, clears it).
    fn on_key_project_filter(&mut self, key: KeyEvent) {
        match on_filter_key(key, false) {
            Some(FilterInput::Leave) => self.set_project_typing(false),
            Some(FilterInput::Enter) => {
                if let Some(target) = self.selected_project_path() {
                    self.request_switch(target);
                }
            }
            Some(FilterInput::Step(down)) => {
                let s = self.project_step(down);
                self.set_project_selected(s);
            }
            Some(FilterInput::Pop) => self.project_filter_pop(),
            Some(FilterInput::Push(c)) => self.project_filter_push(c),
            None => {}
        }
    }

    /// The active filter query, lowercased (empty when not browsing or unfiltered).
    fn project_query(&self) -> String {
        match &self.mode {
            Mode::ProjectBrowser(s) => s.filter.to_lowercase(),
            _ => String::new(),
        }
    }

    fn project_has_filter(&self) -> bool {
        matches!(&self.mode, Mode::ProjectBrowser(s) if !s.filter.is_empty())
    }

    /// The currently-selected project row, cloned (`None` off the browser or when
    /// the list is empty).
    fn selected_project(&self) -> Option<recent::RecentProject> {
        match &self.mode {
            Mode::ProjectBrowser(s) => s.projects.get(s.selected).cloned(),
            _ => None,
        }
    }

    /// The path of the selected project, but only when it passes the active
    /// filter — so Enter never opens a row the query has hidden.
    fn selected_project_path(&self) -> Option<PathBuf> {
        let q = self.project_query();
        self.selected_project()
            .filter(|p| project_matches(p, &q))
            .map(|p| p.path)
    }

    fn set_project_typing(&mut self, on: bool) {
        if let Mode::ProjectBrowser(s) = &mut self.mode {
            s.typing = on;
        }
    }

    fn set_project_selected(&mut self, selected: usize) {
        if let Mode::ProjectBrowser(s) = &mut self.mode {
            s.selected = selected;
        }
    }

    fn clear_project_filter(&mut self) {
        if let Mode::ProjectBrowser(s) = &mut self.mode {
            s.filter.clear();
            s.typing = false;
        }
    }

    fn project_filter_push(&mut self, c: char) {
        if let Mode::ProjectBrowser(s) = &mut self.mode {
            s.filter.push(c);
        }
        self.project_snap();
    }

    fn project_filter_pop(&mut self) {
        if let Mode::ProjectBrowser(s) = &mut self.mode {
            s.filter.pop();
        }
        self.project_snap();
    }

    /// After the filter changes, keep the selection on a visible project: if the
    /// current one no longer matches, jump to the first that does (#612).
    fn project_snap(&mut self) {
        let q = self.project_query();
        let Mode::ProjectBrowser(s) = &self.mode else {
            return;
        };
        let stays = s
            .projects
            .get(s.selected)
            .is_some_and(|p| project_matches(p, &q));
        if !stays {
            if let Some(first) = s.projects.iter().position(|p| project_matches(p, &q)) {
                self.set_project_selected(first);
            }
        }
    }

    /// The next (or previous, by `forward`) project that passes the filter, or the
    /// current selection unchanged when there's no further match that way (#612).
    fn project_step(&self, forward: bool) -> usize {
        let q = self.project_query();
        let Mode::ProjectBrowser(s) = &self.mode else {
            return 0;
        };
        let from = s.selected;
        let n = s.projects.len();
        let mut i = from;
        loop {
            if forward {
                if i + 1 >= n {
                    return from;
                }
                i += 1;
            } else {
                if i == 0 {
                    return from;
                }
                i -= 1;
            }
            if s.projects.get(i).is_some_and(|p| project_matches(p, &q)) {
                return i;
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
    pub(crate) fn browse_into(&mut self, dir: PathBuf) {
        match Self::dir_entries(&dir) {
            Ok(entries) => {
                self.mode = Mode::DirBrowser(Box::new(DirBrowser {
                    cwd: dir,
                    entries,
                    selected: 0,
                }));
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
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
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
    pub(crate) fn on_key_dir_browser(&mut self, key: KeyEvent) {
        let Mode::DirBrowser(state) = &mut self.mode else {
            return;
        };
        if let Some(s) =
            super::super::assist::list_move(state.selected, state.entries.len(), key.code)
        {
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

    /// Prompt for a new name for the project selected in the browser (#612),
    /// stashing its path so the rename knows which folder to move. The prompt is
    /// prefilled with the project's current folder name; renaming moves the folder
    /// and updates the book title together (#690). Renaming the project that's
    /// currently open is deferred to the in-place rename (#424), which also reopens
    /// it at the new path.
    fn prompt_rename_browser_project(&mut self) {
        let q = self.project_query();
        let Some(project) = self.selected_project().filter(|p| project_matches(p, &q)) else {
            return;
        };
        // The open project renames in place (and reopens itself) — route there so
        // the live editor follows the folder.
        if self.is_open_project(&project.path) {
            self.prompt_rename_project();
            return;
        }
        let prefill = project
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.pending_manage = Some(project.path);
        self.open_prompt(
            Pending::RenameBrowserProject,
            "Rename project (folder and book title): ",
            &prefill,
            None,
        );
    }

    /// Rename the stashed browser project to `new_name` (#612): move the folder,
    /// sync the book title to match (#690), record the new path in recents (the old
    /// entry self-prunes), and re-list the browser so the change shows without
    /// reopening the project. Refuses to clobber an existing folder and leaves
    /// everything untouched on any failure (no half-move).
    pub(crate) fn rename_browser_project(&mut self, new_name: &str) {
        let Some(old_root) = self.pending_manage.take() else {
            self.mode = self.editing_mode();
            return;
        };
        let name = crate::project::sanitize_dir_name(new_name);
        if name.is_empty() {
            self.message = Some("Enter a folder name.".into());
            self.reopen_project_browser();
            return;
        }
        let old_root = old_root.canonicalize().unwrap_or(old_root);
        let Some(new_root) = old_root.parent().map(|p| p.join(&name)) else {
            self.message = Some("Can't rename the filesystem root.".into());
            self.reopen_project_browser();
            return;
        };
        if new_root == old_root {
            self.message = Some(format!("Project is already named \u{201c}{name}\u{201d}."));
            self.reopen_project_browser();
            return;
        }
        if new_root.exists() {
            self.message = Some(format!(
                "A folder named \u{201c}{name}\u{201d} already exists here."
            ));
            self.reopen_project_browser();
            return;
        }
        if let Err(e) = std::fs::rename(&old_root, &new_root) {
            self.message = Some(format!("Couldn't rename project: {e}"));
            self.reopen_project_browser();
            return;
        }
        // Bring the book title into sync with the folder so the two don't diverge
        // (#690) — the report, exports, and the recents label all read the title.
        if let Err(e) = set_manifest_title(&new_root, &name) {
            // The folder moved but the title didn't; still record the new path so
            // the entry isn't lost, labeled with the new folder name.
            recent::record(&new_root, &name);
            self.message = Some(format!(
                "Renamed folder, but couldn't update the title: {e}"
            ));
            self.reopen_project_browser();
            return;
        }
        // Record the new path so it stays in recents (the old entry self-prunes —
        // its folder is gone), labeled with the freshly-synced title.
        recent::record(&new_root, &name);
        self.message = Some(format!("Renamed project to \u{201c}{name}\u{201d}."));
        self.reopen_project_browser();
    }

    /// Confirm deleting the project selected in the browser (#612). Refuses to
    /// delete the currently-open project (switch away first), and otherwise
    /// stashes the target and opens a y/N confirm before any files are touched.
    fn prompt_delete_browser_project(&mut self) {
        let q = self.project_query();
        let Some(project) = self.selected_project().filter(|p| project_matches(p, &q)) else {
            return;
        };
        if self.is_open_project(&project.path) {
            self.message =
                Some("That project is open — switch to another before deleting it.".into());
            return;
        }
        let label = project.path.file_name().map_or_else(
            || project.title.clone(),
            |n| n.to_string_lossy().into_owned(),
        );
        self.pending_manage = Some(project.path);
        self.open_prompt(
            Pending::ConfirmDeleteProject,
            format!("Delete project \u{201c}{label}\u{201d} and all its files? (y/N)"),
            "",
            None,
        );
    }

    /// Delete the stashed browser project's folder from disk (#612), then re-list
    /// the browser so it drops out. Guarded upstream (never the open project);
    /// leaves the list intact and reports on any failure. The old recents entry
    /// self-prunes once its folder is gone.
    pub(crate) fn delete_browser_project(&mut self) {
        let Some(root) = self.pending_manage.take() else {
            self.mode = self.editing_mode();
            return;
        };
        // A last guard: never remove the folder the live editor is writing to.
        if self.is_open_project(&root) {
            self.message =
                Some("That project is open — switch to another before deleting it.".into());
            self.reopen_project_browser();
            return;
        }
        match std::fs::remove_dir_all(&root) {
            Ok(()) => {
                let label = root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.message = Some(format!("Deleted project \u{201c}{label}\u{201d}."));
            }
            Err(e) => self.message = Some(format!("Couldn't delete project: {e}")),
        }
        self.reopen_project_browser();
    }

    /// Whether `path` is the project currently open in the editor, compared by
    /// canonical path so a relative or symlinked path still matches.
    fn is_open_project(&self, path: &Path) -> bool {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let open = self
            .project
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.project.root.clone());
        path == open
    }

    /// Re-open the project browser after a manage action changed the library, so
    /// the list reflects the rename / delete. Rebuilds from the freshly-pruned
    /// recents + home scan; the selection resets to the top.
    pub(crate) fn reopen_project_browser(&mut self) {
        self.open_project_browser();
    }
}

/// Whether project `p` passes the browser's active filter (#612): a
/// case-insensitive substring match on its title or path. `q` is the
/// already-lowercased query (empty = every project matches). Pure, and mirrors
/// the same test the UI applies when drawing the filtered list.
fn project_matches(p: &recent::RecentProject, q: &str) -> bool {
    q.is_empty()
        || p.title.to_lowercase().contains(q)
        || p.path.to_string_lossy().to_lowercase().contains(q)
}
