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

/// The full-bleed allowance the print-on-demand publish bundles (#816) set —
/// 0.125", what KDP and IngramSpark want on anything that reaches the page edge.
const PUBLISH_BLEED_INCHES: f32 = 0.125;

/// The default language a publish bundle (#816) fills in when the manifest sets
/// none — a valid BCP-47 tag the preflight and the EPUB `dc:language` require.
const DEFAULT_LANGUAGE: &str = "en";

/// A print-on-demand publish target for the one-click "Prepare for …" bundle
/// (#816): set the target's sane print defaults, run the preflight, and emit the
/// print PDF + reflowable EPUB together. Both stores take the same 6×9 / 0.125"
/// bundle today; the enum keeps them distinct for the label and any future
/// per-store defaults.
#[derive(Clone, Copy, Debug)]
pub enum PublishTarget {
    Kdp,
    IngramSpark,
}

impl PublishTarget {
    /// The store's display name, for the status messages and the guard reasons.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Kdp => "KDP",
            Self::IngramSpark => "IngramSpark",
        }
    }
}

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
    #[allow(clippy::needless_pass_by_value)] // callers (palette.rs, tests) hand over an owned scope
    pub(crate) fn run_export(&mut self, format: ExportFormat, scope: ExportScope) {
        // The continuous view's edits live in the windowed buffer, not the
        // per-chapter ones export reads — compile from the single-chapter view so
        // nothing is exported stale (same constraint as undo / structure edits).
        if self.manuscript.is_some() {
            self.message = Some("Leave the continuous view (Ctrl-l) before exporting.".into());
            return;
        }
        if matches!(scope, ExportScope::Series) {
            let docs = self.export_series_documents();
            let series = self.project.series_title().map(str::to_string);
            let name = bible::slug(series.as_deref().unwrap_or("series"));
            self.write_series(format, &docs, series.as_deref(), &name);
        } else {
            // The active book whole (#7) or a sample of it (#584) — both compile one
            // [`Document`] from the active book, differing only in which chapters
            // the loop keeps (`chapter_span`) and the output filename.
            let doc = self.export_document(scope.chapter_span().as_ref());
            let name = self.export_name(&scope);
            self.write_book(format, &doc, &name);
        }
    }

    /// Export a review copy (#834): a `.docx` with numbered lines and generous
    /// margins for markup, carrying the author's inline annotations (#302) as Word
    /// comments anchored to their ranges — the collaboration/handoff artifact for a
    /// beta reader or editor. Every other export strips annotations; this one alone
    /// carries them. Compiles the active book (the whole novel, never a sample) from
    /// the single-chapter view, like the other exports, so nothing is read stale.
    pub(crate) fn run_review_copy_export(&mut self) {
        if self.manuscript.is_some() {
            self.message =
                Some("Leave the continuous view (Ctrl-l) before exporting a review copy.".into());
            return;
        }
        let doc = self.export_document(None);
        let notes = self.review_chapter_notes();
        let name = format!("{}-review", bible::slug(&self.project.manifest.title));
        self.write_built(
            export::docx::to_docx_review(&doc, &notes),
            &name,
            "review.docx",
        );
    }

    /// The inline annotations (#302) for each chapter, in the order the review
    /// writer visits chapters — front, then the body (which the [`Document`]'s
    /// blocks group into parts, in chapter-index order), then back — matching how
    /// [`export_document`](Self::export_document) assembles the tree. Each note
    /// carries the exact prose it was anchored to (extracted from the chapter body)
    /// so the writer can re-find it without threading byte offsets through the
    /// body's export transforms.
    fn review_chapter_notes(&self) -> Vec<export::docx::ChapterNotes> {
        // One pass per band so the order matches `Document::blocks` (front → body →
        // back). Within a band, chapters stay in manifest-index order.
        let mut front = Vec::new();
        let mut body = Vec::new();
        let mut back = Vec::new();
        for i in 0..self.chapter_count() {
            let notes = self.chapter_review_notes(i);
            match self.project.chapter_band(i) {
                Band::Front => front.push(notes),
                Band::Body => body.push(notes),
                Band::Back => back.push(notes),
            }
        }
        front.extend(body);
        front.extend(back);
        front
    }

    /// One chapter's annotations as [`ReviewNote`]s (#834): each stored range
    /// resolved against the chapter's body text to the exact snippet it anchors, so
    /// the review writer can locate and comment it. A range that no longer falls
    /// within the body (a stale side-file entry) is skipped.
    fn chapter_review_notes(&self, i: usize) -> export::docx::ChapterNotes {
        let text = self.chapter_text(i);
        let file = self.project.chapter_file(i).to_string();
        self.project
            .annotations_for(&file)
            .into_iter()
            .filter_map(|a| {
                let snippet = text.get(a.start..a.end)?;
                Some(export::docx::ReviewNote {
                    text: snippet.to_string(),
                    note: a.note,
                })
            })
            .collect()
    }

    /// Run the export preflight (#815) and show the checklist: consolidate the
    /// export path's own signals (cover DPI, trim, copyright presence, front/back
    /// bands, language) plus the store-facing metadata into one auditable report,
    /// open it in a report overlay, and save it to `export/preflight-<date>.md`
    /// through the shared [`write_export`](Self::write_export) path — the same one
    /// the report snapshots (#601) use. Changes no export behavior; it only reports.
    ///
    /// Checked against the print-interior profile — the strictest target (physical
    /// trim, bleed, and the print-DPI floor apply) — so a clean preflight clears the
    /// screen/ebook targets too. Runs from the single-chapter view only, like the
    /// exports, so the active chapter's live edits are read (not the stale window).
    pub(crate) fn run_preflight(&mut self) {
        if self.manuscript.is_some() {
            self.message =
                Some("Leave the continuous view (Ctrl-l) before the preflight check.".into());
            return;
        }
        let report = self.preflight_report();
        let title = self.project.manifest.title.clone();
        let md = report.to_markdown(&title);
        let lines = md.lines().map(|l| Line::raw(l.to_string())).collect();
        self.overlay = Some(Overlay::report("preflight", lines));
        // Persist alongside the exports so it's an auditable artifact, not just a
        // transient overlay — the same dated `export/<label>-<date>.md` path #601 uses.
        let name = format!("preflight-{}", date::iso(date::today()));
        self.write_export(&name, "md", md.as_bytes());
    }

    /// Build the export preflight [`Report`](export::preflight::Report) for the
    /// active book (#815), checked against the strictest (print-interior) profile.
    /// The pure core [`run_preflight`](Self::run_preflight) renders and saves, and
    /// the publish bundles (#816) gate on before emitting. Reads the active chapter
    /// live from the buffer, the rest from disk (same as the exports).
    fn preflight_report(&self) -> export::preflight::Report {
        let meta = self.publishing_meta();
        let (style, _warnings) = self.export_style();
        let ((w, _h), trim_warning) = self.print_trim();
        let bleed = self.print_bleed();
        let print = export::preflight::PrintInfo {
            media_w_pts: 2.0f32.mul_add(bleed, w),
            bleed_pts: bleed,
            trim_warning,
        };
        // Own each chapter's title and prose so the borrowed [`ChapterInfo`] slice
        // outlives the `preflight` call; the active chapter reads live from the buffer.
        let chapters: Vec<(String, usize, bool, String)> = (0..self.chapter_count())
            .map(|i| {
                (
                    self.project
                        .chapter_title(i)
                        .unwrap_or("Untitled")
                        .to_string(),
                    self.chapter_word_count(i),
                    self.project.chapter_band(i) == Band::Body,
                    self.chapter_text(i),
                )
            })
            .collect();
        let infos: Vec<export::preflight::ChapterInfo<'_>> = chapters
            .iter()
            .map(
                |(title, words, is_body, body)| export::preflight::ChapterInfo {
                    title,
                    words: *words,
                    is_body: *is_body,
                    body,
                },
            )
            .collect();
        export::preflight::preflight(&meta, &style, &infos, Some(&print), true)
    }

    /// One-click "Prepare for KDP / IngramSpark" (#816): set the target's sane
    /// print defaults (6×9 trim, 0.125" bleed, a default `en` language when none
    /// is set), run the preflight (#815), and — unless it fails — emit the
    /// print-interior PDF and the reflowable EPUB together to `export/`. The
    /// highest-leverage publish UX: the KDP/IngramSpark bundle in one step,
    /// composing the mature writers. Runs from the single-chapter view only (the
    /// continuous view holds an unsaved window), like the exports it drives.
    ///
    /// A preflight *failure* (something a store rejects outright — no ISBN, a
    /// non-BCP-47 language) blocks the emit and points the author at the saved
    /// report; warnings don't block but are surfaced. The defaults are persisted to
    /// `book.toml` first so the emitted artifacts and a later export agree.
    #[allow(clippy::needless_pass_by_value)] // the target is a tiny Copy enum handed over by the dispatch
    pub(crate) fn prepare_for_target(&mut self, target: PublishTarget) {
        if self.manuscript.is_some() {
            self.message = Some(format!(
                "Leave the continuous view (Ctrl-l) before preparing for {}.",
                target.label()
            ));
            return;
        }
        // Setting the print defaults / language edits `book.toml`; a series has no
        // single top-level `[print]`/`[style]` (each book carries its own), so guide
        // the author to set them per book rather than silently changing the wrong one.
        if self.project.book_count() > 1 {
            self.message = Some(format!(
                "Preparing for {} targets a standalone novel — set [print] trim/bleed \
                 and the language per book, then run the exports and preflight.",
                target.label()
            ));
            return;
        }
        if let Err(e) = self.apply_publish_defaults() {
            self.message = Some(format!("Couldn't set the {} defaults: {e}", target.label()));
            return;
        }

        // Run the preflight and save it — the auditable record either way.
        let report = self.preflight_report();
        let title = self.project.manifest.title.clone();
        let md = report.to_markdown(&title);
        let name = format!("preflight-{}", date::iso(date::today()));
        self.write_export(&name, "md", md.as_bytes());
        let (_passed, warned, failed) = report.tally();

        // A store rejects a FAIL outright, so don't emit a bundle that would bounce —
        // surface the count and point the author at the saved report to fix against.
        if failed > 0 {
            self.message = Some(format!(
                "Preflight for {} found {failed} failure(s) — nothing exported. \
                 Fix them ({name}.md) and re-run.",
                target.label()
            ));
            return;
        }

        // Preflight clean (or warnings only): emit the print PDF + reflowable EPUB.
        let book = bible::slug(&self.project.manifest.title);
        let doc = self.export_document(None);
        let mut warnings: Vec<String> = Vec::new();
        let (style, style_warnings) = self.export_style();
        warnings.extend(style_warnings);
        // Print interior — the strictest artifact, at the trim/bleed just set.
        let ((w, h), trim_warning) = self.print_trim();
        warnings.extend(trim_warning);
        warnings.extend(export::pdf::pdf_style_warnings(&style));
        let bleed = self.print_bleed();
        warnings.extend(export::pdf::pdf_dpi_warnings(
            &style,
            2.0f32.mul_add(bleed, w),
        ));
        let pdf = export::pdf::to_pdf_print(&doc, w, h, bleed, &style);
        let epub = export::epub::to_epub(&doc, &crate::date::utc_timestamp(), &style);

        // Write both, reporting what landed (or the first write error).
        match (pdf, epub) {
            (Ok(pdf_bytes), Ok(epub_bytes)) => {
                self.write_export(&book, "print.pdf", &pdf_bytes);
                self.write_export(&book, "epub", &epub_bytes);
                let warned_note = if warned > 0 {
                    format!(" ({warned} preflight warning(s) — see {name}.md)")
                } else {
                    String::new()
                };
                self.message = Some(format!(
                    "Prepared for {}: wrote export/{book}.print.pdf + export/{book}.epub{warned_note}",
                    target.label()
                ));
            }
            (Err(e), _) | (_, Err(e)) => self.message = Some(format!("Export failed: {e}")),
        }
        self.note_export_warnings(&warnings);
    }

    /// Set the print-on-demand defaults a publish bundle (#816) targets, persisting
    /// them to `book.toml` (standalone only): the 6×9 trim and 0.125" bleed KDP /
    /// IngramSpark want, and a default `en` language when none is set (so the EPUB's
    /// `dc:language` is a valid BCP-47 tag rather than absent). Only fills gaps and
    /// the print geometry — an author's explicit ISBN/publisher/rights are untouched.
    fn apply_publish_defaults(&mut self) -> std::io::Result<()> {
        // The print geometry: the standard POD trim and the 0.125" full-bleed
        // allowance both stores want. Set on the in-memory manifest, then saved.
        let print = self
            .project
            .manifest
            .print
            .get_or_insert_with(crate::export::pdf::PrintConfig::default);
        if print.trim.is_none() {
            print.trim = Some(crate::export::pdf::DEFAULT_TRIM.to_string());
        }
        print.bleed = Some(PUBLISH_BLEED_INCHES);
        // A default language when the author set none — a valid BCP-47 tag the
        // preflight and the EPUB `dc:language` need (never overriding a set value).
        if self.project.manifest.language.is_none() {
            self.project.manifest.language = Some(DEFAULT_LANGUAGE.to_string());
        }
        self.project.save_manifest()
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
                .map_or_else(|| format!("chapter-{}", i + 1), bible::slug),
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
                self.write_export(name, "md", export::to_markdown(doc).as_bytes());
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
                self.note_export_warnings(&warnings);
            }
            ExportFormat::Pdf => {
                let (style, mut warnings) = self.export_style();
                warnings.extend(export::pdf::pdf_style_warnings(&style));
                warnings.extend(export::pdf::pdf_dpi_warnings(
                    &style,
                    export::pdf::SCREEN_MEDIA_W,
                ));
                self.write_built(export::pdf::to_pdf(doc, &style), name, "pdf");
                self.note_export_warnings(&warnings);
            }
            ExportFormat::PdfPrint => {
                let (style, mut warnings) = self.export_style();
                let ((w, h), trim_warning) = self.print_trim();
                warnings.extend(trim_warning);
                warnings.extend(export::pdf::pdf_style_warnings(&style));
                let bleed = self.print_bleed();
                warnings.extend(export::pdf::pdf_dpi_warnings(
                    &style,
                    2.0f32.mul_add(bleed, w),
                ));
                self.write_built(
                    export::pdf::to_pdf_print(doc, w, h, bleed, &style),
                    name,
                    "print.pdf",
                );
                self.note_export_warnings(&warnings);
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
                self.note_export_warnings(&warnings);
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
                self.note_export_warnings(&warnings);
            }
            ExportFormat::PdfPrint => {
                let (style, mut warnings) = self.export_style();
                let ((w, h), trim_warning) = self.print_trim();
                warnings.extend(trim_warning);
                warnings.extend(export::pdf::pdf_style_warnings(&style));
                let bleed = self.print_bleed();
                warnings.extend(export::pdf::pdf_dpi_warnings(
                    &style,
                    2.0f32.mul_add(bleed, w),
                ));
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
                self.note_export_warnings(&warnings);
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
        let result = path.parent().map_or_else(
            || std::fs::write(&path, bytes),
            |parent| std::fs::create_dir_all(parent).and_then(|()| std::fs::write(&path, bytes)),
        );
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
    fn export_document(&self, span: Option<&std::ops::RangeInclusive<usize>>) -> Document {
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
            if span.is_some_and(|s| !s.contains(&i)) {
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
                    // `body_parts` is non-empty here; `if let` binds it without a panic.
                    if let Some(part) = body_parts.last_mut() {
                        part.chapters.push(chapter);
                    }
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

    /// Book `b`'s own publishing metadata for its divider copyright in a series
    /// export (#810): each book carries its own ISBN/publisher/date/rights (a book
    /// with none falls back to the series/active-book metadata — see
    /// [`Project::book_publishing`]), so an omnibus stamps each book's copyright
    /// with that book's data rather than reusing the active book's for all of them.
    /// Language is a package-level concern kept once on the manifest, so it stays
    /// series-level here.
    fn book_publishing_meta(&self, b: usize) -> export::PubMeta {
        let pubmeta = self.project.book_publishing(b);
        export::PubMeta {
            isbn: pubmeta.isbn,
            publisher: pubmeta.publisher,
            date: pubmeta.pub_date,
            lang: self.project.manifest.language.clone(),
            rights: pubmeta.rights,
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
                    // `None`), so `body_parts` is non-empty here; `if let` binds it
                    // without a panic path.
                    if let Some(part) = body_parts.last_mut() {
                        part.chapters.push(chapter);
                    }
                }
            }
        }
        Document {
            title: self.project.book_title(b).unwrap_or("Untitled").to_string(),
            subtitle: self.project.book_subtitle(b).map(str::to_string),
            author: self.project.manifest.author.clone(),
            // The series title is carried once by the series writer, not per book.
            series: None,
            // Each book in a series carries its OWN publishing metadata (#810): its
            // ISBN/publisher/date/rights (falling back to the series/active-book
            // metadata when unset), so its divider copyright is stamped with its own
            // data rather than the active book's reused for every book.
            publishing: self.book_publishing_meta(b),
            epigraph: self.project.book_epigraph(b),
            front,
            parts: body_parts,
            back,
        }
    }
}
