//! YAML front matter at the top of a chapter file: a `---`-delimited block that
//! carries chapter metadata (status, summary, …) above the prose.
//!
//! For now Quill preserves the block **verbatim** — it splits it off so the
//! editor shows only the body, and re-attaches it byte-for-byte on save, so
//! unknown keys and an author's hand formatting survive untouched. Typed access
//! to individual fields lands with the metadata tickets that build on this.

use std::borrow::Cow;

/// Split chapter-file `text` into its front matter and the body beneath it.
///
/// Front matter is recognized only by the conventional fences: the file must
/// begin with a `---` line and have a later `---` line closing the block. The
/// returned front matter is the verbatim text *between* the fences (each line
/// including its trailing newline); `None` when there is no such block, in which
/// case the whole text is the body. A leading `---` with no closing fence (e.g.
/// a Markdown thematic break) is left in the body.
pub fn split(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (None, text);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if bare(line) == "---" {
            let inner = &rest[..offset];
            let body = &rest[offset + line.len()..];
            return (Some(inner), body);
        }
        offset += line.len();
    }
    // Opening fence but no closing one: not front matter, leave it in the body.
    (None, text)
}

/// Re-attach a front matter block above `body`, inverting [`split`]. With no
/// front matter the body is returned borrowed (no copy); `inner` is expected to
/// already end in a newline (as [`split`] returns it) so the closing fence sits
/// on its own line.
pub fn join<'a>(front: Option<&str>, body: &'a str) -> Cow<'a, str> {
    match front {
        Some(inner) => Cow::Owned(format!("---\n{inner}---\n{body}")),
        None => Cow::Borrowed(body),
    }
}

/// An epigraph: a quotation with an optional attribution, shown above a book or
/// chapter body. Stored as two keys (`epigraph` / `epigraph_by`) in front
/// matter, or two manifest fields for the book. The quotation may be multi-line
/// (verse) — a chapter stores it as a YAML `|` literal block, the book as a TOML
/// `"""` block. The attribution is a single line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Epigraph {
    pub text: String,
    pub attribution: Option<String>,
}

impl Epigraph {
    /// Build an epigraph from a quotation and optional attribution, treating
    /// blank strings as absent. `None` when there's no quotation — an
    /// attribution alone is not an epigraph. Shared by the chapter (front
    /// matter) and book (manifest) accessors.
    pub fn from_parts(text: Option<String>, attribution: Option<String>) -> Option<Epigraph> {
        let text = text.filter(|s| !s.is_empty())?;
        Some(Epigraph {
            text,
            attribution: attribution.filter(|s| !s.is_empty()),
        })
    }
}

/// A chapter's revision status, cycled by the author and shown in the pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Draft,
    Revised,
    Final,
}

impl Status {
    /// The `status:` value as written in front matter.
    pub fn key(self) -> &'static str {
        match self {
            Status::Draft => "draft",
            Status::Revised => "revised",
            Status::Final => "final",
        }
    }

    /// Parse a front-matter `status:` value (case-insensitive).
    pub fn from_key(s: &str) -> Option<Status> {
        match s.trim().to_ascii_lowercase().as_str() {
            "draft" => Some(Status::Draft),
            "revised" => Some(Status::Revised),
            "final" => Some(Status::Final),
            _ => None,
        }
    }

    /// Human label for the status bar.
    pub fn label(self) -> &'static str {
        match self {
            Status::Draft => "Draft",
            Status::Revised => "Revised",
            Status::Final => "Final",
        }
    }

    /// Single-character badge for the chapter pane.
    pub fn badge(self) -> char {
        match self {
            Status::Draft => 'D',
            Status::Revised => 'R',
            Status::Final => 'F',
        }
    }

    /// The next status in the draft → revised → final → draft cycle.
    pub fn next(self) -> Status {
        match self {
            Status::Draft => Status::Revised,
            Status::Revised => Status::Final,
            Status::Final => Status::Draft,
        }
    }
}

/// A chapter's front matter, held as the verbatim block between the `---`
/// fences. Reads and writes are **surgical**: a write replaces (or appends) only
/// the one key's entry, so unknown keys and an author's hand formatting are left
/// exactly as they were — preserving the round-trip guarantee from the
/// foundation while still giving typed access to the fields Quill manages.
///
/// Scope: top-level keys whose value is a scalar or a single-level literal
/// (`|`) block (the metadata Quill writes — e.g. a multi-line epigraph). Nested
/// mappings and sequences are preserved verbatim but not interpreted; if a
/// future field needs those, this is where a real parse-edit-reserialize layer
/// would go.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrontMatter {
    raw: String,
}

