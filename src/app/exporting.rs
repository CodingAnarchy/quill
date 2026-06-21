//! Manuscript export orchestration (#7): assemble the live project into the
//! shared [`crate::export::Document`] and write it out. The structure assembly is
//! here; format-specific rendering lives in `crate::export`. Split out of
//! `app/mod.rs`; `App` methods via `use super::*`.

use super::*;

use crate::export::{self, Chapter, Document, Part};
use crate::frontmatter::Band;
use crate::mode::{Command, ExportFormat, Palette, PaletteItem};

impl App {
    /// Open the export menu (#7): pick a format, then [`run_export`] writes it.
    pub(crate) fn open_export_palette(&mut self) {
        let item = |label, desc, format| PaletteItem { label, desc, command: Command::Export(format) };
        let items = vec![
            item("Manuscript (Markdown)", "One compiled .md file — hand off or convert", ExportFormat::Markdown),
            item("Word (.docx)", "Standard manuscript format", ExportFormat::Docx),
            item("Scrivener (.scriv)", "A project bundle — binder, parts, chapters", ExportFormat::Scriv),
        ];
        self.mode = Mode::Palette(Box::new(Palette { title: "Export", items, selected: 0 }));
    }

    /// Compile the manuscript and write it under `export/<slug>.<ext>` in the
    /// chosen format (#7), overwriting any previous one so it always reflects the
    /// current project.
    pub(crate) fn run_export(&mut self, format: ExportFormat) {
        // The continuous view's edits live in the windowed buffer, not the
        // per-chapter ones export reads — compile from the single-chapter view so
        // nothing is exported stale (same constraint as undo / structure edits).
        if self.manuscript.is_some() {
            self.message = Some("Leave the continuous view (Ctrl-l) before exporting.".into());
            return;
        }
        let doc = self.export_document();
        match format {
            ExportFormat::Markdown => {
                let markdown = export::to_markdown(&doc);
                self.write_export("md", markdown.as_bytes());
            }
            ExportFormat::Docx => match export::docx::to_docx(&doc) {
                Ok(bytes) => self.write_export("docx", &bytes),
                Err(e) => self.message = Some(format!("Export failed: {e}")),
            },
            ExportFormat::Scriv => {
                // A `.scriv` is a bundle directory, not a single file.
                let path = self.export_path("scriv");
                let result = std::fs::create_dir_all(path.parent().unwrap_or(&path))
                    .and_then(|()| export::scriv::write_scriv(&doc, &path));
                self.report_export(&path, result);
            }
        }
    }

    /// `export/<slug>.<ext>` under the project root.
    fn export_path(&self, ext: &str) -> std::path::PathBuf {
        self.project
            .root
            .join("export")
            .join(format!("{}.{ext}", bible::slug(&self.project.manifest.title)))
    }

    /// Write the export bytes to `export/<slug>.<ext>` and report the path.
    fn write_export(&mut self, ext: &str, bytes: &[u8]) {
        let path = self.export_path(ext);
        let result = match path.parent() {
            Some(parent) => std::fs::create_dir_all(parent).and_then(|()| std::fs::write(&path, bytes)),
            None => std::fs::write(&path, bytes),
        };
        self.report_export(&path, result);
    }

    /// Set the status message from an export's outcome and target path.
    fn report_export(&mut self, path: &std::path::Path, result: std::io::Result<()>) {
        let shown = path.strip_prefix(&self.project.root).unwrap_or(path).display();
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
        let mut group: Option<Option<usize>> = None;
        for i in 0..self.chapter_count() {
            let chapter = self.export_chapter(i);
            match self.project.chapter_band(i) {
                Band::Front => front.push(chapter),
                Band::Back => back.push(chapter),
                Band::Body => {
                    let part = parts.iter().position(|(_, range)| range.contains(&i));
                    if group != Some(part) {
                        group = Some(part);
                        body_parts.push(Part {
                            name: part.map(|p| parts[p].0.clone()),
                            chapters: Vec::new(),
                        });
                    }
                    body_parts.last_mut().unwrap().chapters.push(chapter);
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
        let title = self.project.chapter_title(i).unwrap_or("Untitled").to_string();
        let body = export::strip_repeated_title(&self.chapter_text(i), &title);
        Chapter { title, epigraph: self.chapter_epigraph(i), body }
    }

}
