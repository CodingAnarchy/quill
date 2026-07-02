//! Tests for the PDF writer: the screen proof (#463), the print interior (#464),
//! `[style]` parity (#489), image ornaments (#490), and full-Unicode Type0 fonts
//! (#492). Content text is emitted as Identity-H glyph ids, so these assert on PDF
//! structure (raw byte markers) and the font-routing logic rather than grepping the
//! rendered words; visual/qpdf validation is covered in the PR.

use std::io::Read;

use flate2::read::ZlibDecoder;

use super::font::Fonts;
use super::wrap::{words_of, wrap_lines, Hyphenate, Word};
use super::*;
use crate::export::{Chapter, EmbeddedFont, ExportStyle, FontRole, Part};

/// The default (no `[style]`) baseline.
fn plain() -> ExportStyle {
    ExportStyle::default()
}

fn ep(text: &str, by: Option<&str>) -> Epigraph {
    Epigraph {
        text: text.into(),
        attribution: by.map(str::to_string),
    }
}

fn ch(title: &str, epigraph: Option<Epigraph>, body: &str) -> Chapter {
    Chapter {
        title: title.into(),
        epigraph,
        body: body.into(),
        role: None,
    }
}

/// A book exercising every section kind: front matter, a named part with one chapter
/// (epigraph + italic + bold + a scene break + a sub-heading), and back matter.
fn doc() -> Document {
    Document {
        title: "Red State Earth".into(),
        subtitle: Some("a novel".into()),
        author: Some("A. Writer".into()),
        series: Some("The Cycle".into()),
        publishing: Default::default(),
        epigraph: Some(ep("In the beginning.", Some("Anon"))),
        front: vec![ch("Prologue", None, "Before it all.")],
        parts: vec![Part {
            name: Some("Part One".into()),
            chapters: vec![ch(
                "The Find",
                Some(ep("A single line.", None)),
                "The locket *gleamed* and felt **heavy**.\n\n## A turn\n\nScene one.\n\n---\n\nScene two.",
            )],
        }],
        back: vec![ch("Afterword", None, "After it all.")],
    }
}

/// Whether the raw PDF bytes contain `needle` (avoids lossy-UTF-8 offset issues from
/// the embedded binary font streams).
fn has(bytes: &[u8], needle: &str) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
}

fn count(bytes: &[u8], needle: &str) -> usize {
    bytes
        .windows(needle.len())
        .filter(|w| *w == needle.as_bytes())
        .count()
}

/// The concatenated, inflated page content streams as text. Content streams ship
/// FlateDecode-compressed (#662), so tests that assert on drawing operators (`Tj`,
/// `Tm`, `cm`, `Do`, `re W n`) inflate them first rather than grepping raw bytes.
fn content(bytes: &[u8]) -> String {
    let mut s = String::new();
    for (dict, body) in flate_streams(bytes) {
        // Content streams have a bare `<< /Filter … /Length … >>`; the font program
        // carries `/Length1`. Skip the font so the text is real page content.
        if !dict.contains("/Length1") {
            s.push_str(&String::from_utf8_lossy(&body));
        }
    }
    s
}

#[test]
fn is_a_well_formed_pdf() {
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7"), "header");
    assert!(bytes.ends_with(b"%%EOF\n"), "trailer marker");
    for marker in [
        "/Type /Catalog",
        "/Root 1 0 R",
        "/Type /Pages",
        "xref",
        "trailer",
    ] {
        assert!(has(&bytes, marker), "missing {marker}");
    }
}

#[test]
fn a_fenced_code_block_exports_to_a_well_formed_pdf() {
    // A chapter whose body is a fenced block (with a blank line inside) must lay
    // out and serialize without panicking, into a valid PDF (#522).
    let d = Document {
        title: "T".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: Default::default(),
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![ch(
                "C",
                None,
                "Intro.\n\n```\nZENITH // 2053\n\nPRE-IGNIT\n```\n\nAfter.",
            )],
        }],
        back: vec![],
    };
    let bytes = to_pdf(&d, &plain()).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7") && bytes.ends_with(b"%%EOF\n"));
    assert!(has(&bytes, "/Type /Catalog") && has(&bytes, "xref"));
}

#[test]
fn a_fenced_block_renders_in_the_embedded_monospace_face() {
    // A fenced block routes to the bundled DejaVu Sans Mono face (#524), embedded as
    // a Type0 composite like every other face — so the block reads fixed-width, not
    // in the body serif. A document with no fenced block embeds no mono face.
    let mono = Document {
        title: "T".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: Default::default(),
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![ch(
                "C",
                None,
                "Before.\n\n```\nID  NAME\n01  ok\n```\n\nAfter.",
            )],
        }],
        back: vec![],
    };
    let bytes = to_pdf(&mono, &plain()).unwrap();
    assert!(
        has(&bytes, "/BaseFont /DejaVuSansMono"),
        "fenced block uses the bundled monospace face"
    );
    // No fenced block → no mono face embedded.
    assert!(
        !has(&to_pdf(&doc(), &plain()).unwrap(), "DejaVuSansMono"),
        "the mono face is only embedded when a fenced block uses it"
    );
}

