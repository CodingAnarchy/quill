//! XHTML content rendering and `[style]` parity: emphasis/scene breaks, smart
//! typography, fenced code, title-page metadata, ornaments/drop caps, style
//! presets, body CSS, and the generated front-matter leaves (half-title,
//! series list).

use super::*;

#[test]
fn emphasis_and_scene_breaks_render_as_markup_not_literals() {
    let bytes = to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap();
    let chap = part(&bytes, "OEBPS/chap0004.xhtml");
    assert!(chap.contains("<em>gleamed</em>"), "italic: {chap}");
    assert!(chap.contains("<strong>heavy</strong>"), "bold");
    assert!(!chap.contains('*'), "no literal asterisks");
    assert!(chap.contains("<h2>A scene</h2>"), "inner heading");
    assert!(
        chap.contains("scene-break") && !chap.contains("---"),
        "scene break, no rule"
    );
    // Chapter epigraph rendered set-apart before the prose.
    assert!(chap.contains("class=\"epigraph\"") && chap.contains("A line."));
    assert!(
        chap.find("A line.").unwrap() < chap.find("gleamed").unwrap(),
        "epigraph first"
    );
}

#[test]
fn title_page_shows_metadata_and_book_epigraph() {
    let title = part(
        &to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap(),
        "OEBPS/title0001.xhtml",
    );
    assert!(title.contains("The Cycle") && title.contains("Red &lt;State&gt; Earth"));
    assert!(title.contains("a novel") && title.contains("by A. Writer"));
    assert!(title.contains("In the beginning.") && title.contains("— Anon"));
}

#[test]
fn smart_typography_glyphs_pass_through() {
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
            chapters: vec![chapter(
                "C",
                None,
                "He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}",
            )],
        }],
        back: vec![],
    };
    let chap = part(
        &to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap(),
        "OEBPS/chap0002.xhtml",
    );
    assert!(
        chap.contains("He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}"),
        "curly glyphs"
    );
}

#[test]
fn a_fenced_block_renders_as_verbatim_preformatted_xhtml() {
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
            chapters: vec![chapter(
                "C",
                None,
                "Before.\n\n```\nLOG // 2053\n  indented\n```\n\nAfter.",
            )],
        }],
        back: vec![],
    };
    let chap = part(
        &to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap(),
        "OEBPS/chap0002.xhtml",
    );
    // Whitespace and line breaks preserved; not wrapped in a <p>.
    assert!(
        chap.contains("<pre>LOG // 2053\n  indented</pre>"),
        "{chap}"
    );
    assert!(!chap.contains("<p>LOG"), "code is not a paragraph:\n{chap}");
}

#[test]
fn scene_break_renders_the_styled_glyph_default_unchanged() {
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
            chapters: vec![chapter("C", None, "A.\n\n---\n\nB.")],
        }],
        back: vec![],
    };
    // Default style → the plain centered `#`, exactly as before.
    let plain = part(
        &to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap(),
        "OEBPS/chap0002.xhtml",
    );
    assert!(
        plain.contains("<p class=\"scene-break\">#</p>"),
        "default glyph: {plain}"
    );
    // A configured glyph replaces it (and is XML-escaped).
    let style = ExportStyle {
        scene_break: Some("* * &".into()),
        ..Default::default()
    };
    let styled = part(
        &to_epub(&d, MODIFIED, &style).unwrap(),
        "OEBPS/chap0002.xhtml",
    );
    assert!(
        styled.contains("<p class=\"scene-break\">* * &amp;</p>"),
        "styled+escaped: {styled}"
    );
    assert!(!styled.contains(">#</p>"), "default glyph replaced");
}

#[test]
fn heading_alignment_and_drop_cap_render_via_css() {
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
            chapters: vec![chapter("One", None, "First para.\n\nSecond para.")],
        }],
        back: vec![],
    };
    let style = ExportStyle {
        heading_align: Some("left".into()),
        drop_cap: Some(true),
        ..Default::default()
    };
    let bytes = to_epub(&d, MODIFIED, &style).unwrap();
    let css = part(&bytes, "OEBPS/style.css");
    assert!(
        css.contains("h1, h2 { text-align: left; }"),
        "alignment: {css}"
    );
    assert!(
        css.contains(".opening::first-letter {"),
        "drop-cap rule: {css}"
    );
    // Only the chapter's first paragraph carries the `opening` class.
    let chap = part(&bytes, "OEBPS/chap0002.xhtml");
    assert_eq!(chap.matches("class=\"opening\"").count(), 1, "{chap}");
    assert!(
        chap.contains("<p class=\"opening\">First para.</p>"),
        "{chap}"
    );
    assert!(chap.contains("<p>Second para.</p>"), "{chap}");

    // Default style → no alignment override, no drop cap, no opening class.
    let plain = to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap();
    assert!(!part(&plain, "OEBPS/style.css").contains("text-align: left"));
    assert!(!part(&plain, "OEBPS/style.css").contains("first-letter"));
    assert!(!part(&plain, "OEBPS/chap0002.xhtml").contains("opening"));
}

