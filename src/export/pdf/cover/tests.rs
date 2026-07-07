//! Wrap-cover tests (#819): the spine/media-box geometry, the emitted PDF structure
//! (media/trim/bleed boxes, output intent, `/Info`, the front-cover image), and a
//! `qpdf --check` on the rendered bytes when qpdf is installed.

use std::io::{Read, Write};
use std::process::Command as Proc;

use flate2::read::ZlibDecoder;

use super::{to_wrap_cover, Cover, WrapGeometry, DEFAULT_CALIPER_INCHES};
use crate::export::ExportStyle;

/// 6×9 in points.
const TRIM_W: f32 = 6.0 * 72.0;
const TRIM_H: f32 = 9.0 * 72.0;
/// 0.125" bleed in points.
const BLEED: f32 = 0.125 * 72.0;

/// A style carrying a decodable `w`×`h` cover image.
fn style_with_cover(w: u32, h: u32) -> ExportStyle {
    let mut style = ExportStyle {
        cover_image: Some("art/cover.png".into()),
        ..Default::default()
    };
    style.resolve_images(move |_| Ok(png_bytes(w, h)));
    style
}

/// A tiny solid `w`×`h` PNG the writer can decode.
fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 40, 40, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("png encodes");
    bytes.into_inner()
}

fn has(bytes: &[u8], needle: &str) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
}
fn count(bytes: &[u8], needle: &str) -> usize {
    bytes
        .windows(needle.len())
        .filter(|w| *w == needle.as_bytes())
        .count()
}

/// The inflated content of the single page stream (the non-image FlateDecode stream).
fn content(bytes: &[u8]) -> String {
    let open = b"stream\n";
    let close = b"\nendstream";
    let mut out = String::new();
    let mut i = 0;
    while let Some(rel) = bytes[i..].windows(open.len()).position(|w| w == open) {
        let dict_end = i + rel;
        let payload = dict_end + open.len();
        let Some(end_rel) = bytes[payload..]
            .windows(close.len())
            .position(|w| w == close)
        else {
            break;
        };
        let dict_start = bytes[..dict_end]
            .windows(2)
            .rposition(|w| w == b"<<")
            .unwrap_or(dict_end);
        let dict = String::from_utf8_lossy(&bytes[dict_start..dict_end]);
        // The image XObject carries /Subtype /Image; skip it so we read page content.
        if dict.contains("/Filter /FlateDecode") && !dict.contains("/Subtype /Image") {
            let mut inflated = Vec::new();
            if ZlibDecoder::new(&bytes[payload..payload + end_rel])
                .read_to_end(&mut inflated)
                .is_ok()
            {
                out.push_str(&String::from_utf8_lossy(&inflated));
            }
        }
        i = payload + end_rel + close.len();
    }
    out
}

#[test]
#[allow(clippy::float_cmp)]
fn spine_scales_with_page_count() {
    // Spine = page_count × caliper (inches → points). Doubling the pages doubles it.
    let thin = WrapGeometry::new(TRIM_W, TRIM_H, BLEED, 100, DEFAULT_CALIPER_INCHES);
    let thick = WrapGeometry::new(TRIM_W, TRIM_H, BLEED, 200, DEFAULT_CALIPER_INCHES);
    assert_eq!(thin.spine, 100.0 * DEFAULT_CALIPER_INCHES * 72.0);
    assert_eq!(
        thick.spine,
        2.0 * thin.spine,
        "spine scales with page count"
    );
    // A custom caliper flows straight through.
    let custom = WrapGeometry::new(TRIM_W, TRIM_H, BLEED, 300, 0.004);
    assert_eq!(custom.spine, 300.0 * 0.004 * 72.0);
}

#[test]
#[allow(clippy::float_cmp)]
fn media_box_is_two_trims_plus_spine_plus_bleed() {
    let pages = 250;
    let geo = WrapGeometry::new(TRIM_W, TRIM_H, BLEED, pages, DEFAULT_CALIPER_INCHES);
    let spine = pages as f32 * DEFAULT_CALIPER_INCHES * 72.0;
    // width = 2·trim + spine + 2·bleed ; height = trim + 2·bleed.
    let want_w = 2.0f32.mul_add(BLEED, 2.0f32.mul_add(TRIM_W, spine));
    let want_h = 2.0f32.mul_add(BLEED, TRIM_H);
    assert_eq!(geo.media_w(), want_w);
    assert_eq!(geo.media_h(), want_h);
}

#[test]
#[allow(clippy::float_cmp)]
fn a_zero_page_book_has_no_spine_and_a_degenerate_caliper_is_floored() {
    // No pages → the panels meet with no spine.
    let none = WrapGeometry::new(TRIM_W, TRIM_H, BLEED, 0, DEFAULT_CALIPER_INCHES);
    assert_eq!(none.spine, 0.0);
    assert_eq!(none.media_w(), 2.0f32.mul_add(TRIM_W, 2.0 * BLEED));
    // A zero/negative caliper can't collapse a real book's spine — it's floored.
    let floored = WrapGeometry::new(TRIM_W, TRIM_H, BLEED, 300, 0.0);
    assert!(floored.spine > 0.0, "caliper floor keeps a spine");
}

