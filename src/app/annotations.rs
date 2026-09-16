//! Inline author annotations (#302): create a note anchored to a selection (or
//! the word at the cursor), keep it on its phrase as the prose is edited, and
//! review / jump-to / delete the chapter's notes.
//!
//! Annotations are held as live buffer ranges in [`App::annotations`], each tagged
//! with its chapter — buffer-local in the single-chapter view, buffer-absolute
//! (offset by the owning segment) in the continuous view (#370). They're
//! re-anchored on the idle tick from the buffer's edit log
//! ([`Buffer::drain_anchor_edits`]), reloaded and re-translated when the view's
//! layout changes, restored alongside undo/redo, and persisted per chapter to
//! `annotations.toml` (see [`crate::project`]) so the chapter `.md` stays clean
//! and exports never carry them.

use crossterm::event::{KeyCode, KeyEvent};

use super::App;
use crate::annotation::{Live, Remapper, Reply};
use crate::mode::{on_filter_key, FilterInput, JumpPos, Mode, Pending};

impl App {
    /// Reconcile annotations with the buffer on the idle tick (#302/#370): reload
    /// and re-translate when the view's layout changes (chapter switch, continuous
    /// window page, view toggle), otherwise shift each note through the edits since
    /// the last tick. Works in both views — single-chapter notes are buffer-local;
    /// continuous notes are buffer-absolute, offset by their segment.
    ///
    /// Undo/redo is handled separately: a rope swap carries no edit delta, so
    /// annotations are snapshotted alongside the buffer's undo stack and restored,
    /// rather than re-mapped here.
    pub(crate) fn sync_annotations(&mut self) {
        // A layout change (different chapter / window / view) means the buffer was
        // rebuilt: reload from disk and re-translate. Otherwise fold the burst's
        // edits into the live offsets so every note stays on its phrase.
        if self.annotation_context() != self.annotations_context {
            self.reload_annotations_for_view();
        } else {
            self.reanchor_annotations();
        }
    }

    /// Fold the buffer's pending anchor-edit deltas into the live annotation
    /// offsets, so they're current with the buffer as it stands right now — the
    /// delta half of [`sync_annotations`](Self::sync_annotations), split out so a
    /// keystroke-path persist can re-anchor *before* writing and never store a
    /// stale offset (#302/#370). It maps offsets forward through the edits without
    /// reloading, so the caller must still be on the same window/chapter the notes
    /// were anchored against — a layout change goes through
    /// [`reload_annotations_for_view`](Self::reload_annotations_for_view) instead.
    pub(crate) fn reanchor_annotations(&mut self) {
        // Snapshot annotations for any edit group that opened, before the deltas
        // below move them — keeps the undo history aligned with the buffer's (#370).
        self.note_annotation_undo_point();
        let edits = self.edit_buffer_mut().drain_anchor_edits();
        if edits.is_empty() {
            return;
        }
        // Re-anchor every note through the whole edit burst in one pass (#893):
        // compose the deltas into a single coordinate map, then remap each note
        // once. This is linear in the notes (plus near-linear in the edits to build
        // the map), rather than the old walk-all-notes-per-edit — which was
        // O(edits × notes) and turned an annotation-heavy chapter's idle remap
        // quadratic. The result is identical to folding `remap` per edit.
        let mapper = Remapper::new(&edits);
        self.annotations
            .retain_mut(|a| match mapper.remap_range(a.start, a.end) {
                Some((s, e)) => {
                    a.start = s;
                    a.end = e;
                    true
                }
                None => false,
            });
        self.annotations.sort_by_key(|a| (a.start, a.end));
    }

    /// Reload the loaded chapters' annotations from disk, re-translated to the
    /// current window's segment bases, and adopt the new layout signature — the
    /// layout-change half of [`sync_annotations`](Self::sync_annotations), split
    /// out so a reslide can re-anchor to the new window immediately (the
    /// reconciliation otherwise deferred to the idle tick) before a later
    /// same-burst persist reads the offsets (#302/#370). The buffer was rebuilt
    /// around the new layout, so any pending edit deltas are stale — drain and
    /// discard them — and the fresh buffer has no annotation undo history.
    pub(crate) fn reload_annotations_for_view(&mut self) {
        self.edit_buffer_mut().drain_anchor_edits();
        self.annotations = self.load_annotations_for_view();
        self.annotations_context = self.annotation_context();
        self.clear_annotation_history();
    }

