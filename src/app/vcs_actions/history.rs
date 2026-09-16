//! The navigable commit-history view (#587) and its type-to-filter sub-mode
//! (#822): open the log, move / filter the selection, and — from the selected
//! milestone or commit (#833) — diff or restore the current chapter or the whole
//! project. Child module of `vcs_actions`, so these `impl App` methods keep
//! access to `App`'s private fields and reuse the parent's
//! [`diff_lines`](super::diff_lines) colorizer.

use super::super::*;
use super::diff_lines;

impl App {
    /// Open the navigable commit-history view (#587). A read-only list until a row
    /// is chosen; from the selection the author diffs the current chapter against it
    /// or restores that chapter to it (`d`/`r`), or diffs/restores the whole project
    /// against it (`D`/`R`, #833). Named milestones (#833) lead the list, above the
    /// commits. Falls back to a message when there's nothing to browse (no commits
    /// yet, or the log can't be read).
    pub(super) fn open_history(&mut self) {
        match crate::vcs::log(&self.project.root, 200) {
            Ok(commits) if commits.is_empty() => {
                self.message = Some("No commits yet.".into());
            }
            Ok(commits) => {
                // Milestones lead the list; a read failure just drops them, so the
                // history view still opens on the commits alone.
                let milestones = crate::vcs::list_tags(&self.project.root).unwrap_or_default();
                self.mode = Mode::History(Box::new(History {
                    commits,
                    milestones,
                    selected: 0,
                    filter: String::new(),
                    typing: false,
                    findings_anchor: None,
                }));
            }
            Err(e) => self.message = Some(format!("Could not read history: {e}")),
        }
    }

    /// Handle a key in the commit-history view (#587): arrows move the selection
    /// (skipping filtered-out rows), `d` diffs the current chapter against the
    /// selected row, `r` restores the chapter to it (into the editable buffer for
    /// review-and-save). `D`/`R` (#833) diff / restore the *whole project* against
    /// the selection — a milestone or any commit. `f` (#1051) picks two points and
    /// diffs the durable findings store between them — first `f` anchors a point,
    /// second `f` on another row reports which findings resolved / were introduced /
    /// stayed open, a pure local comparison with no AI request. `n` edits the note on
    /// a selected milestone (#966). Esc closes. `/` starts a type-to-filter query
    /// (#822) that narrows the list to rows whose summary, id, or date match; Esc
    /// clears the filter before it closes the view.
    pub(crate) fn on_key_history(&mut self, key: KeyEvent) {
        let Mode::History(history) = &self.mode else {
            return;
        };
        // In the filter-input sub-mode, keystrokes build the query (#822); the
        // view's single-key commands are reachable only outside it.
        if history.typing {
            self.on_key_history_filter(key);
            return;
        }
        match key.code {
            KeyCode::Char('/') => self.set_history_typing(true),
            KeyCode::Up | KeyCode::Char('k') => {
                let s = self.history_filter_step(false);
                self.set_history_selected(s);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let s = self.history_filter_step(true);
                self.set_history_selected(s);
            }
            KeyCode::Char('d') => self.diff_against_selected_commit(),
            KeyCode::Char('r') => self.restore_from_selected_commit(),
            KeyCode::Char('D') => self.diff_project_against_selected(),
            KeyCode::Char('R') => self.restore_project_from_selected(),
            KeyCode::Char('f') => self.findings_diff_step(),
            KeyCode::Char('n') => self.edit_selected_milestone_note(),
            // Esc clears an active filter first (restoring the full list), then a
            // second Esc closes the view.
            KeyCode::Esc => {
                if self.history_has_filter() {
                    self.clear_history_filter();
                } else {
                    self.mode = self.editing_mode();
                }
            }
            _ => {}
        }
    }

