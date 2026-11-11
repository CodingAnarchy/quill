//! Verse embedded in prose (#1337): a song, a ballad, or a poem a character
//! recites, fenced inside a prose chapter so it can be checked on its own.
//!
//! ````markdown
//! ```verse iambic tetrameter abab
//! The wind was loud upon the hill,
//! ...
//! ```
//! ````
//!
//! The words after `verse` on the fence line are the block's targets: a meter,
//! a rhyme scheme, or both, in the same words the chapter browser takes. A block
//! that names no target is checked against the chapter's, if it has any; one
//! that names a target uses only its own. Either way the block exports as verse,
//! never as code.

use std::ops::Range;

use super::meter::Meter;
use super::rhyme::{RhymeFault, RhymeLine, RhymeScheme};
use super::scan::LineScan;

/// The info-string word that makes a fenced block verse.
pub const FENCE_WORD: &str = "verse";

/// What a block or a passage is checked against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Targets {
    pub meter: Option<Meter>,
    pub rhyme: Option<RhymeScheme>,
}

impl Targets {
    /// Read targets from the author's words: a meter, a rhyme scheme, or a meter
    /// then a scheme (`iambic tetrameter abab`, `common meter / ballad stanza`).
    /// A `/`, `;`, or `,` between the two is allowed. The longest reading of the
    /// meter wins, and a scheme written in letters has to read like one, starting
    /// at A and bringing each new letter in order, so a misspelled meter isn't
    /// taken for a scheme. Blank text is no targets; anything else Quill can't
    /// read is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let words: Vec<&str> = text
            .split_whitespace()
            .map(|w| w.trim_end_matches([',', ';']))
            .filter(|w| !matches!(*w, "" | "/" | "|" | "\u{b7}"))
            .collect();
        for split in (0..=words.len()).rev() {
            let (meter_words, rhyme_words) = words.split_at(split);
            let meter = if meter_words.is_empty() {
                None
            } else {
                match Meter::parse(&meter_words.join(" ")) {
                    Some(m) => Some(m),
                    None => continue,
                }
            };
            let rhyme = if rhyme_words.is_empty() {
                None
            } else {
                match RhymeScheme::parse(&rhyme_words.join(" ")) {
                    Some(r) if reads_as_scheme(&r) => Some(r),
                    _ => continue,
                }
            };
            return Some(Self { meter, rhyme });
        }
        None
    }

    /// Whether there's anything to check.
    pub const fn is_empty(&self) -> bool {
        self.meter.is_none() && self.rhyme.is_none()
    }

    /// These targets, or `fallback` when these name none.
    pub fn or(&self, fallback: &Self) -> Self {
        if self.is_empty() {
            fallback.clone()
        } else {
            self.clone()
        }
    }

    /// The targets in the author's words, as the fence line takes them.
    pub fn key(&self) -> String {
        let mut parts = Vec::new();
        if let Some(m) = &self.meter {
            parts.push(m.key());
        }
        if let Some(r) = &self.rhyme {
            parts.push(r.key());
        }
        // A slash keeps a named scheme from reading as part of the meter.
        parts.join(" / ")
    }
}

/// Whether a scheme reads as one an author would write: a named form, or
/// letters that start at A and bring each new letter in turn (X, unrhymed,
/// aside). `ABAB CDCD` does; `iambik`, a misspelled meter, doesn't.
fn reads_as_scheme(scheme: &RhymeScheme) -> bool {
    if scheme.named.is_some() {
        return true;
    }
    let mut next = 'A';
    for c in scheme.letters().chars() {
        match c {
            'X' | ' ' => {}
            c if c == next => {
                next = match next {
                    'W' => 'Y',
                    n => char::from_u32(u32::from(n) + 1).unwrap_or('Z'),
                };
            }
            c if c < next => {}
            _ => return false,
        }
    }
    true
}

/// The text after `verse` on a fence line that opens a verse block, trimmed,
/// or `None` when the line isn't one (`` ```verse iambic tetrameter `` gives
/// `iambic tetrameter`; `` ``` `` and `` ```rust `` give `None`).
pub fn fence_targets(line: &str) -> Option<&str> {
    let info = line.trim_start().strip_prefix("```")?;
    let info = info.trim_start_matches('`').trim();
    let (word, rest) = info.split_once(char::is_whitespace).unwrap_or((info, ""));
    word.eq_ignore_ascii_case(FENCE_WORD).then(|| rest.trim())
}

/// A fenced verse block in a chapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerseBlock {
    /// The whole block, fences included, as byte offsets in the text.
    pub span: Range<usize>,
    /// The verse between the fences.
    pub inner: Range<usize>,
    /// The line number (from 0) of the first line between the fences.
    pub first_line: usize,
    /// The targets the fence line names, when Quill can read them.
    pub targets: Option<Targets>,
    /// The fence line's words after `verse`, as written.
    pub written: String,
}

impl VerseBlock {
    /// What to check the block against: its own targets, else the chapter's.
    /// Unreadable targets check nothing (the report says why).
    pub fn targets_within(&self, chapter: &Targets) -> Targets {
        self.targets
            .as_ref()
            .map_or_else(Targets::default, |t| t.or(chapter))
    }
}

/// The fenced verse blocks in `text` (a chapter body), in order. An unclosed
/// block runs to the end of the text, as Markdown reads it. A fence that isn't
/// verse (code) is skipped whole, so a fence line inside it isn't misread.
pub fn verse_blocks(text: &str) -> Vec<VerseBlock> {
    let mut out = Vec::new();
    // While inside a fence: whether it's verse, and where it and its body began.
    let mut open: Option<(bool, usize, usize, usize, &str)> = None;
    let mut at = 0;
    for (n, line) in text.split_inclusive('\n').enumerate() {
        let start = at;
        at += line.len();
        let bare = line.trim_end_matches(['\n', '\r']);
        if !bare.trim_start().starts_with("```") {
            continue;
        }
        match open.take() {
            Some((true, span_start, inner_start, first_line, written)) => {
                out.push(block(
                    span_start..at,
                    inner_start..start,
                    first_line,
                    written,
                ));
            }
            Some((false, ..)) => {}
            None => {
                let written = fence_targets(bare);
                open = Some((written.is_some(), start, at, n + 1, written.unwrap_or("")));
            }
        }
    }
    if let Some((true, span_start, inner_start, first_line, written)) = open {
        let end = text.len();
        out.push(block(
            span_start..end,
            inner_start.min(end)..end,
            first_line,
            written,
        ));
    }
    out
}

fn block(span: Range<usize>, inner: Range<usize>, first_line: usize, written: &str) -> VerseBlock {
    VerseBlock {
        span,
        inner,
        first_line,
        targets: Targets::parse(written),
        written: written.to_string(),
    }
}

/// Number a passage's scans and rhymes as lines of the whole chapter: each line
/// (and each line a rhyme points back to) moves down by `by`.
pub fn shift_lines(scans: &mut [LineScan], rhymes: &mut [RhymeLine], by: usize) {
    for scan in scans {
        scan.line += by;
    }
    for rhyme in rhymes {
        rhyme.line += by;
        if let Some((with, _)) = &mut rhyme.partner {
            *with += by;
        }
        match &mut rhyme.fault {
            Some(RhymeFault::Missing { with, .. } | RhymeFault::Unwanted { with }) => *with += by,
            None => {}
        }
    }
}
