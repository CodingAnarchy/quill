//! Top-level application state and the editing modes that drive key handling.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::buffer::Buffer;
use crate::project::Project;

/// Editor modes. Quill is mostly modeless for writing, but borrows a light
/// command line for actions that shouldn't be a stray keystroke away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Normal prose entry. Typed characters go straight into the buffer.
    Insert,
    /// A transient prompt at the bottom (e.g. confirm quit with unsaved changes).
    Prompt,
    /// Browsing the chapter list: arrows move a selection, Enter opens it.
    Nav,
}

pub struct App {
    pub buffer: Buffer,
    /// The open project (manifest, root, active chapter index). The `buffer`
    /// above holds the active chapter's text.
    pub project: Project,
    pub mode: Mode,
    pub should_quit: bool,
    /// Distraction-free centering: target column width for the text, in cells.
    pub focus_width: u16,
    /// Whether the chapter / outline pane is shown.
    pub outline_visible: bool,
    /// The chapter highlighted in the pane while in [`Mode::Nav`].
    pub nav_selected: usize,
    /// One-line transient message shown in the status bar.
    pub message: Option<String>,
    /// Text of the active prompt, if any.
    pub prompt: Option<String>,
}

impl App {
    pub fn new(buffer: Buffer, project: Project) -> Self {
        Self {
            buffer,
            project,
            mode: Mode::Insert,
            should_quit: false,
            focus_width: 72,
            outline_visible: false,
            nav_selected: 0,
            message: None,
            prompt: None,
        }
    }

    /// Handle one key event, mutating state. Returns nothing; the caller
    /// re-renders afterward.
    pub fn on_key(&mut self, key: KeyEvent) {
        // A fresh keystroke clears any stale status message.
        self.message = None;

        match self.mode {
            Mode::Prompt => self.on_key_prompt(key),
            Mode::Nav => self.on_key_nav(key),
            Mode::Insert => self.on_key_insert(key),
        }
    }

    /// Save the current chapter (if dirty), then load `index` as the active
    /// chapter. The active index and the live buffer always move together here,
    /// so the two never drift. A no-op if `index` is already active or invalid.
    fn activate_chapter(&mut self, index: usize) {
        if index >= self.project.manifest.chapters.len() || index == self.project.active {
            return;
        }
        if self.buffer.dirty {
            if let Err(e) = self.buffer.save() {
                self.message = Some(format!("Save failed: {e}"));
                return;
            }
        }
        match Buffer::from_path(self.project.chapter_path(index)) {
            Ok(buffer) => {
                self.buffer = buffer;
                self.project.active = index;
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

    fn next_chapter(&mut self) {
        self.activate_chapter(self.project.active + 1);
    }

    fn prev_chapter(&mut self) {
        // At chapter 0 this targets 0 — a no-op via activate_chapter's guard.
        self.activate_chapter(self.project.active.saturating_sub(1));
    }

    fn on_key_nav(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.nav_selected = self.nav_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.nav_selected = (self.nav_selected + 1).min(self.chapter_count() - 1);
            }
            KeyCode::Enter => {
                self.activate_chapter(self.nav_selected);
                self.mode = Mode::Insert;
            }
            KeyCode::Esc => self.mode = Mode::Insert,
            _ => {}
        }
    }

    fn on_key_prompt(&mut self, key: KeyEvent) {
        match key.code {
            // Confirm quit-without-saving.
            KeyCode::Char('y') | KeyCode::Char('Y') => self.should_quit = true,
            _ => {
                self.mode = Mode::Insert;
                self.prompt = None;
            }
        }
    }

    fn on_key_insert(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match (ctrl, key.code) {
            (true, KeyCode::Char('s')) => match self.buffer.save() {
                Ok(()) => self.message = Some("Saved".into()),
                Err(e) => self.message = Some(format!("Save failed: {e}")),
            },
            (true, KeyCode::Char('q')) => {
                if self.buffer.dirty {
                    self.mode = Mode::Prompt;
                    self.prompt = Some("Unsaved changes — quit anyway? (y/N)".into());
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
            // Step to the next / previous chapter.
            (true, KeyCode::Char('n')) => self.next_chapter(),
            (true, KeyCode::Char('p')) => self.prev_chapter(),
            // Focus the chapter pane to browse and jump to one.
            (true, KeyCode::Char('g')) => {
                self.outline_visible = true;
                self.nav_selected = self.project.active;
                self.mode = Mode::Nav;
            }

            (false, KeyCode::Char(c)) => self.buffer.insert_char(c),
            (false, KeyCode::Enter) => self.buffer.insert_newline(),
            (false, KeyCode::Backspace) => self.buffer.backspace(),
            (false, KeyCode::Tab) => {
                for _ in 0..4 {
                    self.buffer.insert_char(' ');
                }
            }

            (false, KeyCode::Left) => self.buffer.move_left(),
            (false, KeyCode::Right) => self.buffer.move_right(),
            (false, KeyCode::Up) => self.buffer.move_up(),
            (false, KeyCode::Down) => self.buffer.move_down(),
            (false, KeyCode::Home) => self.buffer.move_home(),
            (false, KeyCode::End) => self.buffer.move_end(),

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
        assert_eq!(app.project.active, 0);
        assert!(app.buffer.line(0).contains("first"));

        app.on_key(ctrl('n'));
        assert_eq!(app.project.active, 1);
        assert!(app.buffer.line(0).contains("second"));

        app.on_key(ctrl('p'));
        assert_eq!(app.project.active, 0);
        assert!(app.buffer.line(0).contains("first"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn next_previous_clamp_at_the_ends() {
        let (mut app, dir) = two_chapter_app("clamp");
        app.on_key(ctrl('p')); // already at first — no-op
        assert_eq!(app.project.active, 0);

        app.on_key(ctrl('n'));
        app.on_key(ctrl('n')); // already at last — no-op
        assert_eq!(app.project.active, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn switching_chapters_saves_pending_edits() {
        let (mut app, dir) = two_chapter_app("autosave");
        // Edit chapter one, then jump away without an explicit save.
        app.on_key(key(KeyCode::Char('X')));
        assert!(app.buffer.dirty);

        app.on_key(ctrl('n'));
        assert_eq!(app.project.active, 1);

        // Chapter one was persisted on the way out.
        let saved = std::fs::read_to_string(dir.join("one.md")).unwrap();
        assert!(saved.starts_with('X'), "expected edit saved, got: {saved:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nav_mode_selects_and_opens_a_chapter() {
        let (mut app, dir) = two_chapter_app("navmode");
        app.on_key(ctrl('g')); // focus the chapter pane
        assert_eq!(app.mode, Mode::Nav);
        assert_eq!(app.nav_selected, 0);

        app.on_key(key(KeyCode::Down)); // select chapter two
        assert_eq!(app.nav_selected, 1);

        app.on_key(key(KeyCode::Enter)); // open it
        assert_eq!(app.mode, Mode::Insert);
        assert_eq!(app.project.active, 1);
        assert!(app.buffer.line(0).contains("second"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
