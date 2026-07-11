//! The findings triage overlay (#591): opening the stashed findings, its key
//! handler and filter-input sub-mode, and the filter/step/snap helpers that keep
//! the selection on a visible match. Split from the bible staging/apply flow
//! (`super`) so each file stays under the size guideline.

use crossterm::event::{KeyCode, KeyEvent};

use ratatui::text::Line;

use crate::atomic;
use crate::buffer::Buffer;
use crate::frontmatter;
use crate::mode::{Finding, Findings, Mode};
use crate::project::read_existing;
use crate::text::count_matches;

use super::super::App;

/// Whether finding `f` passes the triage overlay's active filter (#822): a
/// case-insensitive substring match on its summary or any of its chapters. `q` is
/// the already-lowercased query (empty = every finding matches). Mirrors the same
/// test the renderer applies when drawing the filtered list.
fn finding_matches(f: &Finding, q: &str) -> bool {
    q.is_empty()
        || f.summary.to_lowercase().contains(q)
        || f.chapters.iter().any(|c| c.to_lowercase().contains(q))
}

/// The open findings as plain Markdown text for the `export/findings-<date>.md`
/// snapshot (#829): a title with the open count, then one section per finding
/// carrying its pass/kind, the chapters it spans, and its one-line summary — the
/// flat triage list an author can hand off or track outside the editor. Pure over
/// the input, so it's directly testable. Within the AI boundary: flags only,
/// never prose.
fn findings_plain_text(findings: &[Finding]) -> String {
    let mut out = vec![
        format!("# Findings — {} open", findings.len()),
        String::new(),
    ];
    for f in findings {
        // The pass label the finding speaks for (#827), or "Other" for a finding
        // not tied to a re-runnable pass.
        out.push(format!("## {}", f.kind.label()));
        let chapters = if f.chapters.is_empty() {
            "(no chapter)".to_string()
        } else {
            f.chapters.join(", ")
        };
        out.push(format!("- Chapters: {chapters}"));
        out.push(format!("- {}", f.summary));
        out.push(String::new());
    }
    out.join("\n")
}

impl App {
    /// Open the stashed findings as a triage overlay, or summarize when there are
    /// none (the bible file has already been written).
    pub(in crate::app) fn open_findings(&mut self) {
        let items = std::mem::take(&mut self.pending_findings);
        if items.is_empty() {
            self.show_ai(vec![
                Line::raw("Analysis complete."),
                Line::raw(""),
                Line::raw("Written to the story bible. No findings."),
            ]);
        } else {
            self.mode = Mode::Findings(Box::new(Findings {
                items,
                ..Default::default()
            }));
        }
    }

    /// Jump the editor to a finding's target chapter (#827): switch to its book
    /// (a series finding may live in another, #341), then activate its first
    /// chapter. A no-op for a finding with no chapters (a Q&A answer row); reports
    /// when the chapter can't be found. Shared by the triage list's Enter and the
    /// manuscript-health dashboard, so both jump identically.
    pub(crate) fn jump_to_finding(&mut self, finding: &Finding) {
        self.focus_finding_book(finding.book);
        if let Some(title) = finding.chapters.first() {
            if let Some(i) = (0..self.chapter_count())
                .find(|&i| self.project.chapter_title(i) == Some(title.as_str()))
            {
                self.activate_chapter(i);
            } else {
                self.message = Some(format!("Chapter '{title}' not found"));
            }
        }
    }

