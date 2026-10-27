//! Designed-export styling, warnings, and the style preview (#468/#483). Resolves
//! the manifest `[style]` (preset + fonts/images) shared by the EPUB and PDF
//! writers, surfaces its warnings on the export message, reads the print trim/bleed
//! (#652), and renders a fixed sample book through the real writer for a preview.

use super::super::*;

use std::sync::atomic::{AtomicU64, Ordering};

use crate::export::{self, Chapter, Document, ExportStyle, Part};
use crate::frontmatter::Epigraph;
use std::process::Command as Proc;

impl App {
    /// The print bleed allowance in points (#652) from the manifest `[print] bleed`,
    /// or `0` when unset. A full-bleed cover extends past the trim by this much.
    pub(super) fn print_bleed(&self) -> f32 {
        self.project
            .manifest
            .print
            .as_ref()
            .map_or(0.0, crate::export::pdf::PrintConfig::bleed_points)
    }

    /// The per-page paper caliper in inches (#819) from the manifest `[print] caliper`,
    /// or the writer's sane default when unset. Sizes the wrap cover's spine.
    pub(super) fn print_caliper(&self) -> f32 {
        self.project.manifest.print.as_ref().map_or(
            export::pdf::DEFAULT_CALIPER_INCHES,
            crate::export::pdf::PrintConfig::caliper_inches,
        )
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
        export::pdf::trim_points(name).map_or_else(
            || {
                // `DEFAULT_TRIM` is a known size, so `trim_points` resolves it; the
                // 6×9 points are the fallback if it somehow didn't, rather than a panic.
                let default = export::pdf::trim_points(export::pdf::DEFAULT_TRIM)
                    .unwrap_or((6.0 * 72.0, 9.0 * 72.0));
                (
                    default,
                    Some(format!(
                        "print: unknown trim '{name}' (use 6x9/5x8/5.5x8.5) — using {}",
                        export::pdf::DEFAULT_TRIM
                    )),
                )
            },
            |dims| (dims, None),
        )
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
    pub(super) fn note_export_warnings(&mut self, warnings: &[String]) {
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
        let path = unique_preview_path("quill-style-preview", "epub");
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
                ));
            }
        }
        self.note_export_warnings(&warnings);
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

    /// Preview the *print interior* style (#1107): render the fixed sample chapter
    /// through the **same** print PDF writer a real export uses — at the manifest's
    /// current `[print]` trim/bleed and resolved `[style]` — so an author can see the
    /// drop cap, heading alignment, trim size, and margins on a representative page
    /// without running a full export. Writes only a temp PDF and opens it in the
    /// system PDF viewer (the same `open_in_default_app` helper the EPUB preview uses);
    /// the project is never modified.
    pub(crate) fn preview_print_interior(&mut self) {
        let (bytes, warnings) = match self.print_interior_preview_pdf() {
            Ok(built) => built,
            Err(e) => {
                self.message = Some(format!("Print preview failed: {e}"));
                return;
            }
        };
        let path = unique_preview_path("quill-print-preview", "pdf");
        if let Err(e) = std::fs::write(&path, &bytes) {
            self.message = Some(format!("Print preview failed: {e}"));
            return;
        }
        match open_in_default_app(&path) {
            Ok(()) => self.message = Some(format!("Print preview opened ({})", path.display())),
            // The PDF is written; only the launch failed — point the author at it.
            Err(e) => {
                self.message = Some(format!(
                    "Wrote {} but couldn't open it: {e}",
                    path.display()
                ));
            }
        }
        self.note_export_warnings(&warnings);
    }

    /// Render the fixed sample chapter (#1107) through the resolved `[style]` and the
    /// **print** PDF writer at the manifest's current `[print]` trim/bleed, returning
    /// the PDF bytes and any styling/trim warnings — the pure, side-effect-free core
    /// of [`preview_print_interior`] (no temp file, no viewer launch). Reuses the same
    /// `to_pdf_print` entry point a real print export drives, so the preview is
    /// faithful; only the sample [`Document`] differs.
    pub(crate) fn print_interior_preview_pdf(&self) -> std::io::Result<(Vec<u8>, Vec<String>)> {
        let (style, mut warnings) = self.export_style();
        let ((w, h), trim_warning) = self.print_trim();
        warnings.extend(trim_warning);
        warnings.extend(export::pdf::pdf_style_warnings(&style));
        let bleed = self.print_bleed();
        let doc = style_preview_document();
        let bytes = export::pdf::to_pdf_print(&doc, w, h, bleed, &style)?;
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
        // Show the generated front matter (#820): a half-title leaf and an "also in
        // this series" page (the sample novel's own title is filtered out of the list).
        half_title: true,
        series_books: vec![
            "The Sample Novel".into(),
            "The Second Sample".into(),
            "The Third Sample".into(),
        ],
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
        story: None,
    }
}

/// A monotonic per-process counter making each preview path unique even when the
/// same process previews repeatedly (a fresh temp file every keystroke), so we never
/// truncate a still-open reader's file.
static PREVIEW_SEQ: AtomicU64 = AtomicU64::new(0);

/// A unique temp path for a style/print preview (#1146): `<stem>-<pid>-<seq>.<ext>`
/// in the system temp dir. Keyed by process id — mirroring the atomic-save temp
/// naming ([`crate::atomic`]) — so concurrent processes (e.g. parallel tests) never
/// collide, plus a monotonic counter so repeated previews within one process each get
/// their own file. Because the name is unique per call, a pre-existing fixed path
/// (stale or planted) is never clobbered and two previews never race on one file.
fn unique_preview_path(stem: &str, ext: &str) -> std::path::PathBuf {
    let pid = std::process::id();
    let seq = PREVIEW_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{stem}-{pid}-{seq}.{ext}"))
}

/// Open `path` in the operating system's default application — the EPUB reader for
/// the style preview (#483), the PDF viewer for the print-interior preview (#1107).
/// Spawns the platform launcher and returns without waiting, so the editor keeps
/// running.
fn open_in_default_app(path: &std::path::Path) -> std::io::Result<()> {
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

#[cfg(test)]
mod tests {
    use super::unique_preview_path;
    use std::collections::HashSet;

    #[test]
    fn preview_paths_are_unique_and_never_a_fixed_name() {
        // Repeated previews must each get a distinct path (#1146): concurrent or
        // back-to-back previews never race on one file, and the old fixed name
        // (`quill-style-preview.epub`) — which a stale or planted file could sit at —
        // is never reused, so it is never clobbered.
        let a = unique_preview_path("quill-style-preview", "epub");
        let b = unique_preview_path("quill-style-preview", "epub");
        assert_ne!(a, b, "repeated previews must not collide");
        for p in [&a, &b] {
            let name = p.file_name().unwrap().to_string_lossy();
            assert_ne!(
                name, "quill-style-preview.epub",
                "must not use the fixed name"
            );
            assert!(name.starts_with("quill-style-preview-"));
            assert!(name.ends_with(".epub"));
            assert!(
                name.contains(&std::process::id().to_string()),
                "path is keyed by pid so concurrent processes don't collide"
            );
            assert_eq!(p.parent(), Some(std::env::temp_dir().as_path()));
        }
        // A burst of previews yields all-distinct paths (the monotonic counter).
        let paths: HashSet<_> = (0..64)
            .map(|_| unique_preview_path("quill-print-preview", "pdf"))
            .collect();
        assert_eq!(paths.len(), 64, "every preview in a burst is a fresh path");
    }
}
