//! The focused heading outline (#657): the Ctrl-O outline pane made navigable.
//!
//! The outline pane normally just lists the active chapter's headings and
//! accents the section the cursor sits in — display-only. Focusing it (a third
//! Ctrl-O press, once the pane is up) enters [`Mode::Outline`], where a
//! selection cursor moves over the headings and Enter jumps the editor cursor to
//! the selected heading's byte offset — the same `Heading.start` the corkboard
//! jumps by. It's read-only over the prose; Enter (or Esc) returns to editing
//! and leaves the pane visible, so the author can keep navigating by section.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::App;
use crate::mode::Mode;

impl App {
    /// Focus the outline pane (#657) so its headings can be navigated. Starts the
    /// selection at the section the cursor is currently in (the pane's existing
    /// highlight), or the first heading. A no-op with a message when the chapter
    /// has no headings — there's nothing to navigate. Operates on the active
    /// chapter's buffer, the exact list the pane renders and jumps by.
    pub(crate) fn focus_outline(&mut self) {
        let headings = self.buffer.syntax().headings();
        if headings.is_empty() {
            self.message =
                Some("No headings here — add a # heading to navigate the outline.".into());
            return;
        }
        // Land on the current section so focusing picks up where the cursor is.
        let cursor = self.buffer.cursor_byte();
        let selected = headings
            .iter()
            .rposition(|h| h.start <= cursor)
            .unwrap_or(0);
        self.mode = Mode::Outline { selected };
    }

    /// Key handling for the focused outline (#657): move the selection (arrows /
    /// j-k), jump the editor cursor to the selected heading (Enter), or return to
    /// editing without moving (Esc). Every exit leaves the outline pane visible.
    pub(crate) fn on_key_outline(&mut self, key: KeyEvent) {
        let Mode::Outline { selected } = self.mode else {
            return;
        };
        // Ctrl-O completes the Ctrl-O cycle: from the focused outline it hides
        // the pane and returns to editing (hidden → shown → focused → hidden).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
            self.outline_visible = false;
            self.mode = self.editing_mode();
            return;
        }
        let len = self.buffer.syntax().headings().len();
        if let Some(s) = super::assist::list_move(selected, len, key.code) {
            self.mode = Mode::Outline { selected: s };
            return;
        }
        match key.code {
            KeyCode::Enter => {
                let start = self
                    .buffer
                    .syntax()
                    .headings()
                    .get(selected)
                    .map(|h| h.start);
                self.mode = self.editing_mode();
                if let Some(start) = start {
                    let at = start.min(self.buffer.len_bytes());
                    self.buffer.cursor = self.buffer.cursor_at_byte(at);
                }
            }
            KeyCode::Esc => self.mode = self.editing_mode(),
            _ => {}
        }
    }
}
