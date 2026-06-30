//! Manuscript export orchestration (#7): assemble the live project into the
//! shared [`crate::export::Document`] and write it out. The structure assembly is
//! here; format-specific rendering lives in `crate::export`. Split out of
//! `app/mod.rs`; `App` methods via `use super::*`.

use super::*;

use crate::export::{self, Chapter, Document, ExportStyle, Part};
use crate::frontmatter::{Band, Epigraph};
use crate::mode::{Action, Command, ExportFormat, ExportScope, Palette, PaletteItem};
use crate::project::{front_chapter_count, part_at_body_position};

/// The export formats with their menu label and description.
const FORMATS: [(ExportFormat, &str, &str); 7] = [
    (
        ExportFormat::Markdown,
        "Manuscript (Markdown)",
        "One compiled .md file — hand off or convert",
    ),
    (
        ExportFormat::Docx,
        "Word (.docx)",
        "Standard manuscript format",
    ),
    (
        ExportFormat::Scriv,
        "Scrivener (.scriv)",
        "A project bundle — binder, parts, chapters",
    ),
    (
        ExportFormat::Vellum,
        "Vellum (.docx)",
        "Word tuned for Vellum — chapters split on Heading 1",
    ),
    (
        ExportFormat::Epub,
        "EPUB (.epub)",
        "A finished ebook — Apple Books, Kobo, Kindle",
    ),
    (
        ExportFormat::Pdf,
        "PDF (.pdf)",
        "A paginated, ready-to-read or print proof",
    ),
    (
        ExportFormat::PdfPrint,
        "PDF (print interior)",
        "Print-on-demand book block — trim, gutter, embedded fonts",
    ),
];

