//! The print **wrap cover** (#819): a single full-bleed PDF holding the back cover,
//! the spine, and the front cover in one press-ready sheet — the other half of
//! "print-ready" from the interior (#464). Print-on-demand services (KDP, IngramSpark)
//! take the wrap as one flattened page whose width is `2×trim + spine + 2×bleed`; the
//! spine width is not a free parameter but a function of the interior page count times
//! the paper's per-page thickness (its *caliper*), so a thicker book needs a wider
//! spine to wrap around the block.
//!
//! Geometry (all in PDF points, 1/72"):
//!
//! ```text
//!  ┌───────────── media box = 2·trim_w + spine + 2·bleed ─────────────┐
//!  │ bleed │   back cover   │ spine │   front cover   │ bleed │        │  height =
//!  │       │    (trim_w)    │       │    (trim_w)     │       │        │  trim_h + 2·bleed
//!  └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! The front-cover art the author already supplies for the interior/EPUB
//! ([`ImageRole::Cover`]) is placed to cover the front panel (bleed included, aspect
//! preserved, overflow cropped) exactly as the interior cover page does — so the wrap
//! and the interior share one image. The back panel carries a reserved barcode/ISBN
//! safe zone (a light rectangle in the lower-outside corner the retailer's barcode
//! sits in) and thin spine fold guides drawn at the panel seams, so a designer can see
//! where the panels meet. No text — the wrap stays DeviceGray strokes + the one image,
//! which keeps it PDF/X-3 conformant like the interior (#817), with no fonts to embed.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::draw::num;
use super::font::{pdf_text_string, take, ObjMap};
use super::icc;
use super::images::{decode_images, emit_images};
use super::write::{flate_stream, serialize};
use super::ImageRole;
use crate::export::ExportStyle;

/// The default per-page paper thickness (caliper), in inches (#819). The spine width
/// is `page_count × caliper`. 0.0025"/page is a sane middle-of-the-road value for
/// uncoated book stock — close to KDP cream (≈0.0025") and a touch over KDP white
/// (≈0.00225") — so an author who never measures their stock gets a spine within a
/// hair of what the printer produces. Authors on a known stock override it.
pub const DEFAULT_CALIPER_INCHES: f32 = 0.0025;

/// A safety floor on the caliper so a zero/negative override can't collapse the spine
/// to nothing (which would misplace the front/back panels). One tenth the default.
const MIN_CALIPER_INCHES: f32 = 0.000_25;

/// The margin, in points, the barcode/ISBN reserved zone is inset from the trim edges —
/// a comfortable safe area a designer works within.
const SAFE_MARGIN: f32 = 18.0; // 0.25"

/// The barcode/ISBN reserved-zone footprint on the back cover, in points: the ~2"×1.2"
/// block a retail EAN-13 barcode occupies, sat in the lower-outside corner.
const BARCODE_W: f32 = 144.0; // 2.0"
const BARCODE_H: f32 = 86.4; // 1.2"

/// The geometry of a wrap cover (#819): the trim, bleed, spine, and the derived media
/// box. Pure measurement — [`Cover`] draws off it. All fields are in PDF points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WrapGeometry {
    /// One panel's finished (trimmed) width — the front and back are each this wide.
    pub trim_w: f32,
    /// The finished (trimmed) height, shared by both panels and the spine.
    pub trim_h: f32,
    /// The bleed allowance added on every outer edge of the media box.
    pub bleed: f32,
    /// The spine width, `page_count × caliper` — the flat between the two panels.
    pub spine: f32,
}

impl WrapGeometry {
    /// The wrap geometry for a `page_count`-page interior at `trim_w`×`trim_h` (points)
    /// with a `bleed` allowance (points) and a per-page `caliper_inches`. The spine is
    /// `page_count × caliper`, with the caliper floored at [`MIN_CALIPER_INCHES`] so a
    /// degenerate value can't collapse it; every dimension is clamped non-negative.
    pub fn new(
        trim_w: f32,
        trim_h: f32,
        bleed: f32,
        page_count: usize,
        caliper_inches: f32,
    ) -> Self {
        let caliper = caliper_inches.max(MIN_CALIPER_INCHES);
        // Spine = pages × caliper, converted inches → points. A 0-page book has no
        // spine, which is fine — the panels just meet in the middle.
        let spine = (page_count as f32 * caliper * 72.0).max(0.0);
        Self {
            trim_w: trim_w.max(0.0),
            trim_h: trim_h.max(0.0),
            bleed: bleed.max(0.0),
            spine,
        }
    }

    /// The media-box width: `2×trim + spine + 2×bleed` — both panels, the spine, and a
    /// bleed on each outer edge.
    pub const fn media_w(&self) -> f32 {
        2.0f32.mul_add(self.bleed, 2.0f32.mul_add(self.trim_w, self.spine))
    }

