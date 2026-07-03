//! The story-bible write / conflict / review flow (#153): staging generated
//! files, the conflict-review and findings overlays, and their key handlers.
//! Extracted from `assist.rs` (#315).

use std::path::PathBuf;

use similar::{ChangeTag, TextDiff};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::ai::bible;
use crate::atomic;
use crate::buffer::Buffer;
use crate::mode::{BibleConflict, BibleReview, Finding, Findings, Mode};
use crate::project::read_existing;
use crate::theme;

use super::assist::list_move;
use super::assist::write_failure_message;
use super::App;

/// The outcome of staging one generated file (#153) — a bible profile or a
/// submission artifact. Distinguishes a successful write from a hand-edit
/// conflict (queue for review) and from an I/O failure (surface to the author),
/// so a failed write is never miscounted as success.
pub(super) enum Staged {
    /// Written to disk, carrying the unified diff (current on disk → proposed),
    /// each line prefixed with `+`/`-`/` ` (#607). Empty when the write was the
    /// first for that file or the content was unchanged, so a caller can show the
    /// delta on a clean revise and stay silent when nothing changed.
    Wrote(Vec<String>),
    /// The generated block was hand-edited since the last pass; queued for review.
    Conflict(BibleConflict),
    /// The write (or its parent-directory creation) failed; carries an
    /// author-facing description.
    Failed(String),
}

/// The tallied outcome of staging a batch of generated files (#503): how many
/// were written, the hand-edit conflicts to review, and any I/O failures. The
/// character pass, the off-manuscript writer, and the notes import all stage a
/// batch then make the same failures → conflicts → clean decision; this collects
/// the counts so that decision lives in one place ([`App::resolve_staged`]).
pub(super) struct StagedTally {
    pub(super) written: usize,
    pub(super) conflicts: Vec<BibleConflict>,
    pub(super) failures: Vec<String>,
}

impl StagedTally {
    /// Tally a batch of staged results into write count, conflicts, and failures.
    pub(super) fn from(staged: impl Iterator<Item = Staged>) -> Self {
        let mut tally = Self {
            written: 0,
            conflicts: Vec::new(),
            failures: Vec::new(),
        };
        for result in staged {
            match result {
                Staged::Wrote(_) => tally.written += 1,
                Staged::Conflict(c) => tally.conflicts.push(c),
                Staged::Failed(msg) => tally.failures.push(msg),
            }
        }
        tally
    }
}

pub(super) fn unified_diff(current: &str, proposed: &str) -> Vec<String> {
    TextDiff::from_lines(current, proposed)
        .iter_all_changes()
        .map(|change| {
            let sign = match change.tag() {
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
                ChangeTag::Equal => ' ',
            };
            format!("{sign}{}", change.value().trim_end_matches('\n'))
        })
        .collect()
}

/// A one-line `+N/-M` summary of a unified diff (#607), e.g.
/// `Updated character/Ada: +3/-1`. Counts the added and removed lines (context
/// lines are ignored), so the author sees the size of a clean revise at a glance.
fn delta_summary(name: &str, diff: &[String]) -> String {
    let added = diff.iter().filter(|l| l.starts_with('+')).count();
    let removed = diff.iter().filter(|l| l.starts_with('-')).count();
    format!("Updated {name}: +{added}/-{removed}")
}

/// Color a pre-rendered unified diff for the AI overlay (#607): additions green,
/// removals red, context dimmed. Mirrors the bible-conflict diff rendering.
fn diff_lines(diff: &[String]) -> Vec<Line<'static>> {
    let theme = theme::current();
    diff.iter()
        .map(|change| {
            let color = match change.chars().next() {
                Some('-') => theme.removed,
                Some('+') => theme.added,
                _ => theme.dim,
            };
            Line::from(Span::styled(change.clone(), Style::default().fg(color)))
        })
        .collect()
}

