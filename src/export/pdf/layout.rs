//! Page geometry and the font plan of one render — the self-contained layer the
//! screen and print profiles differ by, plus the print-interior config and trim
//! sizes that feed it. The [`Pdf`](super::Pdf) builder consults a `Layout` for the
//! text column and glyph metrics; everything here is pure measurement, no drawing.

use serde::{Deserialize, Serialize};

use super::font::Fonts;
use super::{ExportStyle, FontStyle};

/// The default print trim, used when a project sets no `[print] trim`.
pub const DEFAULT_TRIM: &str = "6x9";

/// The print trim sizes the writer supports (#464), in the order the in-app
/// picker (#569) and the unknown-trim warning list them. The single source of
/// truth: every name here resolves through [`trim_points`] (a test asserts it).
pub const TRIM_NAMES: [&str; 3] = ["6x9", "5x8", "5.5x8.5"];

/// Opt-in print-interior settings (#464), read from the manifest `[print]` table.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct PrintConfig {
    /// The book trim size, e.g. `"6x9"` (see [`trim_points`]). Absent → 6×9.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim: Option<String>,
    /// The bleed allowance in inches (#652) — how far full-bleed art (the cover,
    /// full-page images) extends past the trim edge for print-on-demand. POD services
    /// (KDP/IngramSpark) want 0.125" on anything meant to reach the page edge. Absent
    /// or `0` → no bleed (the media box is exactly the trim), leaving today's output
    /// unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bleed: Option<f32>,
}

impl PrintConfig {
    /// The configured bleed in PDF points (1/72"), clamped to non-negative; `0` when
    /// unset. Screen exports never call this — bleed is a print-only allowance.
    pub fn bleed_points(&self) -> f32 {
        (self.bleed.unwrap_or(0.0) * 72.0).max(0.0)
    }
}

/// A named trim size in PDF points (1/72"), or `None` if unrecognized.
pub fn trim_points(name: &str) -> Option<(f32, f32)> {
    let (w, h) = match name {
        "6x9" => (6.0, 9.0),
        "5x8" => (5.0, 8.0),
        "5.5x8.5" => (5.5, 8.5),
        _ => return None,
    };
    Some((w * 72.0, h * 72.0))
}

/// The page geometry and font plan of one render — the only difference between the
/// screen and print profiles.
pub(super) struct Layout {
    /// The trim width/height — the finished page the reader holds. Content margins are
    /// measured from the trim, not the (possibly larger) media box.
    pub(super) page_w: f32,
    pub(super) page_h: f32,
    /// The bleed allowance (#652), in points, added to every side of the media box so
    /// full-bleed art can extend past the trim. `0` for the screen profile and for a
    /// print export with no `[print] bleed`, leaving the media box exactly the trim.
    /// The trim rectangle is then inset by `bleed` within the media box, so all
    /// trim-relative content (text, folios, running heads) is offset by `bleed`.
    pub(super) bleed: f32,
    pub(super) margin_top: f32,
    pub(super) margin_bottom: f32,
    /// The binding-side and outer margins. Equal for the screen profile; mirrored
    /// by page parity for print (inside = the gutter side).
    pub(super) margin_inside: f32,
    pub(super) margin_outside: f32,
    /// Flip inside/outside by page parity (the binding gutter) — print only.
    pub(super) mirrored: bool,
    /// Draw verso/recto running heads — print only.
    pub(super) running_heads: bool,
    /// Begin each section on a recto (odd) page, inserting a blank verso if needed.
    pub(super) recto_starts: bool,
    /// Emit the file as PDF/X (#817): a grayscale `/OutputIntent`, `/GTS_PDFXVersion`
    /// in `/Info`, DeviceGray content. Set for the print interior, which is a genuine
    /// press artifact; the screen profile is an on-screen proof and stays plain.
    pub(super) pdf_x: bool,
    /// The embedded Type0 fonts (body faces + broad fallback) — both profiles embed
    /// (#492), so the PDF renders the full Unicode its fonts cover.
    pub(super) fonts: Fonts,
}

impl Layout {
    /// The screen proof (#463): US-Letter, uniform 1" margins; the body serif (or a
    /// configured font) and the fallback embedded (#492).
    pub(super) fn screen(style: &ExportStyle) -> Self {
        Self {
            page_w: 612.0,
            page_h: 792.0,
            bleed: 0.0,
            margin_top: 72.0,
            margin_bottom: 72.0,
            margin_inside: 72.0,
            margin_outside: 72.0,
            mirrored: false,
            running_heads: false,
            recto_starts: false,
            pdf_x: false,
            fonts: Fonts::new(style),
        }
    }

