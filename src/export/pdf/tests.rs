//! Tests for the PDF writer: the screen proof (#463), the print interior (#464),
//! `[style]` parity (#489), image ornaments (#490), and full-Unicode Type0 fonts
//! (#492). Content text is emitted as Identity-H glyph ids, so these assert on PDF
//! structure (raw byte markers) and the font-routing logic rather than grepping the
//! rendered words; visual/qpdf validation is covered in the PR.

use super::font::Fonts;
use super::*;
use crate::export::{Chapter, ExportStyle, Part};

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
fn fonts_route_glyphs_by_coverage() {
    let fonts = Fonts::new(&plain());
    // ASCII + Latin/Cyrillic/Greek the serif covers → the body face.
    for c in ['A', 'é', 'д', 'α'] {
        assert_eq!(
            fonts.resolve(c, false, false).map(|(n, _)| n),
            Some("F1"),
            "{c} in body"
        );
    }
    // Decorative ornaments the serif lacks → the broad fallback.
    assert_eq!(
        fonts.resolve('❦', false, false).map(|(n, _)| n),
        Some("Ffb")
    );
    assert_eq!(
        fonts.resolve('⚜', false, false).map(|(n, _)| n),
        Some("Ffb")
    );
    assert!(fonts.covers('❦') && fonts.covers('д'));
    // CJK isn't in either bundled font (needs an author-supplied CJK font).
    assert_eq!(fonts.resolve('中', false, false), None, "CJK uncovered");
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
    let bytes = to_pdf_print(&doc(), w, h, &plain()).unwrap();
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
        &to_pdf_print(&mini, w, h, &plain()).unwrap(),
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
    for op in ["/ImSceneBreak Do", "/ImChapterHeading Do", "/ImTitle Do"] {
        assert!(has(&bytes, op), "{op} drawn");
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
    assert!(has(&bytes, "/ImCover Do"), "cover drawn");
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
    let bytes = to_pdf_print(&doc(), w, h, &style).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7") && bytes.ends_with(b"%%EOF\n"));
    assert!(has(&bytes, "/ImCover Do"), "cover drawn in the print PDF");
    assert!(has(&bytes, "MediaBox [0 0 432 648]"), "6x9 trim");
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
        Layout::print(w, h, &plain()),
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
    let bytes = to_pdf_print(&d, w, h, &plain()).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7") && bytes.ends_with(b"%%EOF\n"));
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
