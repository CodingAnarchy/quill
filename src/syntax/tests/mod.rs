use super::*;

mod incremental;

#[test]
fn non_ascii_input_parses_without_crashing_and_keeps_char_boundaries() {
    // #1164: tree-sitter-md's ordered-list scanner calls isdigit() on the raw
    // codepoint (scanner.c:740); a non-ASCII char where a list marker could
    // begin read out of bounds in glibc's ctype table and segfaulted on Linux
    // (macOS's libc didn't fault, which hid it). These lines place emoji/CJK at
    // exactly those scan positions. The ASCII-fold of the parse view fixes it,
    // and collect() still slices the real UTF-8 bytes.
    let doc = "😀 opening line\n\
               中文段落 mixing 漢字 with prose\n\
               1. a real ordered item\n\
               😀. not actually a marker\n\
               - café 🎉 list item\n\
               ## 标题 heading\n";

    // Full parse: if any node offset had fallen mid-UTF-8-char, collect()'s
    // text[s..e] would already have panicked; assert the invariant explicitly.
    let s = Syntax::new(doc);
    for h in &s.highlights {
        assert!(
            doc.is_char_boundary(h.start) && doc.is_char_boundary(h.end),
            "highlight range must land on real char boundaries"
        );
    }
    // The ASCII `##` marker survived the fold, so the CJK heading still parses.
    assert!(
        !s.headings().is_empty(),
        "the `## 标题` heading is recognized"
    );

    // The incremental reparse path folds each rope chunk too: it must not crash
    // and must keep char-boundary offsets.
    let rope = Rope::from_str(doc);
    let mut s2 = Syntax::new(doc);
    s2.reparse(&rope);
    for h in &s2.highlights {
        assert!(doc.is_char_boundary(h.start) && doc.is_char_boundary(h.end));
    }
}

fn all(s: &Syntax) -> Vec<Highlight> {
    s.highlights_in(0, usize::MAX).copied().collect()
}

#[test]
fn detects_atx_heading() {
    let text = "# Chapter One\n\nBody text.\n";
    let s = Syntax::new(text);
    let h = all(&s)
        .into_iter()
        .find(|h| h.kind == HighlightKind::Heading)
        .expect("heading highlight");
    // The heading node range includes the line's trailing newline.
    assert_eq!(text[h.start..h.end].trim_end(), "# Chapter One");
}

#[test]
fn detects_emphasis_and_strong() {
    let text = "plain *ital* and **bold** end\n";
    let s = Syntax::new(text);
    let hs = all(&s);

    let em = hs
        .iter()
        .find(|h| h.kind == HighlightKind::Emphasis)
        .expect("emphasis");
    assert_eq!(&text[em.start..em.end], "*ital*");

    let st = hs
        .iter()
        .find(|h| h.kind == HighlightKind::Strong)
        .expect("strong");
    assert_eq!(&text[st.start..st.end], "**bold**");
}

#[test]
fn highlights_in_filters_by_range() {
    // Two headings on different lines; range query returns only the overlap.
    let text = "# One\n\n## Two\n";
    let s = Syntax::new(text);
    let second_line_start = text.find("## Two").unwrap();
    let hits: Vec<_> = s
        .highlights_in(second_line_start, text.len())
        .copied()
        .collect();
    assert_eq!(hits.len(), 1);
    assert_eq!(text[hits[0].start..hits[0].end].trim_end(), "## Two");
}

// Collect the concealed substrings for readability in the assertions below.
fn concealed(text: &str, s: &Syntax, reveal_at: Option<usize>) -> Vec<String> {
    s.conceal_ranges(reveal_at)
        .iter()
        .map(|&(a, b)| text[a..b].to_string())
        .collect()
}

#[test]
fn conceals_emphasis_and_strong_delimiters() {
    let text = "a *ital* and **bold** z\n";
    let s = Syntax::new(text);
    // Every delimiter char is concealed: `*ital*` has two, `**bold**` has
    // four (the grammar emits each `*` as its own delimiter node).
    let hidden = concealed(text, &s, None);
    assert_eq!(hidden.len(), 6);
    assert!(hidden.iter().all(|d| d == "*"));
}

