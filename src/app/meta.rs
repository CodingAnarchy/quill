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

    /// Set (or clear) the selected file's section role (#179). A blank value
    /// clears it back to a body chapter; an unrecognized name is reported rather
    /// than applied (so a typo doesn't silently leave the role unchanged).
    pub(crate) fn set_chapter_role(&mut self, index: usize, text: &str) {
        let trimmed = text.trim();
        let role = if trimmed.is_empty() {
            None
        } else if let Some(r) = Role::from_key(trimmed) {
            Some(r)
        } else {
            self.message =
                Some(format!("Unknown section role '{trimmed}' — e.g. prologue, epilogue, appendix"));
            return;
        };
        if self.edit_chapter_meta(index, |fm| fm.set_role(role)) {
            let title = self.project.chapter_title(index).unwrap_or("Chapter");
            self.message = Some(match role {
                Some(r) => format!("{title}: {}", r.label()),
                None => format!("{title}: body chapter"),
            });
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
    pub(crate) fn apply_meta(&mut self, field: MetaField, text: &str) {
        // Optional fields clear to `None` on empty input; only one arm uses this.
        let optional = (!text.is_empty()).then(|| text.to_string());
        match field {
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
            MetaField::Role { at } => self.set_chapter_role(at, text),
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