impl App {
    /// Open the export menu (#7): pick a format, then [`run_export`] writes it.
    /// A multi-book series (#332) offers each format twice — the whole series and
    /// just the active book; a standalone novel lists the formats once.
    pub(crate) fn open_export_palette(&mut self) {
        let item = |label: String, desc: &str, format, scope| {
            PaletteItem::new(label, desc, Command::Export(format, scope))
        };
        let mut items = Vec::new();
        items.push(PaletteItem::new(
            "Set export style…",
            "Pick a style preset, the common knobs, and the print trim size (#569)",
            Command::Run(Action::SetExportStyle),
        ));
        items.push(PaletteItem::new(
            "Preview style…",
            "Open a styled sample of your [style] settings in your EPUB reader",
            Command::PreviewStyle,
        ));
        if self.project.book_count() > 1 {
            for (f, label, desc) in FORMATS {
                items.push(item(
                    format!("Series → {label}"),
                    desc,
                    f,
                    ExportScope::Series,
                ));
            }
            for (f, label, desc) in FORMATS {
                items.push(item(
                    format!("This book → {label}"),
                    desc,
                    f,
                    ExportScope::ActiveBook,
                ));
            }
        } else {
            for (f, label, desc) in FORMATS {
                items.push(item(label.to_string(), desc, f, ExportScope::ActiveBook));
            }
        }
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Export",
            items,
            selected: 0,
            collapsed: Default::default(),
        }));
    }

    /// Compile the manuscript and write it under `export/<slug>.<ext>` in the
    /// chosen format (#7), overwriting any previous one so it always reflects the
    /// current project. `scope` picks the active book (#7) or the whole series
    /// (#332); a standalone novel only ever has the one book.
    pub(crate) fn run_export(&mut self, format: ExportFormat, scope: ExportScope) {
        // The continuous view's edits live in the windowed buffer, not the
        // per-chapter ones export reads — compile from the single-chapter view so
        // nothing is exported stale (same constraint as undo / structure edits).
        if self.manuscript.is_some() {
            self.message = Some("Leave the continuous view (Ctrl-l) before exporting.".into());
            return;
        }
        match scope {
            ExportScope::ActiveBook => {
                let doc = self.export_document();
                let name = bible::slug(&self.project.manifest.title);
                self.write_book(format, &doc, &name);
            }
            ExportScope::Series => {
                let docs = self.export_series_documents();
                let series = self.project.series_title().map(str::to_string);
                let name = bible::slug(series.as_deref().unwrap_or("series"));
                self.write_series(format, &docs, series.as_deref(), &name);
            }
        }
    }

    /// Write one book's [`Document`] in `format` to `export/<name>.<ext>`.
    fn write_book(&mut self, format: ExportFormat, doc: &Document, name: &str) {
        match format {
            ExportFormat::Markdown => {
                self.write_export(name, "md", export::to_markdown(doc).as_bytes())
            }
            ExportFormat::Docx => self.write_built(
                export::docx::to_docx(doc, export::docx::Profile::Manuscript),
                name,
                "docx",
            ),
            ExportFormat::Vellum => self.write_built(
                export::docx::to_docx(doc, export::docx::Profile::Vellum),
                name,
                "vellum.docx",
            ),
            ExportFormat::Epub => {
                let (style, warnings) = self.export_style();
                self.write_built(
                    export::epub::to_epub(doc, &crate::date::utc_timestamp(), &style),
                    name,
                    "epub",
                );
                self.note_export_warnings(warnings);
            }
            ExportFormat::Pdf => {
                let (style, mut warnings) = self.export_style();
                warnings.extend(export::pdf::pdf_style_warnings(&style));
                self.write_built(export::pdf::to_pdf(doc, &style), name, "pdf");
                self.note_export_warnings(warnings);
            }
            ExportFormat::PdfPrint => {
                let (style, mut warnings) = self.export_style();
                let ((w, h), trim_warning) = self.print_trim();
                warnings.extend(trim_warning);
                warnings.extend(export::pdf::pdf_style_warnings(&style));
                self.write_built(
                    export::pdf::to_pdf_print(doc, w, h, &style),
                    name,
                    "print.pdf",
                );
                self.note_export_warnings(warnings);
            }
            ExportFormat::Scriv => {
                let path = self.export_path(name, "scriv");
                let result = std::fs::create_dir_all(path.parent().unwrap_or(&path))
                    .and_then(|()| export::scriv::write_scriv(doc, &path));
                self.report_export(&path, result);
            }
        }
    }

    /// The print trim (in points) from the manifest `[print]` table, with a warning
    /// when the configured size is unknown (the default is used instead).
    fn print_trim(&self) -> ((f32, f32), Option<String>) {
        let name = self
            .project
            .manifest
            .print
            .as_ref()
            .and_then(|p| p.trim.as_deref())
            .unwrap_or(export::pdf::DEFAULT_TRIM);
        match export::pdf::trim_points(name) {
            Some(dims) => (dims, None),
            None => (
                export::pdf::trim_points(export::pdf::DEFAULT_TRIM).expect("default trim is valid"),
                Some(format!(
                    "print: unknown trim '{name}' (use 6x9/5x8/5.5x8.5) — using {}",
                    export::pdf::DEFAULT_TRIM
                )),
            ),
        }
    }

    /// The designed-export styling for this project (#468): the manifest's
    /// `[style]`, or the default (which reproduces today's plain output), with its
    /// preset applied and font/image paths resolved (#469/#472). Shared by the EPUB
    /// (#468) and PDF (#489) writers. Returns any resolution warnings to surface
    /// alongside the export result.
    fn export_style(&self) -> (ExportStyle, Vec<String>) {
        let mut style = self.project.manifest.style.clone().unwrap_or_default();
        let root = self.project.root.clone();
        // Resolve the preset first (its values are the base for the rest).
        let mut warnings: Vec<String> = style.apply_preset().into_iter().collect();
        warnings.extend(style.resolve_fonts(|path| std::fs::read(root.join(path))));
        warnings.extend(style.resolve_images(|path| std::fs::read(root.join(path))));
        warnings.extend(style.style_warnings());
        (style, warnings)
    }

    /// Append styling warnings (e.g. an unreadable font) to the export message, so
    /// a misconfigured `[style]` doesn't fail silently.
    fn note_export_warnings(&mut self, warnings: Vec<String>) {
        if warnings.is_empty() {
            return;
        }
        let note = warnings.join("; ");
        self.message = Some(match self.message.take() {
            Some(m) => format!("{m} — {note}"),
            None => note,
        });
    }

    /// Preview the designed-export styling (#483): render a fixed sample book
    /// through the *same* style-resolution and EPUB writer a real export uses, write
    /// it to a temp file, and open it in the system EPUB reader. Lets an author see
    /// a preset + overrides — scene break, ornaments, drop cap, heading alignment,
    /// embedded fonts/images — before committing to a full export. Touches only a
    /// temp file; the project is never modified.
    pub(crate) fn preview_style(&mut self) {
        let (bytes, warnings) = match self.style_preview_epub() {
            Ok(built) => built,
            Err(e) => {
                self.message = Some(format!("Style preview failed: {e}"));
                return;
            }
        };
        let path = std::env::temp_dir().join("quill-style-preview.epub");
        if let Err(e) = std::fs::write(&path, &bytes) {
            self.message = Some(format!("Style preview failed: {e}"));
            return;
        }
        match open_in_default_app(&path) {
            Ok(()) => self.message = Some(format!("Style preview opened ({})", path.display())),
            // The EPUB is written; only the launch failed — point the author at it.
            Err(e) => {
                self.message = Some(format!(
                    "Wrote {} but couldn't open it: {e}",
                    path.display()
                ))
            }
        }
        self.note_export_warnings(warnings);
    }

    /// Render the fixed style-preview sample (#483) through the resolved `[style]`
    /// and the EPUB writer, returning the bytes and any styling warnings — the pure,
    /// side-effect-free core of [`preview_style`] (no temp file, no reader launch).
    pub(crate) fn style_preview_epub(&self) -> std::io::Result<(Vec<u8>, Vec<String>)> {
        let (style, warnings) = self.export_style();
        let doc = style_preview_document();
        let bytes = export::epub::to_epub(&doc, &crate::date::utc_timestamp(), &style)?;
        Ok((bytes, warnings))
    }

    /// Write the whole series (one book per [`Document`], in order) in `format` to
    /// `export/<name>.<ext>`, books delimited as each format allows (#332).
    fn write_series(
        &mut self,
        format: ExportFormat,
        docs: &[Document],
        series: Option<&str>,
        name: &str,
    ) {
        let author = self.project.manifest.author.clone();
        match format {
            ExportFormat::Markdown => self.write_export(
                name,
                "md",
                export::to_markdown_series(docs, series, author.as_deref()).as_bytes(),
            ),
            ExportFormat::Docx => self.write_built(
                export::docx::to_docx_series(
                    docs,
                    series,
                    author.as_deref(),
                    export::docx::Profile::Manuscript,
                ),
                name,
                "docx",
            ),
            ExportFormat::Vellum => self.write_built(
                export::docx::to_docx_series(
                    docs,
                    series,
                    author.as_deref(),
                    export::docx::Profile::Vellum,
                ),
                name,
                "vellum.docx",
            ),
            ExportFormat::Epub => {
                let (style, warnings) = self.export_style();
                self.write_built(
                    export::epub::to_epub_series(
                        docs,
                        series,
                        author.as_deref(),
                        &crate::date::utc_timestamp(),
                        &style,
                    ),
                    name,
                    "epub",
                );
                self.note_export_warnings(warnings);
            }
            ExportFormat::Pdf => {
                let (style, mut warnings) = self.export_style();
                warnings.extend(export::pdf::pdf_style_warnings(&style));
                self.write_built(
                    export::pdf::to_pdf_series(docs, series, author.as_deref(), &style),
                    name,
                    "pdf",
                );
                self.note_export_warnings(warnings);
            }
            ExportFormat::PdfPrint => {
                let (style, mut warnings) = self.export_style();
                let ((w, h), trim_warning) = self.print_trim();
                warnings.extend(trim_warning);
                warnings.extend(export::pdf::pdf_style_warnings(&style));
                self.write_built(
                    export::pdf::to_pdf_series_print(docs, series, author.as_deref(), w, h, &style),
                    name,
                    "print.pdf",
                );
                self.note_export_warnings(warnings);
            }
            ExportFormat::Scriv => {
                let path = self.export_path(name, "scriv");
                let result = std::fs::create_dir_all(path.parent().unwrap_or(&path))
                    .and_then(|()| export::scriv::write_scriv_series(docs, &path));
                self.report_export(&path, result);
            }
        }
    }

    /// Write a compiled binary export's bytes (`.docx` / `.epub`), or report the
    /// build error.
    fn write_built(&mut self, built: std::io::Result<Vec<u8>>, name: &str, ext: &str) {
        match built {
            Ok(bytes) => self.write_export(name, ext, &bytes),
            Err(e) => self.message = Some(format!("Export failed: {e}")),
        }
    }

    /// `export/<name>.<ext>` under the project root.
    fn export_path(&self, name: &str, ext: &str) -> std::path::PathBuf {
        self.project
            .root
            .join("export")
            .join(format!("{name}.{ext}"))
    }

    /// Write the export bytes to `export/<name>.<ext>` and report the path.
    fn write_export(&mut self, name: &str, ext: &str, bytes: &[u8]) {
        let path = self.export_path(name, ext);
        let result = match path.parent() {
            Some(parent) => {
                std::fs::create_dir_all(parent).and_then(|()| std::fs::write(&path, bytes))
            }
            None => std::fs::write(&path, bytes),
        };
        self.report_export(&path, result);
    }

    /// Set the status message from an export's outcome and target path.
    fn report_export(&mut self, path: &std::path::Path, result: std::io::Result<()>) {
        let shown = path
            .strip_prefix(&self.project.root)
            .unwrap_or(path)
            .display();
        self.message = Some(match result {
            Ok(()) => format!("Exported to {shown}"),
            Err(e) => format!("Export failed: {e}"),
        });
    }

    /// Assemble the project into the export tree (#190): book metadata, then the
    /// front matter, the body grouped into parts, and the back matter — each
    /// chapter with its epigraph and prose (active chapter live from its buffer,
    /// the rest from disk; front matter stripped; a repeated title heading dropped).
    fn export_document(&self) -> Document {
        let m = &self.project.manifest;
        let parts = self.part_ranges(); // (name, body-chapter index range), in order
        let mut front = Vec::new();
        let mut back = Vec::new();
        let mut body_parts: Vec<Part> = Vec::new();
        // Group consecutive body chapters by the part they're in, in reading order
        // — body chapters in no defined part form an unnamed (heading-less) group.
        // A fresh group opens whenever the part changes (`prev` starts unset so the
        // first body chapter always opens one).
        let mut prev: Option<Option<usize>> = None;
        for i in 0..self.chapter_count() {
            let chapter = self.export_chapter(i);
            match self.project.chapter_band(i) {
                Band::Front => front.push(chapter),
                Band::Back => back.push(chapter),
                Band::Body => {
                    let part = parts.iter().position(|(_, range)| range.contains(&i));
                    if prev != Some(part) {
                        prev = Some(part);
                        body_parts.push(Part {
                            name: part.map(|p| parts[p].0.clone()),
                            chapters: Vec::new(),
                        });
                    }
                    // The first body chapter pushes a part (prev starts `None`), so
                    // `body_parts` is non-empty by the time we reach here.
                    body_parts
                        .last_mut()
                        .expect("a body part was pushed above")
                        .chapters
                        .push(chapter);
                }
            }
        }

        Document {
            title: m.title.clone(),
            subtitle: m.subtitle.clone(),
            author: m.author.clone(),
            series: m.series.clone(),
            epigraph: m.epigraph(),
            front,
            parts: body_parts,
            back,
        }
    }

    /// Build one chapter for export: its title, epigraph, and prose (front matter
    /// stripped, a repeated title heading dropped).
    fn export_chapter(&self, i: usize) -> Chapter {
        let title = self
            .project
            .chapter_title(i)
            .unwrap_or("Untitled")
            .to_string();
        let body = export::strip_repeated_title(&self.chapter_text(i), &title);
        Chapter {
            title,
            epigraph: self.chapter_epigraph(i),
            body,
        }
    }

    /// One [`Document`] per book of the series, in reading order (#332): the
    /// active book compiled live (its active chapter from the buffer), the others
    /// read from disk.
    fn export_series_documents(&self) -> Vec<Document> {
        (0..self.project.book_count())
            .map(|b| {
                let mut doc = if b == self.project.active_book() {
                    self.export_document()
                } else {
                    self.export_book_document(b)
                };
                // The series title and author are rendered once by the series
                // writer, not repeated above every book.
                doc.series = None;
                doc.author = None;
                doc
            })
            .collect()
    }

    /// Compile a non-active book into a [`Document`] (#332), reading its chapters'
    /// prose and front matter from disk and grouping them into front / parts /
    /// back the same way [`export_document`] does for the active book.
    fn export_book_document(&self, b: usize) -> Document {
        let chapters = self.project.book_chapters(b);
        let parts = self.project.book_parts(b);
        // Band order (front → body → back), then group body chapters by part.
        let mut order: Vec<usize> = (0..chapters.len()).collect();
        order.sort_by_key(|&i| Band::of(chapters[i].role));
        let front_count = front_chapter_count(chapters);
        let (mut front, mut back, mut body_parts) = (Vec::new(), Vec::new(), Vec::<Part>::new());
        let mut prev: Option<Option<usize>> = None;
        for (pos, &ci) in order.iter().enumerate() {
            let ce = &chapters[ci];
            let (body, fm) = self.project.read_chapter_parts(&ce.file);
            let chapter = Chapter {
                title: ce.title.clone(),
                epigraph: fm.and_then(|f| f.epigraph()),
                body: export::strip_repeated_title(&body, &ce.title),
            };
            match Band::of(ce.role) {
                Band::Front => front.push(chapter),
                Band::Back => back.push(chapter),
                Band::Body => {
                    let part = parts.and_then(|ps| {
                        part_at_body_position(ps, pos.saturating_sub(front_count) + 1)
                    });
                    if prev != Some(part) {
                        prev = Some(part);
                        let name = part.map(|pi| self.project.book_part_display_name(b, pi));
                        body_parts.push(Part {
                            name,
                            chapters: Vec::new(),
                        });
                    }
                    // A new part was just pushed on the first body chapter (prev starts
                    // `None`), so `body_parts` is non-empty here.
                    body_parts
                        .last_mut()
                        .expect("a body part was pushed above")
                        .chapters
                        .push(chapter);
                }
            }
        }
        Document {
            title: self.project.book_title(b).unwrap_or("Untitled").to_string(),
            subtitle: self.project.book_subtitle(b).map(str::to_string),
            author: self.project.manifest.author.clone(),
            // The series title is carried once by the series writer, not per book.
            series: None,
            epigraph: self.project.book_epigraph(b),
            front,
            parts: body_parts,
            back,
        }
    }
}

