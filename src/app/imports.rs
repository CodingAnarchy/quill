//! Notes / manuscript import flows (#283/#286/#298): importing Markdown notes
//! into the story bible, starting a new project from a folder of notes, and
//! importing an existing Markdown manuscript as a chaptered project. Split out of
//! `assist.rs` to keep that module under the ~1,000-line convention (#426); a
//! child module, so it keeps access to `App`'s private fields.

use super::assist::write_failure_message;
use super::bible_review::{Staged, StagedTally};
use super::*;
use crate::ai::scaffold;

/// The source stashed between an import wizard's two prompts (#508): the folder
/// of notes to seed a new project's bible from (#286), or the manuscript file to
/// split into chapters (#298). Held in `App::pending_import_source` while the
/// destination prompt is open, then taken by the matching `finish_*`.
pub enum PendingImportSource {
    /// A folder of Markdown notes to import into the new project's bible (#286).
    NotesFolder(PathBuf),
    /// A Markdown / Word / Scrivener manuscript to split into chapters (#298).
    Manuscript(PathBuf),
}

impl App {
    /// Stash `pending` and open the new-project destination prompt under `dest`,
    /// prefilled with `"{base}/"` so the author only types the project's folder
    /// name (#508). Shared by both import wizards' `begin_*` steps.
    fn prompt_new_project_dest(&mut self, pending: PendingImportSource, dest: Pending) {
        self.pending_import_source = Some(pending);
        let prefill = format!("{}/", self.new_project_base().display());
        self.open_prompt(dest, "New project — path:", &prefill, None);
    }

    // --- Notes import (#283) ----------------------------------------------

    /// Prompt for a notes file (or a folder of them) to import into the story
    /// bible. Available with AI off too — the source is preserved regardless.
    pub(crate) fn start_import(&mut self) {
        self.open_prompt(
            Pending::ImportNotes,
            "Import notes — path to a .md file or a folder of them:",
            "",
            None,
        );
    }

    // --- New project from a folder of notes (#286) ------------------------

    /// Prompt for a folder of notes to start a new project from.
    pub(crate) fn start_new_from_notes(&mut self) {
        self.open_prompt(
            Pending::NewFromNotesSource,
            "New project from notes — folder to import:",
            "",
            None,
        );
    }

    /// Validate `folder` holds importable notes, then ask where to create the new
    /// project. Shared by the folder prompt and the directory browser's `N` key.
    pub(crate) fn begin_new_from_notes(&mut self, folder: &str) {
        let folder = folder.trim();
        if collect_import_files(std::path::Path::new(folder)).is_empty() {
            self.message = Some(format!("No Markdown notes to import at {folder}"));
            self.mode = self.editing_mode();
            return;
        }
        let folder = PathBuf::from(folder);
        self.prompt_new_project_dest(
            PendingImportSource::NotesFolder(folder),
            Pending::NewFromNotesDest,
        );
    }

    /// Create the new project at `dest` and import the folder chosen earlier.
    pub(crate) fn finish_new_from_notes(&mut self, dest: &str) {
        let Some(PendingImportSource::NotesFolder(folder)) = self.pending_import_source.take()
        else {
            self.mode = self.editing_mode();
            return;
        };
        if self.create_and_open(std::path::Path::new(dest.trim())) {
            // The new project is now active; import the notes into its bible.
            self.import_notes(&folder.to_string_lossy());
        }
    }

    // --- New project from an existing manuscript (#298) -------------------

    /// Prompt for a manuscript (Markdown / Word / Scrivener / plain text, or a
    /// folder of Markdown chapters) to import as a new project.
    pub(crate) fn start_import_manuscript(&mut self) {
        self.open_prompt(
            Pending::ImportManuscriptSource,
            "Import manuscript — .md, .txt, .docx, .scriv, or a folder of .md chapters:",
            "",
            None,
        );
    }

