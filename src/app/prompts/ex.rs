//! The Vim Ex command line (#207) and the go-to-line jump (#1010): opening the
//! `:` prompt, running a typed command, the bare-line-number and `:%s///`
//! substitute cases, and the shared clamp+move+reveal both `:{N}` and the
//! Ctrl-G prompt funnel through. Split out of `prompts/mod.rs`; a child module,
//! so `App`'s private fields stay reachable via `use super::super::*`.

use super::super::*;

use crate::mode::Pending;
use crate::text::{match_ranges_opts, MatchOpts};

impl App {
    /// Open the Vim Ex command line (#207): `:` then a command (`w`/`q`/`wq`/…).
    pub(in crate::app) fn open_ex_command(&mut self) {
        self.vim_state.op = None;
        self.open_prompt(Pending::ExCommand, ":", "", None);
    }

    /// Run a typed Ex command (#207). The supported set is the file commands a
    /// Vim user reaches for first, plus a bare line number (`:{N}`) and a
    /// whole-buffer substitute (`:%s/pat/rep/[g]`, #826); anything else is
    /// reported, like Vim's E492.
    pub(super) fn run_ex_command(&mut self, cmd: &str) {
        match cmd {
            "" => {} // bare `:` — nothing to do
            "w" => self.save_edit(),
            // In a split, `:q` closes the focused pane (Vim's window `:q`); only with
            // no split does it quit — prompting on unsaved changes, like Ctrl-Q.
            "q" => {
                if self.split.is_some() {
                    self.close_focused_pane(false);
                } else {
                    self.request_quit();
                }
            }
            // Write then close the focused pane (in a split) or quit; don't proceed if
            // the write failed (buffer still dirty).
            "wq" | "x" => {
                self.save_edit();
                if !self.edit_dirty() {
                    if self.split.is_some() {
                        self.close_focused_pane(false);
                    } else {
                        self.should_quit = true;
                    }
                }
            }
            "q!" => {
                if self.split.is_some() {
                    self.close_focused_pane(true); // discard the focused pane's edits
                } else {
                    self.clear_recovery(); // explicit discard — don't re-offer next launch (#263)
                    self.should_quit = true;
                }
            }
            // `:{N}` — jump to line N (1-based, clamped to the buffer), like Vim.
            _ if !cmd.is_empty() && cmd.bytes().all(|b| b.is_ascii_digit()) => {
                self.ex_goto_line(cmd);
            }
            // `:%s/pat/rep/[flags]` — whole-buffer substitute (#826).
            _ if cmd.starts_with("%s") => self.ex_substitute(&cmd[2..]),
            other => self.message = Some(format!("Not an editor command: {other}")),
        }
    }

    /// Open the go-to-line prompt (#1010): a direct "jump to line N" outside Vim's
    /// `:N`, reachable from the run-action menu (and its `Ctrl-G` shortcut). Numeric
    /// input, prefilled with the current line so the author can nudge from where
    /// they are.
    pub(crate) fn open_go_to_line(&mut self) {
        let current = self.edit_buffer().cursor.line + 1; // 1-based, like the prompt asks for
        self.open_prompt(
            Pending::GoToLine,
            "Go to line: ",
            &current.to_string(),
            None,
        );
    }

    /// Resolve the go-to-line prompt's entry (#1010): a 1-based line number moves
    /// the cursor there (clamped to the last content line, first non-blank column,
    /// like Vim's `{count}G`), pushing the jumplist first so `` `` `` returns. A
    /// non-numeric entry is reported; out-of-range clamps rather than failing.
    pub(super) fn go_to_line_input(&mut self, text: &str) {
        let trimmed = text.trim();
        let Ok(n) = trimmed.parse::<usize>() else {
            self.message = Some(format!("Not a line number: {trimmed}"));
            return;
        };
        // Record where we left so `` `` `` returns, like any big jump (#825).
        self.push_jump();
        self.goto_line_1based(n);
    }

