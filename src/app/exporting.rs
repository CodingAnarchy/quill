//! Manuscript export orchestration (#7): assemble the live project into the
//! shared [`crate::export::Document`] and write it out. The structure assembly is
//! here; format-specific rendering lives in `crate::export`. Split out of
//! `app/mod.rs`; `App` methods via `use super::*`.

use super::*;

use crate::export::{self, Document, Item};

impl App {
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

    /// Compile the manuscript to a single Markdown file under `export/` (#190),
    /// overwriting any previous one so it always reflects the current project.
    pub(crate) fn export_markdown(&mut self) {
        // The continuous view's edits live in the windowed buffer, not the
        // per-chapter ones export reads — compile from the single-chapter view so
        // nothing is exported stale (same constraint as undo / structure edits).
        if self.manuscript.is_some() {
            self.message = Some("Leave the continuous view (Ctrl-l) before exporting.".into());
            return;
        }
        let markdown = export::to_markdown(&self.export_document());
        let path = self
            .project
            .root
            .join("export")
            .join(format!("{}.md", bible::slug(&self.project.manifest.title)));
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                self.message = Some(format!("Export failed: {e}"));
                return;
            }
        }
        let shown = path.strip_prefix(&self.project.root).unwrap_or(&path).display();
        self.message = Some(match std::fs::write(&path, markdown) {
            Ok(()) => format!("Exported to {shown}"),
            Err(e) => format!("Export failed: {e}"),
        });
    }
}
