//! Mouse handling (#757 left-click to position the cursor, #764 scroll wheel).
//!
//! Terminal mouse capture is enabled in `main.rs`, so the event loop receives
//! `Event::Mouse`. A left-click in the editor text moves the cursor to the
//! nearest character; the scroll wheel moves the editor viewport (or the focused
//! scrollable overlay) up and down.
//!
//! The hard part of a click is the inverse of the render mapping (screen → buffer
//! byte). Rather than re-derive scroll / soft-wrap / conceal / indent / fenced-code
//! logic, the focused editor renderer records a per-visual-row [`EditorLayout`] as
//! it draws; the click handler just looks a row up and walks its bytes. The map is
//! rebuilt every frame, so it always reflects exactly what's on screen.

use std::rc::Rc;

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use super::{App, OtherPane, Split};
use crate::mode::Mode;

/// One visual (screen) row of the focused editor's prose column, recorded at
/// render time so a click can be inverted to a buffer position without redoing the
/// layout math. Byte offsets are relative to the logical line's start.
#[derive(Clone, Debug)]
pub struct VisualRow {
    /// The logical (buffer) line this visual row belongs to.
    pub line: usize,
    /// Absolute byte offset (in the buffer) of this line's start.
    pub line_start: usize,
    /// Byte offset within the line where this visual row's text begins.
    pub row_start: usize,
    /// Byte offset within the line where this visual row's text ends.
    pub row_end: usize,
    /// A render-only row (chapter/part divider, epigraph, or a concealed fence)
    /// carries no editable text; a click on it maps to the nearest line start.
    pub editable: bool,
    /// Preview fenced code (#723): rendered verbatim and not soft-wrapped, and its
    /// left edge carries no first-line indent.
    pub verbatim: bool,
    /// This row got the display-only first-line paragraph indent (#722), so its
    /// text starts `PARAGRAPH_INDENT` cells in from the column's left edge.
    pub indent: bool,
}

/// The focused editor's on-screen layout for one frame (#757): the prose column
/// rect, one entry per visible visual row, and the conceal ranges that were in
/// effect — everything a click needs to map screen coordinates back to a buffer
/// byte. Rebuilt every render; empty until the first draw or when the editor isn't
/// the drawn surface (an open overlay replaces it).
#[derive(Clone, Debug, Default)]
pub struct EditorLayout {
    /// The centered prose column the body was drawn into.
    pub column: Rect,
    /// One entry per on-screen visual row, top to bottom.
    pub rows: Vec<VisualRow>,
    /// Emphasis-marker conceal ranges (absolute bytes) used for this frame, so the
    /// click's column→byte walk drops the same hidden characters the render did.
    /// Shares the `Rc` the parse caches (#869): recording it is a refcount bump, not
    /// a clone of the ranges.
    pub conceal: Rc<[(usize, usize)]>,
    /// The last logical line of the buffer, so a click below the last row can land
    /// at document end.
    pub last_line: usize,
}

/// The focused list overlay's on-screen layout for one frame (#812): where its
/// rows were drawn, so a click can be mapped back to a list index the way
/// [`EditorLayout`] maps a click to a buffer byte. Recorded by
/// [`crate::ui::list_overlay::draw_list_overlay`] each frame the list is drawn;
/// empty when no list overlay is on screen.
#[derive(Clone, Debug, Default)]
pub struct ListLayout {
    /// Screen row of the first rendered list row (past the box border, help line,
    /// and blank). A click at or below this row, within `height`, is on a list row.
    pub first_row: u16,
    /// How many screen rows the list occupies (its visible viewport).
    pub height: u16,
    /// The list index of the first *visible* row — the scroll offset the overlay
    /// applied so the selection stayed in view. A click on visible row `i` selects
    /// index `first_index + i`.
    pub first_index: usize,
    /// The number of selectable list items, so a click past the last one is
    /// clamped rather than selecting a non-existent row.
    pub len: usize,
}

