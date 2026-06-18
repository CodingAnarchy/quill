//! Top-level application state and the editing modes that drive key handling.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::buffer::Buffer;
use crate::frontmatter::{FrontMatter, Status};
use crate::manuscript::Manuscript;
use crate::project::Project;

/// Shown when the author tries to delete a project's last remaining chapter.
const ONLY_CHAPTER_MSG: &str = "Can't delete the only chapter";

/// Editor modes. Each variant carries the transient state that only makes sense
/// while in that mode, so illegal combinations aren't representable.
pub enum Mode {
    /// Normal prose entry. Typed characters go straight into the buffer.
    Insert,
    /// A transient prompt at the bottom: a y/N confirmation or a line of text
    /// input.
    Prompt(PromptState),
    /// Browsing the chapter list: arrows move the `selected` chapter, Enter
    /// opens it.
    Nav { selected: usize },
}

/// State for an open [`Mode::Prompt`]. `label`/`input` are public for the status
/// bar to render; the rest is the handler's business.
pub struct PromptState {
    /// What confirming the prompt does.
    pending: Pending,
    /// The prompt label shown in the status bar.
    pub label: String,
    /// Text typed so far (empty for y/N confirmations).
    pub input: String,
    /// Where to return when the prompt closes: `Some(selected)` back to the
    /// chapter browser, `None` to insert mode (e.g. the quit prompt).
    nav_return: Option<usize>,
}

/// What a [`Mode::Prompt`] resolves to when confirmed. `at` is the chapter index
/// the action applies to.
#[derive(Clone, Copy)]
enum Pending {
    /// y/N: quit despite unsaved changes.
    Quit,
    /// y/N: delete the chapter at `at`.
    Delete { at: usize },
    /// Text input: title for a new chapter to insert at `at`.
    NewChapter { at: usize },
    /// Text input: new title for the chapter at `at`.
    Rename { at: usize },
    /// Text input: one-line summary for the chapter at `at`.
    SetSummary { at: usize },
    /// Text input: POV character for the chapter at `at`.
    SetPov { at: usize },
}

pub struct App {
    pub buffer: Buffer,
    /// The open project (manifest, root, active chapter index). The `buffer`
    /// above holds the active chapter's text.
    pub project: Project,
    /// When `Some`, the continuous-manuscript view is active: editing and
    /// rendering go through this windowed multi-chapter buffer instead of the
    /// single-chapter `buffer` above (which is then the saved, idle chapter).
    pub manuscript: Option<Manuscript>,
    pub mode: Mode,
    pub should_quit: bool,
    /// Distraction-free centering: target column width for the text, in cells.
    pub focus_width: u16,
    /// Whether the chapter / outline pane is shown.
    pub outline_visible: bool,
    /// One-line transient message shown in the status bar.
    pub message: Option<String>,
}

impl App {
    pub fn new(buffer: Buffer, project: Project) -> Self {
        Self {
            buffer,
            project,
            manuscript: None,
            mode: Mode::Insert,
            should_quit: false,
            focus_width: 72,
            outline_visible: false,
            message: None,
        }
    }

    /// The buffer the author is currently editing: the continuous-view buffer
    /// when that view is active, otherwise the single active chapter. The
    /// renderer and movement go through this so both views share one path.
    pub fn edit_buffer(&self) -> &Buffer {
        match &self.manuscript {
            Some(m) => m.buffer(),
            None => &self.buffer,
        }
    }

    pub fn edit_buffer_mut(&mut self) -> &mut Buffer {
        match &mut self.manuscript {
            Some(m) => m.buffer_mut(),
            None => &mut self.buffer,
        }
    }

    /// Reparse the active edit buffer once input goes idle (called from the
    /// event loop). Routes to whichever buffer is being edited.
    pub fn refresh_syntax(&mut self) {
        self.edit_buffer_mut().refresh_syntax();
    }

    /// Whether the active edit target has unsaved changes (either view).
    fn edit_dirty(&self) -> bool {
        match &self.manuscript {
            Some(m) => m.dirty(),
            None => self.buffer.dirty,
        }
    }

    /// Word count for chapter `index`: live from the buffer when it's active
    /// (updates as you type), otherwise the count cached on the chapter entry.
    pub fn chapter_word_count(&self, index: usize) -> usize {
        if index == self.project.active_index() {
            self.buffer.word_count()
        } else {
            self.project.chapter_words(index)
        }
    }