    /// Keys for the history view's filter-input sub-mode (#822): printable
    /// characters build the query (live), Backspace edits it, the arrows move
    /// within the matches, Enter leaves input, and Esc leaves input too — keeping
    /// the filter so `d`/`r` act on the narrowed list (a second Esc, in command
    /// mode, clears it).
    fn on_key_history_filter(&mut self, key: KeyEvent) {
        match on_filter_key(key, true) {
            Some(FilterInput::Leave) => self.set_history_typing(false),
            Some(FilterInput::Step(down)) => {
                let s = self.history_filter_step(down);
                self.set_history_selected(s);
            }
            Some(FilterInput::Pop) => self.history_filter_pop(),
            Some(FilterInput::Push(c)) => self.history_filter_push(c),
            Some(FilterInput::Enter) | None => {}
        }
    }

    /// The active history filter, lowercased (empty off the view or unfiltered).
    fn history_query(&self) -> String {
        match &self.mode {
            Mode::History(h) => h.filter.to_lowercase(),
            _ => String::new(),
        }
    }

    fn history_has_filter(&self) -> bool {
        matches!(&self.mode, Mode::History(h) if !h.filter.is_empty())
    }

    fn set_history_typing(&mut self, on: bool) {
        if let Mode::History(h) = &mut self.mode {
            h.typing = on;
        }
    }

    fn set_history_selected(&mut self, selected: usize) {
        if let Mode::History(h) = &mut self.mode {
            h.selected = selected;
        }
    }

    fn clear_history_filter(&mut self) {
        if let Mode::History(h) = &mut self.mode {
            h.filter.clear();
            h.typing = false;
        }
    }

    fn history_filter_push(&mut self, c: char) {
        if let Mode::History(h) = &mut self.mode {
            h.filter.push(c);
        }
        self.history_snap();
    }

    fn history_filter_pop(&mut self) {
        if let Mode::History(h) = &mut self.mode {
            h.filter.pop();
        }
        self.history_snap();
    }

    /// After the filter changes, keep the selection on a visible row: if the
    /// current one no longer matches, jump to the first that does (#822). Spans the
    /// combined milestone + commit list (#833).
    fn history_snap(&mut self) {
        let q = self.history_query();
        let Mode::History(h) = &self.mode else {
            return;
        };
        let stays = h.row(h.selected).is_some_and(|r| r.matches(&q));
        if !stays {
            if let Some(first) = h.rows().iter().position(|r| r.matches(&q)) {
                self.set_history_selected(first);
            }
        }
    }

    /// The next (or previous, by `forward`) row that passes the filter, or the
    /// current selection unchanged when there's no further match that way (#822).
    /// Spans the combined milestone + commit list (#833).
    fn history_filter_step(&self, forward: bool) -> usize {
        let q = self.history_query();
        let Mode::History(h) = &self.mode else {
            return 0;
        };
        let from = h.selected;
        let n = h.len();
        let mut i = from;
        loop {
            if forward {
                if i + 1 >= n {
                    return from;
                }
                i += 1;
            } else {
                if i == 0 {
                    return from;
                }
                i -= 1;
            }
            if h.row(i).is_some_and(|r| r.matches(&q)) {
                return i;
            }
        }
    }

    /// Edit the note on the selected milestone (#966), `n` from the history view.
    /// Only a milestone row carries a note, so a commit selection is a no-op with a
    /// hint. The prompt is prefilled with the current note (empty when the tag has
    /// only its name-fallback annotation, so there's no phantom text to clear).
    fn edit_selected_milestone_note(&mut self) {
        let q = self.history_query();
        let Mode::History(h) = &self.mode else {
            return;
        };
        let Some(row) = h.row(h.selected).filter(|r| r.matches(&q)) else {
            return;
        };
        let HistoryRow::Milestone(tag) = row else {
            self.message = Some("Notes attach to milestones, not commits.".into());
            return;
        };
        // The tag falls back to its name when created without a note, so an empty
        // note prefill means "no note yet" rather than the name showing as text.
        let note = if tag.message == tag.name {
            String::new()
        } else {
            tag.message.clone()
        };
        let name = tag.name.clone();
        self.start_edit_milestone_note(&name, &note);
    }

