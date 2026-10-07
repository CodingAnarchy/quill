//! The per-block reparse must be indistinguishable from parsing the whole text
//! at once. The oracle here is `tree_sitter_md::MarkdownParser` run fresh over
//! the full document — the parse `Syntax` used before it cut documents into
//! cached blocks — distilled by the same `collect`.

use std::fmt::Write;

use ropey::Rope;
use tree_sitter_md::MarkdownParser;

use crate::syntax::{
    ascii_fold, collect, conceal_all_from, scene_break_lines_from, Heading, Highlight, Marker,
    Syntax,
};

/// Everything `Syntax` derives from a parse, for whole-value comparison.
#[derive(Debug, PartialEq, Eq)]
struct Derived {
    highlights: Vec<Highlight>,
    markers: Vec<Marker>,
    conceal: Vec<(usize, usize)>,
    headings: Vec<Heading>,
    scene_breaks: Vec<usize>,
}

fn derived(s: &Syntax) -> Derived {
    Derived {
        highlights: s.highlights.clone(),
        markers: s.markers.clone(),
        conceal: s.conceal_all.to_vec(),
        headings: s.headings.clone(),
        scene_breaks: s.scene_breaks.clone(),
    }
}

/// A whole-document parse with `MarkdownParser`, the pre-cutting reference.
fn oracle(text: &str) -> Derived {
    let tree = MarkdownParser::default()
        .parse(&ascii_fold(text.as_bytes()), None)
        .unwrap();
    let c = collect(tree.block_tree(), tree.inline_trees(), |s, e| {
        text[s..e].to_string()
    });
    Derived {
        conceal: conceal_all_from(&c.markers).to_vec(),
        scene_breaks: scene_break_lines_from(&c.highlights, text),
        highlights: c.highlights,
        markers: c.markers,
        headings: c.headings,
    }
}

/// Documents that stress the cuts: blocks that span blank lines (fenced code,
/// HTML, lists, indented code), front-matter-like `---`/`+++` runs mid-document,
/// setext headings, tables, quotes, CRLF endings, and multi-byte text.
const TRICKY: &[&str] = &[
    "",
    "\n",
    "\n\n\n",
    "one para\n",
    "one para",
    "# Title\n\nBody *em* and **strong**.\n\nMore _text_ here.\n",
    "Intro.\n\n```\ncode\n\nstill *code*\n\n```\n\nAfter *em*.\n",
    "Intro.\n\n```\nunclosed fence\n\nPara *x*.\n\n# Heading\n",
    "Intro.\n\n~~~rust\nlet x;\n\n~~~\n\n*after*\n",
    "<div>\n*not em*\n\n*em*\n",
    "<!-- comment\n\n*inside*\n\n-->\n\n*after*\n",
    "<pre>\n\n*x*\n\n</pre>\n\n*y*\n",
    "- a\n\n- b *em*\n\n  continued\n\nPara.\n",
    "1. one\n\n2. two\n\nPara *x*.\n",
    "    indented\n\n    more\n\nPara.\n",
    "> quote *a*\n\n> quote b\n\nPara.\n",
    "Para.\n\n---\ntitle: x\n---\n\nPara *b*.\n",
    "Para.\n\n+++\nx = 1\n+++\n\nPara.\n",
    "---\ntitle: front\n---\n\nBody.\n",
    "Title\n===\n\nSub\n---\n\n---\n\n* * *\n\nEnd.\n",
    "| a | b |\n|---|---|\n| *c* | d |\n\nPara.\n",
    "[ref]: /url\n\n[ref] and *em*\n",
    "Para one.\r\n\r\n# Head\r\n\r\nPara *two*.\r\n",
    "café *crème* 中文\n\n## 标题\n\n😀 **bold** 😀\n",
    "a\n\n\n\nb *c*\n\n\n# d\n",
    "**strong\n\nacross** blank\n",
    "```\n```\n\n```\n",
];

