//! Top-level application state and the editing modes that drive key handling.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::buffer::Buffer;
use crate::project::Project;

/// Editor modes. Quill is mostly modeless for writing, but borrows a light
/// command line for actions that shouldn't be a stray keystroke away.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Normal prose entry. Typed characters go straight into the buffer.
    Insert,
    /// A transient prompt at the bottom (e.g. confirm quit with unsaved changes).
    Prompt,
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
    /// Whether the in-chapter outline pane is shown.
    pub outline_visible: bool,
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
            Mode::Insert => self.on_key_insert(key),
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
