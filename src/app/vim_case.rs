//! Transpose and case-toggle affordances for fast typo cleanup (#1006): `gs`/`gS`
//! transpose the characters/words around the cursor (the classic `xp`), `~`
//! toggles the case under the cursor, and the `g~` / `gu` / `gU` operators
//! case-map a motion or text object. The buffer's motion layer computes each edit
//! as a pure [`CaseEdit`]; these App methods write it back through the standard
//! range-edit primitive, so every one is a single undo step and — flowing through
//! the normal change-dispatch path — `.`-repeatable via the #1004 record.
//!
//! Split out of `vim.rs` to keep that file under the ~1,000-line convention; the
//! Normal-mode dispatch arms that call into here live in `vim.rs`.

use crate::buffer::{CaseChange, CaseEdit};
use crate::mode::Mode;

use super::App;

impl App {
    /// Visual-mode `~` / `u` / `U` (#1006): case-map the current selection, then
    /// leave Visual. `key` picks the mapping the same way the `g~`/`gu`/`gU`
    /// operators do.
    pub(super) fn visual_case(&mut self, key: char, anchor: usize, linewise: bool) {
        let kind = match key {
            'u' => CaseChange::Lower,
            'U' => CaseChange::Upper,
            _ => CaseChange::Toggle,
        };
        let (lo, hi) = self.selection_bytes(anchor, linewise);
        self.apply_case(kind, lo, hi);
        self.mode = Mode::Normal;
    }

    /// `gs`: transpose the two characters around the cursor — Vim's classic `xp` in
    /// one gesture (#1006). One undoable, `.`-repeatable edit.
    pub(super) fn transpose_chars(&mut self) {
        self.vim_state.count = None;
        if let Some(edit) = self.edit_buffer().transpose_chars_edit() {
            self.apply_case_edit(edit);
        }
    }

    /// `gS`: transpose the two adjacent words around the cursor (#1006).
    pub(super) fn transpose_words(&mut self) {
        self.vim_state.count = None;
        if let Some(edit) = self.edit_buffer().transpose_words_edit() {
            self.apply_case_edit(edit);
        }
    }

    /// `~`: toggle the case of the character under the cursor and step right, as in
    /// Vim (#1006). A single undoable, `.`-repeatable edit.
    pub(super) fn toggle_case_under_cursor(&mut self) {
        if let Some(edit) = self.edit_buffer().toggle_case_edit() {
            self.apply_case_edit(edit);
        }
    }

    /// Arm a `g~` / `gu` / `gU` case operator (#1006), awaiting the motion or text
    /// object it acts on. The count is folded onto `case_op` so it survives the
    /// wait; the doubled linewise form (`g~~`, `guu`, `gUU`) and the following
    /// motion are resolved on the next keystroke in [`App::on_key_normal`].
    pub(super) fn case_operator(&mut self, key: char) {
        let kind = match key {
            'u' => CaseChange::Lower,
            'U' => CaseChange::Upper,
            _ => CaseChange::Toggle,
        };
        let count = self.vim_state.count.take().unwrap_or(1);
        self.case_op = Some((kind, count));
    }

    /// Resolve the key that follows a pending `g~` / `gu` / `gU` case operator
    /// (#1006), returning whether it was fully handled here. The operator's own
    /// doubled key (`g~~`, `guu`, `gUU`) case-maps the current line(s) linewise; a
    /// key that is neither the double nor a motion / text-object prefix cancels the
    /// operator, matching Vim. A real motion (`w`, `$`, `iw`, `G`, …) returns
    /// `false` so the caller's motion dispatch runs and consults `case_op`.
    pub(super) fn resolve_case_prefix(&mut self, c: char, count: usize) -> bool {
        let doubled = matches!(
            (self.case_op.map(|(k, _)| k), c),
            (Some(CaseChange::Toggle), '~')
                | (Some(CaseChange::Lower), 'u')
                | (Some(CaseChange::Upper), 'U')
        );
        if doubled {
            if let Some((kind, n)) = self.take_case_op(count) {
                let cur = self.edit_buffer().cursor.line;
                self.case_lines(kind, cur, cur + n.saturating_sub(1));
            }
            return true;
        }
        if !super::vim::is_motion_key(c) && !matches!(c, 'i' | 'a' | 'G') {
            self.case_op = None; // an unknown motion cancels the operator
            return true;
        }
        false
    }

    /// Apply a pending case operator over the byte range `[lo, hi)`.
    pub(super) fn apply_case(&mut self, kind: CaseChange, lo: usize, hi: usize) {
        if let Some(edit) = self.edit_buffer().case_map_edit(lo, hi, kind) {
            self.apply_case_edit(edit);
        }
    }

    /// Case-map the inclusive line range `[first, last]` for a linewise case
    /// operator (`g~j`, `gUU`, `g~G`, #1006). Maps the whole lines' content, one
    /// undo step; the cursor lands at the first line's start, as Vim's linewise
    /// operators leave it.
    pub(super) fn case_lines(&mut self, kind: CaseChange, first: usize, last: usize) {
        let (lo, hi) = {
            let buf = self.edit_buffer();
            let last = last.min(buf.line_count().saturating_sub(1));
            let lo = buf.line_start_byte(first);
            // End at the last line's content (before its newline), so the mapping
            // never touches line breaks.
            let hi = buf.line_start_byte(last) + buf.line(last).len();
            (lo, hi)
        };
        self.apply_case(kind, lo, hi);
    }

    /// Write a computed [`CaseEdit`] back through the standard range-edit primitive
    /// (#1006) — one undo group, seam-aware in the continuous view, and recorded on
    /// the dot-repeat path (#1004) since it bumps the buffer's edit counter. Then
    /// place the cursor where the edit says.
    fn apply_case_edit(&mut self, edit: CaseEdit) {
        let CaseEdit {
            lo,
            hi,
            text,
            cursor,
        } = edit;
        if self.edit_replace_range(lo, hi, &text) {
            let buf = self.edit_buffer_mut();
            buf.cursor = buf.cursor_at_byte(cursor);
            self.cache_focus_words();
        }
    }

    /// Take a pending case operator with its effective count (operator count times
    /// the motion count), the parallel to `take_op` for `g~` / `gu` / `gU`. `None`
    /// when no case operator is armed.
    pub(super) fn take_case_op(&mut self, count: usize) -> Option<(CaseChange, usize)> {
        self.case_op
            .take()
            .map(|(kind, opc)| (kind, count.saturating_mul(opc)))
    }
}