#[test]
fn a_long_preformatted_line_wraps_to_the_text_column() {
    // An over-long fenced line is broken into multiple pieces at character boundaries
    // so it doesn't run off the page edge (#524); a line that fits is left whole.
    let layout = Layout::screen(&plain());
    let col = layout.text_w();
    let long = "x".repeat(400);
    let pieces = mono_wrap(&layout, &long, col);
    assert!(pieces.len() > 1, "an over-long line is wrapped");
    for piece in &pieces {
        assert!(
            layout.measure_mono(piece, MONO_SIZE) <= col,
            "each wrapped piece fits the text column"
        );
    }
    assert_eq!(
        pieces.concat(),
        long,
        "wrapping preserves every character in order"
    );
    assert_eq!(
        mono_wrap(&layout, "ID  NAME", col),
        vec!["ID  NAME".to_string()],
        "a line that fits is left whole"
    );
}

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
fn each_section_starts_on_its_own_page_in_the_screen_profile() {
    // Contents + prologue + part + chapter + afterword, each its own page (the title
    // page shares no page). The Contents page is the front-matter TOC (#603).
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    assert_eq!(count(&bytes, "/Type /Page /Parent"), 6);
}

#[test]
fn builds_a_nested_bookmark_outline() {
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    let s = String::from_utf8_lossy(&bytes);
    assert!(
        has(&bytes, "/PageMode /UseOutlines"),
        "opens with the outline"
    );
    for title in [
        "/Title (Prologue)",
        "/Title (Part One)",
        "/Title (The Find)",
        "/Title (Afterword)",
    ] {
        assert!(has(&bytes, title), "{title} bookmarked");
    }
    let at = s.find("/Title (Part One)").unwrap();
    let item = &s[at..at + 200.min(s.len() - at)];
    assert!(
        item.contains("/First ") && item.contains("/Count 1"),
        "part nests its chapter: {item}"
    );
}

#[test]
fn a_series_bookmarks_every_book_in_order() {
    let book = |title: &str| Document {
        title: title.into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: Default::default(),
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![ch("Ch", None, "Prose.")],
        }],
        back: vec![],
    };
    let bytes = to_pdf_series(
        &[book("Book One"), book("Book Two")],
        Some("The Saga"),
        Some("A. Writer"),
        &plain(),
    )
    .unwrap();
    let s = String::from_utf8_lossy(&bytes);
    let one = s.find("/Title (Book One)").expect("book one bookmarked");
    let two = s.find("/Title (Book Two)").expect("book two bookmarked");
    assert!(one < two, "books in reading order");
}

#[test]
fn a_series_reprints_each_books_copyright_not_just_the_first() {
    // #711: a series omnibus must not silently drop a later book's rights/ISBN. The
    // omnibus's front-matter copyright page is series-level (the first book's), but
    // each book's own copyright is reprinted on its divider page. So a series whose
    // *second* book carries copyright metadata draws strictly more page-content text
    // (Identity-H glyph runs shown with `Tj`) than the same series with no per-book
    // metadata — proof the later book's copyright reached the layout.
    let book = |title: &str, publishing: PubMeta| Document {
        title: title.into(),
        subtitle: None,
        author: None,
        series: None,
        publishing,
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![ch("Ch", None, "Prose.")],
        }],
        back: vec![],
    };
    let bare = || book("Book One", PubMeta::default());
    // Only the *second* book carries metadata, so a first-book-only reader would
    // drop it — the regression this guards.
    let second_meta = PubMeta {
        isbn: Some("978-1-11-111111-1".into()),
        rights: Some("© 2026 B. Author. All rights reserved.".into()),
        ..Default::default()
    };

    let with_meta = to_pdf_series(
        &[bare(), book("Book Two", second_meta)],
        Some("The Saga"),
        Some("A. Writer"),
        &plain(),
    )
    .unwrap();
    let without = to_pdf_series(
        &[bare(), book("Book Two", PubMeta::default())],
        Some("The Saga"),
        Some("A. Writer"),
        &plain(),
    )
    .unwrap();

    let shown = |bytes: &[u8]| count(content(bytes).as_bytes(), " Tj\n");
    assert!(
        shown(&with_meta) > shown(&without),
        "the second book's copyright is reprinted, not dropped: {} vs {}",
        shown(&with_meta),
        shown(&without),
    );
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
    let font = include_bytes!("../../../assets/fonts/LiberationSerif-Italic.ttf");
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
    let body_up = include_bytes!("../../../assets/fonts/LiberationSerif-Regular.ttf");
    let body_it = include_bytes!("../../../assets/fonts/LiberationSerif-Italic.ttf");
    let head = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");
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
    let body_up = include_bytes!("../../../assets/fonts/LiberationSerif-Regular.ttf");
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
fn trim_points_resolves_known_sizes_only() {
    assert_eq!(trim_points("6x9"), Some((432.0, 648.0)));
    assert_eq!(trim_points("5x8"), Some((360.0, 576.0)));
    assert_eq!(trim_points("5.5x8.5"), Some((396.0, 612.0)));
    assert_eq!(trim_points("7x10"), None);
    assert_eq!(DEFAULT_TRIM, "6x9");
}

#[test]
fn print_pdf_embeds_fonts_at_the_chosen_trim() {
    let (w, h) = trim_points("6x9").unwrap();
    let bytes = to_pdf_print(&doc(), w, h, 0.0, &plain()).unwrap();
    assert!(
        has(&bytes, "/Subtype /Type0") && has(&bytes, "/FontFile2"),
        "fonts embedded"
    );
    assert!(has(&bytes, "MediaBox [0 0 432 648]"), "6x9 trim");
}