    /// The media-box height: `trim + 2×bleed`.
    pub const fn media_h(&self) -> f32 {
        2.0f32.mul_add(self.bleed, self.trim_h)
    }

    /// The x of the spine's left edge (where the back panel ends), in media space.
    fn spine_left(&self) -> f32 {
        self.bleed + self.trim_w
    }

    /// The x of the spine's right edge (where the front panel begins), in media space.
    fn spine_right(&self) -> f32 {
        self.bleed + self.trim_w + self.spine
    }
}

/// The wrap-cover builder (#819): lays the front/back/spine onto one page and
/// serializes it to a PDF/X-3 grayscale PDF, mirroring the interior writer's object
/// model (catalog, page tree, media/trim/bleed boxes, output intent, `/Info`).
pub struct Cover<'a> {
    geo: WrapGeometry,
    style: &'a ExportStyle,
    title: String,
    author: String,
}

impl<'a> Cover<'a> {
    /// A wrap cover for `geo`, drawing the `[style]` cover image (#490) onto the front
    /// panel. `title`/`author` feed the `/Info` dictionary (#692).
    pub fn new(geo: WrapGeometry, style: &'a ExportStyle, title: &str, author: &str) -> Self {
        Self {
            geo,
            style,
            title: title.to_string(),
            author: author.to_string(),
        }
    }

    /// The content stream for the single wrap page: the front-cover image (covering the
    /// front panel + bleed, cropped to it), the back-cover barcode/ISBN reserved zone,
    /// and thin spine fold guides. `cover_name` is the emitted image resource, or `None`
    /// when no cover image is configured/decodable — then the front panel is left blank.
    fn content(&self, cover_name: Option<&str>) -> String {
        let geo = &self.geo;
        let mut s = String::new();
        // Front panel: from the spine's right edge out to the media's right edge (so the
        // art bleeds off the outer trim), full media height (bleeds top and bottom too).
        if let Some(name) = cover_name {
            let panel_x = geo.spine_right();
            let panel_w = geo.media_w() - panel_x;
            let panel_h = geo.media_h();
            if let Some((w, h)) = decode_images(self.style)
                .into_iter()
                .find(|i| i.role == ImageRole::Cover)
                .map(|i| (i.width as f32, i.height as f32))
            {
                // Cover the panel (aspect preserved), center, and clip overflow to the
                // panel — the same fill the interior cover page uses, per panel here.
                let scale = (panel_w / w).max(panel_h / h);
                let (dw, dh) = (w * scale, h * scale);
                let x = panel_x + (panel_w - dw) / 2.0;
                let y = (panel_h - dh) / 2.0;
                let _ = writeln!(
                    s,
                    "q {} 0 {} {} re W n {} 0 0 {} {} {} cm /{name} Do Q",
                    num(panel_x),
                    num(panel_w),
                    num(panel_h),
                    num(dw),
                    num(dh),
                    num(x),
                    num(y)
                );
            }
        }
        // Back panel: a light barcode/ISBN reserved zone in the lower-outside (left)
        // corner, inset by the safe margin. Drawn as a thin gray outline — a designer
        // drops the retailer's barcode here; it's a guide, not printed content.
        let bx = geo.bleed + SAFE_MARGIN;
        let by = geo.bleed + SAFE_MARGIN;
        let _ = writeln!(
            s,
            "q 0.5 G 0.75 w {} {} {} {} re S Q",
            num(bx),
            num(by),
            num(BARCODE_W),
            num(BARCODE_H)
        );
        // Spine fold guides: thin dashed lines at each spine edge, full height, marking
        // where the cover wraps around the block. Only when there's a spine to fold.
        if geo.spine > 0.0 {
            let top = geo.media_h();
            let _ = writeln!(
                s,
                "q 0.6 G 0.5 w [3 3] 0 d {} 0 m {} {} l {} 0 m {} {} l S Q",
                num(geo.spine_left()),
                num(geo.spine_left()),
                num(top),
                num(geo.spine_right()),
                num(geo.spine_right()),
                num(top)
            );
        }
        s
    }