#[test]
fn cursor_in_span_reveals_only_that_spans_markers() {
    let text = "a *ital* and **bold** z\n";
    let s = Syntax::new(text);
    let bold_start = text.find("**bold").unwrap();

    // Cursor inside *ital*: its two `*` reveal; the four `*` of **bold** stay
    // hidden (and they're the ones near the bold span).
    let inside_ital = text.find("ital").unwrap() + 1;
    let still_hidden = s.conceal_ranges(Some(inside_ital));
    assert_eq!(still_hidden.len(), 4);
    assert!(still_hidden.iter().all(|&(a, _)| a >= bold_start));

    // Cursor inside **bold**: its four `*` reveal; only the two `*` of *ital*
    // stay hidden.
    let inside_bold = text.find("bold").unwrap() + 1;
    let still_hidden = s.conceal_ranges(Some(inside_bold));
    assert_eq!(still_hidden.len(), 2);
    assert!(still_hidden.iter().all(|&(a, _)| a < bold_start));
}

#[test]
fn fenced_code_block_is_highlighted_and_holds_no_concealed_emphasis() {
    let text = "Intro.\n\n```\nlog *not italic* line\n```\n\nAfter.\n";
    let s = Syntax::new(text);
    let code = all(&s)
        .into_iter()
        .find(|h| h.kind == HighlightKind::Code)
        .expect("code highlight");
    // The whole fence (delimiters + content) is the highlighted span.
    let span = &text[code.start..code.end];
    assert!(
        span.contains("```") && span.contains("log *not italic* line"),
        "{span}"
    );
    // The `*` inside the fence is plain code, so no emphasis delimiter is
    // concealed — the only concealed ranges are the fence lines themselves.
    let hidden = concealed(text, &s, None);
    assert!(hidden.iter().all(|d| d.trim() == "```"), "{hidden:?}");
}

#[test]
fn conceals_whole_fence_lines_and_reveals_them_inside_the_block() {
    // Opening fence carries an info string; both fence rows (with newline) hide.
    let text = "Intro.\n\n```rust\nlet x = 1;\n```\n\nAfter.\n";
    let s = Syntax::new(text);

    // Cursor outside: the two concealed ranges are exactly the fence *lines*,
    // each including its trailing newline (so the whole row disappears, not
    // just the backticks), and the info string is part of the hidden opener.
    let outside = s.conceal_ranges(None);
    assert_eq!(outside.len(), 2, "opening and closing fence lines");
    assert_eq!(&text[outside[0].0..outside[0].1], "```rust\n");
    assert_eq!(&text[outside[1].0..outside[1].1], "```\n");

    // The content line between the fences is never concealed.
    let content = text.find("let x").unwrap();
    assert!(!outside.iter().any(|&(a, b)| a <= content && content < b));

    // Cursor inside the block reveals the raw fences for editing.
    assert!(s.conceal_ranges(Some(content)).is_empty());
    // Cursor on the fence line itself counts as inside (the reveal span is the
    // whole node), so the fences show while editing there too.
    let on_open = outside[0].0;
    assert!(s.conceal_ranges(Some(on_open)).is_empty());
}

#[test]
fn detects_thematic_break_and_conceals_it_reveal_on_cursor() {
    // Every CommonMark thematic-break form gets a `SceneBreak` highlight and a
    // conceal marker over its line, revealed only when the cursor sits on it.
    for marker in ["---", "***", "___", "* * *", "- - -"] {
        let text = format!("Para one.\n\n{marker}\n\nPara two.\n");
        let s = Syntax::new(&text);
        let brk = all(&s)
            .into_iter()
            .find(|h| h.kind == HighlightKind::SceneBreak)
            .unwrap_or_else(|| panic!("scene break for {marker:?}"));
        assert_eq!(text[brk.start..brk.end].trim_end(), marker);

        // Cursor away from the break: the raw markers are concealed (so it can
        // render as a divider). Cursor on the break line: they reveal for editing.
        let away = text.find("Para two").unwrap();
        assert!(
            concealed(&text, &s, Some(away))
                .iter()
                .any(|d| d.trim() == marker),
            "the break conceals for {marker:?}"
        );
        let on_break = brk.start;
        assert!(
            !concealed(&text, &s, Some(on_break))
                .iter()
                .any(|d| d.trim() == marker),
            "the break reveals under the cursor for {marker:?}"
        );
    }
}