/// A fixed sample book for the style preview (#483): generic title-page metadata, a
/// book epigraph, and one named part with a chapter whose opening paragraph, scene
/// break, and a following paragraph exercise every styled element (drop cap, scene
/// break, heading alignment, title/part ornaments). Built from a constant snippet —
/// it never reads the author's chapters.
fn style_preview_document() -> Document {
    Document {
        title: "The Sample Novel".into(),
        subtitle: Some("A Style Preview".into()),
        author: Some("Your Name".into()),
        series: Some("The Sample Series".into()),
        epigraph: Some(Epigraph {
            text: "A book opens on an epigraph.".into(),
            attribution: Some("Anon.".into()),
        }),
        front: Vec::new(),
        parts: vec![Part {
            name: Some("Part One".into()),
            chapters: vec![Chapter {
                title: "Chapter One".into(),
                epigraph: None,
                body: "This opening paragraph shows how a drop cap and the body text \
                       look in your chosen style, running long enough to wrap across a \
                       line or two so the indentation reads true.\n\n\
                       A second paragraph follows, set in the body font.\n\n\
                       ---\n\n\
                       After the scene break the story resumes, so the divider — a glyph \
                       or an image — appears in place between the two scenes."
                    .into(),
            }],
        }],
        back: Vec::new(),
    }
}

/// Open `path` in the operating system's default application (#483) — the EPUB
/// reader, for the style preview. Spawns the platform launcher and returns without
/// waiting, so the editor keeps running.
fn open_in_default_app(path: &std::path::Path) -> std::io::Result<()> {
    use std::process::Command as Proc;
    #[cfg(target_os = "macos")]
    let mut cmd = Proc::new("open");
    #[cfg(target_os = "linux")]
    let mut cmd = Proc::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Proc::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    cmd.arg(path).spawn().map(|_| ())
}
