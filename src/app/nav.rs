//! Chapter-list and reference-file navigation modes — split out of `app/mod.rs` to keep it under the ~1,000-line
//! convention (#404). Pure move; `App`'s methods live across modules.

use super::*;

impl App {
    pub(super) fn on_key_nav(&mut self, key: KeyEvent) {
        let Mode::Nav {
            selected: sel,
            typing,
            ..
        } = self.mode
        else {
            return;
        };
        // In the filter-input sub-mode, keystrokes build the query (#410); the
        // browser's single-key commands are reachable only outside it.
        if typing {
            self.on_key_nav_filter(key, sel);
            return;
        }
        match key.code {
            // Start the type-to-filter / quick-jump query (#410).
            KeyCode::Char('/') => self.set_nav_typing(true),
            // Movement skips chapters filtered out of the list.
            KeyCode::Up | KeyCode::Char('k') => {
                let s = self.nav_step(sel, false);
                self.select(s);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let s = self.nav_step(sel, true);
                self.select(s);
            }
            KeyCode::Enter => {
                self.activate_chapter(sel);
                // Opening a chapter keeps the pane (it shows where you landed);
                // just drop the browser's claim on it (#529).
                self.browser_opened_outline = false;
                self.mode = self.editing_mode();
            }
            // Open the selected chapter in a side pane (#165) — compare two
            // chapters, or keep one as reference while editing another.
            KeyCode::Char('o') => {
                self.open_file_in_pane(self.project.chapter_path(sel));
                self.browser_opened_outline = false;
                self.mode = self.editing_mode();
            }
            // Switch to the reference-file browser (#165): bible entries and notes.
            KeyCode::Char('f') => self.open_ref_files(),
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
            KeyCode::Char('a') => self.open_prompt(
                Pending::NewChapter { at: sel + 1 },
                "New chapter title: ",
                "",
                Some(sel),
            ),
            KeyCode::Char('r') => {
                let title = self.project.manifest.chapters[sel].title.clone();
                self.open_prompt(
                    Pending::Rename { at: sel },
                    "Rename chapter: ",
                    &title,
                    Some(sel),
                );
            }
            // Chapter metadata (front matter): status, summary, POV, epigraph.
            KeyCode::Char('s') => self.cycle_status(sel),
            KeyCode::Char('m') => {
                let prefill = self.project.chapter_summary(sel).unwrap_or("").to_string();
                self.open_prompt(
                    Pending::SetMeta(MetaField::Summary { at: sel }),
                    "Summary: ",
                    &prefill,
                    Some(sel),
                );
            }
            KeyCode::Char('v') => {
                let prefill = self.project.chapter_pov(sel).unwrap_or("").to_string();
                self.open_prompt(
                    Pending::SetMeta(MetaField::Pov { at: sel }),
                    "POV: ",
                    &prefill,
                    Some(sel),
                );
            }
            KeyCode::Char('g') => {
                let text = self
                    .chapter_epigraph(sel)
                    .map(|e| e.text)
                    .unwrap_or_default();
                self.edit_meta(
                    MetaField::ChapterEpigraph { at: sel },
                    "chapter epigraph",
                    text,
                    Some(sel),
                );
            }
            // Set the chapter's word-count goal (#146); blank clears it.
            KeyCode::Char('l') => {
                let prefill = self
                    .project
                    .chapter_goal(sel)
                    .map(|g| g.to_string())
                    .unwrap_or_default();
                self.open_prompt(
                    Pending::SetMeta(MetaField::Goal { at: sel }),
                    "Word goal: ",
                    &prefill,
                    Some(sel),
                );
            }
            // Set the file's section role (#179); blank clears it to a body chapter.
            KeyCode::Char('S') => {
                let prefill = self
                    .project
                    .chapter_role(sel)
                    .map(|r| r.key().to_string())
                    .unwrap_or_default();
                self.open_prompt(
                    Pending::SetMeta(MetaField::Role { at: sel }),
                    "Section role (prologue/epilogue/appendix/…; blank = body): ",
                    &prefill,
                    Some(sel),
                );
            }
            // Project-level book / series metadata.
            KeyCode::Char('t') => {
                let cur = self.project.manifest.title.clone();
                self.open_prompt(
                    Pending::SetMeta(MetaField::BookTitle),
                    "Book title: ",
                    &cur,
                    Some(sel),
                );
            }
            KeyCode::Char('b') => {
                let cur = self.project.manifest.subtitle.clone().unwrap_or_default();
                self.open_prompt(
                    Pending::SetMeta(MetaField::Subtitle),
                    "Subtitle: ",
                    &cur,
                    Some(sel),
                );
            }
            KeyCode::Char('w') => {
                let cur = self.project.manifest.author.clone().unwrap_or_default();
                self.open_prompt(
                    Pending::SetMeta(MetaField::Author),
                    "Author: ",
                    &cur,
                    Some(sel),
                );
            }
            KeyCode::Char('e') => {
                let cur = self.project.manifest.series.clone().unwrap_or_default();
                self.open_prompt(
                    Pending::SetMeta(MetaField::Series),
                    "Series: ",
                    &cur,
                    Some(sel),
                );
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
            // Esc clears an active filter first (restoring the full list), then
            // leaves the browser on a second press (#410).
            KeyCode::Esc => {
                if self.nav_has_filter() {
                    self.clear_nav_filter();
                } else {
                    self.leave_browser();
                }
            }
            _ => {}
        }
    }

    /// Dismiss the chapter browser back to the editor (#529): if Ctrl-G was what
    /// opened the outline pane, hide it again so Esc puts the screen back exactly
    /// as it was; an outline the author had already toggled on with Ctrl-O stays.
    /// Routes through [`editing_mode`](Self::editing_mode) so it lands in Vim
    /// Normal or Insert per the user's config.
    fn leave_browser(&mut self) {
        if self.browser_opened_outline {
            self.outline_visible = false;
            self.browser_opened_outline = false;
        }
        self.mode = self.editing_mode();
    }

    /// Keys for the chapter browser's filter-input sub-mode (#410): printable
    /// characters build the query (live), Backspace edits it, the arrows move
    /// within the matches, Enter quick-jumps to the highlighted one, and Esc
    /// leaves input — keeping the filter so the browser's commands act on the
    /// narrowed list (a second Esc, in command mode, clears it).
    fn on_key_nav_filter(&mut self, key: KeyEvent, sel: usize) {
        match key.code {
            KeyCode::Esc => self.set_nav_typing(false),
            KeyCode::Enter => {
                if self.nav_chapter_matches(sel, &self.nav_query()) {
                    self.activate_chapter(sel);
                    self.browser_opened_outline = false;
                    self.mode = self.editing_mode();
                }
            }
            KeyCode::Up => {
                let s = self.nav_step(sel, false);
                self.select(s);
            }
            KeyCode::Down => {
                let s = self.nav_step(sel, true);
                self.select(s);
            }
            KeyCode::Backspace => self.nav_filter_pop(),
            KeyCode::Char(c) => self.nav_filter_push(c),
            _ => {}
        }
    }

    /// Whether chapter `index` passes the browser's active quick-jump filter
    /// (#410): a case-insensitive substring match on the chapter's title, status
    /// label, or POV — all cached metadata, so filtering reads no files and stays
    /// instant at series scale. `q` is the already-lowercased query (empty = all
    /// match), passed in so a full-list scan lowercases it once, not once per
    /// chapter.
    pub(crate) fn nav_chapter_matches(&self, index: usize, q: &str) -> bool {
        if q.is_empty() {
            return true;
        }
        self.project
            .chapter_title(index)
            .unwrap_or_default()
            .to_lowercase()
            .contains(q)
            || self
                .project
                .chapter_status(index)
                .is_some_and(|s| s.label().to_lowercase().contains(q))
            || self
                .project
                .chapter_pov(index)
                .is_some_and(|p| p.to_lowercase().contains(q))
    }

    /// The active quick-jump query, lowercased (empty when not browsing or with
    /// no filter), for [`nav_chapter_matches`](Self::nav_chapter_matches) (#410).
    pub(crate) fn nav_query(&self) -> String {
        match &self.mode {
            Mode::Nav { filter, .. } => filter.to_lowercase(),
            _ => String::new(),
        }
    }

    /// How many chapters pass the active filter (#410) — for the browser's
    /// status line; lowercases the query once for the whole scan.
    pub(crate) fn nav_match_count(&self) -> usize {
        let q = self.nav_query();
        (0..self.chapter_count())
            .filter(|&i| self.nav_chapter_matches(i, &q))
            .count()
    }

    fn nav_has_filter(&self) -> bool {
        matches!(&self.mode, Mode::Nav { filter, .. } if !filter.is_empty())
    }

    fn nav_selected(&self) -> usize {
        if let Mode::Nav { selected, .. } = &self.mode {
            *selected
        } else {
            0
        }
    }

    fn set_nav_typing(&mut self, on: bool) {
        if let Mode::Nav { typing, .. } = &mut self.mode {
            *typing = on;
        }
    }

    fn clear_nav_filter(&mut self) {
        if let Mode::Nav { filter, typing, .. } = &mut self.mode {
            filter.clear();
            *typing = false;
        }
    }

    fn nav_filter_push(&mut self, c: char) {
        if let Mode::Nav { filter, .. } = &mut self.mode {
            filter.push(c);
        }
        self.nav_snap();
    }

    fn nav_filter_pop(&mut self) {
        if let Mode::Nav { filter, .. } = &mut self.mode {
            filter.pop();
        }
        self.nav_snap();
    }

    /// After the filter changes, keep the selection on a visible chapter: if the
    /// current one no longer matches, jump to the first that does (#410).
    fn nav_snap(&mut self) {
        let q = self.nav_query();
        if !self.nav_chapter_matches(self.nav_selected(), &q) {
            if let Some(first) =
                (0..self.chapter_count()).find(|&i| self.nav_chapter_matches(i, &q))
            {
                self.select(first);
            }
        }
    }

    /// The next (or previous, by `forward`) chapter that passes the filter, or
    /// `from` unchanged when there's no further match in that direction (#410).
    fn nav_step(&self, from: usize, forward: bool) -> usize {
        let n = self.chapter_count();
        let q = self.nav_query();
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
            if self.nav_chapter_matches(i, &q) {
                return i;
            }
        }
    }

    /// Enter the reference-file browser (#165): the project's bible entries and
    /// notes. Stays in the chapter browser with a note when there are none.
    pub(super) fn open_ref_files(&mut self) {
        let files = self.project.reference_files();
        if files.is_empty() {
            self.message =
                Some("No reference files yet — add notes or a bible under the project.".into());
            return;
        }
        self.mode = Mode::RefFiles(Box::new(RefFiles { files, selected: 0 }));
    }

    /// Navigate the reference-file browser (#165): arrows move, Enter opens the
    /// selected file in a side pane, `f`/Esc return to the chapter browser.
    pub(super) fn on_key_reffiles(&mut self, key: KeyEvent) {
        let Mode::RefFiles(state) = &mut self.mode else {
            return;
        };
        if let Some(s) = assist::list_move(state.selected, state.files.len(), key.code) {
            state.selected = s;
            return;
        }
        match key.code {
            KeyCode::Enter => {
                let path = state.files[state.selected].clone();
                self.open_file_in_pane(path);
                self.browser_opened_outline = false;
                self.mode = self.editing_mode();
            }
            // Back to the chapter browser.
            KeyCode::Char('f') | KeyCode::Esc => self.select(self.project.active_index()),
            _ => {}
        }
    }
}
