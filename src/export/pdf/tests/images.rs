//! Image-ornament and cover tests for the PDF writer (#490/#582/#652/#696):
//! XObject embedding, full-page covers, stream compression, and the DPI floor.
use super::*;

#[test]
fn image_ornaments_embed_as_xobjects_and_are_placed() {
    let mut style = ExportStyle {
        scene_break_image: Some("art/break.png".into()),
        chapter_heading_image: Some("art/head.png".into()),
        title_image: Some("art/title.png".into()),
        ..Default::default()
    };
    // The scene break is transparent (→ a soft mask); the others opaque.
    style.resolve_images(|p| Ok(png_bytes(4, 4, if p.contains("break") { 128 } else { 255 })));
    let bytes = to_pdf(&doc(), &style).unwrap();
    assert_eq!(
        count(&bytes, "/Subtype /Image"),
        4,
        "3 images + 1 soft mask"
    );
    assert_eq!(
        count(&bytes, "/SMask"),
        1,
        "only the transparent image gets a mask"
    );
    assert!(has(&bytes, "/XObject <<"), "images registered in resources");
    let drawn = content(&bytes);
    for op in ["/ImSceneBreak Do", "/ImChapterHeading Do", "/ImTitle Do"] {
        assert!(drawn.contains(op), "{op} drawn");
    }
}
#[test]
fn a_cover_image_emits_a_full_page_page_zero() {
    // A configured cover (#582) embeds as an image XObject drawn full-page on the
    // front page, and the file is still a well-formed PDF.
    let mut style = ExportStyle {
        cover_image: Some("art/cover.png".into()),
        ..Default::default()
    };
    style.resolve_images(|_| Ok(png_bytes(600, 900, 255)));
    let bytes = to_pdf(&doc(), &style).unwrap();

    // Structural soundness (qpdf-style markers).
    assert!(bytes.starts_with(b"%PDF-1.7"), "header");
    assert!(bytes.ends_with(b"%%EOF\n"), "trailer marker");
    for marker in ["/Type /Catalog", "/Type /Pages", "xref", "trailer"] {
        assert!(has(&bytes, marker), "missing {marker}");
    }
    // The cover image is embedded and drawn.
    assert!(has(&bytes, "/Subtype /Image"), "cover image XObject");
    assert!(content(&bytes).contains("/ImCover Do"), "cover drawn");
    // One extra page over the no-cover baseline: the cover page leads it.
    let baseline = count(&to_pdf(&doc(), &plain()).unwrap(), "/Type /Page /Parent");
    let with_cover = count(&bytes, "/Type /Page /Parent");
    assert_eq!(with_cover, baseline + 1, "cover adds a front page");

    // Default style → no cover, no extra page.
    assert!(
        !has(&to_pdf(&doc(), &plain()).unwrap(), "/ImCover"),
        "no cover by default"
    );
}
#[test]
fn a_print_pdf_emits_the_cover_full_page() {
    // The print interior also leads with the full-page cover (#582).
    let mut style = ExportStyle {
        cover_image: Some("art/cover.png".into()),
        ..Default::default()
    };
    style.resolve_images(|_| Ok(png_bytes(600, 900, 255)));
    let (w, h) = trim_points("6x9").unwrap();
    let bytes = to_pdf_print(&doc(), w, h, 0.0, &style).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7") && bytes.ends_with(b"%%EOF\n"));
    assert!(
        content(&bytes).contains("/ImCover Do"),
        "cover drawn in the print PDF"
    );
    assert!(has(&bytes, "MediaBox [0 0 432 648]"), "6x9 trim");
}
#[test]
fn an_embedded_image_stream_is_flate_compressed_and_round_trips() {
    // The cover image XObject ships /Filter /FlateDecode (#696), not a raw sample
    // stream, and its bytes inflate back to exactly the w×h×3 RGB samples — so the
    // image renders identically, just smaller on disk.
    let (w, h) = (600u32, 900u32);
    let mut style = ExportStyle {
        cover_image: Some("art/cover.png".into()),
        ..Default::default()
    };
    style.resolve_images(|_| Ok(png_bytes(w, h, 255)));
    let bytes = to_pdf(&doc(), &style).unwrap();

    // The image XObject dict carries the Flate filter (opaque cover → no /SMask).
    assert!(
        has(&bytes, "/Subtype /Image"),
        "cover embeds as an image XObject"
    );
    // Find the compressed image stream among the FlateDecode streams and confirm it
    // inflates to the raw RGB sample count.
    let image_stream = flate_streams(&bytes)
        .into_iter()
        .find(|(d, _)| d.contains("/Subtype /Image"))
        .expect("the image XObject is a FlateDecode stream");
    assert_eq!(
        image_stream.1.len() as u32,
        w * h * 3,
        "inflates to exactly the w×h×3 RGB samples"
    );
}
#[test]
fn a_compressed_cover_shrinks_the_pdf() {
    // The FlateDecode cover stream (#696) is a fraction of the raw RGB it inflates to:
    // a solid image compresses hard, so the whole PDF is far smaller than the
    // uncompressed samples alone would be.
    let (w, h) = (600u32, 900u32);
    let mut style = ExportStyle {
        cover_image: Some("art/cover.png".into()),
        ..Default::default()
    };
    style.resolve_images(|_| Ok(png_bytes(w, h, 255)));
    let bytes = to_pdf(&doc(), &style).unwrap();
    let raw_samples = (w * h * 3) as usize;
    assert!(
        bytes.len() < raw_samples,
        "the whole PDF ({}) is smaller than the uncompressed cover samples ({raw_samples})",
        bytes.len()
    );
}
#[test]
fn a_low_dpi_cover_warns_and_a_high_dpi_one_does_not() {
    // Effective DPI = pixel_width ÷ media-box inches (#696). Screen is Letter (8.5"
    // wide): 600px → ~71 DPI (well under 300) warns; 3000px → ~353 DPI is clean.
    let cover = |px: u32| {
        let mut style = ExportStyle {
            cover_image: Some("art/cover.png".into()),
            ..Default::default()
        };
        style.resolve_images(move |_| Ok(png_bytes(px, (px * 3) / 2, 255)));
        style
    };
    let low = pdf_dpi_warnings(&cover(600), SCREEN_MEDIA_W);
    assert!(
        low.iter().any(|w| w.contains("DPI")),
        "a low-res cover warns: {low:?}"
    );
    assert!(
        pdf_dpi_warnings(&cover(3000), SCREEN_MEDIA_W).is_empty(),
        "a high-res cover is clean"
    );
    // No cover at all → nothing to warn about.
    assert!(
        pdf_dpi_warnings(&plain(), SCREEN_MEDIA_W).is_empty(),
        "no cover, no DPI warning"
    );
}
#[test]
fn cover_dpi_is_measured_against_the_full_media_box() {
    // A 6x9 print cover with bleed is scaled to the *media box* (trim + bleed), so the
    // DPI floor is checked against the wider box, matching what `cover_page` fills.
    let mut style = ExportStyle {
        cover_image: Some("art/cover.png".into()),
        ..Default::default()
    };
    // 1800px wide over a 6.25"-ish media box (6" trim + 0.125" bleed each side) is
    // ~288 DPI — just under the 300 floor, so it warns.
    style.resolve_images(|_| Ok(png_bytes(1800, 2700, 255)));
    let (w, _h) = trim_points("6x9").unwrap();
    let bleed = 0.125 * 72.0;
    let media_w = w + 2.0 * bleed;
    assert!(
        pdf_dpi_warnings(&style, media_w)
            .iter()
            .any(|s| s.contains("DPI")),
        "just-under-300 DPI over the media box warns"
    );
}
