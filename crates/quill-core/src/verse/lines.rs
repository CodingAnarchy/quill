//! Which lines of a chapter are verse, and how they group into stanzas. Shared
//! by the meter and rhyme scans (#1324, #1325), so both read the same lines and
//! number them the same way.

use crate::analysis::is_heading;
use crate::export::is_thematic_break;

/// One line of verse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerseLine<'a> {
    /// Its line number in the text, from 0.
    pub line: usize,
    /// Its stanza, from 0.
    pub stanza: usize,
    /// Its place in the stanza, from 0.
    pub position: usize,
    pub text: &'a str,
}

/// The verse lines of `text` (a chapter body, front matter already removed).
/// Blank lines, headings, scene breaks, and fenced code aren't verse, and each
/// ends a stanza; a line with no letters (a row of numbers, an ornament) is
/// skipped without ending one.
pub fn verse_lines(text: &str) -> Vec<VerseLine<'_>> {
    let mut out = Vec::new();
    let mut stanza = 0;
    let mut position = 0;
    let mut fenced = false;
    for (n, line) in text.lines().enumerate() {
        let fence = line.trim_start().starts_with("```");
        if fence {
            fenced = !fenced;
        }
        let structural = fence
            || fenced
            || line.trim().is_empty()
            || is_heading(line)
            || is_thematic_break(line);
        if structural {
            if position > 0 {
                stanza += 1;
                position = 0;
            }
            continue;
        }
        if !line.chars().any(char::is_alphabetic) {
            continue;
        }
        out.push(VerseLine {
            line: n,
            stanza,
            position,
            text: line,
        });
        position += 1;
    }
    out
}

/// The slot of a stanza's pattern that its `position`th line (from 0) takes,
/// for a pattern of `slots` slots in a stanza of `stanza_len` lines.
///
/// Without a `stretch`, the pattern cycles: ABAB down a stanza of any length.
/// With one (#1348), the slots before it take the stanza's first lines, the
/// slots after it its last lines, and the stretch slot every line between, so
/// `ABC+B` reads ABCB, ABCCB, ABCCCB. A stanza too short to hold the pattern
/// with at least one middle line takes the slots in order from the start, so
/// the line that's missing its shape is the one checked against the wrong slot.
pub fn stanza_slot(
    position: usize,
    stanza_len: usize,
    slots: usize,
    stretch: Option<usize>,
) -> usize {
    if slots == 0 {
        return 0;
    }
    let Some(at) = stretch.filter(|&at| at < slots) else {
        return position % slots;
    };
    let tail = slots - at - 1;
    if position < at || stanza_len < slots {
        return position.min(slots - 1);
    }
    let tail_start = stanza_len - tail;
    if position >= tail_start {
        at + 1 + (position - tail_start)
    } else {
        at
    }
}

/// How many verse lines each stanza holds, indexed by stanza.
pub fn stanza_lengths(lines: &[(usize, usize)]) -> Vec<usize> {
    let mut lengths = Vec::new();
    for &(stanza, position) in lines {
        if lengths.len() <= stanza {
            lengths.resize(stanza + 1, 0);
        }
        lengths[stanza] = lengths[stanza].max(position + 1);
    }
    lengths
}
