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
use std::ops::Range;

use crate::app::App;
use crate::buffer::Buffer;
use crate::syntax::Heading;

/// Galley proofreading mode (#1003): a distinct read-only presentation that paginates
/// the active chapter one screen-page at a time — Space/Backspace to page forward and
/// back — instead of the continuous, cursor-following scroll of the edit view. Proofing
/// against a still page (rather than a moving wall of text) surfaces the errors a scroll
/// glides past. It is a sibling of the distraction-free composition mode ([`App::
/// toggle_distraction_free`]): both hide the chrome and show only the centered prose, but
/// the galley never edits and never follows the cursor — it is its own paged, read-only
/// view, so entering and leaving it leaves the edit buffer's cursor and scroll untouched.
///
/// The state is the current page's top logical line and whether line numbers are shown;
/// it lives beside the buffer, never inside it, so the rope and the file are untouched
/// exactly as folding is. `None` on the app means proofing is off (the normal editor).
#[derive(Clone, Copy)]
pub struct GalleyState {
    /// The logical (buffer) line that starts the page currently on screen. Paging moves
    /// this to the first line of the next / previous screenful of visual rows; the
    /// renderer draws from here down, never from the cursor.
    pub top_line: usize,
    /// Whether each visual row is prefixed with its 1-based logical line number (#1003) —
    /// a reference gutter for a proofing pass. Toggled on with `n`, off by default.
    pub line_numbers: bool,
}

impl GalleyState {
    /// Enter proofing with the page opening on `top_line` (usually the edit cursor's
    /// line, so a pass starts where the author was), line numbers off.
    #[must_use]
    const fn opening_at(top_line: usize) -> Self {
        Self {
            top_line,
            line_numbers: false,
        }
    }
}

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
///
/// Both are kept as sorted lists answered by binary search rather than a set of
/// every hidden line: one range per folded section, so building the view costs
/// O(folded headings), not O(hidden lines) — folding a whole long chapter used to
/// insert every body line into a `HashSet` on every frame.
pub struct FoldView {
    /// Logical (buffer) line ranges hidden by a fold — the body of every collapsed
    /// section, merged into sorted, disjoint, non-adjacent `start..end` ranges (a
    /// nested fold inside a folded section folds into its parent's range). Rendered
    /// as zero visual rows so the section reads as just its header.
    hidden: Vec<Range<usize>>,
    /// Logical line indices of the headings that are collapsed, sorted and
    /// deduplicated — each shows a fold marker so the author can see the section is
    /// folded (and expand it).
    collapsed: Vec<usize>,
}

impl FoldView {
    /// An empty view — nothing hidden, nothing collapsed. The reference pane and
    /// any never-folding surface use this so `render_body` takes one uniform param.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            hidden: Vec::new(),
            collapsed: Vec::new(),
        }
    }

    /// Build a view from each folded section's hidden line range (in any order,
    /// possibly overlapping or empty) and its heading line.
    fn from_parts(mut ranges: Vec<Range<usize>>, mut collapsed: Vec<usize>) -> Self {
        ranges.retain(|r| r.start < r.end);
        ranges.sort_unstable_by_key(|r| r.start);
        let mut hidden: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for r in ranges {
            match hidden.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => hidden.push(r),
            }
        }
        collapsed.sort_unstable();
        collapsed.dedup();
        Self { hidden, collapsed }
    }

    /// Whether logical line `line` is hidden by a fold (renders as zero rows).
    #[must_use]
    pub fn is_hidden(&self, line: usize) -> bool {
        // The last range starting at or before `line` is the only one that can
        // cover it (the ranges are sorted and disjoint).
        let after = self.hidden.partition_point(|r| r.start <= line);
        after > 0 && line < self.hidden[after - 1].end
    }

    /// Whether logical line `line` is a collapsed heading (shows a fold marker).
    #[must_use]
    pub fn is_collapsed_heading(&self, line: usize) -> bool {
        self.collapsed.binary_search(&line).is_ok()
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
        if self.folded.is_empty() {
            return FoldView::none();
        }
        let line_count = buffer.line_count();
        let mut hidden = Vec::new();
        let mut collapsed = Vec::new();
        for (i, h) in headings.iter().enumerate() {
            if !self.folded.contains(&h.start) {
                continue;
            }
            // The line only (no column): `cursor_at_byte` would also walk the
            // line's graphemes for a column this never reads.
            let heading_line = buffer.line_of_byte(h.start);
            collapsed.push(heading_line);
            // Hide every line strictly after the heading's line, up to the first
            // line starting at or past the section end (the line holding the next
            // same-or-higher heading) — the section body.
            let end = first_line_at_or_after(buffer, section_end_byte(headings, i), line_count);
            hidden.push(heading_line + 1..end);
        }
        FoldView::from_parts(hidden, collapsed)
    }
}

