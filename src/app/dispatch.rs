//! Top-level key dispatch: the terminal-event entry point, the mode router that
//! sends each key to its handler, and the editor `Ctrl-`/Insert-mode handling.
//! Split out of [`super`] (#759) to keep `app/mod.rs` under the ~1000-line
//! convention; these are `impl App` methods, so the split is a pure move.

use std::collections::HashMap;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::buffer::Buffer;
use crate::mode::{Action, Mode, Pending, VimPending};

use super::App;

impl App {
    /// Whether Esc has no editing job right now, so a global Esc (leaving
    /// composition mode) may claim it: Insert mode
    /// without Vim, or a Vim Normal mode with no count, operator, or multi-key
    /// command in flight. Vim Insert's Esc drops to Normal, and a pending
    /// operator's Esc cancels it — neither is idle. Visual keeps Esc for
    /// collapsing the selection.
    fn esc_is_idle(&self) -> bool {
        match self.mode {
            Mode::Insert => !self.vim,
            Mode::Normal => {
                let v = &self.vim_state;
                v.count.is_none()
                    && v.op.is_none()
                    && v.pending == VimPending::None
                    && self.case_op.is_none()
            }
            _ => false,
        }
    }

    /// Handle a terminal event, dispatching key presses to the active mode.
    /// Non-key events and key releases/repeats are ignored.
    #[allow(clippy::needless_pass_by_value)] // consumes the event, moving its inner Key/Mouse to handlers
    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(key),
            // Left-click positions the cursor (#757); a drag selects (#812); the
            // wheel scrolls the editor or the focused scrollable overlay (#764).
            Event::Mouse(m) => self.on_mouse(m),
            // A bracketed paste (#811) — the terminal's own paste of the OS
            // clipboard (Ctrl-Shift-V, Cmd-V, middle-click) — inserts at the cursor
            // as one undoable edit, but only in an editing mode; a prompt / list /
            // overlay owns its own input, so a stray paste there is ignored.
            Event::Paste(text)
                if self.overlay.is_none()
                    && !self.surface_pane_focused()
                    && !self.assistant_pane_focused()
                    && matches!(self.mode, Mode::Insert | Mode::Normal | Mode::Visual { .. }) =>
            {
                self.message = None;
                self.scroll_offset.set(0);
                self.insert_pasted_text(&text);
            }
            _ => {}
        }
    }

    /// Handle one key event, mutating state. Returns nothing; the caller
    /// re-renders afterward.
    pub fn on_key(&mut self, key: KeyEvent) {
        // A fresh keystroke clears any stale status message.
        self.message = None;

        // Galley proofreading mode (#1003) is read-only and owns all input while it's on:
        // Space/Backspace page forward/back, `n` toggles the reference line numbers, and
        // Esc (or Ctrl-\) leaves — dropping straight back to the untouched edit position.
        // Every other key does nothing, so no edit can slip in during a proofing pass.
        if self.in_galley() {
            self.on_key_galley(key);
            return;
        }

        // Any editing-mode keystroke (a cursor move or an edit) snaps a read-ahead
        // wheel scroll (#764) back to the cursor, as the acceptance criteria call
        // for. Overlay / prompt / list keys leave it alone — they don't drive the
        // editor viewport.
        if matches!(self.mode, Mode::Insert | Mode::Normal | Mode::Visual { .. })
            && self.overlay.is_none()
            && !self.surface_pane_focused()
            && !self.assistant_pane_focused()
        {
            self.scroll_offset.set(0);
        }

        // Esc leaves distraction-free composition mode (#975) from an editing mode,
        // restoring the chrome — the promised "toggle back" that isn't a second
        // Ctrl-\. Guarded to an Esc that would otherwise do nothing, so it never
        // pre-empts Esc where a prompt/menu/Vim-insert/pending operator owns it; a
        // Visual selection keeps Esc for collapsing the selection first.
        if key.code == KeyCode::Esc
            && self.distraction_free
            && self.overlay.is_none()
            && self.esc_is_idle()
        {
            self.toggle_distraction_free();
            return;
        }

        // F1 opens the user's guide from an editing mode (#281). Guarded so it
        // doesn't pre-empt a key's meaning elsewhere: with an overlay already up,
        // F1 falls through and closes it like any other key; in a prompt/menu the
        // mode owns its keys.
        if key.code == KeyCode::F(1)
            && self.overlay.is_none()
            && matches!(self.mode, Mode::Insert | Mode::Normal | Mode::Visual { .. })
        {
            self.open_guide();
            return;
        }

        // F2 opens the story-bible browser (#590) directly — the dedicated key the
        // Ctrl-K "Browse story bible" action advertises. Same guard as F1: only from
        // an editing mode with no overlay up, so it never pre-empts a prompt / menu /
        // overlay key. `open_bible_files` leaves the continuous view and reports an
        // empty bible itself, so the key is safe from any editing state.
        if key.code == KeyCode::F(2)
            && self.overlay.is_none()
            && matches!(self.mode, Mode::Insert | Mode::Normal | Mode::Visual { .. })
        {
            self.open_bible_files();
            return;
        }

        // While a sequenced run is in progress (a workflow #227, a series-bible
        // refresh #340, or a durable pass queue #923) suspended on an async step,
        // Esc cancels it — cleanly, after the current step finishes (#344). Only in
        // an editing mode: a prompt or palette owns Esc itself (a cost-confirm
        // cancels there).
        if (self.sequenced_run_active() || self.pass_queue_running())
            && key.code == KeyCode::Esc
            && matches!(self.mode, Mode::Insert | Mode::Normal | Mode::Visual { .. })
        {
            self.cancel_run();
            return;
        }

        // The key after a `Ctrl-w` window prefix (#163) is a window command, not
        // editing input.
        if self.window_pending {
            self.window_pending = false;
            self.window_command(key.code);
            return;
        }
        // The assistant pane takes all keys while focused (#307): typing the
        // request, submitting, scrolling, cancel/unfocus on Esc. Ctrl-w still
        // starts a window command (switch focus, close the pane).
        if self.assistant_pane_focused() {
            let ctrl_w = matches!(key.code, KeyCode::Char('w'))
                && key.modifiers.contains(KeyModifiers::CONTROL);
            if ctrl_w {
                self.window_pending = true;
            } else {
                self.on_key_assistant(key);
            }
            return;
        }
        // A read-only surface takes scroll keys: a docked surface pane with scroll
        // focus (#164), or a full-screen overlay (progress/analysis report, AI
        // answer). Either way Ctrl-w still starts a window command — switching
        // focus, closing, or (for an overlay) docking it into a split (#164).
        let surface_key: Option<fn(&mut Self, KeyEvent)> = if self.surface_pane_focused() {
            Some(Self::on_key_surface)
        } else if self.overlay.is_some() {
            Some(Self::on_key_overlay)
        } else {
            None
        };
        if let Some(handle) = surface_key {
            let ctrl_w = matches!(key.code, KeyCode::Char('w'))
                && key.modifiers.contains(KeyModifiers::CONTROL);
            if ctrl_w {
                self.window_pending = true;
            } else {
                handle(self, key);
            }
            return;
        }

        // Non-binding patterns: dispatch on the variant without moving its data.
        match self.mode {
            Mode::Prompt(_) => self.on_key_prompt(key),
            Mode::Nav { .. } => self.on_key_nav(key),
            Mode::EditMeta(_) => self.on_key_edit_meta(key),
            Mode::BibleReview(_) => self.on_key_bible_review(key),
            Mode::ConflictReview(_) => self.on_key_conflict_review(key),
            Mode::Findings(_) => self.on_key_findings(key),
            Mode::Dashboard(_) => self.on_key_dashboard(key),
            Mode::CoOccurrence(_) => self.on_key_cooccurrence(key),
            Mode::History(_) => self.on_key_history(key),
            Mode::Annotations { .. } => self.on_key_annotations(key),
            Mode::Corkboard(_) => self.on_key_corkboard(key),
            Mode::Outline { .. } => self.on_key_outline(key),
            Mode::ModelManager { .. } => self.on_key_model_manager(key),
            Mode::Palette(_) => self.on_key_palette(key),
            Mode::Search(_) => self.on_key_search(key),
            Mode::ProjectSearch(_) => self.on_key_project_search(key),
            Mode::RefFiles(_) => self.on_key_reffiles(key),
            Mode::Tree(_) => self.on_key_tree(key),
            Mode::ProjectBrowser(_) => self.on_key_project_browser(key),
            Mode::DirBrowser(_) => self.on_key_dir_browser(key),
            Mode::ImportPreview(_) => self.on_key_import_preview(key),
            Mode::ReviewImport(_) => self.on_key_review_import(key),
            Mode::BackupBrowser(_) => self.on_key_backup_browser(key),
            Mode::Drafts(_) => self.on_key_drafts(key),
            Mode::QuickSwitch(_) => self.on_key_quick_switch(key),
            // The three editing modes route through the dot-repeat recorder
            // (#1004), which brackets the real handler to capture the keys of a
            // change command (and to intercept `.` itself for replay).
            Mode::Insert | Mode::Normal | Mode::Visual { .. } => self.on_key_editing(key),
        }
    }

    /// Dispatch an editing-mode key (Normal / Insert / Visual) to its handler,
    /// wrapped in dot-repeat recording (#1004): `.` in Normal mode replays the
    /// last change, and every other key is offered to the recorder so a change
    /// command's keys are captured for replay. Kept here (a thin router seam)
    /// rather than in the Vim match arms so it stays out of the way of other
    /// Normal-mode binding work.
    fn on_key_editing(&mut self, key: KeyEvent) {
        // In Normal mode `.` repeats the last change and is never itself recorded.
        if matches!(self.mode, Mode::Normal)
            && matches!(key.code, KeyCode::Char('.'))
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && self.vim_state.pending == crate::mode::VimPending::None
            && self.vim_state.op.is_none()
            // A pending `g~`/`gu`/`gU` case operator (#1006) must not be shadowed:
            // `.` isn't a motion, so it should fall through to normal dispatch,
            // where `resolve_case_prefix` cancels the operator (matching Vim),
            // rather than replaying the previous change and dropping the operator.
            && self.case_op.is_none()
        {
            // `{count}.` repeats the change that many times (Vim's behavior); a
            // bare `.` repeats once. Either way the count is consumed here.
            let count = self.vim_state.count.take().unwrap_or(1);
            self.dot_repeat(count);
            return;
        }
        self.dot_record_around(key, |app, key| match app.mode {
            Mode::Insert => app.on_key_insert(key),
            Mode::Normal => app.on_key_normal(key),
            Mode::Visual { .. } => app.on_key_visual(key),
            _ => {}
        });
    }

    /// Handle a key while the read-only galley proofreading mode is on (#1003). The whole
    /// mode is these few keys — page forward/back, line numbers, exit — and nothing else,
    /// so the manuscript can't be edited during a proofing pass. Exiting leaves the edit
    /// buffer's cursor and scroll exactly as they were.
    fn on_key_galley(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // Page forward / back — the galley's whole navigation.
            KeyCode::Char(' ') | KeyCode::PageDown => self.galley_page_forward(),
            KeyCode::Backspace | KeyCode::PageUp => self.galley_page_back(),
            // Toggle the reference line-number gutter.
            KeyCode::Char('n') => self.toggle_galley_line_numbers(),
            // Esc, or the composition-family toggle key Ctrl-\, exits back to editing.
            KeyCode::Esc => self.exit_galley(),
            KeyCode::Char('\\') if ctrl => self.exit_galley(),
            // Quitting still works from a proofing pass (it routes through the same
            // unsaved-changes guard); every other key is inert — the mode is read-only.
            KeyCode::Char('q' | 'c') if ctrl => self.request_quit(),
            _ => {}
        }
    }

    /// The editor's resting mode: Vim Normal when Vim mode is on, else the
    /// modeless Insert. Every "return to the editor" path routes through this so
    /// it lands in the right mode for the user's configuration.
    pub(super) const fn editing_mode(&self) -> Mode {
        if self.vim {
            Mode::Normal
        } else {
            Mode::Insert
        }
    }

    /// Quit, or — when there are unsaved changes or a background task is still
    /// running — open the confirm prompt first. Shared by Ctrl-Q and `:q`.
    ///
    /// A busy `ai_task`/`vc_task`/`download_task` means work the quit would
    /// silently drop (#870): a Claude pass or batch that may already be billing, a
    /// push/pull mid-flight, or a partial model download. Since the `Task` threads
    /// are detached and never joined, quitting abandons the result; so a busy task
    /// warns and asks before exiting rather than dropping it without a word.
    pub(super) fn request_quit(&mut self) {
        if self.edit_dirty() {
            self.open_prompt(
                Pending::Quit,
                "Unsaved changes — quit anyway? (y/N)",
                "",
                None,
            );
        } else if let Some(work) = self.busy_task_label() {
            self.open_prompt(
                Pending::Quit,
                format!("{work} still running — quit anyway? (y/N)"),
                "",
                None,
            );
        } else {
            self.should_quit = true;
        }
    }

    /// A label for an in-flight background task the quit path would abandon (#870),
    /// or `None` when nothing is running. The AI request is named first (a pass or
    /// batch is the costliest thing to drop), then a remote push/pull, then a model
    /// download. Also the shared "a `Task` is busy" guard behind the navigation
    /// checks that keep a late-landing result from applying to a stale context.
    pub(super) const fn busy_task_label(&self) -> Option<&'static str> {
        if self.ai_task.is_busy() {
            Some("An AI request is")
        } else if self.vc_task.is_busy() {
            Some("A version-control sync is")
        } else if self.download_task.is_busy() {
            Some("A model download is")
        } else {
            None
        }
    }

    /// The editor's `Ctrl-` shortcuts, shared by Insert and Vim Normal mode so
    /// they keep working in both. A `Ctrl-` combo never produces text in either
    /// mode, so handling is fire-and-forget — an unrecognized one is simply
    /// ignored.
    pub(super) fn on_key_ctrl(&mut self, code: KeyCode) {
        // The `Ctrl-` combos that run a registry action resolve through the one
        // binding table on `Action` (#1103), so the shortcut the run-action menu
        // shows is exactly the key that runs the action — it can't drift. Ctrl-E
        // (export) and Ctrl-A (AI) still branch below, as they open a menu / do a
        // readiness check rather than dispatch a single concrete action.
        if let KeyCode::Char(c) = code {
            if let Some(action) = Action::from_ctrl_key(c) {
                if !matches!(action, Action::Export(_) | Action::Ai(_)) {
                    self.dispatch_action(action, false);
                    return;
                }
            }
        }
        match code {
            // Ctrl-Q and Ctrl-C both quit through the same guard (#730). Under raw
            // mode Ctrl-C arrives as a key event, not SIGINT, and terminal users
            // reflexively press it to leave a program — so honor it, but via
            // `request_quit` so a stray press still can't silently drop unsaved
            // work. It only quits from the top-level editor: overlays, prompts,
            // and palettes intercept keys before `on_key_ctrl`, so there Ctrl-C
            // is theirs to handle (Esc-style cancel), which is the least surprising.
            KeyCode::Char('q' | 'c') => self.request_quit(),
            KeyCode::Char('=' | '+') => {
                self.focus_width = (self.focus_width + 4).min(160);
            }
            KeyCode::Char('-') => {
                self.focus_width = self.focus_width.saturating_sub(4).max(32);
            }
            // Ctrl-O cycles the outline pane (#657): hidden → shown (display
            // only) → focused (navigable). A first press reveals it, a second
            // focuses it so its headings can be navigated and jumped to, and a
            // press while focused hides it again — one key, the whole life-cycle.
            KeyCode::Char('o') => {
                if !self.outline_visible {
                    self.outline_visible = true;
                    self.browser_opened_outline = false;
                } else {
                    self.focus_outline();
                }
            }
            // Open the version-control palette (#54) — init / commit / history /
            // changes / restore, in one menu. Ctrl-v: "version control".
            KeyCode::Char('v') => self.open_git_palette(),
            // Ctrl-R (progress report) and Ctrl-T (analysis report) are dispatched
            // above via `Action::from_ctrl_key`.
            // Open the AI command palette (opt-in). When AI is off, offer the in-app
            // on-ramp to enable it (#602) rather than a dead-end config instruction;
            // when it's on but unready (no key), explain rather than list dead actions.
            KeyCode::Char('a') => match self.ai.check_ready() {
                Ok(_) => self.open_ai_palette(),
                Err(_) if !self.ai.enabled => self.offer_enable_ai(None),
                Err(e) => self.message = Some(e.to_string()),
            },
            // Open the export menu (#7) — Markdown / Word. Ctrl-e.
            KeyCode::Char('e') => self.open_export_palette(),
            // Open the unified run-action menu (#225) — every invokable action
            // (save, export, reports, AI passes) in one searchable list. Ctrl-k
            // for "command", the one menu that reaches the others.
            KeyCode::Char('k') => self.open_action_palette(),
            // Open the run-workflow menu (#226) — the author's named action
            // sequences. Ctrl-x: "execute" a workflow.
            KeyCode::Char('x') => self.open_workflow_palette(),
            // Open the project browser (#270) — switch to a recently-opened
            // project without leaving the editor. Ctrl-b: "browse" projects.
            KeyCode::Char('b') => self.open_project_browser(),
            // Toggle the continuous-manuscript view. Ctrl-l ("long" view), not
            // Ctrl-m — terminals deliver Ctrl-m as Enter.
            KeyCode::Char('l') => self.toggle_continuous(),
            // Step to the next / previous chapter (single-chapter view only;
            // in continuous view you just scroll across boundaries).
            KeyCode::Char('n') if self.manuscript.is_none() => self.next_chapter(),
            KeyCode::Char('p') if self.manuscript.is_none() => self.prev_chapter(),
            // Focus the chapter pane to browse. Chapter structure is edited in
            // the single-chapter view, so leave continuous first.
            KeyCode::Char('g') => {
                self.leave_continuous();
                // Remember whether the browser is what opened the pane, so Esc
                // can put the view back exactly as it was (#529) — hiding a pane
                // Ctrl-G revealed, but leaving an outline already up via Ctrl-O.
                self.browser_opened_outline = !self.outline_visible;
                self.outline_visible = true;
                self.select(self.project.active_index());
            }

            // Open the fuzzy quick-switcher (#976): a Ctrl-P-style overlay that
            // jumps to any chapter or scene heading by typing a substring. Ctrl-d
            // ("dart" to a scene) — the go-to keys N/P/G step and browse chapters;
            // this reaches scenes too, without opening a pane.
            KeyCode::Char('d') => self.open_quick_switch(),
            // Undo / redo (#145). Ctrl-z undoes a burst of typing; Ctrl-y redoes.
            // (Vim's redo, Ctrl-r, is taken by the progress report; in Vim mode
            // `u` undoes and Ctrl-y redoes.)
            KeyCode::Char('z') => self.history_step(false),
            KeyCode::Char('y') => self.history_step(true),
            // Find (#144). Ctrl-f opens the query; from there `r` starts a replace.
            KeyCode::Char('f') => self.open_find(),
            // Fold / unfold the section under the cursor (#975): collapse a heading
            // and its body to the header line, in place, or expand it. Ctrl-Space —
            // the "collapse this" key, free in the editor. Fold-all / expand-all and
            // the composition-mode toggle live in the run-action menu (Ctrl-k).
            KeyCode::Char(' ') => self.toggle_fold_at_cursor(),
            // Ctrl-\ (composition mode) is dispatched above via
            // `Action::from_ctrl_key` (toggle distraction-free; Esc also exits it).
            // Window/split prefix (#163): the next key is a window command
            // (s/v split, w/h/j/k/l focus, c/q close). Vim's Ctrl-w.
            KeyCode::Char('w') => self.window_pending = true,

            _ => {}
        }
    }

    pub(super) fn on_key_insert(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if ctrl || alt {
            // The standard editing/motion combos (#641): word-wise cursor motion
            // (Ctrl-←/→) and word-delete (Ctrl/Alt-Backspace). Everything else
            // with Ctrl held is a Quill command (save, find, …).
            match key.code {
                KeyCode::Left => return self.edit_move(Buffer::move_word_backward),
                KeyCode::Right => return self.edit_move(Buffer::move_word_forward),
                KeyCode::Backspace => return self.edit_delete_word_back(),
                // Alt-= opens the spelling suggestions for the flagged word at the
                // cursor (#647), the modeless twin of Vim's `z=`.
                KeyCode::Char('=') if alt => return self.suggest_word_at_cursor(),
                _ if ctrl => return self.on_key_ctrl(key.code),
                _ => return, // an Alt combo we don't bind
            }
        }
        match key.code {
            // In Vim mode, Esc leaves Insert for Normal; without it, Esc is inert.
            KeyCode::Esc if self.vim => self.mode = Mode::Normal,

            KeyCode::Char(c) => self.edit_insert_char(c),
            KeyCode::Enter => self.edit_insert_char('\n'),
            KeyCode::Backspace => self.edit_backspace(),
            KeyCode::Delete => self.edit_delete_forward(),
            KeyCode::Tab => {
                for _ in 0..4 {
                    self.edit_insert_char(' ');
                }
            }

            KeyCode::Left => self.edit_move(Buffer::move_left),
            KeyCode::Right => self.edit_move(Buffer::move_right),
            KeyCode::Up => self.edit_move_wrapped(Buffer::move_up),
            KeyCode::Down => self.edit_move_wrapped(Buffer::move_down),
            KeyCode::Home => self.edit_move_wrapped(Buffer::move_home),
            KeyCode::End => self.edit_move_wrapped(Buffer::move_end),
            KeyCode::PageUp => self.edit_page(false),
            KeyCode::PageDown => self.edit_page(true),

            _ => {}
        }
    }
}

