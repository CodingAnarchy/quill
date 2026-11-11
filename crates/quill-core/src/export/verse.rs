//! Verse chapters on export (#1327). A prose paragraph's lines are soft-wrapped
//! and join into one, but in verse the line break is part of the work: each
//! source line stays its own line, a blank line between stanzas stays a visible
//! gap, and leading spaces stay an indent. Each writer renders a stanza's lines
//! with a hanging indent, so a line too long for the page wraps visibly inside
//! itself rather than reading as a new line.

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

/// A verse chapter's Markdown body: each line of a stanza but the last ends in a
/// backslash, CommonMark's hard line break, so a Markdown reader keeps the lines.
/// Headings, scene breaks, fenced code, and blank lines are left as they are.
pub fn markdown_body(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut out = String::with_capacity(body.len() + lines.len());
    let mut fenced = false;
    for (i, line) in lines.iter().enumerate() {
        let fence = line.trim_start().starts_with("```");
        if fence {
            fenced = !fenced;
        }
        let verse = |l: &str| {
            !l.trim().is_empty()
                && !l.trim_start().starts_with('#')
                && !l.trim_start().starts_with("```")
                && !super::is_thematic_break(l.trim())
        };
        out.push_str(line);
        let next_is_verse = lines.get(i + 1).is_some_and(|n| verse(n));
        if !fence && !fenced && verse(line) && next_is_verse && !line.ends_with('\\') {
            out.push('\\');
        }
        out.push('\n');
    }
    out
}