    /// The revision id of the row selected in the history view, or `None` when that
    /// view isn't open — or the selection is hidden by an active filter, so the
    /// diff/restore actions never act on a row the query narrowed out (#822).
    /// Resolves a milestone or a commit uniformly (#833).
    fn selected_commit_rev(&self) -> Option<String> {
        let q = self.history_query();
        match &self.mode {
            Mode::History(h) => h
                .row(h.selected)
                .filter(|r| r.matches(&q))
                .map(|r| r.rev().to_string()),
            _ => None,
        }
    }

    /// Diff the current chapter against the selected commit (#587), showing the
    /// unified patch in the read-only diff overlay (the same colorizer as the
    /// working-tree "Changes" view). The history view stays open behind it.
    fn diff_against_selected_commit(&mut self) {
        let Some(rev) = self.selected_commit_rev() else {
            return;
        };
        self.save_edit(); // diff against the latest edits, not the last-saved state
        let rel = self.active_chapter_rel();
        // `rev` → working tree: what changed in this chapter since that commit.
        match crate::vcs::diff_file_to_worktree(&self.project.root, &rev, &rel) {
            Ok(d) if d.trim().is_empty() => {
                self.message = Some("This chapter is unchanged since that commit.".into());
            }
            Ok(d) => self.overlay = Some(Overlay::report("diff", diff_lines(&d))),
            Err(e) => self.message = Some(format!("Diff failed: {e}")),
        }
    }

    /// Restore the current chapter to the selected commit (#587) into the editable
    /// buffer, matching the review-and-save semantics of the HEAD restore: the
    /// recovered version loads into the editor, marked dirty, so the author reviews
    /// it and saves (or discards) rather than the file changing underneath them.
    fn restore_from_selected_commit(&mut self) {
        let Some(rev) = self.selected_commit_rev() else {
            return;
        };
        self.leave_continuous(); // restore operates on the single active chapter
        let rel = self.active_chapter_rel();
        match crate::vcs::show_file_at(&self.project.root, &rev, &rel) {
            Ok(Some(text)) => {
                let path = self.project.active_path();
                self.buffer = Buffer::from_file_text(&path, &text);
                self.cache_focus_words();
                self.mode = self.editing_mode();
                let short: String = rev.chars().take(7).collect();
                self.message = Some(format!(
                    "Restored from {short} — review and save to keep it."
                ));
            }
            Ok(None) => {
                self.message = Some("That commit has no version of this chapter.".into());
            }
            Err(e) => self.message = Some(format!("Restore failed: {e}")),
        }
    }

    /// Diff the *whole project* against the selected row (#833) — a milestone or any
    /// commit — showing the unified patch in the read-only diff overlay. The history
    /// view stays open behind it.
    fn diff_project_against_selected(&mut self) {
        let Some(rev) = self.selected_commit_rev() else {
            return;
        };
        self.save_edit(); // diff against the latest edits, not the last-saved state
        match crate::vcs::diff_tree_to_worktree(&self.project.root, &rev) {
            Ok(d) if d.trim().is_empty() => {
                self.message = Some("The manuscript is unchanged since that point.".into());
            }
            Ok(d) => self.overlay = Some(Overlay::report("project diff", diff_lines(&d))),
            Err(e) => self.message = Some(format!("Diff failed: {e}")),
        }
    }

    /// Restore the *whole project* to the selected row (#833) — a milestone or any
    /// commit. Routes through the confirm-first flow, since the restore rewrites the
    /// entire working tree.
    fn restore_project_from_selected(&mut self) {
        let Some(rev) = self.selected_commit_rev() else {
            return;
        };
        self.start_restore_project(Some(rev));
    }

