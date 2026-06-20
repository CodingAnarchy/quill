//! Top-level application state and the key handling that drives editing. The
//! editor-mode types live in [`crate::mode`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

use similar::{ChangeTag, TextDiff};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use ratatui::text::Line;

use crate::ai::{self, bible, Backend, ClaudeBackend, Outcome};
use crate::ai::character::{self, Character};
use crate::ai::continuity::{self, Continuity};
use crate::ai::setting::{self, Setting};
use crate::analysis;
use crate::buffer::{self, Buffer};
use crate::date;
use crate::frontmatter::{Epigraph, FrontMatter, Status};
use crate::history;
use crate::manuscript::Manuscript;
use crate::mode::{
    BibleConflict, BibleReview, Finding, Findings, MetaEdit, MetaField, Mode, Overlay, Pending,
    PromptState,
};
use crate::project::{Manifest, Project};
use crate::report::{self, Momentum, PartDelta};

/// Shown when the author tries to delete a project's last remaining chapter.
mod assist;

const ONLY_CHAPTER_MSG: &str = "Can't delete the only chapter";

pub struct App {
    pub buffer: Buffer,
    /// The open project (manifest, root, active chapter index). The `buffer`
    /// above holds the active chapter's text.
    pub project: Project,
    /// When `Some`, the continuous-manuscript view is active: editing and
    /// rendering go through this windowed multi-chapter buffer instead of the
    /// single-chapter `buffer` above (which is then the saved, idle chapter).
    pub manuscript: Option<Manuscript>,
    pub mode: Mode,
    pub should_quit: bool,
    /// Distraction-free centering: target column width for the text, in cells.
    pub focus_width: u16,
    /// Whether the chapter / outline pane is shown.
    pub outline_visible: bool,
    /// Whether the project is under version control (#54). Cached so the status
    /// bar needn't stat the filesystem every frame; set at open and after the
    /// author initializes a repository.
    pub under_vc: bool,
    /// Count of uncommitted changes (#132), cached for the status bar. Refreshed
    /// on idle and after save / commit / init — never per frame. Zero when clean
    /// or not under version control.
    pub vc_changes: usize,
    /// The open full-screen overlay (progress report, analysis report, or AI
    /// answer), if any — read-only, scrollable, dismissed by any other key.
    /// Pre-rendered once on open so it isn't rebuilt per frame. A single slot:
    /// the overlays are mutually exclusive, and a new one is a new
    /// [`crate::mode::Overlay`] value, not a new field.
    pub overlay: Option<Overlay>,
    /// Height of the terminal in rows, refreshed each frame by the renderer.
    /// Lets the key handler clamp overlay scrolling to the content that exists.
    pub viewport_height: u16,
    /// Words inserted / deleted today and the count of words moved out of the
    /// manuscript today (deleted chapters). Seeded from the day's snapshot on
    /// load; fed to the progress report and persisted to the history log.
    added: usize,
    removed: usize,
    moved_to_unused: usize,
    /// Today's added / deleted words per part, in manifest part order (empty
    /// when the manifest defines no parts). The global counters above equal the
    /// sums of these plus any edits in chapters outside every part.
    part_added: Vec<usize>,
    part_removed: Vec<usize>,
    /// Per-chapter word baselines (chapter file → last counted words) the tally
    /// measures against. Keyed by file identity so it survives reordering; an
    /// edit's delta is attributed to the part of the chapter that changed.
    baselines: HashMap<String, usize>,
    /// AI configuration (opt-in, off by default), loaded from the user's home
    /// config. Drives whether the "Ask the manuscript" command is reachable.
    pub ai: ai::Settings,
    /// Receiver for an in-flight AI request running on a worker thread; `Some`
    /// while a request is pending. Polled each loop tick by [`App::poll_ai`].
    ai_rx: Option<Receiver<Outcome>>,
    /// The loaded local backend, cached after first use (loading a model is slow)
    /// and shared with the worker thread. Empty until the first local request;
    /// stays empty when the Claude backend is used. Type-erased so it needs no
    /// `local`-feature cfg here.
    local_backend: Arc<Mutex<Option<Arc<dyn Backend>>>>,
    /// Continuity findings waiting to be shown — stashed while a timeline
    /// conflict is resolved first, then opened by [`App::open_findings`].
    pending_findings: Vec<Finding>,
    /// All findings accumulated this session (continuity / setting / style /
    /// naming / gaps), de-duplicated — the "revision triage" list.
    triage: Vec<Finding>,
    /// One-line transient message shown in the status bar.
    pub message: Option<String>,
}

impl App {
    pub fn new(buffer: Buffer, project: Project) -> Self {
        let under_vc = crate::vcs::is_repo(&project.root);
        let mut app = Self {
            buffer,
            project,
            manuscript: None,
            mode: Mode::Insert,
            should_quit: false,
            focus_width: 72,
            outline_visible: false,
            under_vc,
            vc_changes: 0,
            overlay: None,
            viewport_height: 0,
            added: 0,
            removed: 0,
            moved_to_unused: 0,
            part_added: Vec::new(),
            part_removed: Vec::new(),
            baselines: HashMap::new(),
            ai: ai::Settings::load(),
            ai_rx: None,
            local_backend: Arc::new(Mutex::new(None)),
            pending_findings: Vec::new(),
            triage: Vec::new(),
            message: None,
        };
        // Baselines for the daily word tally: edits move each chapter from here.
        let parts = app.project.manifest.parts.as_ref().map_or(0, Vec::len);
        app.part_added = vec![0; parts];
        app.part_removed = vec![0; parts];
        app.seed_baselines();
        app.refresh_vc_status();
        app
    }

    /// Refresh the cached uncommitted-changes count (#132). Cheap (stat-based) but
    /// not free, so it's called on idle and after save / commit / init — never per
    /// frame. Zero when the project isn't under version control.
    fn refresh_vc_status(&mut self) {
        self.vc_changes = if self.under_vc {
            crate::vcs::status(&self.project.root).map_or(0, |s| s.count())
        } else {
            0
        };
    }