impl FrontMatter {
    /// Wrap the verbatim block text (between the fences) returned by [`split`].
    pub fn from_raw(inner: &str) -> Self {
        Self {
            raw: inner.to_string(),
        }
    }

    /// The verbatim block, for [`join`].
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Whether the block has no content (no keys to write).
    pub fn is_empty(&self) -> bool {
        self.raw.trim().is_empty()
    }

    /// The value of a top-level `key:`. A plain scalar is unquoted; a `|`/`>`
    /// literal block is de-indented and its lines joined with `\n`. `None` if
    /// absent.
    fn get(&self, key: &str) -> Option<String> {
        let prefix = format!("{key}:");
        let mut lines = self.raw.lines();
        let value = lines.find_map(|line| line.strip_prefix(&prefix))?;
        let value = value.trim();
        if value.starts_with(['|', '>']) {
            // Literal block: gather the indented / blank continuation lines.
            let mut out: Vec<String> = Vec::new();
            for line in lines {
                if line.is_empty() {
                    out.push(String::new());
                } else if line.starts_with(' ') {
                    out.push(line.trim_start().to_string());
                } else {
                    break; // a column-0 line ends the block
                }
            }
            while out.last().is_some_and(|l| l.is_empty()) {
                out.pop(); // a `|` block clips trailing blank lines
            }
            Some(out.join("\n"))
        } else {
            Some(unquote(value))
        }
    }

    /// Write `key`: a single line for a scalar, a `|` literal block for a
    /// multi-line value. Replaces the key's existing entry in place (or appends).
    fn set(&mut self, key: &str, value: &str) {
        let entry = if value.contains('\n') {
            let mut block = format!("{key}: |\n");
            for line in value.split('\n') {
                if line.is_empty() {
                    block.push('\n');
                } else {
                    block.push_str(BLOCK_INDENT);
                    block.push_str(line);
                    block.push('\n');
                }
            }
            block
        } else {
            format!("{key}: {}\n", quote_scalar(value))
        };
        self.replace_key(key, Some(&entry));
    }

    /// Drop `key`'s entry (the `key:` line and, for a block, its continuation),
    /// leaving every other key untouched.
    fn remove(&mut self, key: &str) {
        self.replace_key(key, None);
    }

    /// Splice `key`'s full entry — a scalar line, or a `|` block's `key:` line
    /// plus its indented/blank continuation — replacing it with `replacement`
    /// (or removing it when `None`); appends when the key is absent and a
    /// replacement is given.
    fn replace_key(&mut self, key: &str, replacement: Option<&str>) {
        let prefix = format!("{key}:");
        let segments: Vec<&str> = self.raw.split_inclusive('\n').collect();
        let mut out = String::with_capacity(self.raw.len());
        let mut i = 0;
        let mut done = false;
        while i < segments.len() {
            let seg = segments[i];
            if !done {
                if let Some(value) = bare(seg).strip_prefix(&prefix) {
                    // Found it: skip this entry's whole span.
                    let mut end = i + 1;
                    if value.trim().starts_with(['|', '>']) {
                        while end < segments.len() && is_block_continuation(segments[end]) {
                            end += 1;
                        }
                    }
                    if let Some(rep) = replacement {
                        out.push_str(rep);
                    }
                    i = end;
                    done = true;
                    continue;
                }
            }
            out.push_str(seg);
            i += 1;
        }
        if !done {
            if let Some(rep) = replacement {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(rep);
            }
        }
        self.raw = out;
    }