#[test]
fn caches_scene_break_lines_on_parse_and_reparse() {
    // The scene-break line cache (#908) records the buffer line each break sits
    // on, computed once per parse — the renderer reads it instead of scanning
    // every highlight and doing a per-break line lookup each frame.
    let text = "Para one.\n\n---\n\nPara two.\n\n* * *\n\nPara three.\n";
    let mut s = Syntax::new(text);
    // `---` is on line 2, `* * *` on line 6 (both 0-based, newline-counted).
    assert_eq!(s.scene_break_lines(), &[2, 6]);

    // A reparse recomputes the cache off the rope (here the same document, since
    // an incremental reparse needs its edits recorded to reconcile a change), so
    // the cache is repopulated rather than left stale.
    let rope = Rope::from_str(text);
    s.reparse(&rope);
    assert_eq!(s.scene_break_lines(), &[2, 6]);

    // And the cached line matches the old per-break `byte_to_line` derivation
    // exactly, for every break, on the initial parse.
    let s2 = Syntax::new(text);
    let derived: Vec<usize> = s2
        .highlights_in(0, usize::MAX)
        .filter(|h| h.kind == HighlightKind::SceneBreak)
        .map(|h| Rope::from_str(text).byte_to_line(h.start))
        .collect();
    assert_eq!(s2.scene_break_lines(), derived.as_slice());
}

#[test]
fn a_setext_heading_underline_is_not_a_scene_break() {
    // `---` under a title is a setext heading, not a thematic break — driving off
    // the tree-sitter node (not a line regex) keeps them apart.
    let text = "A Title\n---\n\nBody.\n";
    let s = Syntax::new(text);
    assert!(
        all(&s).iter().all(|h| h.kind != HighlightKind::SceneBreak),
        "the setext underline is not read as a scene break"
    );
    assert!(
        all(&s).iter().any(|h| h.kind == HighlightKind::Heading),
        "it is a heading"
    );
}

#[test]
fn fence_concealment_is_independent_per_block() {
    let text = "```\na\n```\n\nmid\n\n```\nb\n```\n";
    let s = Syntax::new(text);
    // Four fence lines total across the two blocks when the cursor is elsewhere.
    assert_eq!(s.conceal_ranges(Some(text.find("mid").unwrap())).len(), 4);
    // Cursor in the first block reveals only its two fences; the second's stay.
    let in_first = text.find("a\n").unwrap();
    assert_eq!(s.conceal_ranges(Some(in_first)).len(), 2);
}

#[test]
fn conceal_list_is_computed_once_per_parse_and_shared() {
    // The full conceal list (nothing revealed) is cached per parse (#869): two
    // calls hand back the *same* allocation, so a frame reuses it rather than
    // rebuilding an O(markers) vec. The cursor-outside-any-span case reuses it
    // too, and the value is byte-identical to the old per-call filter.
    let text = "a *ital* and **bold** z\n";
    let s = Syntax::new(text);
    let full = s.conceal_ranges(None);
    let full_len = full.len();
    assert!(full_len > 0, "the emphasis/strong delimiters are concealed");

    let again = s.conceal_ranges(None);
    assert!(
        Rc::ptr_eq(&full, &again),
        "the full list is cached, not rebuilt"
    );

    // The cursor at document end sits in no span, so nothing is revealed — the
    // same cached list is returned rather than a fresh filtered copy.
    let outside = s.conceal_ranges(Some(text.len()));
    assert!(
        Rc::ptr_eq(&full, &outside),
        "no reveal reuses the cached list"
    );

    // The cursor inside the *ital* span reveals that span's markers, so the
    // result differs — a freshly built (shorter) list, matching the old filter.
    let inside = s.conceal_ranges(Some(text.find("ital").unwrap()));
    assert!(inside.len() < full_len, "the ital markers are revealed");
    assert!(!inside.is_empty(), "the bold markers stay concealed");
    assert!(
        !Rc::ptr_eq(&full, &inside),
        "a reveal builds a filtered list"
    );
}