    /// Reset the per-chapter word baselines to the current counts, so the next
    /// tally measures edits from here. Used at startup and after a structural
    /// edit that shouldn't be booked as prose.
    fn seed_baselines(&mut self) {
        self.baselines = (0..self.chapter_count())
            .map(|i| (self.project.chapter_file(i).to_string(), self.chapter_word_count(i)))
            .collect();
    }

    /// The buffer the author is currently editing: the continuous-view buffer
    /// when that view is active, otherwise the single active chapter. The
    /// renderer and movement go through this so both views share one path.
    pub fn edit_buffer(&self) -> &Buffer {
        match &self.mode {
            Mode::EditMeta(m) => &m.buffer,
            Mode::BibleReview(r) if r.editing.is_some() => r.editing.as_ref().unwrap(),
            _ => match &self.manuscript {
                Some(m) => m.buffer(),
                None => &self.buffer,
            },
        }
    }

    pub fn edit_buffer_mut(&mut self) -> &mut Buffer {
        match &mut self.mode {
            Mode::EditMeta(m) => &mut m.buffer,
            Mode::BibleReview(r) if r.editing.is_some() => r.editing.as_mut().unwrap(),
            _ => match &mut self.manuscript {
                Some(m) => m.buffer_mut(),
                None => &mut self.buffer,
            },
        }
    }

    /// Called from the event loop once input goes idle: reparse the active
    /// buffer for highlighting, and book the day's word delta. Both are off the
    /// keystroke path, so typing stays instant.
    pub fn on_idle(&mut self) {
        self.edit_buffer_mut().refresh_syntax();
        self.tally_words();
        self.refresh_vc_status();
    }

    /// Attribute each chapter's word change since the last idle tick to today's
    /// global and per-part added / removed counters. Walking per chapter (rather
    /// than the project total) lets a delta be booked to the part of the chapter
    /// that changed; the global counters stay equal to the sum of the parts.
    /// Structural edits (chapter delete) drop their baseline so they aren't
    /// booked as prose.
    fn tally_words(&mut self) {
        for i in 0..self.chapter_count() {
            let words = self.chapter_word_count(i);
            let file = self.project.chapter_file(i);
            let last = self.baselines.get(file).copied().unwrap_or(0);
            let delta = words as i64 - last as i64;
            if delta == 0 {
                continue;
            }
            let (add, rem) = if delta > 0 {
                (delta as usize, 0)
            } else {
                (0, (-delta) as usize)
            };
            self.added += add;
            self.removed += rem;
            // part_of() only yields indices that exist, and the per-part vecs are
            // sized to the part count, so direct indexing is safe — and the global
            // counters stay equal to the sum of the parts by construction.
            if let Some(p) = self.project.manifest.part_of(i) {
                self.part_added[p] += add;
                self.part_removed[p] += rem;
            }
            self.baselines.insert(file.to_string(), words);
        }
    }

    /// Seed today's counters from the history log (resume the day across a
    /// reopen). Called once at startup after the project is open.
    pub fn load_history(&mut self) {
        if let Some(snap) = history::for_date(&self.project.root, &date::iso(date::today())) {
            self.added = snap.added;
            self.removed = snap.removed;
            self.moved_to_unused = snap.moved_to_unused;
            // Resume per-part counters only when they match the current parts
            // config; a changed manifest starts the day's per-part tally fresh.
            if snap.part_added.len() == self.part_added.len() {
                self.part_added = snap.part_added;
                self.part_removed = snap.part_removed;
            }
        }
    }

    /// Write today's snapshot to the history log (best-effort).
    pub fn persist_snapshot(&self) {
        let snapshot = history::Snapshot {
            date: date::iso(date::today()),
            words: self.project_word_count(),
            added: self.added,
            removed: self.removed,
            todo: self.count_todo(),
            moved_to_unused: self.moved_to_unused,
            parts: self.part_word_counts(),
            part_added: self.part_added.clone(),
            part_removed: self.part_removed.clone(),
        };
        let _ = history::persist(&self.project.root, &snapshot);
    }

