//! The focused heading outline (#657): the Ctrl-O outline pane made navigable.
//!
//! The outline pane normally just lists the active chapter's headings and
//! accents the section the cursor sits in — display-only. Focusing it (a third
//! Ctrl-O press, once the pane is up) enters [`Mode::Outline`], where a
//! selection cursor moves over the headings and Enter jumps the editor cursor to
//! the selected heading's byte offset — the same `Heading.start` the corkboard
//! jumps by. Enter (or Esc) returns to editing and leaves the pane visible, so the
//! author can keep navigating by section.
//!
//! The focused outline also *restructures* the manuscript's skeleton (#1008),
//! keyboard-native, without opening the corkboard: `<` / `>` promote or demote the
//! selected heading's level (fewer / more `#`), and `K` / `J` move the selected
//! section — its heading plus the whole body under it, folded-in deeper sub-headings
//! and all — up or down past its adjacent sibling, carrying the body with it. Each is
//! one gesture-scoped, undoable edit: it reuses the section-span logic
//! ([`section_end_byte`], shared with the folds and the corkboard) to find the block,
//! and rewrites the affected bytes through [`crate::buffer::Buffer::replace_byte_range`]
//! — the same undoable, incrementally-reparsed edit path find-and-replace uses — so a
//! single `u` reverses the move and body text is never split, dropped, or corrupted.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::folds::section_end_byte;
use super::list_filter::FilteredList;
use super::App;
use crate::mode::{ListFilter, Mode};
use crate::syntax::Heading;

/// The focused outline as a [`FilteredList`] (#822): rows are the active
/// chapter's headings, matched on their title.
struct OutlineList;

impl FilteredList for OutlineList {
    fn state(app: &App) -> Option<(usize, &ListFilter)> {
        match &app.mode {
            Mode::Outline { selected, filter } => Some((*selected, filter)),
            _ => None,
        }
    }

    fn state_mut(app: &mut App) -> Option<(&mut usize, &mut ListFilter)> {
        match &mut app.mode {
            Mode::Outline { selected, filter } => Some((selected, filter)),
            _ => None,
        }
    }

    fn len(app: &App) -> usize {
        app.outline_entries().len()
    }

    fn matches(app: &App, index: usize, q: &str) -> bool {
        app.outline_heading_matches(index, q)
    }
}

