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
        self.mode = Mode::Outline {
            selected,
            filter: String::new(),
            typing: false,
        };
    }

    /// Key handling for the focused outline (#657): move the selection (arrows /
    /// j-k, skipping filtered-out headings), jump the editor cursor to the selected
    /// heading (Enter), or return to editing without moving (Esc). Every exit
    /// leaves the outline pane visible. `/` starts a type-to-filter query (#822)
    /// that narrows the pane to headings whose title matches; Esc clears the filter
    /// before it returns to editing.
    pub(crate) fn on_key_outline(&mut self, key: KeyEvent) {
        let Mode::Outline {
            selected, typing, ..
        } = self.mode
        else {
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
        // Re-clamp against the live heading list every keystroke (#717): the list
        // can shrink between focusing the outline and now (an idle reparse after an
        // edit trims a heading), and a `selected` left past the end would make Enter
        // silently no-op instead of landing on a scene. An empty list means the pane
        // has nothing left to navigate — return to editing rather than hold a focus
        // that can't move or jump.
        let Some(selected) = len.checked_sub(1).map(|last| selected.min(last)) else {
            self.mode = self.editing_mode();
            return;
        };
        self.set_outline_selected(selected);
        // In the filter-input sub-mode, keystrokes build the query (#822); the
        // pane's single-key commands are reachable only outside it.
        if typing {
            self.on_key_outline_filter(key);
            return;
        }
        match key.code {
            KeyCode::Char('/') => self.set_outline_typing(true),
            KeyCode::Up | KeyCode::Char('k') => {
                let s = self.outline_step(false);
                self.set_outline_selected(s);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let s = self.outline_step(true);
                self.set_outline_selected(s);
            }
            KeyCode::Enter if self.outline_query_matches(selected) => {
                let start = self
                    .buffer
                    .syntax()
                    .headings()
                    .get(selected)
                    .map(|h| h.start);
                // Leave a jump-back target (#825) so `` `` `` returns to where the
                // author was reading before the outline sent them across the chapter.
                self.push_jump();
                self.mode = self.editing_mode();
                if let Some(start) = start {
                    let at = start.min(self.buffer.len_bytes());
                    self.buffer.cursor = self.buffer.cursor_at_byte(at);
                }
            }
            // Esc clears an active filter first (restoring the full outline), then
            // a second Esc returns to editing.
            KeyCode::Esc => {
                if self.outline_has_filter() {
                    self.clear_outline_filter();
                } else {
                    self.mode = self.editing_mode();
                }
            }
            _ => {}
        }
    }

    /// Keys for the outline's filter-input sub-mode (#822): printable characters
    /// build the query (live), Backspace edits it, the arrows move within the
    /// matches, Enter/Esc leave input — keeping the filter so the pane's commands
    /// act on the narrowed set (a second Esc, in command mode, clears it).
    fn on_key_outline_filter(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.set_outline_typing(false),
            KeyCode::Up => {
                let s = self.outline_step(false);
                self.set_outline_selected(s);
            }
            KeyCode::Down => {
                let s = self.outline_step(true);
                self.set_outline_selected(s);
            }
            KeyCode::Backspace => self.outline_filter_pop(),
            KeyCode::Char(c) => self.outline_filter_push(c),
            _ => {}
        }
    }

    /// The active outline filter, lowercased (empty off the outline/unfiltered).
    pub(crate) fn outline_query(&self) -> String {
        match &self.mode {
            Mode::Outline { filter, .. } => filter.to_lowercase(),
            _ => String::new(),
        }
    }

    const fn outline_has_filter(&self) -> bool {
        matches!(&self.mode, Mode::Outline { filter, .. } if !filter.is_empty())
    }

    /// Whether heading `index` passes the outline's active filter (#822): a
    /// case-insensitive substring match on its title. `q` is the already-lowercased
    /// query (empty = every heading matches).
    pub(crate) fn outline_heading_matches(&self, index: usize, q: &str) -> bool {
        if q.is_empty() {
            return true;
        }
        self.buffer
            .syntax()
            .headings()
            .get(index)
            .is_some_and(|h| h.title.to_lowercase().contains(q))
    }

    fn outline_query_matches(&self, index: usize) -> bool {
        let q = self.outline_query();
        self.outline_heading_matches(index, &q)
    }

    const fn set_outline_typing(&mut self, on: bool) {
        if let Mode::Outline { typing, .. } = &mut self.mode {
            *typing = on;
        }
    }

    const fn set_outline_selected(&mut self, selected: usize) {
        if let Mode::Outline { selected: s, .. } = &mut self.mode {
            *s = selected;
        }
    }

    fn clear_outline_filter(&mut self) {
        if let Mode::Outline { filter, typing, .. } = &mut self.mode {
            filter.clear();
            *typing = false;
        }
    }

    fn outline_filter_push(&mut self, c: char) {
        if let Mode::Outline { filter, .. } = &mut self.mode {
            filter.push(c);
        }
        self.outline_snap();
    }

    fn outline_filter_pop(&mut self) {
        if let Mode::Outline { filter, .. } = &mut self.mode {
            filter.pop();
        }
        self.outline_snap();
    }

    /// After the filter changes, keep the selection on a visible heading: if the
    /// current one no longer matches, jump to the first that does (#822).
    fn outline_snap(&mut self) {
        let q = self.outline_query();
        let sel = match &self.mode {
            Mode::Outline { selected, .. } => *selected,
            _ => return,
        };
        if !self.outline_heading_matches(sel, &q) {
            let len = self.buffer.syntax().headings().len();
            if let Some(first) = (0..len).find(|&i| self.outline_heading_matches(i, &q)) {
                self.set_outline_selected(first);
            }
        }
    }

    /// The next (or previous, by `forward`) heading that passes the filter, or the
    /// current selection unchanged when there's no further match that way (#822).
    fn outline_step(&self, forward: bool) -> usize {
        let q = self.outline_query();
        let from = match &self.mode {
            Mode::Outline { selected, .. } => *selected,
            _ => return 0,
        };
        let n = self.buffer.syntax().headings().len();
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
            if self.outline_heading_matches(i, &q) {
                return i;
            }
        }
    }
}
