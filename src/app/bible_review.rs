//! The story-bible write / conflict / review flow (#153): staging generated
//! files, the conflict-review and findings overlays, and their key handlers.
//! Extracted from `assist.rs` (#315).

use std::path::PathBuf;

use similar::{ChangeTag, TextDiff};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use ratatui::text::Line;

use crate::ai::bible;
use crate::buffer::Buffer;
use crate::mode::{BibleConflict, BibleReview, Finding, Findings, Mode};

use super::App;
use super::assist::write_failure_message;
use super::assist::list_move;

/// The outcome of staging one generated file (#153) — a bible profile or a
/// submission artifact. Distinguishes a successful write from a hand-edit
/// conflict (queue for review) and from an I/O failure (surface to the author),
/// so a failed write is never miscounted as success.
pub(super) enum Staged {
    /// Written to disk.
    Wrote,
    /// The generated block was hand-edited since the last pass; queued for review.
    Conflict(BibleConflict),
    /// The write (or its parent-directory creation) failed; carries an
    /// author-facing description.
    Failed(String),
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

impl App {
    /// Stage a generated file: write it, or report a hand-edit conflict, or
    /// report an I/O failure (#153). `render` receives the author's preserved
    /// Notes block and returns the full file content. Shared by the character,
    /// continuity/setting/scaffold, and submission passes.
    pub(super) fn stage_bible(&self, path: PathBuf, name: &str, render: impl Fn(&str) -> String) -> Staged {
        let existing = std::fs::read_to_string(&path).ok();
        let notes = bible::extract_notes(existing.as_deref().unwrap_or(""));
        let proposed = render(&notes);
        if let Some(current) = &existing {
            if bible::is_hand_edited(current) {
                let diff = unified_diff(current, &proposed);
                return Staged::Conflict(BibleConflict { path, name: name.to_string(), proposed, diff });
            }
        }
        match self.write_file(&path, &proposed) {
            Ok(()) => Staged::Wrote,
            Err(msg) => Staged::Failed(msg),
        }
    }

    /// Write `content` to `path`, creating parent directories. On failure returns
    /// an author-facing message (the project-relative path plus the OS error), so
    /// the caller can surface it instead of silently dropping the write (#153).
    pub(super) fn write_file(&self, path: &std::path::Path, content: &str) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", self.display_path(path)))?;
        }
        std::fs::write(path, content).map_err(|e| format!("{}: {e}", self.display_path(path)))
    }

    /// A project-relative label for `path`, for author-facing messages.
    pub(super) fn display_path(&self, path: &std::path::Path) -> String {
        path.strip_prefix(&self.project.root).unwrap_or(path).display().to_string()
    }

    /// Open a findings-only result (analysis passes that write no bible file):
    /// clear any prior overlay, stash the findings, and open the overlay.
    pub(super) fn show_findings(&mut self, findings: Vec<Finding>) {
        self.overlay = None;
        self.accumulate_triage(&findings);
        self.pending_findings = findings;
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
        self.accumulate_triage(&findings);
        self.pending_findings = findings;
        match self.stage_bible(path, name, render) {
            Staged::Wrote => {
                self.record_pass_state(); // the bible now reflects this manuscript state (#90)
                self.open_findings();
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
                    f.items.remove(f.selected);
                    if f.items.is_empty() {
                        self.mode = self.editing_mode();
                    } else {
                        f.selected = f.selected.min(f.items.len() - 1);
                    }
                }
            }
            KeyCode::Enter => {
                let target = f.items.get(f.selected).and_then(|it| it.chapters.first()).cloned();
                self.mode = self.editing_mode();
                if let Some(title) = target {
                    if let Some(i) = (0..self.chapter_count()).find(|&i| self.project.chapter_title(i) == Some(title.as_str())) {
                        self.activate_chapter(i);
                    } else {
                        self.message = Some(format!("Chapter '{title}' not found"));
                    }
                }
            }
            // Turn the finding into an `xxx` TODO marker in its chapter (a note,
            // not prose). The keypress is the confirmation.
            KeyCode::Char('i') => {
                if let Some(item) = f.items.get(f.selected) {
                    let chapter = item.chapters.first().cloned();
                    let note = item.summary.clone();
                    if let Some(title) = chapter {
                        self.insert_todo(&title, &note);
                    } else {
                        self.message = Some("No chapter to mark".into());
                    }
                }
            }
            KeyCode::Esc => self.mode = Mode::Insert,
            _ => {}
        }
    }

    /// Append an `xxx — <note>` TODO marker to the body of the chapter titled
    /// `title` (a note for the author; never prose). The active chapter is
    /// flushed and reloaded so its open buffer sees the marker; other chapters
    /// are appended on disk.
    pub(super) fn insert_todo(&mut self, title: &str, note: &str) {
        let Some(i) = (0..self.chapter_count())
            .find(|&i| self.project.chapter_title(i) == Some(title))
        else {
            self.message = Some(format!("Chapter '{title}' not found"));
            return;
        };
        let marker = format!("\n\nxxx — {note}\n");

        if i == self.project.active_index() {
            // Persist current edits, append, and reload so the buffer is current.
            if self.flush_active().is_err() {
                return;
            }
        }
        let path = self.project.chapter_path(i);
        let mut body = std::fs::read_to_string(&path).unwrap_or_default();
        body.push_str(&marker);
        if std::fs::write(&path, body).is_err() {
            self.message = Some("Failed to write marker".into());
            return;
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
    }
}