impl ListLayout {
    /// Reset to empty, so a stale layout from a previous frame can't take a click
    /// after the list overlay has closed. Called at the top of each render.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// The display-only first-line indent width in cells (mirrors `editor::PARAGRAPH_INDENT`).
const INDENT_CELLS: u16 = 4;

/// How many visual rows one scroll-wheel notch moves the viewport (#764).
const SCROLL_STEP: usize = 3;

impl App {
    /// Dispatch a mouse event (#757/#764/#812): a left press positions the cursor
    /// in the editor (or selects a row in a focused list); dragging the left button
    /// extends a Visual selection; release leaves it active; the wheel scrolls the
    /// focused scrollable surface. Other mouse events are ignored.
    pub(crate) fn on_mouse(&mut self, ev: MouseEvent) {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => self.on_press(ev.column, ev.row),
            MouseEventKind::Drag(MouseButton::Left) => self.on_drag(ev.column, ev.row),
            MouseEventKind::Up(MouseButton::Left) => self.mouse_anchor = None,
            MouseEventKind::ScrollUp => self.on_scroll(-1),
            MouseEventKind::ScrollDown => self.on_scroll(1),
            _ => {}
        }
    }

    /// A left-button press (#757/#812): position the cursor at the click in the
    /// editor and remember the press byte so a following drag can anchor a Visual
    /// selection there; or, when a scrollable list overlay is focused, select the
    /// clicked row. A plain press-and-release (no drag) is just a cursor move — the
    /// press byte is only consumed by [`Self::on_drag`], and cleared on release.
    fn on_press(&mut self, col: u16, row: u16) {
        // A focused list overlay (corkboard / outline / findings / …) takes a click
        // on one of its rows as a selection (#812), before the editor click path.
        if self.click_list_row(col, row) {
            return;
        }
        // A click in a split's other pane switches focus to it (#163 follow-up),
        // before the editor click path — otherwise the focused-editor gate below
        // would swallow a click meant to focus a docked surface / assistant.
        if self.focus_pane_from_click(col, row) {
            return;
        }
        // Only the plain editing surface takes clicks: an open overlay, a focused
        // docked surface / assistant pane, or a modal mode owns the screen instead.
        if !self.editor_takes_click() {
            return;
        }
        let Some(byte) = self.click_byte(col, row) else {
            return;
        };
        let buffer = self.edit_buffer_mut();
        buffer.cursor = buffer.cursor_at_byte(byte);
        // Remember where the press landed so a drag can anchor a selection there.
        self.mouse_anchor = Some(byte);
        // Leaving Visual on a fresh press matches clicking away from a selection.
        if matches!(self.mode, Mode::Visual { .. }) {
            self.mode = self.editing_mode();
        }
        // A click is a cursor move, so any read-ahead scroll snaps back (#764) and
        // the status message clears the way a keystroke would.
        self.scroll_offset.set(0);
        self.message = None;
    }

    /// A left-button drag (#812): extend a Visual selection to the drag point,
    /// entering Visual mode (anchored at the press byte) on the first drag. A
    /// no-op unless a press landed in the editor text (so `mouse_anchor` is set)
    /// and the drag maps to a buffer byte — a drag that started outside the prose
    /// column, or in a mode that doesn't select, does nothing.
    fn on_drag(&mut self, col: u16, row: u16) {
        let Some(anchor) = self.mouse_anchor else {
            return;
        };
        if !self.editor_takes_click() {
            return;
        }
        let Some(byte) = self.click_byte(col, row) else {
            return;
        };
        // Enter Visual (characterwise) on the first drag, anchored at the press;
        // subsequent drags just move the cursor (the active end), extending it.
        if !matches!(self.mode, Mode::Visual { .. }) {
            self.mode = Mode::Visual {
                anchor,
                linewise: false,
            };
        }
        let buffer = self.edit_buffer_mut();
        buffer.cursor = buffer.cursor_at_byte(byte);
        self.scroll_offset.set(0);
        self.message = None;
    }

