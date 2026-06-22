//! Scrivener (`.scriv`) export (#192). A Scrivener 3 project is a *bundle
//! directory*, not a single file: a `<name>.scrivx` XML binder, a
//! `Files/version.txt` marker, and one `Files/Data/<UUID>/content.rtf` per
//! document. We map the export [`Document`] tree onto Scrivener's binder — a
//! "Manuscript" draft folder holding the front matter as text documents, each
//! part as a folder of chapter documents, and the back matter as trailing text
//! documents — so the structure (and `*`/`**` emphasis, as RTF) survives the
//! round trip into Scrivener. Grounded in the format of a real Scrivener 3
//! project; the bundle opens directly in Scrivener.app.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

use super::{classify_block, inline_runs, xml_escape, Chapter, Document, Part, Prose, SCENE_BREAK};

/// Write `doc` as a Scrivener `.scriv` bundle at `bundle` (the `<name>.scriv`
/// directory). Any existing bundle there is replaced so the export always
/// reflects the current project.
pub fn write_scriv(doc: &Document, bundle: &Path) -> std::io::Result<()> {
    if bundle.exists() {
        std::fs::remove_dir_all(bundle)?;
    }
    let data = bundle.join("Files").join("Data");
    std::fs::create_dir_all(&data)?;
    // The version marker Scrivener 3 writes for its project format.
    std::fs::write(bundle.join("Files").join("version.txt"), "23")?;

    // Build the binder tree, writing each document's content.rtf as we go. A
    // running counter seeds stable, unique UUIDs (binder item ↔ Data dir).
    let mut n = 0usize;
    let mut binder = String::new();
    binder.push_str(&format!(
        "<BinderItem UUID=\"{}\" Type=\"DraftFolder\"><Title>Manuscript</Title><Children>",
        uuid("draft")
    ));
    for chapter in &doc.front {
        binder.push_str(&text_item(chapter, &data, &mut n)?);
    }
    for part in &doc.parts {
        binder.push_str(&part_items(part, &data, &mut n)?);
    }
    for chapter in &doc.back {
        binder.push_str(&text_item(chapter, &data, &mut n)?);
    }
    binder.push_str("</Children></BinderItem>");
    // The two other special folders every Scrivener project carries, so it opens
    // without rebuilding them: a place for research, and the trash.
    binder.push_str(&format!(
        "<BinderItem UUID=\"{}\" Type=\"ResearchFolder\"><Title>Research</Title></BinderItem>",
        uuid("research")
    ));
    binder.push_str(&format!(
        "<BinderItem UUID=\"{}\" Type=\"TrashFolder\"><Title>Trash</Title></BinderItem>",
        uuid("trash")
    ));

    let scrivx = format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<ScrivenerProject Identifier=\"{id}\" Version=\"2.0\" Creator=\"Quill\">",
            "<Binder>{binder}</Binder></ScrivenerProject>"
        ),
        id = uuid("project"),
        binder = binder,
    );
    let name = bundle.file_stem().and_then(|s| s.to_str()).unwrap_or("Manuscript");
    std::fs::write(bundle.join(format!("{name}.scrivx")), scrivx)
}

/// A named part becomes a binder folder wrapping its chapters; an unnamed part
/// (ungrouped body chapters) contributes its chapters directly, with no folder.
fn part_items(part: &Part, data: &Path, n: &mut usize) -> std::io::Result<String> {
    let mut chapters = String::new();
    for chapter in &part.chapters {
        chapters.push_str(&text_item(chapter, data, n)?);
    }
    // An unnamed part (ungrouped body chapters) contributes its chapters directly,
    // with no folder; a named part wraps them in a binder folder.
    let Some(name) = &part.name else {
        return Ok(chapters);
    };
    let uuid = uuid(&format!("folder:{n}"));
    *n += 1;
    Ok(format!(
        "<BinderItem UUID=\"{uuid}\" Type=\"Folder\"><Title>{}</Title><Children>{chapters}</Children></BinderItem>",
        xml_escape(name),
    ))
}

