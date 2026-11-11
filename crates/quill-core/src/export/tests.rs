//! The shared export helpers: block segmentation, classification, epigraphs,
//! and the Markdown writer.

use super::*;

fn epigraph(text: &str, by: Option<&str>) -> Epigraph {
    Epigraph {
        text: text.to_string(),
        attribution: by.map(str::to_string),
    }
}

fn chapter(title: &str, epigraph: Option<Epigraph>, body: &str) -> Chapter {
    Chapter {
        verse: false,
        title: title.to_string(),
        epigraph,
        body: body.to_string(),
        role: None,
    }
}

fn doc() -> Document {
    Document {
        title: "Red State Earth".into(),
        subtitle: Some("a novel".into()),
        author: Some("A. Writer".into()),
        series: Some("The Cycle".into()),
        publishing: PubMeta::default(),
        epigraph: Some(epigraph("In the beginning.", Some("Anon"))),
        half_title: false,
        series_books: Vec::new(),
        front: vec![chapter("Prologue", None, "Before it all.")],
        parts: vec![Part {
            name: Some("Part One".into()),
            chapters: vec![chapter(
                "The Find",
                Some(epigraph("A line.", None)),
                "The locket *gleamed*.",
            )],
        }],
        back: vec![],
        story: None,
        novel: None,
    }
}

#[test]
fn series_markdown_lists_books_in_order_under_the_series_title() {
    let book = |title: &str, body: &str| Document {
        title: title.into(),
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
            chapters: vec![chapter("Ch", None, body)],
        }],
        back: vec![],
        story: None,
        novel: None,
    };
    let md = to_markdown_series(
        &[book("Book One", "alpha"), book("Book Two", "beta")],
        Some("The Saga"),
        Some("A. Writer"),
    );
    assert!(md.starts_with("The Saga\n\n"), "{md}");
    let one = md.find("# Book One").expect("book one heading");
    let two = md.find("# Book Two").expect("book two heading");
    assert!(one < two, "books in order");
    assert!(md.contains("alpha") && md.contains("beta"));
}

#[test]
fn a_fenced_code_block_is_kept_whole_and_classified_preformatted() {
    // A fence with a blank line inside must not be split on the blank line.
    let body = "Intro line.\n\n```\nZENITH // 2053\n\nPRE-IGNIT\n```\n\nAfter.";
    let blocks = body_blocks(body);
    assert_eq!(blocks.len(), 3, "{blocks:?}");
    assert_eq!(&*blocks[0], "Intro line.");
    assert_eq!(&*blocks[2], "After.");
    // The fenced block is preformatted: fences gone, inner breaks preserved.
    match classify_block(&blocks[1], false) {
        Some(Prose::Preformatted(code)) => {
            assert_eq!(code, "ZENITH // 2053\n\nPRE-IGNIT");
        }
        other => panic!("expected Preformatted, got {other:?}"),
    }
}

#[test]
fn an_unterminated_fence_runs_to_the_end_of_the_body() {
    // No closing fence: everything after the opener is the preformatted block.
    let blocks = body_blocks("```\nlog one\nlog two");
    assert_eq!(blocks.len(), 1);
    match classify_block(&blocks[0], false) {
        Some(Prose::Preformatted(code)) => assert_eq!(code, "log one\nlog two"),
        other => panic!("expected Preformatted, got {other:?}"),
    }
}

#[test]
fn markdown_export_preserves_a_fenced_code_block_verbatim() {
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
                "Before.\n\n```\nLOG: --ok\n```\n\nAfter.",
            )],
        }],
        back: vec![],
        story: None,
        novel: None,
    };
    // The body is emitted verbatim, so the fence (and its literal `--`) survive.
    assert!(
        to_markdown(&d).contains("```\nLOG: --ok\n```"),
        "{}",
        to_markdown(&d)
    );
}

#[test]
fn smart_typography_glyphs_pass_through_to_markdown() {
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
        story: None,
        novel: None,
    };
    assert!(
        to_markdown(&d).contains("He said \u{201C}hi\u{2014}bye\u{2026}\u{201D}"),
        "curly glyphs reach the Markdown/Vellum output"
    );
}

#[test]
fn renders_the_book_header_and_epigraph() {
    let md = to_markdown(&doc());
    assert!(md.starts_with("The Cycle\n\n# Red State Earth\n\n*a novel*\n\nby A. Writer\n\n"));
    assert!(md.contains("> In the beginning.\n>\n> — Anon\n"), "{md}");
}

#[test]
fn renders_items_in_order_with_headings() {
    let md = to_markdown(&doc());
    let prologue = md.find("## Prologue").unwrap();
    let part = md.find("## Part One").unwrap();
    let chapter = md.find("## The Find").unwrap();
    assert!(
        prologue < part && part < chapter,
        "reading order preserved:\n{md}"
    );
    // The chapter epigraph renders before its body; prose is verbatim.
    let find = &md[chapter..];
    assert!(
        find.find("> A line.").unwrap() < find.find("The locket *gleamed*.").unwrap(),
        "{md}"
    );
}

