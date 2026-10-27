//! Embedded/subset fonts (#638, #709): `@font-face` declarations, the
//! true-italic sibling, glyph subsetting, and the `format(...)` hint.

use super::*;

#[test]
fn fonts_are_embedded_and_applied_via_the_stylesheet() {
    let d = Document {
        title: "T".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
        epigraph: None,
        half_title: false,
        series_books: Vec::new(),
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![chapter("C", None, "Body.")],
        }],
        back: vec![],
        story: None,
    };
    let mut style = ExportStyle {
        body_font: Some("b.otf".into()),
        heading_font: Some("h.ttf".into()),
        ..Default::default()
    };
    style.resolve_fonts(|p| Ok(format!("font:{p}").into_bytes()));
    let bytes = to_epub(&d, MODIFIED, &style).unwrap();

    // The font files are embedded under OEBPS/fonts/, named by role.
    assert!(
        names(&bytes).iter().any(|n| n == "OEBPS/fonts/body.otf"),
        "body font file"
    );
    assert!(
        names(&bytes).iter().any(|n| n == "OEBPS/fonts/heading.ttf"),
        "heading font file"
    );
    // The OPF manifest declares them with their media types.
    let opf = part(&bytes, "OEBPS/content.opf");
    assert!(
        opf.contains("href=\"fonts/body.otf\" media-type=\"font/otf\""),
        "{opf}"
    );
    assert!(
        opf.contains("href=\"fonts/heading.ttf\" media-type=\"font/ttf\""),
        "{opf}"
    );
    // The stylesheet declares @font-face and applies the families.
    let css = part(&bytes, "OEBPS/style.css");
    assert!(
        css.contains("@font-face") && css.contains("url(\"fonts/body.otf\")"),
        "{css}"
    );
    assert!(
        css.contains("body { font-family: \"b\""),
        "body family: {css}"
    );
    assert!(
        css.contains("h1, h2 { font-family: \"h\""),
        "heading family: {css}"
    );

    // Default style → no fonts embedded, no @font-face.
    let plain = to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap();
    assert!(
        !names(&plain).iter().any(|n| n.starts_with("OEBPS/fonts/")),
        "no fonts by default"
    );
    assert!(!part(&plain, "OEBPS/style.css").contains("@font-face"));
}

#[test]
fn a_true_italic_sibling_embeds_and_gets_an_italic_font_face() {
    // A body font with a `-italic` sibling on disk (#638): both files embed, and
    // the stylesheet declares an upright and an italic `@font-face` for the same
    // family with a `400 700` weight range — so <em>/<strong> resolve to the
    // right face instead of the reader faux-styling one weight.
    let d = Document {
        title: "T".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
        epigraph: None,
        half_title: false,
        series_books: Vec::new(),
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![chapter("C", None, "Body *emph* and **bold**.")],
        }],
        back: vec![],
        story: None,
    };
    let mut style = ExportStyle {
        body_font: Some("fonts/eb-garamond.ttf".into()),
        ..Default::default()
    };
    style.resolve_fonts(|path| match path {
        "fonts/eb-garamond.ttf" => Ok(b"upright".to_vec()),
        "fonts/eb-garamond-italic.ttf" => Ok(b"italic".to_vec()),
        other => Err(std::io::Error::other(format!("no {other}"))),
    });
    let bytes = to_epub(&d, MODIFIED, &style).unwrap();

    // Both files embed under OEBPS/fonts/.
    assert!(
        names(&bytes).iter().any(|n| n == "OEBPS/fonts/body.ttf"),
        "upright body font"
    );
    assert!(
        names(&bytes)
            .iter()
            .any(|n| n == "OEBPS/fonts/body-italic.ttf"),
        "true-italic body font"
    );
    // The OPF declares both.
    let opf = part(&bytes, "OEBPS/content.opf");
    assert!(opf.contains("href=\"fonts/body.ttf\""), "{opf}");
    assert!(opf.contains("href=\"fonts/body-italic.ttf\""), "{opf}");
    // The stylesheet has an upright and an italic @font-face, each spanning the
    // regular→bold weight range.
    let css = part(&bytes, "OEBPS/style.css");
    assert!(
        css.contains("font-style: normal; font-weight: 400 700; src: url(\"fonts/body.ttf\")"),
        "upright face: {css}"
    );
    assert!(
        css.contains(
            "font-style: italic; font-weight: 400 700; src: url(\"fonts/body-italic.ttf\")"
        ),
        "italic face: {css}"
    );
}

#[test]
fn embedded_fonts_are_subset_smaller_than_the_source() {
    // A real serif family is embedded; the package copy must be subset to the
    // book's glyphs (#709), so far smaller than the ~380 KB source, yet still
    // render every character the text uses — punctuation and the smart-typography
    // em-dash/curly quotes included.
    let source = include_bytes!("../../../../assets/fonts/LiberationSerif-Regular.ttf");
    let d = Document {
        title: "The Book".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
        epigraph: None,
        half_title: false,
        series_books: Vec::new(),
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![chapter(
                "One",
                None,
                "She paused — then, \u{201c}quietly,\u{201d} left. \u{2018}Café\u{2019}",
            )],
        }],
        back: vec![],
        story: None,
    };
    let mut style = ExportStyle {
        body_font: Some("fonts/body.ttf".into()),
        ..Default::default()
    };
    style.resolve_fonts(|_| Ok(source.to_vec()));
    let bytes = to_epub(&d, MODIFIED, &style).unwrap();

    let embedded = part_bytes(&bytes, "OEBPS/fonts/body.ttf");
    assert!(
        embedded.len() < source.len() / 4,
        "subset {} should be far smaller than source {}",
        embedded.len(),
        source.len()
    );
    // The subset still resolves every glyph actually used, via a rebuilt cmap.
    let face = ttf_parser::Face::parse(&embedded, 0).expect("subset parses");
    assert!(face.tables().cmap.is_some(), "subset keeps a cmap");
    for c in [
        'S', 'h', 'e', ',', '.', '\u{2014}', '\u{201c}', '\u{201d}', 'é',
    ] {
        assert!(face.glyph_index(c).is_some(), "subset covers {c:?}");
    }
}

#[test]
fn font_face_declarations_carry_a_format_hint() {
    // Each `@font-face src:` gets a `format(...)` hint matching the media type
    // (#709), so a reader that can't handle the format has a fallback signal.
    let d = Document {
        title: "T".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
        epigraph: None,
        half_title: false,
        series_books: Vec::new(),
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![chapter("C", None, "Body.")],
        }],
        back: vec![],
        story: None,
    };
    let mut style = ExportStyle {
        body_font: Some("b.otf".into()),
        heading_font: Some("h.ttf".into()),
        ..Default::default()
    };
    style.resolve_fonts(|p| Ok(format!("font:{p}").into_bytes()));
    let bytes = to_epub(&d, MODIFIED, &style).unwrap();
    let css = part(&bytes, "OEBPS/style.css");
    assert!(
        css.contains("src: url(\"fonts/body.otf\") format(\"opentype\");"),
        "otf hint: {css}"
    );
    assert!(
        css.contains("src: url(\"fonts/heading.ttf\") format(\"truetype\");"),
        "ttf hint: {css}"
    );
}