/// Write one chapter's `content.rtf` and return its `<BinderItem>` (a Text
/// document, titled, marked to include when Scrivener compiles).
fn text_item(chapter: &Chapter, data: &Path, n: &mut usize) -> std::io::Result<String> {
    let id = uuid(&format!("doc:{n}"));
    *n += 1;
    let dir = data.join(&id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("content.rtf"), to_rtf(chapter))?;
    Ok(format!(
        concat!(
            "<BinderItem UUID=\"{id}\" Type=\"Text\"><Title>{title}</Title>",
            "<MetaData><IncludeInCompile>Yes</IncludeInCompile></MetaData></BinderItem>"
        ),
        id = id,
        title = xml_escape(&chapter.title),
    ))
}

/// Render a chapter's epigraph and prose as an RTF document (Times New Roman
/// 12pt). Each Markdown paragraph becomes an RTF paragraph; a `#` heading line
/// is set bold; `*`/`**` emphasis carries through as italic/bold runs.
fn to_rtf(chapter: &Chapter) -> String {
    let mut body = String::new();
    if let Some(epigraph) = &chapter.epigraph {
        for line in epigraph.text.lines() {
            body.push_str(&rtf_paragraph(&[(line.to_string(), false, true)]));
        }
        if let Some(by) = &epigraph.attribution {
            body.push_str(&rtf_paragraph(&[(format!("— {by}"), false, true)]));
        }
    }
    for block in chapter.body.split("\n\n") {
        match classify_block(block) {
            None => {}
            // A centered scene divider, the conventional Scrivener separator.
            Some(Prose::SceneBreak) => body.push_str(&format!("\\pard\\qc\\sa180 {SCENE_BREAK}\\par\n")),
            Some(Prose::Heading(heading)) => body.push_str(&rtf_paragraph(&[(heading, true, false)])),
            Some(Prose::Paragraph(text)) => body.push_str(&rtf_paragraph(&inline_runs(&text))),
        }
    }
    format!(
        "{{\\rtf1\\ansi\\ansicpg1252\\deff0{{\\fonttbl{{\\f0\\froman Times New Roman;}}}}\n\\f0\\fs24\n{body}}}"
    )
}

/// One RTF paragraph (`\pard … \par`) from a sequence of styled runs.
fn rtf_paragraph(runs: &[(String, bool, bool)]) -> String {
    let mut out = String::from("\\pard\\sa180 ");
    for (text, bold, italic) in runs {
        let escaped = rtf_escape(text);
        let tags = match (bold, italic) {
            (true, true) => "\\b\\i ",
            (true, false) => "\\b ",
            (false, true) => "\\i ",
            (false, false) => "",
        };
        if tags.is_empty() {
            out.push_str(&escaped);
        } else {
            out.push('{');
            out.push_str(tags);
            out.push_str(&escaped);
            out.push('}');
        }
    }
    out.push_str("\\par\n");
    out
}

/// Escape text for an RTF run: the control characters `\ { }`, and any
/// non-ASCII character as a `\uN?` Unicode escape (RTF's signed-16-bit form;
/// characters beyond the BMP fall back to `?`).
fn rtf_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            c if c.is_ascii() => out.push(c),
            c if (c as u32) <= 0xFFFF => {
                // RTF \u takes a signed 16-bit code unit; > 32767 wraps negative.
                out.push_str(&format!("\\u{}?", c as u32 as i16));
            }
            _ => out.push('?'),
        }
    }
    out
}

/// A stable UUID-shaped identifier derived from `seed` — deterministic, so the
/// same project structure exports the same IDs, with no RNG dependency.
fn uuid(seed: &str) -> String {
    let hi = hash(seed);
    let lo = hash(&format!("{seed}/lo"));
    format!(
        "{:08X}-{:04X}-{:04X}-{:04X}-{:012X}",
        (hi >> 32) as u32,
        (hi >> 16) as u16,
        hi as u16,
        (lo >> 48) as u16,
        lo & 0xFFFF_FFFF_FFFF,
    )
}