    pub(crate) fn on_key_findings(&mut self, key: KeyEvent) {
        let Mode::Findings(f) = &self.mode else {
            return;
        };
        // In the filter-input sub-mode, keystrokes build the query (#822); the
        // overlay's single-key commands are reachable only outside it.
        if f.typing {
            self.on_key_findings_filter(key);
            return;
        }
        // The selected finding, only when it passes the active filter — so a
        // command never acts on a row the query has hidden (#822).
        let selected = f.selected;
        let acts_on_selected =
            self.finding_query_matches(selected) && selected < self.findings_len();
        match key.code {
            KeyCode::Char('/') => self.set_findings_typing(true),
            // Movement skips findings the filter has hidden.
            KeyCode::Up | KeyCode::Char('k') => {
                let s = self.findings_step(false);
                self.set_findings_selected(s);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let s = self.findings_step(true);
                self.set_findings_selected(s);
            }
            KeyCode::Char('d') if acts_on_selected => {
                // Dismiss durably (#591): the finding leaves the overlay and the
                // triage list, and a record on disk keeps a re-run from
                // re-surfacing it.
                let Mode::Findings(f) = &mut self.mode else {
                    return;
                };
                let dropped = f.items.remove(selected);
                if f.items.is_empty() {
                    self.mode = self.editing_mode();
                } else {
                    f.selected = f.selected.min(f.items.len() - 1);
                }
                self.dismiss_finding(&dropped);
                // Keep the selection on a still-visible match after the removal.
                self.findings_snap();
            }
            KeyCode::Enter if acts_on_selected => {
                let item = self.finding_at(selected).cloned();
                self.mode = self.editing_mode();
                if let Some(item) = item {
                    self.jump_to_finding(&item);
                }
            }
            // Turn the finding into an `xxx` TODO marker in its chapter (a note,
            // not prose). The keypress is the confirmation.
            KeyCode::Char('i') if acts_on_selected => {
                let item = self.finding_at(selected).cloned();
                if let Some(item) = item {
                    if let Some(title) = item.chapters.first().cloned() {
                        // A series finding (#341) may sit in another book; switch
                        // there first so the marker lands in the right chapter.
                        self.focus_finding_book(item.book);
                        // Only drop the finding once the marker is actually
                        // written (#591); a failed mark leaves it in triage.
                        if self.insert_todo(&title, &item.summary) {
                            self.resolve_finding(&item);
                            self.drop_finding_from_overlay(selected);
                            self.findings_snap();
                        }
                    } else {
                        self.message = Some("No chapter to mark".into());
                    }
                }
            }
            // Re-check just this finding (#608): re-run its originating pass scoped
            // to its chapters and clear or keep it by the result, so verifying one
            // fix doesn't cost a whole-manuscript sweep.
            KeyCode::Char('r') if acts_on_selected => {
                self.recheck_finding(selected);
            }
            // Re-check *all* open findings (#830): re-run each one's pass scoped to
            // its chapters (grouped by pass + chapters) and reconcile the whole list
            // at once — clearing the fixed, keeping the rest. Acts on every open
            // finding, not just the selection, so it ignores the filter.
            KeyCode::Char('R') => self.recheck_all(),
            // Save the open findings to `export/findings-<date>.md` (#829): the flat
            // triage list — each finding's pass + chapters + summary — filed as a
            // durable revision to-do, distinct from the grouped health dashboard.
            KeyCode::Char('s') => self.save_findings(),
            // Esc clears an active filter first (restoring the full list), then a
            // second Esc closes the overlay.
            KeyCode::Esc => {
                if self.findings_has_filter() {
                    self.clear_findings_filter();
                } else {
                    self.mode = Mode::Insert;
                }
            }
            _ => {}
        }
    }

    /// Save the open findings to `export/findings-<date>.md` (#829): the flat
    /// triage list — each finding's pass + chapters + summary — through the same
    /// [`write_export`](Self::write_export) path the report snapshots (#601) and
    /// the health dashboard (#827) use, so a revision to-do can leave the editor as
    /// a durable file a co-author can act on. Distinct from the grouped health
    /// dashboard: this is the flat list exactly as the overlay shows it. A no-op
    /// off the findings overlay. Within the AI boundary — it writes flags to an
    /// off-manuscript file, never prose.
    fn save_findings(&mut self) {
        let Mode::Findings(f) = &self.mode else {
            return;
        };
        if f.items.is_empty() {
            self.message = Some("No findings to save".into());
            return;
        }
        let body = findings_plain_text(&f.items);
        let name = format!("findings-{}", crate::date::iso(crate::date::today()));
        self.write_export(&name, "md", body.as_bytes());
    }