    /// Current word total for each part, in manifest part order (empty when no
    /// parts). Each part sums the live counts of the chapters in its range.
    /// Each part's display name (its `name`, or `"{label} {n}"` when unnamed) and
    /// the 0-based chapter range it spans (`start..end`, clamped to the chapters
    /// that exist). Empty when the book defines no parts. The single home for the
    /// 1-based→0-based range conversion and the unnamed-part naming.
    fn part_ranges(&self) -> Vec<(String, std::ops::Range<usize>)> {
        let Some(parts) = self.project.manifest.parts.as_ref() else {
            return Vec::new();
        };
        let label = self.project.manifest.part_label();
        let chapters = self.chapter_count();
        parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let name = p.name.clone().unwrap_or_else(|| format!("{label} {}", i + 1));
                let range = p.chapters[0].saturating_sub(1)..p.chapters[1].min(chapters);
                (name, range)
            })
            .collect()
    }

    fn part_word_counts(&self) -> Vec<usize> {
        self.part_ranges()
            .into_iter()
            .map(|(_, range)| range.map(|i| self.chapter_word_count(i)).sum())
            .collect()
    }

    /// Whether the active edit target has unsaved changes (either view).
    fn edit_dirty(&self) -> bool {
        match &self.manuscript {
            Some(m) => m.dirty(),
            None => self.buffer.dirty,
        }
    }

    /// Word count for chapter `index`: live from the buffer when it's active
    /// (updates as you type), otherwise the count cached on the chapter entry.
    pub fn chapter_word_count(&self, index: usize) -> usize {
        if index == self.project.active_index() {
            self.buffer.word_count()
        } else {
            self.project.chapter_words(index)
        }
    }

    /// Total words across the whole project (the active chapter live, the rest
    /// cached). Used by the progress report.
    pub fn project_word_count(&self) -> usize {
        (0..self.chapter_count())
            .map(|i| self.chapter_word_count(i))
            .sum()
    }

    /// Put the project under version control, or report that it already is (#130).
    /// Opt-in and non-destructive: it never clobbers an existing repo or
    /// `.gitignore`.
    fn init_version_control(&mut self) {
        if self.under_vc {
            self.message = Some("Already under version control.".into());
            return;
        }
        match crate::vcs::init(&self.project.root) {
            Ok(()) => {
                self.under_vc = true;
                self.refresh_vc_status();
                self.message = Some("Initialized version control (git).".into());
            }
            Err(e) => self.message = Some(format!("Version control: {e}")),
        }
    }

    /// Begin a commit (#131): flush unsaved edits so the snapshot captures them,
    /// then prompt for a message. A no-op (with a hint) if the project isn't under
    /// version control or a save fails.
    fn start_commit(&mut self) {
        if !self.under_vc {
            self.message = Some("Not under version control — Ctrl-v to initialize.".into());
            return;
        }
        self.save_edit(); // commit the latest edits, not the last-saved state
        if self.edit_dirty() {
            return; // a save failed; save_edit already set the error message
        }
        self.open_prompt(Pending::Commit, "Commit message: ", "", None);
    }

    /// Commit the staged project with `message` and report the result.
    fn commit(&mut self, message: &str) {
        self.message = Some(match crate::vcs::commit(&self.project.root, message) {
            Ok(Some(id)) => format!("Committed {id}"),
            Ok(None) => "Nothing to commit — working tree clean.".into(),
            Err(e) => format!("Commit failed: {e}"),
        });
        self.refresh_vc_status(); // the tree is clean now (or unchanged on failure)
    }

    /// The progress report's rows, computed once when it's opened.
    fn report_lines(&self) -> Vec<Line<'static>> {
        let momentum = Momentum {
            added: self.added,
            removed: self.removed,
            moved_to_unused: self.moved_to_unused,
        };
        report::lines(
            &self.project.manifest,
            self.project_word_count(),
            self.chapter_count(),
            self.count_todo(),
            &momentum,
            &self.part_deltas(),
            date::today(),
        )
    }

    /// The open overlay's lines, if any (test inspection helper).
    #[cfg(test)]
    pub fn overlay_lines(&self) -> Option<&[Line<'static>]> {
        self.overlay.as_ref().map(|o| o.lines.as_slice())
    }

    /// The open overlay's scroll offset (0 when none is open).
    pub(crate) fn overlay_scroll(&self) -> u16 {
        self.overlay.as_ref().map_or(0, |o| o.scroll)
    }

    /// The largest valid scroll offset for the open overlay: enough to bring its
    /// last line into view, zero when it already fits (or none is open). Shares
    /// the bound formula with the renderer ([`crate::ui::max_scroll`]).
    fn overlay_max_scroll(&self) -> u16 {
        self.overlay.as_ref().map_or(0, |o| crate::ui::max_scroll(o.lines.len(), self.viewport_height))
    }

    /// Handle a key while a full-screen overlay is open: scroll on the navigation
    /// keys, close on anything else (so it stays "press a key to dismiss").
    fn on_key_overlay(&mut self, key: KeyEvent) {
        let max = self.overlay_max_scroll() as i32;
        let page = self.viewport_height.saturating_sub(3).max(1) as i32; // keep a line of context
        let cur = self.overlay_scroll() as i32;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // `Some(row)` scrolls to `row`; `None` dismisses the overlay.
        let target = match (ctrl, key.code) {
            (false, KeyCode::Down | KeyCode::Char('j')) => Some((cur + 1).clamp(0, max)),
            (false, KeyCode::Up | KeyCode::Char('k')) => Some((cur - 1).clamp(0, max)),
            (false, KeyCode::PageDown | KeyCode::Char(' ')) | (true, KeyCode::Char('d')) => Some((cur + page).clamp(0, max)),
            (false, KeyCode::PageUp | KeyCode::Char('b')) | (true, KeyCode::Char('u')) => Some((cur - page).clamp(0, max)),
            (false, KeyCode::Home | KeyCode::Char('g')) => Some(0),
            (false, KeyCode::End | KeyCode::Char('G')) => Some(max),
            _ => None,
        };
        match target {
            Some(row) => {
                if let Some(overlay) = self.overlay.as_mut() {
                    overlay.scroll = row as u16;
                }
            }
            None => self.overlay = None,
        }
    }

    /// The analysis report's rows (readability per chapter + overall), computed
    /// once when it's opened. Scans every chapter's prose (active chapter live,
    /// the rest from disk) — on demand, never per frame.
    fn analysis_lines(&self) -> Vec<Line<'static>> {
        let threshold = self.project.manifest.run_on_threshold();
        let mut chapters = Vec::with_capacity(self.chapter_count());
        let mut longest = analysis::LongestSentences::new(5);
        let mut word_counts = HashMap::new();
        let mut phrase_counts = HashMap::new();
        let mut tokens = Vec::new();
        for i in 0..self.chapter_count() {
            let title = self.project.chapter_title(i).unwrap_or("Untitled").to_string();
            // Measure prose only — drop heading lines so a chapter/scene title
            // doesn't glue onto its first sentence or skew readability.
            let text = analysis::prose(&self.chapter_text(i));
            let stats = analysis::stats(&text);
            let sentences = analysis::sentences(&text);
            let sentence_lengths: Vec<usize> =
                sentences.iter().map(|s| buffer::count_words(s)).collect();
            for (s, &words) in sentences.iter().zip(&sentence_lengths) {
                longest.consider(&title, s, words);
            }
            // Frequency tallies accumulate across the whole manuscript; phrases
            // are gathered per chapter so they don't span a chapter boundary.
            analysis::tally_content_words(&text, &mut word_counts);
            analysis::tally_phrases(&text, &mut phrase_counts);
            // Tokenize once: the just-appended slice is this chapter's lexical
            // variety; the whole pool is the overall variety (a true MATTR, not a
            // mean of the chapters').
            let token_start = tokens.len();
            analysis::tally_tokens(&text, &mut tokens);
            // Pacing uses the rolled-up chapter word count (headings included),
            // matching the chapter list and progress report.
            chapters.push(analysis::ChapterAnalysis {
                title,
                words: self.chapter_word_count(i),
                stats,
                sentence_lengths,
                style: analysis::style_flags(&text),
                dialogue: analysis::dialogue_split(&text),
                paragraph_lengths: analysis::paragraph_lengths(&text),
                lexical_variety: analysis::lexical_variety_tokens(&tokens[token_start..]),
            });
        }
        let longest = longest.into_vec();
        let parts = self.analysis_parts();
        // Style-flag list sizes / cutoffs (whole-manuscript signals).
        const TOP_WORDS: usize = 12;
        const TOP_PHRASES: usize = 8;
        const PHRASE_MIN: usize = 3; // a phrase must recur at least this often to list
        let top_words = analysis::top_counts(&word_counts, TOP_WORDS, 1);
        let top_phrases = analysis::top_counts(&phrase_counts, TOP_PHRASES, PHRASE_MIN);
        let report = analysis::Report {
            title: &self.project.manifest.title,
            chapters: &chapters,
            longest: &longest,
            parts: &parts,
            run_on_threshold: threshold,
            top_words: &top_words,
            top_phrases: &top_phrases,
            overall_variety: analysis::lexical_variety_tokens(&tokens),
        };
        analysis::lines(&report)
    }

    /// The part groupings for the pacing section. Empty when the book defines no
    /// parts.
    fn analysis_parts(&self) -> Vec<analysis::PartSpan> {
        self.part_ranges()
            .into_iter()
            .map(|(name, range)| analysis::PartSpan { name, range })
            .collect()
    }

    /// Build the per-part rows for the progress report: each part's display name,
    /// current word total, and today's added / removed within it.
    fn part_deltas(&self) -> Vec<PartDelta> {
        let words = self.part_word_counts();
        self.part_ranges()
            .into_iter()
            .enumerate()
            // `words` and the per-part counters are all sized to the parts list.
            .map(|(i, (name, _))| PartDelta {
                name,
                words: words[i],
                added: self.part_added[i],
                removed: self.part_removed[i],
            })
            .collect()
    }

    /// The body text of chapter `index`: live from the active buffer, otherwise
    /// from disk. The shared basis for whole-manuscript reads (TODO counting,
    /// the AI request).
    fn chapter_text(&self, index: usize) -> String {
        if index == self.project.active_index() {
            self.buffer.text()
        } else {
            self.project.chapter_body(index)
        }
    }

    /// Count the configured TODO marker across the manuscript (case-insensitive).
    fn count_todo(&self) -> usize {
        let needle = self.project.manifest.todo_marker().to_lowercase();
        if needle.is_empty() {
            return 0;
        }
        (0..self.chapter_count())
            .map(|i| self.chapter_text(i).to_lowercase().matches(&needle).count())
            .sum()
    }


    /// Handle a terminal event, dispatching key presses to the active mode.
    /// Non-key events and key releases/repeats are ignored.
    pub fn handle_event(&mut self, ev: Event) {
        if let Event::Key(key) = ev {
            if key.kind == KeyEventKind::Press {
                self.on_key(key);
            }
        }
    }

    /// Handle one key event, mutating state. Returns nothing; the caller
    /// re-renders afterward.
    pub fn on_key(&mut self, key: KeyEvent) {
        // A fresh keystroke clears any stale status message.
        self.message = None;

        // A full-screen overlay (progress report, analysis report, AI answer) is
        // read-only and scrollable: navigation keys scroll it, any other key
        // closes it.
        if self.overlay.is_some() {
            self.on_key_overlay(key);
            return;
        }

        // Non-binding patterns: dispatch on the variant without moving its data.
        match self.mode {
            Mode::Prompt(_) => self.on_key_prompt(key),
            Mode::Nav { .. } => self.on_key_nav(key),
            Mode::EditMeta(_) => self.on_key_edit_meta(key),
            Mode::BibleReview(_) => self.on_key_bible_review(key),
            Mode::Findings(_) => self.on_key_findings(key),
            Mode::Palette(_) => self.on_key_palette(key),
            Mode::Insert => self.on_key_insert(key),
        }
    }

    /// Save the current chapter (if dirty), then load `index` as the active
    /// chapter. The active index and the live buffer always move together here,
    /// so the two never drift. A no-op if `index` is already active or invalid.
    /// Save the active chapter if it has unsaved edits and refresh its cached
    /// word count. Returns `Err(())` (and sets a status message) if the save
    /// failed, so callers can abort before switching away from it.
    fn flush_active(&mut self) -> Result<(), ()> {
        if self.buffer.dirty {
            if let Err(e) = self.buffer.save() {
                self.message = Some(format!("Save failed: {e}"));
                return Err(());
            }
        }
        // The chapter is now on disk; refresh its cached count.
        self.project.set_active_words(self.buffer.word_count());
        Ok(())
    }

    fn activate_chapter(&mut self, index: usize) {
        if index >= self.project.manifest.chapters.len() || index == self.project.active_index() {
            return;
        }
        // Flush the chapter we're leaving before loading the next.
        if self.flush_active().is_err() {
            return;
        }
        match Buffer::from_path(self.project.chapter_path(index)) {
            Ok(buffer) => {
                self.buffer = buffer;
                self.project.set_active(index);
                // No need to cache the new active chapter's count: while active,
                // chapter_word_count reads it live from the buffer.
                if let Some(title) = self.project.active_title() {
                    self.message = Some(format!("Chapter: {title}"));
                }
            }
            Err(e) => self.message = Some(format!("Open failed: {e}")),
        }
    }

    fn chapter_count(&self) -> usize {
        self.project.manifest.chapters.len()
    }

    // --- Continuous-manuscript view ---------------------------------------

    /// Toggle between the single-chapter view and the continuous view.
    fn toggle_continuous(&mut self) {
        if self.manuscript.is_some() {
            self.leave_continuous();
        } else {
            self.enter_continuous();
        }
    }

    /// Enter the continuous view: flush the single chapter, then open a window
    /// of chapters around it.
    fn enter_continuous(&mut self) {
        if self.flush_active().is_err() {
            return;
        }
        self.manuscript = Some(Manuscript::open(&self.project));
        self.message = Some("Continuous view".into());
    }

    /// Leave the continuous view: save its chapters, then reopen the single
    /// chapter the cursor was in. Stays in continuous view if the save fails, so
    /// no edits are lost.
    fn leave_continuous(&mut self) {
        let Some(mut m) = self.manuscript.take() else {
            return;
        };
        if let Err(e) = m.save(&mut self.project) {
            self.message = Some(format!("Save failed: {e}"));
            self.manuscript = Some(m);
            return;
        }
        self.project.set_active(m.current_chapter());
        match Buffer::from_path(self.project.active_path()) {
            Ok(buffer) => {
                self.buffer = buffer;
                self.message = Some("Chapter view".into());
            }
            Err(e) => self.message = Some(format!("Open failed: {e}")),
        }
    }

    // --- Edit dispatch (single chapter or continuous) ----------------------

    fn edit_insert_char(&mut self, c: char) {
        match &mut self.manuscript {
            Some(m) => m.insert_char(c),
            None => self.buffer.insert_char(c),
        }
    }

    fn edit_backspace(&mut self) {
        match &mut self.manuscript {
            Some(m) => m.backspace(),
            None => self.buffer.backspace(),
        }
    }

    /// Apply a cursor movement to the active edit buffer, then page the window
    /// if the continuous view's cursor crossed into another chapter.
    fn edit_move(&mut self, mv: impl FnOnce(&mut Buffer)) {
        mv(self.edit_buffer_mut());
        if let Some(mut m) = self.manuscript.take() {
            if let Err(e) = m.reslide(&mut self.project) {
                self.message = Some(format!("Save failed: {e}"));
            }
            self.manuscript = Some(m);
        }
    }

    /// Save the active edit target (the continuous window, or the single chapter).
    fn save_edit(&mut self) {
        if let Some(mut m) = self.manuscript.take() {
            let result = m.save(&mut self.project);
            self.manuscript = Some(m);
            self.message = Some(match result {
                Ok(()) => "Saved".into(),
                Err(e) => format!("Save failed: {e}"),
            });
        } else {
            match self.buffer.save() {
                Ok(()) => {
                    self.project.set_active_words(self.buffer.word_count());
                    self.message = Some("Saved".into());
                }
                Err(e) => self.message = Some(format!("Save failed: {e}")),
            }
        }
        // Checkpoint today's momentum to the history log.
        self.persist_snapshot();
    }

    fn next_chapter(&mut self) {
        self.activate_chapter(self.project.active_index() + 1);
    }

    fn prev_chapter(&mut self) {
        // At chapter 0 this targets 0 — a no-op via activate_chapter's guard.
        self.activate_chapter(self.project.active_index().saturating_sub(1));
    }

    // --- Chapter structure editing ----------------------------------------
    //
    // Each wraps a `Project` op and returns the chapter the browser should now
    // select. The active chapter is tracked by file identity, so insert/rename/
    // move need no active fixup; only delete (of the active chapter) reloads.

    fn create_chapter(&mut self, at: usize, title: &str) -> usize {
        match self.project.add_chapter(at, title) {
            Ok(()) => self.message = Some(format!("Added '{title}'")),
            Err(e) => self.message = Some(format!("Add failed: {e}")),
        }
        at.min(self.chapter_count() - 1)
    }

    fn rename_chapter(&mut self, at: usize, title: &str) {
        match self.project.rename_chapter(at, title) {
            Ok(()) => self.message = Some(format!("Renamed to '{title}'")),
            Err(e) => self.message = Some(format!("Rename failed: {e}")),
        }
    }

    /// Move chapter `sel` up or down one slot; returns its new position (the
    /// active chapter follows its content automatically via identity tracking).
    fn move_chapter(&mut self, sel: usize, up: bool) -> usize {
        let target = if up {
            sel.checked_sub(1)
        } else {
            (sel + 1 < self.chapter_count()).then_some(sel + 1)
        };
        let Some(j) = target else { return sel };
        match self.project.swap_chapters(sel, j) {
            Ok(()) => j,
            Err(e) => {
                self.message = Some(format!("Move failed: {e}"));
                sel
            }
        }
    }

    /// Delete chapter `at`; returns the chapter the browser should select next.
    fn delete_chapter(&mut self, at: usize) -> usize {
        if self.chapter_count() <= 1 {
            self.message = Some(ONLY_CHAPTER_MSG.into());
            return at;
        }
        // Deleting the active chapter is the one structural edit that changes the
        // buffer's content — every other op keeps the active file present, so its
        // index just re-derives.
        let deleting_active = at == self.project.active_index();
        if deleting_active && self.buffer.dirty {
            // Persist the latest text first so the copy moved to unused/ is current.
            let _ = self.buffer.save();
        }
        // The chapter's words leave the manuscript — booked as "moved to unused"
        // today, not as words deleted (that's prose editing).
        let removed_words = self.chapter_word_count(at);
        if let Err(e) = self.project.remove_chapter(at) {
            self.message = Some(format!("Delete failed: {e}"));
            return at.min(self.chapter_count() - 1);
        }

        if deleting_active {
            // The active file is gone — point at a neighbor and reload.
            let neighbor = at.min(self.chapter_count() - 1);
            self.project.set_active(neighbor);
            match Buffer::from_path(self.project.active_path()) {
                Ok(buffer) => self.buffer = buffer,
                Err(e) => self.message = Some(format!("Open failed: {e}")),
            }
        }
        self.moved_to_unused += removed_words;
        // Absorb the structural drop so the next tally doesn't book it as removed:
        // re-baseline every chapter to its current count (this also drops the
        // deleted file's stale entry, since the map is rebuilt by file identity).
        self.seed_baselines();
        self.message = Some("Moved chapter to unused/".into());
        at.min(self.chapter_count() - 1)
    }

    // --- Chapter metadata (front matter) ----------------------------------

    /// Cycle the selected chapter's status (none → Draft → Revised → Final → …).
    fn cycle_status(&mut self, index: usize) {
        let next = match self.project.chapter_status(index) {
            None => Status::Draft,
            Some(s) => s.next(),
        };
        if self.edit_chapter_meta(index, |fm| fm.set_status(next)) {
            let title = self.project.chapter_title(index).unwrap_or("Chapter");
            self.message = Some(format!("{title}: {}", next.label()));
        }
    }

    /// Apply a front-matter edit to chapter `index`. The active chapter is edited
    /// through its live buffer (persisted on the next save, like prose); any other
    /// chapter is edited on disk immediately. Either way the pane cache is updated.
    /// Returns whether the edit succeeded.
    fn edit_chapter_meta(&mut self, index: usize, f: impl FnOnce(&mut FrontMatter)) -> bool {
        // Metadata is edited only from the chapter browser, which is always
        // single-chapter mode (Ctrl-G leaves the continuous view first). So the
        // active chapter's live front matter is `self.buffer`, never a manuscript
        // segment — if that ever changes, the active branch below must too.
        debug_assert!(
            self.manuscript.is_none(),
            "chapter metadata edited while the continuous view is active"
        );
        if index == self.project.active_index() {
            f(self.buffer.front_matter_mut());
            self.buffer.dirty = true;
            self.project.cache_chapter_meta(index, self.buffer.front_matter());
            true
        } else {
            match self.project.edit_chapter_front_matter(index, f) {
                Ok(()) => true,
                Err(e) => {
                    self.message = Some(format!("Save failed: {e}"));
                    false
                }
            }
        }
    }

    /// Apply a book/series metadata edit to the manifest and persist it.
    fn set_book_meta(&mut self, f: impl FnOnce(&mut Manifest)) {
        f(&mut self.project.manifest);
        if let Err(e) = self.project.save_manifest() {
            self.message = Some(format!("Save failed: {e}"));
        }
    }

    /// Write a metadata field's text — chapter fields go through the chapter's
    /// front matter, book fields through the manifest. The single place that maps
    /// a [`MetaField`] to where it's stored.
    fn apply_meta(&mut self, field: MetaField, text: &str) {
        // Optional fields clear to `None` on empty input; only one arm uses this.
        let optional = (!text.is_empty()).then(|| text.to_string());
        match field {
            MetaField::Summary { at } => {
                self.edit_chapter_meta(at, |fm| fm.set_summary(text));
            }
            MetaField::Pov { at } => {
                self.edit_chapter_meta(at, |fm| fm.set_pov(text));
            }
            MetaField::ChapterEpigraph { at } => {
                self.edit_chapter_meta(at, |fm| fm.set_epigraph_text(text));
            }
            MetaField::ChapterEpigraphBy { at } => {
                self.edit_chapter_meta(at, |fm| fm.set_epigraph_by(text));
            }
            // The book title is required, so empty input keeps the current one.
            MetaField::BookTitle => {
                if !text.is_empty() {
                    self.set_book_meta(|m| m.title = text.to_string());
                }
            }
            MetaField::Subtitle => self.set_book_meta(|m| m.subtitle = optional),
            MetaField::Author => self.set_book_meta(|m| m.author = optional),
            MetaField::Series => self.set_book_meta(|m| m.series = optional),
            MetaField::BookEpigraph => self.set_book_meta(|m| m.epigraph = optional),
            MetaField::BookEpigraphBy => self.set_book_meta(|m| m.epigraph_by = optional),
        }
    }

    /// The current epigraph of chapter `index`: from the live buffer if it's the
    /// active chapter (latest unsaved edits), otherwise read from its file.
    fn chapter_epigraph(&self, index: usize) -> Option<Epigraph> {
        if index == self.project.active_index() {
            self.buffer.front_matter().and_then(FrontMatter::epigraph)
        } else {
            self.project
                .chapter_front_matter(index)
                .and_then(|fm| fm.epigraph())
        }
    }

    /// Open a prompt. `nav_return` is the chapter to reselect when it closes
    /// (`None` returns to insert mode, e.g. the quit prompt).
    fn open_prompt(
        &mut self,
        pending: Pending,
        label: impl Into<String>,
        prefill: &str,
        nav_return: Option<usize>,
    ) {
        self.mode = Mode::Prompt(PromptState {
            pending,
            label: label.into(),
            input: prefill.to_string(),
            nav_return,
        });
    }

    /// Enter (or stay in) the chapter browser with `selected` highlighted.
    fn select(&mut self, selected: usize) {
        self.mode = Mode::Nav { selected };
    }

    /// The prompt state, while in [`Mode::Prompt`]. Only call from a prompt key
    /// handler, which has already matched the variant.
    fn prompt_mut(&mut self) -> &mut PromptState {
        match &mut self.mode {
            Mode::Prompt(state) => state,
            _ => unreachable!("prompt editing runs only in Prompt mode"),
        }
    }

    fn on_key_nav(&mut self, key: KeyEvent) {
        let Mode::Nav { selected: sel } = self.mode else {
            return;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.select(sel.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => {
                let last = self.chapter_count() - 1;
                self.select((sel + 1).min(last));
            }
            KeyCode::Enter => {
                self.activate_chapter(sel);
                self.mode = Mode::Insert;
            }
            // Reorder the selected chapter.
            KeyCode::Char('K') => {
                let s = self.move_chapter(sel, true);
                self.select(s);
            }
            KeyCode::Char('J') => {
                let s = self.move_chapter(sel, false);
                self.select(s);
            }
            // Add / rename / delete (each returns to the browser when done).
            KeyCode::Char('a') => {
                self.open_prompt(Pending::NewChapter { at: sel + 1 }, "New chapter title: ", "", Some(sel))
            }
            KeyCode::Char('r') => {
                let title = self.project.manifest.chapters[sel].title.clone();
                self.open_prompt(Pending::Rename { at: sel }, "Rename chapter: ", &title, Some(sel));
            }
            // Chapter metadata (front matter): status, summary, POV, epigraph.
            KeyCode::Char('s') => self.cycle_status(sel),
            KeyCode::Char('m') => {
                let prefill = self.project.chapter_summary(sel).unwrap_or("").to_string();
                self.open_prompt(Pending::SetMeta(MetaField::Summary { at: sel }), "Summary: ", &prefill, Some(sel));
            }
            KeyCode::Char('v') => {
                let prefill = self.project.chapter_pov(sel).unwrap_or("").to_string();
                self.open_prompt(Pending::SetMeta(MetaField::Pov { at: sel }), "POV: ", &prefill, Some(sel));
            }
            KeyCode::Char('g') => {
                let text = self.chapter_epigraph(sel).map(|e| e.text).unwrap_or_default();
                self.edit_meta(MetaField::ChapterEpigraph { at: sel }, "chapter epigraph", text, Some(sel));
            }
            // Project-level book / series metadata.
            KeyCode::Char('t') => {
                let cur = self.project.manifest.title.clone();
                self.open_prompt(Pending::SetMeta(MetaField::BookTitle), "Book title: ", &cur, Some(sel));
            }
            KeyCode::Char('b') => {
                let cur = self.project.manifest.subtitle.clone().unwrap_or_default();
                self.open_prompt(Pending::SetMeta(MetaField::Subtitle), "Subtitle: ", &cur, Some(sel));
            }
            KeyCode::Char('w') => {
                let cur = self.project.manifest.author.clone().unwrap_or_default();
                self.open_prompt(Pending::SetMeta(MetaField::Author), "Author: ", &cur, Some(sel));
            }
            KeyCode::Char('e') => {
                let cur = self.project.manifest.series.clone().unwrap_or_default();
                self.open_prompt(Pending::SetMeta(MetaField::Series), "Series: ", &cur, Some(sel));
            }
            KeyCode::Char('G') => {
                let text = self.project.manifest.epigraph.clone().unwrap_or_default();
                self.edit_meta(MetaField::BookEpigraph, "book epigraph", text, Some(sel));
            }
            KeyCode::Char('d') => {
                if self.chapter_count() <= 1 {
                    self.message = Some(ONLY_CHAPTER_MSG.into());
                } else {
                    let title = self.project.manifest.chapters[sel].title.clone();
                    self.open_prompt(
                        Pending::Delete { at: sel },
                        format!("Delete '{title}'? (y/N)"),
                        "",
                        Some(sel),
                    );
                }
            }
            KeyCode::Esc => self.mode = Mode::Insert,
            _ => {}
        }
    }

    fn on_key_prompt(&mut self, key: KeyEvent) {
        let Mode::Prompt(state) = &self.mode else {
            return;
        };
        let pending = state.pending;
        let nav_return = state.nav_return;

        // Where to go when the prompt closes without acting.
        let closed = closed_mode(nav_return);

        match pending {
            Pending::Quit => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.should_quit = true,
                _ => self.mode = closed,
            },
            Pending::Delete { at } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let selected = self.delete_chapter(at);
                    self.select(selected);
                }
                _ => self.mode = closed,
            },
            Pending::ConfirmAi(action) => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.mode = closed;
                    self.run_ai_action(action);
                }
                _ => self.mode = closed,
            },
            // The text prompts share the editing keys; only Enter differs.
            Pending::NewChapter { .. }
            | Pending::Rename { .. }
            | Pending::SetMeta(_)
            | Pending::AskAi
            | Pending::Commit => {
                match key.code {
                    KeyCode::Char(c) => self.prompt_mut().input.push(c),
                    KeyCode::Backspace => {
                        self.prompt_mut().input.pop();
                    }
                    KeyCode::Enter => {
                        let text = std::mem::take(&mut self.prompt_mut().input);
                        let text = text.trim();
                        let fallback = nav_return.unwrap_or(0);
                        match pending {
                            Pending::NewChapter { at } => {
                                let sel =
                                    if text.is_empty() { fallback } else { self.create_chapter(at, text) };
                                self.select(sel);
                            }
                            Pending::Rename { at } => {
                                if !text.is_empty() {
                                    self.rename_chapter(at, text);
                                }
                                self.select(fallback);
                            }
                            Pending::SetMeta(field) => {
                                // Single-line metadata prompts (summary, POV,
                                // attribution, book fields). The multi-line
                                // epigraph quotations are edited in the editor
                                // (Mode::EditMeta), which chains here for the
                                // attribution.
                                self.apply_meta(field, text);
                                self.select(fallback);
                            }
                            Pending::AskAi => {
                                // Back to editing immediately; the answer arrives
                                // asynchronously in the overlay.
                                self.mode = closed;
                                self.ask_ai(text);
                            }
                            Pending::Commit => {
                                self.mode = closed;
                                if text.is_empty() {
                                    self.message = Some("Commit cancelled — empty message.".into());
                                } else {
                                    self.commit(text);
                                }
                            }
                            // Confirmations never reach the text arm.
                            _ => self.select(fallback),
                        }
                    }
                    KeyCode::Esc => self.mode = closed,
                    _ => {}
                }
            }
        }
    }

    fn on_key_insert(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match (ctrl, key.code) {
            (true, KeyCode::Char('s')) => self.save_edit(),
            (true, KeyCode::Char('q')) => {
                if self.edit_dirty() {
                    self.open_prompt(Pending::Quit, "Unsaved changes — quit anyway? (y/N)", "", None);
                } else {
                    self.should_quit = true;
                }
            }
            (true, KeyCode::Char('=') | KeyCode::Char('+')) => {
                self.focus_width = (self.focus_width + 4).min(160);
            }
            (true, KeyCode::Char('-')) => {
                self.focus_width = self.focus_width.saturating_sub(4).max(32);
            }
            (true, KeyCode::Char('o')) => {
                self.outline_visible = !self.outline_visible;
            }
            // Put the project under version control (#54). Ctrl-v: "version control".
            (true, KeyCode::Char('v')) => self.init_version_control(),
            // Commit a snapshot of the project (#131). Ctrl-k: "checkpoint".
            (true, KeyCode::Char('k')) => self.start_commit(),
            // Open the progress report (scroll to read; any other key closes).
            (true, KeyCode::Char('r')) => {
                self.overlay = Some(Overlay::report("progress", self.report_lines()));
            }
            // Open the local analysis report — editing metrics. Ctrl-t: "text"
            // analysis. Scrollable (it can run past one screen); any other key closes.
            (true, KeyCode::Char('t')) => {
                self.overlay = Some(Overlay::report("analysis", self.analysis_lines()));
            }
            // Open the AI command palette (opt-in). When AI isn't ready, explain
            // how to enable it rather than listing dead actions.
            (true, KeyCode::Char('a')) => match self.ai.check_ready() {
                Ok(_) => self.open_ai_palette(),
                Err(e) => self.message = Some(e.to_string()),
            },
            // Toggle the continuous-manuscript view. Ctrl-l ("long" view), not
            // Ctrl-m — terminals deliver Ctrl-m as Enter.
            (true, KeyCode::Char('l')) => self.toggle_continuous(),
            // Step to the next / previous chapter (single-chapter view only;
            // in continuous view you just scroll across boundaries).
            (true, KeyCode::Char('n')) if self.manuscript.is_none() => self.next_chapter(),
            (true, KeyCode::Char('p')) if self.manuscript.is_none() => self.prev_chapter(),
            // Focus the chapter pane to browse. Chapter structure is edited in
            // the single-chapter view, so leave continuous first.
            (true, KeyCode::Char('g')) => {
                self.leave_continuous();
                self.outline_visible = true;
                self.mode = Mode::Nav {
                    selected: self.project.active_index(),
                };
            }

            (false, KeyCode::Char(c)) => self.edit_insert_char(c),
            (false, KeyCode::Enter) => self.edit_insert_char('\n'),
            (false, KeyCode::Backspace) => self.edit_backspace(),
            (false, KeyCode::Tab) => {
                for _ in 0..4 {
                    self.edit_insert_char(' ');
                }
            }

            (false, KeyCode::Left) => self.edit_move(Buffer::move_left),
            (false, KeyCode::Right) => self.edit_move(Buffer::move_right),
            (false, KeyCode::Up) => self.edit_move(Buffer::move_up),
            (false, KeyCode::Down) => self.edit_move(Buffer::move_down),
            (false, KeyCode::Home) => self.edit_move(Buffer::move_home),
            (false, KeyCode::End) => self.edit_move(Buffer::move_end),

            _ => {}
        }
    }

    // --- Editing a multi-line metadata field in the editor ------------------

    /// Open `target`'s text in the editor as a scratch buffer (`Mode::EditMeta`).
    fn edit_meta(
        &mut self,
        target: MetaField,
        label: &str,
        current: String,
        nav_return: Option<usize>,
    ) {
        self.mode = Mode::EditMeta(Box::new(MetaEdit {
            target,
            buffer: Buffer::from_text(&current),
            label: label.to_string(),
            nav_return,
        }));
    }

    fn on_key_edit_meta(&mut self, key: KeyEvent) {
        match (key.modifiers.contains(KeyModifiers::CONTROL), key.code) {
            (true, KeyCode::Char('s')) => self.commit_meta_edit(),
            (_, KeyCode::Esc) => self.cancel_meta_edit(),
            _ => {
                self.handle_text_edit_key(key);
            }
        }
    }

    /// Apply a plain text-editing key (insert / delete / move) to the active edit
    /// buffer; returns whether it was consumed. Shared by the metadata editor and
    /// the bible-conflict editor, which add their own Ctrl-S / Esc on top.
    fn handle_text_edit_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }
        match key.code {
            KeyCode::Char(c) => self.edit_buffer_mut().insert_char(c),
            KeyCode::Enter => self.edit_buffer_mut().insert_char('\n'),
            KeyCode::Backspace => self.edit_buffer_mut().backspace(),
            KeyCode::Tab => {
                for _ in 0..4 {
                    self.edit_buffer_mut().insert_char(' ');
                }
            }
            KeyCode::Left => self.edit_buffer_mut().move_left(),
            KeyCode::Right => self.edit_buffer_mut().move_right(),
            KeyCode::Up => self.edit_buffer_mut().move_up(),
            KeyCode::Down => self.edit_buffer_mut().move_down(),
            KeyCode::Home => self.edit_buffer_mut().move_home(),
            KeyCode::End => self.edit_buffer_mut().move_end(),
            _ => return false,
        }
        true
    }

    /// Store the edited quotation, then chain to the single-line attribution
    /// prompt (or return to the browser if the quotation was cleared).
    fn commit_meta_edit(&mut self) {
        let (target, nav_return, text) = match &self.mode {
            Mode::EditMeta(m) => (m.target, m.nav_return, m.buffer.text()),
            _ => return,
        };
        let text = text.trim();
        self.apply_meta(target, text);

        let fallback = nav_return.unwrap_or(0);
        if text.is_empty() {
            self.select(fallback);
            return;
        }
        match target {
            MetaField::ChapterEpigraph { at } => {
                let by = self.chapter_epigraph(at).and_then(|e| e.attribution).unwrap_or_default();
                self.open_prompt(
                    Pending::SetMeta(MetaField::ChapterEpigraphBy { at }),
                    "Epigraph by: ",
                    &by,
                    nav_return,
                );
            }
            MetaField::BookEpigraph => {
                let by = self.project.manifest.epigraph_by.clone().unwrap_or_default();
                self.open_prompt(
                    Pending::SetMeta(MetaField::BookEpigraphBy),
                    "Epigraph by: ",
                    &by,
                    nav_return,
                );
            }
            _ => self.select(fallback),
        }
    }

    fn cancel_meta_edit(&mut self) {
        let nav_return = match &self.mode {
            Mode::EditMeta(m) => m.nav_return,
            _ => return,
        };
        self.mode = closed_mode(nav_return);
    }

}

/// Where an overlay (prompt or metadata editor) returns when it closes without
/// acting: back to the chapter browser at `selected`, or to insert mode.
fn closed_mode(nav_return: Option<usize>) -> Mode {
    match nav_return {
        Some(selected) => Mode::Nav { selected },
        None => Mode::Insert,
    }
}

#[cfg(test)]
mod tests;
