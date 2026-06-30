//! Chapter & book metadata editing (#150): status / summary / POV / goal /
//! epigraph and the book-level fields, plus the in-editor multi-line metadata
//! edit mode. Split out of `app/mod.rs`; `App` methods via `use super::*`.

use super::*;

impl App {
    /// Cycle the selected chapter's status (none → Draft → Revised → Final → …).
    pub(crate) fn cycle_status(&mut self, index: usize) {
        let next = match self.project.chapter_status(index) {
            None => Status::Draft,
            Some(s) => s.next(),
        };
        if self.edit_chapter_meta(index, |fm| fm.set_status(next)) {
            let title = self.project.chapter_title(index).unwrap_or("Chapter");
            self.message = Some(format!("{title}: {}", next.label()));
        }
    }

    /// Set (or clear) the selected file's section role (#179), then re-sort it
    /// into its band (#180). A blank value clears it back to a body chapter; an
    /// unrecognized name is reported rather than applied (so a typo doesn't
    /// silently leave the role unchanged). Returns the file's index after the
    /// re-sort, so the browser selection can follow it.
    pub(crate) fn set_chapter_role(&mut self, index: usize, text: &str) -> Option<usize> {
        let trimmed = text.trim();
        let role = if trimmed.is_empty() {
            None
        } else if let Some(r) = Role::from_key(trimmed) {
            Some(r)
        } else {
            self.message = Some(format!(
                "Unknown section role '{trimmed}' — e.g. prologue, epilogue, appendix"
            ));
            return None;
        };
        let file = self.project.chapter_file(index).to_string();
        if !self.edit_chapter_meta(index, |fm| fm.set_role(role)) {
            return None;
        }
        // The new role may move the file into another band — re-sort and persist
        // the order (#180).
        if self.project.reorder_by_band() {
            if let Err(e) = self.project.save_manifest() {
                self.message = Some(format!("Save failed: {e}"));
            }
        }
        let moved = self.project.index_of_file(&file);
        let title = moved
            .and_then(|i| self.project.chapter_title(i))
            .unwrap_or("Chapter");
        self.message = Some(match role {
            Some(r) => format!("{title}: {}", r.label()),
            None => format!("{title}: body chapter"),
        });
        moved
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
            self.project
                .cache_chapter_meta(index, self.buffer.front_matter());
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
    /// a [`MetaField`] to where it's stored. Returns a new browser selection index
    /// when the edit re-sorted the chapter list (a role change, #180), else `None`.
    pub(crate) fn apply_meta(&mut self, field: MetaField, text: &str) -> Option<usize> {
        // Epigraphs (chapter & book, quotation & attribution) are display prose
        // that ships in the EPUB/PDF, so they get the same smart-typography pass
        // as the body when the setting is on — committed whole here rather than
        // per-keystroke, but through the shared `typography` core so the two paths
        // can't drift (#519). Off, or any other field, leaves the text verbatim.
        let smartened;
        let text = if self.smart_typography && field.is_epigraph() {
            smartened = typography::smarten(text);
            smartened.as_str()
        } else {
            text
        };
        // Optional fields clear to `None` on empty input; only one arm uses this.
        let optional = (!text.is_empty()).then(|| text.to_string());
        match field {
            MetaField::Role { at } => return self.set_chapter_role(at, text),
            MetaField::Summary { at } => {
                self.edit_chapter_meta(at, |fm| fm.set_summary(text));
            }
            MetaField::Pov { at } => {
                self.edit_chapter_meta(at, |fm| fm.set_pov(text));
            }
            MetaField::Goal { at } => {
                // A blank, non-positive, or unparseable value clears the goal.
                let goal = text.trim().parse::<usize>().ok().filter(|&n| n > 0);
                self.edit_chapter_meta(at, |fm| fm.set_goal(goal));
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
        None // only a role change re-sorts the list
    }

    // --- Project goals (#550) ----------------------------------------------
    //
    // Mirrors the chapter word-goal flow (a numeric prompt with the current value
    // prefilled), but walks the project's goals in turn: target words, target
    // chapters, then the optional daily target. Each is persisted to the `[goals]`
    // table of `book.toml` as it's entered, so the progress report (Ctrl-R)
    // reflects it immediately.

    /// Start the goal-setting flow (`goals:set`, #550): prompt for the first goal
    /// field, prefilled with its current value. Enter on each saves it and chains
    /// to the next; Esc stops the chain.
    pub(crate) fn start_set_goals(&mut self) {
        self.prompt_goal(GoalField::Words);
    }

    /// Open the numeric prompt for one goal field, prefilled with the current
    /// value (blank when unset). The required fields say their value must be
    /// positive; the optional daily target says blank clears it.
    pub(crate) fn prompt_goal(&mut self, field: GoalField) {
        let current = self.current_goal(field);
        let prefill = current.map(|n| n.to_string()).unwrap_or_default();
        let hint = if field.is_optional() {
            "blank clears it"
        } else {
            "a positive number"
        };
        let label = format!("{} ({hint}): ", capitalize(field.label()));
        self.open_prompt(Pending::SetGoal(field), label, &prefill, None);
    }

    /// The current value of goal `field` for prefilling the prompt. Words and
    /// chapters read 0 (unset) as absent, so a brand-new project starts blank;
    /// words-per-page prefills its current value or the standard default (250), so
    /// it's a single Enter to keep. The daily target prefills only when set.
    fn current_goal(&self, field: GoalField) -> Option<usize> {
        let goals = self.project.goals();
        match field {
            GoalField::Words => goals.map(|g| g.words).filter(|&n| n > 0),
            GoalField::Chapters => goals.map(|g| g.chapters).filter(|&n| n > 0),
            GoalField::WordsPerPage => {
                Some(goals.map_or(DEFAULT_WORDS_PER_PAGE, |g| g.words_per_page))
            }
            GoalField::Daily => goals.and_then(|g| g.daily),
        }
    }

    /// Apply one goal prompt's input (#550): clear an optional goal on blank,
    /// reject a blank/non-positive/non-numeric value for a required one, else
    /// persist it. On success, chain to the next field in the flow; on a rejected
    /// value, re-open the same prompt with the message shown.
    pub(crate) fn apply_goal(&mut self, field: GoalField, text: &str) {
        let text = text.trim();
        let value = if text.is_empty() {
            if !field.is_optional() {
                self.message = Some(format!(
                    "{} must be a positive number.",
                    capitalize(field.label())
                ));
                self.prompt_goal(field);
                return;
            }
            None // blank clears the optional daily target
        } else {
            match text.parse::<usize>() {
                Ok(n) if n > 0 => Some(n),
                _ => {
                    self.message = Some(format!(
                        "{} must be a positive whole number — got '{text}'.",
                        capitalize(field.label())
                    ));
                    self.prompt_goal(field);
                    return;
                }
            }
        };
        if let Err(e) = self.project.set_goal(field, value) {
            self.message = Some(format!("Couldn't save goals: {e}"));
            self.mode = self.editing_mode();
            return;
        }
        match next_goal_field(field) {
            Some(next) => self.prompt_goal(next),
            None => {
                self.mode = self.editing_mode();
                self.message = Some("Writing goals updated.".into());
            }
        }
    }

    // --- Book details (#571) -----------------------------------------------
    //
    // Mirrors the chained goal flow (#550): a first-class "Book details…"
    // run-action walks the book-level manifest fields in turn — title → subtitle
    // → author → series — each persisted as it's entered, so the same fields the
    // chapter-browser single-letter keys (`t`/`b`/`w`/`e`) set are now reachable
    // (and discoverable) from the run-action palette. Each step routes through the
    // shared [`apply_meta`] setter, so an app-set field reads back identical to a
    // single-letter-key one.

    /// Start the book-details flow (`book:details`, #571): prompt for the first
    /// field (title), prefilled with its current value. Enter on each saves it and
    /// chains to the next; Esc stops the chain.
    pub(crate) fn start_book_details(&mut self) {
        self.prompt_book_field(BookField::Title);
    }

    /// Open the text prompt for one book-details field, prefilled with the current
    /// manifest value (blank when unset).
    pub(crate) fn prompt_book_field(&mut self, field: BookField) {
        let current = self.current_book_field(field);
        let label = format!("{}: ", field.label());
        self.open_prompt(Pending::SetBookField(field), label, &current, None);
    }

    /// The current manifest value of book `field`, for prefilling the prompt.
    fn current_book_field(&self, field: BookField) -> String {
        let m = &self.project.manifest;
        match field {
            BookField::Title => m.title.clone(),
            BookField::Subtitle => m.subtitle.clone().unwrap_or_default(),
            BookField::Author => m.author.clone().unwrap_or_default(),
            BookField::Series => m.series.clone().unwrap_or_default(),
        }
    }

    /// Apply one book-details prompt's input (#571): persist it through the shared
    /// metadata setter (so it stores and reads back exactly like the chapter-browser
    /// single-letter keys), then chain to the next field — or finish on the last.
    pub(crate) fn apply_book_field(&mut self, field: BookField, text: &str) {
        self.apply_meta(field.meta(), text);
        match next_book_field(field) {
            Some(next) => self.prompt_book_field(next),
            None => {
                self.mode = self.editing_mode();
                self.message = Some("Book details updated.".into());
            }
        }
    }

    /// The current epigraph of chapter `index`: from the live buffer if it's the
    /// active chapter (latest unsaved edits), otherwise read from its file.
    pub(crate) fn chapter_epigraph(&self, index: usize) -> Option<Epigraph> {
        if index == self.project.active_index() {
            self.buffer.front_matter().and_then(FrontMatter::epigraph)
        } else {
            self.project
                .chapter_front_matter(index)
                .and_then(|fm| fm.epigraph())
        }
    }

    // --- Editing a multi-line metadata field in the editor ------------------

    /// Open `target`'s text in the editor as a scratch buffer (`Mode::EditMeta`).
    pub(crate) fn edit_meta(
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

    pub(crate) fn on_key_edit_meta(&mut self, key: KeyEvent) {
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
    pub(crate) fn handle_text_edit_key(&mut self, key: KeyEvent) -> bool {
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
            KeyCode::Up => {
                let w = self.wrap_w();
                self.edit_buffer_mut().move_up(w);
            }
            KeyCode::Down => {
                let w = self.wrap_w();
                self.edit_buffer_mut().move_down(w);
            }
            KeyCode::Home => {
                let w = self.wrap_w();
                self.edit_buffer_mut().move_home(w);
            }
            KeyCode::End => {
                let w = self.wrap_w();
                self.edit_buffer_mut().move_end(w);
            }
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
                let by = self
                    .chapter_epigraph(at)
                    .and_then(|e| e.attribution)
                    .unwrap_or_default();
                self.open_prompt(
                    Pending::SetMeta(MetaField::ChapterEpigraphBy { at }),
                    "Epigraph by: ",
                    &by,
                    nav_return,
                );
            }
            MetaField::BookEpigraph => {
                let by = self
                    .project
                    .manifest
                    .epigraph_by
                    .clone()
                    .unwrap_or_default();
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
        self.mode = self.closed_mode(nav_return);
    }
}

/// The next field in the goal-setting flow (#550): the three motivating goals
/// first (words → chapters → daily), then words-per-page last (rarely changed,
/// prefilled with its current/default value, so it's a single Enter to keep).
fn next_goal_field(field: GoalField) -> Option<GoalField> {
    match field {
        GoalField::Words => Some(GoalField::Chapters),
        GoalField::Chapters => Some(GoalField::Daily),
        GoalField::Daily => Some(GoalField::WordsPerPage),
        GoalField::WordsPerPage => None,
    }
}

/// The next field in the book-details flow (#571): title → subtitle → author →
/// series, then done. Walks the same fields the chapter-browser `t`/`b`/`w`/`e`
/// keys set, in that order.
fn next_book_field(field: BookField) -> Option<BookField> {
    match field {
        BookField::Title => Some(BookField::Subtitle),
        BookField::Subtitle => Some(BookField::Author),
        BookField::Author => Some(BookField::Series),
        BookField::Series => None,
    }
}

/// Capitalize the first character of a label for a prompt / message
/// (e.g. "target words" → "Target words").
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
