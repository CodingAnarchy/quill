//! Scrivener `.scriv` import (#402): the inverse of `export::scriv`. Read the
//! `.scrivx` binder for the Manuscript draft folder's order and titles, and each
//! text document's `Files/Data/<UUID>/content.rtf`, converting RTF bold/italic
//! runs to `**`/`*`. See the crate module doc; the shared XML scanner lives in
//! `mod.rs`.

use std::path::Path;

use crate::export::{push_run, Run, SCENE_BREAK};

use super::{
    attr_value, element_text, fallback_chapter, finalize_titles, for_each_child, join_paragraphs,
    wrap_emphasis, Dropped, Imported, ImportedChapter,
};

/// Parse a Scrivener `.scriv` bundle directory: read its `.scrivx` binder for the
/// Manuscript draft folder's reading order and titles, and each text document's
/// `Files/Data/<UUID>/content.rtf`. The bundle is read only. Errors if there's no
/// `.scrivx` binder (i.e. it isn't a Scrivener project).
pub fn parse_scriv(bundle: &Path) -> std::io::Result<Imported> {
    let scrivx_path = std::fs::read_dir(bundle)?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("scrivx"))
        })
        .ok_or_else(|| std::io::Error::other("not a Scrivener project (no .scrivx binder)"))?;
    let scrivx = std::fs::read_to_string(&scrivx_path)?;
    // Scrivener's project name is the bundle's name; it doesn't store a separate
    // title/author in the binder Quill writes.
    let title = bundle
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    let data = bundle.join("Files").join("Data");
    Ok(parse_binder(&scrivx, title, |uuid| {
        std::fs::read_to_string(data.join(uuid).join("content.rtf")).ok()
    }))
}

/// The pure core of [`parse_scriv`]: walk the binder XML's Manuscript draft folder
/// into ordered chapters, pulling each text document's body from `read_rtf(uuid)`.
/// Folders are flattened into their text documents (the chapter model is flat);
/// the Research and Trash folders are skipped (only the draft folder is walked).
fn parse_binder(
    scrivx: &str,
    title: Option<String>,
    read_rtf: impl Fn(&str) -> Option<String>,
) -> Imported {
    let mut chapters = Vec::new();
    let mut unreadable_bodies = 0;
    let mut dropped = Dropped::default();
    if let Some(draft) = draft_folder(scrivx).and_then(children_group) {
        collect_items(
            draft,
            &read_rtf,
            &mut chapters,
            &mut unreadable_bodies,
            &mut dropped,
        );
    }
    // No draft documents → one empty fallback chapter (`.scriv` has no preamble).
    fallback_chapter(&mut chapters, &title, String::new());
    finalize_titles(&mut chapters);
    Imported {
        title,
        author: None,
        chapters,
        unreadable_bodies,
        dropped,
    }
}

/// The inner XML of the `DraftFolder` binder item (the "Manuscript" folder).
fn draft_folder(scrivx: &str) -> Option<&str> {
    let mut draft = None;
    for_each_child(scrivx, "BinderItem", |attrs, inner| {
        if draft.is_none() && attr_value(attrs, "Type").as_deref() == Some("DraftFolder") {
            draft = inner;
        }
    });
    draft
}

/// Walk a binder item's direct children, in order: each `Text` document becomes a
/// chapter (its `content.rtf` converted from RTF); a folder recurses (its title is
/// dropped — the chapter list is flat). Nesting-aware (via [`for_each_child`]) so
/// folders nest correctly.
///
/// A listed `Text` document whose `content.rtf` can't be read (`read_rtf`
/// returns `None`) still becomes a chapter — with an empty body — but is tallied
/// in `unreadable` rather than silently passed off as empty, so the caller can
/// warn that content was dropped (#511).
fn collect_items(
    children: &str,
    read_rtf: &impl Fn(&str) -> Option<String>,
    out: &mut Vec<ImportedChapter>,
    unreadable: &mut usize,
    dropped: &mut Dropped,
) {
    for_each_child(children, "BinderItem", |attrs, inner| {
        let Some(inner) = inner else { return };
        if attr_value(attrs, "Type").as_deref() == Some("Text") {
            let uuid = attr_value(attrs, "UUID").unwrap_or_default();
            let body = match read_rtf(&uuid) {
                Some(rtf) => rtf_to_markdown(&rtf, dropped),
                None => {
                    *unreadable += 1;
                    String::new()
                }
            };
            out.push(ImportedChapter {
                title: element_text(inner, "Title").unwrap_or_default(),
                body,
            });
        }
        // Recurse into any children (a folder's documents, or a text item's own).
        if let Some(grandchildren) = children_group(inner) {
            collect_items(grandchildren, read_rtf, out, unreadable, dropped);
        }
    });
}

