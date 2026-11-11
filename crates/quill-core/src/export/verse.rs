//! Verse chapters on export (#1327). A prose paragraph's lines are soft-wrapped
//! and join into one, but in verse the line break is part of the work: each
//! source line stays its own line, a blank line between stanzas stays a visible
//! gap, and leading spaces stay an indent. Each writer renders a stanza's lines
//! with a hanging indent, so a line too long for the page wraps visibly inside
//! itself rather than reading as a new line.

use crate::verse::fence_targets;

/// One line of a stanza.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerseLine {
    /// Indent steps: two leading spaces (or half a tab) a step.
    pub indent: usize,
    /// The line's text, indent removed, Markdown emphasis left in for the writer.
    pub text: String,
}

/// The deepest indent a writer sets; deeper lines sit at this one.
pub const MAX_INDENT: usize = 8;

/// A stanza (a block of the body between blank lines) as its lines.
pub fn stanza(block: &str) -> Vec<VerseLine> {
    block
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let lead: usize = line
                .chars()
                .take_while(|c| c.is_whitespace())
                .map(|c| if c == '\t' { 4 } else { 1 })
                .sum();
            VerseLine {
                indent: (lead / 2).min(MAX_INDENT),
                text: line.trim().to_string(),
            }
        })
        .collect()
}

/// A chapter's Markdown body with its verse kept as lines: in a verse chapter
/// (`chapter_verse`) and in a fenced verse block (#1337), each line of a stanza
/// but the last ends in a backslash, CommonMark's hard line break, so a Markdown
/// reader keeps the lines. A verse block's fence lines become blank lines, so its
/// stanzas stand apart from the prose around them. Headings, scene breaks, code
/// fences, and blank lines are left as they are.
pub fn markdown_body(body: &str, chapter_verse: bool) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut out = String::with_capacity(body.len() + lines.len());
    // Whether each line is verse, and which lines are verse fences to blank.
    let mut verse = Vec::with_capacity(lines.len());
    let mut blank = Vec::with_capacity(lines.len());
    let mut fence: Option<bool> = None;
    for line in &lines {
        let is_fence = line.trim_start().starts_with("```");
        let (is_verse, is_blank) = match fence {
            Some(in_verse) if is_fence => {
                fence = None;
                (false, in_verse)
            }
            Some(in_verse) => (in_verse && stanza_line(line), false),
            None if is_fence => {
                let opens_verse = fence_targets(line).is_some();
                fence = Some(opens_verse);
                (false, opens_verse)
            }
            None => (chapter_verse && stanza_line(line), false),
        };
        verse.push(is_verse);
        blank.push(is_blank);
    }
    for (i, line) in lines.iter().enumerate() {
        if blank[i] {
            out.push('\n');
            continue;
        }
        out.push_str(line);
        let next_is_verse = verse.get(i + 1).copied().unwrap_or(false);
        if verse[i] && next_is_verse && !line.ends_with('\\') {
            out.push('\\');
        }
        out.push('\n');
    }
    out
}

/// Whether a line of verse text is a line of a stanza, rather than a gap, a
/// heading, or a scene break.
fn stanza_line(line: &str) -> bool {
    !line.trim().is_empty()
        && !line.trim_start().starts_with('#')
        && !super::is_thematic_break(line.trim())
}
