//! Project switching and directory browsing: the project/dir browsers and the
//! new-project flow. Extracted from `mod.rs` (#314).

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};

use crate::buffer::Buffer;
use crate::config::ProjectsConfig;
use crate::mode::{
    Command, DirBrowser, DirEntry, Mode, Palette, PaletteItem, Pending, ProjectBrowser,
};
use crate::project::{Project, ProjectTemplate, TEMPLATES};
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
                PaletteItem::new(
                    label,
                    format!("book {} of {count}", i + 1),
                    Command::SwitchBook(i),
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Switch book",
            items,
            selected: active,
            collapsed: Default::default(),
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
        let title = self.project.book_title(index).unwrap_or("").to_string();
        if self.reload_active_book() {
            self.message = Some(format!("Book: {title}"));
        }
    }

    /// Reload the editor onto the active book's first chapter after the project's
    /// active book changed (#329/#648): open the buffer and re-baseline the daily
    /// tally for the new book's structure. Returns whether the reload landed (a
    /// buffer-open failure sets a message and returns false). Shared by switching
    /// books and the series mutators (add / remove a book).
    fn reload_active_book(&mut self) -> bool {
        match Buffer::from_path(self.project.active_path()) {
            Ok(buffer) => {
                self.buffer = buffer;
                // The new book has its own parts and chapters: re-baseline so its
                // existing words aren't counted as today's writing and the per-part
                // counters match its structure (#329).
                self.reset_tally_for_active_book();
                true
            }
            Err(e) => {
                self.message = Some(format!("Open failed: {e}"));
                false
            }
        }
    }

    /// Prompt for a series title to convert this standalone novel into a series
    /// (#648). The current book becomes book one; a blank title uses the book's own
    /// title as the series name. A no-op (with a message) if it's already a series.
    pub(crate) fn start_convert_to_series(&mut self) {
        if self.project.book_count() > 1 {
            self.message = Some("This project is already a series.".into());
            return;
        }
        let prefill = self.project.manifest.series.clone().unwrap_or_default();
        self.open_prompt(
            Pending::ConvertToSeries,
            "Series title (blank uses the book title): ",
            &prefill,
            None,
        );
    }

    /// Convert the standalone novel into a series titled `series_title` (#648),
    /// then offer to add a second book straight away — turning one book into a
    /// series is only useful alongside another. A blank title falls back to the
    /// current book's title.
    pub(crate) fn convert_to_series(&mut self, series_title: &str) {
        let title = series_title.trim();
        let title = if title.is_empty() {
            self.project.manifest.title.clone()
        } else {
            title.to_string()
        };
        match self.project.convert_to_series(&title) {
            Ok(()) => {
                self.message = Some(format!("Started series \u{201c}{title}\u{201d}."));
                // A one-book series is only useful with a second book; chain to it.
                self.prompt_add_book();
            }
            Err(e) => {
                self.message = Some(format!("Couldn't start series: {e}"));
                self.mode = self.editing_mode();
            }
        }
    }

    /// Prompt for the title of a new book to add to the series (#648). A standalone
    /// novel isn't a series yet, so it routes to the convert-to-series flow first
    /// (which chains back here once the series exists). Note a freshly-converted
    /// one-book series *is* a series, so this gates on series membership, not the
    /// book count.
    pub(crate) fn prompt_add_book(&mut self) {
        if self.project.series_title().is_none() {
            self.start_convert_to_series();
            return;
        }
        let n = self.project.book_count() + 1;
        self.open_prompt(
            Pending::AddBook,
            "New book title: ",
            &format!("Book {n}"),
            None,
        );
    }

    /// Add a new book titled `title` to the series and switch to it (#648): flush
    /// the outgoing chapter, create the book with a starter chapter, and reload the
    /// editor on it. A blank title is rejected (re-prompts).
    pub(crate) fn add_book(&mut self, title: &str) {
        let title = title.trim();
        if title.is_empty() {
            self.message = Some("Enter a book title.".into());
            self.prompt_add_book();
            return;
        }
        // The continuous view windows one book; drop to single-chapter, saving it,
        // and persist the chapter we're leaving before swapping books.
        self.leave_continuous();
        if self.flush_active().is_err() {
            return;
        }
        match self.project.add_book(title) {
            Ok(_) => {
                if self.reload_active_book() {
                    self.message = Some(format!("Added book \u{201c}{title}\u{201d}."));
                }
                self.mode = self.editing_mode();
            }
            Err(e) => {
                self.message = Some(format!("Couldn't add book: {e}"));
                self.mode = self.editing_mode();
            }
        }
    }

    /// Open the book-management palette (#648): each book with a remove and a
    /// move-up / move-down entry, plus "Add a book…". The single place an author
    /// reorders or removes books. A no-op (with a message) for a standalone novel —
    /// there's nothing to manage until it's a series (use "Add a book…" to start).
    pub(crate) fn open_manage_books(&mut self) {
        if self.project.book_count() <= 1 {
            // Nothing to reorder/remove yet; offer to start a series instead.
            self.prompt_add_book();
            return;
        }
        let count = self.project.book_count();
        let active = self.project.active_book();
        let mut items = Vec::new();
        for i in 0..count {
            let title = self
                .project
                .book_title(i)
                .unwrap_or("(untitled)")
                .to_string();
            let marker = if i == active { "  ·  current" } else { "" };
            items.push(PaletteItem::new(
                format!("{title}{marker}"),
                format!("switch to book {}", i + 1),
                Command::SwitchBook(i),
            ));
            if i > 0 {
                items.push(PaletteItem::new(
                    format!("  ↑ move \u{201c}{title}\u{201d} up"),
                    "reorder earlier in the series".to_string(),
                    Command::MoveBook { index: i, up: true },
                ));
            }
            if i + 1 < count {
                items.push(PaletteItem::new(
                    format!("  ↓ move \u{201c}{title}\u{201d} down"),
                    "reorder later in the series".to_string(),
                    Command::MoveBook {
                        index: i,
                        up: false,
                    },
                ));
            }
            items.push(PaletteItem::new(
                format!("  ✕ remove \u{201c}{title}\u{201d}"),
                "retire its chapters to unused/ (confirmed)".to_string(),
                Command::RemoveBook(i),
            ));
        }
        items.push(PaletteItem::new(
            "Add a book…".to_string(),
            "append a new book to the series".to_string(),
            Command::AddBook,
        ));
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Manage books",
            items,
            selected: 0,
            collapsed: Default::default(),
        }));
    }

    /// Confirm removing book `index` before touching any files (#648): its chapters
    /// are retired to `unused/`, not deleted, but it's still a structural change, so
    /// it takes an explicit y/N. Guarded — a series always keeps at least one book.
    pub(crate) fn confirm_remove_book(&mut self, index: usize) {
        if self.project.book_count() <= 1 {
            self.message = Some("A series must keep at least one book.".into());
            return;
        }
        let title = self.project.book_title(index).unwrap_or("this book");
        self.open_prompt(
            Pending::ConfirmRemoveBook { index },
            format!("Remove book \u{201c}{title}\u{201d}? Its chapters move to unused/ (y/N)"),
            "",
            None,
        );
    }

    /// Remove book `index` from the series (#648): retire its chapter files, and —
    /// if the active book changed — reload the editor onto the new active book.
    /// Flushes the outgoing chapter first so no work is lost.
    pub(crate) fn remove_book(&mut self, index: usize) {
        self.leave_continuous();
        if self.flush_active().is_err() {
            return;
        }
        match self.project.remove_book(index) {
            Ok(active_changed) => {
                if !active_changed || self.reload_active_book() {
                    self.message = Some("Book removed — its chapters are in unused/.".into());
                }
            }
            Err(e) => self.message = Some(format!("Couldn't remove book: {e}")),
        }
        self.mode = self.editing_mode();
    }

    /// Move book `index` up or down in the series order (#648), then re-open the
    /// manage-books palette so the new order is visible and more moves are one
    /// keystroke away. Reordering never moves files or changes the active book.
    pub(crate) fn move_book(&mut self, index: usize, up: bool) {
        match self.project.move_book(index, up) {
            Ok(true) => self.message = Some("Reordered books.".into()),
            Ok(false) => {}
            Err(e) => self.message = Some(format!("Couldn't reorder books: {e}")),
        }
        self.open_manage_books();
    }

    /// Open the project browser over the recently-opened projects (newest first).
    /// Empty until at least one project has been opened.
    pub(super) fn open_project_browser(&mut self) {
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
    pub(super) fn on_key_project_browser(&mut self, key: KeyEvent) {
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
        match key.code {
            KeyCode::Esc => self.set_project_typing(false),
            KeyCode::Enter => {
                if let Some(target) = self.selected_project_path() {
                    self.request_switch(target);
                }
            }
            KeyCode::Up => {
                let s = self.project_step(false);
                self.set_project_selected(s);
            }
            KeyCode::Down => {
                let s = self.project_step(true);
                self.set_project_selected(s);
            }
            KeyCode::Backspace => self.project_filter_pop(),
            KeyCode::Char(c) => self.project_filter_push(c),
            _ => {}
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

    /// Offer a structure template for the new project at `target` (#611), then
    /// create it. Refuses to clobber an existing project up front (so a bad path
    /// reports before the picker), stashes the target, and opens a small picker of
    /// the curated [`TEMPLATES`]; the selection routes to
    /// [`Self::create_project_with_template`].
    pub(super) fn create_project_at(&mut self, target: PathBuf) {
        if Project::manifest_path(&target).exists() {
            self.message = Some(format!("A project already exists at {}", target.display()));
            self.mode = self.editing_mode();
            return;
        }
        self.pending_manage = Some(target);
        let items = TEMPLATES
            .iter()
            .enumerate()
            .map(|(i, t)| PaletteItem::new(t.name, t.desc, Command::NewProjectTemplate(i)))
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "New project — structure",
            items,
            selected: 0,
            collapsed: Default::default(),
        }));
    }

    /// Create the stashed new project (#611) with the template at `index`, then
    /// open it in place. Offers the setup wizard only when the project was
    /// actually created and opened (#282), and the wizard isn't disabled.
    pub(super) fn create_project_with_template(&mut self, index: usize) {
        let Some(target) = self.pending_manage.take() else {
            self.mode = self.editing_mode();
            return;
        };
        let template = ProjectTemplate::at(index);
        if self.create_and_open_with_template(&target, template)
            && ProjectsConfig::load().wizard_enabled()
        {
            self.start_wizard();
        }
    }

    /// Create a new empty project at `target` and switch to it in place. Returns
    /// whether the switch landed (false if a project already exists there,
    /// creation fails, or a save aborts the switch), with a clear message set on
    /// failure. Shared by "new project from notes" (#286) and manuscript import
    /// (#298), which scaffold their own content and so want the empty default.
    pub(crate) fn create_and_open(&mut self, target: &Path) -> bool {
        self.create_and_open_with_template(target, ProjectTemplate::at(0))
    }

    /// Create a new project at `target` scaffolded from `template` (#611) and
    /// switch to it in place. Returns whether the switch landed (false if a
    /// project already exists there, creation fails, or a save aborts the switch),
    /// with a clear message set on failure.
    fn create_and_open_with_template(&mut self, target: &Path, template: &ProjectTemplate) -> bool {
        if Project::manifest_path(target).exists() {
            self.message = Some(format!("A project already exists at {}", target.display()));
            self.mode = self.editing_mode();
            return false;
        }
        match Project::create_with_template(target, template) {
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

    /// Prompt for the open project's new name (#424), prefilled with the book
    /// title. Renaming moves the folder and updates the book title together
    /// (#690), so the folder and the title stay in sync.
    pub(crate) fn prompt_rename_project(&mut self) {
        let prefill = self.project.manifest.title.clone();
        self.open_prompt(
            Pending::RenameProject,
            "Rename project (folder and book title): ",
            &prefill,
            None,
        );
    }

    /// Rename the open project to `new_name`, in place: save the current work,
    /// move the folder, sync the book title to match (#690), and reopen the project
    /// at the new path so every save from here lands there (#424). Refuses to
    /// clobber an existing folder, and leaves the project untouched on any failure
    /// (no half-move).
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
        // The folder moved; now bring the book title into sync so the folder and
        // the title (report, exports, recents) don't diverge (#690). Write it onto
        // the manifest at the new location before reopening, since open_in_place
        // adopts whatever the manifest on disk says.
        if let Err(e) = set_manifest_title(&new_root, &name) {
            // The folder is at the new path but the title didn't update — report,
            // then still reopen at the new path so the editor follows the folder.
            self.message = Some(format!(
                "Renamed folder, but couldn't update the title: {e}"
            ));
            self.open_in_place(new_root.clone());
            return;
        }
        // Reopen at the new path (the old recents entry self-prunes — its folder is
        // gone). open_in_place sets an "Opened …" message; replace it with the
        // rename confirmation.
        self.open_in_place(new_root.clone());
        if self.project.root == new_root {
            self.message = Some(format!("Renamed project to \u{201c}{name}\u{201d}."));
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
        let label = project
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| project.title.clone());
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
    pub(super) fn reopen_project_browser(&mut self) {
        self.open_project_browser();
    }
}

/// Set the book title in the manifest of the project at `root` to `title` and
/// write it back (#690), so renaming a project keeps its folder and its title in
/// sync. Loads the manifest first, so a multi-book series (`[[books]]`) round-trips
/// through the series form rather than being flattened. `title` is the sanitized
/// folder name, keeping the two identical.
fn set_manifest_title(root: &Path, title: &str) -> std::io::Result<()> {
    let mut project = Project::open(root)?;
    project.manifest.title = title.to_string();
    project.save_manifest()
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