    /// Validate the chosen path is an importable manuscript, then ask where to
    /// create the new project. Accepts a `.md` / `.txt` / `.docx` file, a `.scriv`
    /// bundle directory, or a plain folder of per-chapter `.md` files. Shared by
    /// the file prompt and the run-action entry point.
    pub(crate) fn begin_import_manuscript(&mut self, file: &str) {
        let path = std::path::Path::new(file.trim());
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        // A `.scriv` is a bundle directory; a plain directory is importable when
        // it holds `.md` chapters (#593); the file formats are single files.
        let importable = if ext.eq_ignore_ascii_case("scriv") {
            path.is_dir()
        } else if path.is_dir() {
            !collect_import_files(path).is_empty()
        } else {
            path.is_file()
                && (ext.eq_ignore_ascii_case("md")
                    || ext.eq_ignore_ascii_case("txt")
                    || ext.eq_ignore_ascii_case("docx"))
        };
        if !importable {
            self.message = Some(format!(
                "Not an importable manuscript (.md, .txt, .docx, .scriv, or a folder of .md): {}",
                path.display()
            ));
            self.mode = self.editing_mode();
            return;
        }
        let source = path.to_path_buf();
        self.prompt_new_project_dest(
            PendingImportSource::Manuscript(source),
            Pending::ImportManuscriptDest,
        );
    }

    /// Parse the manuscript chosen earlier and open the import-preview (#702)
    /// instead of writing straight to disk: the author sees the chapters the split
    /// produced (titles + count), the heading level it split on, and any
    /// dropped-element warnings, then confirms or cancels. The write+open runs only
    /// on confirm ([`confirm_import_preview`](Self::confirm_import_preview)). The
    /// source is read only — never moved or altered.
    pub(crate) fn finish_import_manuscript(&mut self, dest: &str) {
        let Some(PendingImportSource::Manuscript(source)) = self.pending_import_source.take()
        else {
            self.mode = self.editing_mode();
            return;
        };
        let dest = std::path::PathBuf::from(dest.trim());
        if Project::manifest_path(&dest).exists() {
            self.message = Some(format!("A project already exists at {}", dest.display()));
            self.mode = self.editing_mode();
            return;
        }
        // A Markdown source is re-splittable at a chosen level, so keep its text;
        // other formats (`.docx`/`.scriv`/folder/`.txt`) don't split on a Markdown
        // heading level, so the preview offers no level toggle for them.
        let ext = source
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let markdown = (ext == "md" && source.is_file())
            .then(|| std::fs::read_to_string(&source).ok())
            .flatten();

        let parsed = match read_manuscript(&source) {
            Ok(p) => p,
            Err(e) => {
                self.message = Some(format!("Couldn't read {}: {e}", source.display()));
                self.mode = self.editing_mode();
                return;
            }
        };
        // Some documents may have been listed but unreadable (e.g. a corrupt
        // `.scriv` bundle): the body comes in empty and the parser counts it, so
        // we can warn the author rather than report a clean success (#511). The
        // scanners also tally structural content they can't carry — footnotes
        // (converted to inline markers), images, tables, comments (#646).
        let warning = import_warning_suffix(parsed.unreadable_bodies, parsed.dropped);
        let chapters: Vec<(String, String)> = parsed
            .chapters
            .into_iter()
            .map(|c| (c.title, c.body))
            .collect();

        // For a Markdown source, record which heading level the split used and the
        // levels the manuscript actually uses, so the preview can re-split on them.
        let (level, levels) = match &markdown {
            Some(text) => {
                use crate::manuscript_import as import;
                let (_, body) = crate::frontmatter::split(text);
                let level = import::chapter_level(body);
                let levels: Vec<usize> = (1..=6)
                    .filter(|&l| import::heading_count(body, l) > 0)
                    .collect();
                (Some(level), levels)
            }
            None => (None, Vec::new()),
        };

        self.mode = Mode::ImportPreview(Box::new(ImportPreview {
            source,
            dest,
            markdown,
            title: parsed.title,
            author: parsed.author,
            chapters,
            level,
            levels,
            warning,
            scroll: 0,
        }));
    }