#[test]
fn print_pdf_starts_chapters_on_a_recto_inserting_a_blank() {
    let mini = Document {
        title: "T".into(),
        subtitle: None,
        author: Some("Auth".into()),
        series: None,
        publishing: Default::default(),
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![ch("One", None, "Body.")],
        }],
        back: vec![],
    };
    let (w, h) = trim_points("6x9").unwrap();
    let screen = count(&to_pdf(&mini, &plain()).unwrap(), "/Type /Page /Parent");
    let print = count(
        &to_pdf_print(&mini, w, h, 0.0, &plain()).unwrap(),
        "/Type /Page /Parent",
    );
    // A one-entry Contents page joins the front matter (#603): title + contents +
    // chapter on screen; print inserts a blank verso before each recto section start.
    assert_eq!(screen, 3, "title + contents + chapter");
    assert_eq!(print, 5, "title, blank+contents recto, blank+chapter recto");
}

/// Encode a tiny `w`×`h` solid-red PNG with the given alpha — a real raster the PDF
/// writer can decode and embed.
fn png_bytes(w: u32, h: u32, alpha: u8) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, alpha]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

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

#[test]
fn bleed_extends_the_media_box_by_the_allowance_on_each_side() {
    // With `[print] bleed`, the media box grows by the bleed on every side (#652), so
    // full-bleed art has room to run past the trim. 0.125" = 9pt → 6x9 (432×648)
    // becomes 450×666; no bleed leaves the media box exactly the trim.
    let bleed = 0.125 * 72.0; // 9pt
    let (w, h) = trim_points("6x9").unwrap();
    let bled = to_pdf_print(&doc(), w, h, bleed, &plain()).unwrap();
    assert!(
        has(&bled, "MediaBox [0 0 450 666]"),
        "trim + 9pt bleed each side"
    );

    let plain_box = to_pdf_print(&doc(), w, h, 0.0, &plain()).unwrap();
    assert!(
        has(&plain_box, "MediaBox [0 0 432 648]"),
        "no bleed = the trim"
    );
}

#[test]
fn bleed_lets_the_cover_fill_to_the_bleed_edge() {
    // The cover fills the whole media box (trim + bleed) — clipped to the media box so
    // a mismatched aspect crops rather than leaving a white border (#652). The image is
    // 600×900 (aspect 0.667); at 6x9+9pt (450×666) it covers by scaling ×0.75 to
    // 450×675, centered → the placement `450 0 0 675 0 -4.50 cm`, clipped to 450×666.
    let mut style = ExportStyle {
        cover_image: Some("art/cover.png".into()),
        ..Default::default()
    };
    style.resolve_images(|_| Ok(png_bytes(600, 900, 255)));
    let bleed = 0.125 * 72.0; // 9pt
    let (w, h) = trim_points("6x9").unwrap();
    let bytes = to_pdf_print(&doc(), w, h, bleed, &style).unwrap();
    assert!(
        has(&bytes, "MediaBox [0 0 450 666]"),
        "media box = trim + bleed"
    );
    // The cover fills to the bleed edge (height 675 > media 666 → overflow), clipped.
    let drawn = content(&bytes);
    assert!(
        drawn.contains("0 0 450 666 re W n"),
        "cover clipped to the media box"
    );
    assert!(
        drawn.contains("450 0 0 675 0 -4.50 cm /ImCover Do"),
        "cover scaled to cover the media box and centered"
    );
}

#[test]
fn bleed_leaves_the_body_margins_measured_from_the_trim() {
    // Only full-bleed art bleeds; the text column keeps its trim-relative margins
    // (#652). Adding a 9pt bleed enlarges the media box but the column width is
    // unchanged, and the column's left/top shift by exactly the bleed (the trim is
    // inset by the bleed within the media box) — not by more.
    let (w, h) = trim_points("6x9").unwrap();
    let no_bleed = Layout::print(w, h, 0.0, &plain());
    let bleed = 9.0;
    let bled = Layout::print(w, h, bleed, &plain());

    assert_eq!(no_bleed.text_w(), bled.text_w(), "column width unchanged");
    assert_eq!(
        bled.left(0) - no_bleed.left(0),
        bleed,
        "column left shifts by exactly the bleed"
    );
    assert_eq!(
        bled.text_top() - no_bleed.text_top(),
        bleed,
        "column top shifts by exactly the bleed"
    );
    assert_eq!(
        bled.text_bottom() - no_bleed.text_bottom(),
        bleed,
        "column bottom shifts by exactly the bleed"
    );
    // The media box grew by twice the bleed; the trim didn't.
    assert_eq!(bled.media_w() - no_bleed.media_w(), 2.0 * bleed);
    assert_eq!(bled.media_h() - no_bleed.media_h(), 2.0 * bleed);
}