    /// The print interior (#464) at `trim_w`×`trim_h` points, with a `bleed` allowance
    /// (#652, points; `0` for none): generous top/bottom margins, a wider binding
    /// (inside) margin than outside for the gutter, with running heads, recto starts,
    /// and embedded fonts. Margins clear KDP's minimums for a typical book. Margins are
    /// measured from the trim; the bleed only enlarges the media box so full-bleed art
    /// can run past the trim edge.
    pub(super) fn print(trim_w: f32, trim_h: f32, bleed: f32, style: &ExportStyle) -> Self {
        Self {
            page_w: trim_w,
            page_h: trim_h,
            bleed,
            margin_top: 54.0,     // 0.75"
            margin_bottom: 54.0,  // 0.75"
            margin_inside: 63.0,  // 0.875" (0.5" outside + 0.375" gutter)
            margin_outside: 36.0, // 0.5"
            mirrored: true,
            running_heads: true,
            recto_starts: true,
            pdf_x: true,
            fonts: Fonts::new(style),
        }
    }

    /// The media-box width — the trim plus a bleed allowance on each side (#652). Equal
    /// to the trim when there's no bleed. This is the `/MediaBox` the page tree records.
    pub(super) const fn media_w(&self) -> f32 {
        2.0f32.mul_add(self.bleed, self.page_w)
    }

    /// The media-box height — the trim plus a bleed allowance on each side (#652).
    pub(super) const fn media_h(&self) -> f32 {
        2.0f32.mul_add(self.bleed, self.page_h)
    }

    /// The `/BleedBox` rectangle `[x0 y0 x1 y1]` — the full media box (trim + bleed on
    /// every side, #692). A print shop's bleed extends to this edge. Equal to the
    /// `/MediaBox` here, but emitted explicitly so the file states it.
    pub(super) const fn bleed_box(&self) -> (f32, f32, f32, f32) {
        (0.0, 0.0, self.media_w(), self.media_h())
    }

    /// The `/TrimBox` rectangle `[x0 y0 x1 y1]` — the finished page, inset within the
    /// media box by the bleed on every side (#692). This is the "cut here" box a print
    /// shop trims to; with no bleed it coincides with the media box.
    pub(super) fn trim_box(&self) -> (f32, f32, f32, f32) {
        (
            self.bleed,
            self.bleed,
            self.bleed + self.page_w,
            self.bleed + self.page_h,
        )
    }

    pub(super) fn text_top(&self) -> f32 {
        // The trim sits inset by `bleed` within the media box, so a trim-relative y is
        // offset up by the bleed of the bottom edge.
        self.bleed + self.page_h - self.margin_top
    }

    pub(super) fn text_bottom(&self) -> f32 {
        self.bleed + self.margin_bottom
    }

    pub(super) fn text_w(&self) -> f32 {
        self.page_w - self.margin_inside - self.margin_outside
    }

    /// A y-coordinate `frac` of the way up the trim page, mapped into media-box space
    /// (#652) — the trim is inset by `bleed`, so this offsets by the bottom bleed. Used
    /// for the fractional placements (title block, copyright, divider) that measure from
    /// the trim, not the media box. With no bleed this is just `frac * page_h`.
    pub(super) const fn trim_y(&self, frac: f32) -> f32 {
        self.page_h.mul_add(frac, self.bleed)
    }

    /// The y-baseline of the running head, in media-box space (#652): trim-relative,
    /// half a top margin down from the trim's top edge.
    pub(super) fn head_baseline(&self) -> f32 {
        self.margin_top.mul_add(-0.5, self.bleed + self.page_h)
    }

    /// The y-baseline of the folio, in media-box space (#652): trim-relative, centered
    /// in the bottom margin.
    pub(super) const fn folio_baseline(&self) -> f32 {
        self.margin_bottom.mul_add(0.5, self.bleed)
    }

    /// The left edge of the text column on page `idx`. Recto pages are even (page 1
    /// is the first, a recto); on a mirrored layout the binding (inside) margin sits
    /// on the left of a recto and the right of a verso. Offset by the bleed of the
    /// media box's left edge (#652), so the column stays trim-relative.
    pub(super) fn left(&self, idx: usize) -> f32 {
        let margin = if self.mirrored && idx % 2 == 1 {
            self.margin_outside
        } else {
            self.margin_inside
        };
        self.bleed + margin
    }

    /// The rendered width of `text` at `size` for the `style` face (body, or the
    /// heading family when `heading`), in points (#638).
    pub(super) fn measure(&self, text: &str, size: f32, style: FontStyle, heading: bool) -> f32 {
        text.chars()
            .map(|c| self.fonts.advance_em(c, style, heading))
            .sum::<f32>()
            / 1000.0
            * size
    }

    /// The rendered width of `text` at `size` in the monospace face, in points (#524).
    pub(super) fn measure_mono(&self, text: &str, size: f32) -> f32 {
        text.chars()
            .map(|c| self.fonts.advance_mono(c))
            .sum::<f32>()
            / 1000.0
            * size
    }

    /// Whether the fonts can render every character of `glyph`.
    pub(super) fn renders(&self, glyph: &str) -> bool {
        glyph.chars().all(|c| self.fonts.covers(c))
    }
}