    /// Set a free-text field, removing the key when the value is blank so the
    /// block doesn't accumulate empty entries.
    fn put_text(&mut self, key: &str, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            self.remove(key);
        } else {
            self.set(key, value);
        }
    }

    pub fn status(&self) -> Option<Status> {
        self.get("status").as_deref().and_then(Status::from_key)
    }

    pub fn set_status(&mut self, status: Status) {
        self.set("status", status.key());
    }

    pub fn summary(&self) -> Option<String> {
        self.get("summary").filter(|s| !s.is_empty())
    }

    pub fn set_summary(&mut self, summary: &str) {
        self.put_text("summary", summary);
    }

    pub fn pov(&self) -> Option<String> {
        self.get("pov").filter(|s| !s.is_empty())
    }

    pub fn set_pov(&mut self, pov: &str) {
        self.put_text("pov", pov);
    }

    /// The chapter's word-count goal, if set (#146). A non-positive or
    /// unparseable value reads as "no goal".
    pub fn goal(&self) -> Option<usize> {
        self.get("goal").and_then(|v| v.trim().parse().ok()).filter(|&n| n > 0)
    }

    /// Set the chapter's word-count goal; `0` clears it.
    pub fn set_goal(&mut self, goal: usize) {
        if goal == 0 {
            self.remove("goal");
        } else {
            self.set("goal", &goal.to_string());
        }
    }

    /// The chapter epigraph, if a quotation is set. The attribution is optional.
    pub fn epigraph(&self) -> Option<Epigraph> {
        Epigraph::from_parts(self.get("epigraph"), self.get("epigraph_by"))
    }

    pub fn set_epigraph_text(&mut self, text: &str) {
        self.put_text("epigraph", text);
    }

    pub fn set_epigraph_by(&mut self, attribution: &str) {
        self.put_text("epigraph_by", attribution);
    }
}

/// Indent for the lines of a `|` literal block scalar.
const BLOCK_INDENT: &str = "  ";

/// A line without its trailing newline.
fn bare(line: &str) -> &str {
    line.strip_suffix('\n').unwrap_or(line)
}

/// Whether `segment` continues a block scalar: an indented line, or a blank
/// line (a stanza break within verse). A column-0 non-empty line ends the block.
fn is_block_continuation(segment: &str) -> bool {
    let line = bare(segment);
    line.is_empty() || line.starts_with(' ')
}

/// Strip surrounding YAML quotes from a scalar value, undoing the escaping
/// [`quote_scalar`] applies. Plain (unquoted) values are returned as-is.
fn unquote(value: &str) -> String {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        let mut out = String::with_capacity(value.len());
        let mut chars = value[1..value.len() - 1].chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                },
                _ => out.push(c),
            }
        }
        out
    } else if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].replace("''", "'")
    } else {
        value.to_string()
    }
}

