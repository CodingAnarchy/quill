//! Image-ornament embedding for the PDF writer (#490): decode the configured
//! `[style]` raster ornaments and emit them as PDF image XObjects (uncompressed RGB
//! samples plus a grayscale `/SMask` for alpha). Placement lives in the layout
//! module; this is the decode + object emission.

use crate::export::{ExportStyle, ImageRole};

use super::font::{take, ObjMap};

/// A decoded `[style]` image ornament, ready to embed as a PDF image XObject: the
/// role it fills, a resource name, pixel dimensions, the RGB samples, and the alpha
/// channel when the source had transparency (→ a soft mask).
pub(super) struct PdfImage {
    pub(super) role: ImageRole,
    pub(super) name: String,
    pub(super) width: u32,
    pub(super) height: u32,
    rgb: Vec<u8>,
    alpha: Option<Vec<u8>>,
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
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        let mut alpha = Vec::with_capacity((width * height) as usize);
        let mut has_alpha = false;
        for px in rgba.pixels() {
            rgb.extend_from_slice(&[px[0], px[1], px[2]]);
            alpha.push(px[3]);
            has_alpha |= px[3] != 255;
        }
        images.push(PdfImage {
            role,
            name: name.to_string(),
            width,
            height,
            rgb,
            alpha: has_alpha.then_some(alpha),
        });
    }
    images
}

/// Emit each decoded image as a PDF image XObject (RGB samples, uncompressed) with a
/// grayscale `/SMask` when the source had alpha, returning the (resource name, object
/// id) pairs for the page-tree `/Resources /XObject` dictionary.
pub(super) fn emit_images(
    images: &[PdfImage],
    next: &mut u32,
    out: &mut ObjMap,
) -> Vec<(String, u32)> {
    let mut names = Vec::new();
    for img in images {
        let smask = img.alpha.as_ref().map(|alpha| {
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
            image_xobject(img.width, img.height, "DeviceRGB", &img.rgb, smask),
        );
        names.push((img.name.clone(), id));
    }
    names
}

/// One image XObject: an (uncompressed) sample stream of `color`/8-bit data, with an
/// optional soft mask reference.
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
    let mut v = format!(
        "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
         /ColorSpace /{color} /BitsPerComponent 8{smask} /Length {} >>\nstream\n",
        samples.len()
    )
    .into_bytes();
    v.extend_from_slice(samples);
    v.extend_from_slice(b"\nendstream");
    v
}
