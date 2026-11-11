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