    /// Total words across the whole project. (The nav pane sums its per-frame
    /// counts directly; this is the API the progress report (#4) will use.)
    #[allow(dead_code)]
    pub fn project_word_count(&self) -> usize {
        (0..self.chapter_count())
            .map(|i| self.chapter_word_count(i))
            .sum()
    }

    /// Handle a terminal event, dispatching key presses to the active mode.
    /// Non-key events and key releases/repeats are ignored.
    pub fn handle_event(&mut self, ev: Event) {
        if let Event::Key(key) = ev {
            if key.kind == KeyEventKind::Press {
                self.on_key(key);
            }
        }
    }

    /// Handle one key event, mutating state. Returns nothing; the caller
    /// re-renders afterward.
    pub fn on_key(&mut self, key: KeyEvent) {
        // A fresh keystroke clears any stale status message.
        self.message = None;

        // Non-binding patterns: dispatch on the variant without moving its data.
        match self.mode {
            Mode::Prompt(_) => self.on_key_prompt(key),
            Mode::Nav { .. } => self.on_key_nav(key),
            Mode::Insert => self.on_key_insert(key),
        }
    }

    /// Save the current chapter (if dirty), then load `index` as the active
    /// chapter. The active index and the live buffer always move together here,
    /// so the two never drift. A no-op if `index` is already active or invalid.
    /// Save the active chapter if it has unsaved edits and refresh its cached
    /// word count. Returns `Err(())` (and sets a status message) if the save
    /// failed, so callers can abort before switching away from it.
    fn flush_active(&mut self) -> Result<(), ()> {
        if self.buffer.dirty {
            if let Err(e) = self.buffer.save() {
                self.message = Some(format!("Save failed: {e}"));
                return Err(());
            }
        }
        // The chapter is now on disk; refresh its cached count.
        self.project.set_active_words(self.buffer.word_count());
        Ok(())
    }

    fn activate_chapter(&mut self, index: usize) {
        if index >= self.project.manifest.chapters.len() || index == self.project.active_index() {
            return;
        }
        // Flush the chapter we're leaving before loading the next.
        if self.flush_active().is_err() {
            return;
        }
        match Buffer::from_path(self.project.chapter_path(index)) {
            Ok(buffer) => {
                self.buffer = buffer;
                self.project.set_active(index);
                // No need to cache the new active chapter's count: while active,
                // chapter_word_count reads it live from the buffer.
                if let Some(title) = self.project.active_title() {
                    self.message = Some(format!("Chapter: {title}"));
                }
            }
            Err(e) => self.message = Some(format!("Open failed: {e}")),
        }
    }

    fn chapter_count(&self) -> usize {
        self.project.manifest.chapters.len()
    }

    // --- Continuous-manuscript view ---------------------------------------

    /// Toggle between the single-chapter view and the continuous view.
    fn toggle_continuous(&mut self) {
        if self.manuscript.is_some() {
            self.leave_continuous();
        } else {
            self.enter_continuous();
        }
    }

    /// Enter the continuous view: flush the single chapter, then open a window
    /// of chapters around it.
    fn enter_continuous(&mut self) {
        if self.flush_active().is_err() {
            return;
        }
        self.manuscript = Some(Manuscript::open(&self.project));
        self.message = Some("Continuous view".into());
    }

    /// Leave the continuous view: save its chapters, then reopen the single
    /// chapter the cursor was in. Stays in continuous view if the save fails, so
    /// no edits are lost.
    fn leave_continuous(&mut self) {
        let Some(mut m) = self.manuscript.take() else {
            return;
        };
        if let Err(e) = m.save(&mut self.project) {
            self.message = Some(format!("Save failed: {e}"));
            self.manuscript = Some(m);
            return;
        }
        self.project.set_active(m.current_chapter());
        match Buffer::from_path(self.project.active_path()) {
            Ok(buffer) => {
                self.buffer = buffer;
                self.message = Some("Chapter view".into());
            }
            Err(e) => self.message = Some(format!("Open failed: {e}")),
        }
    }

    // --- Edit dispatch (single chapter or continuous) ----------------------

    fn edit_insert_char(&mut self, c: char) {
        match &mut self.manuscript {
            Some(m) => m.insert_char(c),
            None => self.buffer.insert_char(c),
        }
    }

    fn edit_backspace(&mut self) {
        match &mut self.manuscript {
            Some(m) => m.backspace(),
            None => self.buffer.backspace(),
        }
    }