    /// A signature of the current view's chapter layout (#370): the active file in
    /// the single view, the loaded-chapter indices in the continuous view. Stable
    /// across prose edits; changes when the buffer is rebuilt around a new layout.
    fn annotation_context(&self) -> String {
        self.manuscript.as_ref().map_or_else(
            || format!("S:{}", self.project.active_file()),
            |m| {
                let ids: Vec<String> = m.loaded_chapters().iter().map(usize::to_string).collect();
                format!("C:{}", ids.join(","))
            },
        )
    }

    /// Load every loaded chapter's annotations for the current view (#370),
    /// translated to live buffer offsets — buffer-local in the single view, offset
    /// by the owning segment's start in the continuous view.
    fn load_annotations_for_view(&self) -> Vec<crate::annotation::Live> {
        let mut out = Vec::new();
        for (file, base) in self.chapter_bases() {
            for a in self.project.annotations_for(&file) {
                out.push(Live {
                    chapter: file.clone(),
                    start: base + a.start,
                    end: base + a.end,
                    note: a.note,
                    replies: a.replies,
                    resolved: a.resolved,
                    reviewer: a.reviewer,
                });
            }
        }
        out.sort_by_key(|a| (a.start, a.end));
        out
    }

    /// Each loaded chapter's `(file, base_byte)` for the current view (#370): the
    /// active chapter at 0 in the single view; each segment's chapter and start
    /// byte in the continuous view. The seam for translating annotation offsets.
    fn chapter_bases(&self) -> Vec<(String, usize)> {
        self.manuscript.as_ref().map_or_else(
            || vec![(self.project.active_file().to_string(), 0)],
            |m| {
                m.segment_spans()
                    .into_iter()
                    .map(|(idx, start, _end)| (self.project.chapter_file(idx).to_string(), start))
                    .collect()
            },
        )
    }

    /// The chapter `(file, base_byte)` that owns a buffer byte (#370) — the active
    /// chapter in the single view, or the segment containing `byte` in continuous.
    fn owning_chapter(&self, byte: usize) -> (String, usize) {
        if let Some(m) = &self.manuscript {
            let spans = m.segment_spans();
            for &(idx, start, end) in &spans {
                if byte >= start && byte < end {
                    return (self.project.chapter_file(idx).to_string(), start);
                }
            }
            // Byte at the very end maps to the last segment.
            if let Some(&(idx, start, _)) = spans.last() {
                return (self.project.chapter_file(idx).to_string(), start);
            }
        }
        (self.project.active_file().to_string(), 0)
    }

    /// Snapshot the current annotations when a new undo group has opened (#370),
    /// so the app's annotation history stays one-entry-per-group aligned with the
    /// buffer's undo stack. Works in the continuous view too (#649): its notes are
    /// buffer-absolute [`Live`] ranges, so a snapshot restores them the same way an
    /// undo restores the rope, keeping them anchored across the windowed buffer.
    pub(crate) fn note_annotation_undo_point(&mut self) {
        let opened = self.edit_buffer_mut().take_checkpointed();
        if !opened {
            return;
        }
        self.anno_undo.push_back(self.annotations.clone());
        while self.anno_undo.len() > crate::buffer::Buffer::MAX_UNDO_GROUPS {
            self.anno_undo.pop_front();
        }
        self.anno_redo.clear(); // a new edit invalidates redo, mirroring the buffer
    }

    /// Swap the live annotations to match an undo/redo of the rope (#370): pop the
    /// pre-step snapshot off the matching stack and push the current onto the
    /// other, the exact mirror of the buffer's `step`.
    pub(crate) fn restore_annotation_history(&mut self, redo: bool) {
        let current = self.annotations.clone();
        let popped = if redo {
            self.anno_redo.pop_back()
        } else {
            self.anno_undo.pop_back()
        };
        if let Some(prev) = popped {
            if redo {
                self.anno_undo.push_back(current);
            } else {
                self.anno_redo.push_back(current);
            }
            self.annotations = prev;
        }
    }

    /// Drop the annotation undo/redo history — when the edit buffer is replaced
    /// (chapter switch, continuous view, crash recovery), its undo stack resets
    /// too (#370).
    pub(crate) fn clear_annotation_history(&mut self) {
        self.anno_undo.clear();
        self.anno_redo.clear();
    }