/// The remappable Vim commands (#208): each named action and its default key.
/// `[editor.keys]` binds an action name to a different key; the action set is the
/// single-key Normal/Visual commands (`gg` and the `"` register prefix, being
/// multi-key, aren't remappable). Documented in the README.
const VIM_ACTIONS: &[(&str, char)] = &[
    ("left", 'h'),
    ("down", 'j'),
    ("up", 'k'),
    ("right", 'l'),
    ("word_forward", 'w'),
    ("word_backward", 'b'),
    ("word_end", 'e'),
    ("line_start", '0'),
    ("first_non_blank", '^'),
    ("line_end", '$'),
    ("paragraph_forward", '}'),
    ("paragraph_backward", '{'),
    ("last_line", 'G'),
    ("find_forward", 'f'),
    ("find_backward", 'F'),
    ("till_forward", 't'),
    ("till_backward", 'T'),
    ("repeat_find", ';'),
    ("repeat_find_reverse", ','),
    ("delete", 'd'),
    ("change", 'c'),
    ("yank", 'y'),
    ("delete_char", 'x'),
    ("delete_to_line_end", 'D'),
    ("change_to_line_end", 'C'),
    ("put_after", 'p'),
    ("put_before", 'P'),
    ("undo", 'u'),
    ("insert", 'i'),
    ("append", 'a'),
    ("insert_line_start", 'I'),
    ("append_line_end", 'A'),
    ("open_below", 'o'),
    ("open_above", 'O'),
    ("visual", 'v'),
    ("visual_line", 'V'),
    ("search_forward", '/'),
    ("search_backward", '?'),
    ("search_next", 'n'),
    ("search_prev", 'N'),
    ("search_word", '*'),
    ("ex_command", ':'),
];