#[test]
fn a_copyright_page_adds_a_front_matter_page() {
    // Publishing metadata (#583) renders a copyright page after the title page:
    // one extra page over the no-metadata baseline, in a still-valid PDF.
    let mut d = doc();
    d.publishing = PubMeta {
        publisher: Some("Quill & Co.".into()),
        date: Some("2026".into()),
        rights: Some("© 2026 A. Writer".into()),
        ..Default::default()
    };
    let bytes = to_pdf(&d, &plain()).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7") && bytes.ends_with(b"%%EOF\n"));
    let baseline = count(&to_pdf(&doc(), &plain()).unwrap(), "/Type /Page /Parent");
    let with_copyright = count(&bytes, "/Type /Page /Parent");
    assert_eq!(
        with_copyright,
        baseline + 1,
        "copyright adds a front-matter page"
    );

    // No publishing metadata → no extra page (default output unchanged).
    assert_eq!(
        count(&to_pdf(&doc(), &plain()).unwrap(), "/Type /Page /Parent"),
        baseline,
        "no copyright page by default"
    );
}

#[test]
fn a_contents_page_lists_titles_with_folios() {
    // The front matter carries a printed "Contents" page (#603): the heading, every
    // section title, and — for chapters/front/back, not the part heading — a folio (a
    // page label = page index + 1). Content is Identity-H glyph ids, so assert on the
    // page count and that the outline folios the Contents shows are the real body pages.
    let d = doc();
    let bytes = to_pdf(&d, &plain()).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7") && bytes.ends_with(b"%%EOF\n"));

    // The Contents page is a real extra front-matter page: title + prologue + part +
    // chapter + afterword = 5 section pages, plus the one Contents page = 6.
    assert_eq!(
        count(&bytes, "/Type /Page /Parent"),
        6,
        "a Contents page joins the front matter"
    );

    // The Contents folios come from the same outline the bookmarks use; reserving the
    // TOC first makes the body land after it. Rebuild the outline the way render_book
    // does to read the folios the Contents page shows.
    let outline = {
        let mut pdf = Pdf::new(
            Layout::screen(&plain()),
            StyleBits::from_style(&plain()),
            vec![],
        );
        pdf.title_page(None, "T", None, None, None);
        pdf.reserve_contents(&doc_toc_titles(&d));
        book_outline(&mut pdf, &d)
    };
    // Four entries total (prologue, part, chapter, afterword); the part carries no
    // folio, so three folios are shown.
    let all = toc_entries(&outline);
    assert_eq!(all.len(), 4, "prologue, part, chapter, afterword");
    let folios: Vec<usize> = all.into_iter().flatten().collect();
    assert_eq!(folios.len(), 3, "the part has no folio");
    assert!(
        folios.iter().all(|&p| p >= 2),
        "the body follows the reserved front matter, got {folios:?}"
    );
}

#[test]
fn the_contents_page_reserves_body_pagination() {
    // Reserving the Contents page before the body means the body's folios already
    // account for it — the first body section starts after the reserved page.
    let d = doc();
    let mut pdf = Pdf::new(
        Layout::screen(&plain()),
        StyleBits::from_style(&plain()),
        vec![],
    );
    pdf.title_page(None, "T", None, None, None);
    let toc_start = pdf.reserve_contents(&doc_toc_titles(&d)).unwrap().start;
    let prologue = pdf.chapter(&d.front[0]);
    assert!(
        prologue > toc_start,
        "the body follows the reserved Contents page ({prologue} > {toc_start})"
    );
}

#[test]
fn a_long_contents_flows_onto_a_second_page() {
    // More chapters than fit on one Contents page flow onto a second reserved page —
    // the plan and the reservation agree on the page count, one folio line per entry.
    let many: Vec<Chapter> = (0..60)
        .map(|i| ch(&format!("Chapter Number {i}"), None, "Body."))
        .collect();
    let d = Document {
        title: "Big".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: Default::default(),
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: many,
        }],
        back: vec![],
    };
    let (w, h) = trim_points("6x9").unwrap();
    let mut pdf = Pdf::new(
        Layout::print(w, h, 0.0, &plain()),
        StyleBits::from_style(&plain()),
        vec![],
    );
    pdf.title_page(None, "Big", None, None, None);
    let titles = doc_toc_titles(&d);
    let plan = pdf.reserve_contents(&titles).unwrap();
    let pages = plan.lines.iter().map(|l| l.page).max().unwrap() + 1;
    assert!(
        pages >= 2,
        "60 chapters overflow one Contents page, got {pages}"
    );
    assert_eq!(
        plan.lines
            .iter()
            .filter(|l| l.folio_entry.is_some())
            .count(),
        titles.len(),
        "one folio line per entry across the pages"
    );
    // And the whole book still serializes to a well-formed PDF.
    let bytes = to_pdf_print(&d, w, h, 0.0, &plain()).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7") && bytes.ends_with(b"%%EOF\n"));
}

#[test]
fn roman_numerals_render_the_conventional_forms() {
    // The front-matter folio converter (#651): the compact subtractive forms.
    for (n, s) in [
        (1, "i"),
        (2, "ii"),
        (3, "iii"),
        (4, "iv"),
        (5, "v"),
        (9, "ix"),
        (10, "x"),
        (14, "xiv"),
        (40, "xl"),
        (49, "xlix"),
        (90, "xc"),
        (2026, "mmxxvi"),
    ] {
        assert_eq!(roman(n), s, "{n} → {s}");
    }
    assert_eq!(roman(0), "", "zero has no numeral");
}

