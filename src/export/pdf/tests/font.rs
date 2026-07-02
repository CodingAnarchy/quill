//! Font tests for the PDF writer: Type0 embedding, glyph coverage/routing,
//! synthesized italics, subsetting, and stream compression (#492/#638/#662).
use super::super::font::Fonts;
use super::*;
use crate::export::{EmbeddedFont, FontRole};

#[test]
fn text_is_drawn_with_embedded_type0_fonts() {
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    for marker in [
        "/Subtype /Type0",
        "/Encoding /Identity-H",
        "/Subtype /CIDFontType2",
        "/FontFile2",
        "/ToUnicode",
        "/BaseFont /LiberationSerif",
    ] {
        assert!(has(&bytes, marker), "missing {marker}");
    }
    assert!(!has(&bytes, "/BaseFont /Times"), "no standard-14 fonts");
    assert!(
        has(&bytes, "MediaBox [0 0 612 792]"),
        "US-Letter screen page"
    );
}
#[test]
fn emphasis_routes_to_the_bold_and_italic_faces() {
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    assert!(
        has(&bytes, "/BaseFont /LiberationSerif-Italic"),
        "italic used"
    );
    assert!(has(&bytes, "/BaseFont /LiberationSerif-Bold"), "bold used");
}
#[test]
fn fonts_route_glyphs_by_coverage() {
    let fonts = Fonts::new(&plain());
    let reg = FontStyle::REGULAR;
    // ASCII + Latin/Cyrillic/Greek the serif covers → the regular body face.
    for c in ['A', 'é', 'д', 'α'] {
        assert_eq!(
            fonts.resolve(c, reg, false).map(|(n, _)| n),
            Some("F1"),
            "{c} in body"
        );
    }
    // Decorative ornaments the serif lacks → the broad fallback.
    assert_eq!(fonts.resolve('❦', reg, false).map(|(n, _)| n), Some("Ffb"));
    assert_eq!(fonts.resolve('⚜', reg, false).map(|(n, _)| n), Some("Ffb"));
    assert!(fonts.covers('❦') && fonts.covers('д'));
    // CJK isn't in either bundled font (needs an author-supplied CJK font).
    assert_eq!(fonts.resolve('中', reg, false), None, "CJK uncovered");
    assert!(!fonts.covers('中'));
}
#[test]
fn ornaments_render_via_the_fallback_font_without_warning() {
    // The #492 win: an ornament #489 had to skip now renders, via the fallback.
    let style = ExportStyle {
        scene_break: Some("❦".into()),
        title_ornament: Some("⚜".into()),
        ..Default::default()
    };
    let bytes = to_pdf(&doc(), &style).unwrap();
    assert!(
        has(&bytes, "/BaseFont /DejaVuSans"),
        "fallback embedded for the ornaments"
    );
    assert!(
        pdf_style_warnings(&style).is_empty(),
        "no skipped-glyph warning: {:?}",
        pdf_style_warnings(&style)
    );
}
#[test]
fn a_preset_renders_its_ornaments_in_the_pdf() {
    let mut style = ExportStyle {
        preset: Some("classic".into()),
        ..Default::default()
    };
    style.apply_preset();
    let bytes = to_pdf(&doc(), &style).unwrap();
    assert!(
        has(&bytes, "/BaseFont /DejaVuSans"),
        "classic ornament embedded"
    );
    assert!(
        pdf_style_warnings(&style).is_empty(),
        "classic is fully renderable"
    );
}
#[test]
fn a_plain_pdf_embeds_no_fallback_and_no_images() {
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    assert!(
        !has(&bytes, "DejaVuSans"),
        "no fallback when every glyph is in the serif"
    );
    assert!(!has(&bytes, "/Subtype /Image"), "no images by default");
    assert!(!has(&bytes, "/XObject"), "no XObject resources by default");
}
#[test]
fn heading_alignment_maps_from_style() {
    let bits = |a: Option<&str>| {
        StyleBits::from_style(&ExportStyle {
            heading_align: a.map(str::to_string),
            ..Default::default()
        })
        .heading_align
    };
    assert_eq!(bits(None), Align::Center, "default centered");
    assert_eq!(bits(Some("left")), Align::Left);
    assert_eq!(bits(Some("right")), Align::RightInset(0.0));
    assert_eq!(
        bits(Some("justify")),
        Align::Left,
        "headings aren't justified"
    );
}
#[test]
fn a_configured_body_font_replaces_the_serif() {
    let mut style = ExportStyle {
        body_font: Some("body.ttf".into()),
        ..Default::default()
    };
    let font = include_bytes!("../../../../assets/fonts/LiberationSerif-Italic.ttf");
    let warnings = style.resolve_fonts(|_| Ok(font.to_vec()));
    assert!(warnings.is_empty(), "font resolves: {warnings:?}");
    let bytes = to_pdf(&doc(), &style).unwrap();
    // The configured font takes the regular (body) slot; the default serif regular
    // is gone, but the bundled bold/italic faces remain.
    assert!(
        !has(&bytes, "/BaseFont /LiberationSerif /"),
        "default serif regular replaced"
    );
    assert!(
        has(&bytes, "/BaseFont /LiberationSerif-Bold"),
        "bundled bold kept"
    );
}
/// A resolved [`EmbeddedFont`] for `role`/`italic` with `family` and `bytes` — the
/// shape `resolve_fonts` fills, built directly so a face-routing test needn't touch
/// the filesystem (#638, hermetic per #531).
fn embedded(role: FontRole, italic: bool, family: &str, bytes: &[u8]) -> EmbeddedFont {
    EmbeddedFont {
        role,
        family: family.into(),
        italic,
        href: format!(
            "fonts/{}{}.ttf",
            if role == FontRole::Body {
                "body"
            } else {
                "heading"
            },
            if italic { "-italic" } else { "" }
        ),
        media_type: "font/ttf",
        bytes: bytes.to_vec(),
    }
}
#[test]
fn all_four_body_styles_and_the_heading_resolve_to_the_chosen_families() {
    // The #638 fix, end to end through the font plan: a body family with a real
    // italic sibling and a *distinct* heading family. Each of the four body styles
    // resolves to a body face (never the bundled Liberation fallback, never the
    // heading font), true italics use the italic file, and the heading is its own
    // face — not conflated with body-bold.
    let body_up = include_bytes!("../../../../assets/fonts/LiberationSerif-Regular.ttf");
    let body_it = include_bytes!("../../../../assets/fonts/LiberationSerif-Italic.ttf");
    let head = include_bytes!("../../../../assets/fonts/DejaVuSans.ttf");
    let style = ExportStyle {
        fonts: vec![
            embedded(FontRole::Body, false, "BookBody", body_up),
            embedded(FontRole::Body, true, "BookBody", body_it),
            embedded(FontRole::Heading, false, "DisplayHead", head),
        ],
        ..Default::default()
    };
    let fonts = Fonts::new(&style);

    // The four body styles land in the four distinct body slots F1..F4.
    let name = |s, heading| fonts.resolve('a', s, heading).map(|(n, _)| n);
    assert_eq!(name(FontStyle::REGULAR, false), Some("F1"));
    assert_eq!(name(FontStyle::BOLD, false), Some("F2"));
    assert_eq!(name(FontStyle::ITALIC, false), Some("F3"));
    assert_eq!(name(FontStyle::BOLD_ITALIC, false), Some("F4"));
    // The heading resolves to its own faces, distinct from the body slots.
    assert_eq!(name(FontStyle::BOLD, true), Some("Fh2"));
    assert_eq!(name(FontStyle::REGULAR, true), Some("Fh1"));

    // Italic body styles use a real italic file, so no synthetic slant is applied.
    assert!(
        !fonts.is_synthetic_oblique(FontStyle::ITALIC, false),
        "true italic, not synthesized"
    );
    assert!(!fonts.is_synthetic_oblique(FontStyle::BOLD_ITALIC, false));

    // And in the emitted PDF the body italic is the chosen family, the heading is
    // the display family, and no Liberation face leaks into a body slot.
    let bytes = to_pdf(&doc(), &style).unwrap();
    assert!(has(&bytes, "/BaseFont /BookBody"), "body family embedded");
    assert!(
        has(&bytes, "/BaseFont /DisplayHead"),
        "heading family embedded and distinct from body"
    );
    assert!(
        !has(&bytes, "/BaseFont /LiberationSerif-Italic"),
        "no bundled italic fallback in body — the chosen italic is used"
    );
}
#[test]
fn an_italic_style_without_a_true_italic_file_is_synthesized() {
    // A body font with no `-italic` sibling (#638): italic styles fall back to the
    // upright file, drawn with a synthetic slant — never the bundled Liberation
    // italic, never a different family.
    let body_up = include_bytes!("../../../../assets/fonts/LiberationSerif-Regular.ttf");
    let style = ExportStyle {
        fonts: vec![embedded(FontRole::Body, false, "OnlyUpright", body_up)],
        ..Default::default()
    };
    let fonts = Fonts::new(&style);
    // Every style resolves to a body slot (the upright file backs the italic slots).
    for s in [
        FontStyle::REGULAR,
        FontStyle::BOLD,
        FontStyle::ITALIC,
        FontStyle::BOLD_ITALIC,
    ] {
        let n = fonts.resolve('a', s, false).map(|(n, _)| n);
        assert!(
            matches!(n, Some("F1" | "F2" | "F3" | "F4")),
            "{s:?} → a body slot, got {n:?}"
        );
    }
    // The italic slots are synthetically obliqued; upright ones are not.
    assert!(fonts.is_synthetic_oblique(FontStyle::ITALIC, false));
    assert!(fonts.is_synthetic_oblique(FontStyle::BOLD_ITALIC, false));
    assert!(!fonts.is_synthetic_oblique(FontStyle::REGULAR, false));

    // The PDF carries the chosen family and a sheared text matrix, and no bundled
    // italic fallback.
    let bytes = to_pdf(&doc(), &style).unwrap();
    assert!(has(&bytes, "/BaseFont /OnlyUpright"), "chosen family used");
    assert!(
        !has(&bytes, "/BaseFont /LiberationSerif-Italic"),
        "no bundled italic — the slope is synthesized on the chosen family"
    );
    // The oblique shear appears in a text matrix (`1 0 0.21 1 …  Tm`).
    assert!(
        content(&bytes).contains("1 0 0.21 1"),
        "synthetic oblique shear in a Tm"
    );
}
#[test]
fn embedded_fonts_are_subset_to_keep_pdfs_small() {
    // Each bundled face is ~390 KB and DejaVu ~750 KB; a short doc uses a few dozen
    // glyphs, so subsetting must keep the whole PDF far under a single whole font.
    let style = ExportStyle {
        scene_break: Some("❦".into()), // pulls in (and subsets) the fallback too
        ..Default::default()
    };
    let bytes = to_pdf(&doc(), &style).unwrap();
    assert!(has(&bytes, "/FontFile2"), "fonts still embedded");
    assert!(
        bytes.len() < 200_000,
        "subset keeps the PDF small, got {} bytes",
        bytes.len()
    );
}
#[test]
fn content_and_font_streams_are_flate_compressed() {
    // Both the page content streams and the embedded /FontFile2 programs ship with
    // /Filter /FlateDecode (#662); nothing is left as a raw uncompressed stream.
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    assert!(
        has(&bytes, "/Filter /FlateDecode"),
        "streams are compressed"
    );
    // A raw content stream would read "<< /Length N >>\nstream" with no filter; a
    // font program "<< /Length N /Length1 M >>\nstream". Neither should appear.
    assert!(
        !has(&bytes, "/Length1") || has(&bytes, "/Filter /FlateDecode /Length"),
        "font program carries a Flate filter"
    );
    let streams = flate_streams(&bytes);
    assert!(
        streams.iter().any(|(d, _)| d.contains("/Length1")),
        "the embedded font program is a FlateDecode stream"
    );
    // At least one compressed stream inflates to a page content stream (Identity-H
    // text runs end in `Tj`, font switches in `Tf`), proving the round-trip.
    assert!(
        streams.iter().any(|(_, body)| {
            let s = String::from_utf8_lossy(body);
            s.contains(" Tf") && s.contains("> Tj")
        }),
        "a content stream round-trips through inflate"
    );
}
#[test]
fn a_font_program_records_its_uncompressed_length1() {
    // /Length1 is the *uncompressed* TrueType program length, and the compressed
    // /Length is smaller — so the font program genuinely shrank.
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    let (dict, inflated) = flate_streams(&bytes)
        .into_iter()
        .find(|(d, _)| d.contains("/Length1"))
        .expect("an embedded font program");
    let field = |key: &str| -> usize {
        let at = dict.find(key).unwrap() + key.len();
        dict[at..]
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap()
    };
    let length1 = field("/Length1 ");
    let length = field("/Length ");
    assert_eq!(length1, inflated.len(), "/Length1 is the uncompressed size");
    assert!(
        length < length1,
        "compressed /Length {length} < uncompressed /Length1 {length1}"
    );
}
#[test]
fn compression_shrinks_a_pdf_with_an_embedded_custom_font() {
    // A doc with a fair amount of text: the FlateDecode streams inflate back to more
    // bytes than the whole compressed PDF, confirming a real size win from #662.
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    let inflated: usize = flate_streams(&bytes).iter().map(|(_, b)| b.len()).sum();
    assert!(inflated > 0, "some streams are compressed");
    assert!(
        inflated > bytes.len() / 2,
        "inflated stream payload ({inflated}) dwarfs the compressed PDF ({})",
        bytes.len()
    );
}
#[test]
fn tounicode_cmap_is_flate_compressed() {
    // The /ToUnicode CMap ships FlateDecode-compressed (#712), the last uncompressed
    // stream type. A compressed stream that inflates to CMap text (begincmap … the
    // Adobe-Identity-UCS name … bfchar entries) proves the filter and the round-trip.
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    let cmap = flate_streams(&bytes)
        .into_iter()
        .find(|(_, body)| {
            let s = String::from_utf8_lossy(body);
            s.contains("begincmap") && s.contains("/Adobe-Identity-UCS")
        })
        .expect("the /ToUnicode CMap is a FlateDecode stream");
    let body = String::from_utf8_lossy(&cmap.1);
    assert!(
        body.contains("beginbfchar") && body.contains("endcmap"),
        "the inflated CMap round-trips to its bfchar mapping"
    );
}