/// The inner XML of the first direct `<Children>` element, or `None`.
fn children_group(item: &str) -> Option<&str> {
    let mut group = None;
    for_each_child(item, "Children", |_attrs, inner| {
        if group.is_none() {
            group = inner;
        }
    });
    group
}

/// Convert a document's RTF to Markdown: each `\par` paragraph becomes a line,
/// bold/italic runs (`\b` / `\i`, including `{\i …}` groups) become `**`/`*`, and
/// the centered scene-break glyph becomes a `---` thematic break. Best-effort: it
/// skips font/colour/style tables and unknown control words, decodes `\uN`/`\'XX`,
/// and otherwise emits text — enough for prose RTF (round-trips Quill's export).
///
/// Structural content that has no place in the flat prose model is tallied in
/// `dropped` rather than silently lost (#646): footnotes (`\footnote`) convert to
/// inline `[^N]` markers (their body text is skipped), and images (`\pict`) and
/// tables (`\trowd`/`\intbl` rows) are counted as dropped.
fn rtf_to_markdown(rtf: &str, dropped: &mut Dropped) -> String {
    let chars: Vec<char> = rtf.chars().collect();
    let mut paras: Vec<String> = Vec::new();
    let mut runs: Vec<Run> = Vec::new();
    let mut cur = String::new();
    let (mut bold, mut italic, mut skipping) = (false, false, false);
    // Table rows (`\intbl`/`\row`) come in contiguous runs; count each contiguous
    // block as one table, resetting the flag when a plain paragraph (`\pard`) ends
    // the table's cells.
    let mut in_table = false;
    let mut stack: Vec<(bool, bool, bool)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                push_run(&mut runs, &mut cur, bold, italic);
                stack.push((bold, italic, skipping));
                i += 1;
            }
            '}' => {
                push_run(&mut runs, &mut cur, bold, italic);
                if let Some((b, it, sk)) = stack.pop() {
                    (bold, italic, skipping) = (b, it, sk);
                }
                i += 1;
            }
            '\\' => {
                i += 1;
                let Some(&c) = chars.get(i) else { break };
                if c.is_ascii_alphabetic() {
                    let start = i;
                    while chars.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
                        i += 1;
                    }
                    let word: String = chars[start..i].iter().collect();
                    let num_start = i;
                    if chars.get(i).is_some_and(|&c| c == '-') {
                        i += 1;
                    }
                    while chars.get(i).is_some_and(char::is_ascii_digit) {
                        i += 1;
                    }
                    let param: Option<i64> =
                        chars[num_start..i].iter().collect::<String>().parse().ok();
                    if chars.get(i) == Some(&' ') {
                        i += 1; // a single trailing space delimits a control word
                    }
                    match word.as_str() {
                        "b" => {
                            push_run(&mut runs, &mut cur, bold, italic);
                            bold = param != Some(0);
                        }
                        "i" => {
                            push_run(&mut runs, &mut cur, bold, italic);
                            italic = param != Some(0);
                        }
                        "plain" => {
                            push_run(&mut runs, &mut cur, bold, italic);
                            (bold, italic) = (false, false);
                        }
                        "par" => {
                            push_run(&mut runs, &mut cur, bold, italic);
                            paras.push(runs_to_markdown(&runs));
                            runs.clear();
                        }
                        // A plain paragraph reset ends any table we were inside.
                        "pard" => in_table = false,
                        // Table cells: count each contiguous run of table rows as
                        // one dropped table (the cell text isn't carried through).
                        "intbl" | "trowd" => {
                            if !in_table {
                                in_table = true;
                                dropped.tables += 1;
                            }
                        }
                        // A footnote becomes an inline marker; its body is skipped.
                        "footnote" => {
                            push_run(&mut runs, &mut cur, bold, italic);
                            dropped.footnotes += 1;
                            runs.push(Run::new(format!("[^{}]", dropped.footnotes), false, false));
                            skipping = true;
                        }
                        // An embedded image/object is dropped; count it, then skip
                        // its (binary) group content like any skip destination.
                        "pict" | "object" => {
                            dropped.images += 1;
                            skipping = true;
                        }
                        "line" | "tab" => {
                            if !skipping {
                                cur.push(' ');
                            }
                        }
                        "u" => {
                            if let Some(ch) =
                                param.and_then(|p| char::from_u32(p.rem_euclid(65536) as u32))
                            {
                                if !skipping {
                                    cur.push(ch);
                                }
                            }
                            // Skip the single substitution char that follows \uN.
                            if chars
                                .get(i)
                                .is_some_and(|&c| c != '\\' && c != '{' && c != '}')
                            {
                                i += 1;
                            }
                        }
                        w if is_skip_destination(w) => skipping = true,
                        _ => {} // unknown control word — no text
                    }
                } else {
                    match c {
                        '\\' | '{' | '}' => {
                            if !skipping {
                                cur.push(c);
                            }
                            i += 1;
                        }
                        '*' => {
                            skipping = true; // an ignorable destination group
                            i += 1;
                        }
                        '\'' => {
                            let hex: String =
                                chars[i + 1..(i + 3).min(chars.len())].iter().collect();
                            i += 3;
                            if !skipping {
                                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                                    cur.push(char::from(byte)); // ANSI byte (Latin-1)
                                }
                            }
                        }
                        _ => i += 1,
                    }
                }
            }
            '\n' | '\r' => i += 1, // raw newlines are not text in RTF
            c => {
                if !skipping {
                    cur.push(c);
                }
                i += 1;
            }
        }
    }
    push_run(&mut runs, &mut cur, bold, italic);
    if !runs.is_empty() {
        paras.push(runs_to_markdown(&runs));
    }
    // The centered scene-break glyph round-trips back to a thematic break.
    for p in &mut paras {
        if p.trim() == SCENE_BREAK {
            *p = "---".to_string();
        }
    }
    join_paragraphs(&paras)
}