impl App {
    /// Stage a generated file: write it, or report a hand-edit conflict, or
    /// report an I/O failure (#153). `render` receives the author's preserved
    /// Notes block and returns the full file content. Shared by the character,
    /// continuity/setting/scaffold, and submission passes.
    pub(super) fn stage_bible(
        &self,
        path: PathBuf,
        name: &str,
        render: impl Fn(&str) -> String,
    ) -> Staged {
        let existing = std::fs::read_to_string(&path).ok();
        let notes = bible::extract_notes(existing.as_deref().unwrap_or(""));
        let proposed = render(&notes);
        if let Some(current) = &existing {
            if bible::is_hand_edited(current) {
                let diff = unified_diff(current, &proposed);
                return Staged::Conflict(BibleConflict {
                    path,
                    name: name.to_string(),
                    proposed,
                    diff,
                });
            }
        }
        // Diff the on-disk version against what we're about to write so a clean
        // revise can show the delta (#607); computed before the write, since after
        // it the file equals `proposed`. An unchanged write diffs to only context
        // lines, which the caller treats as "nothing changed".
        let diff = match &existing {
            Some(current) if current != &proposed => unified_diff(current, &proposed),
            _ => Vec::new(),
        };
        match self.write_file(&path, &proposed) {
            Ok(()) => Staged::Wrote(diff),
            Err(msg) => Staged::Failed(msg),
        }
    }