    /// Switch pane focus from a left-click when the window is split (#163 follow-up):
    /// a click in the other pane focuses it (swap editors, or hand scroll/key focus to
    /// a docked surface / assistant), and a click back in the focused editor's pane
    /// while a surface/assistant holds focus returns focus to the editor. Returns
    /// whether the click was consumed. Uses the pane rects the renderer records each
    /// frame (`pane_rects`), so the divider column between panes is never a hit.
    fn focus_pane_from_click(&mut self, col: u16, row: u16) -> bool {
        let Some((focused_rect, other_rect)) = self.pane_rects.get() else {
            return false;
        };
        let hit = |r: ratatui::layout::Rect| {
            col >= r.x
                && col < r.x.saturating_add(r.width)
                && row >= r.y
                && row < r.y.saturating_add(r.height)
        };
        let other_focused = self.surface_pane_focused() || self.assistant_pane_focused();
        if hit(other_rect) {
            // An editor pane is never focused in place, so a click always swaps it in;
            // a surface/assistant only needs focusing when it doesn't already have it.
            if self.other_pane_is_editor() || !other_focused {
                self.focus_other_pane();
            }
            return true;
        }
        if hit(focused_rect) && other_focused {
            self.focus_other_pane(); // hand focus back to the editor
            return true;
        }
        false
    }

    /// Whether the plain editing surface should take a click / drag — not an open
    /// overlay, a focused docked surface / assistant pane, or a non-editing modal.
    const fn editor_takes_click(&self) -> bool {
        self.overlay.is_none()
            && !self.surface_pane_focused()
            && !self.assistant_pane_focused()
            && matches!(
                self.mode,
                Mode::Insert | Mode::Normal | Mode::Visual { .. } | Mode::EditMeta(_)
            )
    }

    /// The absolute buffer byte a click at screen `(col, row)` maps to, using the
    /// render-time layout (#757) — `None` when the click is outside the prose
    /// column. Wraps [`Self::click_target`], resolving its (line, byte-in-line) to
    /// an absolute offset.
    fn click_byte(&self, col: u16, row: u16) -> Option<usize> {
        let (line, byte) = self.click_target(col, row)?;
        Some(self.edit_buffer().line_start_byte(line) + byte)
    }

    /// Map a screen `(col, row)` to a `(logical line, byte-within-line)` in the
    /// active editor buffer, using the layout recorded at render time. `None` when
    /// the click is outside the prose column's horizontal band or above its top;
    /// a click below the last drawn row lands at the end of the document.
    fn click_target(&self, col: u16, row: u16) -> Option<(usize, usize)> {
        let layout = self.editor_layout.borrow();
        let column = layout.column;
        if column.width == 0 || layout.rows.is_empty() {
            return None;
        }
        // Horizontal band: the prose column plus a click past its right edge maps to
        // the row's end. A click left of the column is out of scope (it's margin).
        if col < column.x || row < column.y {
            return None;
        }
        let idx = (row - column.y) as usize;
        let Some(vrow) = layout.rows.get(idx) else {
            // Below the last drawn row: land at the end of the document.
            let last = layout.last_line;
            return Some((last, self.edit_buffer().line(last).len()));
        };
        // A render-only row (divider / epigraph / hidden fence) has no editable
        // text; put the cursor at its logical line's start — a sensible edit point.
        if !vrow.editable {
            return Some((vrow.line, 0));
        }
        let text = self.edit_buffer().line(vrow.line);
        let byte = byte_at_click(&text, vrow, &layout.conceal, col.saturating_sub(column.x));
        Some((vrow.line, byte))
    }

    /// A click on a row of a focused scrollable list overlay (#812): select that
    /// row. Uses the render-time [`ListLayout`] to map the click's screen row to a
    /// list index (accounting for the overlay's scroll offset), then moves the
    /// active list mode's selection there. Returns whether the click hit a list row
    /// (so the editor click path is skipped). Only the modal list modes that
    /// navigate a `selected` index are affected; a full-screen report / guide
    /// overlay (no selection) records no list layout and so is untouched.
    fn click_list_row(&mut self, col: u16, row: u16) -> bool {
        // The click must fall on the vertical band the list rows occupy.
        let (first_row, height, first_index, len) = {
            let l = self.list_layout.borrow();
            (l.first_row, l.height, l.first_index, l.len)
        };
        if len == 0 || height == 0 || row < first_row || col == 0 {
            return false;
        }
        let offset = (row - first_row) as usize;
        if offset >= height as usize {
            return false;
        }
        // Clamp a click past the last item to the last row rather than miss.
        let index = (first_index + offset).min(len - 1);
        self.select_list_index(index)
    }