#[test]
fn strips_a_body_heading_only_when_it_repeats_the_title() {
    // A leading `# Title` that matches is dropped (the compiled heading has it).
    assert_eq!(
        strip_repeated_title("# The Find\n\nProse.\n", "The Find"),
        "Prose.\n"
    );
    // A differing heading is kept (don't lose the author's text).
    assert_eq!(
        strip_repeated_title("# Prologue\n\nBefore.\n", "A Prologue"),
        "# Prologue\n\nBefore.\n"
    );
    // No leading heading → untouched.
    assert_eq!(
        strip_repeated_title("Just prose.\n", "Title"),
        "Just prose.\n"
    );
}

#[test]
fn parses_nested_and_plain_emphasis() {
    assert_eq!(
        inline_runs("plain"),
        vec![Run::new("plain", TextStyle::Plain)]
    );
    assert_eq!(
        inline_runs("a *b* c"),
        vec![
            Run::new("a ", TextStyle::Plain),
            Run::new("b", TextStyle::Italic),
            Run::new(" c", TextStyle::Plain)
        ]
    );
    assert_eq!(
        inline_runs("**bold**"),
        vec![Run::new("bold", TextStyle::Bold)]
    );
    // Underscores are literal (not emphasis), so snake_case survives.
    assert_eq!(
        inline_runs("snake_case"),
        vec![Run::new("snake_case", TextStyle::Plain)]
    );
    // A stray/unbalanced `*` is kept literal, not turned into runaway italics.
    assert_eq!(
        inline_runs("5 * 3 = 15"),
        vec![Run::new("5 * 3 = 15", TextStyle::Plain)]
    );
}

#[test]
fn xml_escape_escapes_the_markup_chars() {
    assert_eq!(xml_escape("a & b < c > d"), "a &amp; b &lt; c &gt; d");
}

#[test]
fn xml_escape_escapes_quotes_for_attribute_safety() {
    // Quotes must be escaped: `xml_escape` feeds attribute values (the EPUB
    // `alt="{}"`, #818), where a raw `"` would close the attribute early and
    // yield non-well-formed XHTML that epubcheck rejects.
    assert_eq!(
        xml_escape("The \"Real\" Story"),
        "The &quot;Real&quot; Story"
    );
    assert_eq!(xml_escape("it's a 'trap'"), "it&#39;s a &#39;trap&#39;");
}

#[test]
fn xml_escape_strips_forbidden_control_chars_but_keeps_legal_whitespace() {
    // C0 controls XML 1.0 forbids can't be escaped — left raw they corrupt
    // the document — so they're stripped.
    assert_eq!(xml_escape("a\u{0}b\u{7}c\u{1f}d"), "abcd");
    // Tab, newline, and carriage return are legal XML whitespace — kept.
    assert_eq!(xml_escape("a\tb\nc\rd"), "a\tb\nc\rd");
}

#[test]
fn recognizes_thematic_breaks_but_not_prose() {
    // The scene-break forms an author might type.
    for br in ["---", "***", "___", "* * *", "- - -", "----"] {
        assert!(is_thematic_break(br), "{br:?} is a scene break");
        assert!(
            matches!(classify_block(br, false), Some(Prose::SceneBreak)),
            "{br:?}"
        );
    }
    // Prose that merely contains the same characters is not a break.
    for prose in [
        "a — dash",
        "5 * 3 = 15",
        "--",
        "well - maybe",
        "* one bullet",
    ] {
        assert!(!is_thematic_break(prose), "{prose:?} is prose, not a break");
    }
    // Headings and paragraphs still classify as before.
    assert!(
        matches!(classify_block("## A scene", false), Some(Prose::Heading(h)) if h == "A scene")
    );
    assert!(classify_block("  ", false).is_none());
}

#[test]
fn markdown_keeps_a_scene_break_as_a_thematic_break() {
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
            chapters: vec![chapter("One", None, "Scene one.\n\n---\n\nScene two.")],
        }],
        back: vec![],
        story: None,
        novel: None,
    };
    assert!(
        to_markdown(&d).contains("Scene one.\n\n---\n\nScene two."),
        "{}",
        to_markdown(&d)
    );
}

#[test]
fn a_multiline_epigraph_quotes_every_line() {
    let d = Document {
        title: "T".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
        epigraph: Some(epigraph("Roses are red\n\nViolets are blue", None)),
        half_title: false,
        series_books: Vec::new(),
        front: vec![],
        parts: vec![],
        back: vec![],
        story: None,
        novel: None,
    };
    let md = to_markdown(&d);
    assert!(
        md.contains("> Roses are red\n>\n> Violets are blue\n"),
        "{md}"
    );
}