    /// Handle a key while the import preview (#702) is open: arrows / j-k / paging
    /// scroll the chapter list, `1`–`6` re-split a Markdown source at that heading
    /// level (re-previewing without writing), Enter confirms the import, and Esc
    /// (or any other key) cancels — writing nothing.
    pub(crate) fn on_key_import_preview(&mut self, key: KeyEvent) {
        let Mode::ImportPreview(preview) = &mut self.mode else {
            return;
        };
        let last = preview.chapters.len().saturating_sub(1);
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => preview.scroll = (preview.scroll + 1).min(last),
            KeyCode::Up | KeyCode::Char('k') => preview.scroll = preview.scroll.saturating_sub(1),
            KeyCode::PageDown => preview.scroll = (preview.scroll + 10).min(last),
            KeyCode::PageUp => preview.scroll = preview.scroll.saturating_sub(10),
            KeyCode::Home => preview.scroll = 0,
            KeyCode::End => preview.scroll = last,
            KeyCode::Char(c @ '1'..='6') => {
                let level = c.to_digit(10).unwrap() as usize;
                self.repreview_import_at(level);
            }
            KeyCode::Enter => self.confirm_import_preview(),
            _ => {
                self.mode = self.editing_mode();
                self.message = Some("Import cancelled.".into());
            }
        }
    }

    /// Re-split the previewed Markdown manuscript (#702) at heading `level` and
    /// refresh the preview in place — nothing is written. A no-op for a level the
    /// source doesn't use, or a non-Markdown source (which has no level to toggle).
    fn repreview_import_at(&mut self, level: usize) {
        let Mode::ImportPreview(preview) = &mut self.mode else {
            return;
        };
        let Some(text) = preview.markdown.as_deref() else {
            return;
        };
        if !preview.levels.contains(&level) {
            return;
        }
        let parsed = crate::manuscript_import::parse_markdown_at(text, level);
        preview.title = parsed.title;
        preview.author = parsed.author;
        preview.chapters = parsed
            .chapters
            .into_iter()
            .map(|c| (c.title, c.body))
            .collect();
        preview.level = Some(level);
        preview.scroll = 0;
    }

    /// Confirm the import preview (#702): create the project at the chosen
    /// destination, write the previewed chapters in reading order, and open it.
    /// Runs only when the author confirms — Esc from the preview writes nothing.
    pub(crate) fn confirm_import_preview(&mut self) {
        let editing = self.editing_mode();
        let Mode::ImportPreview(preview) = std::mem::replace(&mut self.mode, editing) else {
            return;
        };
        let ImportPreview {
            source,
            dest,
            title,
            author,
            chapters,
            warning,
            ..
        } = *preview;
        let built = Project::create(&dest).and_then(|mut p| {
            p.import_chapters(title.as_deref(), author.as_deref(), &chapters)
                .map(|()| chapters.len())
        });
        match built {
            Ok(n) => {
                self.switch_to(dest); // open the finished project fresh from disk
                self.message = Some(format!(
                    "Imported {n} chapters{warning} from {}",
                    source.display()
                ));
            }
            Err(e) => {
                self.message = Some(format!("Import failed: {e}"));
                // `editing_mode()` is already set; leave it so the author lands back
                // in the editor with the failure message.
            }
        }
    }

    /// Import Markdown notes into the story bible: preserve every source file
    /// under `bible/sources/`, then (when AI is on) run one extractive pass over
    /// their combined text to generate/update the bible. A folder imports all the
    /// `.md` files directly inside it, collectively.
    pub(crate) fn import_notes(&mut self, path: &str) {
        let path = std::path::PathBuf::from(path.trim());
        let files = collect_import_files(&path);
        if files.is_empty() {
            self.message = Some(format!("No Markdown files to import at {}", path.display()));
            return;
        }

        // Preserve each source and gather its text, labelled so the AI can tell
        // the files apart in the combined input.
        let dest_dir = self.project.root.join("bible").join("sources");
        let mut combined = String::new();
        let mut preserved = 0;
        let mut failures = Vec::new();
        for f in &files {
            let text = match std::fs::read_to_string(f) {
                Ok(t) => t,
                Err(e) => {
                    failures.push(format!("{}: {e}", f.display()));
                    continue;
                }
            };
            let name = f
                .file_name()
                .map_or_else(|| "notes.md".into(), |n| n.to_string_lossy().into_owned());
            // Two imported folders can each hold a `characters.md`; keying only by
            // filename would let the second silently overwrite the first. Give
            // colliding names unique destinations so no source is lost.
            let dest = unique_dest(&dest_dir, &name);
            if let Err(msg) = self.write_file(&dest, &text) {
                failures.push(msg);
                continue;
            }
            preserved += 1;
            combined.push_str(&format!("## {name}\n\n{text}\n\n"));
        }
        if preserved == 0 {
            self.message = Some(write_failure_message(&failures));
            return;
        }

        let label = match &files[..] {
            [one] => one
                .file_name()
                .map_or_else(|| "notes".into(), |n| n.to_string_lossy().into_owned()),
            _ => format!("{preserved} files"),
        };
        // With AI off, the source is safely preserved but the bible can't be built.
        if self.ai.check_ready().is_err() {
            self.message = Some(format!(
                "Saved {label} to bible/sources — enable AI to build the story bible from it."
            ));
            return;
        }
        self.import_source = Some(label);
        self.spawn_generative(
            &combined,
            "⟳ Importing notes…",
            |backend, notes, model| {
                let req = ai::import::build_request(notes.to_string(), &model);
                Outcome::Import(
                    backend
                        .complete(&req)
                        .and_then(|r| ai::import::parse(&r.text)),
                )
            },
        );
    }

    /// Apply an import pass: stage the generated character and setting files (and
    /// surface any hand-edit conflicts together in one review), exactly like the
    /// character/setting passes.
    pub(crate) fn apply_import(&mut self, result: Result<ai::import::Imported, ai::AiError>) {
        self.overlay = None;
        let source = self.import_source.take().unwrap_or_else(|| "notes".into());
        let Some(imported) = self.ok_or_report(result) else {
            return;
        };

        let mut chars = imported.characters;
        for c in &mut chars {
            self.enrich_mentions(c);
        }
        let setting = imported.setting;
        let timeline = imported.timeline;
        let outline = imported.outline;
        let setting_has = !setting.categories.is_empty() || !setting.gazetteer.is_empty();
        let timeline_has = !timeline.events.is_empty();
        let outline_has = !outline.outline.is_empty();
        if chars.is_empty() && !setting_has && !timeline_has && !outline_has {
            self.message = Some(format!(
                "Imported {source}: no bible content found in the notes."
            ));
            return;
        }

        let today = date::iso(date::today());
        let bible = self.project.root.join("bible");
        // Stage every produced file; collect all hand-edit conflicts into one
        // review so a single import never opens several in a row.
        let mut staged: Vec<Staged> = Vec::new();
        for c in &chars {
            let path = bible
                .join("characters")
                .join(format!("{}.md", bible::slug(&c.name)));
            staged.push(self.stage_bible(path, &c.name, |notes| {
                character::render_profile(c, notes, &today)
            }));
        }
        if !chars.is_empty() {
            // Refresh the cross-cast relationship index (#707) from the imported
            // profiles — the same aggregation the character pass runs.
            self.write_relationship_index(&chars, &today);
        }
        if setting_has {
            staged.push(
                self.stage_bible(bible.join("setting.md"), "setting", |notes| {
                    setting::render(&setting, notes, &today)
                }),
            );
        }
        if timeline_has {
            staged.push(
                self.stage_bible(bible.join("timeline.md"), "timeline", |notes| {
                    continuity::render_timeline(&timeline, notes, &today)
                }),
            );
        }
        if outline_has {
            staged.push(
                self.stage_bible(bible.join("outline.md"), "outline", |notes| {
                    scaffold::render_outline(&outline, notes, &today)
                }),
            );
        }

        let tally = StagedTally::from(staged.into_iter());

        let count = chars.len();
        let summary_source = source.clone();
        self.resolve_staged(
            tally,
            |written, pending| {
                format!("Imported {source}: wrote {written} · {pending} need review")
            },
            |app, written| {
                app.show_ai(import_summary_lines(
                    &summary_source,
                    count,
                    setting_has,
                    timeline_has,
                    outline_has,
                    written,
                ));
            },
        );
    }
}

