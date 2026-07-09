//! Heading/scene folding (#975): collapse a section — a heading plus its body up
//! to the next same-or-higher heading — to its header line, in place. Purely
//! display-only, modeled on the scene-break dividers (#908): the fold state lives
//! beside the buffer, never inside it, so the rope and the file are byte-for-byte
//! untouched. The renderer reads [`FoldState`] each frame to decide which buffer
//! lines a fold hides (they render as zero rows, like a concealed fence) and which
//! headings show a collapsed marker.
//!
//! Fold state is keyed on a heading's *start byte* (from the syntax cache), not its
//! line number, so an edit above a folded heading that shifts its position doesn't
//! silently unfold or mis-target it — the next reparse moves the heading's start
//! byte and the fold travels with it. A key that no longer matches any heading
//! (the heading was deleted or its text rewritten past recognition) is simply inert
//! until [`FoldState::prune`] drops it, so a stale fold never hides live prose.

use std::collections::HashSet;

use crate::app::App;
use crate::buffer::Buffer;
use crate::syntax::Heading;

/// The set of collapsed headings, keyed by each heading's start byte in the buffer
/// (#975). Empty by default — nothing folds until the author collapses a section.
#[derive(Default, Clone)]
pub struct FoldState {
    /// Start bytes of the headings that are currently folded. A `HashSet` because
    /// the only questions asked are "is this heading folded?" and "toggle it".
    folded: HashSet<usize>,
}

/// The buffer lines a fold hides, plus which heading lines carry a collapse marker
/// — computed once per frame from the headings and the fold state. `hidden` lines
/// render as zero rows (like a concealed fence, #723); `collapsed` heading lines
/// render their header text with a leading fold glyph.
pub struct FoldView {
    /// Logical (buffer) line indices hidden by a fold — the body of every collapsed
    /// section. Rendered as zero visual rows so the section reads as just its header.
    pub hidden: HashSet<usize>,
    /// Logical line indices of the headings that are collapsed — each shows a fold
    /// marker so the author can see the section is folded (and expand it).
    pub collapsed: HashSet<usize>,
}

impl FoldView {
    /// An empty view — nothing hidden, nothing collapsed. The reference pane and
    /// any never-folding surface use this so `render_body` takes one uniform param.
    #[must_use]
    pub fn none() -> Self {
        Self {
            hidden: HashSet::new(),
            collapsed: HashSet::new(),
        }
    }

    /// Whether logical line `line` is hidden by a fold (renders as zero rows).
    #[must_use]
    pub fn is_hidden(&self, line: usize) -> bool {
        self.hidden.contains(&line)
    }

    /// Whether logical line `line` is a collapsed heading (shows a fold marker).
    #[must_use]
    pub fn is_collapsed_heading(&self, line: usize) -> bool {
        self.collapsed.contains(&line)
    }
}

impl FoldState {
    /// Whether the heading starting at byte `start` is folded — the tests' probe
    /// into the fold set.
    #[cfg(test)]
    #[must_use]
    pub fn is_folded(&self, start: usize) -> bool {
        self.folded.contains(&start)
    }

    /// Whether nothing at all is folded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }

    /// Toggle the fold at heading start byte `start`; returns the new folded state.
    pub fn toggle(&mut self, start: usize) -> bool {
        if self.folded.remove(&start) {
            false
        } else {
            self.folded.insert(start);
            true
        }
    }

    /// Fold every heading in `headings` (fold-all).
    pub fn fold_all(&mut self, headings: &[Heading]) {
        for h in headings {
            self.folded.insert(h.start);
        }
    }

    /// Expand everything (expand-all).
    pub fn clear(&mut self) {
        self.folded.clear();
    }

    /// Unfold the section containing byte `at`, if it is folded — the reveal used
    /// when the cursor (a search jump, a click, a chapter switch) lands inside a
    /// collapsed section, so folding never hides the cursor. Returns whether a fold
    /// was actually opened. Reveals every folded heading whose body covers `at`, so
    /// nested folds around the cursor open together and its line is left visible.
    pub fn reveal_at(&mut self, headings: &[Heading], at: usize) -> bool {
        let mut opened = false;
        for (i, h) in headings.iter().enumerate() {
            if !self.folded.contains(&h.start) {
                continue;
            }
            let section_end = section_end_byte(headings, i);
            // The heading's own line stays visible; only its body is hidden. Reveal
            // when `at` lands past the heading's start and before the section end —
            // i.e. somewhere in the hidden body.
            if at > h.start && at < section_end {
                self.folded.remove(&h.start);
                opened = true;
            }
        }
        opened
    }

    /// Drop fold keys that no longer match any live heading (#975): after an edit
    /// deletes or rewrites a heading, its start byte is stale — pruning keeps the
    /// set from growing without bound and guarantees a stale key can never hide
    /// prose. Cheap; run on the idle reparse alongside the syntax refresh.
    pub fn prune(&mut self, headings: &[Heading]) {
        self.folded
            .retain(|start| headings.iter().any(|h| h.start == *start));
    }

    /// Compute the display view for this frame: which buffer lines every fold hides
    /// and which heading lines are collapsed. Skips absent folds cheaply — an empty
    /// state yields an empty view without touching the buffer.
    #[must_use]
    pub fn view(&self, buffer: &Buffer, headings: &[Heading]) -> FoldView {
        let mut hidden = HashSet::new();
        let mut collapsed = HashSet::new();
        if self.folded.is_empty() {
            return FoldView { hidden, collapsed };
        }
        for (i, h) in headings.iter().enumerate() {
            if !self.folded.contains(&h.start) {
                continue;
            }
            let heading_line = buffer.cursor_at_byte(h.start).line;
            collapsed.insert(heading_line);
            let section_end = section_end_byte(headings, i);
            // Hide every line strictly after the heading's line, up to the line that
            // holds the next same-or-higher heading (exclusive) — the section body.
            let mut line = heading_line + 1;
            while line < buffer.line_count() {
                let line_start = buffer.line_start_byte(line);
                if line_start >= section_end {
                    break;
                }
                hidden.insert(line);
                line += 1;
            }
        }
        FoldView { hidden, collapsed }
    }
}