    /// Move the focused list mode's selection to `index` (#812). Returns whether a
    /// list mode was actually selectable — `false` for a mode with no navigable
    /// `selected` row, so the caller can fall through to the editor click path.
    fn select_list_index(&mut self, index: usize) -> bool {
        match &mut self.mode {
            Mode::Findings(f) => f.selected = index,
            // Only the finding rows of the dashboard are selectable; a click on a
            // header row (a kind/chapter heading) is ignored so the selection stays
            // on a jump target (#827).
            Mode::Dashboard(d) => {
                if d.finding_at(index).is_some() {
                    d.selected = index;
                } else {
                    return false;
                }
            }
            // Only the pair / absence rows of the co-occurrence report are
            // selectable; a click on a section header is ignored (#922).
            Mode::CoOccurrence(c) => {
                if c.chapter_at(index).is_some() {
                    c.selected = index;
                } else {
                    return false;
                }
            }
            Mode::History(h) => h.selected = index,
            Mode::Corkboard(c) => c.selected = index,
            Mode::BackupBrowser(b) => b.selected = index,
            // The annotation list can be sorted/filtered (#933), so a click's screen
            // position is a display position — map it back to the annotation index
            // the visible order holds there before selecting.
            Mode::Annotations { .. } => {
                let order = self.annotation_visible_order();
                let Some(&idx) = order.get(index) else {
                    return false;
                };
                self.set_annotations_selected(idx);
            }
            Mode::Outline { selected, .. } => {
                *selected = index;
            }
            _ => return false,
        }
        self.message = None;
        true
    }

    /// Scroll the focused scrollable surface by `notches` (positive = down, toward
    /// later content) (#764). Routes to an open full-screen overlay, a focused docked
    /// surface / assistant transcript, or — the common case — the editor viewport.
    fn on_scroll(&mut self, notches: i32) {
        let delta = notches * SCROLL_STEP as i32;
        // A full-screen overlay owns the screen: scroll it, clamped to its content.
        if let Some(overlay) = self.overlay.as_mut() {
            let max = crate::ui::max_scroll(overlay.lines.len(), self.viewport_height);
            overlay.scroll = clamp_add(overlay.scroll, delta, max);
            return;
        }
        // A focused docked surface (report/analysis) scrolls the same way (#164).
        if self.surface_pane_focused() {
            let height = self.surface_height.get();
            self.scroll_focused_surface(delta, height);
            return;
        }
        // The focused assistant transcript scrolls in "rows up from the bottom"
        // (#307), so a wheel-down (positive) reduces the scroll-up.
        if self.assistant_pane_focused() {
            let max = self.assistant_scroll_max.get();
            self.scroll_assistant(-delta, max);
            return;
        }
        // A modal list overlay (Tree, palette, findings, …) scrolls its selection.
        if self.scroll_modal_list(notches) {
            return;
        }
        // Otherwise the editor viewport: a signed offset decoupled from the cursor,
        // clamped by the renderer so it can't run past the top of the chapter or
        // leave blank space past the end. A subsequent cursor move/edit snaps it back.
        self.scroll_editor(notches);
    }

    /// Shift the editor's decoupled viewport offset (#764/#936) by `notches`
    /// (positive scrolls down toward later content, negative scrolls up toward the
    /// start of the chapter). The renderer clamps the stored offset against the real
    /// content each frame — the top of the chapter one way, no blank tail the other —
    /// so this can add unconditionally; over-scroll is corrected on the next render.
    fn scroll_editor(&self, notches: i32) {
        let step = (notches * SCROLL_STEP as i32) as isize;
        // The renderer stores the offset clamped to the content each frame, so `cur`
        // never sits past the last-drawn maximum — a reversing wheel here responds at
        // once rather than draining a runaway accumulation.
        let next = self.scroll_offset.get().saturating_add(step);
        self.scroll_offset.set(next);
    }