/// Read a manuscript into the shared parsed form, dispatching on extension
/// (#401/#402/#593): a `.docx` from its bytes, a `.scriv` bundle from its
/// directory, a plain folder as one chapter per `.md` file, a `.txt` as plain
/// text, anything else (a `.md`) as Markdown. The source is read only — never
/// altered.
fn read_manuscript(path: &std::path::Path) -> std::io::Result<crate::manuscript_import::Imported> {
    use crate::manuscript_import as import;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("docx") {
        import::parse_docx(&std::fs::read(path)?)
    } else if ext.eq_ignore_ascii_case("scriv") {
        import::parse_scriv(path)
    } else if path.is_dir() {
        import::parse_markdown_folder(path)
    } else if ext.eq_ignore_ascii_case("txt") {
        Ok(import::parse_text(&std::fs::read_to_string(path)?))
    } else {
        Ok(import::parse_markdown(&std::fs::read_to_string(path)?))
    }
}

/// A parenthetical warning suffix for the import success message when the source
/// listed content the scanner couldn't fully carry (#511/#646): unreadable bodies
/// (`.scriv`), plus footnotes/endnotes converted to inline markers and
/// images/tables/comments dropped (`.docx`/`.scriv`). Empty when the import was
/// clean, so a clean document reports no warning.
fn import_warning_suffix(
    unreadable_bodies: usize,
    dropped: crate::manuscript_import::Dropped,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if unreadable_bodies > 0 {
        parts.push(format!("{unreadable_bodies} bodies unreadable"));
    }
    let notes = dropped.notes();
    if notes > 0 {
        parts.push(format!(
            "{notes} note{} kept as markers",
            if notes == 1 { "" } else { "s" }
        ));
    }
    let mut lost: Vec<String> = Vec::new();
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    if dropped.images > 0 {
        lost.push(plural(dropped.images, "image", "images"));
    }
    if dropped.tables > 0 {
        lost.push(plural(dropped.tables, "table", "tables"));
    }
    if dropped.comments > 0 {
        lost.push(plural(dropped.comments, "comment", "comments"));
    }
    if !lost.is_empty() {
        parts.push(format!("{} dropped", lost.join(", ")));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join("; "))
    }
}

