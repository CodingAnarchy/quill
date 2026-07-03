//! Designed-export styling, warnings, and the style preview (#468/#483). Resolves
//! the manifest `[style]` (preset + fonts/images) shared by the EPUB and PDF
//! writers, surfaces its warnings on the export message, reads the print trim/bleed
//! (#652), and renders a fixed sample book through the real writer for a preview.

use super::super::*;

use crate::export::{self, Chapter, Document, ExportStyle, Part};
use crate::frontmatter::Epigraph;

impl App {
    /// The print bleed allowance in points (#652) from the manifest `[print] bleed`,
    /// or `0` when unset. A full-bleed cover extends past the trim by this much.
    pub(super) fn print_bleed(&self) -> f32 {
        self.project
            .manifest
            .print
            .as_ref()
            .map(|p| p.bleed_points())
            .unwrap_or(0.0)
    }

    /// The print trim (in points) from the manifest `[print]` table, with a warning
    /// when the configured size is unknown (the default is used instead).
    pub(super) fn print_trim(&self) -> ((f32, f32), Option<String>) {
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
    pub(super) fn export_style(&self) -> (ExportStyle, Vec<String>) {
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
    pub(super) fn note_export_warnings(&mut self, warnings: Vec<String>) {
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
        publishing: export::PubMeta::default(),
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
                role: None,
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