#[test]
fn the_media_box_in_the_pdf_matches_the_geometry() {
    let pages = 180;
    let geo = WrapGeometry::new(TRIM_W, TRIM_H, BLEED, pages, DEFAULT_CALIPER_INCHES);
    let bytes = to_wrap_cover(geo, &style_with_cover(1800, 2700), "The Book", "An Author").unwrap();
    // The emitted /MediaBox states the wrap dimensions (num() drops trailing .00).
    let media = format!(
        "/MediaBox [0 0 {} {}]",
        super::super::draw::num(geo.media_w()),
        super::super::draw::num(geo.media_h())
    );
    assert!(
        has(&bytes, &media),
        "media box states the wrap size: {media}"
    );
}

#[test]
fn the_pdf_is_well_formed_pdf_x_with_the_cover_on_the_front() {
    let geo = WrapGeometry::new(TRIM_W, TRIM_H, BLEED, 200, DEFAULT_CALIPER_INCHES);
    let bytes = to_wrap_cover(geo, &style_with_cover(1800, 2700), "Title", "Author").unwrap();
    // Structural soundness (qpdf-style markers).
    assert!(bytes.starts_with(b"%PDF-1.7"), "header");
    assert!(bytes.ends_with(b"%%EOF\n"), "trailer marker");
    for marker in [
        "/Type /Catalog",
        "/Type /Pages",
        "/Type /Page ",
        "xref",
        "trailer",
    ] {
        assert!(has(&bytes, marker), "missing {marker}");
    }
    // One page, the trim/bleed boxes a print shop reads, and the front-cover image.
    assert_eq!(
        count(&bytes, "/Type /Page /Parent"),
        1,
        "a single wrap page"
    );
    assert!(has(&bytes, "/TrimBox"), "trim box");
    assert!(has(&bytes, "/BleedBox"), "bleed box");
    assert!(has(&bytes, "/Subtype /Image"), "cover image XObject");
    let drawn = content(&bytes);
    assert!(
        drawn.contains("/ImCover Do"),
        "cover drawn on the front panel"
    );
    // The reserved barcode/ISBN zone (a stroked rectangle) and the spine fold guides.
    assert!(drawn.contains(" re S"), "barcode reserved zone stroked");
    assert!(drawn.contains(" l S"), "spine fold guides drawn");
    // PDF/X-3 conformance, like the interior (#817): a grayscale output intent + keys.
    assert!(has(&bytes, "/GTS_PDFX"), "PDF/X output intent");
    assert!(
        has(&bytes, "/GTS_PDFXVersion"),
        "PDF/X version key in /Info"
    );
    assert!(has(&bytes, "/OutputIntents"), "catalog names the intent");
    // No uncalibrated RGB in the samples: the cover flattened to DeviceGray.
    assert!(has(&bytes, "/DeviceGray"), "grayscale samples");
    assert!(!has(&bytes, "/DeviceRGB"), "no uncalibrated RGB");
}

#[test]
fn no_cover_image_leaves_the_front_blank_but_still_valid() {
    // Without a configured cover, the wrap still renders (guides only, no image).
    let geo = WrapGeometry::new(TRIM_W, TRIM_H, BLEED, 120, DEFAULT_CALIPER_INCHES);
    let bytes = to_wrap_cover(geo, &ExportStyle::default(), "Untitled", "").unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7"), "still a PDF");
    assert!(!has(&bytes, "/Subtype /Image"), "no cover image emitted");
    assert!(!has(&bytes, "/ImCover"), "no cover reference");
    assert_eq!(count(&bytes, "/Type /Page /Parent"), 1, "one page");
    assert!(has(&bytes, "/TrimBox"), "still carries a trim box");
}

#[test]
fn the_front_panel_art_sits_right_of_the_spine() {
    // The front-cover art is placed on the right panel — its clip rectangle begins at
    // the spine's right edge, never at x=0 (which would be the back panel).
    let pages = 300;
    let geo = WrapGeometry::new(TRIM_W, TRIM_H, BLEED, pages, DEFAULT_CALIPER_INCHES);
    let style = style_with_cover(1800, 2700);
    let cover = Cover::new(geo, &style, "T", "A");
    let stream = cover.content(Some("ImCover"));
    let front_x = super::super::draw::num(geo.bleed + geo.trim_w + geo.spine);
    // The image's clip re starts at the spine's right edge.
    assert!(
        stream.contains(&format!("q {front_x} 0 ")),
        "front art clipped to the right (front) panel starting at {front_x}"
    );
}

/// `qpdf --check` on a rendered wrap cover — the acceptance gate (#819). Skipped when
/// qpdf isn't installed so the suite stays hermetic on machines without it.
#[test]
fn qpdf_check_passes_when_available() {
    let Ok(which) = Proc::new("sh").arg("-c").arg("command -v qpdf").output() else {
        return;
    };
    if !which.status.success() {
        return; // qpdf not installed — skip.
    }
    let geo = WrapGeometry::new(TRIM_W, TRIM_H, BLEED, 222, DEFAULT_CALIPER_INCHES);
    let bytes = to_wrap_cover(geo, &style_with_cover(1800, 2700), "Title", "Author").unwrap();
    let dir = std::env::temp_dir().join(format!("quill-wrap-{}.pdf", std::process::id()));
    {
        let mut f = std::fs::File::create(&dir).expect("temp file");
        f.write_all(&bytes).expect("write pdf");
    }
    let out = Proc::new("qpdf")
        .arg("--check")
        .arg(&dir)
        .output()
        .expect("run qpdf");
    let _ = std::fs::remove_file(&dir);
    assert!(
        out.status.success(),
        "qpdf --check failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