    /// Scroll a focused docked surface pane (#164) by `delta` rows, clamped to its
    /// content in a box of `height` rows.
    fn scroll_focused_surface(&mut self, delta: i32, height: u16) {
        if let Some(Split {
            other: OtherPane::Surface { overlay, .. },
            ..
        }) = self.split.as_mut()
        {
            let max = crate::ui::max_scroll(overlay.lines.len(), height);
            overlay.scroll = clamp_add(overlay.scroll, delta, max);
        }
    }

    /// Scroll the focused assistant transcript (#307) by `delta` rows-up (positive
    /// scrolls toward earlier content), clamped to `max`.
    fn scroll_assistant(&mut self, delta: i32, max: u16) {
        if let Some(Split {
            other: OtherPane::Assistant { conv, .. },
            ..
        }) = self.split.as_mut()
        {
            conv.set_scroll_up(clamp_add(conv.scroll_up(), delta, max));
        }
    }

    /// Route a wheel notch to an open modal list overlay (#525) by moving its
    /// selection like the arrow keys do — reusing each mode's own Up/Down handling
    /// so clamping and side effects stay identical. Returns whether a modal list
    /// was scrolled.
    fn scroll_modal_list(&mut self, notches: i32) -> bool {
        let is_list = matches!(
            self.mode,
            Mode::Nav { .. }
                | Mode::Findings(_)
                | Mode::Dashboard(_)
                | Mode::CoOccurrence(_)
                | Mode::History(_)
                | Mode::Annotations { .. }
                | Mode::Outline { .. }
                | Mode::Corkboard(_)
                | Mode::BackupBrowser(_)
                | Mode::ModelManager { .. }
                | Mode::Palette(_)
                | Mode::ProjectSearch(_)
                | Mode::RefFiles(_)
                | Mode::Tree(_)
                | Mode::ProjectBrowser(_)
                | Mode::DirBrowser(_)
                | Mode::QuickSwitch(_)
        );
        if !is_list {
            return false;
        }
        let code = if notches >= 0 {
            KeyCode::Down
        } else {
            KeyCode::Up
        };
        for _ in 0..notches.unsigned_abs() {
            self.on_key(KeyEvent {
                code,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: crossterm::event::KeyEventState::NONE,
            });
        }
        true
    }
}

/// Clamp `base + delta` to the range `[0, max]` (a `u16` scroll offset moved by a
/// signed row delta). Shared by the overlay / surface scroll routing.
pub fn clamp_add(base: u16, delta: i32, max: u16) -> u16 {
    let next = i32::from(base) + delta;
    next.clamp(0, i32::from(max)) as u16
}

/// The byte offset within `text` (relative to the logical line) nearest a click at
/// display column `rel_col` (measured from the prose column's left edge), for the
/// recorded visual row `vrow`. Walks the row's source characters accumulating
/// visible cell width, skipping concealed markers exactly as the renderer did, so
/// the click lands on the same character that's on screen. A click past the row's
/// last visible character clamps to the row's end.
fn byte_at_click(text: &str, vrow: &VisualRow, conceal: &[(usize, usize)], rel_col: u16) -> usize {
    // Peel the display-only first-line indent (#722) off the clicked column: the
    // indent shifts every character right but isn't in the buffer.
    let mut target = rel_col;
    if vrow.indent {
        if target < INDENT_CELLS {
            return vrow.row_start;
        }
        target -= INDENT_CELLS;
    }
    let seg = &text[vrow.row_start..vrow.row_end];
    let mut width = 0u16;
    for (local, ch) in seg.char_indices() {
        let abs = vrow.line_start + vrow.row_start + local;
        // Verbatim preview code renders every character (nothing concealed); prose
        // drops concealed emphasis markers, so they don't consume a click column.
        if !vrow.verbatim && is_concealed(abs, conceal) {
            continue;
        }
        let w = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
        // Land on the character whose cell the click falls within: once half a
        // wide char is passed we're closer to the next one, matching "nearest".
        if target < width + w.div_ceil(2).max(1) {
            return vrow.row_start + local;
        }
        width += w;
    }
    // Past the row's last character: the row's end (which is the line end for the
    // final row, and the wrap point otherwise).
    vrow.row_end
}

fn is_concealed(byte: usize, conceal: &[(usize, usize)]) -> bool {
    conceal.iter().any(|&(s, e)| byte >= s && byte < e)
}