    /// One `f` press in the findings-diff flow (#1051). The first press anchors the
    /// selected row as point A (a hint confirms it). The second press, on a different
    /// row, diffs the durable findings store between the two points and reports the
    /// result; pressing `f` again on the same row is a no-op with a nudge. Anchoring
    /// on a hidden (filtered-out) row is refused, matching the diff/restore actions.
    fn findings_diff_step(&mut self) {
        let Some(rev) = self.selected_commit_rev() else {
            return;
        };
        let anchor = match &self.mode {
            Mode::History(h) => h.findings_anchor.clone(),
            _ => return,
        };
        match anchor {
            None => {
                self.set_findings_anchor(Some(rev));
                self.message =
                    Some("Findings diff: point A set — pick another row and press f.".into());
            }
            Some(a) if a == rev => {
                self.message = Some("Pick a different row for point B.".into());
            }
            Some(a) => {
                self.set_findings_anchor(None);
                self.show_findings_diff(&a, &rev);
            }
        }
    }

    fn set_findings_anchor(&mut self, anchor: Option<String>) {
        if let Mode::History(h) = &mut self.mode {
            h.findings_anchor = anchor;
        }
    }

    /// Diff the durable findings store between two revisions (#1051) and open the
    /// result in the read-only report overlay. Pure and local: it reads
    /// `bible/findings.toml` as it existed at each point out of git history (never a
    /// model call, never prose), classifies each finding as resolved / introduced /
    /// still-open by its content key, and renders the three groups. The history view
    /// stays open behind the overlay.
    fn show_findings_diff(&mut self, rev_a: &str, rev_b: &str) {
        let before = match self.findings_store_at(rev_a) {
            Ok(store) => store,
            Err(e) => {
                self.message = Some(format!("Findings diff failed: {e}"));
                return;
            }
        };
        let after = match self.findings_store_at(rev_b) {
            Ok(store) => store,
            Err(e) => {
                self.message = Some(format!("Findings diff failed: {e}"));
                return;
            }
        };
        let diff = findings_store::diff_findings(&before, &after);
        if diff.is_empty() {
            self.message = Some("No findings at either point — nothing to compare.".into());
            return;
        }
        self.overlay = Some(Overlay::report("findings diff", findings_diff_lines(&diff)));
    }

    /// Read the durable findings store as it existed at `rev` (#1051) — the
    /// `bible/findings.toml` blob out of git history, parsed the same best-effort way
    /// as the on-disk load. A revision with no store yet (it predates findings, or
    /// the file was absent) reads as the empty store, so it diffs cleanly against a
    /// point that has one. Errors only when the revision itself can't be resolved.
    fn findings_store_at(&self, rev: &str) -> Result<findings_store::FindingsStore, String> {
        let text =
            crate::vcs::show_file_at(&self.project.root, rev, findings_store::store_rel_path())?
                .unwrap_or_default();
        Ok(findings_store::FindingsStore::from_toml(&text))
    }
}

/// Render a findings diff (#1051) as report lines: a header line per group
/// (resolved / introduced / still-open) with its count, then one line per finding
/// carrying its pass label, chapters, and summary. Resolved reads green, introduced
/// red, still-open dim — the same additions/removals palette the diff overlays use.
/// A group with no findings is omitted entirely.
fn findings_diff_lines(diff: &findings_store::FindingsDiff) -> Vec<Line<'static>> {
    let theme = theme::current();
    let mut lines = Vec::new();
    let mut group = |title: &str, color: ratatui::style::Color, items: &[Finding]| {
        if items.is_empty() {
            return;
        }
        if !lines.is_empty() {
            lines.push(Line::from(String::new()));
        }
        lines.push(Line::from(Span::styled(
            format!("{title} ({})", items.len()),
            Style::default().fg(theme.heading),
        )));
        for f in items {
            let chapters = if f.chapters.is_empty() {
                "(no chapter)".to_string()
            } else {
                f.chapters.join(", ")
            };
            lines.push(Line::from(Span::styled(
                format!("  [{}] {} — {}", f.kind.label(), chapters, f.summary),
                Style::default().fg(color),
            )));
        }
    };
    group("Resolved", theme.added, &diff.resolved);
    group("Introduced", theme.removed, &diff.introduced);
    group("Still open", theme.dim, &diff.still_open);
    lines
}
