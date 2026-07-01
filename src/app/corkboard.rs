//! The read-only corkboard (#589, Phase 1): an index-card overview of the active
//! chapter's scenes, built from the `##`/`###` Markdown headings Quill already
//! parses (the same headings the Ctrl-O outline lists). Each card is a scene
//! heading plus the first line of prose beneath it as a synopsis; the view is
//! navigable and Enter jumps the editor cursor to that scene.
//!
//! It's read-only and derives entirely from the already-parsed headings, so it
//! stays off the keystroke path — building the cards is a walk over the heading
//! list plus one slice per scene, done once when the view opens. Phase 2
//! (per-scene metadata — POV / location / summary blocks — and scene reordering,
//! possibly cross-chapter) is a documented follow-up on #589.

use crossterm::event::{KeyCode, KeyEvent};

use super::App;
use crate::mode::{Card, Corkboard, Mode};

impl App {
    /// Open the corkboard for the active chapter (#589, Phase 1), or report when
    /// the chapter has no scene headings to card up. The cards are captured now
    /// from the already-parsed headings, so navigating them never re-parses.
    pub(crate) fn open_corkboard(&mut self) {
        let cards = self.corkboard_cards();
        if cards.is_empty() {
            self.message =
                Some("No scenes here — add a ## or ### heading to build the corkboard.".into());
            return;
        }
        self.mode = Mode::Corkboard(Box::new(Corkboard { cards, selected: 0 }));
    }

    /// Build the corkboard cards for the active buffer (#589): one per scene
    /// heading, in document order, each with the first non-empty line of prose
    /// under it as a synopsis and the heading's byte offset for the jump. Reads
    /// the already-parsed headings ([`crate::syntax::Syntax::headings`]) and a
    /// single slice per scene — no re-parse.
    pub(crate) fn corkboard_cards(&self) -> Vec<Card> {
        let buffer = self.edit_buffer();
        let text = buffer.text();
        let headings = buffer.syntax().headings();
        headings
            .iter()
            .enumerate()
            .map(|(i, h)| {
                // The scene body runs from the end of this heading's line up to
                // the next heading (or the end of the chapter).
                let body_start = line_end(&text, h.start);
                let body_end = headings.get(i + 1).map(|n| n.start).unwrap_or(text.len());
                let synopsis =
                    first_prose_line(&text[body_start.min(text.len())..body_end.min(text.len())]);
                Card {
                    title: h.title.clone(),
                    level: h.level,
                    synopsis,
                    start: h.start,
                }
            })
            .collect()
    }

    /// Key handling for the read-only corkboard (#589): move (arrows / j-k), jump
    /// the editor cursor to the selected scene (Enter), close (Esc).
    pub(crate) fn on_key_corkboard(&mut self, key: KeyEvent) {
        let Mode::Corkboard(board) = &mut self.mode else {
            return;
        };
        if let Some(s) = super::assist::list_move(board.selected, board.cards.len(), key.code) {
            board.selected = s;
            return;
        }
        match key.code {
            KeyCode::Enter => {
                let start = board.cards.get(board.selected).map(|c| c.start);
                self.mode = self.editing_mode();
                if let Some(start) = start {
                    let buf = self.edit_buffer_mut();
                    let at = start.min(buf.len_bytes());
                    buf.cursor = buf.cursor_at_byte(at);
                }
            }
            KeyCode::Esc => self.mode = self.editing_mode(),
            _ => {}
        }
    }
}

/// The byte offset just past the newline that ends the line containing `at` — the
/// start of the scene body under a heading. The heading node's range already
/// covers its own line; this finds where the following prose begins.
fn line_end(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    match text[at..].find('\n') {
        Some(nl) => at + nl + 1,
        None => text.len(),
    }
}

/// The first non-empty, non-heading line of `body`, trimmed — the card's
/// synopsis. Skips blank lines and any further `#` heading lines (a card's body
/// ends before the next scene, but a leading blank run is common). Empty when the
/// scene has no prose yet.
fn first_prose_line(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("")
        .to_string()
}
