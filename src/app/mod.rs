//! Top-level application state and the key handling that drives editing. The
//! editor-mode types live in [`crate::mode`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use similar::{ChangeTag, TextDiff};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use ratatui::text::Line;

use crate::ai::{self, bible, Backend, ClaudeBackend, Outcome};
use crate::ai::character::{self, Character};
use crate::ai::continuity::{self, Continuity};
use crate::ai::setting::{self, Setting};
use crate::buffer::Buffer;
use crate::date;
use crate::frontmatter::{Epigraph, FrontMatter, Status};
use crate::history;
use crate::manuscript::Manuscript;
use crate::mode::{
    BibleConflict, BibleReview, Finding, Findings, MetaEdit, MetaField, Mode, Pending, PromptState,
};
use crate::project::{Manifest, Project};
use crate::report::{self, Momentum, PartDelta};

/// Shown when the author tries to delete a project's last remaining chapter.
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
    /// When `Some`, the progress report is open: its pre-computed rows, shown
    /// full-screen until any key dismisses it. Computed once on open so the
    /// report (which scans files for TODO markers) isn't rebuilt per frame.
    pub report: Option<Vec<Line<'static>>>,
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
    /// When `Some`, the AI answer overlay is open (mirrors `report`): shown
    /// full-screen until any key dismisses it.
    pub ai_panel: Option<Vec<Line<'static>>>,
    /// Receiver for an in-flight AI request running on a worker thread; `Some`
    /// while a request is pending. Polled each loop tick by [`App::poll_ai`].
    ai_rx: Option<Receiver<Outcome>>,
    /// Continuity findings waiting to be shown — stashed while a timeline
    /// conflict is resolved first, then opened by [`App::open_findings`].
    pending_findings: Vec<Finding>,
    /// One-line transient message shown in the status bar.
    pub message: Option<String>,
}