/// Lay out `d` the way `render_book` does — title, copyright, reserved Contents, then
/// the body — so `folio_label` reflects the real front/body split (#651). Returns the
/// laid-out `Pdf` and the per-entry folio page indices the Contents shows.
fn laid_out(d: &Document) -> (Pdf, Vec<Option<usize>>) {
    let (w, h) = trim_points("6x9").unwrap();
    let mut pdf = Pdf::new(
        Layout::print(w, h, 0.0, &plain()),
        StyleBits::from_style(&plain()),
        vec![],
    );
    pdf.title_page(
        d.series.as_deref(),
        &d.title,
        d.subtitle.as_deref(),
        d.author.as_deref(),
        d.epigraph.as_ref(),
    );
    pdf.copyright_page(&d.publishing);
    pdf.reserve_contents(&doc_toc_titles(d));
    pdf.begin_body();
    pdf.outline = book_outline(&mut pdf, d);
    let folios = toc_entries(&pdf.outline);
    (pdf, folios)
}

#[test]
fn front_matter_folios_are_roman_and_the_body_restarts_at_arabic_one() {
    // The #651 scheme: the title page is unnumbered, the following front-matter pages
    // (copyright, Contents) carry roman folios, and the first body page is Arabic "1".
    let mut d = doc();
    d.publishing = PubMeta {
        rights: Some("© 2026 A. Writer".into()),
        ..Default::default()
    };
    let (pdf, _) = laid_out(&d);
    // Page 0 is the title page (opener, non-blank): counted but unnumbered.
    assert_eq!(pdf.folio_label(0), None, "title page unnumbered");
    // The pages before the body are all roman (or unnumbered), never Arabic.
    for i in 0..pdf.body_start {
        if let Some(label) = pdf.folio_label(i) {
            assert!(
                label.chars().all(|c| "ivxlcdm".contains(c)),
                "front-matter folio {label:?} at page {i} is roman"
            );
        }
    }
    // At least one front-matter page actually shows a roman folio (copyright/Contents).
    assert!(
        (0..pdf.body_start).any(|i| pdf.folio_label(i).as_deref() == Some("ii")),
        "a front-matter page is folio ii"
    );
    // The first numbered body page (a recto start may insert a blank verso first) is
    // Arabic 1.
    let first_body = (pdf.body_start..pdf.meta.len())
        .find(|&i| !pdf.meta[i].blank)
        .expect("a body page");
    assert_eq!(
        pdf.folio_label(first_body),
        Some("1".to_string()),
        "the body starts at Arabic 1"
    );
    // No body page carries a roman folio.
    for i in pdf.body_start..pdf.meta.len() {
        if let Some(label) = pdf.folio_label(i) {
            assert!(
                label.chars().all(|c| c.is_ascii_digit()),
                "body folio {label:?} at page {i} is Arabic"
            );
        }
    }
}

#[test]
fn blank_and_cover_pages_carry_no_folio() {
    // Recto starts insert blank versos, and a cover leads page 0 (#651/#582): none of
    // those pages take a folio, and skipping them keeps the count contiguous.
    let mut style = ExportStyle {
        cover_image: Some("art/cover.png".into()),
        ..Default::default()
    };
    style.resolve_images(|_| Ok(png_bytes(600, 900, 255)));
    let (w, h) = trim_points("6x9").unwrap();
    let mut pdf = Pdf::new(
        Layout::print(w, h, 0.0, &style),
        StyleBits::from_style(&style),
        decode_images(&style),
    );
    let d = doc();
    pdf.cover_page();
    pdf.title_page(None, &d.title, None, d.author.as_deref(), None);
    pdf.copyright_page(&d.publishing);
    pdf.reserve_contents(&doc_toc_titles(&d));
    pdf.begin_body();
    pdf.outline = book_outline(&mut pdf, &d);
    // Every blank page (cover + inserted blank versos) is unnumbered.
    for (i, m) in pdf.meta.iter().enumerate() {
        if m.blank {
            assert_eq!(pdf.folio_label(i), None, "blank page {i} unnumbered");
        }
    }
    // The Arabic body folios are contiguous (1, 2, 3, …) despite the blank versos.
    let body: Vec<String> = (pdf.body_start..pdf.meta.len())
        .filter_map(|i| pdf.folio_label(i))
        .collect();
    let expected: Vec<String> = (1..=body.len()).map(|n| n.to_string()).collect();
    assert_eq!(body, expected, "body folios run 1,2,3,… skipping blanks");
}

#[test]
fn contents_folios_use_the_body_scheme() {
    // The printed Contents lists each entry's folio under the same scheme as the page
    // decoration (#651): body chapters land on Arabic labels, not physical indices.
    let (pdf, folios) = laid_out(&doc());
    let labels: Vec<String> = folios
        .iter()
        .filter_map(|f| f.and_then(|i| pdf.folio_label(i)))
        .collect();
    // Prologue, chapter, afterword each get a folio (the part heading doesn't).
    assert_eq!(labels.len(), 3, "three entries carry a folio");
    // Every Contents folio is Arabic (the body), matching the decoration scheme.
    for label in &labels {
        assert!(
            label.chars().all(|c| c.is_ascii_digit()),
            "Contents folio {label:?} is Arabic"
        );
    }
    // The prologue (first body section) is folio 1, and the folios ascend.
    assert_eq!(labels[0], "1", "the first body entry is folio 1");
    let nums: Vec<usize> = labels.iter().map(|l| l.parse().unwrap()).collect();
    assert!(
        nums.windows(2).all(|w| w[0] < w[1]),
        "Contents folios ascend, got {nums:?}"
    );
    // The label matches what the page itself is numbered.
    for (entry, label) in folios.iter().flatten().zip(&labels) {
        assert_eq!(
            pdf.folio_label(*entry).as_deref(),
            Some(label.as_str()),
            "Contents folio matches the page's own folio"
        );
    }
}

