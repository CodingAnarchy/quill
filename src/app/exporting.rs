//! Manuscript export orchestration (#7): assemble the live project into the
//! shared [`crate::export::Document`] and write it out. The structure assembly is
//! here; format-specific rendering lives in `crate::export`. Split out of
//! `app/mod.rs`; `App` methods via `use super::*`.

use super::*;

use crate::export::{self, Document, Item};
use crate::mode::{Command, ExportFormat, Palette, PaletteItem};

impl App {
    /// Open the export menu (#7): pick a format, then [`run_export`] writes it.
    pub(crate) fn open_export_palette(&mut self) {
        let item = |label, desc, format| PaletteItem { label, desc, command: Command::Export(format) };
        let items = vec![
            item("Manuscript (Markdown)", "One compiled .md file — hand off or convert", ExportFormat::Markdown),
            item("Word (.docx)", "Standard manuscript format", ExportFormat::Docx),
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
        }
    }

    /// Write the export bytes to `export/<slug>.<ext>` and report the path.
    fn write_export(&mut self, ext: &str, bytes: &[u8]) {
        let path = self
            .project
            .root
            .join("export")
            .join(format!("{}.{ext}", bible::slug(&self.project.manifest.title)));
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                self.message = Some(format!("Export failed: {e}"));
                return;
            }
        }
        let shown = path.strip_prefix(&self.project.root).unwrap_or(&path).display();
        self.message = Some(match std::fs::write(&path, bytes) {
            Ok(()) => format!("Exported to {shown}"),
            Err(e) => format!("Export failed: {e}"),
        });
    }

    /// Assemble the project into the export structure model (#190): book metadata,
    /// then parts and chapters in reading order (front matter → body → back
    /// matter, already the manifest order) with their epigraphs and prose. The
    /// active chapter's prose comes live from its buffer, the rest from disk.
    fn export_document(&self) -> Document {
        let m = &self.project.manifest;
        let parts = self.part_ranges();
        let mut items = Vec::new();
        for i in 0..self.chapter_count() {
            // A part heading precedes its first chapter.
            if let Some((name, _)) = parts.iter().find(|(_, range)| range.start == i) {
                items.push(Item::Part(name.clone()));
            }
            let title = self.project.chapter_title(i).unwrap_or("Untitled").to_string();
            let body = export::strip_repeated_title(&self.chapter_text(i), &title);
            items.push(Item::Chapter { title, epigraph: self.chapter_epigraph(i), body });
        }
        Document {
            title: m.title.clone(),
            subtitle: m.subtitle.clone(),
            author: m.author.clone(),
            series: m.series.clone(),
            epigraph: m.epigraph(),
            items,
        }
    }

}
