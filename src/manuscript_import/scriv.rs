//! Scrivener `.scriv` import (#402): the inverse of `export::scriv`. Read the
//! `.scrivx` binder for the Manuscript draft folder's order and titles, and each
//! text document's `Files/Data/<UUID>/content.rtf`, converting RTF bold/italic
//! runs to `**`/`*`. See the crate module doc; the shared XML scanner lives in
//! `mod.rs`.

use std::path::Path;

use crate::export::{push_run, Run, TextStyle, SCENE_BREAK};

use super::{
    attr_value, element_text, fallback_chapter, finalize_titles, for_each_child,
    for_each_child_counted, join_paragraphs, wrap_emphasis, Dropped, Imported, ImportedChapter,
};
use std::path::Component;

/// The largest `\ucN` substitution count honored. RTF's `\uc` sets how many ANSI
/// fallback chars trail each `\uN` Unicode escape; real documents use 0–2 (Word
/// and Scrivener emit `\uc1`, one `\'XX` byte per glyph). An out-of-range value
/// (hostile or corrupt, e.g. `즙999`) would make [`skip_substitution`] eat
/// that many trailing chars, silently swallowing prose — so it's clamped to this
/// sane ceiling, above any legitimate value yet far short of eating a paragraph
/// (#1139).
const MAX_UC: usize = 2;

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
    // A `BinderItem` recovered past because it was unbalanced (its `</BinderItem>`
    // missing) is a listed document whose body can't be read — surface it in
    // `unreadable` rather than let it vanish silently (#1139).
    let recovered = for_each_child_counted(children, "BinderItem", |attrs, inner| {
        let Some(inner) = inner else { return };
        if attr_value(attrs, "Type").as_deref() == Some("Text") {
            let uuid = attr_value(attrs, "UUID").unwrap_or_default();
            // Reject a traversing/absolute UUID before it's ever joined onto the
            // bundle path: a `read_rtf` keyed off it must never read outside the
            // bundle. A rejected document still becomes a counted empty chapter.
            let body = safe_uuid(&uuid).and_then(read_rtf).map_or_else(
                || {
                    *unreadable += 1;
                    String::new()
                },
                |rtf| rtf_to_markdown(&rtf, dropped),
            );
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
    *unreadable += recovered;
}

/// Accept a binder `UUID` only if it's a single, path-safe file-name component,
/// so joining it onto `Files/Data/` can never escape the bundle (#1139 security).
/// The binder is untrusted input: a hostile `UUID="../../../secret"` (or absolute
/// `/etc/...`, or one with a `/`/`\` separator) would otherwise read a
/// `content.rtf` *outside* the bundle into a chapter. Rejects an empty UUID, one
/// with any path separator, `..`, or a leading `.`/root/prefix component; accepts
/// exactly one normal component (a real Scrivener UUID is a hyphenated hex string).
fn safe_uuid(uuid: &str) -> Option<&str> {
    if uuid.is_empty()
        || uuid.contains('/')
        || uuid.contains('\\')
        || uuid.contains('\0')
        || uuid == ".."
        || uuid == "."
    {
        return None;
    }
    // A single normal path component and nothing else — no `..`, root, prefix, or
    // current-dir component that `Path::join` would resolve outside the bundle.
    let mut comps = Path::new(uuid).components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(c)), None) if c == uuid => Some(uuid),
        _ => None,
    }
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
    // `\ucN` sets how many substitution bytes follow each `\uN` Unicode escape
    // (default 1) — the count of "ANSI equivalent" chars to skip after the glyph.
    let mut uc: usize = 1;
    // A `\f1` (monospace) run marks preformatted content the writer produced from
    // a fenced block (#522); track it per-run and per-paragraph so the paragraph
    // can be re-wrapped as a Markdown code fence on `\par`.
    let mut mono = false;
    let mut para_mono = false;
    let mut stack: Vec<(bool, bool, bool, bool)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' => {
                push_run(&mut runs, &mut cur, bold, italic);
                stack.push((bold, italic, skipping, mono));
                i += 1;
            }
            '}' => {
                push_run(&mut runs, &mut cur, bold, italic);
                if let Some((b, it, sk, mo)) = stack.pop() {
                    (bold, italic, skipping, mono) = (b, it, sk, mo);
                }
                i += 1;
            }
            '\\' => {
                i += 1;
                let Some(&c) = chars.get(i) else { break };
                if c.is_ascii_alphabetic() {
                    let start = i;
                    while chars.get(i).is_some_and(char::is_ascii_alphabetic) {
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
                            let text = runs_to_markdown(&runs);
                            // A monospace (`\f1`) paragraph is a fenced/preformatted
                            // block (#522); re-wrap it as a Markdown code fence so
                            // its interior line breaks and fence survive the import.
                            paras.push(if para_mono { fence(&text) } else { text });
                            runs.clear();
                            para_mono = false;
                        }
                        // A plain paragraph reset ends any table we were inside, and
                        // clears the monospace run (a new paragraph starts plain).
                        "pard" => {
                            in_table = false;
                            mono = false;
                        }
                        // Font select: `\f1` is the writer's monospace fence font;
                        // any other `\fN` (e.g. `\f0`) returns to prose.
                        "f" => mono = param == Some(1),
                        // Unicode substitution-char skip count for the `\uN` escapes
                        // that follow (RTF default is 1).
                        "uc" => {
                            if let Some(n) = param.filter(|&n| n >= 0) {
                                // Clamp to `MAX_UC` so an out-of-range count can't
                                // make `skip_substitution` swallow trailing prose.
                                uc = (n as usize).min(MAX_UC);
                            }
                        }
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
                            runs.push(Run::new(
                                format!("[^{}]", dropped.footnotes),
                                TextStyle::Plain,
                            ));
                            skipping = true;
                        }
                        // An embedded image/object is dropped; count it, then skip
                        // its (binary) group content like any skip destination.
                        "pict" | "object" => {
                            dropped.images += 1;
                            skipping = true;
                        }
                        // `\line` is a hard break: map it to a real newline so
                        // multi-line preformatted content survives (#522) rather
                        // than collapsing to one line; `\tab` stays a space.
                        "line" | "tab" => {
                            if !skipping {
                                if mono {
                                    para_mono = true;
                                }
                                cur.push(if word == "line" { '\n' } else { ' ' });
                            }
                        }
                        "u" => {
                            if let Some(ch) =
                                param.and_then(|p| char::from_u32(p.rem_euclid(65536) as u32))
                            {
                                if !skipping {
                                    if mono {
                                        para_mono = true;
                                    }
                                    cur.push(ch);
                                }
                            }
                            // Skip the `\uc`-count ANSI substitution chars that
                            // follow `\uN`. Each substitution is a single literal
                            // char OR a `\'XX` hex byte — the standard Word/Scrivener
                            // form is `\uN\'XX` (e.g. em-dash `舒\'97`). The old
                            // guard bailed on the leading `\`, leaving the `\'XX` to
                            // be re-decoded and doubling every such glyph (#402).
                            i = skip_substitution(&chars, i, uc);
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
                                    cur.push(cp1252_byte(byte)); // ANSI byte (cp1252)
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
                    if mono {
                        para_mono = true;
                    }
                    cur.push(c);
                }
                i += 1;
            }
        }
    }
    push_run(&mut runs, &mut cur, bold, italic);
    if !runs.is_empty() {
        let text = runs_to_markdown(&runs);
        paras.push(if para_mono { fence(&text) } else { text });
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

/// Advance past the `count` ANSI substitution "code points" that trail a `\uN`
/// Unicode escape, starting at `i` in `chars`. Each substitution is either a
/// literal char (`\uN?` — Quill's own export) or a `\'XX` hex-byte escape (the
/// standard Word/Scrivener `\uN\'XX` form). Consuming the whole `\'XX` here is
/// what stops the byte being re-decoded into a second, garbage glyph (#402).
/// Stops early at a group boundary (`{`/`}`) or a non-substitution control word.
fn skip_substitution(chars: &[char], mut i: usize, count: usize) -> usize {
    for _ in 0..count {
        match chars.get(i) {
            // A `\'XX` byte: consume the `\`, `'`, and two hex digits together.
            Some('\\') if chars.get(i + 1) == Some(&'\'') => i += 4,
            // A single literal substitution char (a group boundary `{`/`}` or the
            // start of any other control word `\` ends the substitution instead).
            Some(c) if !matches!(c, '{' | '}' | '\\') => i += 1,
            _ => break,
        }
    }
    i.min(chars.len())
}

/// Wrap `text` as a Markdown fenced code block, mirroring the writer's monospace
/// (`\f1`) paragraph (#522). Trailing whitespace on the block is trimmed so the
/// fence sits flush; interior line breaks (from `\line`) are preserved.
fn fence(text: &str) -> String {
    format!("```\n{}\n```", text.trim_end_matches(['\n', ' ']))
}

/// Decode one `\'XX` ANSI byte to a `char`. Word/Scrivener write cp1252
/// (`\ansicpg1252`), whose 0x80–0x9F range holds the typographic glyphs (smart
/// quotes, dashes, ellipsis) rather than Latin-1's C1 control characters; the
/// rest of the range coincides with Latin-1, so `char::from` covers it. Mapping
/// this range through the cp1252 table fixes those glyphs on foreign import
/// (#402). Undefined cp1252 slots (0x81/0x8D/0x8F/0x90/0x9D) fall back to the
/// Latin-1 code point.
fn cp1252_byte(byte: u8) -> char {
    match byte {
        0x80 => '\u{20AC}', // €
        0x82 => '\u{201A}', // ‚
        0x83 => '\u{0192}', // ƒ
        0x84 => '\u{201E}', // „
        0x85 => '\u{2026}', // …
        0x86 => '\u{2020}', // †
        0x87 => '\u{2021}', // ‡
        0x88 => '\u{02C6}', // ˆ
        0x89 => '\u{2030}', // ‰
        0x8A => '\u{0160}', // Š
        0x8B => '\u{2039}', // ‹
        0x8C => '\u{0152}', // Œ
        0x8E => '\u{017D}', // Ž
        0x91 => '\u{2018}', // ‘
        0x92 => '\u{2019}', // ’
        0x93 => '\u{201C}', // “
        0x94 => '\u{201D}', // ”
        0x95 => '\u{2022}', // •
        0x96 => '\u{2013}', // –
        0x97 => '\u{2014}', // —
        0x98 => '\u{02DC}', // ˜
        0x99 => '\u{2122}', // ™
        0x9A => '\u{0161}', // š
        0x9B => '\u{203A}', // ›
        0x9C => '\u{0153}', // œ
        0x9E => '\u{017E}', // ž
        0x9F => '\u{0178}', // Ÿ
        // Latin-1 covers the rest (and the undefined cp1252 slots fall back here).
        b => char::from(b),
    }
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
    use crate::export::scriv::write_scriv;
    use crate::export::{Chapter, Document, Part, PubMeta};
    use proptest::prelude::*;
    use std::fs;

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
    fn rtf_uc_substitution_does_not_double_accented_and_smart_chars() {
        // Foreign Scrivener/Word write each non-ASCII glyph as `\uN\'XX` (the
        // Unicode escape plus a cp1252 substitution byte), with no space between.
        // Before the fix the substitution `\'XX` was re-decoded as a second glyph,
        // doubling every such char (#402). Em-dash (舒\'97), plus é and ñ.
        let body = "caf\\u233\\'e9 se\\u241\\'f1or\\u8212\\'97done";
        let out = md(&rtf(body));
        assert_eq!(
            out, "café señor—done\n",
            "each \\uN\\'XX glyph decodes once, not doubled"
        );
    }

    #[test]
    fn rtf_cp1252_high_bytes_decode_to_typographic_glyphs() {
        // 0x80–0x9F is cp1252 typography, not Latin-1 C1 controls: \'93/\'94 are
        // curly double quotes, \'97 an em-dash, \'85 an ellipsis (#402).
        let body = "\\'93Hello\\'94\\'97yes\\'85";
        let out = md(&rtf(body));
        assert_eq!(out, "\u{201C}Hello\u{201D}\u{2014}yes\u{2026}\n");
        assert!(
            !out.chars().any(|c| ('\u{80}'..='\u{9F}').contains(&c)),
            "no C1 control chars leak through: {out:?}"
        );
    }

    #[test]
    fn rtf_line_breaks_are_preserved_as_newlines() {
        // `\line`-joined content (the writer's preformatted form) must keep its
        // breaks rather than collapsing into one line (#522). A monospace (`\f1`)
        // run is re-wrapped as a Markdown fence.
        let body = "\\pard\\sa180\\f1 LOG one\\line LOG two\\line LOG three\\f0\\par";
        let out = md(&rtf(body));
        assert_eq!(out, "```\nLOG one\nLOG two\nLOG three\n```\n");
    }

    #[test]
    fn rtf_line_break_outside_mono_keeps_newline_without_a_fence() {
        // Even without a monospace run, `\line` survives as a hard break (a plain
        // multi-line paragraph), rather than collapsing to a single space.
        let body = "Line one\\line Line two\\par";
        let out = md(&rtf(body));
        assert_eq!(out, "Line one\nLine two\n");
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
        let doc = Document {
            title: "Round Trip".into(),
            subtitle: None,
            author: None,
            series: None,
            publishing: PubMeta::default(),
            epigraph: None,
            half_title: false,
            series_books: Vec::new(),
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
            story: None,
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
    fn binder_unbalanced_item_still_imports_siblings_and_surfaces_the_drop() {
        // A hostile/corrupt binder: the second document's `</BinderItem>` is
        // missing, so the draft folder's own `</BinderItem>` looks like the item's
        // close. Before #1139 this threw `matching_close` off, `draft_folder`
        // returned None, and the WHOLE manuscript vanished into the empty fallback
        // — reported as success with `unreadable_bodies == 0`. Now the well-formed
        // siblings import and the dropped item is surfaced.
        let scrivx = r#"<?xml version="1.0"?><ScrivenerProject><Binder>
<BinderItem UUID="d" Type="DraftFolder"><Title>Manuscript</Title><Children>
<BinderItem UUID="u1" Type="Text"><Title>One</Title></BinderItem>
<BinderItem UUID="u2" Type="Text"><Title>Two</Title>
<BinderItem UUID="u3" Type="Text"><Title>Three</Title></BinderItem>
</Children></BinderItem>
</Binder></ScrivenerProject>"#;
        let out = parse_binder(scrivx, Some("My Novel".into()), |uuid| {
            Some(rtf(&format!("Body {uuid}.")))
        });
        let titles: Vec<&str> = out.chapters.iter().map(|c| c.title.as_str()).collect();
        // The well-formed first chapter imports — the manuscript is NOT reduced to
        // the empty fallback — and the unbalanced item is surfaced, not silent.
        assert!(
            titles.contains(&"One"),
            "well-formed chapters still import: {titles:?}"
        );
        assert_ne!(
            (titles.as_slice(), out.unreadable_bodies),
            (["Manuscript"].as_slice(), 0),
            "never only-fallback-as-success on an unbalanced element"
        );
        assert!(
            out.unreadable_bodies > 0,
            "the dropped/unbalanced item is surfaced via unreadable_bodies"
        );
    }

    #[test]
    fn safe_uuid_rejects_traversing_and_absolute_paths() {
        // A path-safe Scrivener UUID is accepted…
        assert_eq!(safe_uuid("1A2B-3C4D"), Some("1A2B-3C4D"));
        // …but anything that could escape `Files/Data/` is rejected.
        for hostile in [
            "",
            "..",
            ".",
            "../../../secret",
            "/etc/passwd",
            "a/b",
            "a\\b",
            "..\\..\\secret",
            "sub/../../../secret",
        ] {
            assert_eq!(safe_uuid(hostile), None, "rejected: {hostile:?}");
        }
    }

    #[test]
    fn binder_traversing_uuid_never_reads_outside_the_bundle() {
        // A hostile binder names a traversing UUID. The reader must never be asked
        // to read it — the document comes in empty and is counted, not read from
        // outside the bundle (#1139 security).
        let scrivx = r#"<?xml version="1.0"?><ScrivenerProject><Binder>
<BinderItem UUID="d" Type="DraftFolder"><Title>Manuscript</Title><Children>
<BinderItem UUID="../../../secret" Type="Text"><Title>Evil</Title></BinderItem>
<BinderItem UUID="u1" Type="Text"><Title>Good</Title></BinderItem>
</Children></BinderItem>
</Binder></ScrivenerProject>"#;
        let requested = std::cell::RefCell::new(Vec::<String>::new());
        let out = parse_binder(scrivx, Some("My Novel".into()), |uuid| {
            requested.borrow_mut().push(uuid.to_string());
            Some(rtf(&format!("Body {uuid}.")))
        });
        assert!(
            !requested
                .borrow()
                .iter()
                .any(|u| u.contains("..") || u.contains('/')),
            "no traversing UUID is ever read: {:?}",
            requested.borrow()
        );
        // The evil document is present but empty (its body was never read), and the
        // benign sibling still imports normally.
        let evil = out.chapters.iter().find(|c| c.title == "Evil").unwrap();
        assert_eq!(evil.body, "", "the rejected document has no body");
        assert!(out.unreadable_bodies >= 1, "the rejected read is surfaced");
        assert!(out.chapters.iter().any(|c| c.title == "Good"));
    }

    #[test]
    fn rtf_out_of_range_uc_does_not_eat_trailing_prose() {
        // A hostile/corrupt `\ucN` with a huge count would, unclamped, make the
        // substitution-skip consume the rest of the paragraph after a `\uN` escape,
        // silently swallowing prose (#1139). Clamped, the glyph decodes and the
        // trailing prose survives.
        let body = "caf\\uc999999\\u233\\'e9 and the rest of the sentence survives.";
        let out = md(&rtf(body));
        assert!(
            out.contains("rest of the sentence survives"),
            "trailing prose isn't eaten by an out-of-range \\uc: {out:?}"
        );
        assert!(out.contains("café"), "the glyph still decodes: {out:?}");
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

    proptest! {
        /// Feeding arbitrary bytes as the binder XML never panics: a garbage or
        /// truncated `.scrivx` surfaces an empty/fallback result, never a crash
        /// (#1098). Import runs on untrusted input, so this is the no-panic bar
        /// for the binder parser's pure core.
        #[test]
        fn parse_binder_never_panics_on_arbitrary_scrivx(
            scrivx in ".{0,4096}"
        ) {
            let _ = parse_binder(&scrivx, Some("X".into()), |_| None);
        }

        /// The RTF-to-Markdown converter shares the untrusted-body surface; an
        /// arbitrary `content.rtf` body (garbage, truncated, unbalanced braces)
        /// must never panic it.
        #[test]
        fn parse_binder_never_panics_on_arbitrary_rtf(
            body in ".{0,4096}"
        ) {
            let scrivx = r#"<?xml version="1.0"?><ScrivenerProject><Binder>
<BinderItem UUID="d" Type="DraftFolder"><Title>Manuscript</Title><Children>
<BinderItem UUID="u1" Type="Text"><Title>One</Title></BinderItem>
</Children></BinderItem>
</Binder></ScrivenerProject>"#;
            let _ = parse_binder(scrivx, Some("X".into()), |_| Some(body.clone()));
        }

        /// A real `.scriv` bundle whose `.scrivx` is arbitrary bytes never panics
        /// end to end — the filesystem path, not just the pure core.
        #[test]
        fn parse_scriv_never_panics_on_arbitrary_binder(
            scrivx in prop::collection::vec(any::<u8>(), 0..4096)
        ) {
            use std::fs;
            let dir = crate::testutil::scratch_dir("scriv-import", "fuzz-binder")
                .join("Fuzz.scriv");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("Fuzz.scrivx"), &scrivx).unwrap();
            let _ = parse_scriv(&dir);
        }
    }

    #[test]
    fn parse_scriv_reports_a_missing_binder_document() {
        // Build a minimal `.scriv` bundle, then remove one document's content so
        // it's listed in the binder but unreadable on disk (#511).
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
