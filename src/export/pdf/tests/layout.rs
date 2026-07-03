//! Layout tests for the PDF writer: well-formedness, preformatted blocks, print
//! trim/recto/bleed, justification/hyphenation, style warnings, and the /Info dict.

// The bleed/trim geometry asserts exact point offsets (the bleed, `2.0 * bleed`)
// from deterministic layout arithmetic — an exact compare is the intended check.
#![allow(clippy::float_cmp)]

use super::super::wrap::{words_of, wrap_lines, Hyphenate, Word};
use super::*;

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
fn each_section_starts_on_its_own_page_in_the_screen_profile() {
    // Contents + prologue + part + chapter + afterword, each its own page (the title
    // page shares no page). The Contents page is the front-matter TOC (#603).
    let bytes = to_pdf(&doc(), &plain()).unwrap();
    assert_eq!(count(&bytes, "/Type /Page /Parent"), 6);
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
    pdf.y = BODY_LEADING.mul_add(0.5, pdf.layout.text_bottom());
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
    pdf.y = leading.mul_add(3.5, pdf.layout.text_bottom());
    let brk = pdf.widow_orphan_break(n, leading);
    assert_eq!(
        brk,
        Some(3),
        "break before the 4th line so lines 4 and 5 travel together"
    );
    // Sanity: a paragraph that splits leaving two lines on each side needs no help.
    pdf.y = leading.mul_add(1.5, pdf.layout.text_bottom()); // two lines fit
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
