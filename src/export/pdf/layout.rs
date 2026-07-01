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
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct PrintConfig {
    /// The book trim size, e.g. `"6x9"` (see [`trim_points`]). Absent → 6×9.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim: Option<String>,
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
    pub(super) page_w: f32,
    pub(super) page_h: f32,
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
    /// The embedded Type0 fonts (body faces + broad fallback) — both profiles embed
    /// (#492), so the PDF renders the full Unicode its fonts cover.
    pub(super) fonts: Fonts,
}

impl Layout {
    /// The screen proof (#463): US-Letter, uniform 1" margins; the body serif (or a
    /// configured font) and the fallback embedded (#492).
    pub(super) fn screen(style: &ExportStyle) -> Self {
        Layout {
            page_w: 612.0,
            page_h: 792.0,
            margin_top: 72.0,
            margin_bottom: 72.0,
            margin_inside: 72.0,
            margin_outside: 72.0,
            mirrored: false,
            running_heads: false,
            recto_starts: false,
            fonts: Fonts::new(style),
        }
    }

    /// The print interior (#464) at `trim_w`×`trim_h` points: generous top/bottom
    /// margins, a wider binding (inside) margin than outside for the gutter, with
    /// running heads, recto starts, and embedded fonts. Margins clear KDP's minimums
    /// for a typical book.
    pub(super) fn print(trim_w: f32, trim_h: f32, style: &ExportStyle) -> Self {
        Layout {
            page_w: trim_w,
            page_h: trim_h,
            margin_top: 54.0,     // 0.75"
            margin_bottom: 54.0,  // 0.75"
            margin_inside: 63.0,  // 0.875" (0.5" outside + 0.375" gutter)
            margin_outside: 36.0, // 0.5"
            mirrored: true,
            running_heads: true,
            recto_starts: true,
            fonts: Fonts::new(style),
        }
    }

    pub(super) fn text_top(&self) -> f32 {
        self.page_h - self.margin_top
    }

    pub(super) fn text_bottom(&self) -> f32 {
        self.margin_bottom
    }

    pub(super) fn text_w(&self) -> f32 {
        self.page_w - self.margin_inside - self.margin_outside
    }

    /// The left edge of the text column on page `idx`. Recto pages are even (page 1
    /// is the first, a recto); on a mirrored layout the binding (inside) margin sits
    /// on the left of a recto and the right of a verso.
    pub(super) fn left(&self, idx: usize) -> f32 {
        if self.mirrored && idx % 2 == 1 {
            self.margin_outside
        } else {
            self.margin_inside
        }
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
