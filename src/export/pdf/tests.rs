//! Tests for the PDF writer (#463) and the print-interior profile (#464).

use super::*;
use crate::export::{Chapter, Part};

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
    let bytes = to_pdf(&doc()).unwrap();
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
    let s = text(&to_pdf(&doc()).unwrap());
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
    let bytes = to_pdf(&doc()).unwrap();
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
    let s = text(&to_pdf(&doc()).unwrap());
    // title + prologue + part page + chapter + afterword, no blanks inserted.
    assert_eq!(count(&s, "/Type /Page /Parent"), 5);
}

#[test]
fn builds_a_nested_bookmark_outline() {
    let s = text(&to_pdf(&doc()).unwrap());
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
    let s = text(&to_pdf_print(&doc(), w, h).unwrap());
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
    let screen = count(&text(&to_pdf(&mini).unwrap()), "/Type /Page /Parent");
    let print = count(
        &text(&to_pdf_print(&mini, w, h).unwrap()),
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
    let visible = shown(&to_pdf_print(&d, w, h).unwrap());
    // Title page mentions each once; running heads on continuation pages add more.
    assert!(
        count(&visible, "TITLEX") + count(&visible, "AUTHORX") > 2,
        "running heads render on continuation pages: {visible}"
    );
}