    /// Write `content` to `path`, creating parent directories. On failure returns
    /// an author-facing message (the project-relative path plus the OS error), so
    /// the caller can surface it instead of silently dropping the write (#153).
    pub(super) fn write_file(&self, path: &std::path::Path, content: &str) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("{}: {e}", self.display_path(path)))?;
        }
        atomic::write(path, content.as_bytes())
            .map_err(|e| format!("{}: {e}", self.display_path(path)))
    }

    /// A project-relative label for `path`, for author-facing messages.
    pub(super) fn display_path(&self, path: &std::path::Path) -> String {
        path.strip_prefix(&self.project.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    /// Make the shared three-way decision after staging a batch (#503): a failed
    /// write is reported and wins (nothing is claimed saved); otherwise any
    /// hand-edit conflicts open a single review with a caller-supplied summary
    /// line (`conflict_message`, given the write count and pending count); a
    /// clean batch runs `on_clean` for the caller's success summary. The handoff
    /// shape — `BibleReview { conflicts, editing: None }` — and the failure
    /// message live here so a change touches one place, not three.
    pub(super) fn resolve_staged(
        &mut self,
        tally: StagedTally,
        conflict_message: impl FnOnce(usize, usize) -> String,
        on_clean: impl FnOnce(&mut Self, usize),
    ) {
        let StagedTally {
            written,
            conflicts,
            failures,
        } = tally;
        if !failures.is_empty() {
            self.message = Some(write_failure_message(&failures));
        } else if conflicts.is_empty() {
            on_clean(self, written);
        } else {
            let pending = conflicts.len();
            self.mode = Mode::BibleReview(Box::new(BibleReview {
                conflicts,
                editing: None,
            }));
            self.message = Some(conflict_message(written, pending));
        }
    }

    /// Accumulate a pass's findings into durable triage and stash the still-open
    /// ones for the overlay (#591): a finding the author already dismissed is
    /// filtered out here, so a re-run that re-produces it neither re-enters triage
    /// nor pops the overlay again.
    fn stash_findings(&mut self, findings: Vec<Finding>) {
        self.accumulate_triage(&findings);
        self.pending_findings = findings
            .into_iter()
            .filter(|f| !self.findings_store.is_dismissed(f))
            .collect();
    }

    /// Open a findings-only result (analysis passes that write no bible file):
    /// clear any prior overlay, stash the findings, and open the overlay.
    pub(super) fn show_findings(&mut self, findings: Vec<Finding>) {
        self.overlay = None;
        self.stash_findings(findings);
        self.open_findings();
    }

    /// Write a single bible file and surface a set of findings — the shared shape
    /// of the continuity and setting passes. Clears any prior overlay, stashes the
    /// findings, writes the file, then opens the findings overlay — or, if the
    /// file was hand-edited, reviews that conflict first (`advance_bible_review`
    /// flushes the findings when the review queue drains).
    pub(super) fn apply_bible_with_findings(
        &mut self,
        path: PathBuf,
        name: &str,
        render: impl Fn(&str) -> String,
        findings: Vec<Finding>,
    ) {
        self.overlay = None;
        self.stash_findings(findings);
        match self.stage_bible(path, name, render) {
            Staged::Wrote(diff) => {
                self.record_pass_state(); // the bible now reflects this manuscript state (#90)
                if self.series_bible_running() {
                    // Mid series-bible refresh (#341): the findings are already in
                    // revision triage. Don't pop an overlay the next book's pass
                    // would immediately clobber — the driver's summary points the
                    // author to triage when the whole run finishes.
                    self.pending_findings.clear();
                } else {
                    // Surface what the revise changed (#607) so the author can
                    // trust or correct the AI-written entry: a `+N/-M` summary
                    // line always, and — when there are no findings to triage —
                    // the diff itself in the AI overlay. A no-change write (empty
                    // diff) shows nothing.
                    self.show_bible_delta(name, diff);
                }
            }
            Staged::Conflict(conflict) => {
                self.mode = Mode::BibleReview(Box::new(BibleReview {
                    conflicts: vec![conflict],
                    editing: None,
                }));
                self.message = Some(format!("{name} edited — review, then findings"));
            }
            Staged::Failed(msg) => {
                // The analysis still holds, so show the findings — but report the
                // failed write instead of claiming it was saved to the bible.
                self.message = Some(write_failure_message(&[msg]));
                let items = std::mem::take(&mut self.pending_findings);
                if !items.is_empty() {
                    self.mode = Mode::Findings(Box::new(Findings { items, selected: 0 }));
                }
            }
        }
    }

    /// After a clean revise write (#607), let the author see what the model
    /// changed. `diff` is the unified diff of the just-written tracker (empty when
    /// nothing changed). With findings pending, open the findings overlay and put
    /// the `+N/-M` summary in the message line, so triage still leads. With no
    /// findings, show the diff itself in the AI overlay so the delta isn't buried.
    /// A no-change write (empty diff) surfaces nothing new — the plain summary.
    fn show_bible_delta(&mut self, name: &str, diff: Vec<String>) {
        if diff.is_empty() {
            // Nothing changed on disk: keep the historic clean-write behavior.
            self.open_findings();
            return;
        }
        let summary = delta_summary(name, &diff);
        if self.pending_findings.is_empty() {
            // No triage to lead with — show the delta the author can trust or
            // correct, as a "view changes" diff in the AI overlay.
            let mut lines = vec![Line::raw(summary), Line::raw("")];
            lines.extend(diff_lines(&diff));
            self.show_ai(lines);
        } else {
            // Findings lead; the summary rides the message line under the overlay.
            self.open_findings();
            self.message = Some(summary);
        }
    }

    /// Open the stashed findings as a triage overlay, or summarize when there are
    /// none (the bible file has already been written).
    pub(super) fn open_findings(&mut self) {
        let items = std::mem::take(&mut self.pending_findings);
        if items.is_empty() {
            self.show_ai(vec![
                Line::raw("Analysis complete."),
                Line::raw(""),
                Line::raw("Written to the story bible. No findings."),
            ]);
        } else {
            self.mode = Mode::Findings(Box::new(Findings { items, selected: 0 }));
        }
    }

    pub(crate) fn on_key_bible_review(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let editing = matches!(&self.mode, Mode::BibleReview(r) if r.editing.is_some());

        if editing {
            // Editing the proposed version before saving (same text keys as
            // EditMeta, with bible-specific Ctrl-S / Esc).
            match (ctrl, key.code) {
                (true, KeyCode::Char('s')) => self.save_bible_edit(),
                (_, KeyCode::Esc) => {
                    if let Mode::BibleReview(r) = &mut self.mode {
                        r.editing = None; // back to the diff for this conflict
                    }
                }
                _ => {
                    self.handle_text_edit_key(key);
                }
            }
            return;
        }

        // Diff view: choose how to resolve the front conflict.
        match key.code {
            KeyCode::Char('o') => {
                let failed = if let Mode::BibleReview(r) = &self.mode {
                    r.conflicts
                        .first()
                        .and_then(|front| self.write_file(&front.path, &front.proposed).err())
                } else {
                    None
                };
                // A failed overwrite keeps the conflict (so the author can retry)
                // and reports the error rather than silently advancing.
                if let Some(msg) = failed {
                    self.message = Some(write_failure_message(&[msg]));
                } else {
                    self.advance_bible_review();
                }
            }
            KeyCode::Char('k') => self.advance_bible_review(),
            KeyCode::Char('e') => {
                if let Mode::BibleReview(r) = &mut self.mode {
                    if let Some(front) = r.conflicts.first() {
                        let buf = Buffer::from_text(&front.proposed);
                        r.editing = Some(buf);
                    }
                }
            }
            KeyCode::Esc => {
                self.mode = self.editing_mode();
                self.message = Some("Review cancelled — kept your versions".into());
            }
            _ => {}
        }
    }

    /// Persist the edited proposed version, then advance to the next conflict. A
    /// failed write keeps the conflict and reports the error (#153).
    pub(super) fn save_bible_edit(&mut self) {
        let failed = if let Mode::BibleReview(r) = &self.mode {
            r.conflicts
                .first()
                .zip(r.editing.as_ref())
                .and_then(|(front, buf)| self.write_file(&front.path, &buf.text()).err())
        } else {
            None
        };
        if let Some(msg) = failed {
            self.message = Some(write_failure_message(&[msg]));
            return; // leave the editor open on the unsaved conflict
        }
        if let Mode::BibleReview(r) = &mut self.mode {
            r.editing = None;
        }
        self.advance_bible_review();
    }

    /// Drop the resolved front conflict; close the review when the queue empties.
    pub(super) fn advance_bible_review(&mut self) {
        if let Mode::BibleReview(r) = &mut self.mode {
            if !r.conflicts.is_empty() {
                r.conflicts.remove(0);
            }
            if !r.conflicts.is_empty() {
                return;
            }
        }
        // Queue drained. A continuity pass leaves findings stashed — show them now;
        // a character pass leaves none, so this falls back to insert mode.
        if self.pending_findings.is_empty() {
            self.mode = self.editing_mode();
            self.message = Some("Bible review complete".into());
        } else {
            self.open_findings();
        }
    }

    pub(crate) fn on_key_findings(&mut self, key: KeyEvent) {
        let Mode::Findings(f) = &mut self.mode else {
            return;
        };
        if let Some(s) = list_move(f.selected, f.items.len(), key.code) {
            f.selected = s;
            return;
        }
        match key.code {
            KeyCode::Char('d') => {
                if f.selected < f.items.len() {
                    // Dismiss durably (#591): the finding leaves the overlay and
                    // the triage list, and a record on disk keeps a re-run from
                    // re-surfacing it.
                    let dropped = f.items.remove(f.selected);
                    if f.items.is_empty() {
                        self.mode = self.editing_mode();
                    } else {
                        f.selected = f.selected.min(f.items.len() - 1);
                    }
                    self.dismiss_finding(&dropped);
                }
            }
            KeyCode::Enter => {
                let item = f.items.get(f.selected).cloned();
                self.mode = self.editing_mode();
                if let Some(item) = item {
                    self.focus_finding_book(item.book);
                    if let Some(title) = item.chapters.first() {
                        if let Some(i) = (0..self.chapter_count())
                            .find(|&i| self.project.chapter_title(i) == Some(title.as_str()))
                        {
                            self.activate_chapter(i);
                        } else {
                            self.message = Some(format!("Chapter '{title}' not found"));
                        }
                    }
                }
            }
            // Turn the finding into an `xxx` TODO marker in its chapter (a note,
            // not prose). The keypress is the confirmation.
            KeyCode::Char('i') => {
                let selected = f.selected;
                let item = f.items.get(selected).cloned();
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
                        }
                    } else {
                        self.message = Some("No chapter to mark".into());
                    }
                }
            }
            // Re-check just this finding (#608): re-run its originating pass scoped
            // to its chapters and clear or keep it by the result, so verifying one
            // fix doesn't cost a whole-manuscript sweep.
            KeyCode::Char('r') => {
                let selected = f.selected;
                self.recheck_finding(selected);
            }
            KeyCode::Esc => self.mode = Mode::Insert,
            _ => {}
        }
    }

    /// The finding at `index` in the open findings overlay, if any (#608).
    pub(super) fn finding_at(&self, index: usize) -> Option<&Finding> {
        match &self.mode {
            Mode::Findings(f) => f.items.get(index),
            _ => None,
        }
    }

    /// Remove `finding` from the open findings overlay by content (#608), closing
    /// the overlay when the list empties and clamping the selection otherwise. Used
    /// by the re-check, which knows the finding it cleared but not its current row.
    pub(super) fn remove_finding_from_overlay(&mut self, finding: &Finding) {
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
    pub(super) fn resolve_finding(&mut self, finding: &Finding) {
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
    pub(super) fn insert_todo(&mut self, title: &str, note: &str) -> bool {
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
        }
        // Non-active chapters: the marker is on disk; the report's `xxx` count
        // reads bodies live, and leaving the word cache untouched keeps an
        // AI-inserted note out of the author's word tally.
        self.message = Some(format!("Marked xxx in '{title}'"));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{delta_summary, unified_diff, Staged, StagedTally};
    use crate::mode::BibleConflict;
    use std::path::PathBuf;

    fn conflict(name: &str) -> BibleConflict {
        BibleConflict {
            path: PathBuf::from(format!("{name}.md")),
            name: name.into(),
            proposed: String::new(),
            diff: Vec::new(),
        }
    }

    #[test]
    fn tally_sorts_each_staged_kind() {
        let tally = StagedTally::from(
            [
                Staged::Wrote(Vec::new()),
                Staged::Conflict(conflict("a")),
                Staged::Wrote(Vec::new()),
                Staged::Failed("oops".into()),
            ]
            .into_iter(),
        );
        assert_eq!(tally.written, 2);
        assert_eq!(tally.conflicts.len(), 1);
        assert_eq!(tally.conflicts[0].name, "a");
        assert_eq!(tally.failures, vec!["oops".to_string()]);
    }

    #[test]
    fn delta_summary_counts_added_and_removed() {
        // A revise that swaps one line for two: one removal, two additions; the
        // context line is not counted.
        let diff = unified_diff(
            "Ada is calm.\nContext.\n",
            "Ada is bold.\nAda is new.\nContext.\n",
        );
        assert_eq!(
            delta_summary("character/Ada", &diff),
            "Updated character/Ada: +2/-1"
        );
    }

    #[test]
    fn identical_content_has_no_delta() {
        // An unchanged revise diffs to context only: +0/-0, which the write path
        // treats as "nothing changed" and surfaces silently.
        let diff = unified_diff("Ada is calm.\n", "Ada is calm.\n");
        assert_eq!(
            delta_summary("character/Ada", &diff),
            "Updated character/Ada: +0/-0"
        );
    }

    #[test]
    fn empty_batch_tallies_to_zero() {
        let tally = StagedTally::from(std::iter::empty());
        assert_eq!(tally.written, 0);
        assert!(tally.conflicts.is_empty());
        assert!(tally.failures.is_empty());
    }
}