/// The `x` coordinates of every `… Tm` matrix in `content`, in order — where each
/// glyph run is positioned. A justified line places one run per word, so these are the
/// per-word pen positions.
fn tm_xs(content: &str) -> Vec<f32> {
    content
        .lines()
        .filter(|l| l.ends_with(" Tm"))
        .filter_map(|l| {
            // `1 0 {shear} 1 {x} {y} Tm` — the x is the 5th of six numbers.
            let n: Vec<&str> = l.trim_end_matches(" Tm").split_whitespace().collect();
            n.get(4).and_then(|s| s.parse::<f32>().ok())
        })
        .collect()
}

#[test]
fn body_lines_are_justified_flush_to_the_right_margin() {
    // A justified body line fills the column: its last word ends at the right text edge
    // within a hair of measurement tolerance (#650). Only the last line stays ragged.
    let style = plain();
    let (w, h) = trim_points("6x9").unwrap();
    let mut pdf = Pdf::new(
        Layout::print(w, h, 0.0, &style),
        StyleBits::from_style(&style),
        vec![],
    );
    pdf.start_page();
    let full = pdf.layout.text_w();
    let words: Vec<Word> = words_of(
        &[Run::new(
            "The quick brown fox jumps over the lazy dog again and again and yet again to \
             fill the line with plenty of words so that it wraps to several lines within \
             the narrower print column and can be justified across both margins cleanly",
            false,
            false,
        )],
        false,
    );
    let lines = wrap_lines(&pdf.layout, &words, BODY_SIZE, Hyphenate::Off, |_| full);
    assert!(lines.len() >= 2, "the sample wraps to multiple lines");
    let (first, _) = &lines[0];
    let first: Vec<Word> = first.clone();
    let left = pdf.left();
    pdf.draw_justified_line(left, pdf.y, &first, BODY_SIZE, full);

    let content = pdf.cur().clone();
    let xs = tm_xs(&content);
    assert_eq!(xs.len(), first.len(), "one positioned run per word");
    // The last word starts at its pen x; its right edge is x + its measured width.
    let last = first.last().unwrap();
    let last_w = pdf
        .layout
        .measure(&last.text, BODY_SIZE, last.style(), last.heading);
    let right_edge = xs.last().unwrap() + last_w;
    let target_right = left + full;
    assert!(
        (right_edge - target_right).abs() < 0.5,
        "justified line reaches the right margin: {right_edge} vs {target_right}"
    );
    // The first word sits flush at the left edge, and words march strictly rightward.
    assert!((xs[0] - left).abs() < 0.01, "first word flush left");
    assert!(
        xs.windows(2).all(|p| p[1] > p[0]),
        "words placed left to right: {xs:?}"
    );
    // The inter-word gap is stretched past a normal space (the line is spread to fill).
    let space = pdf
        .layout
        .measure(" ", BODY_SIZE, FontStyle::REGULAR, false);
    let first_word_w = pdf.layout.measure(
        &first[0].text,
        BODY_SIZE,
        first[0].style(),
        first[0].heading,
    );
    let applied_gap = xs[1] - xs[0] - first_word_w;
    assert!(
        applied_gap >= space,
        "gap {applied_gap} spread to at least a normal space {space}"
    );
}

#[test]
fn the_last_line_of_a_paragraph_is_not_justified() {
    // The final line stays flush-left (ragged), never stretched — otherwise a short
    // closing line would sprawl across the column (#650).
    let style = plain();
    let mut pdf = Pdf::new(
        Layout::screen(&style),
        StyleBits::from_style(&style),
        vec![],
    );
    pdf.start_page();
    let left = pdf.left();
    let start_y = pdf.y;
    // A short two-word paragraph: one line, drawn flush-left.
    pdf.paragraph(&[Run::new("Two words.", false, false)]);
    let content = pdf.cur().clone();
    let xs = tm_xs(&content);
    // One run for the whole (single, last) line — flush-left, indented, not per-word.
    assert!(!xs.is_empty(), "the line is drawn");
    assert!(
        (xs[0] - (left + INDENT)).abs() < 0.01,
        "first (and only) line is indented and flush-left, x={}",
        xs[0]
    );
    assert!(pdf.y < start_y, "the baseline advanced");
}

#[test]
fn a_paragraph_does_not_orphan_its_first_line_at_a_page_bottom() {
    // With room for only one line, a multi-line paragraph moves to the next page rather
    // than strand its first line at the bottom (#650).
    let style = plain();
    let mut pdf = Pdf::new(
        Layout::screen(&style),
        StyleBits::from_style(&style),
        vec![],
    );
    pdf.start_page();
    // Drop to one line of room above the bottom margin.
    pdf.y = pdf.layout.text_bottom() + BODY_LEADING * 0.5;
    let pages_before = pdf.pages.len();
    pdf.paragraph(&[Run::new(
        "This is a paragraph long enough to wrap across at least two full lines of body \
         text so that an orphan would form if it began here at the very bottom.",
        false,
        false,
    )]);
    assert!(
        pdf.pages.len() > pages_before,
        "the paragraph started a fresh page instead of orphaning its first line"
    );
}