    /// Apply a cursor movement to the active edit buffer, then page the window
    /// if the continuous view's cursor crossed into another chapter.
    fn edit_move(&mut self, mv: impl FnOnce(&mut Buffer)) {
        mv(self.edit_buffer_mut());
        if let Some(mut m) = self.manuscript.take() {
            if let Err(e) = m.reslide(&mut self.project) {
                self.message = Some(format!("Save failed: {e}"));
            }
            self.manuscript = Some(m);
        }
    }

    /// Save the active edit target (the continuous window, or the single chapter).
    fn save_edit(&mut self) {
        if let Some(mut m) = self.manuscript.take() {
            let result = m.save(&mut self.project);
            self.manuscript = Some(m);
            self.message = Some(match result {
                Ok(()) => "Saved".into(),
                Err(e) => format!("Save failed: {e}"),
            });
        } else {
            match self.buffer.save() {
                Ok(()) => {
                    self.project.set_active_words(self.buffer.word_count());
                    self.message = Some("Saved".into());
                }
                Err(e) => self.message = Some(format!("Save failed: {e}")),
            }
        }
    }

    fn next_chapter(&mut self) {
        self.activate_chapter(self.project.active_index() + 1);
    }

    fn prev_chapter(&mut self) {
        // At chapter 0 this targets 0 — a no-op via activate_chapter's guard.
        self.activate_chapter(self.project.active_index().saturating_sub(1));
    }

    // --- Chapter structure editing ----------------------------------------
    //
    // Each wraps a `Project` op and returns the chapter the browser should now
    // select. The active chapter is tracked by file identity, so insert/rename/
    // move need no active fixup; only delete (of the active chapter) reloads.

    fn create_chapter(&mut self, at: usize, title: &str) -> usize {
        match self.project.add_chapter(at, title) {
            Ok(()) => self.message = Some(format!("Added '{title}'")),
            Err(e) => self.message = Some(format!("Add failed: {e}")),
        }
        at.min(self.chapter_count() - 1)
    }

    fn rename_chapter(&mut self, at: usize, title: &str) {
        match self.project.rename_chapter(at, title) {
            Ok(()) => self.message = Some(format!("Renamed to '{title}'")),
            Err(e) => self.message = Some(format!("Rename failed: {e}")),
        }
    }

    /// Move chapter `sel` up or down one slot; returns its new position (the
    /// active chapter follows its content automatically via identity tracking).
    fn move_chapter(&mut self, sel: usize, up: bool) -> usize {
        let target = if up {
            sel.checked_sub(1)
        } else {
            (sel + 1 < self.chapter_count()).then_some(sel + 1)
        };
        let Some(j) = target else { return sel };
        match self.project.swap_chapters(sel, j) {
            Ok(()) => j,
            Err(e) => {
                self.message = Some(format!("Move failed: {e}"));
                sel
            }
        }
    }

    /// Delete chapter `at`; returns the chapter the browser should select next.
    fn delete_chapter(&mut self, at: usize) -> usize {
        if self.chapter_count() <= 1 {
            self.message = Some(ONLY_CHAPTER_MSG.into());
            return at;
        }
        // Deleting the active chapter is the one structural edit that changes the
        // buffer's content — every other op keeps the active file present, so its
        // index just re-derives.
        let deleting_active = at == self.project.active_index();
        if deleting_active && self.buffer.dirty {
            // Persist the latest text first so the copy moved to unused/ is current.
            let _ = self.buffer.save();
        }
        if let Err(e) = self.project.remove_chapter(at) {
            self.message = Some(format!("Delete failed: {e}"));
            return at.min(self.chapter_count() - 1);
        }

        if deleting_active {
            // The active file is gone — point at a neighbor and reload.
            let neighbor = at.min(self.chapter_count() - 1);
            self.project.set_active(neighbor);
            match Buffer::from_path(self.project.active_path()) {
                Ok(buffer) => self.buffer = buffer,
                Err(e) => self.message = Some(format!("Open failed: {e}")),
            }
        }
        self.message = Some("Moved chapter to unused/".into());
        at.min(self.chapter_count() - 1)
    }

    // --- Chapter metadata (front matter) ----------------------------------

    /// Cycle the selected chapter's status (none → Draft → Revised → Final → …).
    fn cycle_status(&mut self, index: usize) {
        let next = match self.project.chapter_status(index) {
            None => Status::Draft,
            Some(s) => s.next(),
        };
        if self.edit_chapter_meta(index, |fm| fm.set_status(next)) {
            let title = self.project.chapter_title(index).unwrap_or("Chapter");
            self.message = Some(format!("{title}: {}", next.label()));
        }
    }

