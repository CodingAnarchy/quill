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
            // Go to a chapter directly by number or title (#588): a prompt that
            // jumps without scrolling the list.
            KeyCode::Char('#') => self.open_prompt(
                Pending::GoToChapter,
                "Go to chapter (number or title): ",
                "",
                Some(sel),
            ),
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
            // Split the active chapter at the cursor into two (#585). Only the
            // open chapter can be split — its live cursor is the seam — so point
            // the author at it rather than prompting for a title that won't apply.
            KeyCode::Char('x') => {
                if sel == self.project.active_index() {
                    self.open_prompt(
                        Pending::SplitChapter { at: sel },
                        "Title for the new chapter after the cursor: ",
                        "",
                        Some(sel),
                    );
                } else {
                    self.message = Some("Open a chapter (Enter) before splitting it.".into());
                }
            }
            // Merge the selected chapter into the one above it (#585).
            KeyCode::Char('M') => {
                if sel == 0 {
                    self.message = Some("No chapter above to merge into.".into());
                } else {
                    let title = self.project.manifest.chapters[sel].title.clone();
                    let above = self
                        .project
                        .chapter_title(sel - 1)
                        .unwrap_or("the chapter above")
                        .to_string();
                    self.open_prompt(
                        Pending::Merge { at: sel },
                        format!("Merge '{title}' into '{above}'? (y/N)"),
                        "",
                        Some(sel),
                    );
                }
            }
            // Duplicate the selected chapter (#585), prefilling a "(copy)" title.
            KeyCode::Char('c') => {
                let title = self.project.manifest.chapters[sel].title.clone();
                self.open_prompt(
                    Pending::Duplicate { at: sel },
                    "Title for the duplicate: ",
                    &format!("{title} (copy)"),
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

    /// Resolve a go-to-chapter query (#588) to a chapter index: a 1-based number
    /// (`"12"` → index 11, in range), else the first chapter whose title contains
    /// the query (case-insensitive). Returns `None` for a blank query, an
    /// out-of-range number, or no title match. A purely numeric query is treated
    /// as a number even if it would also match a title, so `#` + a chapter number
    /// is unambiguous.
    pub(crate) fn resolve_chapter_target(&self, query: &str) -> Option<usize> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        if let Ok(number) = query.parse::<usize>() {
            return (1..=self.chapter_count())
                .contains(&number)
                .then(|| number - 1);
        }
        let needle = query.to_lowercase();
        (0..self.chapter_count()).find(|&i| {
            self.project
                .chapter_title(i)
                .unwrap_or_default()
                .to_lowercase()
                .contains(&needle)
        })
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
        self.mode = Mode::RefFiles(Box::new(RefFiles {
            files,
            selected: 0,
            bible_only: false,
        }));
    }

    /// Open the story-bible browser (#590): the `bible/`-only view of the reference
    /// browser, reached from the Ctrl-K "Browse story bible" run-action. Enter
    /// opens the selected bible file in an editable pane (the shared #165 flow), so
    /// the author can read and edit it — the characters, timeline, setting, and
    /// outline the AI tracks — and save it back with Ctrl-S. Reports when the
    /// project has no bible yet, rather than opening an empty list.
    pub(super) fn open_bible_files(&mut self) {
        let files = self.project.bible_files();
        if files.is_empty() {
            self.message =
                Some("No story bible yet — run an AI tracking pass (Ctrl-A) to build one.".into());
            return;
        }
        // The browser draws in the outline strip (like the chapter browser), so
        // reveal it; remember when we opened it so Esc puts the view back (#529).
        self.leave_continuous();
        self.browser_opened_outline = !self.outline_visible;
        self.outline_visible = true;
        self.mode = Mode::RefFiles(Box::new(RefFiles {
            files,
            selected: 0,
            bible_only: true,
        }));
    }

    /// Navigate the reference-file browser (#165/#590): arrows move, Enter opens
    /// the selected file in an editable side pane. In the all-files view (#165)
    /// `f`/Esc return to the chapter browser; in the bible-only view (#590) Esc
    /// returns to the editor.
    pub(super) fn on_key_reffiles(&mut self, key: KeyEvent) {
        let Mode::RefFiles(state) = &mut self.mode else {
            return;
        };
        if let Some(s) = assist::list_move(state.selected, state.files.len(), key.code) {
            state.selected = s;
            return;
        }
        let bible_only = state.bible_only;
        match key.code {
            KeyCode::Enter => {
                let path = state.files[state.selected].clone();
                self.open_file_in_pane(path);
                self.browser_opened_outline = false;
                self.mode = self.editing_mode();
            }
            // The bible view (#590) is entered from the editor — Esc drops back
            // there, restoring the outline strip if this browser revealed it. The
            // all-files view (#165) is a sub-mode of the chapter browser, so
            // `f`/Esc return there.
            KeyCode::Esc if bible_only => self.leave_browser(),
            KeyCode::Char('f') | KeyCode::Esc => self.select(self.project.active_index()),
            _ => {}
        }
    }
}