/// Render a scalar for writing, double-quoting (and escaping) it only when a
/// plain YAML scalar wouldn't round-trip the value.
fn quote_scalar(value: &str) -> String {
    let needs_quoting = value.is_empty()
        || value.starts_with(char::is_whitespace)
        || value.ends_with(char::is_whitespace)
        || value.contains(['"', '\'', ':', '#', '\n', '\r', '\t']);
    if !needs_quoting {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_front_matter_leaves_text_as_the_body() {
        let text = "# Chapter\n\nProse here.\n";
        let (front, body) = split(text);
        assert_eq!(front, None);
        assert_eq!(body, text);
    }

    #[test]
    fn splits_a_fenced_block_from_the_body() {
        let text = "---\nstatus: draft\npov: Mara\n---\n# Chapter\n\nProse.\n";
        let (front, body) = split(text);
        assert_eq!(front, Some("status: draft\npov: Mara\n"));
        assert_eq!(body, "# Chapter\n\nProse.\n");
    }

    #[test]
    fn split_then_join_round_trips() {
        let text = "---\nstatus: draft\n---\nThe body.\n";
        let (front, body) = split(text);
        assert_eq!(&*join(front, body), text);
    }

    #[test]
    fn an_empty_block_round_trips() {
        let text = "---\n---\nBody only.\n";
        let (front, body) = split(text);
        assert_eq!(front, Some(""));
        assert_eq!(body, "Body only.\n");
        assert_eq!(&*join(front, body), text);
    }

    #[test]
    fn a_leading_rule_without_a_closing_fence_is_body() {
        // A Markdown thematic break at the top is not front matter.
        let text = "---\njust a horizontal rule, no closing fence\n";
        let (front, body) = split(text);
        assert_eq!(front, None);
        assert_eq!(body, text);
    }

    #[test]
    fn join_none_is_the_body_unchanged() {
        assert_eq!(&*join(None, "Body.\n"), "Body.\n");
    }

    #[test]
    fn set_replaces_a_key_in_place_and_leaves_others_untouched() {
        let mut fm = FrontMatter::from_raw("status: draft\npov: Mara\ncustom: keep me\n");
        fm.set_status(Status::Final);
        // The status line changed; the other keys (incl. an unknown one) are intact.
        assert_eq!(fm.status(), Some(Status::Final));
        assert_eq!(fm.pov().as_deref(), Some("Mara"));
        assert!(fm.as_str().contains("custom: keep me\n"));
        assert_eq!(fm.as_str(), "status: final\npov: Mara\ncustom: keep me\n");
    }

    #[test]
    fn set_appends_a_missing_key() {
        let mut fm = FrontMatter::default();
        fm.set_status(Status::Draft);
        fm.set_pov("Jonah");
        assert_eq!(fm.as_str(), "status: draft\npov: Jonah\n");
    }

    #[test]
    fn summary_round_trips_through_quoting() {
        // A summary with a colon must be quoted to stay valid, and read back whole.
        let mut fm = FrontMatter::default();
        fm.set_summary("The reckoning: part one");
        assert!(fm.as_str().contains(r#"summary: "The reckoning: part one""#));
        assert_eq!(fm.summary().as_deref(), Some("The reckoning: part one"));
    }

    #[test]
    fn blank_text_removes_the_key() {
        let mut fm = FrontMatter::from_raw("summary: old\nstatus: draft\n");
        fm.set_summary("   ");
        assert_eq!(fm.summary(), None);
        assert!(!fm.as_str().contains("summary"));
        // Other keys survive the removal.
        assert_eq!(fm.status(), Some(Status::Draft));
    }

    #[test]
    fn status_cycles() {
        assert_eq!(Status::Draft.next(), Status::Revised);
        assert_eq!(Status::Revised.next(), Status::Final);
        assert_eq!(Status::Final.next(), Status::Draft);
    }

    #[test]
    fn multiline_value_round_trips_as_a_literal_block() {
        let mut fm = FrontMatter::from_raw("status: draft\n");
        fm.set_epigraph_text("Line one\nLine two\nLine three");
        // Written as a `|` literal block, indented, other keys intact.
        assert_eq!(
            fm.as_str(),
            "status: draft\nepigraph: |\n  Line one\n  Line two\n  Line three\n"
        );
        assert_eq!(
            fm.epigraph().unwrap().text,
            "Line one\nLine two\nLine three"
        );
        assert_eq!(fm.status(), Some(Status::Draft));
    }

    #[test]
    fn a_block_preserves_interior_blank_lines() {
        let mut fm = FrontMatter::default();
        fm.set_epigraph_text("Roses are red\n\nViolets are blue");
        fm.set_epigraph_by("Anon"); // a key after the block stays separate
        let ep = fm.epigraph().unwrap();
        assert_eq!(ep.text, "Roses are red\n\nViolets are blue");
        assert_eq!(ep.attribution.as_deref(), Some("Anon"));
    }

    #[test]
    fn replacing_a_block_with_a_scalar_drops_the_old_continuation() {
        let mut fm = FrontMatter::default();
        fm.set_epigraph_text("Line one\nLine two");
        fm.set_status(Status::Final); // a key written after the block
        // Shrink the block back to a single line.
        fm.set_epigraph_text("Just one line");
        assert_eq!(fm.epigraph().unwrap().text, "Just one line");
        assert!(fm.as_str().contains("epigraph: Just one line\n"));
        assert!(!fm.as_str().contains("Line two"));
        assert_eq!(fm.status(), Some(Status::Final));
    }

    #[test]
    fn removing_a_block_drops_its_whole_span() {
        let mut fm = FrontMatter::from_raw("status: draft\n");
        fm.set_epigraph_text("A\nB\nC");
        fm.set_epigraph_text(""); // clear
        assert!(fm.epigraph().is_none());
        assert!(!fm.as_str().contains("epigraph"));
        assert_eq!(fm.status(), Some(Status::Draft)); // sibling untouched
    }

    #[test]
    fn epigraph_round_trips_quotation_and_attribution() {
        let mut fm = FrontMatter::default();
        fm.set_epigraph_text("Call me Ishmael.");
        fm.set_epigraph_by("Melville");
        let ep = fm.epigraph().unwrap();
        assert_eq!(ep.text, "Call me Ishmael.");
        assert_eq!(ep.attribution.as_deref(), Some("Melville"));
    }

    #[test]
    fn epigraph_without_a_quotation_is_none_and_clearing_removes_keys() {
        let mut fm = FrontMatter::default();
        // Attribution alone is not an epigraph.
        fm.set_epigraph_by("Nobody");
        assert!(fm.epigraph().is_none());

        fm.set_epigraph_text("A line.");
        assert!(fm.epigraph().is_some());
        // Clearing the quotation drops the epigraph; keys are removed.
        fm.set_epigraph_text("");
        assert!(fm.epigraph().is_none());
        assert!(!fm.as_str().contains("epigraph:"));
    }
}