/// The first logical line whose start byte is at or past `byte` (`line_count` when
/// none is) — where a fold's hidden body stops. O(log n) on the rope.
fn first_line_at_or_after(buffer: &Buffer, byte: usize, line_count: usize) -> usize {
    if byte > buffer.len_bytes() {
        return line_count;
    }
    let line = buffer.line_of_byte(byte);
    let end = if buffer.line_start_byte(line) >= byte {
        line
    } else {
        line + 1
    };
    end.min(line_count)
}

/// The byte offset where the section opened by heading `idx` ends: the start of the
/// next heading at the same or a higher (numerically lower) level, or the document
/// end if none follows. Shared by [`FoldState::view`] and [`FoldState::reveal_at`],
/// and by the outline restructure (#1008), which moves a section's whole span.
pub fn section_end_byte(headings: &[Heading], idx: usize) -> usize {
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
            return FoldView::none();
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

    /// Whether the read-only galley proofreading mode is on (#1003).
    #[must_use]
    pub const fn in_galley(&self) -> bool {
        self.galley.is_some()
    }

    /// Enter (or leave) the galley proofreading mode (#1003): a distinct read-only,
    /// paginated presentation of the active chapter. Entering opens the first page on the
    /// edit cursor's own line so the pass starts where the author was, and — crucially —
    /// touches neither the buffer cursor nor the edit view's scroll offset, so leaving
    /// drops straight back to the exact edit position. Leaving simply forgets the paging
    /// state. Composes atop the same chrome-free presentation as composition mode.
    pub(crate) fn toggle_galley(&mut self) {
        if self.galley.is_some() {
            self.galley = None;
            self.message = Some("Proofreading mode off.".into());
        } else {
            let top = self.edit_buffer().cursor.line;
            self.galley = Some(GalleyState::opening_at(top));
            self.message = Some(
                "Proofreading mode — Space/Backspace page · n line numbers · Esc exits.".into(),
            );
        }
    }

    /// Leave the galley proofreading mode (#1003) if it is on — the Esc / toggle-off exit
    /// that restores the edit view exactly as it was (the cursor and scroll were never
    /// disturbed while proofing). A no-op when proofing is already off.
    pub(super) fn exit_galley(&mut self) {
        if self.galley.is_some() {
            self.galley = None;
            self.message = Some("Proofreading mode off.".into());
        }
    }

    /// Page the galley forward (#1003): advance the page top to the first logical line of
    /// the next screenful of visual rows, clamped so the last page can't scroll past the
    /// end of the chapter (no blank pages beyond the final line). Read-only — it moves the
    /// page, never the edit cursor.
    pub(super) fn galley_page_forward(&mut self) {
        let (wrap, height) = self.galley_geometry.get();
        let (wrap, height) = (wrap as usize, height as usize);
        let buffer = self.edit_buffer();
        let Some(g) = self.galley else { return };
        // The furthest a page may start and still fill the screen from the bottom — so the
        // final page shows the chapter's tail, not blank space past the end.
        let last_top = crate::ui::galley_last_top(buffer, wrap, height);
        let next = crate::ui::galley_next_top(buffer, g.top_line, wrap, height).min(last_top);
        if next > g.top_line {
            if let Some(g) = self.galley.as_mut() {
                g.top_line = next;
            }
        }
    }

    /// Page the galley back (#1003): retreat the page top to the first line of the
    /// previous screenful of visual rows, clamped at the start of the chapter. Read-only.
    pub(super) fn galley_page_back(&mut self) {
        let (wrap, height) = self.galley_geometry.get();
        let buffer = self.edit_buffer();
        let Some(g) = self.galley else { return };
        let prev = crate::ui::galley_prev_top(buffer, g.top_line, wrap as usize, height as usize);
        if let Some(g) = self.galley.as_mut() {
            g.top_line = prev;
        }
    }

    /// Toggle the galley's reference line numbers (#1003) — the gutter that numbers each
    /// visual row, on request during a proofing pass.
    pub(crate) fn toggle_galley_line_numbers(&mut self) {
        if let Some(g) = self.galley.as_mut() {
            g.line_numbers = !g.line_numbers;
            self.message = Some(if g.line_numbers {
                "Line numbers on.".into()
            } else {
                "Line numbers off.".into()
            });
        }
    }
}