#[test]
fn title_and_part_ornaments_render_when_configured() {
    // `doc()` has a title page (with author) and a named part ("Part One").
    let style = ExportStyle {
        title_ornament: Some("❧".into()),
        part_ornament: Some("* * &".into()),
        ..Default::default()
    };
    let bytes = to_epub(&doc(), MODIFIED, &style).unwrap();
    let css = part(&bytes, "OEBPS/style.css");
    assert!(css.contains(".ornament {"), "ornament rule: {css}");
    // Title-page ornament (inside the centered title block).
    let title = part(&bytes, "OEBPS/title0001.xhtml");
    assert!(title.contains("<p class=\"ornament\">❧</p>"), "{title}");
    // Part-divider ornament, XML-escaped.
    let part_doc = part(&bytes, "OEBPS/part0003.xhtml");
    assert!(
        part_doc.contains("<p class=\"ornament\">* * &amp;</p>"),
        "{part_doc}"
    );

    // Default style → no ornaments, no rule.
    let plain = to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap();
    assert!(!part(&plain, "OEBPS/style.css").contains(".ornament"));
    assert!(!part(&plain, "OEBPS/title0001.xhtml").contains("ornament"));
}

#[test]
fn a_preset_styles_the_export_and_explicit_keys_still_override_it() {
    // `classic` bundles a scene break, drop cap, and ornaments; one explicit
    // key overrides just that setting and the rest of the preset stands.
    let mut style = ExportStyle {
        preset: Some("classic".into()),
        scene_break: Some("~ ~ ~".into()),
        ..Default::default()
    };
    assert!(style.apply_preset().is_none());
    let bytes = to_epub(&doc(), MODIFIED, &style).unwrap();

    let css = part(&bytes, "OEBPS/style.css");
    assert!(css.contains(".opening::first-letter {"), "drop cap: {css}");
    assert!(css.contains(".ornament {"), "ornaments: {css}");

    // The preset's title ornament renders; the overridden scene break wins.
    assert!(part(&bytes, "OEBPS/title0001.xhtml").contains("<p class=\"ornament\">❦</p>"));
    let chap = part(&bytes, "OEBPS/chap0004.xhtml");
    assert!(
        chap.contains("<p class=\"scene-break\">~ ~ ~</p>"),
        "{chap}"
    );
    assert!(
        chap.contains("class=\"opening\""),
        "drop cap applied: {chap}"
    );

    // `modern` is the contrasting bundle: left headings, a bullet break, no
    // drop cap.
    let mut modern = ExportStyle {
        preset: Some("modern".into()),
        ..Default::default()
    };
    assert!(modern.apply_preset().is_none());
    let mbytes = to_epub(&doc(), MODIFIED, &modern).unwrap();
    let mcss = part(&mbytes, "OEBPS/style.css");
    assert!(mcss.contains("text-align: left;"), "modern align: {mcss}");
    assert!(
        !mcss.contains("first-letter"),
        "modern has no drop cap: {mcss}"
    );
    assert!(part(&mbytes, "OEBPS/chap0004.xhtml").contains("<p class=\"scene-break\">•</p>"));

    // A genre preset from the library (#482) renders its own bundle too:
    // `fantasy` is a diamond scene break with fleur-de-lis ornaments.
    let mut fantasy = ExportStyle {
        preset: Some("fantasy".into()),
        ..Default::default()
    };
    assert!(fantasy.apply_preset().is_none());
    let fbytes = to_epub(&doc(), MODIFIED, &fantasy).unwrap();
    assert!(part(&fbytes, "OEBPS/title0001.xhtml").contains("<p class=\"ornament\">⚜</p>"));
    assert!(part(&fbytes, "OEBPS/chap0004.xhtml").contains("<p class=\"scene-break\">❖</p>"));
}

