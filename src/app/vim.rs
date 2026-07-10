//! Vim modal editing for the App: Normal/Visual key dispatch and the
//! operator/motion/register machinery (#208). Extracted from `mod.rs` (#314).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::buffer::{Buffer, FindKind, TextObject};
use crate::mode::{Find, LastChange, Mode, Op, VimPending, Yank};

use super::App;

impl App {
    /// Vim Normal mode (#27): keys are commands. This foundation covers entering
    /// Insert (`i`/`a`/`I`/`A`/`o`/`O`) and basic cursor motion (`hjkl` + arrows);
    /// richer motions, operators, and Visual mode are later children of #27.
    pub(super) fn on_key_normal(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            self.on_key_ctrl(key.code);
            return;
        }
        let KeyCode::Char(c) = key.code else {
            return self.on_key_normal_special(key.code);
        };

        // Resolve a multi-key command first: this char is its second key.
        match self.vim_state.pending {
            VimPending::G => {
                self.vim_state.pending = VimPending::None;
                match c {
                    'g' => {
                        let target = self
                            .vim_state
                            .count
                            .take()
                            .map_or(0, |n| n.saturating_sub(1));
                        self.goto_line(target); // gg → first line (or {count}gg); operator-aware
                    }
                    // `gq` / `gw` reflow the current paragraph to the wrap column
                    // (#1005), collapsing its hard breaks. A single-edit change, so
                    // `.` repeats it (#1004).
                    'q' | 'w' => {
                        self.vim_state.count = None;
                        self.vim_state.op = None;
                        self.reflow_paragraph();
                    }
                    // Case operators (#1006): `g~`/`gu`/`gU` begin an operator that
                    // toggles / lower-cases / upper-cases the following motion or
                    // text object. `g~~`/`guu`/`gUU` (a repeated key) act on the line.
                    '~' | 'u' | 'U' => self.case_operator(c),
                    // Transpose (#1006): `gs` swaps the two characters around the
                    // cursor (the classic `xp`); `gS` swaps two adjacent words.
                    's' => self.transpose_chars(),
                    'S' => self.transpose_words(),
                    _ => {
                        self.vim_state.count = None; // an unknown `g_` aborts
                        self.vim_state.op = None;
                    }
                }
                return;
            }
            VimPending::Find(kind) => {
                self.vim_state.pending = VimPending::None;
                let count = self.take_count();
                self.vim_state.last_find = Some(Find { kind, target: c });
                // f/t are inclusive; charwise() moves or operates (dfx) as needed.
                self.charwise(1, true, move |b| b.find_char(c, kind, count));
                return;
            }
            VimPending::Object { around } => {
                self.vim_state.pending = VimPending::None;
                self.apply_text_object(c, around);
                return;
            }
            VimPending::Register => {
                self.vim_state.pending = VimPending::None;
                if c == '+' || c == '*' {
                    // The system-clipboard registers (#811): a following `p` / `P`
                    // pastes the OS clipboard.
                    self.vim_state.clipboard_register = true;
                } else if c.is_ascii_alphabetic() {
                    self.vim_state.selected_register = Some(c.to_ascii_lowercase());
                }
                return;
            }
            // `]s` / `[s` jump to the next / previous misspelling (#359); `]r` / `[r`
            // step the active style-flag sweep (#1001); `]]` / `[[` step scene
            // heading to scene heading (#1009, Vim's own section motion); `]a` / `[a`
            // step to the next / previous OPEN annotation (#1009). Vim's own
            // bracket-nav bindings; any other second key cancels.
            VimPending::Bracket { forward } => {
                self.vim_state.pending = VimPending::None;
                self.vim_state.count = None;
                let dir = if forward { 1 } else { -1 };
                match c {
                    's' => self.navigate_spell_flag(dir),
                    'r' => self.navigate_style_flag(dir),
                    ']' if forward => self.jump_to_scene_heading(true),
                    '[' if !forward => self.jump_to_scene_heading(false),
                    'a' => self.jump_to_open_annotation(forward),
                    _ => {}
                }
                return;
            }
            // `z=` opens the spelling-suggestions pick-list for the flagged word at
            // the cursor (#647), Vim's own spell-suggest binding; any other second
            // key cancels.
            VimPending::Z => {
                self.vim_state.pending = VimPending::None;
                self.vim_state.count = None;
                if c == '=' {
                    self.suggest_word_at_cursor();
                }
                return;
            }
            // `m<x>` records a mark; `` `<x> `` / `'<x>` jumps to it, and `` `` `` /
            // `''` back-jumps through the jumplist (#825). Any other second key
            // after a jump cancels; the count is dropped either way.
            VimPending::Mark { set } => {
                self.vim_state.pending = VimPending::None;
                self.vim_state.count = None;
                if set {
                    self.set_mark(c);
                } else if c == '`' || c == '\'' {
                    self.jump_back();
                } else {
                    self.jump_to_mark(c);
                }
                return;
            }
            VimPending::None => {}
        }

