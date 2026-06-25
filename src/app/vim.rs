//! Vim modal editing for the App: Normal/Visual key dispatch and the
//! operator/motion/register machinery (#208). Extracted from `mod.rs` (#314).



use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};


use crate::buffer::{Buffer, FindKind, TextObject};
use crate::mode::{
    Find, Mode, Op, VimPending, Yank,
};

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
                if c == 'g' {
                    let target = self.vim_state.count.take().map_or(0, |n| n.saturating_sub(1));
                    self.goto_line(target); // gg → first line (or {count}gg); operator-aware
                } else {
                    self.vim_state.count = None; // an unknown `g_` aborts
                    self.vim_state.op = None;
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
                if c.is_ascii_alphabetic() {
                    self.vim_state.selected_register = Some(c.to_ascii_lowercase());
                }
                return;
            }
            // `]s` / `[s` jump to the next / previous misspelling (#359), Vim's
            // own spell-nav bindings; any other second key cancels.
            VimPending::Bracket { forward } => {
                self.vim_state.pending = VimPending::None;
                self.vim_state.count = None;
                if c == 's' {
                    self.navigate_spell_flag(if forward { 1 } else { -1 });
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
            let count = self.vim_state.count.unwrap_or(0).saturating_mul(10).saturating_add(d);
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
            _ => None,
        };
        if let Some(p) = prefix {
            self.vim_state.pending = p;
            return;
        }

        // Terminal commands consume the count.
        let raw = self.vim_state.count.take();
        let count = raw.unwrap_or(1);
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
            'v' | 'V' => self.mode = Mode::Visual { anchor: self.edit_buffer().cursor_byte(), linewise: c == 'V' },
            // `i`/`a` are a text-object prefix while an operator is pending,
            // otherwise they enter Insert.
            'i' if self.vim_state.op.is_some() => self.vim_state.pending = VimPending::Object { around: false },
            'a' if self.vim_state.op.is_some() => self.vim_state.pending = VimPending::Object { around: true },
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
            'u' => self.history_step(false),

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

            // Line motions.
            '0' => self.charwise(1, false, Buffer::move_line_start),
            '^' => self.charwise(1, false, Buffer::move_first_non_blank),
            '$' => self.charwise(1, true, Buffer::move_line_end),

            // Linewise vertical motions (`j`/`k`) and document jump (`G`).
            'j' => self.vertical(count, true),
            'k' => self.vertical(count, false),
            'G' => {
                let target = raw.map_or_else(|| self.edit_buffer().last_line_index(), |n| n.saturating_sub(1));
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
            _ => None,
        };
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
        self.vim_state.op.take().map(|(op, opc)| (op, count.saturating_mul(opc)))
    }

    /// A charwise motion: move the cursor, or — when an operator is pending —
    /// delete/change the span from the cursor to where the motion lands.
    /// `inclusive` covers the landing grapheme (Vim's `e`, `$`, `f`, `t`).
    fn charwise(&mut self, count: usize, inclusive: bool, step: impl Fn(&mut Buffer)) {
        if let Some((op, n)) = self.take_op(count) {
            let (lo, hi) = {
                let buf = self.edit_buffer_mut();
                let start = buf.cursor_byte();
                let saved = buf.cursor;
                for _ in 0..n {
                    step(buf);
                }
                let target = buf.cursor_byte();
                buf.cursor = saved;
                let hi = start.max(target);
                (start.min(target), if inclusive { buf.grapheme_after(hi) } else { hi })
            };
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
        if let Some((op, n)) = self.take_op(count) {
            let cur = self.edit_buffer().cursor.line;
            let other = if down { cur + n } else { cur.saturating_sub(n) };
            self.operate_linewise(op, cur.min(other), cur.max(other));
        } else {
            let mv = if down { Buffer::move_down } else { Buffer::move_up };
            self.vim_repeat(count, move |s| s.edit_move_wrapped(mv));
        }
    }

    /// Jump to `target` (0-based line): move the cursor, or — with an operator
    /// pending (`dG`, `dgg`) — act linewise over the lines between.
    fn goto_line(&mut self, target: usize) {
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
        if self.guard_vim_edit() {
            return;
        }
        if lo < hi {
            let text = {
                let buf = self.edit_buffer_mut();
                let text = buf.byte_slice(lo, hi);
                if op == Op::Yank {
                    buf.cursor = buf.cursor_at_byte(lo);
                } else {
                    buf.replace_byte_range(lo, hi, "");
                }
                text
            };
            self.store_register(Yank { text, linewise: false });
            if op != Op::Yank {
                self.cache_focus_words();
            }
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
        if self.guard_vim_edit() {
            return;
        }
        let text = {
            let buf = self.edit_buffer_mut();
            let last = last.min(buf.line_count().saturating_sub(1));
            let start = buf.line_start_byte(first);
            // The register always holds the full lines (with their newlines).
            let end = buf.next_line_byte(last);
            let text = buf.byte_slice(start, end);
            match op {
                Op::Delete => buf.replace_byte_range(start, end, ""),
                Op::Change => {
                    // Clear the lines' content but keep one line to type on.
                    let content_end = buf.line_start_byte(last) + buf.line(last).len();
                    buf.replace_byte_range(start, content_end, "");
                }
                Op::Yank => {}
            }
            text
        };
        self.store_register(Yank { text, linewise: true });
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
    /// `"x` selected one.
    fn store_register(&mut self, yank: Yank) {
        if let Some(name) = self.vim_state.selected_register.take() {
            self.vim_state.registers.named.insert(name, yank.clone());
        }
        self.vim_state.registers.unnamed = Some(yank);
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
        if self.guard_vim_edit() {
            return;
        }
        let Some(yank) = self.active_register() else { return };
        let body = yank.text.repeat(count);
        let buf = self.edit_buffer_mut();
        if yank.linewise {
            let line = buf.cursor.line;
            let at = if after { buf.next_line_byte(line) } else { buf.line_start_byte(line) };
            buf.replace_byte_range(at, at, &body);
            buf.move_to_line(if after { line + 1 } else { line });
        } else {
            let line_end = buf.line_start_byte(buf.cursor.line) + buf.line(buf.cursor.line).len();
            let at = if after {
                buf.grapheme_after(buf.cursor_byte()).min(line_end)
            } else {
                buf.cursor_byte()
            };
            buf.replace_byte_range(at, at, &body);
            buf.move_left(); // land on the last grapheme put, as Vim does
        }
        self.cache_focus_words();
    }

    /// Vim editing commands need the per-chapter buffer; like undo (#145), they're
    /// guarded out of the continuous view, whose segment bookkeeping a raw range
    /// edit would desync. Returns whether the command was blocked.
    fn guard_vim_edit(&mut self) -> bool {
        if self.manuscript.is_some() {
            self.message = Some("Editing commands aren't available in the continuous view — Ctrl-l to return.".into());
            true
        } else {
            false
        }
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
                    Mode::Visual { anchor, linewise: want_line }
                };
            }
            // `o` swaps the fixed and active ends of the selection.
            'o' => {
                let cur = self.edit_buffer().cursor_byte();
                let buf = self.edit_buffer_mut();
                buf.cursor = buf.cursor_at_byte(anchor);
                self.mode = Mode::Visual { anchor: cur, linewise };
            }
            // Operators act on the selection, then leave Visual (Insert for `c`).
            'd' | 'x' => self.visual_operate(Op::Delete, anchor, linewise),
            'c' | 's' => self.visual_operate(Op::Change, anchor, linewise),
            'y' => self.visual_operate(Op::Yank, anchor, linewise),
            // `p` replaces the selection with the register.
            'p' => self.visual_put(anchor, linewise),
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
        if self.guard_vim_edit() {
            return;
        }
        let Some(yank) = self.active_register() else {
            self.mode = Mode::Normal;
            return;
        };
        let (lo, hi) = self.selection_bytes(anchor, linewise);
        let buf = self.edit_buffer_mut();
        buf.replace_byte_range(lo, hi, &yank.text);
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
    fn vim_repeat(&mut self, n: usize, mut motion: impl FnMut(&mut App)) {
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
        let kind = if reverse { find.kind.reversed() } else { find.kind };
        let target = find.target;
        self.charwise(1, true, move |b| b.find_char(target, kind, count));
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
fn is_motion_key(c: char) -> bool {
    c.is_ascii_digit()
        || matches!(
            c,
            'h' | 'j' | 'k' | 'l' | ' ' | 'w' | 'b' | 'e' | '0' | '^' | '$' | 'g' | 'G' | 'f' | 'F'
                | 't' | 'T' | ';' | ',' | '"'
        )
}
