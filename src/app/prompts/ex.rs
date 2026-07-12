//! The Vim Ex command line (#207) and the go-to-line jump (#1010): opening the
//! `:` prompt, running a typed command, the bare-line-number and the ranged
//! `:s///` substitute cases (`:s`, `:%s`, `:{N},{M}s`, …; #826/#1106), and the
//! shared clamp+move+reveal both `:{N}` and the Ctrl-G prompt funnel through.
//! Split out of `prompts/mod.rs`; a child module, so `App`'s private fields stay
//! reachable via `use super::super::*`.

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
            // Substitute with an optional line range (#826, #1106): `:s/…` (current
            // line), `:%s/…` (whole buffer), `:{N}s/…`, `:{N},{M}s/…`, `:.,$s/…`.
            _ if is_substitute(cmd) => self.ex_substitute_ranged(cmd),
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

    /// A substitute with an optional line range (#826, #1106): splits the leading
    /// range off the command (`%`, `.`, `$`, `{N}`, `{N},{M}`, `.,$`, …), resolves
    /// it to a 0-based inclusive line span, then substitutes within it. A bare `:s`
    /// (no range) acts on the current line, as Vim does. An unparseable range is
    /// reported rather than acted on.
    fn ex_substitute_ranged(&mut self, cmd: &str) {
        // Everything up to the `s` is the range; the rest (`/pat/rep/flags`) is
        // the substitute body. `is_substitute` guaranteed an `s` is present.
        let Some(s_at) = cmd.find('s') else { return };
        let (range, rest) = (&cmd[..s_at], &cmd[s_at + 1..]);
        let Some((first, last)) = self.resolve_line_range(range) else {
            self.message = Some(format!("Not an editor command: {cmd}"));
            return;
        };
        self.ex_substitute(first, last, rest);
    }

    /// Resolve an Ex line-range prefix to a 0-based inclusive `(first, last)` span,
    /// clamped to the buffer (#1106). Understands an empty range (the current
    /// line), `%` (the whole buffer), and comma-separated endpoints each of `.`
    /// (current line), `$` (last line), or a 1-based number. `None` on anything
    /// malformed.
    fn resolve_line_range(&self, range: &str) -> Option<(usize, usize)> {
        let cur = self.edit_buffer().cursor.line;
        let last = self.edit_buffer().last_line_index();
        if range.is_empty() {
            return Some((cur, cur));
        }
        if range == "%" {
            return Some((0, last));
        }
        let endpoint = |tok: &str| -> Option<usize> {
            match tok {
                "." => Some(cur),
                "$" => Some(last),
                _ => tok
                    .parse::<usize>()
                    .ok()
                    .map(|n| n.saturating_sub(1).min(last)),
            }
        };
        let mut parts = range.splitn(2, ',');
        let a = endpoint(parts.next()?)?;
        let b = match parts.next() {
            Some(tok) => endpoint(tok)?,
            None => a,
        };
        Some((a.min(b), a.max(b)))
    }

    /// Substitute `pat` with `rep` across the 0-based inclusive line span
    /// `[first, last]`, reusing the range-edit path find/replace goes through
    /// (#826, #1106). The `g` flag replaces every match on each line; without it,
    /// only the first per line (Vim's default). A missing trailing delimiter is
    /// tolerated. Reports the number of substitutions.
    fn ex_substitute(&mut self, first: usize, last: usize, rest: &str) {
        // The delimiter is the first character after `s` (Vim allows any; `/` is by
        // far the most common). An empty argument is a malformed command.
        let Some(delim) = rest.chars().next() else {
            self.message = Some("Not an editor command: s".into());
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

        // Restrict the search to the requested line span's byte range, so matches
        // outside it are left alone (a `:%s` passes the whole buffer here).
        let (lo, hi) = {
            let buf = self.edit_buffer();
            (buf.line_start_byte(first), buf.next_line_byte(last))
        };
        // Match over the span, then keep either every match (`g`) or the first on
        // each line (Vim's per-line default). Substitute is case-sensitive by
        // default, like Vim without `ignorecase`.
        let text = self.edit_buffer().byte_slice(lo, hi);
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
        // Offsets are span-relative, so shift them back by `lo` into the buffer.
        let mut replaced = 0usize;
        for &(start, end) in targets.iter().rev() {
            self.edit_replace_range(lo + start, lo + end, replacement);
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

/// Whether `cmd` is a substitute command (#1106): an optional line-range prefix
/// (digits, `%`, `.`, `$`, `,`) followed by `s` and a delimiter. The prefix chars
/// are the ones [`App::resolve_line_range`] understands; the `s` and a following
/// delimiter distinguish `:s/…` from a bare `:s` typo or a `:set`-style command
/// (unsupported here).
fn is_substitute(cmd: &str) -> bool {
    let Some(s_at) = cmd.find('s') else {
        return false;
    };
    let range = &cmd[..s_at];
    let range_ok = range
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '%' | '.' | '$' | ','));
    // A delimiter must follow the `s` (e.g. `s/`), so `:s` alone or `:set` don't
    // match. The delimiter is any non-alphanumeric character, as in Vim.
    let has_delim = cmd[s_at + 1..]
        .chars()
        .next()
        .is_some_and(|d| !d.is_alphanumeric());
    range_ok && has_delim
}
