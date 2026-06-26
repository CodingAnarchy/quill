//! Chapter-list and reference-file navigation modes — split out of `app/mod.rs` to keep it under the ~1,000-line
//! convention (#404). Pure move; `App`'s methods live across modules.

use super::*;

impl App {
    pub(super) fn on_key_nav(&mut self, key: KeyEvent) {
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
                self.mode = self.editing_mode();
            }
            // Open the selected chapter in a side pane (#165) — compare two
            // chapters, or keep one as reference while editing another.
            KeyCode::Char('o') => {
                self.open_file_in_pane(self.project.chapter_path(sel));
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
            // Set the chapter's word-count goal (#146); blank clears it.
            KeyCode::Char('l') => {
                let prefill = self.project.chapter_goal(sel).map(|g| g.to_string()).unwrap_or_default();
                self.open_prompt(Pending::SetMeta(MetaField::Goal { at: sel }), "Word goal: ", &prefill, Some(sel));
            }
            // Set the file's section role (#179); blank clears it to a body chapter.
            KeyCode::Char('S') => {
                let prefill = self.project.chapter_role(sel).map(|r| r.key().to_string()).unwrap_or_default();
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

    /// Enter the reference-file browser (#165): the project's bible entries and
    /// notes. Stays in the chapter browser with a note when there are none.
    pub(super) fn open_ref_files(&mut self) {
        let files = self.project.reference_files();
        if files.is_empty() {
            self.message = Some("No reference files yet — add notes or a bible under the project.".into());
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
                self.mode = self.editing_mode();
            }
            // Back to the chapter browser.
            KeyCode::Char('f') | KeyCode::Esc => self.select(self.project.active_index()),
            _ => {}
        }
    }
}