    /// Apply a front-matter edit to chapter `index`. The active chapter is edited
    /// through its live buffer (persisted on the next save, like prose); any other
    /// chapter is edited on disk immediately. Either way the pane cache is updated.
    /// Returns whether the edit succeeded.
    fn edit_chapter_meta(&mut self, index: usize, f: impl FnOnce(&mut FrontMatter)) -> bool {
        // Metadata is edited only from the chapter browser, which is always
        // single-chapter mode (Ctrl-G leaves the continuous view first). So the
        // active chapter's live front matter is `self.buffer`, never a manuscript
        // segment — if that ever changes, the active branch below must too.
        debug_assert!(
            self.manuscript.is_none(),
            "chapter metadata edited while the continuous view is active"
        );
        if index == self.project.active_index() {
            f(self.buffer.front_matter_mut());
            self.buffer.dirty = true;
            self.project.cache_chapter_meta(index, self.buffer.front_matter());
            true
        } else {
            match self.project.edit_chapter_front_matter(index, f) {
                Ok(()) => true,
                Err(e) => {
                    self.message = Some(format!("Save failed: {e}"));
                    false
                }
            }
        }
    }

    /// Open a prompt. `nav_return` is the chapter to reselect when it closes
    /// (`None` returns to insert mode, e.g. the quit prompt).
    fn open_prompt(
        &mut self,
        pending: Pending,
        label: impl Into<String>,
        prefill: &str,
        nav_return: Option<usize>,
    ) {
        self.mode = Mode::Prompt(PromptState {
            pending,
            label: label.into(),
            input: prefill.to_string(),
            nav_return,
        });
    }

    /// Enter (or stay in) the chapter browser with `selected` highlighted.
    fn select(&mut self, selected: usize) {
        self.mode = Mode::Nav { selected };
    }

    /// The prompt state, while in [`Mode::Prompt`]. Only call from a prompt key
    /// handler, which has already matched the variant.
    fn prompt_mut(&mut self) -> &mut PromptState {
        match &mut self.mode {
            Mode::Prompt(state) => state,
            _ => unreachable!("prompt editing runs only in Prompt mode"),
        }
    }