impl App {
    pub fn new(buffer: Buffer, project: Project) -> Self {
        let mut app = Self {
            buffer,
            project,
            manuscript: None,
            mode: Mode::Insert,
            should_quit: false,
            focus_width: 72,
            outline_visible: false,
            report: None,
            added: 0,
            removed: 0,
            moved_to_unused: 0,
            part_added: Vec::new(),
            part_removed: Vec::new(),
            baselines: HashMap::new(),
            ai: ai::Settings::load(),
            ai_panel: None,
            ai_rx: None,
            pending_findings: Vec::new(),
            message: None,
        };
        // Baselines for the daily word tally: edits move each chapter from here.
        let parts = app.project.manifest.parts.as_ref().map_or(0, Vec::len);
        app.part_added = vec![0; parts];
        app.part_removed = vec![0; parts];
        app.seed_baselines();
        app
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
    fn part_word_counts(&self) -> Vec<usize> {
        let Some(parts) = self.project.manifest.parts.as_ref() else {
            return Vec::new();
        };
        let chapters = self.chapter_count();
        parts
            .iter()
            .map(|p| {
                let start = p.chapters[0].saturating_sub(1); // 1-based, inclusive
                let end = p.chapters[1].min(chapters); // clamp to existing chapters
                (start..end).map(|i| self.chapter_word_count(i)).sum()
            })
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

    /// Build the per-part rows for the report: each part's display name (its
    /// `name`, or `"{label} {n}"` when unnamed), current word total, and today's
    /// added / removed within it.
    fn part_deltas(&self) -> Vec<PartDelta> {
        let Some(parts) = self.project.manifest.parts.as_ref() else {
            return Vec::new();
        };
        let label = self.project.manifest.part_label();
        let words = self.part_word_counts();
        parts
            .iter()
            .enumerate()
            // `words` and the per-part counters are all sized to the parts list.
            .map(|(i, p)| PartDelta {
                name: p.name.clone().unwrap_or_else(|| format!("{label} {}", i + 1)),
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

    // --- AI: "Ask the manuscript" -----------------------------------------

    /// The whole manuscript as plain text for an AI request: each chapter under
    /// its title heading, the active chapter live from its buffer and the rest
    /// from disk (the `count_todo` pattern).
    fn manuscript_text(&self) -> String {
        let mut out = String::new();
        for i in 0..self.chapter_count() {
            if let Some(title) = self.project.chapter_title(i) {
                out.push_str("# ");
                out.push_str(title);
                out.push_str("\n\n");
            }
            out.push_str(self.chapter_text(i).trim_end());
            out.push_str("\n\n");
        }
        out
    }

    /// Start an "Ask the manuscript" request on a worker thread. The manuscript
    /// is assembled here (on the main thread) and moved into the thread with the
    /// question and a backend; the result returns via `ai_rx`, polled each tick.
    /// A no-op with a status message when AI isn't ready or a request is already
    /// in flight.
    fn ask_ai(&mut self, question: &str) {
        let question = question.trim().to_string();
        if question.is_empty() {
            return;
        }
        self.spawn_ai("⟳ Sending to Claude…", move |backend, manuscript, model| {
            Outcome::Answer(backend.complete(&ai::build_qa_request(manuscript, &question, &model)))
        });
    }

    /// Start a character-extraction pass; profiles are written / conflicts queued
    /// by [`App::poll_ai`].
    fn analyze_characters(&mut self) {
        self.spawn_ai("⟳ Analyzing characters…", |backend, manuscript, model| {
            let req = character::build_request(manuscript, &model);
            Outcome::Characters(backend.complete(&req).and_then(|r| character::parse(&r.text)))
        });
    }

    /// Start a timeline & continuity pass; the timeline + findings are applied by
    /// [`App::poll_ai`].
    fn analyze_continuity(&mut self) {
        self.spawn_ai("⟳ Checking continuity…", |backend, manuscript, model| {
            let req = continuity::build_request(manuscript, &model);
            Outcome::Continuity(backend.complete(&req).and_then(|r| continuity::parse(&r.text)))
        });
    }

    /// Start a setting & worldbuilding pass; the setting bible + rule findings are
    /// applied by [`App::poll_ai`].
    fn analyze_setting(&mut self) {
        self.spawn_ai("⟳ Building setting bible…", |backend, manuscript, model| {
            let req = setting::build_request(manuscript, &model);
            Outcome::Setting(backend.complete(&req).and_then(|r| setting::parse(&r.text)))
        });
    }

    /// Run an AI request on a worker thread: assemble the manuscript here, hand it
    /// (with a backend + model) to `run` off-thread, and deliver the `Outcome` via
    /// `ai_rx` for [`App::poll_ai`]. A no-op with a status message when AI isn't
    /// ready or a request is already in flight.
    fn spawn_ai(
        &mut self,
        message: &str,
        run: impl FnOnce(ClaudeBackend, String, String) -> Outcome + Send + 'static,
    ) {
        if self.ai_rx.is_some() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        let key = match self.ai.check_ready() {
            Ok(key) => key.to_string(),
            Err(e) => {
                self.message = Some(e.to_string());
                return;
            }
        };
        let backend = ClaudeBackend::new(key);
        let model = self.ai.model.clone();
        let manuscript = self.manuscript_text();

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(run(backend, manuscript, model));
        });
        self.ai_rx = Some(rx);
        self.message = Some(message.to_string());
    }

    /// Poll the in-flight AI request (non-blocking). On a result, dispatch by
    /// feature and clear the pending state. Called once per loop iteration so the
    /// result lands within the poll window.
    pub fn poll_ai(&mut self) {
        let Some(rx) = &self.ai_rx else { return };
        match rx.try_recv() {
            Ok(outcome) => {
                self.ai_rx = None;
                self.message = None;
                self.ai_panel = None; // clear any prior overlay before showing this result
                match outcome {
                    Outcome::Answer(r) => self.apply_qa_result(r),
                    Outcome::Characters(r) => self.apply_characters(r),
                    Outcome::Continuity(r) => self.apply_continuity(r),
                    Outcome::Setting(r) => self.apply_setting(r),
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.ai_rx = None;
                self.message = Some("AI: request failed".into());
            }
        }
    }

    /// Apply a completed Q&A result: open the answer overlay on success, or set a
    /// status message on error. Shared by the async poll path and tests (which
    /// drive a mock backend synchronously).
    fn apply_qa_result(&mut self, result: Result<ai::Response, ai::AiError>) {
        match result {
            Ok(resp) => self.ai_panel = Some(ai::answer_to_lines(&resp)),
            Err(e) => self.message = Some(format!("AI: {e}")),
        }
    }

    /// Apply extracted characters: enrich with local mention stats, write each
    /// profile to `bible/characters/`, and either summarize (no conflicts) or
    /// queue hand-edited files for review.
    fn apply_characters(&mut self, result: Result<Vec<Character>, ai::AiError>) {
        self.ai_panel = None; // a new result supersedes any prior overlay
        let mut chars = match result {
            Ok(c) => c,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
        };
        if chars.is_empty() {
            self.message = Some("No characters found.".into());
            return;
        }
        for c in &mut chars {
            self.enrich_mentions(c);
        }

        let dir = self.project.root.join("bible").join("characters");
        let today = date::iso(date::today());
        let mut written = 0;
        let mut drift = 0;
        let mut conflicts = Vec::new();
        for c in &chars {
            drift += c.drift.len();
            let path = dir.join(format!("{}.md", bible::slug(&c.name)));
            match self.stage_bible(path, &c.name, |notes| character::render_profile(c, notes, &today)) {
                Ok(()) => written += 1,
                Err(conflict) => conflicts.push(conflict),
            }
        }

        if conflicts.is_empty() {
            self.ai_panel = Some(character_summary_lines(chars.len(), written, drift));
        } else {
            let pending = conflicts.len();
            self.mode = Mode::BibleReview(Box::new(BibleReview { conflicts, editing: None }));
            self.message = Some(format!("Wrote {written} · {pending} need review"));
        }
    }

    /// Write a story-bible file, or return a conflict if its generated block was
    /// hand-edited since the last pass. `render` receives the author's preserved
    /// Notes block and returns the full file content. `Ok(true)` when written.
    /// Shared by the character and continuity passes.
    fn stage_bible(
        &self,
        path: PathBuf,
        name: &str,
        render: impl Fn(&str) -> String,
    ) -> Result<(), BibleConflict> {
        let existing = std::fs::read_to_string(&path).ok();
        let notes = bible::extract_notes(existing.as_deref().unwrap_or(""));
        let proposed = render(&notes);
        if let Some(current) = &existing {
            if bible::is_hand_edited(current) {
                let diff = unified_diff(current, &proposed);
                return Err(BibleConflict { path, name: name.to_string(), proposed, diff });
            }
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, &proposed);
        Ok(())
    }

    /// Count a character's name/alias mentions across the manuscript and record
    /// the last chapter it appears in — local "screen time" / "last seen", free.
    fn enrich_mentions(&self, c: &mut Character) {
        let needles: Vec<String> = std::iter::once(&c.name)
            .chain(&c.aliases)
            .map(|n| n.to_lowercase())
            .filter(|n| !n.is_empty())
            .collect();
        let mut total = 0;
        let mut last = None;
        for i in 0..self.chapter_count() {
            let text = self.chapter_text(i).to_lowercase();
            let count: usize = needles.iter().map(|n| text.matches(n.as_str()).count()).sum();
            if count > 0 {
                total += count;
                last = self.project.chapter_title(i).map(str::to_string);
            }
        }
        c.mentions = total;
        c.last_chapter = last;
    }

    /// Apply a continuity pass: write the ordered timeline to `bible/timeline.md`
    /// (conflict-reviewed like any bible file) and open the findings overlay. If
    /// the timeline was hand-edited, resolve that first; the findings follow.
    fn apply_continuity(&mut self, result: Result<Continuity, ai::AiError>) {
        let cont = match result {
            Ok(c) => c,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
        };
        let findings = cont
            .findings
            .iter()
            .map(|f| Finding {
                summary: format!("{}: {}", f.kind, f.detail),
                chapters: f.chapters.clone(),
            })
            .collect();
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("timeline.md");
        self.apply_bible_with_findings(path, "timeline", |notes| {
            continuity::render_timeline(&cont, notes, &today)
        }, findings);
    }

    /// Apply a setting pass: write `bible/setting.md` and surface rule-violation
    /// findings.
    fn apply_setting(&mut self, result: Result<Setting, ai::AiError>) {
        let setting = match result {
            Ok(s) => s,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
        };
        let findings = setting
            .findings
            .iter()
            .map(|f| Finding {
                summary: format!("{}: {}", f.rule, f.violation),
                chapters: f.chapters.clone(),
            })
            .collect();
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("setting.md");
        self.apply_bible_with_findings(path, "setting", |notes| {
            setting::render(&setting, notes, &today)
        }, findings);
    }

    /// Write a single bible file and surface a set of findings — the shared shape
    /// of the continuity and setting passes. Clears any prior overlay, stashes the
    /// findings, writes the file, then opens the findings overlay — or, if the
    /// file was hand-edited, reviews that conflict first (`advance_bible_review`
    /// flushes the findings when the review queue drains).
    fn apply_bible_with_findings(
        &mut self,
        path: PathBuf,
        name: &str,
        render: impl Fn(&str) -> String,
        findings: Vec<Finding>,
    ) {
        self.ai_panel = None;
        self.pending_findings = findings;
        match self.stage_bible(path, name, render) {
            Ok(()) => self.open_findings(),
            Err(conflict) => {
                self.mode = Mode::BibleReview(Box::new(BibleReview {
                    conflicts: vec![conflict],
                    editing: None,
                }));
                self.message = Some(format!("{name} edited — review, then findings"));
            }
        }
    }

    /// Open the stashed findings as a triage overlay, or summarize when there are
    /// none (the bible file has already been written).
    fn open_findings(&mut self) {
        let items = std::mem::take(&mut self.pending_findings);
        if items.is_empty() {
            self.ai_panel = Some(vec![
                Line::raw("Analysis complete."),
                Line::raw(""),
                Line::raw("Written to the story bible. No findings."),
            ]);
        } else {
            self.mode = Mode::Findings(Box::new(Findings { items, selected: 0 }));
        }
    }

    /// Whether an AI request is currently in flight (for the status indicator).
    pub fn ai_busy(&self) -> bool {
        self.ai_rx.is_some()
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

        // The progress report and AI answer are read-only overlays: any key
        // closes them.
        if self.report.is_some() {
            self.report = None;
            return;
        }
        if self.ai_panel.is_some() {
            self.ai_panel = None;
            return;
        }

        // Non-binding patterns: dispatch on the variant without moving its data.
        match self.mode {
            Mode::Prompt(_) => self.on_key_prompt(key),
            Mode::Nav { .. } => self.on_key_nav(key),
            Mode::EditMeta(_) => self.on_key_edit_meta(key),
            Mode::BibleReview(_) => self.on_key_bible_review(key),
            Mode::Findings(_) => self.on_key_findings(key),
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
            // The text prompts share the editing keys; only Enter differs.
            Pending::NewChapter { .. }
            | Pending::Rename { .. }
            | Pending::SetMeta(_)
            | Pending::AskAi => {
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
            // Open the progress report (any key closes it).
            (true, KeyCode::Char('r')) => self.report = Some(self.report_lines()),
            // Ask the manuscript a question (AI, opt-in). When AI isn't ready,
            // explain how to enable it rather than opening the prompt.
            (true, KeyCode::Char('a')) => match self.ai.check_ready() {
                Ok(_) => self.open_prompt(Pending::AskAi, "Ask the manuscript: ", "", None),
                Err(e) => self.message = Some(e.to_string()),
            },
            // Build/update the character story bible (AI, opt-in).
            (true, KeyCode::Char('b')) => match self.ai.check_ready() {
                Ok(_) => self.analyze_characters(),
                Err(e) => self.message = Some(e.to_string()),
            },
            // Timeline & continuity check (AI, opt-in).
            (true, KeyCode::Char('t')) => match self.ai.check_ready() {
                Ok(_) => self.analyze_continuity(),
                Err(e) => self.message = Some(e.to_string()),
            },
            // Setting & worldbuilding bible (AI, opt-in).
            (true, KeyCode::Char('w')) => match self.ai.check_ready() {
                Ok(_) => self.analyze_setting(),
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

    // --- Story-bible conflict review --------------------------------------

    fn on_key_bible_review(&mut self, key: KeyEvent) {
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
                if let Mode::BibleReview(r) = &self.mode {
                    if let Some(front) = r.conflicts.first() {
                        let _ = std::fs::write(&front.path, &front.proposed);
                    }
                }
                self.advance_bible_review();
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
                self.mode = Mode::Insert;
                self.message = Some("Review cancelled — kept your versions".into());
            }
            _ => {}
        }
    }

    /// Persist the edited proposed version, then advance to the next conflict.
    fn save_bible_edit(&mut self) {
        if let Mode::BibleReview(r) = &mut self.mode {
            if let (Some(buf), Some(front)) = (&r.editing, r.conflicts.first()) {
                let _ = std::fs::write(&front.path, buf.text());
            }
            r.editing = None;
        }
        self.advance_bible_review();
    }

    /// Drop the resolved front conflict; close the review when the queue empties.
    fn advance_bible_review(&mut self) {
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
            self.mode = Mode::Insert;
            self.message = Some("Bible review complete".into());
        } else {
            self.open_findings();
        }
    }

    fn on_key_findings(&mut self, key: KeyEvent) {
        let Mode::Findings(f) = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                f.selected = f.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                f.selected = (f.selected + 1).min(f.items.len().saturating_sub(1));
            }
            KeyCode::Char('d') => {
                if f.selected < f.items.len() {
                    f.items.remove(f.selected);
                    if f.items.is_empty() {
                        self.mode = Mode::Insert;
                    } else {
                        f.selected = f.selected.min(f.items.len() - 1);
                    }
                }
            }
            KeyCode::Enter => {
                let target = f.items.get(f.selected).and_then(|it| it.chapters.first()).cloned();
                self.mode = Mode::Insert;
                if let Some(title) = target {
                    if let Some(i) = (0..self.chapter_count()).find(|&i| self.project.chapter_title(i) == Some(title.as_str())) {
                        self.activate_chapter(i);
                    } else {
                        self.message = Some(format!("Chapter '{title}' not found"));
                    }
                }
            }
            KeyCode::Esc => self.mode = Mode::Insert,
            _ => {}
        }
    }
}

/// A unified line diff (current → proposed), each line signed `+`/`-`/` `.
/// Computed once when a conflict is staged (not per frame).
fn unified_diff(current: &str, proposed: &str) -> Vec<String> {
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

/// The summary overlay shown after a clean character pass (no conflicts).
fn character_summary_lines(total: usize, written: usize, drift: usize) -> Vec<Line<'static>> {
    vec![
        Line::raw("Character analysis complete."),
        Line::raw(""),
        Line::raw(format!("Characters analyzed: {total}")),
        Line::raw(format!("Profiles written:    {written}  (bible/characters/)")),
        Line::raw(format!("Drift flags:         {drift}")),
        Line::raw(""),
        Line::raw("Open the bible files to review; the Notes section is yours to edit."),
    ]
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
