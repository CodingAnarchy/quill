//! Tests for the PDF writer (#463) and the print-interior profile (#464).

use super::*;
use crate::export::{Chapter, ExportStyle, Part};

/// The default (no `[style]`) — screen/print output is the plain #463/#464 baseline.
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

/// A book exercising every section kind: front matter, a named part with one
/// chapter (epigraph + emphasis + a scene break + a sub-heading), and back
/// matter.
fn doc() -> Document {
    Document {
        title: "Red State Earth".into(),
        subtitle: Some("a novel".into()),
        author: Some("A. Writer".into()),
        series: Some("The Cycle".into()),
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

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Reconstruct the visible text by concatenating every `(...) Tj` body.
fn shown(bytes: &[u8]) -> String {
    let s = text(bytes);
    let b = s.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'(' {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let mut body = String::new();
        while j < b.len() && b[j] != b')' {
            if b[j] == b'\\' {
                j += 2;
                continue;
            }
            body.push(b[j] as char);
            j += 1;
        }
        if s[j..].starts_with(") Tj") {
            out.push_str(&body);
        }
        i = j + 1;
    }
    out
}

fn count(s: &str, needle: &str) -> usize {
    s.matches(needle).count()
}

#[test]
fn is_a_well_formed_pdf_with_a_locatable_xref() {
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    let s = text(&bytes);
    assert!(s.starts_with("%PDF-1.7"), "header");
    assert!(s.trim_end().ends_with("%%EOF"), "trailer marker");
    assert!(s.contains("/Type /Catalog") && s.contains("/Root 1 0 R"));
    assert!(s.contains("/Type /Pages") && s.contains("trailer"));
    let sx = s.rfind("startxref").expect("startxref");
    let off: usize = s[sx + "startxref".len()..]
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(&s[off..off + 4], "xref", "startxref locates the table");
}

#[test]
fn screen_pdf_uses_the_standard_times_faces() {
    let s = text(&to_pdf(&doc(), &plain()).unwrap());
    for base in [
        "Times-Roman",
        "Times-Bold",
        "Times-Italic",
        "Times-BoldItalic",
    ] {
        assert!(s.contains(&format!("/BaseFont /{base}")), "{base} declared");
    }
    assert_eq!(
        count(&s, "/Encoding /WinAnsiEncoding"),
        4,
        "every face WinAnsi-encoded"
    );
    assert!(!s.contains("/FontFile2"), "screen profile embeds nothing");
    assert!(s.contains("MediaBox [0 0 612 792]"), "US-Letter");
}

#[test]
fn carries_metadata_structure_and_emphasis() {
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    let visible = shown(&bytes);
    for piece in [
        "Red State Earth",
        "a novel",
        "by A. Writer",
        "Prologue",
        "Part One",
        "The Find",
        "Afterword",
    ] {
        assert!(visible.contains(piece), "{piece:?} missing from: {visible}");
    }
    let raw = text(&bytes);
    assert!(
        raw.contains("/F2 ") && raw.contains("/F3 "),
        "bold + italic faces used"
    );
    assert!(!visible.contains('*'), "no literal asterisks: {visible}");
    assert!(
        visible.contains(SCENE_BREAK) && !visible.contains("---"),
        "scene divider: {visible}"
    );
}

#[test]
fn each_section_starts_on_its_own_page_in_the_screen_profile() {
    let s = text(&to_pdf(&doc(), &plain()).unwrap());
    // title + prologue + part page + chapter + afterword, no blanks inserted.
    assert_eq!(count(&s, "/Type /Page /Parent"), 5);
}

#[test]
fn builds_a_nested_bookmark_outline() {
    let s = text(&to_pdf(&doc(), &plain()).unwrap());
    assert!(
        s.contains("/PageMode /UseOutlines"),
        "opens with the outline"
    );
    for title in [
        "/Title (Prologue)",
        "/Title (Part One)",
        "/Title (The Find)",
        "/Title (Afterword)",
    ] {
        assert!(s.contains(title), "{title} bookmarked");
    }
    let at = s.find("/Title (Part One)").unwrap();
    let item = &s[at..at + 200.min(s.len() - at)];
    assert!(
        item.contains("/First ") && item.contains("/Count 1"),
        "part nests its chapter: {item}"
    );
}

#[test]
fn a_series_carries_every_book_in_order_under_the_series_title() {
    let book = |title: &str| Document {
        title: title.into(),
        subtitle: None,
        author: None,
        series: None,
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
    let visible = shown(&bytes);
    assert!(
        visible.contains("The Saga") && visible.contains("by A. Writer"),
        "series title page"
    );
    let one = visible.find("Book One").expect("book one");
    let two = visible.find("Book Two").expect("book two");
    assert!(one < two, "books in reading order");
    let s = text(&bytes);
    assert!(
        s.contains("/Title (Book One)") && s.contains("/Title (Book Two)"),
        "books bookmarked"
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
fn print_pdf_embeds_the_fonts_at_the_chosen_trim() {
    let (w, h) = trim_points("6x9").unwrap();
    let s = text(&to_pdf_print(&doc(), w, h, &plain()).unwrap());
    // The bundled faces are embedded as TrueType with a descriptor + program.
    assert!(s.contains("/Subtype /TrueType"), "embedded TrueType font");
    assert_eq!(count(&s, "/FontFile2"), 4, "four font programs embedded");
    assert_eq!(
        count(&s, "/FontDescriptor"),
        8,
        "a descriptor per face (dict + descriptor ref)"
    );
    assert!(
        s.contains("/BaseFont /LiberationSerif"),
        "the embedded face is named"
    );
    assert!(
        !s.contains("/BaseFont /Times-Roman"),
        "no reliance on a viewer font"
    );
    // 6×9 inches = 432×648 points.
    assert!(
        s.contains("MediaBox [0 0 432 648]"),
        "trim size 6x9 = 432x648 pts"
    );
    // Bookmark destinations jump to the top of the trim-height page, not 792.
    assert!(
        s.contains("/XYZ null 648 null"),
        "outline dest uses the trim height"
    );
}

#[test]
fn print_pdf_starts_chapters_on_a_recto_inserting_a_blank() {
    // Title page is the only thing before the first section. A recto start for
    // that section forces a blank verso between them.
    let mini = Document {
        title: "T".into(),
        subtitle: None,
        author: Some("Auth".into()),
        series: None,
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![ch("One", None, "Body.")],
        }],
        back: vec![],
    };
    let (w, h) = trim_points("6x9").unwrap();
    let screen = count(
        &text(&to_pdf(&mini, &plain()).unwrap()),
        "/Type /Page /Parent",
    );
    let print = count(
        &text(&to_pdf_print(&mini, w, h, &plain()).unwrap()),
        "/Type /Page /Parent",
    );
    assert_eq!(screen, 2, "title + chapter");
    assert_eq!(print, 3, "title + blank verso + recto chapter");
}

#[test]
fn print_pdf_draws_running_heads() {
    // A body long enough to spill onto a second (non-opening) page, which is
    // where running heads appear.
    let long = "The river ran on past the town and into the hills beyond. ".repeat(120);
    let d = Document {
        title: "TITLEX".into(),
        subtitle: None,
        author: Some("AUTHORX".into()),
        series: None,
        epigraph: None,
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![ch("One", None, &long)],
        }],
        back: vec![],
    };
    let (w, h) = trim_points("6x9").unwrap();
    let visible = shown(&to_pdf_print(&d, w, h, &plain()).unwrap());
    // Title page mentions each once; running heads on continuation pages add more.
    assert!(
        count(&visible, "TITLEX") + count(&visible, "AUTHORX") > 2,
        "running heads render on continuation pages: {visible}"
    );
}

/// A bundled face reused as a "configured" font in tests (any valid TTF works).
const TEST_FONT: &[u8] = include_bytes!("../../../assets/fonts/LiberationSerif-Regular.ttf");

/// The x of the text matrix immediately before `needle` — to check line placement.
fn tm_x_before(s: &str, needle: &str) -> f32 {
    let at = s.find(needle).expect("needle present");
    let tm = s[..at].rfind("1 0 0 1 ").expect("a Tm precedes it");
    s[tm + "1 0 0 1 ".len()..]
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn style_glyphs_and_ornaments_carry_into_the_pdf() {
    // WinAnsi glyphs (asterisks, an asterisk ornament) render; the scene break
    // replaces the default `#`.
    let style = ExportStyle {
        scene_break: Some("* * *".into()),
        title_ornament: Some("*".into()),
        ..Default::default()
    };
    let visible = shown(&to_pdf(&doc(), &style).unwrap());
    assert!(
        visible.contains("* * *"),
        "configured scene break: {visible}"
    );
    assert!(!visible.contains(" # "), "default glyph replaced");
    assert!(visible.contains('*'), "renderable ornament drawn");
    assert!(
        pdf_style_warnings(&style).is_empty(),
        "all-WinAnsi style, no warnings"
    );
}

#[test]
fn a_preset_styles_the_pdf_within_the_font_set() {
    // `classic` resolves (via apply_preset) to a `* * *` scene break, a drop cap,
    // and `❦` ornaments. The WinAnsi glyphs and drop cap render; the decorative
    // ornament can't (it's outside the serif fonts) and is reported.
    let mut style = ExportStyle {
        preset: Some("classic".into()),
        ..Default::default()
    };
    style.apply_preset();
    let bytes = to_pdf(&doc(), &style).unwrap();
    let visible = shown(&bytes);
    assert!(visible.contains("* * *"), "classic scene break renders");
    assert!(
        text(&bytes).contains(&format!("/F1 {} Tf", BODY_LEADING * 3.0)),
        "classic drop cap"
    );
    assert!(!visible.contains('❦'), "the ❦ ornament can't render in PDF");
    assert!(
        pdf_style_warnings(&style).iter().any(|w| w.contains('❦')),
        "the skipped ornament is reported"
    );
}

#[test]
fn pdf_style_warnings_flag_images_and_unrenderable_glyphs() {
    // A non-WinAnsi scene break falls back to `#` and is reported.
    let glyphy = ExportStyle {
        scene_break: Some("❖".into()),
        ..Default::default()
    };
    let visible = shown(&to_pdf(&doc(), &glyphy).unwrap());
    assert!(visible.contains('#'), "falls back to the default divider");
    assert!(
        pdf_style_warnings(&glyphy)
            .iter()
            .any(|w| w.contains("font set")),
        "glyph reported"
    );
    // An SVG image ornament can't be rasterized for PDF and is reported.
    let mut svg = ExportStyle {
        scene_break_image: Some("art/break.svg".into()),
        ..Default::default()
    };
    svg.resolve_images(|_| Ok(b"<svg/>".to_vec()));
    assert!(
        pdf_style_warnings(&svg).iter().any(|w| w.contains("SVG")),
        "SVG image ornament reported as unsupported"
    );
    // A raster (PNG) ornament is supported → no warning.
    let mut png = ExportStyle {
        scene_break_image: Some("art/break.png".into()),
        ..Default::default()
    };
    png.resolve_images(|_| Ok(png_bytes(2, 2, 255)));
    assert!(
        pdf_style_warnings(&png).is_empty(),
        "a PNG ornament is embeddable, no warning: {:?}",
        pdf_style_warnings(&png)
    );
}

/// Encode a tiny `w`×`h` solid-red PNG with the given alpha (255 = opaque) — a real
/// raster the PDF writer can decode and embed.
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
    // Opaque images → no soft mask; transparent ones add an /SMask.
    style.resolve_images(|p| Ok(png_bytes(4, 4, if p.contains("break") { 128 } else { 255 })));
    let bytes = to_pdf(&doc(), &style).unwrap();
    let s = text(&bytes);
    // Three image XObjects, registered for the pages and drawn in the content.
    assert_eq!(count(&s, "/Subtype /Image"), 4, "3 images + 1 soft mask");
    assert_eq!(
        count(&s, "/SMask"),
        1,
        "only the transparent scene break gets a mask"
    );
    assert!(s.contains("/XObject <<"), "images registered in resources");
    for name in ["/ImSceneBreak Do", "/ImChapterHeading Do", "/ImTitle Do"] {
        assert!(s.contains(name), "{name} drawn in the content stream");
    }
    // The scene-break image replaces the glyph — no `#` divider remains.
    assert!(
        !shown(&bytes).contains('#'),
        "scene glyph replaced by the image"
    );
}

#[test]
fn no_image_ornaments_means_no_xobjects() {
    let s = text(&to_pdf(&doc(), &ExportStyle::default()).unwrap());
    assert!(
        !s.contains("/Subtype /Image"),
        "no images embedded by default"
    );
    assert!(!s.contains("/XObject"), "no XObject resource dict");
}

#[test]
fn drop_cap_sets_a_large_initial_only_when_styled() {
    let cap = format!("/F1 {} Tf", BODY_LEADING * 3.0);
    let off = ExportStyle::default();
    assert!(
        !text(&to_pdf(&doc(), &off).unwrap()).contains(&cap),
        "no drop cap by default"
    );
    let on = ExportStyle {
        drop_cap: Some(true),
        ..Default::default()
    };
    assert!(
        text(&to_pdf(&doc(), &on).unwrap()).contains(&cap),
        "drop cap initial is set large"
    );
}

#[test]
fn heading_alignment_places_the_chapter_title() {
    let book = |align: Option<&str>| {
        let style = ExportStyle {
            heading_align: align.map(str::to_string),
            ..Default::default()
        };
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part {
                name: None,
                chapters: vec![ch("HEADX", None, "")],
            }],
            back: vec![],
        };
        text(&to_pdf(&d, &style).unwrap())
    };
    // Left-aligned headings sit at the text-column left edge (the 72pt margin);
    // centered (the default) sits further in.
    let left_x = tm_x_before(&book(Some("left")), "(HEADX)");
    let center_x = tm_x_before(&book(None), "(HEADX)");
    assert!(
        (left_x - 72.0).abs() < 0.5,
        "left heading at the margin: {left_x}"
    );
    assert!(center_x > 80.0, "centered heading is indented: {center_x}");
}

#[test]
fn a_configured_body_font_is_embedded_even_in_the_screen_pdf() {
    let mut style = ExportStyle {
        body_font: Some("body.ttf".into()),
        ..Default::default()
    };
    let warnings = style.resolve_fonts(|_| Ok(TEST_FONT.to_vec()));
    assert!(warnings.is_empty(), "font resolves: {warnings:?}");
    let s = text(&to_pdf(&doc(), &style).unwrap());
    // The screen profile embeds nothing by default; a configured font switches it
    // to the embedded plan.
    assert!(s.contains("/FontFile2"), "configured body font embedded");
    assert!(s.contains("/Subtype /TrueType"), "embedded as TrueType");
    assert!(
        !s.contains("/BaseFont /Times-Roman"),
        "no longer the standard face"
    );
}
