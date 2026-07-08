//! The publish flows (#815/#816): the export preflight checklist and the one-click
//! "Prepare for KDP / IngramSpark" bundle that sets the store's print defaults, runs
//! the preflight, and — unless it fails — emits the print PDF + reflowable EPUB
//! together. Checked against the strictest (print-interior) profile.

use super::super::super::*;

use crate::ai::bible;
use crate::export;
use crate::frontmatter::Band;

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

impl App {
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
        // Resolve the active index once — `active_index()` is O(n), so the per-chapter
        // count/text reads would otherwise make this O(chapters²) (#865, #867).
        let active = self.project.active_index();
        let chapters: Vec<(String, usize, bool, String)> = (0..self.chapter_count())
            .map(|i| {
                (
                    self.project
                        .chapter_title(i)
                        .unwrap_or("Untitled")
                        .to_string(),
                    self.chapter_word_count_with_active(i, active),
                    self.project.chapter_band(i) == Band::Body,
                    self.chapter_text_with_active(i, active),
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
}