    /// Annotated byte ranges to mark in the editor, clamped to the buffer so a
    /// stale offset (e.g. after an undo) never indexes past the end. Buffer-
    /// absolute, so it marks the right spans in both the single and continuous
    /// views (#370).
    pub(crate) fn annotation_ranges(&self) -> Vec<(usize, usize)> {
        let len = self.edit_buffer().len_bytes();
        self.annotations
            .iter()
            .filter_map(|a| {
                let (s, e) = (a.start.min(len), a.end.min(len));
                (s < e).then_some((s, e))
            })
            .collect()
    }

    /// Begin annotating the selection (Visual mode) or the word at the cursor.
    /// Opens the note prompt; the annotation is created on Enter.
    pub(crate) fn start_annotation(&mut self) {
        let range = match self.mode {
            Mode::Visual { anchor, linewise } => Some(self.selection_bytes(anchor, linewise)),
            _ => self.word_at_cursor(),
        };
        match range {
            Some((start, end)) => self.start_annotation_range(start, end),
            None => {
                self.message =
                    Some("Select text or place the cursor on a word to annotate.".into());
            }
        }
    }

    /// Open the note prompt for the byte range `[start, end)` (#370: in either
    /// view — the range is a buffer offset, anchored to its owning chapter).
    pub(crate) fn start_annotation_range(&mut self, start: usize, end: usize) {
        if start >= end {
            self.message = Some("Select text or place the cursor on a word to annotate.".into());
            return;
        }
        let snippet = self.edit_buffer().byte_slice(start, end);
        let label = format!("Note on \"{}\": ", crate::text::ellipsize(&snippet, 30));
        self.open_prompt(Pending::Annotate { start, end }, label, "", None);
    }

    /// Create the annotation for a confirmed note (#302). An empty note cancels.
    /// The range is buffer-absolute; it's tagged with the chapter that owns it
    /// (#370) so it persists to the right file in either view.
    #[allow(clippy::needless_pass_by_value)] // callers (prompts.rs, tests) pass owned notes
    pub(crate) fn create_annotation(&mut self, start: usize, end: usize, note: String) {
        let note = note.trim().to_string();
        if note.is_empty() {
            return;
        }
        let (chapter, _base) = self.owning_chapter(start);
        self.annotations.push(Live {
            chapter,
            start,
            end,
            note,
            replies: Vec::new(),
            resolved: false,
            reviewer: None,
        });
        self.annotations.sort_by_key(|a| (a.start, a.end));
        self.persist_annotations();
        self.message = Some("Annotation added.".into());
    }

    /// Open the annotation review list for the loaded chapter(s) (#370: the active
    /// chapter, or the whole continuous window), or report when there are none.
    pub(crate) fn open_annotation_review(&mut self) {
        if self.annotations.is_empty() {
            self.message = Some("No annotations here.".into());
            return;
        }
        self.mode = Mode::Annotations {
            selected: 0,
            filter: String::new(),
            typing: false,
            show: crate::mode::AnnoShow::default(),
            sort: crate::mode::AnnoSort::default(),
            reviewer: None,
        };
    }

    /// Re-open the review list with `index` selected (#889): the landing spot after
    /// a reply prompt closes (confirmed or cancelled), so the author stays on the
    /// thread. Falls back to the editor if every annotation is gone.
    pub(crate) fn reopen_annotation_review(&mut self, index: usize) {
        if self.annotations.is_empty() {
            self.mode = self.editing_mode();
            return;
        }
        let selected = index.min(self.annotations.len() - 1);
        self.mode = Mode::Annotations {
            selected,
            filter: String::new(),
            typing: false,
            show: crate::mode::AnnoShow::default(),
            sort: crate::mode::AnnoSort::default(),
            reviewer: None,
        };
    }

    /// Open the reply prompt for the annotation at `index` (#889): the author types
    /// a follow-up that's appended to the thread on Enter. The thread's opening note
    /// is shown as the prompt label so it's clear what's being replied to.
    pub(crate) fn start_annotation_reply(&mut self, index: usize) {
        let Some(a) = self.annotations.get(index) else {
            return;
        };
        let label = format!("Reply to \"{}\": ", crate::text::ellipsize(&a.note, 30));
        self.open_prompt(Pending::AnnotateReply { index }, label, "", None);
    }

