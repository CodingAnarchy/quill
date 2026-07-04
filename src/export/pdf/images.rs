//! Image-ornament embedding for the PDF writer (#490): decode the configured
//! `[style]` raster ornaments and emit them as PDF image XObjects (FlateDecode-
//! compressed grayscale samples plus a grayscale `/SMask` for alpha, #696/#817).
//! Placement lives in the layout module; this is the decode + object emission.
//!
//! Samples are DeviceGray, not DeviceRGB (#817): the print interior is PDF/X-3
//! conformant (calibrated grayscale + a bundled ICC output intent), which forbids
//! uncalibrated DeviceRGB. A novel interior is grayscale anyway — the ornaments the
//! author supplies (scene breaks, chapter marks) render the same in gray as they did
//! in RGB once flattened to luminance.

use crate::export::{ExportStyle, ImageRole};

use super::font::{take, ObjMap};
use super::write::flate_stream;

/// A decoded `[style]` image ornament, ready to embed as a PDF image XObject: the
/// role it fills, a resource name, pixel dimensions, the grayscale samples, and the
/// alpha channel when the source had transparency (→ a soft mask).
pub(super) struct PdfImage {
    pub(super) role: ImageRole,
    pub(super) name: String,
    pub(super) width: u32,
    pub(super) height: u32,
    /// One 8-bit luminance sample per pixel (DeviceGray, #817).
    gray: Vec<u8>,
    alpha: Option<Vec<u8>>,
}

/// The Rec. 601 luma of an sRGB pixel, rounded to an 8-bit DeviceGray sample (#817).
/// A novel interior prints grayscale, so ornaments are flattened to luminance; the
/// standard 0.299/0.587/0.114 weights match how a mono press separates the color.
fn luma(r: u8, g: u8, b: u8) -> u8 {
    let y = 0.114f32.mul_add(
        f32::from(b),
        0.299f32.mul_add(f32::from(r), 0.587 * f32::from(g)),
    );
    // Values land in [0, 255] by construction; round to the nearest sample.
    y.round().clamp(0.0, 255.0) as u8
}

/// Decode the configured image ornaments to RGB(+alpha) samples for embedding.
/// Anything the raster codec can't read (SVG, a corrupt file) is skipped — those are
/// reported by `pdf_style_warnings`.
pub(super) fn decode_images(style: &ExportStyle) -> Vec<PdfImage> {
    let mut images = Vec::new();
    let roles = [
        (ImageRole::SceneBreak, "ImSceneBreak"),
        (ImageRole::ChapterHeading, "ImChapterHeading"),
        (ImageRole::Title, "ImTitle"),
        (ImageRole::Cover, "ImCover"),
    ];
    for (role, name) in roles {
        let Some(embedded) = style.image(role) else {
            continue;
        };
        let Ok(decoded) = image::load_from_memory(&embedded.bytes) else {
            continue;
        };
        let rgba = decoded.to_rgba8();
        let (width, height) = rgba.dimensions();
        let mut gray = Vec::with_capacity((width * height) as usize);
        let mut alpha = Vec::with_capacity((width * height) as usize);
        let mut has_alpha = false;
        for px in rgba.pixels() {
            gray.push(luma(px[0], px[1], px[2]));
            alpha.push(px[3]);
            has_alpha |= px[3] != 255;
        }
        images.push(PdfImage {
            role,
            name: name.to_string(),
            width,
            height,
            gray,
            alpha: has_alpha.then_some(alpha),
        });
    }
    images
}

/// Emit each decoded image as a PDF image XObject (FlateDecode-compressed DeviceGray
/// samples, #696/#817), returning the (resource name, object id) pairs for the
/// page-tree `/Resources /XObject` dictionary. DeviceGray keeps the print interior
/// PDF/X-3 conformant (no uncalibrated RGB).
///
/// When `flatten` (the PDF/X print interior, #817), a transparent source is composited
/// over white — the paper — and emitted opaque, with no `/SMask`: PDF/X-3:2003 (a PDF
/// 1.3 base) forbids soft-mask transparency, and on a white page the composite renders
/// identically. The screen proof passes `flatten = false` and keeps the `/SMask`, which
/// composites correctly against any background a viewer shows.
pub(super) fn emit_images(
    images: &[PdfImage],
    flatten: bool,
    next: &mut u32,
    out: &mut ObjMap,
) -> Vec<(String, u32)> {
    let mut names = Vec::new();
    for img in images {
        // Flatten alpha onto white so the print interior carries no transparency; the
        // screen proof keeps the samples as-is and rides an /SMask for the alpha.
        let (gray, alpha) = match (&img.alpha, flatten) {
            (Some(alpha), true) => (flatten_on_white(&img.gray, alpha), None),
            (alpha, _) => (img.gray.clone(), alpha.as_ref()),
        };
        let smask = alpha.map(|alpha| {
            let id = take(next);
            out.insert(
                id,
                image_xobject(img.width, img.height, "DeviceGray", alpha, None),
            );
            id
        });
        let id = take(next);
        out.insert(
            id,
            image_xobject(img.width, img.height, "DeviceGray", &gray, smask),
        );
        names.push((img.name.clone(), id));
    }
    names
}

/// Composite grayscale samples over a white background by their alpha, yielding opaque
/// samples (#817): `out = gray·α + 255·(1−α)`. Used to strip transparency from the
/// PDF/X print interior — on white paper the result is what the `/SMask` would show.
fn flatten_on_white(gray: &[u8], alpha: &[u8]) -> Vec<u8> {
    gray.iter()
        .zip(alpha)
        .map(|(&g, &a)| {
            let a = f32::from(a) / 255.0;
            f32::from(g)
                .mul_add(a, 255.0 * (1.0 - a))
                .round()
                .clamp(0.0, 255.0) as u8
        })
        .collect()
}

/// One image XObject: a FlateDecode-compressed sample stream of `color`/8-bit data
/// (#696), with an optional soft mask reference. The XObject dictionary entries ride
/// as the `extra` dict fields of [`flate_stream`], which supplies `/Filter
/// /FlateDecode /Length` and the zlib payload; the samples round-trip via inflate so
/// the image renders identically, just smaller on disk.
fn image_xobject(
    width: u32,
    height: u32,
    color: &str,
    samples: &[u8],
    smask: Option<u32>,
) -> Vec<u8> {
    let smask = smask
        .map(|id| format!(" /SMask {id} 0 R"))
        .unwrap_or_default();
    let extra = format!(
        " /Type /XObject /Subtype /Image /Width {width} /Height {height} \
         /ColorSpace /{color} /BitsPerComponent 8{smask}"
    );
    flate_stream(samples, &extra)
}