        // A key bound to a workflow (#228) runs it — a standalone Normal command,
        // so only when nothing (operator, count) is pending. Checked before the
        // remap and built-ins, so a bound key overrides its default meaning, the
        // author's choice (like the #208 remaps).
        if self.vim_state.op.is_none() && self.vim_state.count.is_none() {
            if let Some(name) = self.triggers.keys.get(&c).cloned() {
                self.run_workflow_named(&name);
                return;
            }
        }

        // Past the pending second-keys (find targets, register names, object
        // ids), a command key goes through the user's remaps (#208).
        let c = self.remap_key(c);

        // A digit (other than a leading `0`, which is the line-start motion)
        // accumulates the count prefix.
        if c.is_ascii_digit() && !(c == '0' && self.vim_state.count.is_none()) {
            let d = c.to_digit(10).unwrap_or(0) as usize;
            let count = self
                .vim_state
                .count
                .unwrap_or(0)
                .saturating_mul(10)
                .saturating_add(d);
            self.vim_state.count = Some(count);
            return;
        }

        // Commands that await another key keep the count (and any operator)
        // pending.
        let prefix = match c {
            'g' => Some(VimPending::G),
            'f' => Some(VimPending::Find(FindKind::FindFwd)),
            'F' => Some(VimPending::Find(FindKind::FindBack)),
            't' => Some(VimPending::Find(FindKind::TillFwd)),
            'T' => Some(VimPending::Find(FindKind::TillBack)),
            // `"x` selects the register for the next yank / delete / put.
            '"' => Some(VimPending::Register),
            // `]s` / `[s` spell-flag navigation (#359) — only as a standalone
            // command, never as an operator motion.
            ']' if self.vim_state.op.is_none() => Some(VimPending::Bracket { forward: true }),
            '[' if self.vim_state.op.is_none() => Some(VimPending::Bracket { forward: false }),
            // `z=` opens the spelling suggestions for the flagged word (#647) — only
            // as a standalone command, never as an operator motion.
            'z' if self.vim_state.op.is_none() => Some(VimPending::Z),
            // Marks (#825): `m<x>` sets a mark; `` `<x> `` / `'<x>` jumps to it (or,
            // as `` `` `` / `''`, back through the jumplist). Standalone commands,
            // never operator motions — an operator falls through to cancel below.
            'm' if self.vim_state.op.is_none() => Some(VimPending::Mark { set: true }),
            '`' | '\'' if self.vim_state.op.is_none() => Some(VimPending::Mark { set: false }),
            _ => None,
        };
        if let Some(p) = prefix {
            self.vim_state.pending = p;
            return;
        }

        // Terminal commands consume the count.
        let raw = self.vim_state.count.take();
        let count = raw.unwrap_or(1);

        // A case operator (`g~`/`gu`/`gU`, #1006) awaiting its motion resolves its
        // own doubled key (`g~~`) linewise here and cancels on a non-motion; a true
        // motion falls through to the dispatch below, which consults `case_op`.
        if self.case_op.is_some() && self.resolve_case_prefix(c, count) {
            return;
        }