/// The remappable Vim actions and their default keys, for the in-app key-binding
/// view / remapper (#1109). Read-only accessor over [`VIM_ACTIONS`] so the
/// Settings menu can list the bindings and offer a remap without duplicating the
/// list (its single source of truth stays here, beside the dispatch that uses it).
pub(super) const fn vim_remap_actions() -> &'static [(&'static str, char)] {
    VIM_ACTIONS
}

/// Build the pressed-key → default-key remap from the `[editor.keys]` config
/// (#208). Each entry is `action = key`; the result maps that key to the
/// action's default key, so the existing dispatch runs unchanged. Returns any
/// bad entries (unknown action, or a value that isn't a single key) for
/// reporting — they're skipped, leaving the default binding in place.
pub(super) fn build_keymap(keys: &HashMap<String, String>) -> (HashMap<char, char>, Vec<String>) {
    let mut map = HashMap::new();
    let mut errors = Vec::new();
    for (action, key) in keys {
        let Some(&(_, default)) = VIM_ACTIONS.iter().find(|(name, _)| name == action) else {
            errors.push(format!("unknown action '{action}'"));
            continue;
        };
        let mut chars = key.chars();
        match (chars.next(), chars.next()) {
            (Some(k), None) => {
                if map.insert(k, default).is_some() {
                    errors.push(format!("key '{k}' is bound by more than one action"));
                }
            }
            _ => errors.push(format!("'{action}' must be a single key, got '{key}'")),
        }
    }
    (map, errors)
}

/// The new scroll offset for a scroll key over a `cur` position within
/// `[0, max]`, paging by `page` lines — or `None` if `code` isn't a scroll key.
/// Shared by the full-screen overlay and the docked surface pane (#164).
pub(super) fn scroll_target(
    code: KeyCode,
    ctrl: bool,
    cur: u16,
    max: u16,
    page: u16,
) -> Option<u16> {
    let (cur, max, page) = (i32::from(cur), i32::from(max), i32::from(page));
    let target = match (ctrl, code) {
        (false, KeyCode::Down | KeyCode::Char('j')) => cur + 1,
        (false, KeyCode::Up | KeyCode::Char('k')) => cur - 1,
        (false, KeyCode::PageDown | KeyCode::Char(' ')) | (true, KeyCode::Char('d')) => cur + page,
        (false, KeyCode::PageUp | KeyCode::Char('b')) | (true, KeyCode::Char('u')) => cur - page,
        (false, KeyCode::Home | KeyCode::Char('g')) => 0,
        (false, KeyCode::End | KeyCode::Char('G')) => max,
        _ => return None,
    };
    Some(target.clamp(0, max) as u16)
}
