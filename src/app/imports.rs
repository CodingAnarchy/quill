//! Notes / manuscript import flows (#283/#286/#298): importing Markdown notes
//! into the story bible, starting a new project from a folder of notes, and
//! importing an existing Markdown manuscript as a chaptered project. Split out of
//! `assist.rs` to keep that module under the ~1,000-line convention (#426); a
//! child module, so it keeps access to `App`'s private fields.

use super::assist::write_failure_message;
use super::bible_review::Staged;
use super::*;
use crate::ai::scaffold;

impl App {
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
        self.pending_notes_folder = Some(std::path::PathBuf::from(folder));
        let prefill = format!("{}/", self.new_project_base().display());
        self.open_prompt(
            Pending::NewFromNotesDest,
            "New project — path:",
            &prefill,
            None,
        );
    }

    /// Create the new project at `dest` and import the folder chosen earlier.
    pub(crate) fn finish_new_from_notes(&mut self, dest: &str) {
        let Some(folder) = self.pending_notes_folder.take() else {
            self.mode = self.editing_mode();
            return;
        };
        if self.create_and_open(std::path::Path::new(dest.trim())) {
            // The new project is now active; import the notes into its bible.
            self.import_notes(&folder.to_string_lossy());
        }
    }

    // --- New project from an existing manuscript (#298) -------------------

    /// Prompt for a manuscript file (Markdown or Word) to import as a new project.
    pub(crate) fn start_import_manuscript(&mut self) {
        self.open_prompt(
            Pending::ImportManuscriptSource,
            "Import manuscript — Markdown (.md) or Word (.docx) file:",
            "",
            None,
        );
    }

    /// Validate the chosen file is an importable manuscript (`.md` or `.docx`),
    /// then ask where to create the new project. Shared by the file prompt and the
    /// run-action entry point.
    pub(crate) fn begin_import_manuscript(&mut self, file: &str) {
        let path = std::path::Path::new(file.trim());
        let importable = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("docx"));
        if !(path.is_file() && importable) {
            self.message = Some(format!(
                "Not a Markdown (.md) or Word (.docx) manuscript: {}",
                path.display()
            ));
            self.mode = self.editing_mode();
            return;
        }
        self.pending_manuscript_source = Some(path.to_path_buf());
        let prefill = format!("{}/", self.new_project_base().display());
        self.open_prompt(
            Pending::ImportManuscriptDest,
            "New project — path:",
            &prefill,
            None,
        );
    }

    /// Create the new project at `dest` and import the manuscript chosen earlier:
    /// split it into per-chapter files in reading order. The source is read only —
    /// never moved or altered.
    pub(crate) fn finish_import_manuscript(&mut self, dest: &str) {
        let Some(source) = self.pending_manuscript_source.take() else {
            self.mode = self.editing_mode();
            return;
        };
        let dest = std::path::PathBuf::from(dest.trim());
        if Project::manifest_path(&dest).exists() {
            self.message = Some(format!("A project already exists at {}", dest.display()));
            self.mode = self.editing_mode();
            return;
        }
        let parsed = match read_manuscript(&source) {
            Ok(p) => p,
            Err(e) => {
                self.message = Some(format!("Couldn't read {}: {e}", source.display()));
                self.mode = self.editing_mode();
                return;
            }
        };
        let chapters: Vec<(String, String)> = parsed
            .chapters
            .into_iter()
            .map(|c| (c.title, c.body))
            .collect();
        let built = Project::create(&dest).and_then(|mut p| {
            p.import_chapters(parsed.title.as_deref(), parsed.author.as_deref(), &chapters)
                .map(|()| chapters.len())
        });
        match built {
            Ok(n) => {
                self.switch_to(dest); // open the finished project fresh from disk
                self.message = Some(format!("Imported {n} chapters from {}", source.display()));
            }
            Err(e) => {
                self.message = Some(format!("Import failed: {e}"));
                self.mode = self.editing_mode();
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
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "notes.md".into());
            if let Err(msg) = self.write_file(&dest_dir.join(&name), &text) {
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
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "notes".into()),
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

        let mut conflicts = Vec::new();
        let mut failures = Vec::new();
        let mut written = 0;
        for result in staged {
            match result {
                Staged::Wrote => written += 1,
                Staged::Conflict(x) => conflicts.push(x),
                Staged::Failed(m) => failures.push(m),
            }
        }

        if !failures.is_empty() {
            self.message = Some(write_failure_message(&failures));
        } else if conflicts.is_empty() {
            self.show_ai(import_summary_lines(
                &source,
                chars.len(),
                setting_has,
                timeline_has,
                outline_has,
                written,
            ));
        } else {
            let pending = conflicts.len();
            self.mode = Mode::BibleReview(Box::new(BibleReview {
                conflicts,
                editing: None,
            }));
            self.message = Some(format!(
                "Imported {source}: wrote {written} · {pending} need review"
            ));
        }
    }
}

/// Read a manuscript file into the shared parsed form, dispatching on extension
/// (#401): a `.docx` is parsed from its bytes, anything else (a `.md`) as text.
/// The source is read only — never altered.
fn read_manuscript(path: &std::path::Path) -> std::io::Result<crate::manuscript_import::Imported> {
    let is_docx = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("docx"));
    if is_docx {
        crate::manuscript_import::parse_docx(&std::fs::read(path)?)
    } else {
        Ok(crate::manuscript_import::parse_markdown(
            &std::fs::read_to_string(path)?,
        ))
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