    fn on_key_nav(&mut self, key: KeyEvent) {
        let Mode::Nav { selected: sel } = self.mode else {
            return;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.select(sel.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => {
                let last = self.chapter_count() - 1;
                self.select((sel + 1).min(last));
            }
            KeyCode::Enter => {
                self.activate_chapter(sel);
                self.mode = Mode::Insert;
            }
            // Reorder the selected chapter.
            KeyCode::Char('K') => {
                let s = self.move_chapter(sel, true);
                self.select(s);
            }
            KeyCode::Char('J') => {
                let s = self.move_chapter(sel, false);
                self.select(s);
            }
            // Add / rename / delete (each returns to the browser when done).
            KeyCode::Char('a') => {
                self.open_prompt(Pending::NewChapter { at: sel + 1 }, "New chapter title: ", "", Some(sel))
            }
            KeyCode::Char('r') => {
                let title = self.project.manifest.chapters[sel].title.clone();
                self.open_prompt(Pending::Rename { at: sel }, "Rename chapter: ", &title, Some(sel));
            }
            // Metadata: cycle status, edit summary / POV (front matter).
            KeyCode::Char('s') => self.cycle_status(sel),
            KeyCode::Char('m') => {
                let prefill = self.project.chapter_summary(sel).unwrap_or("").to_string();
                self.open_prompt(Pending::SetSummary { at: sel }, "Summary: ", &prefill, Some(sel));
            }
            KeyCode::Char('v') => {
                let prefill = self.project.chapter_pov(sel).unwrap_or("").to_string();
                self.open_prompt(Pending::SetPov { at: sel }, "POV: ", &prefill, Some(sel));
            }
            KeyCode::Char('d') => {
                if self.chapter_count() <= 1 {
                    self.message = Some(ONLY_CHAPTER_MSG.into());
                } else {
                    let title = self.project.manifest.chapters[sel].title.clone();
                    self.open_prompt(
                        Pending::Delete { at: sel },
                        format!("Delete '{title}'? (y/N)"),
                        "",
                        Some(sel),
                    );
                }
            }
            KeyCode::Esc => self.mode = Mode::Insert,
            _ => {}
        }
    }

    fn on_key_prompt(&mut self, key: KeyEvent) {
        let Mode::Prompt(state) = &self.mode else {
            return;
        };
        let pending = state.pending;
        let nav_return = state.nav_return;

        // Where to go when the prompt closes without acting.
        let closed = match nav_return {
            Some(selected) => Mode::Nav { selected },
            None => Mode::Insert,
        };

        match pending {
            Pending::Quit => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.should_quit = true,
                _ => self.mode = closed,
            },
            Pending::Delete { at } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let selected = self.delete_chapter(at);
                    self.select(selected);
                }
                _ => self.mode = closed,
            },
            // The text prompts share the editing keys; only Enter differs.
            Pending::NewChapter { .. }
            | Pending::Rename { .. }
            | Pending::SetSummary { .. }
            | Pending::SetPov { .. } => {
                match key.code {
                    KeyCode::Char(c) => self.prompt_mut().input.push(c),
                    KeyCode::Backspace => {
                        self.prompt_mut().input.pop();
                    }
                    KeyCode::Enter => {
                        let text = std::mem::take(&mut self.prompt_mut().input);
                        let text = text.trim();
                        // All text prompts return to the chapter they acted on;
                        // a new chapter lands on the one just created.
                        let fallback = nav_return.unwrap_or(0);
                        let selected = match pending {
                            Pending::NewChapter { at } if !text.is_empty() => {
                                self.create_chapter(at, text)
                            }
                            Pending::Rename { at } if !text.is_empty() => {
                                self.rename_chapter(at, text);
                                fallback
                            }
                            Pending::SetSummary { at } => {
                                self.edit_chapter_meta(at, |fm| fm.set_summary(text));
                                fallback
                            }
                            Pending::SetPov { at } => {
                                self.edit_chapter_meta(at, |fm| fm.set_pov(text));
                                fallback
                            }
                            // Empty title for New/Rename: a no-op. (The summary
                            // and POV arms above run on empty text on purpose —
                            // that clears the field.)
                            _ => fallback,
                        };
                        self.select(selected);
                    }
                    KeyCode::Esc => self.mode = closed,
                    _ => {}
                }
            }
        }
    }

    fn on_key_insert(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match (ctrl, key.code) {
            (true, KeyCode::Char('s')) => self.save_edit(),
            (true, KeyCode::Char('q')) => {
                if self.edit_dirty() {
                    self.open_prompt(Pending::Quit, "Unsaved changes — quit anyway? (y/N)", "", None);
                } else {
                    self.should_quit = true;
                }
            }
            (true, KeyCode::Char('=') | KeyCode::Char('+')) => {
                self.focus_width = (self.focus_width + 4).min(160);
            }
            (true, KeyCode::Char('-')) => {
                self.focus_width = self.focus_width.saturating_sub(4).max(32);
            }
            (true, KeyCode::Char('o')) => {
                self.outline_visible = !self.outline_visible;
            }
            // Toggle the continuous-manuscript view. Ctrl-l ("long" view), not
            // Ctrl-m — terminals deliver Ctrl-m as Enter.
            (true, KeyCode::Char('l')) => self.toggle_continuous(),
            // Step to the next / previous chapter (single-chapter view only;
            // in continuous view you just scroll across boundaries).
            (true, KeyCode::Char('n')) if self.manuscript.is_none() => self.next_chapter(),
            (true, KeyCode::Char('p')) if self.manuscript.is_none() => self.prev_chapter(),
            // Focus the chapter pane to browse. Chapter structure is edited in
            // the single-chapter view, so leave continuous first.
            (true, KeyCode::Char('g')) => {
                self.leave_continuous();
                self.outline_visible = true;
                self.mode = Mode::Nav {
                    selected: self.project.active_index(),
                };
            }

            (false, KeyCode::Char(c)) => self.edit_insert_char(c),
            (false, KeyCode::Enter) => self.edit_insert_char('\n'),
            (false, KeyCode::Backspace) => self.edit_backspace(),
            (false, KeyCode::Tab) => {
                for _ in 0..4 {
                    self.edit_insert_char(' ');
                }
            }

            (false, KeyCode::Left) => self.edit_move(Buffer::move_left),
            (false, KeyCode::Right) => self.edit_move(Buffer::move_right),
            (false, KeyCode::Up) => self.edit_move(Buffer::move_up),
            (false, KeyCode::Down) => self.edit_move(Buffer::move_down),
            (false, KeyCode::Home) => self.edit_move(Buffer::move_home),
            (false, KeyCode::End) => self.edit_move(Buffer::move_end),

            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;
    use std::path::PathBuf;

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn nav_sel(app: &App) -> usize {
        match &app.mode {
            Mode::Nav { selected } => *selected,
            _ => panic!("expected Nav mode"),
        }
    }

    fn prompt_input(app: &App) -> &str {
        match &app.mode {
            Mode::Prompt(state) => state.input.as_str(),
            _ => panic!("expected Prompt mode"),
        }
    }

    /// A two-chapter project on disk with distinct content per chapter.
    fn two_chapter_app(name: &str) -> (App, PathBuf) {
        let mut dir = std::env::temp_dir();
        dir.push(format!("quill-app-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            Project::manifest_path(&dir),
            "title = \"Book\"\n\n\
             [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
             [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("one.md"), "first chapter\n").unwrap();
        std::fs::write(dir.join("two.md"), "second chapter\n").unwrap();

        let project = Project::open(&dir).unwrap();
        let buffer = Buffer::from_path(project.active_path()).unwrap();
        (App::new(buffer, project), dir)
    }

    #[test]
    fn ctrl_n_and_ctrl_p_switch_chapters() {
        let (mut app, dir) = two_chapter_app("nav");
        assert_eq!(app.project.active_index(), 0);
        assert!(app.buffer.line(0).contains("first"));

        app.on_key(ctrl('n'));
        assert_eq!(app.project.active_index(), 1);
        assert!(app.buffer.line(0).contains("second"));

        app.on_key(ctrl('p'));
        assert_eq!(app.project.active_index(), 0);
        assert!(app.buffer.line(0).contains("first"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn next_previous_clamp_at_the_ends() {
        let (mut app, dir) = two_chapter_app("clamp");
        app.on_key(ctrl('p')); // already at first — no-op
        assert_eq!(app.project.active_index(), 0);

        app.on_key(ctrl('n'));
        app.on_key(ctrl('n')); // already at last — no-op
        assert_eq!(app.project.active_index(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn switching_chapters_saves_pending_edits() {
        let (mut app, dir) = two_chapter_app("autosave");
        // Edit chapter one, then jump away without an explicit save.
        app.on_key(key(KeyCode::Char('X')));
        assert!(app.buffer.dirty);

        app.on_key(ctrl('n'));
        assert_eq!(app.project.active_index(), 1);

        // Chapter one was persisted on the way out.
        let saved = std::fs::read_to_string(dir.join("one.md")).unwrap();
        assert!(saved.starts_with('X'), "expected edit saved, got: {saved:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nav_mode_selects_and_opens_a_chapter() {
        let (mut app, dir) = two_chapter_app("navmode");
        app.on_key(ctrl('g')); // focus the chapter pane
        assert!(matches!(app.mode, Mode::Nav { .. }));
        assert_eq!(nav_sel(&app), 0);

        app.on_key(key(KeyCode::Down)); // select chapter two
        assert_eq!(nav_sel(&app), 1);

        app.on_key(key(KeyCode::Enter)); // open it
        assert!(matches!(app.mode, Mode::Insert));
        assert_eq!(app.project.active_index(), 1);
        assert!(app.buffer.line(0).contains("second"));

        std::fs::remove_dir_all(&dir).ok();
    }

    fn typed(app: &mut App, s: &str) {
        for c in s.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }

    fn project_titles(project: &Project) -> Vec<String> {
        project
            .manifest
            .chapters
            .iter()
            .map(|c| c.title.clone())
            .collect()
    }

    fn titles(app: &App) -> Vec<String> {
        project_titles(&app.project)
    }

    #[test]
    fn create_chapter_inserts_after_the_selection() {
        let (mut app, dir) = two_chapter_app("create");
        app.on_key(ctrl('g')); // Nav, selection on chapter 0
        app.on_key(key(KeyCode::Char('a'))); // add after selection
        assert!(matches!(app.mode, Mode::Prompt(_)));
        typed(&mut app, "Interlude");
        app.on_key(key(KeyCode::Enter));

        assert_eq!(titles(&app), vec!["One", "Interlude", "Two"]);
        assert!(matches!(app.mode, Mode::Nav { .. }));
        // Active chapter (was 0) is unchanged; insertion was after it.
        assert_eq!(app.project.active_index(), 0);
        // Persisted.
        assert_eq!(project_titles(&Project::open(&dir).unwrap()), titles(&app));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rename_chapter_edits_the_prefilled_title() {
        let (mut app, dir) = two_chapter_app("rename");
        app.on_key(ctrl('g'));
        app.on_key(key(KeyCode::Char('r'))); // prefilled with "One"
        assert_eq!(prompt_input(&app), "One");
        for _ in 0..3 {
            app.on_key(key(KeyCode::Backspace));
        }
        typed(&mut app, "Uno");
        app.on_key(key(KeyCode::Enter));

        assert_eq!(titles(&app)[0], "Uno");
        assert_eq!(project_titles(&Project::open(&dir).unwrap())[0], "Uno");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn move_selected_reorders_and_active_follows() {
        let (mut app, dir) = two_chapter_app("move");
        app.on_key(ctrl('g')); // selection 0 == active 0
        app.on_key(key(KeyCode::Char('J'))); // move chapter 0 down

        assert_eq!(titles(&app), vec!["Two", "One"]);
        assert_eq!(nav_sel(&app), 1);
        // The active chapter ("One") followed its content to index 1.
        assert_eq!(app.project.active_title(), Some("One"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_moves_file_to_unused_and_keeps_active_consistent() {
        let (mut app, dir) = two_chapter_app("delete");
        app.on_key(ctrl('g'));
        app.on_key(key(KeyCode::Down)); // select chapter 1 ("Two")
        app.on_key(key(KeyCode::Char('d')));
        app.on_key(key(KeyCode::Char('y'))); // confirm

        assert_eq!(titles(&app), vec!["One"]);
        assert!(dir.join("unused").join("two.md").exists());
        // Active was chapter 0; untouched by deleting a later chapter.
        assert_eq!(app.project.active_index(), 0);
        assert!(app.buffer.line(0).contains("first"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn deleting_the_active_chapter_reloads_a_neighbor() {
        let (mut app, dir) = two_chapter_app("delete-active");
        app.on_key(ctrl('g')); // selection 0 == active 0 ("One")
        app.on_key(key(KeyCode::Char('d')));
        app.on_key(key(KeyCode::Char('y')));

        assert_eq!(titles(&app), vec!["Two"]);
        assert_eq!(app.project.active_index(), 0);
        // Buffer was reloaded to the surviving chapter.
        assert!(app.buffer.line(0).contains("second"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cannot_delete_the_only_chapter() {
        let (mut app, dir) = two_chapter_app("last");
        // Remove one chapter, leaving a single chapter.
        app.on_key(ctrl('g'));
        app.on_key(key(KeyCode::Char('d')));
        app.on_key(key(KeyCode::Char('y')));
        assert_eq!(app.chapter_count(), 1);

        // Attempt to delete the last one — refused.
        app.on_key(key(KeyCode::Char('d')));
        assert!(matches!(app.mode, Mode::Nav { .. })); // no confirm prompt opened
        assert_eq!(app.chapter_count(), 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn active_chapter_survives_insert_before_it() {
        let (mut app, dir) = two_chapter_app("identity");
        // Active is chapter 0 ("One"). Insert a chapter ahead of it.
        app.create_chapter(0, "Prologue");

        // Tracked by identity: "One" is now at index 1 and still active, with
        // its buffer untouched (no reload, no index arithmetic).
        assert_eq!(app.project.active_index(), 1);
        assert_eq!(app.project.active_title(), Some("One"));
        assert!(app.buffer.line(0).contains("first"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cycling_status_on_the_active_chapter_persists_to_front_matter() {
        let (mut app, dir) = two_chapter_app("status-active");
        app.on_key(ctrl('g')); // Nav, selection on chapter 0 (the active one)
        app.on_key(key(KeyCode::Char('s'))); // none → Draft
        assert_eq!(app.project.chapter_status(0), Some(crate::frontmatter::Status::Draft));
        app.on_key(key(KeyCode::Char('s'))); // Draft → Revised
        assert_eq!(app.project.chapter_status(0), Some(crate::frontmatter::Status::Revised));

        // Active-chapter metadata persists on save (like prose).
        app.on_key(key(KeyCode::Enter)); // open the selected chapter (back to Insert)
        app.on_key(ctrl('s'));
        let on_disk = std::fs::read_to_string(dir.join("one.md")).unwrap();
        assert!(on_disk.starts_with("---\nstatus: revised\n---\n"), "got:\n{on_disk}");
        assert!(on_disk.contains("first chapter"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn editing_a_non_active_chapters_summary_writes_immediately() {
        let (mut app, dir) = two_chapter_app("summary-nonactive");
        app.on_key(ctrl('g'));
        app.on_key(key(KeyCode::Down)); // select chapter 1 (not active)
        app.on_key(key(KeyCode::Char('m'))); // summary prompt
        typed(&mut app, "The reckoning: part one");
        app.on_key(key(KeyCode::Enter));

        // Non-active edits hit disk right away, with quoting, prose untouched.
        let on_disk = std::fs::read_to_string(dir.join("two.md")).unwrap();
        assert!(on_disk.starts_with("---\nsummary: \"The reckoning: part one\"\n---\n"), "got:\n{on_disk}");
        assert!(on_disk.contains("second chapter"));
        assert_eq!(app.project.chapter_summary(1), Some("The reckoning: part one"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn metadata_survives_reopening_the_project() {
        let (mut app, dir) = two_chapter_app("meta-reopen");
        app.on_key(ctrl('g'));
        app.on_key(key(KeyCode::Down)); // chapter 1
        app.on_key(key(KeyCode::Char('s'))); // Draft
        app.on_key(key(KeyCode::Char('v'))); // POV prompt
        typed(&mut app, "Mara");
        app.on_key(key(KeyCode::Enter));

        // Reopen from disk: the cached metadata is read back from front matter.
        let reopened = Project::open(&dir).unwrap();
        assert_eq!(reopened.chapter_status(1), Some(crate::frontmatter::Status::Draft));
        assert_eq!(reopened.chapter_pov(1), Some("Mara"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn continuous_view_edits_persist_to_the_right_chapter() {
        let (mut app, dir) = two_chapter_app("continuous"); // "first chapter", "second chapter"
        app.on_key(ctrl('l')); // enter continuous view
        assert!(app.manuscript.is_some());

        // Move into chapter two and append to it.
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::End));
        typed(&mut app, "Z");

        app.on_key(ctrl('l')); // leave continuous — saves on the way out
        assert!(app.manuscript.is_none());

        // The edit landed in chapter two's file; chapter one is untouched.
        assert_eq!(std::fs::read_to_string(dir.join("two.md")).unwrap(), "second chapterZ\n");
        assert_eq!(std::fs::read_to_string(dir.join("one.md")).unwrap(), "first chapter\n");
        // Back in single-chapter view at the chapter the cursor was in.
        assert_eq!(app.project.active_index(), 1);
        assert!(app.buffer.line(0).contains("second"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn entering_continuous_flushes_the_single_chapter() {
        let (mut app, dir) = two_chapter_app("continuous-flush");
        // Edit chapter one in single view, then enter continuous without saving.
        app.on_key(key(KeyCode::Char('X')));
        assert!(app.buffer.dirty);

        app.on_key(ctrl('l')); // entering must flush the dirty single chapter
        assert!(std::fs::read_to_string(dir.join("one.md")).unwrap().starts_with('X'));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn word_counts_roll_up_and_update_live() {
        let (mut app, dir) = two_chapter_app("counts"); // "first chapter", "second chapter"
        assert_eq!(app.chapter_word_count(0), 2); // active, live from buffer
        assert_eq!(app.chapter_word_count(1), 2); // cached from file
        assert_eq!(app.project_word_count(), 4);

        // Add a word to the active chapter; its count and the total grow live.
        app.on_key(key(KeyCode::Char('X')));
        app.on_key(key(KeyCode::Char(' ')));
        assert_eq!(app.chapter_word_count(0), 3);
        assert_eq!(app.project_word_count(), 5);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn word_count_cache_tracks_add_and_delete() {
        let (mut app, dir) = two_chapter_app("counts-structure");
        app.on_key(ctrl('g'));
        app.on_key(key(KeyCode::Char('a'))); // add an empty chapter after ch.0
        typed(&mut app, "New");
        app.on_key(key(KeyCode::Enter));

        assert_eq!(app.chapter_count(), 3);
        assert_eq!(app.project_word_count(), 4); // the new chapter is empty

        // Delete it (the new chapter is the current selection).
        app.on_key(key(KeyCode::Char('d')));
        app.on_key(key(KeyCode::Char('y')));
        assert_eq!(app.chapter_count(), 2);
        assert_eq!(app.project_word_count(), 4);

        std::fs::remove_dir_all(&dir).ok();
    }
}