/// Render parsed RTF runs to Markdown, each wrapped per its bold/italic flags.
fn runs_to_markdown(runs: &[Run]) -> String {
    runs.iter()
        .map(|r| wrap_emphasis(&r.text, r.bold, r.italic))
        .collect()
}

/// RTF destination groups whose text isn't document content (font/colour/style
/// tables, metadata) — skipped wholesale.
fn is_skip_destination(word: &str) -> bool {
    matches!(
        word,
        "fonttbl"
            | "colortbl"
            | "stylesheet"
            | "info"
            | "header"
            | "footer"
            | "pgdsctbl"
            | "listtable"
            | "listoverridetable"
            | "rsidtbl"
            | "generator"
            | "themedata"
            | "colorschememapping"
            | "latentstyles"
            | "datastore"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal RTF document body (one paragraph) like `export::scriv` writes.
    fn rtf(body: &str) -> String {
        format!("{{\\rtf1\\ansi\\deff0{{\\fonttbl{{\\f0\\froman Times New Roman;}}}}\n\\f0\\fs24\n\\pard\\sa180 {body}\\par\n}}")
    }

    /// Convert RTF to Markdown, discarding the dropped-element tally — for the
    /// tests that only care about the prose.
    fn md(rtf: &str) -> String {
        rtf_to_markdown(rtf, &mut Dropped::default())
    }

    #[test]
    fn rtf_converts_emphasis_paragraphs_and_unicode() {
        let body = "The locket {\\i gleamed} and felt {\\b heavy}. caf\\u233 ?";
        let out = md(&rtf(body));
        assert_eq!(out, "The locket *gleamed* and felt **heavy**. café\n");
    }

    #[test]
    fn rtf_skips_the_font_table_and_decodes_escapes() {
        // The fonttbl text ("Times New Roman;") must not leak into the body.
        let out = md(&rtf("a \\{brace\\} and 5 \\\\ slash"));
        assert_eq!(out, "a {brace} and 5 \\ slash\n");
        assert!(
            !out.contains("Times New Roman"),
            "font table skipped: {out}"
        );
    }

    #[test]
    fn rtf_scene_break_glyph_becomes_a_thematic_break() {
        let body = format!(
            "{{\\rtf1\\ansi\\deff0\n\\pard\\sa180 Scene one.\\par\n\\pard\\qc\\sa180 {SCENE_BREAK}\\par\n\\pard\\sa180 Scene two.\\par\n}}"
        );
        let out = md(&body);
        assert_eq!(out, "Scene one.\n\n---\n\nScene two.\n");
    }

    #[test]
    fn rtf_clean_document_reports_nothing_dropped() {
        let mut dropped = Dropped::default();
        rtf_to_markdown(&rtf("Just clean prose."), &mut dropped);
        assert_eq!(dropped, Dropped::default());
    }

    #[test]
    fn rtf_footnote_becomes_inline_marker_and_is_counted() {
        // A Word-style footnote: the reference mark, then the footnote body group
        // (whose text must not leak into the prose).
        let body = "A claim{\\super \\chftn}{\\footnote \\pard\\plain \\chftn The source.} stands.";
        let mut dropped = Dropped::default();
        let out = rtf_to_markdown(&rtf(body), &mut dropped);
        assert_eq!(out, "A claim[^1] stands.\n", "footnote body is not inlined");
        assert!(!out.contains("The source"), "footnote text is skipped");
        assert_eq!(dropped.footnotes, 1);
        assert_eq!(dropped.notes(), 1);
        assert_eq!(dropped.lost(), 0, "a footnote is converted, not lost");
    }

    #[test]
    fn rtf_image_is_counted_as_dropped() {
        let body = "Before {\\pict\\pngblip 89504e47} after.";
        let mut dropped = Dropped::default();
        let out = rtf_to_markdown(&rtf(body), &mut dropped);
        assert!(!out.contains("89504e47"), "image bytes are skipped: {out}");
        assert_eq!(dropped.images, 1);
        assert_eq!(dropped.lost(), 1);
    }

    #[test]
    fn rtf_table_is_counted_once_per_contiguous_block() {
        // Two rows of one table, then a plain paragraph, then a second table.
        let body = "{\\rtf1\\ansi\n\
             \\trowd\\intbl a\\cell\\row\n\
             \\trowd\\intbl b\\cell\\row\n\
             \\pard Prose.\\par\n\
             \\trowd\\intbl c\\cell\\row\n\
             }";
        let mut dropped = Dropped::default();
        rtf_to_markdown(body, &mut dropped);
        assert_eq!(dropped.tables, 2, "two contiguous table blocks");
    }

    #[test]
    fn binder_flattens_folders_skips_research_and_trash_keeps_order() {
        let scrivx = r#"<?xml version="1.0"?><ScrivenerProject><Binder>
<BinderItem UUID="d" Type="DraftFolder"><Title>Manuscript</Title><Children>
<BinderItem UUID="u1" Type="Text"><Title>One</Title></BinderItem>
<BinderItem UUID="f1" Type="Folder"><Title>Part One</Title><Children>
<BinderItem UUID="u2" Type="Text"><Title>Two</Title></BinderItem>
</Children></BinderItem>
</Children></BinderItem>
<BinderItem UUID="r" Type="ResearchFolder"><Title>Research</Title></BinderItem>
<BinderItem UUID="t" Type="TrashFolder"><Title>Trash</Title></BinderItem>
</Binder></ScrivenerProject>"#;
        let out = parse_binder(scrivx, Some("My Novel".into()), |uuid| match uuid {
            "u1" => Some(rtf("Alpha.")),
            "u2" => Some(rtf("Beta.")),
            _ => None,
        });
        assert_eq!(out.title.as_deref(), Some("My Novel"));
        let titles: Vec<&str> = out.chapters.iter().map(|c| c.title.as_str()).collect();
        // The folder is flattened; its text doc follows the top-level one; Research
        // and Trash are skipped.
        assert_eq!(titles, vec!["One", "Two"]);
        assert_eq!(out.chapters[0].body, "Alpha.\n");
        assert_eq!(out.chapters[1].body, "Beta.\n");
    }

    #[test]
    fn binder_flattens_deeply_nested_folders_in_order() {
        // A folder nested two deep, plus sibling text docs at each level, exercises
        // the nesting-aware walk over `for_each_child` (depth in both directions).
        let scrivx = r#"<?xml version="1.0"?><ScrivenerProject><Binder>
<BinderItem UUID="d" Type="DraftFolder"><Title>Manuscript</Title><Children>
<BinderItem UUID="u1" Type="Text"><Title>One</Title></BinderItem>
<BinderItem UUID="f1" Type="Folder"><Title>Part One</Title><Children>
<BinderItem UUID="u2" Type="Text"><Title>Two</Title></BinderItem>
<BinderItem UUID="f2" Type="Folder"><Title>Section A</Title><Children>
<BinderItem UUID="u3" Type="Text"><Title>Three</Title></BinderItem>
</Children></BinderItem>
<BinderItem UUID="u4" Type="Text"><Title>Four</Title></BinderItem>
</Children></BinderItem>
<BinderItem UUID="u5" Type="Text"><Title>Five</Title></BinderItem>
</Children></BinderItem>
</Binder></ScrivenerProject>"#;
        let out = parse_binder(scrivx, Some("My Novel".into()), |uuid| {
            Some(rtf(&format!("Body of {uuid}.")))
        });
        let titles: Vec<&str> = out.chapters.iter().map(|c| c.title.as_str()).collect();
        // Depth-first reading order; every nested folder flattened to its docs.
        assert_eq!(titles, vec!["One", "Two", "Three", "Four", "Five"]);
        assert_eq!(out.chapters[2].body, "Body of u3.\n");
    }

    #[test]
    fn round_trips_a_quill_scriv_export() {
        use crate::export::scriv::write_scriv;
        use crate::export::{Chapter, Document, Part};
        let doc = Document {
            title: "Round Trip".into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: Default::default(),
            epigraph: None,
            front: vec![Chapter {
                title: "Prologue".into(),
                epigraph: None,
                body: "Before *it* all.".into(),
                role: None,
            }],
            parts: vec![Part {
                name: Some("Part One".into()),
                chapters: vec![Chapter {
                    title: "The Find".into(),
                    epigraph: None,
                    body: "The locket **gleamed**.".into(),
                    role: None,
                }],
            }],
            back: vec![Chapter {
                title: "Epilogue".into(),
                epigraph: None,
                body: "After.".into(),
                role: None,
            }],
        };
        let dir =
            crate::testutil::scratch_dir("scriv-import", "round-trip").join("Round Trip.scriv");
        write_scriv(&doc, &dir).unwrap();
        let out = parse_scriv(&dir).unwrap();

        assert_eq!(
            out.title.as_deref(),
            Some("Round Trip"),
            "title from the bundle name"
        );
        // Binder reading order, folder flattened (the part title isn't a chapter).
        let titles: Vec<&str> = out.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["Prologue", "The Find", "Epilogue"]);
        assert_eq!(
            out.chapters[0].body, "Before *it* all.\n",
            "italic round-trips"
        );
        assert_eq!(
            out.chapters[1].body, "The locket **gleamed**.\n",
            "bold round-trips"
        );
    }

    #[test]
    fn parse_scriv_rejects_a_dir_without_a_binder() {
        let dir = crate::testutil::scratch_dir("scriv-import", "no-binder");
        assert!(
            parse_scriv(&dir).is_err(),
            "a directory with no .scrivx is rejected"
        );
    }

    #[test]
    fn binder_counts_documents_whose_content_is_unreadable() {
        // Two listed Text documents; the second's content.rtf can't be read.
        let scrivx = r#"<?xml version="1.0"?><ScrivenerProject><Binder>
<BinderItem UUID="d" Type="DraftFolder"><Title>Manuscript</Title><Children>
<BinderItem UUID="u1" Type="Text"><Title>One</Title></BinderItem>
<BinderItem UUID="u2" Type="Text"><Title>Two</Title></BinderItem>
</Children></BinderItem>
</Binder></ScrivenerProject>"#;
        let out = parse_binder(scrivx, Some("My Novel".into()), |uuid| match uuid {
            "u1" => Some(rtf("Alpha.")),
            _ => None, // "u2" is listed but its content.rtf is missing/unreadable
        });
        // Both chapters are kept, in order; the unreadable one comes in empty.
        let titles: Vec<&str> = out.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["One", "Two"]);
        assert_eq!(out.chapters[0].body, "Alpha.\n");
        assert_eq!(out.chapters[1].body, "");
        // …and the drop is surfaced rather than swallowed.
        assert_eq!(out.unreadable_bodies, 1);
    }

    #[test]
    fn parse_scriv_reports_a_missing_binder_document() {
        // Build a minimal `.scriv` bundle, then remove one document's content so
        // it's listed in the binder but unreadable on disk (#511).
        use std::fs;

        let dir = crate::testutil::scratch_dir("scriv-import", "missing-doc").join("Gap.scriv");
        let data = dir.join("Files").join("Data");
        fs::create_dir_all(data.join("u1")).unwrap();
        fs::create_dir_all(data.join("u2")).unwrap();
        fs::write(data.join("u1").join("content.rtf"), rtf("Alpha.")).unwrap();
        // u2's content.rtf is deliberately never written — the body is unreadable.
        let scrivx = r#"<?xml version="1.0"?><ScrivenerProject><Binder>
<BinderItem UUID="d" Type="DraftFolder"><Title>Manuscript</Title><Children>
<BinderItem UUID="u1" Type="Text"><Title>One</Title></BinderItem>
<BinderItem UUID="u2" Type="Text"><Title>Two</Title></BinderItem>
</Children></BinderItem>
</Binder></ScrivenerProject>"#;
        fs::write(dir.join("Gap.scrivx"), scrivx).unwrap();

        let out = parse_scriv(&dir).unwrap();
        let titles: Vec<&str> = out.chapters.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["One", "Two"]);
        assert_eq!(out.chapters[1].body, "", "the missing document is empty");
        assert_eq!(
            out.unreadable_bodies, 1,
            "the unreadable document is reported, not silently dropped"
        );
    }
}