    /// Render the wrap cover to a PDF/X-3 grayscale PDF (#817/#819): one page at the
    /// wrap media box, the front-cover image XObject, the trim/bleed boxes a print shop
    /// reads, a grayscale output intent, and an `/Info` dictionary.
    pub fn render(&self) -> Vec<u8> {
        let geo = &self.geo;
        let mut objects: ObjMap = BTreeMap::new();
        // 1 catalog, 2 pages, 3 the single page object; the rest allocated from 4 up.
        let mut next = 4u32;
        // Emit the cover image (flattened onto white — PDF/X forbids transparency), if
        // one is configured. Only the Cover role rides the wrap; other ornaments don't.
        let cover_images: Vec<_> = decode_images(self.style)
            .into_iter()
            .filter(|i| i.role == ImageRole::Cover)
            .collect();
        let image_names = emit_images(&cover_images, true, &mut next, &mut objects);
        let cover_name = image_names.first().map(|(name, _)| name.clone());
        let content = self.content(cover_name.as_deref());
        let content_id = take(&mut next);
        objects.insert(content_id, flate_stream(content.as_bytes(), ""));

        // The single page: media box = full wrap; trim box = the finished sheet inset by
        // the bleed on every side; bleed box = the full media box (#692).
        let (mw, mh) = (geo.media_w(), geo.media_h());
        let xobjects = image_names
            .iter()
            .fold(String::new(), |mut acc, (name, id)| {
                let _ = write!(acc, "/{name} {id} 0 R ");
                acc
            });
        let resources = if xobjects.is_empty() {
            "/Resources << >>".to_string()
        } else {
            format!("/Resources << /XObject << {xobjects}>> >>")
        };
        // The trim box is the finished wrap inset by the bleed on every side: from
        // (bleed, bleed) to (media − bleed, media − bleed).
        let trim_box = format!(
            "/TrimBox [{} {} {} {}]",
            num(geo.bleed),
            num(geo.bleed),
            num(mw - geo.bleed),
            num(mh - geo.bleed)
        );
        let bleed_box = format!("/BleedBox [0 0 {} {}]", num(mw), num(mh));
        objects.insert(
            3,
            format!(
                "<< /Type /Page /Parent 2 0 R /Contents {content_id} 0 R {trim_box} {bleed_box} >>"
            )
            .into_bytes(),
        );
        objects.insert(
            2,
            format!(
                "<< /Type /Pages /Kids [ 3 0 R ] /Count 1 /MediaBox [0 0 {} {}] {resources} >>",
                num(mw),
                num(mh)
            )
            .into_bytes(),
        );

        // The PDF/X output intent (#817): a grayscale intent naming the bundled ICC
        // destination profile, referenced from the catalog — the wrap is a press
        // artifact like the interior, so it carries the same conformance.
        let icc_id = take(&mut next);
        objects.insert(icc_id, flate_stream(icc::GRAY_ICC, " /N 1"));
        let intent_id = take(&mut next);
        objects.insert(
            intent_id,
            format!(
                "<< /Type /OutputIntent /S /GTS_PDFX \
                 /OutputConditionIdentifier {ident} /OutputCondition {cond} \
                 /Info {ident} /DestOutputProfile {icc_id} 0 R >>",
                ident = pdf_text_string(icc::OUTPUT_CONDITION),
                cond = pdf_text_string(icc::OUTPUT_CONDITION),
            )
            .into_bytes(),
        );
        objects.insert(
            1,
            format!(
                "<< /Type /Catalog /Pages 2 0 R /PageMode /UseNone \
                 /OutputIntents [{intent_id} 0 R] >>"
            )
            .into_bytes(),
        );

        // `/Info`: Title/Author from the manifest, a Producer, and the PDF/X version
        // keys a conformance reader keys off (#817).
        let info_id = take(&mut next);
        let mut info = String::from("<<");
        if !self.title.is_empty() {
            let _ = write!(info, " /Title {}", pdf_text_string(&self.title));
        }
        if !self.author.is_empty() {
            let _ = write!(info, " /Author {}", pdf_text_string(&self.author));
        }
        let _ = write!(info, " /Producer {}", pdf_text_string("Quill"));
        let _ = write!(
            info,
            " /GTS_PDFXVersion {ver} /GTS_PDFXConformance {ver}",
            ver = pdf_text_string(icc::PDFX_VERSION)
        );
        info.push_str(" >>");
        objects.insert(info_id, info.into_bytes());
        serialize(&objects, Some(info_id))
    }
}

/// Render a print wrap cover to PDF bytes (#819) for the given [`WrapGeometry`]: the
/// front-cover art on the front panel, a reserved barcode/ISBN zone on the back, and a
/// spine already sized from the interior `page_count × caliper`. `title`/`author` feed
/// the `/Info` dictionary. Build the geometry with [`WrapGeometry::new`].
// Fallible signature kept for the uniform export path (like `to_pdf_print`).
#[allow(clippy::unnecessary_wraps)]
pub fn to_wrap_cover(
    geo: WrapGeometry,
    style: &ExportStyle,
    title: &str,
    author: &str,
) -> std::io::Result<Vec<u8>> {
    Ok(Cover::new(geo, style, title, author).render())
}

#[cfg(test)]
mod tests;
