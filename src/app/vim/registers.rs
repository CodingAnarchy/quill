//! Vim registers, put, and the Visual-mode operators (#205/#811): storing yanks
//! (mirrored to the system clipboard), the paste paths, `p`/`P` put, and the
//! Visual selection's operate/put/bytes helpers. Split out of `vim.rs` (pure move).
use crate::mode::{Mode, Op, Yank};

use super::App;

impl App {
    /// Record a yank into the unnamed register, and into the named register if
    /// `"x` selected one. Also mirror the text onto the system clipboard (#811),
    /// so a Vim yank / delete or a Visual copy is available to paste into another
    /// app — the internal registers keep working exactly as before, the clipboard
    /// is an additional destination.
    pub(super) fn store_register(&mut self, yank: Yank) {
        self.clipboard.set(&yank.text);
        if let Some(name) = self.vim_state.selected_register.take() {
            self.vim_state.registers.named.insert(name, yank.clone());
        }
        self.vim_state.registers.unnamed = Some(yank);
    }
    /// Insert the system clipboard's contents (#811) at the cursor as one
    /// undoable edit — the seam-read paste path (what Quill itself copied, over
    /// OSC 52). A no-op when the clipboard is empty.
    pub(crate) fn paste_from_clipboard(&mut self) {
        if let Some(text) = self.clipboard.get().filter(|t| !t.is_empty()) {
            self.insert_pasted_text(&text);
        }
    }
    /// Insert `text` at the cursor (or over a Visual selection) as one undoable
    /// edit (#811). The shared paste path: the terminal's bracketed-paste event
    /// delivers the OS clipboard here, and [`Self::paste_from_clipboard`] routes
    /// the seam-read text through it too. The text lands verbatim
    /// (characterwise), through the range-edit path so the continuous view's seam
    /// bookkeeping stays correct (#700), and the whole insert is a single undo
    /// group.
    pub(crate) fn insert_pasted_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        // A Visual selection is replaced by the paste, matching `p` in Visual mode;
        // otherwise the text is inserted at the cursor.
        let (lo, hi) = if let Mode::Visual { anchor, linewise } = self.mode {
            self.mode = self.editing_mode();
            self.selection_bytes(anchor, linewise)
        } else {
            let at = self.edit_buffer().cursor_byte();
            (at, at)
        };
        self.edit_replace_range(lo, hi, text);
        self.cache_focus_words();
    }
    /// The register `p` / `P` reads from: the `"x`-selected named register, else
    /// the unnamed one.
    fn active_register(&mut self) -> Option<Yank> {
        match self.vim_state.selected_register.take() {
            Some(name) => self.vim_state.registers.named.get(&name).cloned(),
            None => self.vim_state.registers.unnamed.clone(),
        }
    }
    /// `p` / `P`: put the active register `count` times — inline for a charwise
    /// yank, on new lines for a linewise one.
    pub(super) fn put(&mut self, after: bool, count: usize) {
        // `"+p` / `"*p`: paste the OS clipboard (#811) as a characterwise put,
        // rather than an internal register.
        if self.vim_state.clipboard_register {
            self.vim_state.clipboard_register = false;
            for _ in 0..count {
                self.paste_from_clipboard();
            }
            return;
        }
        let Some(yank) = self.active_register() else {
            return;
        };
        let body = yank.text.repeat(count);
        // Where the put lands (computed against the current buffer), then insert
        // through the range-edit path so the continuous view keeps its seam
        // bookkeeping correct (#700). An insert never crosses a seam, but the
        // primitive still attributes the added lines to the right chapter.
        let (at, line) = {
            let buf = self.edit_buffer();
            let line = buf.cursor.line;
            if yank.linewise {
                let at = if after {
                    buf.next_line_byte(line)
                } else {
                    buf.line_start_byte(line)
                };
                (at, line)
            } else {
                let line_end = buf.line_start_byte(line) + buf.line(line).len();
                let at = if after {
                    buf.grapheme_after(buf.cursor_byte()).min(line_end)
                } else {
                    buf.cursor_byte()
                };
                (at, line)
            }
        };
        self.edit_replace_range(at, at, &body);
        let buf = self.edit_buffer_mut();
        if yank.linewise {
            buf.move_to_line(if after { line + 1 } else { line });
        } else {
            buf.move_left(); // land on the last grapheme put, as Vim does
        }
        self.cache_focus_words();
    }
    /// The inclusive line range of the current Visual selection.
    fn selection_lines(&self, anchor: usize) -> (usize, usize) {
        let buf = self.edit_buffer();
        let a = buf.cursor_at_byte(anchor).line;
        let b = buf.cursor.line;
        (a.min(b), a.max(b))
    }
    /// The byte range the current Visual selection covers — whole lines when
    /// linewise, else the chars between the ends (inclusive of the later one).
    /// Shared by the operators, put, and the highlight.
    pub(crate) fn selection_bytes(&self, anchor: usize, linewise: bool) -> (usize, usize) {
        let buf = self.edit_buffer();
        if linewise {
            let (first, last) = self.selection_lines(anchor);
            (buf.line_start_byte(first), buf.next_line_byte(last))
        } else {
            let cur = buf.cursor_byte();
            (anchor.min(cur), buf.grapheme_after(anchor.max(cur)))
        }
    }
    /// Apply an operator to the Visual selection, then leave Visual.
    pub(super) fn visual_operate(&mut self, op: Op, anchor: usize, linewise: bool) {
        self.vim_state.count = None;
        if linewise {
            let (first, last) = self.selection_lines(anchor);
            self.operate_linewise(op, first, last);
        } else {
            let (lo, hi) = self.selection_bytes(anchor, false);
            self.operate_charwise(op, lo, hi);
        }
        // `c` left us in Insert; delete/yank return to Normal.
        if op != Op::Change {
            self.mode = Mode::Normal;
        }
    }
    /// `p` in Visual: replace the selection with the register, then leave Visual.
    pub(super) fn visual_put(&mut self, anchor: usize, linewise: bool) {
        let Some(yank) = self.active_register() else {
            self.mode = Mode::Normal;
            return;
        };
        let (lo, hi) = self.selection_bytes(anchor, linewise);
        // Through the range-edit path so the continuous view's seam bookkeeping
        // stays correct (#700); a selection spanning a seam is clamped there.
        self.edit_replace_range(lo, hi, &yank.text);
        let buf = self.edit_buffer_mut();
        buf.cursor = buf.cursor_at_byte(lo);
        self.cache_focus_words();
        self.mode = Mode::Normal;
    }
}