fn hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontmatter::Epigraph;

    fn epigraph(text: &str, by: Option<&str>) -> Epigraph {
        Epigraph { text: text.to_string(), attribution: by.map(str::to_string) }
    }

    fn chapter(title: &str, epigraph: Option<Epigraph>, body: &str) -> Chapter {
        Chapter { title: title.to_string(), epigraph, body: body.to_string() }
    }

    fn doc() -> Document {
        Document {
            title: "Red <State> Earth".into(),
            subtitle: None,
            author: Some("A. Writer".into()),
            series: None,
            epigraph: None,
            front: vec![chapter("Prologue", None, "Before it all.")],
            parts: vec![Part {
                name: Some("Part One".into()),
                chapters: vec![chapter(
                    "The Find",
                    Some(epigraph("A line.", Some("Anon"))),
                    "The locket *gleamed* and felt **heavy**.\n\n## A scene\n\nMore. 5 * 3 stays literal.",
                )],
            }],
            back: vec![chapter("Epilogue", None, "After.")],
        }
    }

    #[test]
    fn writes_a_bundle_with_binder_version_and_rtf() {
        let dir = std::env::temp_dir().join("quill-scriv-test").join("Book.scriv");
        write_scriv(&doc(), &dir).unwrap();

        // version marker + the .scrivx binder named after the bundle.
        assert_eq!(std::fs::read_to_string(dir.join("Files/version.txt")).unwrap(), "23");
        let scrivx = std::fs::read_to_string(dir.join("Book.scrivx")).unwrap();

        // Manuscript draft folder; the part is a folder, chapters are text docs,
        // in reading order (prologue → part → epilogue), titles XML-escaped.
        assert!(scrivx.contains("Type=\"DraftFolder\""), "{scrivx}");
        let prologue = scrivx.find("<Title>Prologue</Title>").unwrap();
        let part = scrivx.find("<Title>Part One</Title>").unwrap();
        let find = scrivx.find("<Title>The Find</Title>").unwrap();
        let epilogue = scrivx.find("<Title>Epilogue</Title>").unwrap();
        assert!(prologue < part && part < find && find < epilogue, "reading order:\n{scrivx}");
        assert!(scrivx.contains("Type=\"Folder\""), "part is a folder:\n{scrivx}");

        // One content.rtf per document; emphasis carried through, stray * literal.
        let mut rtfs = Vec::new();
        for entry in std::fs::read_dir(dir.join("Files/Data")).unwrap() {
            let rtf = entry.unwrap().path().join("content.rtf");
            if rtf.exists() {
                rtfs.push(std::fs::read_to_string(rtf).unwrap());
            }
        }
        assert_eq!(rtfs.len(), 3, "front + chapter + back");
        let find_rtf = rtfs.iter().find(|r| r.contains("gleamed")).unwrap();
        assert!(find_rtf.starts_with("{\\rtf1"), "valid RTF header:\n{find_rtf}");
        assert!(find_rtf.contains("{\\i gleamed}"), "italic run:\n{find_rtf}");
        assert!(find_rtf.contains("{\\b heavy}"), "bold run:\n{find_rtf}");
        assert!(find_rtf.contains("{\\i A line.}"), "epigraph italic:\n{find_rtf}");
        assert!(find_rtf.contains("5 * 3 stays literal"), "stray * verbatim:\n{find_rtf}");

        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn a_scene_break_becomes_a_centered_divider_in_rtf() {
        let rtf = to_rtf(&chapter("One", None, "Scene one.\n\n---\n\nScene two."));
        assert!(!rtf.contains("---"), "no literal rule: {rtf}");
        // A centered `#` divider between the scenes.
        assert!(rtf.contains("\\pard\\qc\\sa180 #\\par"), "centered divider: {rtf}");
        let one = rtf.find("Scene one.").unwrap();
        let hash = rtf.find("\\qc\\sa180 #").unwrap();
        let two = rtf.find("Scene two.").unwrap();
        assert!(one < hash && hash < two, "divider between scenes: {rtf}");
    }

    #[test]
    fn rtf_escapes_controls_and_unicode() {
        assert_eq!(rtf_escape("a\\b{c}"), "a\\\\b\\{c\\}");
        assert_eq!(rtf_escape("café"), "caf\\u233?");
    }

    #[test]
    fn an_unnamed_part_adds_no_folder() {
        let d = Document {
            title: "T".into(),
            subtitle: None,
            author: None,
            series: None,
            epigraph: None,
            front: vec![],
            parts: vec![Part { name: None, chapters: vec![chapter("One", None, "Body.")] }],
            back: vec![],
        };
        let dir = std::env::temp_dir().join("quill-scriv-unnamed").join("T.scriv");
        write_scriv(&d, &dir).unwrap();
        let scrivx = std::fs::read_to_string(dir.join("T.scrivx")).unwrap();
        assert!(!scrivx.contains("Type=\"Folder\""), "no part folder:\n{scrivx}");
        assert!(scrivx.contains("<Title>One</Title>"));
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }
}