#[test]
fn a_paragraph_does_not_widow_its_last_line_at_a_page_top() {
    // When the natural break would leave exactly the last line alone on the next page,
    // the split moves one line earlier so two lines carry over together (#650).
    let n = 5usize; // a five-line paragraph
    let leading = BODY_LEADING;
    let style = plain();
    let mut pdf = Pdf::new(
        Layout::screen(&style),
        StyleBits::from_style(&style),
        vec![],
    );
    pdf.start_page();
    // Place y so that exactly four of the five lines fit here — the natural break would
    // strand line five (a widow).
    pdf.y = pdf.layout.text_bottom() + leading * 3.5;
    let brk = pdf.widow_orphan_break(n, leading);
    assert_eq!(
        brk,
        Some(3),
        "break before the 4th line so lines 4 and 5 travel together"
    );
    // Sanity: a paragraph that splits leaving two lines on each side needs no help.
    pdf.y = pdf.layout.text_bottom() + leading * 1.5; // two lines fit
    assert_eq!(
        pdf.widow_orphan_break(4, leading),
        None,
        "≥2 lines each side"
    );
}

/// Concatenate every word's text across all wrapped lines (a hyphenated head keeps its
/// trailing `-`), so a test can look for a break point and reconstruct the whole word.
fn wrapped_texts(lines: &[(Vec<Word>, f32)]) -> Vec<String> {
    lines
        .iter()
        .flat_map(|(line, _)| line.iter().map(|w| w.text.clone()))
        .collect()
}

#[test]
fn a_long_word_hyphenates_at_a_justified_line_break() {
    // A justified body line that can't fit a long word whole breaks it at a valid en-US
    // point: the head ends with a hyphen on one line and the tail continues on the next,
    // tightening the fill instead of forcing a loose gap (#678).
    let layout = Layout::screen(&plain());
    let words = words_of(
        &[Run::new(
            "The complicated extraordinary machinery",
            false,
            false,
        )],
        false,
    );
    // A column just wide enough that "extraordinary" overflows after "complicated" and
    // must break to keep the line full.
    let width = layout.measure(
        "The complicated extra",
        BODY_SIZE,
        FontStyle::REGULAR,
        false,
    );
    let lines = wrap_lines(&layout, &words, BODY_SIZE, Hyphenate::On, |_| width);
    let texts = wrapped_texts(&lines);
    // Some fragment ends in a hyphen (a taken break), and no fragment overflows the
    // column — the head was chosen to fit.
    assert!(
        texts.iter().any(|t| t.ends_with('-')),
        "a long word was hyphenated: {texts:?}"
    );
    for t in &texts {
        assert!(
            layout.measure(t, BODY_SIZE, FontStyle::REGULAR, false) <= width + 0.01,
            "no fragment overflows the column: {t:?}"
        );
    }
    // Stripping the break hyphens reconstructs the original words in order (nothing lost).
    let joined: String = texts
        .iter()
        .map(|t| t.strip_suffix('-').unwrap_or(t))
        .collect::<Vec<_>>()
        .join(" ");
    // The head/tail of a split are adjacent words with no space, so collapse the pair.
    assert!(
        joined.replace("- ", "").contains("extraordinary") || joined.contains("extraordinary"),
        "the split word round-trips: {joined:?}"
    );
}

#[test]
fn hyphenation_is_off_for_non_justified_flows() {
    // Headings/epigraphs pass `Hyphenate::Off`, so even a long word that overflows a
    // narrow width is never broken — it moves whole to the next line (#678).
    let layout = Layout::screen(&plain());
    let words = words_of(&[Run::new("The extraordinary heading", true, false)], true);
    let width = layout.measure("The extra", HEADING_SIZE, FontStyle::REGULAR, true);
    let lines = wrap_lines(&layout, &words, HEADING_SIZE, Hyphenate::Off, |_| width);
    let texts = wrapped_texts(&lines);
    assert!(
        !texts.iter().any(|t| t.ends_with('-')),
        "no heading word is hyphenated: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "extraordinary"),
        "the long word stays whole: {texts:?}"
    );
}

#[test]
fn short_words_and_stub_fragments_are_never_hyphenated() {
    // A short word (below the minimum) and one whose only breaks would leave a stub are
    // left whole even when hyphenation is on — a broken stub reads worse than a gap.
    let layout = Layout::screen(&plain());
    // "rivers" has no en-US pattern break; "cat" is too short. Neither breaks.
    for word in ["cat", "rivers", "the"] {
        let words = words_of(
            &[Run::new(format!("A {word} here now"), false, false)],
            false,
        );
        // A width so tight each word wraps to its own line, forcing the break attempt.
        let width = layout.measure("A r", BODY_SIZE, FontStyle::REGULAR, false);
        let lines = wrap_lines(&layout, &words, BODY_SIZE, Hyphenate::On, |_| width);
        let texts = wrapped_texts(&lines);
        assert!(
            texts.iter().any(|t| t == word),
            "{word:?} stays whole: {texts:?}"
        );
    }
}