    /// Append a reply to the annotation at `index` (#889) and re-open the review
    /// list on it. An empty reply adds nothing but still returns to the list.
    #[allow(clippy::needless_pass_by_value)] // the prompt hands over an owned string
    pub(crate) fn add_annotation_reply(&mut self, index: usize, text: String) {
        let text = text.trim().to_string();
        if !text.is_empty() {
            if let Some(a) = self.annotations.get_mut(index) {
                a.replies.push(Reply {
                    author: "author".to_string(),
                    text,
                });
                self.persist_annotations();
                self.message = Some("Reply added.".into());
            }
        }
        self.reopen_annotation_review(index);
    }

    /// Toggle the resolved/open state of the annotation at `index` (#889): a
    /// resolved thread is dimmed in the list and hidden by the open filter.
    pub(crate) fn toggle_annotation_resolved(&mut self, index: usize) {
        if let Some(a) = self.annotations.get_mut(index) {
            a.resolved = !a.resolved;
            let now = a.resolved;
            self.persist_annotations();
            self.message = Some(if now {
                "Annotation resolved.".into()
            } else {
                "Annotation reopened.".into()
            });
            // Resolving may hide the row under the open filter; keep the selection on
            // a still-visible note.
            self.annotation_snap();
        }
    }

    /// Cycle the resolved/open filter (#889): all → open → resolved → all. Snaps the
    /// selection onto a note the new filter keeps visible.
    pub(crate) fn cycle_annotation_show(&mut self) {
        if let Mode::Annotations { show, .. } = &mut self.mode {
            *show = show.next();
        }
        self.annotation_snap();
    }

    /// Cycle the sort order (#933): position → status → reviewer → position. Snaps
    /// the selection onto a still-visible note (the same notes, reordered).
    pub(crate) fn cycle_annotation_sort(&mut self) {
        if let Mode::Annotations { sort, .. } = &mut self.mode {
            *sort = sort.next();
        }
        self.annotation_snap();
    }

    /// Cycle the per-reviewer filter (#933): all reviewers → each present reviewer
    /// in turn → back to all. The reviewers are the author's own bucket then the
    /// named editors alphabetically; with only one (or none) present, cycling is a
    /// no-op. Snaps the selection onto a note the new filter keeps visible.
    pub(crate) fn cycle_annotation_reviewer(&mut self) {
        let reviewers = self.annotation_reviewers();
        // Nothing to narrow by when every note shares one reviewer bucket.
        if reviewers.len() < 2 {
            return;
        }
        let current = self.annotation_reviewer_filter();
        // all → first → … → last → all.
        let next = match current
            .as_deref()
            .and_then(|c| reviewers.iter().position(|r| r == c))
        {
            None => Some(reviewers[0].clone()),
            Some(i) if i + 1 < reviewers.len() => Some(reviewers[i + 1].clone()),
            Some(_) => None,
        };
        if let Mode::Annotations { reviewer, .. } = &mut self.mode {
            *reviewer = next;
        }
        self.annotation_snap();
    }