#[test]
fn reparse_refreshes_the_cached_conceal_list() {
    // A reparse rebuilds the cached full list so it tracks the current markers
    // (#869) — here the emphasis is gone, so the list empties and is shared.
    let mut s = Syntax::new("plain *text* here\n");
    assert!(
        !s.conceal_ranges(None).is_empty(),
        "the emphasis is concealed"
    );
    let rope = Rope::from_str("plain text here\n");
    s.reparse(&rope);
    assert!(
        s.conceal_ranges(None).is_empty(),
        "the removed emphasis leaves nothing to conceal after reparse"
    );
}

#[test]
fn malformed_emphasis_has_no_markers() {
    // A lone `*` and an unclosed `**` are plain text — nothing to conceal,
    // nothing highlighted.
    let text = "a lone * star and **unclosed run\n";
    let s = Syntax::new(text);
    assert!(s.conceal_ranges(None).is_empty());
    assert!(all(&s).iter().all(|h| h.kind == HighlightKind::Heading));
}

#[test]
fn extracts_atx_headings_with_level_and_title() {
    let text = "# Book One\n\nIntro.\n\n## Scene A\n\ntext\n\n### Beat ###\n\nmore\n";
    let s = Syntax::new(text);
    let got: Vec<(u8, &str)> = s
        .headings()
        .iter()
        .map(|h| (h.level, h.title.as_str()))
        .collect();
    assert_eq!(got, vec![(1, "Book One"), (2, "Scene A"), (3, "Beat")]);
}

#[test]
fn extracts_setext_headings() {
    let text = "Title Line\n=====\n\nbody\n\nA Subsection\n---\n";
    let s = Syntax::new(text);
    let got: Vec<(u8, &str)> = s
        .headings()
        .iter()
        .map(|h| (h.level, h.title.as_str()))
        .collect();
    assert_eq!(got, vec![(1, "Title Line"), (2, "A Subsection")]);
}

#[test]
fn headings_are_in_document_order_and_empty_when_none() {
    assert!(Syntax::new("Just prose, no headings.\n")
        .headings()
        .is_empty());

    let text = "## Second-level first\n\n# Top level after\n";
    let s = Syntax::new(text);
    let starts: Vec<usize> = s.headings().iter().map(|h| h.start).collect();
    assert!(starts.windows(2).all(|w| w[0] <= w[1]));
}

/// The indexed lookups (`conceal_ranges` finding the cursor's enclosing spans by
/// binary search, `highlights_in` skipping spans that end before the window) must
/// agree exactly with a naive scan of every marker / highlight — at every cursor
/// byte, over documents with nesting, indented and quoted fences, scene breaks,
/// headings, and emphasis spanning lines.
#[test]
fn indexed_lookups_match_a_naive_scan_everywhere() {
    let docs = [
        "# Title *em*\n\nPlain *one* and **two** and ***both*** here.\n\
         A _span\nacross_ lines and **nested *inner* run**.\n\n---\n\nAfter.\n",
        "Intro.\n\n  ```rust\n  let x = *y*;\n  ```\n\nText *a* b.\n\n\
         > ```\n> quoted\n> ```\n\n* * *\n\n~~~\nraw\n~~~\n\nEnd **x**.\n",
        "- item *one*\n- item **two** with *more*\n\n```\nunclosed *x*\n",
    ];
    for doc in docs {
        let syntax = Syntax::new(doc);
        for b in 0..=doc.len() {
            let naive: Vec<(usize, usize)> = if syntax
                .markers
                .iter()
                .any(|m| m.span_start <= b && b <= m.span_end)
            {
                syntax
                    .markers
                    .iter()
                    .filter(|m| !(m.span_start <= b && b <= m.span_end))
                    .map(|m| (m.delim_start, m.delim_end))
                    .collect()
            } else {
                syntax.conceal_all.to_vec()
            };
            assert_eq!(
                syntax.conceal_ranges(Some(b)).to_vec(),
                naive,
                "conceal at byte {b} of {doc:?}"
            );
            for e in b..=(b + 12).min(doc.len() + 1) {
                let fast: Vec<Highlight> = syntax.highlights_in(b, e).copied().collect();
                let slow: Vec<Highlight> = syntax
                    .highlights
                    .iter()
                    .filter(|h| h.start < e && h.end > b)
                    .copied()
                    .collect();
                assert_eq!(fast, slow, "highlights in {b}..{e} of {doc:?}");
            }
        }
    }
}
