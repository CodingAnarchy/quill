//! The export palette (#7/#584): open the format menu (whole book, whole series
//! #332, or a chapter/part/range sample #584), dispatch the pick to the compile +
//! write path, and name each scoped artifact so a sample never clobbers the full
//! export beside it.

use super::super::super::*;

use crate::ai::bible;
use crate::frontmatter::Band;
use crate::mode::{Action, Command, ExportFormat, ExportScope, Palette, PaletteItem};

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
    /// Open the export menu (#7): pick a format, then [`run_export`](Self::run_export)
    /// writes it. A multi-book series (#332) offers each format twice — the whole
    /// series and just the active book; a standalone novel lists the formats once.
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
            // The print wrap cover (#819) — a standalone artifact whose spine is sized
            // from this book's interior page count, so it sits with the whole-book
            // formats (a series wraps per book). Runs the action, not a Document compile.
            items.push(PaletteItem::new(
                "PDF wrap cover (front + spine + back)",
                "Print wrap — spine sized from the interior page count",
                Command::Run(Action::PdfWrapCover),
            ));
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

    /// The output base name (sans extension) for a scoped export. A whole-book
    /// export is named for the book; a *sample* (#584) appends a scope tag — the
    /// chapter or part slug, or `chapters-N-M` for a range — so a sample never
    /// overwrites the full export sitting beside it.
    pub(super) fn export_name(&self, scope: &ExportScope) -> String {
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
}