    /// Key handling for the annotation review list: move (skipping filtered-out
    /// notes), jump (Enter), reply (`a`, #889), resolve/reopen (`r`, #889), cycle
    /// the open/resolved filter (`f`, #889), delete (`d`), close (Esc). `/` starts a
    /// type-to-filter query (#822) that narrows the list to notes whose text or
    /// anchored snippet match; Esc clears the filter before it closes the list.
    /// Multi-reviewer triage (#933): `s` cycles the sort (position/status/reviewer),
    /// `R` cycles the per-reviewer filter, and `]`/`[` jump to the next/previous
    /// open thread.
    pub(crate) fn on_key_annotations(&mut self, key: KeyEvent) {
        let Mode::Annotations {
            selected, typing, ..
        } = self.mode
        else {
            return;
        };
        // In the filter-input sub-mode, keystrokes build the query (#822); the
        // list's single-key commands are reachable only outside it.
        if typing {
            self.on_key_annotations_filter(key);
            return;
        }
        // Only act on the selection when it passes the active filter (#822).
        let acts_on_selected =
            selected < self.annotations.len() && self.annotation_query_matches(selected);
        match key.code {
            KeyCode::Char('/') => self.set_annotations_typing(true),
            KeyCode::Up | KeyCode::Char('k') => {
                let s = self.annotation_step(false);
                self.set_annotations_selected(s);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let s = self.annotation_step(true);
                self.set_annotations_selected(s);
            }
            KeyCode::Enter if acts_on_selected => {
                let start = self.annotations.get(selected).map(|a| a.start);
                self.mode = self.editing_mode();
                if let Some(start) = start {
                    let buf = self.edit_buffer_mut();
                    let at = start.min(buf.len_bytes());
                    buf.cursor = buf.cursor_at_byte(at);
                    // The note can anchor inside a collapsed section (#975); reveal
                    // it so jumping to the note doesn't strand the cursor hidden.
                    self.reveal_fold_at_cursor();
                }
            }
            KeyCode::Char('a') if acts_on_selected => self.start_annotation_reply(selected),
            KeyCode::Char('r') if acts_on_selected => self.toggle_annotation_resolved(selected),
            KeyCode::Char('f') => self.cycle_annotation_show(),
            KeyCode::Char('s') => self.cycle_annotation_sort(),
            KeyCode::Char('R') => self.cycle_annotation_reviewer(),
            // `]`/`[` jump to the next/previous open thread (#933), the spell nav's
            // `]s`/`[s` for triage.
            KeyCode::Char(']') => self.jump_annotation_open(true),
            KeyCode::Char('[') => self.jump_annotation_open(false),
            KeyCode::Char('d') if acts_on_selected => {
                self.annotations.remove(selected);
                self.persist_annotations();
                if self.annotations.is_empty() {
                    self.mode = self.editing_mode();
                } else {
                    let s = selected.min(self.annotations.len() - 1);
                    self.set_annotations_selected(s);
                    self.annotation_snap();
                }
            }
            // Esc clears an active filter first (restoring the full list), then a
            // second Esc closes the list.
            KeyCode::Esc => {
                if self.annotation_has_filter() {
                    self.clear_annotation_filter();
                } else {
                    self.mode = self.editing_mode();
                }
            }
            _ => {}
        }
    }

    /// Keys for the annotation list's filter-input sub-mode (#822): printable
    /// characters build the query (live), Backspace edits it, the arrows move
    /// within the matches, Enter/Esc leave input — keeping the filter so the
    /// list's commands act on the narrowed set (a second Esc, in command mode,
    /// clears it).
    fn on_key_annotations_filter(&mut self, key: KeyEvent) {
        match on_filter_key(key, true) {
            Some(FilterInput::Leave) => self.set_annotations_typing(false),
            Some(FilterInput::Step(down)) => {
                let s = self.annotation_step(down);
                self.set_annotations_selected(s);
            }
            Some(FilterInput::Pop) => self.annotation_filter_pop(),
            Some(FilterInput::Push(c)) => self.annotation_filter_push(c),
            Some(FilterInput::Enter) | None => {}
        }
    }

    /// The active annotation filter, lowercased (empty off the list/unfiltered).
    fn annotation_query(&self) -> String {
        match &self.mode {
            Mode::Annotations { filter, .. } => filter.to_lowercase(),
            _ => String::new(),
        }
    }

    /// Whether any narrowing filter is active — the `/` text query (#822) or the
    /// per-reviewer filter (#933). The first Esc clears these before a second closes
    /// the list.
    const fn annotation_has_filter(&self) -> bool {
        matches!(
            &self.mode,
            Mode::Annotations { filter, reviewer, .. } if !filter.is_empty() || reviewer.is_some()
        )
    }

    /// The active resolved/open filter (#889): `All` off the list.
    const fn annotation_show(&self) -> crate::mode::AnnoShow {
        match &self.mode {
            Mode::Annotations { show, .. } => *show,
            _ => crate::mode::AnnoShow::All,
        }
    }

    /// The active sort order (#933): `Position` off the list.
    const fn annotation_sort(&self) -> crate::mode::AnnoSort {
        match &self.mode {
            Mode::Annotations { sort, .. } => *sort,
            _ => crate::mode::AnnoSort::Position,
        }
    }

    /// The active per-reviewer filter (#933): `Some(name)` narrows to that
    /// reviewer's notes ([`AUTHOR_LABEL`](crate::annotation::AUTHOR_LABEL) for the
    /// author's own); `None` (off the list, or when cleared) shows every reviewer.
    fn annotation_reviewer_filter(&self) -> Option<String> {
        match &self.mode {
            Mode::Annotations { reviewer, .. } => reviewer.clone(),
            _ => None,
        }
    }