        match c {
            // Operators: `d`/`c`/`y` await a motion or text object; a repeated
            // operator (`dd`/`cc`/`yy`) acts linewise on the current line(s).
            'd' => self.operator(Op::Delete, count),
            'c' => self.operator(Op::Change, count),
            'y' => self.operator(Op::Yank, count),
            // `Y` yanks the line (Vim's default), like `yy`.
            'Y' => {
                let cur = self.edit_buffer().cursor.line;
                self.operate_linewise(Op::Yank, cur, cur + count.saturating_sub(1));
            }
            // Put the register after (`p`) / before (`P`) the cursor or line.
            'p' => self.put(true, count),
            'P' => self.put(false, count),
            // Enter Visual mode (#205): `v` characterwise, `V` linewise.
            'v' | 'V' => {
                self.mode = Mode::Visual {
                    anchor: self.edit_buffer().cursor_byte(),
                    linewise: c == 'V',
                }
            }
            // `i`/`a` are a text-object prefix while an operator (delete/change/yank
            // or a `g~`/`gu`/`gU` case op, #1006) is pending, otherwise they enter
            // Insert.
            'i' if self.vim_state.op.is_some() || self.case_op.is_some() => {
                self.vim_state.pending = VimPending::Object { around: false }
            }
            'a' if self.vim_state.op.is_some() || self.case_op.is_some() => {
                self.vim_state.pending = VimPending::Object { around: true }
            }
            'i' => self.mode = Mode::Insert,
            // Append: after the cursor's grapheme, but never past the line end.
            'a' => {
                self.edit_move(|b| {
                    if b.cursor.col < b.line_len() {
                        b.move_right();
                    }
                });
                self.mode = Mode::Insert;
            }
            'I' => {
                self.edit_move(Buffer::move_first_non_blank);
                self.mode = Mode::Insert;
            }
            'A' => {
                self.edit_move(Buffer::move_line_end);
                self.mode = Mode::Insert;
            }
            'o' => self.open_line(false),
            'O' => self.open_line(true),

            // Single-key edits.
            'x' => self.delete_chars(count),
            'D' => self.delete_to_line_end(Op::Delete),
            'C' => self.delete_to_line_end(Op::Change),
            // `J` joins the next line up; `{count}J` folds `count` lines (#1005).
            // A single-edit change, so `.` repeats it (#1004).
            'J' => self.join_lines(count),
            'u' => self.history_step(false),
            // `~`: toggle the case of the character under the cursor (#1006).
            '~' => self.vim_repeat(count, Self::toggle_case_under_cursor),

            // Character motions (charwise: move, or an operator range).
            'h' => self.charwise(count, false, Buffer::move_left),
            'l' | ' ' => self.charwise(count, false, Buffer::move_right),

            // Word motions; `cw` acts like `ce` (to the word end), as in Vim.
            'w' if matches!(self.vim_state.op, Some((Op::Change, _))) => {
                self.charwise(count, true, Buffer::move_word_end);
            }
            'w' => self.charwise(count, false, Buffer::move_word_forward),
            'b' => self.charwise(count, false, Buffer::move_word_backward),
            'e' => self.charwise(count, true, Buffer::move_word_end),

            // Paragraph motions (#821): `}` to the next blank-line boundary, `{`
            // to the previous — exclusive charwise, so `d}` cuts to there.
            '}' => self.charwise(count, false, Buffer::move_paragraph_forward),
            '{' => self.charwise(count, false, Buffer::move_paragraph_backward),

            // Line motions.
            '0' => self.charwise(1, false, Buffer::move_line_start),
            '^' => self.charwise(1, false, Buffer::move_first_non_blank),
            '$' => self.charwise(1, true, Buffer::move_line_end),

            // Linewise vertical motions (`j`/`k`) and document jump (`G`).
            'j' => self.vertical(count, true),
            'k' => self.vertical(count, false),
            'G' => {
                let target = raw.map_or_else(
                    || self.edit_buffer().last_line_index(),
                    |n| n.saturating_sub(1),
                );
                self.goto_line(target);
            }

            // Repeat the last find: `;` same direction, `,` reversed.
            ';' => self.repeat_find(count, false),
            ',' => self.repeat_find(count, true),

            // Search (#206): `/` forward, `?` backward, `n`/`N` repeat / reverse,
            // `*` the word under the cursor.
            '/' => self.open_vim_search(false),
            '?' => self.open_vim_search(true),
            'n' => self.vim_repeat(count, |s| s.vim_search_repeat(false)),
            'N' => self.vim_repeat(count, |s| s.vim_search_repeat(true)),
            '*' => self.vim_search_word(),

            // Ex command line (#207): `:w`, `:q`, `:wq`, …
            ':' => self.open_ex_command(),

            // Anything else cancels a dangling operator.
            _ => self.vim_state.op = None,
        }

