//! The file-writing plumbing (#7/#332): render a compiled [`Document`] (or a whole
//! series) in each format and write it under `export/<name>.<ext>`, plus the print
//! wrap-cover export (#819) whose spine is sized from this book's interior page
//! count. Shared by every export path and the report snapshots (#601).

use super::super::super::*;

use crate::ai::bible;
use crate::export::{self, Document};
use crate::mode::ExportFormat;

impl App {
    /// Export the print wrap cover (#819): a single full-bleed PDF at
    /// `2×trim + spine + 2×bleed` with the front-cover art on the front panel, a
    /// reserved barcode/ISBN zone on the back, and a spine sized from *this book's*
    /// interior page count × the paper caliper. Lays the print interior out at the same
    /// trim/bleed the print-interior export uses to read the page count (so the spine
    /// matches the block that ships beside it), then renders the wrap. Standalone only —
    /// the spine is one book's, so a series wraps per book (guarded in the palette).
    /// Compiles from the single-chapter view, like the other exports, so nothing is read
    /// stale. Writes to `export/<book>.cover.pdf`.
    pub(crate) fn run_wrap_cover_export(&mut self) {
        if self.manuscript.is_some() {
            self.message =
                Some("Leave the continuous view (Ctrl-l) before exporting the wrap cover.".into());
            return;
        }
        let doc = self.export_document(None);
        let (style, mut warnings) = self.export_style();
        let ((w, h), trim_warning) = self.print_trim();
        warnings.extend(trim_warning);
        warnings.extend(export::pdf::pdf_style_warnings(&style));
        let bleed = self.print_bleed();
        // The front panel is one trim wide (+ bleed); check the cover DPI against that
        // width, the same floor the interior cover check uses (#696).
        warnings.extend(export::pdf::pdf_dpi_warnings(&style, bleed + w));
        // A cover with no front-cover image is a hollow wrap — surface it (not a hard
        // fail; the geometry, barcode zone, and spine guides are still useful to a
        // designer who'll drop the art in).
        if style.image(crate::export::ImageRole::Cover).is_none() {
            warnings
                .push("cover: no cover image set — the wrap's front panel is left blank".into());
        }
        // Lay the interior out at the very trim/bleed the print export uses so the spine
        // width matches the page count of the block that ships with this cover (#819).
        let page_count = export::pdf::print_page_count(&doc, w, h, bleed, &style);
        let geo = export::pdf::WrapGeometry::new(w, h, bleed, page_count, self.print_caliper());
        let author = doc.author.as_deref().unwrap_or_default();
        let built = export::pdf::to_wrap_cover(geo, &style, &doc.title, author);
        let name = bible::slug(&self.project.manifest.title);
        self.write_built(built, &name, "cover.pdf");
        self.note_export_warnings(&warnings);
    }

    /// Write one book's [`Document`] in `format` to `export/<name>.<ext>`.
    pub(super) fn write_book(&mut self, format: ExportFormat, doc: &Document, name: &str) {
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
    pub(super) fn write_series(
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
    pub(super) fn write_built(&mut self, built: std::io::Result<Vec<u8>>, name: &str, ext: &str) {
        match built {
            Ok(bytes) => self.write_export(name, ext, &bytes),
            Err(e) => self.message = Some(format!("Export failed: {e}")),
        }
    }

    /// `export/<name>.<ext>` under the project root.
    pub(super) fn export_path(&self, name: &str, ext: &str) -> std::path::PathBuf {
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
            Ok(()) => match self.missing_stories() {
                0 => format!("Exported to {shown}"),
                // A collection exports without stories it can't read (#1245).
                n => format!(
                    "Exported to {shown} — {n} missing {} left out (Ctrl-G to locate)",
                    if n == 1 { "story" } else { "stories" }
                ),
            },
            Err(e) => format!("Export failed: {e}"),
        });
    }
}