    /// The distinct reviewer labels present across the loaded annotations (#933),
    /// in a stable order — the author's own bucket
    /// ([`AUTHOR_LABEL`](crate::annotation::AUTHOR_LABEL)) first when any note is
    /// author-authored, then the named reviewers alphabetically. The cycle order
    /// the `R` per-reviewer filter steps through.
    fn annotation_reviewers(&self) -> Vec<String> {
        let mut named: Vec<String> = self
            .annotations
            .iter()
            .filter_map(|a| a.reviewer.clone())
            .collect();
        named.sort();
        named.dedup();
        let mut out = Vec::new();
        if self.annotations.iter().any(|a| a.reviewer.is_none()) {
            out.push(crate::annotation::AUTHOR_LABEL.to_string());
        }
        out.extend(named);
        out
    }

    /// Whether the annotation at `index` passes the text filter (#822), the
    /// resolved/open filter (#889), and the per-reviewer filter (#933). The text
    /// query is a case-insensitive substring match on the note, any reply text, or
    /// the snippet of prose it anchors; clamped to the buffer so a stale offset
    /// never indexes past the end.
    pub(crate) fn annotation_matches(&self, index: usize, q: &str) -> bool {
        let Some(a) = self.annotations.get(index) else {
            return false;
        };
        // The resolved/open filter applies whether or not a text query is set.
        if !self.annotation_show().admits(a.resolved) {
            return false;
        }
        // The per-reviewer filter (#933), likewise independent of the text query.
        if let Some(who) = self.annotation_reviewer_filter() {
            if a.reviewer_label() != who {
                return false;
            }
        }
        if q.is_empty() {
            return true;
        }
        if a.note.to_lowercase().contains(q)
            || a.replies.iter().any(|r| r.text.to_lowercase().contains(q))
        {
            return true;
        }
        let len = self.edit_buffer().len_bytes();
        let (s, e) = (a.start.min(len), a.end.min(len));
        s < e
            && self
                .edit_buffer()
                .byte_slice(s, e)
                .to_lowercase()
                .contains(q)
    }

    fn annotation_query_matches(&self, index: usize) -> bool {
        let q = self.annotation_query();
        self.annotation_matches(index, &q)
    }

    const fn set_annotations_typing(&mut self, on: bool) {
        if let Mode::Annotations { typing, .. } = &mut self.mode {
            *typing = on;
        }
    }

    pub(crate) const fn set_annotations_selected(&mut self, selected: usize) {
        if let Mode::Annotations { selected: s, .. } = &mut self.mode {
            *s = selected;
        }
    }

    /// Clear the active narrowing filters (#822/#933): the `/` text query and the
    /// per-reviewer filter, restoring the full (still status-filtered / sorted)
    /// list. The first Esc runs this; a second then closes the list.
    fn clear_annotation_filter(&mut self) {
        if let Mode::Annotations {
            filter,
            typing,
            reviewer,
            ..
        } = &mut self.mode
        {
            filter.clear();
            *typing = false;
            *reviewer = None;
        }
        self.annotation_snap();
    }

    fn annotation_filter_push(&mut self, c: char) {
        if let Mode::Annotations { filter, .. } = &mut self.mode {
            filter.push(c);
        }
        self.annotation_snap();
    }

    fn annotation_filter_pop(&mut self) {
        if let Mode::Annotations { filter, .. } = &mut self.mode {
            filter.pop();
        }
        self.annotation_snap();
    }

    /// The visible annotation indices in display order (#933): the notes that pass
    /// the text/status/reviewer filters, ordered by the active sort. This is the
    /// single source of truth the renderer, the arrow keys, and the snap all read,
    /// so the list, the selection, and the navigation always agree on order.
    ///
    /// The indices are into [`App::annotations`] (kept in position order), so
    /// `Position` is just the filtered indices unchanged; `Status` and `Reviewer`
    /// stable-sort them by a group key, keeping position order within each group.
    pub(crate) fn annotation_visible_order(&self) -> Vec<usize> {
        let q = self.annotation_query();
        let mut order: Vec<usize> = (0..self.annotations.len())
            .filter(|&i| self.annotation_matches(i, &q))
            .collect();
        // A stable sort preserves the position order the filtered indices already
        // carry, so it's the tie-break within every group.
        match self.annotation_sort() {
            crate::mode::AnnoSort::Position => {}
            crate::mode::AnnoSort::Status => {
                // Open (false) before resolved (true).
                order.sort_by_key(|&i| self.annotations[i].resolved);
            }
            crate::mode::AnnoSort::Reviewer => {
                order.sort_by_key(|&i| self.annotations[i].reviewer_label().to_string());
            }
        }
        order
    }