    /// Keys for the findings overlay's filter-input sub-mode (#822): printable
    /// characters build the query (live), Backspace edits it, the arrows move
    /// within the matches, Enter/Esc leave input — keeping the filter so the
    /// overlay's commands act on the narrowed list (a second Esc, in command mode,
    /// clears it).
    fn on_key_findings_filter(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.set_findings_typing(false),
            KeyCode::Up => {
                let s = self.findings_step(false);
                self.set_findings_selected(s);
            }
            KeyCode::Down => {
                let s = self.findings_step(true);
                self.set_findings_selected(s);
            }
            KeyCode::Backspace => self.findings_filter_pop(),
            KeyCode::Char(c) => self.findings_filter_push(c),
            _ => {}
        }
    }

    fn findings_len(&self) -> usize {
        match &self.mode {
            Mode::Findings(f) => f.items.len(),
            _ => 0,
        }
    }

    /// The active findings filter, lowercased (empty off the overlay/unfiltered).
    fn findings_query(&self) -> String {
        match &self.mode {
            Mode::Findings(f) => f.filter.to_lowercase(),
            _ => String::new(),
        }
    }

    fn findings_has_filter(&self) -> bool {
        matches!(&self.mode, Mode::Findings(f) if !f.filter.is_empty())
    }

    /// Whether the finding at `index` passes the active filter (#822).
    fn finding_query_matches(&self, index: usize) -> bool {
        let q = self.findings_query();
        self.finding_at(index)
            .is_some_and(|it| finding_matches(it, &q))
    }

    fn set_findings_typing(&mut self, on: bool) {
        if let Mode::Findings(f) = &mut self.mode {
            f.typing = on;
        }
    }

    fn set_findings_selected(&mut self, selected: usize) {
        if let Mode::Findings(f) = &mut self.mode {
            f.selected = selected;
        }
    }

    fn clear_findings_filter(&mut self) {
        if let Mode::Findings(f) = &mut self.mode {
            f.filter.clear();
            f.typing = false;
        }
    }

    fn findings_filter_push(&mut self, c: char) {
        if let Mode::Findings(f) = &mut self.mode {
            f.filter.push(c);
        }
        self.findings_snap();
    }

    fn findings_filter_pop(&mut self) {
        if let Mode::Findings(f) = &mut self.mode {
            f.filter.pop();
        }
        self.findings_snap();
    }

    /// After the filter changes, keep the selection on a visible finding: if the
    /// current one no longer matches, jump to the first that does (#822).
    fn findings_snap(&mut self) {
        let q = self.findings_query();
        let Mode::Findings(f) = &self.mode else {
            return;
        };
        let stays = f
            .items
            .get(f.selected)
            .is_some_and(|it| finding_matches(it, &q));
        if !stays {
            if let Some(first) = f.items.iter().position(|it| finding_matches(it, &q)) {
                self.set_findings_selected(first);
            }
        }
    }

    /// The next (or previous, by `forward`) finding that passes the filter, or the
    /// current selection unchanged when there's no further match that way (#822).
    fn findings_step(&self, forward: bool) -> usize {
        let q = self.findings_query();
        let Mode::Findings(f) = &self.mode else {
            return 0;
        };
        let from = f.selected;
        let n = f.items.len();
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
            if f.items.get(i).is_some_and(|it| finding_matches(it, &q)) {
                return i;
            }
        }
    }

    /// The finding at `index` in the open findings overlay, if any (#608).
    pub(in crate::app) fn finding_at(&self, index: usize) -> Option<&Finding> {
        match &self.mode {
            Mode::Findings(f) => f.items.get(index),
            _ => None,
        }
    }

    /// Remove `finding` from the open findings overlay by content (#608), closing
    /// the overlay when the list empties and clamping the selection otherwise. Used
    /// by the re-check, which knows the finding it cleared but not its current row.
    pub(in crate::app) fn remove_finding_from_overlay(&mut self, finding: &Finding) {
        if let Mode::Findings(f) = &mut self.mode {
            if let Some(i) = f.items.iter().position(|it| it == finding) {
                self.drop_finding_from_overlay(i);
            }
        }
    }

    /// Dismiss a finding durably (#591): mark it dismissed in the store (so a
    /// re-run doesn't re-surface it) and drop it from the standing triage list.
    fn dismiss_finding(&mut self, finding: &Finding) {
        let root = self.project.root.clone();
        self.findings_store.dismiss(&root, finding);
        self.triage.retain(|f| f != finding);
    }

    /// Resolve a finding (#591): the author marked it `xxx`, so drop its record
    /// from the store entirely and from the triage list. A recurrence in a later
    /// manuscript is a fresh finding that a new pass may surface again. Also used by
    /// the single-finding re-check (#608) when a fix clears a finding.
    pub(in crate::app) fn resolve_finding(&mut self, finding: &Finding) {
        let root = self.project.root.clone();
        self.findings_store.resolve(&root, finding);
        self.triage.retain(|f| f != finding);
    }

    /// Remove the finding at `index` from the open findings overlay, closing the
    /// overlay when the list empties and clamping the selection otherwise.
    fn drop_finding_from_overlay(&mut self, index: usize) {
        if let Mode::Findings(f) = &mut self.mode {
            if index < f.items.len() {
                f.items.remove(index);
            }
            if f.items.is_empty() {
                self.mode = self.editing_mode();
            } else if let Mode::Findings(f) = &mut self.mode {
                f.selected = f.selected.min(f.items.len() - 1);
            }
        }
    }

    /// Switch to the book a series finding belongs to (#341) before jumping or
    /// marking, so a finding from a non-active book lands in the right chapter.
    /// A `None` book (active-book / standalone passes) or the already-active book
    /// is a no-op.
    fn focus_finding_book(&mut self, book: Option<usize>) {
        if let Some(b) = book {
            if b != self.project.active_book() {
                self.switch_book(b);
            }
        }
    }

    /// Append an `xxx — <note>` TODO marker to the body of the chapter titled
    /// `title` (a note for the author; never prose). The active chapter is
    /// flushed and reloaded so its open buffer sees the marker; other chapters
    /// are appended on disk. Returns whether the marker was written, so triage can
    /// drop a finding only once it's actually captured (#591).
    pub(in crate::app) fn insert_todo(&mut self, title: &str, note: &str) -> bool {
        let Some(i) =
            (0..self.chapter_count()).find(|&i| self.project.chapter_title(i) == Some(title))
        else {
            self.message = Some(format!("Chapter '{title}' not found"));
            return false;
        };
        let marker = format!("\n\nxxx — {note}\n");

        if i == self.project.active_index() {
            // Persist current edits, append, and reload so the buffer is current.
            if self.flush_active().is_err() {
                return false;
            }
        }
        let path = self.project.chapter_path(i);
        // A failed read must abort: appending onto an empty base would replace the
        // chapter's prose with just the marker while reporting success (#565).
        let Ok(mut body) = read_existing(&path) else {
            self.message = Some("Failed to read chapter; marker not written".into());
            return false;
        };
        body.push_str(&marker);
        if atomic::write(&path, body.as_bytes()).is_err() {
            self.message = Some("Failed to write marker".into());
            return false;
        }
        if i == self.project.active_index() {
            // Reload so the open buffer shows the marker and isn't overwritten on
            // the next save.
            if let Ok(buffer) = Buffer::from_path(&path) {
                self.buffer = buffer;
            }
            self.cache_focus_words();
        } else {
            // Non-active chapters: the marker is on disk. The TODO roll-up now reads
            // the cached per-chapter count (#867), so refresh this chapter's cached
            // TODO count to include the note. The word cache is left untouched, so an
            // AI-inserted note still stays out of the author's word tally.
            let todo = count_matches(
                frontmatter::split(&body).1,
                self.project.manifest.todo_marker(),
            );
            self.project.set_chapter_todo(i, todo);
        }
        self.message = Some(format!("Marked xxx in '{title}'"));
        true
    }
}
