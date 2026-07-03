//! Manuscript export (#7): open the export palette, compile the live project
//! (active book or whole series #332) into the shared [`crate::export::Document`],
//! and write it out. Format-specific rendering lives in `crate::export`; here is
//! the palette wiring, the structure assembly, and the file-writing plumbing.

use super::super::*;

use crate::ai::bible;
use crate::export::{self, Chapter, Document, Part};
use crate::frontmatter::Band;
use crate::mode::{Action, Command, ExportFormat, ExportScope, Palette, PaletteItem};
use crate::project::{front_chapter_count, part_at_body_position};

/// The collapsible group headers the sample-export entries (#584) sit under in the
/// export palette — static so they can key the palette's collapsed set.
const SAMPLE_CHAPTERS: &str = "Sample · one chapter";
const SAMPLE_PARTS: &str = "Sample · one part";
const SAMPLE_RANGE: &str = "Sample · a range";

/// The set of sample-export group headers to collapse by default.
type SampleGroups = std::collections::HashSet<&'static str>;

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
        // Sample exports (#584): a single chapter, a part, or a contiguous range of
        // the active book, in any format — for a beta reader or a submission. Each
        // sits under a collapsed group so the everyday whole-book formats stay up
        // top; the labels carry the chapter/part name, the (static) headers don't.
        let collapsed = self.append_sample_export_items(&mut items);
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Export",
            items,
            selected: 0,
            collapsed,
        }));
    }

    /// Append the per-chapter / per-part / range sample-export entries (#584) to the
    /// export palette and return the set of group headers to collapse by default.
    /// Only the active book's body chapters are offered (front/back matter and the
    /// other books of a series are never sampled); a range entry needs at least
    /// three body chapters to be a meaningful "first chapters" sample.
    fn append_sample_export_items(&self, items: &mut Vec<PaletteItem>) -> SampleGroups {
        let item = |label: String, desc: String, format, scope| {
            PaletteItem::new(label, desc, Command::Export(format, scope))
        };
        let body: Vec<usize> = (0..self.chapter_count())
            .filter(|&i| self.project.chapter_band(i) == Band::Body)
            .collect();
        let mut collapsed = SampleGroups::new();
        // One chapter.
        for &i in &body {
            let title = self
                .project
                .chapter_title(i)
                .unwrap_or("Untitled")
                .to_string();
            for (f, label, _) in FORMATS {
                items.push(
                    item(
                        format!("Chapter: {title} → {label}"),
                        format!("Just this chapter as {label}"),
                        f,
                        ExportScope::Chapter(i),
                    )
                    .in_group(SAMPLE_CHAPTERS),
                );
            }
            collapsed.insert(SAMPLE_CHAPTERS);
        }
        // One part (each part's chapter range).
        for (name, range) in self.part_ranges() {
            if range.is_empty() {
                continue;
            }
            for (f, label, _) in FORMATS {
                items.push(
                    item(
                        format!("Part: {name} → {label}"),
                        format!("This part as {label}"),
                        f,
                        ExportScope::Part {
                            name: name.clone(),
                            start: range.start,
                            end: range.end - 1,
                        },
                    )
                    .in_group(SAMPLE_PARTS),
                );
            }
            collapsed.insert(SAMPLE_PARTS);
        }
        // The first three chapters — the classic submission sample.
        if body.len() >= 3 {
            let (start, end) = (body[0], body[2]);
            for (f, label, _) in FORMATS {
                items.push(
                    item(
                        format!("First three chapters → {label}"),
                        format!("A submission sample as {label}"),
                        f,
                        ExportScope::Range { start, end },
                    )
                    .in_group(SAMPLE_RANGE),
                );
            }
            collapsed.insert(SAMPLE_RANGE);
        }
        collapsed
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
        match &scope {
            ExportScope::Series => {
                let docs = self.export_series_documents();
                let series = self.project.series_title().map(str::to_string);
                let name = bible::slug(series.as_deref().unwrap_or("series"));
                self.write_series(format, &docs, series.as_deref(), &name);
            }
            // The active book whole (#7) or a sample of it (#584) — both compile one
            // [`Document`] from the active book, differing only in which chapters
            // the loop keeps (`chapter_span`) and the output filename.
            _ => {
                let doc = self.export_document(scope.chapter_span());
                let name = self.export_name(&scope);
                self.write_book(format, &doc, &name);
            }
        }
    }

    /// The output base name (sans extension) for a scoped export. A whole-book
    /// export is named for the book; a *sample* (#584) appends a scope tag — the
    /// chapter or part slug, or `chapters-N-M` for a range — so a sample never
    /// overwrites the full export sitting beside it.
    fn export_name(&self, scope: &ExportScope) -> String {
        let book = bible::slug(&self.project.manifest.title);
        let tag = match scope {
            ExportScope::Chapter(i) => self
                .project
                .chapter_title(*i)
                .map(bible::slug)
                .unwrap_or_else(|| format!("chapter-{}", i + 1)),
            ExportScope::Part { name, .. } => bible::slug(name),
            // 1-based, inclusive — what an author counting chapters expects.
            ExportScope::Range { start, end } => format!("chapters-{}-{}", start + 1, end + 1),
            ExportScope::ActiveBook | ExportScope::Series => return book,
        };
        format!("{book}-{tag}")
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
                warnings.extend(export::pdf::pdf_dpi_warnings(
                    &style,
                    export::pdf::SCREEN_MEDIA_W,
                ));
                self.write_built(export::pdf::to_pdf(doc, &style), name, "pdf");
                self.note_export_warnings(warnings);
            }
            ExportFormat::PdfPrint => {
                let (style, mut warnings) = self.export_style();
                let ((w, h), trim_warning) = self.print_trim();
                warnings.extend(trim_warning);
                warnings.extend(export::pdf::pdf_style_warnings(&style));
                let bleed = self.print_bleed();
                warnings.extend(export::pdf::pdf_dpi_warnings(&style, w + 2.0 * bleed));
                self.write_built(
                    export::pdf::to_pdf_print(doc, w, h, bleed, &style),
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
                warnings.extend(export::pdf::pdf_dpi_warnings(
                    &style,
                    export::pdf::SCREEN_MEDIA_W,
                ));
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
                let bleed = self.print_bleed();
                warnings.extend(export::pdf::pdf_dpi_warnings(&style, w + 2.0 * bleed));
                self.write_built(
                    export::pdf::to_pdf_series_print(
                        docs,
                        series,
                        author.as_deref(),
                        w,
                        h,
                        bleed,
                        &style,
                    ),
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

    /// Write the export bytes to `export/<name>.<ext>` and report the path. Shared
    /// by the manuscript exports and the report snapshots (#601).
    pub(crate) fn write_export(&mut self, name: &str, ext: &str, bytes: &[u8]) {
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
    ///
    /// `span` is `None` for a whole-book export (every chapter); a *sample* (#584)
    /// passes an inclusive manifest chapter-index range, which keeps only the body
    /// chapters in it — front and back matter are dropped from a sample, and the
    /// kept body chapters still group under their part headings.
    fn export_document(&self, span: Option<std::ops::RangeInclusive<usize>>) -> Document {
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
            // A sample keeps only its chapters; the rest of the book is skipped.
            if span.as_ref().is_some_and(|s| !s.contains(&i)) {
                continue;
            }
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
            publishing: self.publishing_meta(),
            epigraph: m.epigraph(),
            front,
            parts: body_parts,
            back,
        }
    }

    /// The book-level publishing metadata (#583) read from the manifest, for the
    /// export tree — the OPF `dc:*`, the copyright page, and the PDF/DOCX. Shared
    /// by the active-book and per-book document builders so both carry it.
    fn publishing_meta(&self) -> export::PubMeta {
        let m = &self.project.manifest;
        export::PubMeta {
            isbn: m.isbn.clone(),
            publisher: m.publisher.clone(),
            date: m.pub_date.clone(),
            lang: m.language.clone(),
            rights: m.rights.clone(),
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
            role: self.project.chapter_role(i),
        }
    }

    /// One [`Document`] per book of the series, in reading order (#332): the
    /// active book compiled live (its active chapter from the buffer), the others
    /// read from disk.
    fn export_series_documents(&self) -> Vec<Document> {
        (0..self.project.book_count())
            .map(|b| {
                let mut doc = if b == self.project.active_book() {
                    self.export_document(None)
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
                role: ce.role,
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
            // Publishing metadata is series-level (one ISBN/publisher per project
            // manifest); each book in a series carries the same.
            publishing: self.publishing_meta(),
            epigraph: self.project.book_epigraph(b),
            front,
            parts: body_parts,
            back,
        }
    }
}