    /// After a filter/sort change, keep the selection on a visible note: if the
    /// current one no longer matches, jump to the first in display order (#822/#933).
    fn annotation_snap(&mut self) {
        let sel = match &self.mode {
            Mode::Annotations { selected, .. } => *selected,
            _ => return,
        };
        let order = self.annotation_visible_order();
        if !order.contains(&sel) {
            if let Some(&first) = order.first() {
                self.set_annotations_selected(first);
            }
        }
    }

    /// The next (or previous, by `forward`) visible note in display order, or the
    /// current selection unchanged at the end that way (#822/#933). Moves within
    /// [`annotation_visible_order`](Self::annotation_visible_order) so the arrow
    /// keys follow the sorted, filtered list the renderer shows.
    fn annotation_step(&self, forward: bool) -> usize {
        let from = match &self.mode {
            Mode::Annotations { selected, .. } => *selected,
            _ => return 0,
        };
        let order = self.annotation_visible_order();
        let Some(pos) = order.iter().position(|&i| i == from) else {
            // The selection isn't visible; land on the first visible note either way.
            return order.first().copied().unwrap_or(from);
        };
        let next = if forward {
            pos + 1
        } else {
            pos.wrapping_sub(1)
        };
        order.get(next).copied().unwrap_or(from)
    }

    /// Jump to the next (or previous, by `forward`) OPEN thread in display order
    /// (#933), like the spell nav's `]s`/`[s`. Wraps the whole visible list so a
    /// jump from the last open note lands back on the first, giving a round-robin
    /// walk through the still-open triage. A no-op when nothing visible is open.
    pub(crate) fn jump_annotation_open(&mut self, forward: bool) {
        let from = match &self.mode {
            Mode::Annotations { selected, .. } => *selected,
            _ => return,
        };
        let order = self.annotation_visible_order();
        // The open threads among the visible notes, in display order.
        let open: Vec<usize> = order
            .iter()
            .copied()
            .filter(|&i| !self.annotations[i].resolved)
            .collect();
        let n = open.len();
        if n == 0 {
            self.message = Some("No open threads.".into());
            return;
        }
        // The current selection's position in the display order — the reference
        // point the jump steps from, whether or not the selection itself is open.
        let cur_pos = order.iter().position(|&i| i == from).unwrap_or(0);
        // The open-list position the jump starts from. When the selection is itself
        // open, that's its own index. Otherwise it's the count of open notes at or
        // before the cursor — the index of the first open note *after* it — so a `]`
        // lands there and a `[` steps back one; both wrap the open list.
        let base = open.iter().position(|&i| i == from).map_or_else(
            || {
                open.iter()
                    .filter(|&&i| order.iter().position(|&o| o == i) <= Some(cur_pos))
                    .count()
            },
            // A `]` from an open note advances past it; the shared `+1`/`-1` below
            // handles the step, so an on-open start is offset back by one here to
            // cancel it and land the neighbor, matching the off-open case's "next".
            |p| if forward { p + 1 } else { p + n },
        );
        let target = if forward {
            base % n
        } else {
            (base + n - 1) % n
        };
        self.set_annotations_selected(open[target]);
    }