#[test]
fn body_css_justifies_hyphenates_and_controls_widows_and_orphans() {
    // Print/ebook readiness (#650): the body paragraph rule carries justification,
    // automatic hyphenation (with the -webkit- prefix for older WebKit readers), and
    // widow/orphan protection; inset epigraph blocks stay ragged, not justified.
    let css = part(
        &to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap(),
        "OEBPS/style.css",
    );
    let p_rule = css
        .lines()
        .find(|l| l.starts_with("p {"))
        .expect("the body paragraph rule");
    assert!(
        p_rule.contains("text-align: justify"),
        "justified: {p_rule}"
    );
    assert!(p_rule.contains("hyphens: auto"), "hyphenated: {p_rule}");
    assert!(
        p_rule.contains("-webkit-hyphens: auto"),
        "webkit hyphens: {p_rule}"
    );
    assert!(p_rule.contains("orphans: 2"), "orphans: {p_rule}");
    assert!(p_rule.contains("widows: 2"), "widows: {p_rule}");
    // Inset epigraph paragraphs are not justified (a set-apart block).
    assert!(
        css.contains(".epigraph p { text-align: initial"),
        "epigraphs stay ragged: {css}"
    );
}

#[test]
fn a_half_title_page_precedes_the_title_page_when_enabled() {
    // #820: with `half_title` on, a bare half-title leaf carrying only the book
    // title is generated ahead of the full title page — spine order cover →
    // half-title → title → copyright → body.
    let d = Document {
        half_title: true,
        ..doc()
    };
    let bytes = to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap();
    // The default style configures no cover image, so the half-title is the first
    // spine entry (`halftitle0001`) and the full title page follows (`title0002`).
    let half = part(&bytes, "OEBPS/halftitle0001.xhtml");
    assert!(
        half.contains("<body epub:type=\"halftitlepage\">"),
        "the half-title carries the EPUB structural term: {half}"
    );
    assert!(
        half.contains("half-title-page") && half.contains("Red &lt;State&gt; Earth"),
        "the half-title shows only the book title: {half}"
    );
    // It sits ahead of the full title page in the spine (the OPF item order is the
    // spine order): halftitle0001 before title0002.
    let opf = part(&bytes, "OEBPS/content.opf");
    assert!(
        opf.find("halftitle0001").unwrap() < opf.find("title0002").unwrap(),
        "half-title precedes the full title page: {opf}"
    );
    // And it is a bare page — no author, subtitle, or series line leaks onto it.
    assert!(
        !half.contains("by A. Writer") && !half.contains("The Cycle"),
        "the half-title is bare: {half}"
    );
}

#[test]
fn a_half_title_page_is_absent_by_default() {
    // The default leaves the front matter unchanged (cover → title → …), so the
    // conventional half-title stays opt-in.
    let bytes = to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap();
    assert!(
        !names(&bytes).iter().any(|n| n.contains("halftitle")),
        "no half-title page unless enabled"
    );
}

#[test]
fn a_series_book_generates_an_also_in_this_series_page() {
    // #820: a book that is part of a series carries an "also in this series" page
    // listing the *other* books — never itself.
    let d = Document {
        series_books: vec![
            "Red <State> Earth".into(),
            "Blue State Moon".into(),
            "Green State Sun".into(),
        ],
        ..doc()
    };
    let bytes = to_epub(&d, MODIFIED, &ExportStyle::default()).unwrap();
    // No cover / no copyright metadata, so: title0001 → series0002 → body.
    let page = part(&bytes, "OEBPS/series0002.xhtml");
    assert!(
        page.contains(SERIES_LIST_HEADING),
        "the page carries the heading: {page}"
    );
    assert!(
        page.contains("Blue State Moon") && page.contains("Green State Sun"),
        "the other books are listed: {page}"
    );
    assert!(
        !page.contains("Red &lt;State&gt; Earth"),
        "the book never lists itself: {page}"
    );
    // It slots into the front matter after the title page.
    let opf = part(&bytes, "OEBPS/content.opf");
    assert!(
        opf.find("series0002").unwrap() > opf.find("title0001").unwrap(),
        "series list follows the title page: {opf}"
    );
    assert!(
        page.contains("<body epub:type=\"frontmatter\">"),
        "the series list is front matter: {page}"
    );
}

#[test]
fn a_standalone_book_omits_the_series_page() {
    // A book with no other series titles (a standalone novel) generates no "also
    // in this series" page, so a standalone export is unchanged.
    let bytes = to_epub(&doc(), MODIFIED, &ExportStyle::default()).unwrap();
    assert!(
        !names(&bytes).iter().any(|n| n.contains("series0")),
        "no series-list page for a standalone book"
    );
    // Even when the list holds only the book's own title, it lists nothing — so no
    // page is emitted.
    let only_self = Document {
        series_books: vec!["Red <State> Earth".into()],
        ..doc()
    };
    let bytes = to_epub(&only_self, MODIFIED, &ExportStyle::default()).unwrap();
    assert!(
        !names(&bytes).iter().any(|n| n.contains("series0")),
        "a lone self-title lists nothing, so no page"
    );
}