/// The Markdown files to import from `path` (#283): the file itself if it's a
/// `.md` file, or every `.md` directly inside it if it's a folder (sorted), so a
/// folder imports its notes collectively. Empty when the path is neither.
fn collect_import_files(path: &std::path::Path) -> Vec<std::path::PathBuf> {
    let is_md = |p: &std::path::Path| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
    };
    if path.is_dir() {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_md(p))
            .collect();
        files.sort();
        files
    } else if path.is_file() && is_md(path) {
        vec![path.to_path_buf()]
    } else {
        Vec::new()
    }
}

/// Summary lines for a completed notes import (#283/#288).
fn import_summary_lines(
    source: &str,
    characters: usize,
    setting: bool,
    timeline: bool,
    outline: bool,
    written: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::raw(format!("Imported {source} into the story bible.")),
        Line::raw(""),
        Line::raw(format!("Characters: {characters}")),
    ];
    let yes = |on: bool| if on { "updated" } else { "—" };
    lines.push(Line::raw(format!("Setting:    {}", yes(setting))));
    lines.push(Line::raw(format!("Timeline:   {}", yes(timeline))));
    lines.push(Line::raw(format!("Outline:    {}", yes(outline))));
    lines.push(Line::raw(format!("Files written: {written}  (bible/)")));
    lines.push(Line::raw(""));
    lines.push(Line::raw(
        "The original notes are preserved under bible/sources/.",
    ));
    lines
}

#[cfg(test)]
mod tests {
    use super::import_warning_suffix;
    use crate::manuscript_import::Dropped;

    #[test]
    fn clean_import_has_no_warning_suffix() {
        assert_eq!(import_warning_suffix(0, Dropped::default()), "");
    }

    #[test]
    fn unreadable_bodies_alone_are_reported() {
        assert_eq!(
            import_warning_suffix(2, Dropped::default()),
            " (2 bodies unreadable)"
        );
    }

    #[test]
    fn dropped_elements_are_summarized_with_notes_kept() {
        let dropped = Dropped {
            footnotes: 3,
            endnotes: 0,
            images: 2,
            tables: 1,
            comments: 0,
        };
        assert_eq!(
            import_warning_suffix(0, dropped),
            " (3 notes kept as markers; 2 images, 1 table dropped)"
        );
    }

    #[test]
    fn singular_element_labels_are_not_pluralized() {
        let dropped = Dropped {
            footnotes: 1,
            images: 1,
            tables: 0,
            comments: 1,
            endnotes: 0,
        };
        assert_eq!(
            import_warning_suffix(0, dropped),
            " (1 note kept as markers; 1 image, 1 comment dropped)"
        );
    }

    #[test]
    fn unreadable_and_dropped_combine_in_one_suffix() {
        let dropped = Dropped {
            tables: 1,
            ..Dropped::default()
        };
        assert_eq!(
            import_warning_suffix(1, dropped),
            " (1 bodies unreadable; 1 table dropped)"
        );
    }
}