    /// `]a` / `[a`: step the cursor to the next (`forward`) or previous OPEN
    /// (unresolved) annotation (#1009), so a reviewer's still-open notes are a
    /// comment-to-comment keystroke rather than a trip through the review overlay.
    /// Works from any cursor position and in either view, and only lands on OPEN
    /// threads — resolved ones are skipped.
    ///
    /// Within the loaded buffer it's a plain cursor move (the loaded notes are
    /// buffer-absolute, matched against the cursor byte). When there's no open note
    /// left this way in the loaded chapter(s) it crosses into the nearest adjacent
    /// chapter that has one — read from that chapter's side file, so an unloaded
    /// chapter's open notes are still reachable — as a big jump that pushes the
    /// jumplist (so `` `` `` returns) and activates the chapter. At the ends it
    /// reports there's no more rather than moving silently.
    pub(crate) fn jump_to_open_annotation(&mut self, forward: bool) {
        let cur = self.edit_buffer().cursor_byte();
        // The next OPEN note within the loaded buffer, in the requested direction.
        // The list is kept sorted by start, so a directional scan finds the nearest.
        let within = {
            let open = self
                .annotations
                .iter()
                .filter(|a| !a.resolved)
                .map(|a| a.start);
            if forward {
                open.filter(|&s| s > cur).min()
            } else {
                open.filter(|&s| s < cur).max()
            }
        };
        if let Some(at) = within {
            let buffer = self.edit_buffer_mut();
            let at = at.min(buffer.len_bytes());
            buffer.cursor = buffer.cursor_at_byte(at);
            self.reveal_fold_at_cursor();
            return;
        }
        // No open note left this way in the loaded chapter(s) — cross into the
        // nearest adjacent chapter that has one, reading its side file directly so
        // an unloaded chapter's open notes are reachable without loading it.
        let active = self.project.active_index();
        let count = self.chapter_count();
        let target = if forward {
            (active + 1..count).find_map(|ch| self.first_open_annotation(ch).map(|b| (ch, b)))
        } else {
            (0..active)
                .rev()
                .find_map(|ch| self.last_open_annotation(ch).map(|b| (ch, b)))
        };
        match target {
            Some((chapter, byte)) => {
                self.push_jump();
                self.restore_jump(JumpPos { chapter, byte });
            }
            None => {
                self.message = Some(if forward {
                    "No next open annotation.".into()
                } else {
                    "No previous open annotation.".into()
                });
            }
        }
    }

    /// The chapter-local byte offset of chapter `ch`'s first OPEN annotation, or
    /// `None` when it has no open notes (#1009). Reads the chapter's side file so an
    /// unloaded chapter is still a valid cross-chapter jump target.
    fn first_open_annotation(&self, ch: usize) -> Option<usize> {
        let file = self.project.chapter_file(ch);
        self.project
            .annotations_for(file)
            .into_iter()
            .filter(|a| !a.resolved)
            .map(|a| a.start)
            .min()
    }

    /// The chapter-local byte offset of chapter `ch`'s last OPEN annotation, or
    /// `None` when it has none (#1009) — the backward-crossing counterpart of
    /// [`first_open_annotation`](Self::first_open_annotation).
    fn last_open_annotation(&self, ch: usize) -> Option<usize> {
        let file = self.project.chapter_file(ch);
        self.project
            .annotations_for(file)
            .into_iter()
            .filter(|a| !a.resolved)
            .map(|a| a.start)
            .max()
    }

    /// Write the loaded chapters' live annotations to their side files (#302/#370):
    /// for each loaded chapter, the notes tagged to it, translated from
    /// buffer-absolute back to chapter-local offsets. Other chapters' notes are
    /// left untouched.
    pub(crate) fn persist_annotations(&mut self) {
        let bases = self.chapter_bases();
        self.persist_annotations_to(&bases);
    }

    /// Persist annotations for the chapters in `bases` (#370). Taken explicitly so
    /// the continuous-view seams can persist while the manuscript is moved out of
    /// `self` (save / reslide / leaving the view).
    pub(crate) fn persist_annotations_to(&mut self, bases: &[(String, usize)]) {
        for (file, base) in bases {
            let local: Vec<Live> = self
                .annotations
                .iter()
                .filter(|a| &a.chapter == file)
                .map(|a| Live {
                    chapter: file.clone(),
                    start: a.start.saturating_sub(*base),
                    end: a.end.saturating_sub(*base),
                    note: a.note.clone(),
                    replies: a.replies.clone(),
                    resolved: a.resolved,
                    reviewer: a.reviewer.clone(),
                })
                .collect();
            if let Err(e) = self.project.set_chapter_annotations(file, &local) {
                self.message = Some(format!("Couldn't save annotations: {e}"));
            }
        }
    }

    /// The byte range of the word under the cursor (letters/digits/apostrophes),
    /// or `None` when the cursor isn't on a word. Used when annotating without a
    /// selection.
    fn word_at_cursor(&self) -> Option<(usize, usize)> {
        let buf = self.edit_buffer();
        let text = buf.text();
        let cur = buf.cursor_byte().min(text.len());
        let is_word = |c: char| c.is_alphanumeric() || c == '\'' || c == '\u{2019}';

        let mut start = cur;
        for (i, c) in text[..cur].char_indices().rev() {
            if is_word(c) {
                start = i;
            } else {
                break;
            }
        }
        let mut end = cur;
        for (i, c) in text[cur..].char_indices() {
            if is_word(c) {
                end = cur + i + c.len_utf8();
            } else {
                break;
            }
        }
        (start < end).then_some((start, end))
    }
}
