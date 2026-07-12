//! Vim modal editing for the App: Normal/Visual key dispatch (#208). Extracted
//! from `mod.rs` (#314); the operator/motion, register/put, and dot-repeat
//! machinery live in sibling submodules, alongside the case affordances in
//! `vim_case.rs`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::buffer::{Buffer, FindKind};
use crate::mode::{Find, Mode, Op, VimPending};

use super::App;

mod dot;
mod operators;
mod registers;

impl App {
    pub(crate) fn on_key_normal(&mut self, key: KeyEvent) {
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
            // `{count}r<char>` (#1106): replace the `count` characters under the
            // cursor with `char`. Resolved here as the pending second key so the
            // dot-repeat recorder captures the whole command (`.` then repeats it).
            VimPending::Replace { count } => {
                self.vim_state.pending = VimPending::None;
                self.replace_chars(count, c);
                return;
            }
            // `>>` / `<<` (#1106): the doubled key resolves the linewise indent /
            // dedent over `count` lines; any other second key cancels.
            VimPending::Indent { dedent, count } => {
                self.vim_state.pending = VimPending::None;
                if c == if dedent { '<' } else { '>' } {
                    self.indent_lines(dedent, count);
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
            // `{count}r` (#1106): await the replacement char, then swap the `count`
            // characters under the cursor for it. Awaiting a key, so the count (and
            // the dot recording) stay pending until the char arrives.
            'r' => self.vim_state.pending = VimPending::Replace { count },
            // `S` (#1106): substitute the whole line — change it, like `cc`. Vim's
            // standard binding (transpose stays on `gs`/`gS`, which never conflicted).
            'S' => {
                let cur = self.edit_buffer().cursor.line;
                self.operate_linewise(Op::Change, cur, cur + count.saturating_sub(1));
            }
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

            // `%` (#1106): jump to the matching bracket. Operator-aware and
            // inclusive, so `d%` / `c%` cut the whole pair, matching Vim.
            '%' => self.match_pair(),

            // `>>` / `<<` (#1106): await the doubled key, then indent / dedent
            // `count` lines a level. Awaiting a key, so the dot recording keeps
            // composing until the second `>` / `<` arrives — `.` repeats it.
            '>' => {
                self.vim_state.pending = VimPending::Indent {
                    dedent: false,
                    count,
                }
            }
            '<' => {
                self.vim_state.pending = VimPending::Indent {
                    dedent: true,
                    count,
                }
            }

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
    /// Vim Visual mode (#205): motions extend the selection; `d`/`c`/`y` act on
    /// it and leave Visual. Motion handling is reused from Normal mode (which just
    /// moves the cursor here, since no operator is pending).
    pub(crate) fn on_key_visual(&mut self, key: KeyEvent) {
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
    pub(super) fn vim_repeat(&mut self, n: usize, mut motion: impl FnMut(&mut Self)) {
        for _ in 0..n {
            motion(self);
        }
    }
}

/// Whether `c` is a Normal-mode motion key — a count digit, a motion, or a
/// multi-key motion prefix (`g`/`f`/`"`). These are the keys Visual mode forwards
/// to the Normal handler to extend the selection. Keep in sync with the motion
/// arms of [`App::on_key_normal`]; a new motion added there must be listed here
/// to work in Visual mode too (forwarding fails safe — an omission just means the
/// motion doesn't extend a selection, never a stray edit).
pub(in crate::app) const fn is_motion_key(c: char) -> bool {
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
                | '%'
        )
}
