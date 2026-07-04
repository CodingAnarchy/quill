//! Export preflight / validation report (#815): a single, auditable checklist run
//! *before* an export to catch what a print/ebook store (KDP, IngramSpark) rejects
//! or flags — a missing ISBN / cover / copyright page, a low-resolution cover, a
//! non-BCP-47 language tag, bleed configured with no full-bleed cover, an empty
//! chapter. The signals already exist, scattered across the export path
//! ([`pdf::pdf_dpi_warnings`], [`PubMeta::has_copyright_page`], the trim warning,
//! [`Role`](crate::frontmatter::Role)/[`Band`](crate::frontmatter::Band)); this
//! module consolidates them into one structured [`Report`] so "why did KDP bounce
//! this?" becomes a fixable list.
//!
//! Pure over its inputs — it does no I/O and changes no export behavior — so the
//! same [`preflight`] core the run-action displays is trivially testable against a
//! book with known issues (a clean book passes; a book with no ISBN, a low-DPI
//! cover, and an empty chapter warns/fails on exactly those lines).

use std::fmt::Write as _;

use super::{pdf, ExportStyle, ImageRole, PubMeta};

/// The outcome of a single preflight check (#815). A [`Pass`](Status::Pass) is a
/// requirement met; a [`Warn`](Status::Warn) is something a store may flag but
/// accept (a soft-blurry cover, an empty chapter); a [`Fail`](Status::Fail) is
/// something a store rejects outright (no ISBN, a non-BCP-47 language tag).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Warn,
    Fail,
}

impl Status {
    /// The single-word tag shown at the head of a rendered check line.
    const fn tag(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }
}

/// One line of the preflight report (#815): a status and the message explaining it
/// — what was checked and, when it isn't a clean pass, how to fix it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub status: Status,
    pub message: String,
}

impl Check {
    fn pass(message: impl Into<String>) -> Self {
        Self {
            status: Status::Pass,
            message: message.into(),
        }
    }

    fn warn(message: impl Into<String>) -> Self {
        Self {
            status: Status::Warn,
            message: message.into(),
        }
    }

    fn fail(message: impl Into<String>) -> Self {
        Self {
            status: Status::Fail,
            message: message.into(),
        }
    }

    /// The check rendered as one line — `TAG  message` — for the report body and
    /// the saved Markdown.
    pub fn line(&self) -> String {
        format!("{}  {}", self.status.tag(), self.message)
    }
}

/// The full preflight report (#815): every check in run order. Rendered into the
/// report overlay and saved to `export/preflight-<date>.md`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    /// The number of checks that passed, warned, and failed — for the summary line
    /// and for a caller that wants to know whether anything blocks publication.
    pub fn tally(&self) -> (usize, usize, usize) {
        let mut passed = 0;
        let mut warned = 0;
        let mut failed = 0;
        for c in &self.checks {
            match c.status {
                Status::Pass => passed += 1,
                Status::Warn => warned += 1,
                Status::Fail => failed += 1,
            }
        }
        (passed, warned, failed)
    }

    /// The whole report as plain text: a title, the summary, then each check on its
    /// own line — the Markdown saved under `export/`, and the basis for the overlay.
    pub fn to_markdown(&self, title: &str) -> String {
        let (passed, warned, failed) = self.tally();
        let mut out = format!("# Export preflight — {title}\n\n");
        let _ = write!(
            out,
            "{passed} passed · {warned} warning(s) · {failed} failure(s)\n\n"
        );
        for check in &self.checks {
            out.push_str(&check.line());
            out.push('\n');
        }
        out
    }
}

/// One chapter's shape for the empty-chapter / emphasis checks (#815): its title,
/// its word count (so an empty body is caught without re-reading the file here),
/// whether it's a real body chapter (a deliberately terse dedication or colophon
/// isn't "empty" the way a stubbed body chapter is), and its prose (for the
/// optional emphasis scan). The caller assembles these from the project it already
/// has loaded — this module reads no files.
pub struct ChapterInfo<'a> {
    pub title: &'a str,
    pub words: usize,
    pub is_body: bool,
    /// The chapter's prose, for the optional unbalanced-emphasis scan. The caller
    /// already reads it for the word count; passing it here keeps the check pure.
    pub body: &'a str,
}

