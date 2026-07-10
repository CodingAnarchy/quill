//! Vim operators, motions, and text objects (#208): the `d`/`c`/`y` operators,
//! the charwise / linewise motion machinery they and the standalone motions share,
//! text-object resolution, and the `o`/`O` open-line. Split out of `vim.rs` to keep
//! the dispatch under the ~1,000-line convention (pure move).
use crate::buffer::{Buffer, TextObject};
use crate::mode::{Mode, Op, Yank};

use super::App;

impl App {
    /// `d` / `c`: begin an operator awaiting a motion, or — when the same
    /// operator is already pending (`dd`/`cc`) — act linewise on the current
    /// line(s). `count` is this keypress's count.
    pub(super) fn operator(&mut self, op: Op, count: usize) {
        match self.vim_state.op.take() {
            Some((pending, opc)) => {
                if pending == op {
                    let lines = opc.saturating_mul(count).max(1);
                    let cur = self.edit_buffer().cursor.line;
                    self.operate_linewise(op, cur, cur + lines - 1);
                }
                // A mismatched operator (`dc`) just cancels.
            }
            None => self.vim_state.op = Some((op, count)),
        }
    }
    /// Resolve an `i`/`a` text object (`diw`, `ci"`, …) and apply the pending
    /// operator to its range.
    pub(super) fn apply_text_object(&mut self, id: char, around: bool) {
        let object = match id {
            'w' => Some(TextObject::Word),
            '"' | '\'' | '`' => Some(TextObject::Quote(id)),
            '(' | ')' | 'b' => Some(TextObject::Pair('(', ')')),
            '[' | ']' => Some(TextObject::Pair('[', ']')),
            '{' | '}' | 'B' => Some(TextObject::Pair('{', '}')),
            '<' | '>' => Some(TextObject::Pair('<', '>')),
            // `ip`/`ap` paragraph and `is`/`as` sentence objects (#821), the most
            // useful prose objects. `i{`/`a{` still reach the bracket pair above.
            'p' => Some(TextObject::Paragraph),
            's' => Some(TextObject::Sentence),
            _ => None,
        };
        // A pending case operator (`g~iw`, `gUaw`, …) acts on the object's range;
        // otherwise the delete/change/yank operator does (#1006).
        if let Some((kind, _)) = self.take_case_op(1) {
            self.vim_state.op = None;
            if let Some((lo, hi)) = object.and_then(|o| self.edit_buffer().text_object(o, around)) {
                self.apply_case(kind, lo, hi);
            }
            return;
        }
        match (object, self.vim_state.op.take()) {
            (Some(object), Some((op, _))) => {
                if let Some((lo, hi)) = self.edit_buffer().text_object(object, around) {
                    self.operate_charwise(op, lo, hi);
                }
            }
            _ => self.vim_state.op = None, // an unknown object cancels
        }
    }
    /// Take a pending operator together with the effective count — the operator's
    /// own count times the motion's count (`2d3w` → 6). `None` when no operator is
    /// pending, so the motion just moves the cursor.
    fn take_op(&mut self, count: usize) -> Option<(Op, usize)> {
        self.vim_state
            .op
            .take()
            .map(|(op, opc)| (op, count.saturating_mul(opc)))
    }
    /// The byte range `[lo, hi)` a charwise motion covers from the cursor: run the
    /// `step` `n` times on a scratch cursor, then take the span between where it
    /// started and landed (extended over the landing grapheme when `inclusive`).
    /// The cursor is restored, so this is a pure range computation shared by the
    /// delete/change operators and the case operators (#1006).
    fn charwise_range(
        &mut self,
        n: usize,
        inclusive: bool,
        step: &impl Fn(&mut Buffer),
    ) -> (usize, usize) {
        let buf = self.edit_buffer_mut();
        let start = buf.cursor_byte();
        let saved = buf.cursor;
        for _ in 0..n {
            step(buf);
        }
        let target = buf.cursor_byte();
        buf.cursor = saved;
        let hi = start.max(target);
        (
            start.min(target),
            if inclusive {
                buf.grapheme_after(hi)
            } else {
                hi
            },
        )
    }
    /// A charwise motion: move the cursor, or — when an operator is pending —
    /// delete/change the span from the cursor to where the motion lands.
    /// `inclusive` covers the landing grapheme (Vim's `e`, `$`, `f`, `t`).
    pub(super) fn charwise(&mut self, count: usize, inclusive: bool, step: impl Fn(&mut Buffer)) {
        // A pending case operator (`g~w`, `gU$`, #1006) shares the same range
        // computation as delete/change; resolve its span and case-map it.
        if let Some((kind, n)) = self.take_case_op(count) {
            let (lo, hi) = self.charwise_range(n, inclusive, &step);
            self.apply_case(kind, lo, hi);
            return;
        }
        if let Some((op, n)) = self.take_op(count) {
            let (lo, hi) = self.charwise_range(n, inclusive, &step);
            self.operate_charwise(op, lo, hi);
        } else {
            self.edit_move(move |b| {
                for _ in 0..count {
                    step(b);
                }
            });
        }
    }
    /// A linewise vertical motion (`j`/`k`): move by visual rows, or — with an
    /// operator pending — delete/change whole lines.
    pub(super) fn vertical(&mut self, count: usize, down: bool) {
        if let Some((kind, n)) = self.take_case_op(count) {
            let cur = self.edit_buffer().cursor.line;
            let other = if down { cur + n } else { cur.saturating_sub(n) };
            self.case_lines(kind, cur.min(other), cur.max(other));
            return;
        }
        if let Some((op, n)) = self.take_op(count) {
            let cur = self.edit_buffer().cursor.line;
            let other = if down { cur + n } else { cur.saturating_sub(n) };
            self.operate_linewise(op, cur.min(other), cur.max(other));
        } else {
            let mv = if down {
                Buffer::move_down
            } else {
                Buffer::move_up
            };
            self.vim_repeat(count, move |s| s.edit_move_wrapped(mv));
        }
    }
    /// Jump to `target` (0-based line): move the cursor, or — with an operator
    /// pending (`dG`, `dgg`) — act linewise over the lines between.
    pub(super) fn goto_line(&mut self, target: usize) {
        if let Some((kind, _)) = self.take_case_op(1) {
            let cur = self.edit_buffer().cursor.line;
            self.case_lines(kind, cur.min(target), cur.max(target));
            return;
        }
        if let Some((op, _)) = self.take_op(1) {
            let cur = self.edit_buffer().cursor.line;
            self.operate_linewise(op, cur.min(target), cur.max(target));
        } else {
            self.edit_move(move |b| b.move_to_line(target));
        }
    }
    /// `D` / `C`: delete (or change) from the cursor to the end of the line.
    pub(super) fn delete_to_line_end(&mut self, op: Op) {
        let buf = self.edit_buffer();
        let lo = buf.cursor_byte();
        let hi = buf.line_start_byte(buf.cursor.line) + buf.line(buf.cursor.line).len();
        self.operate_charwise(op, lo, hi);
    }
    /// `x`: delete `count` graphemes from under the cursor, never past the line.
    pub(super) fn delete_chars(&mut self, count: usize) {
        let (lo, hi) = {
            let buf = self.edit_buffer();
            let lo = buf.cursor_byte();
            let line_end = buf.line_start_byte(buf.cursor.line) + buf.line(buf.cursor.line).len();
            let mut hi = lo;
            for _ in 0..count {
                hi = buf.grapheme_after(hi).min(line_end);
            }
            (lo, hi)
        };
        self.operate_charwise(Op::Delete, lo, hi);
    }
    /// Apply a charwise operator to byte range `[lo, hi)`: record the text in the
    /// register, then — for delete/change — remove it; yank leaves the text and
    /// just moves the cursor to the start. Change ends in Insert mode.
    pub(super) fn operate_charwise(&mut self, op: Op, lo: usize, hi: usize) {
        if lo < hi {
            let text = self.edit_buffer().byte_slice(lo, hi);
            if op == Op::Yank {
                let buf = self.edit_buffer_mut();
                buf.cursor = buf.cursor_at_byte(lo);
            } else {
                // Route through the range-edit path so the continuous view keeps
                // its per-chapter seam bookkeeping correct (#700); a range that
                // would cross a seam is clamped there, never merging chapters.
                self.edit_replace_range(lo, hi, "");
                self.cache_focus_words();
            }
            self.store_register(Yank {
                text,
                linewise: false,
            });
        }
        if op == Op::Change {
            self.mode = Mode::Insert;
        }
    }
    /// Apply a linewise operator to the inclusive line range `[first, last]`:
    /// record the whole lines, then delete them (`dd`), clear them to one blank
    /// line and enter Insert (`cc`), or — for yank — leave them and move to the
    /// first.
    pub(super) fn operate_linewise(&mut self, op: Op, first: usize, last: usize) {
        let (text, start, end, content_end) = {
            let buf = self.edit_buffer();
            let last = last.min(buf.line_count().saturating_sub(1));
            let start = buf.line_start_byte(first);
            // The register always holds the full lines (with their newlines).
            let end = buf.next_line_byte(last);
            let content_end = buf.line_start_byte(last) + buf.line(last).len();
            (buf.byte_slice(start, end), start, end, content_end)
        };
        // Route the removal through the range-edit path so the continuous view's
        // seam bookkeeping stays correct (#700). Delete drops the full lines
        // (clamped at a seam so the chapter's final newline survives); Change
        // clears the lines' content but keeps one line to type on.
        match op {
            Op::Delete => {
                self.edit_replace_range(start, end, "");
            }
            Op::Change => {
                self.edit_replace_range(start, content_end, "");
            }
            Op::Yank => {}
        }
        self.store_register(Yank {
            text,
            linewise: true,
        });
        self.edit_move(move |b| b.move_to_line(first));
        if op != Op::Yank {
            self.cache_focus_words();
        }
        if op == Op::Change {
            self.edit_move(Buffer::move_line_start);
            self.mode = Mode::Insert;
        }
    }
    /// Replay the last `f`/`t`/`F`/`T`, in its own direction (`reverse == false`)
    /// or the opposite — Vim's `;` and `,`. Operator-aware, so `d;` works.
    pub(super) fn repeat_find(&mut self, count: usize, reverse: bool) {
        let Some(find) = self.vim_state.last_find else {
            self.vim_state.op = None;
            return;
        };
        let kind = if reverse {
            find.kind.reversed()
        } else {
            find.kind
        };
        let target = find.target;
        self.charwise(1, true, move |b| b.find_char(target, kind, count));
    }
    /// Vim `o` / `O`: open a blank line below (`above == false`) or above the
    /// current logical line and enter Insert mode there. Anchors on logical line
    /// boundaries so a soft-wrapped paragraph isn't split mid-way.
    pub(super) fn open_line(&mut self, above: bool) {
        if above {
            self.edit_move(Buffer::move_line_start);
            self.edit_insert_char('\n');
            self.edit_move_wrapped(Buffer::move_up); // onto the new blank line
        } else {
            self.edit_move(Buffer::move_line_end);
            self.edit_insert_char('\n');
        }
        self.mode = Mode::Insert;
    }
}