impl App {
    /// Focus the outline pane (#657) so its headings can be navigated. Starts the
    /// selection at the section the cursor is currently in (the pane's existing
    /// highlight), or the first heading. A no-op with a message when the chapter
    /// has no headings — there's nothing to navigate. Operates on the active
    /// chapter's buffer, the exact list the pane renders and jumps by.
    ///
    /// Refused in the continuous view (like the corkboard): the pane and the
    /// `<`/`>`/`K`/`J` restructure gestures reachable only once it's focused all
    /// read and rewrite `self.buffer` — the *stale* single-chapter buffer left
    /// from before the window opened, not the live multi-chapter buffer on screen.
    /// Entering here would show a different chapter's headings than what's visible
    /// and land any restructure on the invisible stale buffer, which `flush_active`
    /// would then write to disk — a lost update. The author leaves the view
    /// (Ctrl-l) first, so `self.buffer` is the live, single-chapter buffer again.
    pub(crate) fn focus_outline(&mut self) {
        if self.manuscript.is_some() {
            self.message =
                Some("Leave the continuous view (Ctrl-l) to restructure headings.".into());
            return;
        }
        // The outline pane shows the active chapter's structure. When a non-chapter
        // reference pane is focused (a bible entry / note, #165) the pane lists the
        // chapters flat, so there's no outline to focus — point the author back to
        // the manuscript rather than navigate the invisible reference file.
        if !self.focus_shows_active_chapter() {
            self.message = Some("Focus a chapter (Ctrl-w w) to navigate its outline.".into());
            return;
        }
        let headings = self.outline_entries();
        if headings.is_empty() {
            self.message = Some(if self.editing_story() {
                "No scenes yet — write some prose to navigate the outline.".into()
            } else {
                "No headings here — add a # heading to navigate the outline.".into()
            });
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
            filter: ListFilter::new(),
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
            selected,
            filter: ListFilter { typing, .. },
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
        let len = self.outline_entries().len();
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
        OutlineList::select(self, selected);
        // In the filter-input sub-mode, keystrokes build the query (#822); the
        // pane's single-key commands are reachable only outside it.
        if typing {
            self.on_key_list_filter::<OutlineList>(key);
            return;
        }
        // `/`, the arrows, and Esc-clears-the-filter are the shared list keys.
        if self.on_key_list_command::<OutlineList>(key) {
            return;
        }
        match key.code {
            // A story's outline lists scenes (#1240): they have no levels to
            // promote, and moving one swaps it with its neighbouring scene.
            KeyCode::Char('<' | '>') if self.editing_story() => {
                self.message = Some("Scenes have no heading levels to change.".into());
            }
            KeyCode::Char('K' | '[' | 'J' | ']') if self.editing_story() => {
                let dir = if matches!(key.code, KeyCode::Char('K' | '[')) {
                    -1
                } else {
                    1
                };
                if let Some(to) = self.move_story_scene(selected, dir) {
                    OutlineList::select(self, to);
                }
            }
            // Restructure the skeleton (#1008): promote/demote the heading's level,
            // or move the whole section past its sibling — one undoable gesture each.
            KeyCode::Char('<') => self.outline_reheading(selected, -1),
            KeyCode::Char('>') => self.outline_reheading(selected, 1),
            KeyCode::Char('K' | '[') => self.outline_move_section(selected, -1),
            KeyCode::Char('J' | ']') => self.outline_move_section(selected, 1),
            KeyCode::Enter if OutlineList::selected_matches(self) => {
                let start = self.outline_entries().get(selected).map(|h| h.start);
                // Leave a jump-back target (#825) so `` `` `` returns to where the
                // author was reading before the outline sent them across the chapter.
                self.push_jump();
                self.mode = self.editing_mode();
                if let Some(start) = start {
                    let at = start.min(self.buffer.len_bytes());
                    self.buffer.cursor = self.buffer.cursor_at_byte(at);
                }
            }
            // With no filter to clear, Esc returns to editing.
            KeyCode::Esc => self.mode = self.editing_mode(),
            _ => {}
        }
    }

    /// The active outline filter, lowercased (empty off the outline/unfiltered).
    pub(crate) fn outline_query(&self) -> String {
        OutlineList::query(self)
    }

    /// Whether heading `index` passes the outline's active filter (#822): a
    /// case-insensitive substring match on its title. `q` is the already-lowercased
    /// query (empty = every heading matches).
    pub(crate) fn outline_heading_matches(&self, index: usize, q: &str) -> bool {
        if q.is_empty() {
            return true;
        }
        self.outline_entries()
            .get(index)
            .is_some_and(|h| h.title.to_lowercase().contains(q))
    }

    /// Promote (`dir` −1, one fewer `#`) or demote (`dir` +1, one more `#`) the
    /// level of the selected heading (#1008), rewriting just its `#` prefix through
    /// the undoable edit path. Clamped to the `#`…`######` range: a level-1 heading
    /// won't promote and a level-6 won't demote (a no-op with a note). Only the
    /// marker run changes — the heading text and every byte of the body under it are
    /// untouched — so the whole gesture is one undo step. Leaves the outline focused
    /// on the same heading; the pane re-derives its level on the next reparse.
    fn outline_reheading(&mut self, selected: usize, dir: isize) {
        let buffer = &self.buffer;
        let headings = buffer.syntax().headings();
        let Some(h) = headings.get(selected) else {
            return;
        };
        let level = h.level;
        let new_level = match dir {
            d if d < 0 => level.saturating_sub(1),
            _ => level.saturating_add(1),
        };
        if new_level == 0 || new_level > 6 || new_level == level {
            self.message = Some(if dir < 0 {
                "Already a top-level heading — can't promote further.".into()
            } else {
                "Already the deepest heading level — can't demote further.".into()
            });
            return;
        }
        // The heading's `#` marker run: from its start byte to the first non-`#`
        // byte (the parser's `start` is the line's first `#`). Replace only that run
        // with `new_level` hashes, so the title and the body stay byte-for-byte.
        let text = self.buffer.text();
        let start = h.start;
        let marks_end = text[start..]
            .find(|c: char| c != '#')
            .map_or(text.len(), |off| start + off);
        let hashes = "#".repeat(new_level as usize);
        self.buffer.replace_byte_range(start, marks_end, &hashes);
        self.commit_outline_edit();
        self.message = Some(if dir < 0 {
            "Heading promoted.".into()
        } else {
            "Heading demoted.".into()
        });
    }

    /// Move the selected section up (`dir` −1) or down (`dir` +1) past its adjacent
    /// sibling (#1008): its heading and the entire block of body under it — including
    /// any deeper sub-headings that fold into it — swap places with the neighbouring
    /// sibling's whole block, carrying every byte of prose. A *sibling* is the
    /// nearest heading in that direction at the same level with no shallower heading
    /// between them (so a section never jumps out from under its parent). A no-op at
    /// the first/last sibling (a note, no edit). One undoable `replace_byte_range`
    /// over the two adjacent blocks, so a single `u` restores the prior order and the
    /// body is never split. The outline follows the moved section to its new slot.
    fn outline_move_section(&mut self, selected: usize, dir: isize) {
        let headings = self.buffer.syntax().headings().to_vec();
        let Some(sibling) = sibling_index(&headings, selected, dir) else {
            self.message = Some(if dir < 0 {
                "Already the first section at this level.".into()
            } else {
                "Already the last section at this level.".into()
            });
            return;
        };
        let text = self.buffer.text();
        let len = text.len();
        // The two blocks in body order (lower start first); `a` precedes `b`.
        let (a, b) = (selected.min(sibling), selected.max(sibling));
        let a_start = headings[a].start;
        let b_start = headings[b].start;
        let b_end = section_end_byte(&headings, b).min(len);
        // The sibling scan guarantees `a`'s block ends exactly where `b`'s begins, so
        // the pair `[a_start, b_end)` is one contiguous span holding just the two
        // blocks — swapping their order reorders the sections without touching a byte
        // outside them. `a` runs to `b`'s start; `b` runs to the span end.
        let block_a = &text[a_start..b_start];
        let block_b = &text[b_start..b_end];
        // Trim each block's trailing blank lines and rejoin the swapped pair with one
        // blank line between the headings, restoring the span's original trailing
        // whitespace (a final newline, typically) so spacing stays even across
        // repeated moves rather than the last block accreting or losing blanks. Only
        // trailing whitespace is trimmed — every non-blank byte of prose is carried.
        let tail = &text[a_start..b_end][text[a_start..b_end].trim_end().len()..];
        let swapped = format!("{}\n\n{}{}", block_b.trim_end(), block_a.trim_end(), tail);
        self.buffer.replace_byte_range(a_start, b_end, &swapped);
        self.commit_outline_edit();
        // Follow the moved section: after the swap the selected heading sits where
        // its sibling was in body order, so its new index is the sibling's old one.
        OutlineList::select(self, sibling);
        self.message = Some("Section moved.".into());
    }

    /// Finish an outline restructure gesture (#1008): seal the undo group so the
    /// whole rewrite is one `u`, refresh the syntax so the pane and any follow-up
    /// key see the new heading list at once, and save + re-cache the chapter through
    /// [`Self::flush_active`] so the change is durable (matching the corkboard's
    /// per-gesture persist). A save failure surfaces a message but leaves the
    /// in-buffer edit — the author can retry the save.
    fn commit_outline_edit(&mut self) {
        self.buffer.seal_undo_group();
        self.buffer.refresh_syntax();
        let _ = self.flush_active();
    }
}

/// The index of the selected heading's adjacent sibling in `dir` (−1 previous, +1
/// next), or `None` at that end (#1008). Siblings share a level with no shallower
/// heading between them, so a section reorders only among peers under the same
/// parent — never past a heading that owns it. A heading deeper than `selected` is
/// one of its sub-beats (skipped); a shallower one is a boundary the scan stops at.
fn sibling_index(headings: &[Heading], selected: usize, dir: isize) -> Option<usize> {
    let level = headings.get(selected)?.level;
    let mut i = selected;
    loop {
        i = i.checked_add_signed(dir)?;
        let h = headings.get(i)?;
        if h.level < level {
            return None; // hit the parent / a shallower boundary before a sibling
        }
        if h.level == level {
            return Some(i); // the adjacent peer
        }
        // h.level > level: a sub-heading of a neighbour — keep scanning past it.
    }
}