/// The byte offset where the section opened by heading `idx` ends: the start of the
/// next heading at the same or a higher (numerically lower) level, or the document
/// end if none follows. Shared by [`FoldState::view`] and [`FoldState::reveal_at`].
fn section_end_byte(headings: &[Heading], idx: usize) -> usize {
    let level = headings[idx].level;
    headings[idx + 1..]
        .iter()
        .find(|h| h.level <= level)
        .map_or(usize::MAX, |h| h.start)
}

impl App {
    /// Toggle the fold of the section the cursor is in (#975): the last heading at
    /// or before the cursor. Collapses it to its header, or expands it if already
    /// folded. A no-op with a status note when the cursor sits above the first
    /// heading (there's no section to fold there).
    pub(super) fn toggle_fold_at_cursor(&mut self) {
        let buffer = self.edit_buffer();
        let cursor = buffer.cursor_byte();
        let headings = buffer.syntax().headings();
        let Some(h) = headings.iter().rev().find(|h| h.start <= cursor) else {
            self.message = Some("No section to fold here.".into());
            return;
        };
        let start = h.start;
        let folded = self.folds.toggle(start);
        if folded {
            self.snap_cursor_out_of_folds();
        }
        self.message = Some(if folded {
            "Section folded.".into()
        } else {
            "Section expanded.".into()
        });
    }

    /// Fold every section in the active buffer (#975) — collapse the whole chapter
    /// to its heading outline, in place.
    pub(super) fn fold_all(&mut self) {
        let headings = self.edit_buffer().syntax().headings().to_vec();
        if headings.is_empty() {
            self.message = Some("No sections to fold.".into());
            return;
        }
        self.folds.fold_all(&headings);
        self.snap_cursor_out_of_folds();
        self.message = Some("All sections folded.".into());
    }

    /// If folding just hid the cursor's line, move the cursor up to the collapsed
    /// heading whose section now hides it (#975) — so the cursor is always on a
    /// visible line and the renderer's cursor-follow math never sees a zero-row
    /// cursor line. The natural editor behavior: folding a section the cursor is in
    /// parks the cursor on the fold's header.
    fn snap_cursor_out_of_folds(&mut self) {
        let view = self.fold_view();
        let buffer = self.edit_buffer();
        let line = buffer.cursor.line;
        if !view.is_hidden(line) {
            return;
        }
        // Walk up to the nearest heading at or before the cursor whose own line is
        // still visible — the collapsed header the cursor should rest on. (With
        // nested folds the immediately-enclosing heading may itself be hidden under
        // a higher-level fold, so skip past any hidden heading line.)
        let cursor = buffer.cursor_byte();
        let headings = buffer.syntax().headings();
        let target = headings
            .iter()
            .rev()
            .filter(|h| h.start <= cursor)
            .map(|h| (h.start, buffer.cursor_at_byte(h.start).line))
            .find(|&(_, hline)| !view.is_hidden(hline))
            .map(|(start, _)| start);
        if let Some(start) = target {
            let buffer = self.edit_buffer_mut();
            buffer.cursor = buffer.cursor_at_byte(start);
        }
    }

    /// Expand every folded section (#975).
    pub(super) fn expand_all_folds(&mut self) {
        if self.folds.is_empty() {
            self.message = Some("Nothing is folded.".into());
            return;
        }
        self.folds.clear();
        self.message = Some("All sections expanded.".into());
    }

    /// Toggle distraction-free composition mode (#975): hide the chrome (panes,
    /// status bar) so only the prose shows, or restore it.
    pub(super) fn toggle_distraction_free(&mut self) {
        self.distraction_free = !self.distraction_free;
        self.message = Some(if self.distraction_free {
            "Composition mode on — Esc or the toggle restores the layout.".into()
        } else {
            "Composition mode off.".into()
        });
    }

    /// Reveal the folded section the cursor now sits in, if any (#975): the search
    /// jump / click / navigation reveal, so a fold never hides the cursor. Called on
    /// the paths that move the cursor programmatically. Returns whether a fold opened.
    pub(crate) fn reveal_fold_at_cursor(&mut self) -> bool {
        if self.folds.is_empty() {
            return false;
        }
        let buffer = self.edit_buffer();
        let at = buffer.cursor_byte();
        let headings = buffer.syntax().headings().to_vec();
        self.folds.reveal_at(&headings, at)
    }

    /// The fold display view for the active buffer this frame (#975): which lines a
    /// fold hides and which headings are collapsed. Empty (a cheap no-op) when
    /// nothing is folded.
    #[must_use]
    pub fn fold_view(&self) -> FoldView {
        if self.folds.is_empty() {
            return FoldView {
                hidden: HashSet::new(),
                collapsed: HashSet::new(),
            };
        }
        let buffer = self.edit_buffer();
        self.folds.view(buffer, buffer.syntax().headings())
    }

    /// Drop fold keys whose heading no longer exists (#975): run on the idle reparse
    /// so an edit that deletes or rewrites a heading can't leave a stale fold hiding
    /// live prose.
    pub(crate) fn prune_folds(&mut self) {
        if self.folds.is_empty() {
            return;
        }
        let headings = self.edit_buffer().syntax().headings().to_vec();
        self.folds.prune(&headings);
    }
}