        // A `"x` register selection only lives until the operator/put that uses
        // it. If this key completed a command (nothing left pending), drop it.
        if self.vim_state.op.is_none() && self.vim_state.pending == VimPending::None {
            self.vim_state.selected_register = None;
            self.vim_state.clipboard_register = false;
        }
    }

    /// `d` / `c`: begin an operator awaiting a motion, or — when the same
    /// operator is already pending (`dd`/`cc`) — act linewise on the current
    /// line(s). `count` is this keypress's count.
    fn operator(&mut self, op: Op, count: usize) {
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
    fn apply_text_object(&mut self, id: char, around: bool) {
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
    fn charwise(&mut self, count: usize, inclusive: bool, step: impl Fn(&mut Buffer)) {
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
    fn vertical(&mut self, count: usize, down: bool) {
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
    fn goto_line(&mut self, target: usize) {
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
    fn delete_to_line_end(&mut self, op: Op) {
        let buf = self.edit_buffer();
        let lo = buf.cursor_byte();
        let hi = buf.line_start_byte(buf.cursor.line) + buf.line(buf.cursor.line).len();
        self.operate_charwise(op, lo, hi);
    }

    /// `x`: delete `count` graphemes from under the cursor, never past the line.
    fn delete_chars(&mut self, count: usize) {
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
    fn operate_charwise(&mut self, op: Op, lo: usize, hi: usize) {
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
    fn operate_linewise(&mut self, op: Op, first: usize, last: usize) {
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

    /// Record a yank into the unnamed register, and into the named register if
    /// `"x` selected one. Also mirror the text onto the system clipboard (#811),
    /// so a Vim yank / delete or a Visual copy is available to paste into another
    /// app — the internal registers keep working exactly as before, the clipboard
    /// is an additional destination.
    fn store_register(&mut self, yank: Yank) {
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
    fn put(&mut self, after: bool, count: usize) {
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

    /// Vim Visual mode (#205): motions extend the selection; `d`/`c`/`y` act on
    /// it and leave Visual. Motion handling is reused from Normal mode (which just
    /// moves the cursor here, since no operator is pending).
    pub(super) fn on_key_visual(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            self.on_key_ctrl(key.code);
            return;
        }
        let Mode::Visual { anchor, linewise } = self.mode else {
            return;
        };
        // Mid multi-key motion (an `f`/`g` second key, or a `"` register name):
        // let the Normal handler resolve it, extending the selection.
        if self.vim_state.pending != VimPending::None {
            self.on_key_normal(key);
            return;
        }
        // Non-character keys (Esc, arrows) handled directly; command chars go
        // through the user's remaps (#208) first.
        let c = match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Char(c) => self.remap_key(c),
            _ => return self.on_key_normal(key), // arrows / Home / End extend the selection
        };
        match c {
            // `v` / `V` switch the selection kind, or leave Visual if already it.
            'v' | 'V' => {
                let want_line = c == 'V';
                self.mode = if linewise == want_line {
                    Mode::Normal
                } else {
                    Mode::Visual {
                        anchor,
                        linewise: want_line,
                    }
                };
            }
            // `o` swaps the fixed and active ends of the selection.
            'o' => {
                let cur = self.edit_buffer().cursor_byte();
                let buf = self.edit_buffer_mut();
                buf.cursor = buf.cursor_at_byte(anchor);
                self.mode = Mode::Visual {
                    anchor: cur,
                    linewise,
                };
            }
            // Operators act on the selection, then leave Visual (Insert for `c`).
            'd' | 'x' => self.visual_operate(Op::Delete, anchor, linewise),
            'c' | 's' => self.visual_operate(Op::Change, anchor, linewise),
            'y' => self.visual_operate(Op::Yank, anchor, linewise),
            // `p` replaces the selection with the register.
            'p' => self.visual_put(anchor, linewise),
            // `~` / `u` / `U` case-map the selection, then leave Visual (#1006).
            '~' | 'u' | 'U' => self.visual_case(c, anchor, linewise),
            // `m` annotates the selection with a note (#302), then leaves Visual.
            'm' => {
                let (start, end) = self.selection_bytes(anchor, linewise);
                self.mode = Mode::Normal;
                self.start_annotation_range(start, end);
            }
            // Motions (and counts / multi-key prefixes) extend the selection by
            // reusing the Normal-mode dispatch (which remaps again to the same key).
            _ if is_motion_key(c) => self.on_key_normal(key),
            _ => {} // ignore anything else, so nothing slips into Insert / edits
        }
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
    fn visual_operate(&mut self, op: Op, anchor: usize, linewise: bool) {
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
    fn visual_put(&mut self, anchor: usize, linewise: bool) {
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

    /// Normal-mode handling for non-character keys: the arrow keys move (count-
    /// aware), Home/End go to the row edges, and Esc clears any pending command.
    fn on_key_normal_special(&mut self, code: KeyCode) {
        let count = self.take_count();
        // The arrow keys mirror hjkl, so they compose with a pending operator too
        // (`d<Down>` works like `dj`).
        match code {
            KeyCode::Left => self.charwise(count, false, Buffer::move_left),
            KeyCode::Right => self.charwise(count, false, Buffer::move_right),
            KeyCode::Up => self.vertical(count, false),
            KeyCode::Down => self.vertical(count, true),
            KeyCode::Home => self.edit_move_wrapped(Buffer::move_home),
            KeyCode::End => self.edit_move_wrapped(Buffer::move_end),
            _ => {} // Esc and the rest: the count is already cleared above.
        }
        // Esc / Home / End (and the rest) also cancel a pending operator/command;
        // the motion arrows already consumed any operator via charwise/vertical.
        self.vim_state.pending = VimPending::None;
        self.vim_state.op = None;
        self.case_op = None; // a dangling `g~`/`gu`/`gU` (#1006) cancels too
    }

    /// Take the pending count, defaulting to 1, and clear it.
    fn take_count(&mut self) -> usize {
        self.vim_state.count.take().unwrap_or(1)
    }

    /// Translate a pressed key through the user's remaps (#208): a remapped key
    /// runs the default key's action; an unmapped key is itself. Applied to
    /// command keys only — not to find targets, register names, or object ids.
    fn remap_key(&self, c: char) -> char {
        self.vim_keys.get(&c).copied().unwrap_or(c)
    }

    /// Run `motion` `n` times — how a count repeats a simple motion.
    fn vim_repeat(&mut self, n: usize, mut motion: impl FnMut(&mut Self)) {
        for _ in 0..n {
            motion(self);
        }
    }

    /// Replay the last `f`/`t`/`F`/`T`, in its own direction (`reverse == false`)
    /// or the opposite — Vim's `;` and `,`. Operator-aware, so `d;` works.
    fn repeat_find(&mut self, count: usize, reverse: bool) {
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

    // --- Dot-repeat (`.`) recording and replay (#1004) ---------------------

    /// Run one editing-mode key through `handle`, recording it into the
    /// dot-repeat buffer when it's part of a change command (#1004). The single
    /// choke point every Normal / Insert / Visual key passes through, so a change
    /// is captured no matter how many keystrokes (and mode hops) compose it.
    ///
    /// The rule mirrors Vim's: while a change is being *composed* (an operator or
    /// its motion is pending, or we're in Insert mode after `i`/`a`/`o`/`c…`)
    /// every key is appended; the moment the command settles back into a resting
    /// Normal state, the composed keys are committed as the last change *iff* they
    /// edited the buffer. A pure motion or a cancelled operator edits nothing, so
    /// it's discarded and the previous `.` target survives.
    pub(super) fn dot_record_around(
        &mut self,
        key: KeyEvent,
        handle: impl FnOnce(&mut Self, KeyEvent),
    ) {
        // A replay drives the same handlers; don't record the keys it feeds back.
        if self.vim_state.dot.replaying {
            handle(self, key);
            return;
        }

        let was_composing = self.dot_composing();
        // Begin a fresh recording when a resting Normal-mode key might start a
        // change; continue an in-progress one. A key pressed while at rest that
        // turns out to be a bare motion is recorded then discarded on settle.
        if !was_composing && matches!(self.mode, Mode::Normal) {
            self.vim_state.dot.pending.clear();
            self.vim_state.dot.recording = true;
            self.vim_state.dot.start_seq = self.edit_buffer().edit_seq();
        }
        if self.vim_state.dot.recording {
            self.vim_state.dot.pending.push(key);
        }

        handle(self, key);

        // Still composing (operator/pending awaiting a key, or now in Insert):
        // keep collecting until the command settles.
        if self.dot_composing() {
            return;
        }
        self.dot_settle();
    }

    /// Whether a change command is mid-composition: an operator or a multi-key
    /// command is awaiting its next key, or we're in Insert mode (an `i`/`c…`
    /// session runs until `Esc`). Visual mode is a selection being built, not a
    /// committed change, so it doesn't count as composing here.
    fn dot_composing(&self) -> bool {
        matches!(self.mode, Mode::Insert)
            || self.vim_state.op.is_some()
            || self.case_op.is_some() // a `g~`/`gu`/`gU` awaiting its motion (#1006)
            || self.vim_state.pending != VimPending::None
    }

    /// A composed command has settled back into resting Normal mode: commit its
    /// keys as the last change if it edited the buffer, else drop them.
    fn dot_settle(&mut self) {
        if !self.vim_state.dot.recording {
            return;
        }
        self.vim_state.dot.recording = false;
        let edited = self.edit_buffer().edit_seq() != self.vim_state.dot.start_seq;
        let keys = std::mem::take(&mut self.vim_state.dot.pending);
        if edited && !keys.is_empty() {
            self.vim_state.dot.last = Some(LastChange { keys });
        }
    }

    /// Vim `.`: replay the last recorded change at the cursor as one undo step.
    /// Re-feeds the recorded keystrokes through the normal dispatch, so every
    /// change command repeats through exactly the code that first ran it. A no-op
    /// when nothing has been changed yet. `count` replays it that many times
    /// (Vim's `{count}.`), all folded into the single undo step.
    pub(super) fn dot_repeat(&mut self, count: usize) {
        let Some(change) = self.vim_state.dot.last.clone() else {
            return;
        };
        // Seal the current undo group so the replay's edits open a fresh one; it
        // stays open across the whole (synchronous, no idle tick) replay, so the
        // repeated change undoes in a single step — like the original edit.
        self.edit_buffer_mut().seal_undo_group();
        self.note_annotation_undo_point();

        self.vim_state.dot.replaying = true;
        for _ in 0..count.max(1) {
            for key in change.keys.iter().copied() {
                match self.mode {
                    Mode::Insert => self.on_key_insert(key),
                    Mode::Normal => self.on_key_normal(key),
                    Mode::Visual { .. } => self.on_key_visual(key),
                    _ => {}
                }
            }
        }
        // A replayed insert-session change (`i…`/`c…`) leaves Insert mode only if
        // the recorded `Esc` was replayed; it was, since recording ran through the
        // settling `Esc`. Clear any transient Vim state the replay left pending.
        self.vim_state.dot.replaying = false;
        self.vim_state.op = None;
        self.case_op = None;
        self.vim_state.pending = VimPending::None;
        // Close the replay's undo group so a following edit (a keystroke or a
        // further `.`) is its own step, not folded into this repeat.
        self.edit_buffer_mut().seal_undo_group();
    }

    /// Vim `o` / `O`: open a blank line below (`above == false`) or above the
    /// current logical line and enter Insert mode there. Anchors on logical line
    /// boundaries so a soft-wrapped paragraph isn't split mid-way.
    fn open_line(&mut self, above: bool) {
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

/// Whether `c` is a Normal-mode motion key — a count digit, a motion, or a
/// multi-key motion prefix (`g`/`f`/`"`). These are the keys Visual mode forwards
/// to the Normal handler to extend the selection. Keep in sync with the motion
/// arms of [`App::on_key_normal`]; a new motion added there must be listed here
/// to work in Visual mode too (forwarding fails safe — an omission just means the
/// motion doesn't extend a selection, never a stray edit).
pub(super) const fn is_motion_key(c: char) -> bool {
    c.is_ascii_digit()
        || matches!(
            c,
            'h' | 'j'
                | 'k'
                | 'l'
                | ' '
                | 'w'
                | 'b'
                | 'e'
                | '0'
                | '^'
                | '$'
                | 'g'
                | 'G'
                | 'f'
                | 'F'
                | 't'
                | 'T'
                | ';'
                | ','
                | '"'
                | '{'
                | '}'
        )
}