#[test]
fn style_warnings_flag_svg_and_truly_uncovered_glyphs() {
    // SVG can't be rasterized for PDF.
    let mut svg = ExportStyle {
        scene_break_image: Some("art/break.svg".into()),
        ..Default::default()
    };
    svg.resolve_images(|_| Ok(b"<svg/>".to_vec()));
    assert!(
        pdf_style_warnings(&svg).iter().any(|w| w.contains("SVG")),
        "SVG reported"
    );
    // A raster ornament is fine — no warning.
    let mut png = ExportStyle {
        scene_break_image: Some("art/break.png".into()),
        ..Default::default()
    };
    png.resolve_images(|_| Ok(png_bytes(2, 2, 255)));
    assert!(
        pdf_style_warnings(&png).is_empty(),
        "raster ornament needs no warning"
    );
    // A CJK scene-break glyph no embedded font covers → reported.
    let cjk = ExportStyle {
        scene_break: Some("中".into()),
        ..Default::default()
    };
    assert!(
        pdf_style_warnings(&cjk).iter().any(|w| w.contains('中')),
        "uncovered glyph reported"
    );
}

/// Inflate every FlateDecode stream in `bytes` and return each `(dict, inflated)`
/// pair, where `dict` is the stream's `<< … >>` dictionary text. Walks the raw PDF
/// object by object: each `stream\n … \nendstream` payload preceded by a dict that
/// names `/Filter /FlateDecode` is decompressed with zlib inflate.
fn flate_streams(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let open = b"stream\n";
    let close = b"\nendstream";
    let mut i = 0;
    while let Some(rel) = bytes[i..].windows(open.len()).position(|w| w == open) {
        let dict_end = i + rel;
        let payload = dict_end + open.len();
        let end_rel = bytes[payload..]
            .windows(close.len())
            .position(|w| w == close)
            .expect("every stream is terminated");
        let dict_start = bytes[..dict_end]
            .windows(2)
            .rposition(|w| w == b"<<")
            .expect("a stream has a preceding dict");
        let dict = String::from_utf8_lossy(&bytes[dict_start..dict_end]).into_owned();
        if dict.contains("/Filter /FlateDecode") {
            let mut inflated = Vec::new();
            ZlibDecoder::new(&bytes[payload..payload + end_rel])
                .read_to_end(&mut inflated)
                .expect("FlateDecode stream inflates");
            out.push((dict, inflated));
        }
        i = payload + end_rel + close.len();
    }
    out
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
fn bleed_export_emits_trim_and_bleed_boxes() {
    // A bleed print export states where the finished page is (#692): every page carries
    // a /TrimBox inset within the media box by the bleed, and a /BleedBox equal to the
    // media box, so a print shop knows where to cut. 6x9 (432×648) + 9pt bleed each side
    // → media 450×666, trim inset to [9 9 441 657].
    let bleed = 0.125 * 72.0; // 9pt
    let (w, h) = trim_points("6x9").unwrap();
    let bytes = to_pdf_print(&doc(), w, h, bleed, &plain()).unwrap();
    assert!(
        has(&bytes, "MediaBox [0 0 450 666]"),
        "media box = trim + bleed"
    );
    assert!(
        has(&bytes, "/TrimBox [9 9 441 657]"),
        "trim inset within the media box by the bleed"
    );
    assert!(
        has(&bytes, "/BleedBox [0 0 450 666]"),
        "bleed box = the full media box"
    );
    // On every page, not just one.
    let pages = count(&bytes, "/Type /Page ");
    assert!(pages > 1, "the doc spans several pages");
    assert_eq!(count(&bytes, "/TrimBox"), pages, "a TrimBox on every page");
    assert_eq!(
        count(&bytes, "/BleedBox"),
        pages,
        "a BleedBox on every page"
    );
}

#[test]
fn no_bleed_export_makes_trim_and_bleed_coincide_with_the_media_box() {
    // With no bleed the trim, bleed, and media boxes coincide — still emitted so the
    // file is unambiguous (#692). 6x9 = 432×648.
    let (w, h) = trim_points("6x9").unwrap();
    let bytes = to_pdf_print(&doc(), w, h, 0.0, &plain()).unwrap();
    assert!(
        has(&bytes, "/TrimBox [0 0 432 648]"),
        "trim = the media box"
    );
    assert!(
        has(&bytes, "/BleedBox [0 0 432 648]"),
        "bleed = the media box"
    );
}

#[test]
fn the_trailer_references_an_info_dict_with_title_author_and_producer() {
    // The PDF carries document metadata (#692): an /Info dict with Title/Author from the
    // manifest and Producer "Quill", referenced from the trailer so readers and library
    // systems can index it.
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    assert!(has(&bytes, "/Info "), "the trailer references /Info");
    assert!(has(&bytes, "/Title (Red State Earth)"), "manifest title");
    assert!(has(&bytes, "/Author (A. Writer)"), "manifest author");
    assert!(has(&bytes, "/Producer (Quill)"), "producer is Quill");
}

#[test]
fn info_dict_omits_an_absent_author_but_still_names_the_producer() {
    // A book with no author (#692): /Info drops /Author rather than emitting an empty
    // one, but Title and Producer are always present.
    let mut d = doc();
    d.author = None;
    let bytes = to_pdf(&d, &plain()).unwrap();
    assert!(
        has(&bytes, "/Title (Red State Earth)"),
        "title still present"
    );
    assert!(has(&bytes, "/Producer (Quill)"), "producer still present");
    assert!(!has(&bytes, "/Author"), "no /Author when unset");
}
