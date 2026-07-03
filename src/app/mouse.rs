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
    pub conceal: Vec<(usize, usize)>,
    /// The last logical line of the buffer, so a click below the last row can land
    /// at document end.
    pub last_line: usize,
}

/// The display-only first-line indent width in cells (mirrors `editor::PARAGRAPH_INDENT`).
const INDENT_CELLS: u16 = 4;

/// How many visual rows one scroll-wheel notch moves the viewport (#764).
const SCROLL_STEP: usize = 3;

impl App {
    /// Dispatch a mouse event (#757/#764): a left-click positions the cursor in the
    /// editor; the wheel scrolls the focused scrollable surface (an overlay, the
    /// assistant transcript, or the editor viewport). Other mouse events are ignored.
    pub(crate) fn on_mouse(&mut self, ev: MouseEvent) {
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => self.on_click(ev.column, ev.row),
            MouseEventKind::ScrollUp => self.on_scroll(-1),
            MouseEventKind::ScrollDown => self.on_scroll(1),
            _ => {}
        }
    }

    /// Move the cursor to the character nearest a left-click at screen `(col, row)`
    /// (#757). A no-op unless the plain editor is the active surface and the click
    /// falls in its prose column — clicks in an overlay, a modal, a prompt, the
    /// outline strip, or the status line are out of scope here.
    fn on_click(&mut self, col: u16, row: u16) {
        // Only the plain editing surface takes clicks: an open overlay, a focused
        // docked surface / assistant pane, or a modal mode owns the screen instead.
        if self.overlay.is_some()
            || self.surface_pane_focused()
            || self.assistant_pane_focused()
            || !matches!(
                self.mode,
                Mode::Insert | Mode::Normal | Mode::Visual { .. } | Mode::EditMeta(_)
            )
        {
            return;
        }
        let Some((line, byte)) = self.click_target(col, row) else {
            return;
        };
        let buffer = self.edit_buffer_mut();
        let line_start = buffer.line_start_byte(line);
        buffer.cursor = buffer.cursor_at_byte(line_start + byte);
        // A click is a cursor move, so any read-ahead scroll snaps back (#764) and
        // the status message clears the way a keystroke would.
        self.scroll_offset.set(0);
        self.message = None;
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
        // Otherwise the editor viewport: a read-ahead offset decoupled from the
        // cursor, clamped by the renderer so it can't run past the top or leave
        // blank space past the end. A subsequent cursor move/edit snaps it back.
        self.scroll_editor(notches);
    }

    /// Shift the editor's decoupled read-ahead viewport offset (#764) by `notches`
    /// (positive scrolls down). Clamped to zero at the top; the renderer clamps the
    /// bottom against the real content so no blank space shows past the end.
    fn scroll_editor(&mut self, notches: i32) {
        let step = notches * SCROLL_STEP as i32;
        let cur = self.scroll_offset.get();
        // The renderer stores the offset clamped to the content each frame, so `cur`
        // is never larger than the last-drawn maximum — a wheel-up here responds at
        // once rather than draining a runaway accumulation.
        let next = if step >= 0 {
            cur.saturating_add(step as usize)
        } else {
            cur.saturating_sub((-step) as usize)
        };
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
                | Mode::History(_)
                | Mode::Annotations { .. }
                | Mode::Outline { .. }
                | Mode::Corkboard(_)
                | Mode::ModelManager { .. }
                | Mode::Palette(_)
                | Mode::ProjectSearch(_)
                | Mode::RefFiles(_)
                | Mode::Tree(_)
                | Mode::ProjectBrowser(_)
                | Mode::DirBrowser(_)
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