/// The print configuration the preflight checks against (#815): the media-box
/// width (points) the cover is scaled to fill, the bleed allowance, and any trim
/// warning — all read from the manifest by the caller, none re-derived here.
pub struct PrintInfo {
    /// The full media-box width (points) a print cover fills: trim width plus bleed
    /// on both sides. Feeds the DPI floor, matching what the PDF writer emits.
    pub media_w_pts: f32,
    /// The `[print] bleed` allowance in points, `0` when unset.
    pub bleed_pts: f32,
    /// A warning about the configured trim (an unknown size falling back to the
    /// default), threaded through from the app's `print_trim` so the report carries
    /// it too rather than re-deriving the trim table.
    pub trim_warning: Option<String>,
}

/// Whether a language tag is *structurally* a BCP-47 tag (#815): a primary language
/// subtag of 2–3 ASCII letters (ISO 639), then any number of `-`-joined subtags
/// each 1–8 alphanumerics (script, region, variant). This is the shape KDP/EPUB
/// expect for `dc:language`; it won't catch a well-formed-but-unassigned tag
/// (`en-ZZ`), but it catches the common mistakes — a full language *name*
/// ("English"), an underscore locale (`en_US`), or stray whitespace — which are
/// what get flagged. (The 4–8-letter reserved primary range exists in the grammar
/// but is unassigned, so a 4+-letter primary is far likelier a typo than a tag.)
fn is_bcp47(tag: &str) -> bool {
    let mut subtags = tag.split('-');
    let Some(primary) = subtags.next() else {
        return false;
    };
    let primary_ok =
        (2..=3).contains(&primary.len()) && primary.chars().all(|c| c.is_ascii_alphabetic());
    if !primary_ok {
        return false;
    }
    subtags.all(|s| (1..=8).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Whether a chapter's prose has balanced Markdown emphasis (#815): an even count
/// of `*` and `_` markers so no `*italic` or `**bold` span is left open — an
/// unbalanced marker leaks a literal `*` into the exported text. A coarse count (it
/// doesn't parse spans), so it errs toward silence: it flags only a clearly odd
/// total, the kind that renders a literal asterisk in the book.
fn emphasis_is_balanced(body: &str) -> bool {
    body.matches('*').count().is_multiple_of(2) && body.matches('_').count().is_multiple_of(2)
}

/// Run the export preflight (#815): check the assembled book's publishing metadata
/// and print configuration against store requirements and return the structured
/// [`Report`]. Pure over its inputs — reuses the export path's own signals
/// ([`pdf::pdf_dpi_warnings`], [`PubMeta::has_copyright_page`], the trim warning,
/// the front/back-matter band via `is_body`) rather than re-deriving them — so the
/// report the author sees is exactly what the export would warn about, plus the
/// store-facing metadata the transient warnings never covered.
///
/// `print` is `Some` only for a print-interior target, where a physical trim,
/// bleed, and print-DPI floor apply; a screen/ebook preflight passes `None` and is
/// checked against the screen media box.
pub fn preflight(
    meta: &PubMeta,
    style: &ExportStyle,
    chapters: &[ChapterInfo<'_>],
    print: Option<&PrintInfo>,
    check_emphasis: bool,
) -> Report {
    let mut checks = Vec::new();

    // --- Store-facing metadata (what KDP/IngramSpark demand up front) ---
    checks.push(match &meta.isbn {
        Some(isbn) if !isbn.trim().is_empty() => Check::pass(format!("ISBN set ({isbn})")),
        _ => Check::fail("no ISBN — print/ebook stores require one (set [book] isbn in book.toml)"),
    });

    checks.push(if meta.has_copyright_page() {
        Check::pass("copyright page will be generated (publishing metadata is set)")
    } else {
        Check::warn("no copyright page — set rights/publisher/date/ISBN so one is generated")
    });

    // Language: present and structurally BCP-47. An unset tag falls back to `en`
    // (a warn with a note); a malformed one fails, since a store rejects it.
    checks.push(match &meta.lang {
        None => Check::warn("no language set — defaulting to 'en' (set [book] language)"),
        Some(tag) if is_bcp47(tag) => {
            Check::pass(format!("language '{tag}' is a valid BCP-47 tag"))
        }
        Some(tag) => Check::fail(format!(
            "language '{tag}' is not a BCP-47 tag (use e.g. 'en', 'fr', 'es-419')"
        )),
    });

    // --- Cover (presence + resolution) ---
    let has_cover = style.image(ImageRole::Cover).is_some();
    if has_cover {
        checks.push(Check::pass("cover image is set"));
        // DPI floor: reuse the PDF writer's own check so the report matches the
        // emitted file. A print target measures against the full media box (trim +
        // bleed); a screen/ebook target against the screen media box.
        let media_w = print.map_or(pdf::SCREEN_MEDIA_W, |p| p.media_w_pts);
        let dpi_warnings = pdf::pdf_dpi_warnings(style, media_w);
        if dpi_warnings.is_empty() {
            checks.push(Check::pass(
                "cover resolution is at or above the print DPI floor",
            ));
        } else {
            checks.extend(dpi_warnings.into_iter().map(Check::warn));
        }
    } else {
        checks.push(Check::fail(
            "no cover image — ebook/print stores require one (set [style] cover_image)",
        ));
    }

    // --- Bleed / full-bleed cover + trim (print only) ---
    if let Some(print) = print {
        if let Some(w) = &print.trim_warning {
            checks.push(Check::warn(w.clone()));
        }
        if print.bleed_pts > 0.0 {
            checks.push(if has_cover {
                Check::pass("bleed is set and a cover is present to fill it")
            } else {
                Check::fail(
                    "bleed is set but there's no full-bleed cover to fill it \
                     (add a cover or clear [print] bleed)",
                )
            });
        }
    }

    // --- Empty chapters ---
    let empty: Vec<&str> = chapters
        .iter()
        .filter(|c| c.is_body && c.words == 0)
        .map(|c| c.title)
        .collect();
    checks.push(if empty.is_empty() {
        Check::pass("no empty chapters")
    } else {
        Check::warn(format!(
            "empty chapter(s): {} — write or remove before publishing",
            empty.join(", ")
        ))
    });

    // --- Unbalanced emphasis (optional) ---
    if check_emphasis {
        let unbalanced: Vec<&str> = chapters
            .iter()
            .filter(|c| !emphasis_is_balanced(c.body))
            .map(|c| c.title)
            .collect();
        checks.push(if unbalanced.is_empty() {
            Check::pass("emphasis markers are balanced")
        } else {
            Check::warn(format!(
                "unbalanced emphasis (*/_) in: {} — a stray marker prints literally",
                unbalanced.join(", ")
            ))
        });
    }

    Report { checks }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width`×`width` solid PNG — a real raster [`pdf_dpi_warnings`] can decode
    /// to measure the effective DPI, the same way the export path embeds a cover.
    fn png(width: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(width, width, image::Rgba([255, 0, 0, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode test cover");
        bytes.into_inner()
    }

    /// A style carrying a square cover of `width_px` pixels, resolved through the
    /// real image-resolution path — so the DPI check decodes it exactly as an export
    /// does. A narrow cover trips the DPI floor; a wide one clears it.
    fn style_with_cover(width_px: u32) -> ExportStyle {
        let mut style = ExportStyle {
            cover_image: Some("cover.png".into()),
            ..Default::default()
        };
        let warnings = style.resolve_images(|_| Ok(png(width_px)));
        assert!(warnings.is_empty(), "test cover resolves cleanly");
        style
    }

    fn chapter<'a>(title: &'a str, words: usize, body: &'a str) -> ChapterInfo<'a> {
        ChapterInfo {
            title,
            words,
            is_body: true,
            body,
        }
    }

    /// A book with known issues — no ISBN, a low-DPI cover, and an empty chapter —
    /// fails/warns on exactly those, and passes the checks that are clean.
    #[test]
    fn a_book_with_known_issues_reports_them() {
        let meta = PubMeta {
            lang: Some("en".into()),
            ..Default::default()
        };
        let style = style_with_cover(200); // ~24 DPI over the screen box — under 300
        let chapters = [
            chapter("One", 1200, "The night was *dark*."),
            chapter("Two", 0, ""),
        ];
        let report = preflight(&meta, &style, &chapters, None, false);
        let text = report.to_markdown("Test Book");

        assert!(
            text.contains("FAIL  no ISBN"),
            "missing ISBN should fail:\n{text}"
        );
        assert!(
            text.lines()
                .any(|l| l.starts_with("WARN") && l.contains("DPI")),
            "low-DPI cover should warn:\n{text}"
        );
        assert!(
            text.contains("empty chapter(s): Two"),
            "the empty chapter should be named:\n{text}"
        );
        // The language is a valid tag, so that line passes even in a flawed book.
        assert!(
            text.contains("PASS  language 'en'"),
            "a valid language still passes:\n{text}"
        );
        let (_, warned, failed) = report.tally();
        assert!(failed >= 1 && warned >= 2, "counts roll up: {report:?}");
    }

    /// A fully-specified book with a high-resolution cover and no empty chapters
    /// passes every check.
    #[test]
    fn a_clean_book_passes() {
        let meta = PubMeta {
            isbn: Some("978-1-234-56789-7".into()),
            publisher: Some("Quill Press".into()),
            date: Some("2026".into()),
            lang: Some("en-US".into()),
            rights: Some("© 2026 A. Writer.".into()),
        };
        let style = style_with_cover(3000); // ~353 DPI over the screen box — clean
        let chapters = [
            chapter("One", 1200, "A balanced **bold** and *italic*."),
            chapter("Two", 800, "More prose."),
        ];
        let report = preflight(&meta, &style, &chapters, None, true);
        let (_, warned, failed) = report.tally();
        assert_eq!(
            (warned, failed),
            (0, 0),
            "a clean book has no warnings or failures:\n{}",
            report.to_markdown("Clean Book")
        );
    }

    /// A non-BCP-47 language tag (a full language name, an underscore locale) is
    /// rejected; well-formed tags pass.
    #[test]
    fn language_tag_is_validated() {
        assert!(is_bcp47("en"));
        assert!(is_bcp47("fr"));
        assert!(is_bcp47("es-419"));
        assert!(is_bcp47("en-US"));
        assert!(is_bcp47("zh-Hant"));
        assert!(!is_bcp47("English"));
        assert!(!is_bcp47("en_US"));
        assert!(!is_bcp47("e"));
        assert!(!is_bcp47(""));
        assert!(!is_bcp47("en US"));
    }

    /// Bleed set with no cover fails on the full-bleed check; the check is print-only.
    #[test]
    fn bleed_without_a_cover_fails() {
        let meta = PubMeta::default();
        let no_cover = ExportStyle::default();
        let print = PrintInfo {
            media_w_pts: 468.0,
            bleed_pts: 9.0,
            trim_warning: None,
        };
        let chapters = [chapter("One", 500, "prose")];
        let report = preflight(&meta, &no_cover, &chapters, Some(&print), false);
        assert!(
            report
                .to_markdown("t")
                .contains("FAIL  bleed is set but there's no full-bleed cover"),
            "bleed with no cover should fail:\n{}",
            report.to_markdown("t")
        );
    }

    /// The unbalanced-emphasis check names the offending chapter, and only runs when
    /// asked.
    #[test]
    fn unbalanced_emphasis_is_optional_and_named() {
        let meta = PubMeta::default();
        let style = ExportStyle::default();
        let chapters = [chapter("Bad", 5, "an *open span")];
        let off = preflight(&meta, &style, &chapters, None, false);
        assert!(
            !off.to_markdown("t").contains("emphasis"),
            "emphasis check is off by default"
        );
        let on = preflight(&meta, &style, &chapters, None, true);
        assert!(
            on.to_markdown("t")
                .contains("unbalanced emphasis (*/_) in: Bad"),
            "the emphasis check names the chapter:\n{}",
            on.to_markdown("t")
        );
    }
}