    /// `:{N}` — move the cursor to 1-based line `n`, clamped to the buffer's last
    /// content line, landing on its first non-blank column like Vim's `{count}G`.
    fn ex_goto_line(&mut self, digits: &str) {
        // Only reached with an all-digit `cmd`, but a value too large for a
        // `usize` still means "past the end", so clamp to the last line.
        let n: usize = digits.parse().unwrap_or(usize::MAX);
        self.goto_line_1based(n);
    }

    /// Move the cursor to 1-based line `n`, clamped to the buffer's last content
    /// line (skipping a trailing newline's empty line, like Vim's `{count}G`),
    /// landing on its first non-blank column — then reveal any collapsed section
    /// the landing sits inside so the cursor is never hidden (#975). The single
    /// clamp+move+reveal both `:{N}` and the Ctrl-G prompt (#1010) funnel through,
    /// so the reveal can't drift out of one of them again. Reports when `n` ran
    /// past the end (both callers want the note).
    fn goto_line_1based(&mut self, n: usize) {
        let target = n.saturating_sub(1); // 1-based → 0-based
        let last = self.edit_buffer().last_line_index();
        self.edit_buffer_mut().move_to_line(target.min(last));
        // The target line can sit inside a collapsed section (#975); reveal it so
        // the cursor lands somewhere visible.
        self.reveal_fold_at_cursor();
        if target > last {
            self.message = Some(format!("Past the end — moved to line {}.", last + 1));
        }
    }

    /// `:%s/pat/rep/[flags]` — substitute `pat` with `rep` across the whole
    /// buffer, reusing the range-edit path find/replace goes through (#826). The
    /// `g` flag replaces every match on each line; without it, only the first per
    /// line (Vim's default). A missing trailing delimiter is tolerated. Reports
    /// the number of substitutions.
    fn ex_substitute(&mut self, rest: &str) {
        // The delimiter is the first character after `%s` (Vim allows any; `/`
        // is by far the most common). An empty argument is a malformed command.
        let Some(delim) = rest.chars().next() else {
            self.message = Some("Not an editor command: %s".into());
            return;
        };
        // Split into pattern / replacement / flags on the delimiter. A trailing
        // delimiter (and thus the flags field) may be omitted.
        let body = &rest[delim.len_utf8()..];
        let mut parts = body.splitn(3, delim);
        let pattern = parts.next().unwrap_or("");
        let replacement = parts.next().unwrap_or("");
        let flags = parts.next().unwrap_or("");
        if pattern.is_empty() {
            self.message = Some("E33: No previous substitute pattern".into());
            return;
        }
        let global = flags.contains('g');

        // Match over the whole buffer, then keep either every match (`g`) or the
        // first on each line (Vim's per-line default). `%s` is case-sensitive by
        // default, like Vim without `ignorecase`.
        let text = self.edit_buffer().text();
        let opts = MatchOpts {
            case_sensitive: true,
            whole_word: false,
            regex: false,
        };
        let ranges = match_ranges_opts(&text, pattern, opts);
        let targets: Vec<(usize, usize)> = if global {
            ranges
        } else {
            let mut last_line: Option<usize> = None;
            ranges
                .into_iter()
                .filter(|&(start, _)| {
                    let line = text[..start].bytes().filter(|&b| b == b'\n').count();
                    if last_line == Some(line) {
                        false
                    } else {
                        last_line = Some(line);
                        true
                    }
                })
                .collect()
        };

        // Replace right-to-left so earlier byte offsets stay valid as the text
        // shifts — the same range-edit path find/replace uses, so it is undoable.
        let mut replaced = 0usize;
        for &(start, end) in targets.iter().rev() {
            self.edit_replace_range(start, end, replacement);
            replaced += 1;
        }
        if replaced > 0 {
            self.cache_focus_words();
        }
        self.message = Some(match replaced {
            0 => format!("Pattern not found: {pattern}"),
            1 => "Replaced 1 occurrence.".into(),
            n => format!("Replaced {n} occurrences."),
        });
    }
}