#[test]
fn whole_parse_of_tricky_documents_matches_the_oracle() {
    for doc in TRICKY {
        assert_eq!(derived(&Syntax::new(doc)), oracle(doc), "doc {doc:?}");
    }
}

#[test]
fn reparse_between_tricky_documents_matches_the_oracle() {
    // Reparsing from any parse to any other reuses whatever blocks they share;
    // the result must still equal a fresh whole parse.
    let mut s = Syntax::new("");
    for doc in TRICKY.iter().chain(TRICKY.iter().rev()) {
        s.reparse(&Rope::from_str(doc));
        assert_eq!(derived(&s), oracle(doc), "doc {doc:?}");
    }
}

/// A tiny deterministic LCG (Numerical Recipes constants) so the edit sequence
/// is reproducible without a dependency.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Fragments an author (or a slip of the hand) might insert, weighted toward
/// markup that changes block or inline structure.
const SNIPPETS: &[&str] = &[
    "*",
    "**",
    "_",
    "__",
    "\n",
    "\n\n",
    "# ",
    "## ",
    "```",
    "```\n",
    "\n```\n",
    "~~~\n",
    "---\n",
    "\n---\n\n",
    "* * *",
    "- ",
    "1. ",
    "> ",
    "    ",
    "\t",
    "+++\n",
    "===\n",
    "<div>\n",
    "<!-- ",
    "-->",
    "| a | b |\n|---|---|\n",
    "[x]: /u\n",
    "`code`",
    "word ",
    "é",
    "中",
    "😀",
    "\r\n",
    "Title\n---\n",
    " ",
    "x",
];

const BASE: &str = "# Chapter One\n\nThe night was *dark* and the road was **long**.\n\n\
                    She said _nothing_ at all.\n\n---\n\nA new scene begins here.\n\
                    It continues on a second line.\n\n## A Section\n\n\
                    ```\nlet code = *not em*;\n```\n\n- item one\n- item *two*\n\n\
                    > quoted **text**\n\nCafé 中文 😀 *emphasis*.\n\nThe end.\n";

/// Apply one random edit to `text`: an insertion of a snippet or a deletion of
/// a short range, both at char boundaries.
fn random_edit(rng: &mut Lcg, text: &mut String) {
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .collect();
    let at = boundaries[rng.below(boundaries.len())];
    if rng.below(3) == 0 && text.len() > 1 {
        let i = boundaries.iter().position(|&b| b == at).unwrap();
        let end = boundaries[(i + 1 + rng.below(6)).min(boundaries.len() - 1)];
        text.replace_range(at..end, "");
    } else {
        text.insert_str(at, SNIPPETS[rng.below(SNIPPETS.len())]);
    }
}

#[test]
fn random_edits_reparse_identically_to_a_fresh_parse() {
    for seed in 0..8 {
        let mut rng = Lcg(seed);
        let mut text = BASE.repeat(2);
        let mut s = Syntax::new(&text);
        for step in 0..120 {
            // Sometimes several edits land between reparses (an input batch).
            for _ in 0..=rng.below(3) {
                random_edit(&mut rng, &mut text);
            }
            s.reparse(&Rope::from_str(&text));
            let got = derived(&s);
            assert_eq!(got, oracle(&text), "seed {seed} step {step}: {text:?}");
            assert_eq!(got, derived(&Syntax::new(&text)), "seed {seed} step {step}");
        }
    }
}

#[test]
fn typing_in_a_paragraph_reparses_only_that_block() {
    // The point of the cut: one keystroke sends one block through tree-sitter,
    // however long the document.
    let mut text = String::from("# Title\n\n");
    for i in 0..200 {
        write!(
            text,
            "Paragraph {i} with *em* and **strong** words.

"
        )
        .unwrap();
    }
    let mut s = Syntax::new(&text);
    let at = text.find("Paragraph 100").unwrap();
    text.insert(at + 3, 'x');
    let before = s.segments.fresh_parses;
    s.reparse(&Rope::from_str(&text));
    assert_eq!(s.segments.fresh_parses - before, 1, "only the edited block");
    assert_eq!(derived(&s), oracle(&text));
}
