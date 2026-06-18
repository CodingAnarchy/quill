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
        if line.strip_suffix('\n').unwrap_or(line) == "---" {
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
/// fences. Reads and writes are **surgical**: a write replaces (or appends)
/// only the one `key:` line, so unknown keys and an author's hand formatting are
/// left exactly as they were — preserving the round-trip guarantee from the
/// foundation while still giving typed access to the fields Quill manages.
///
/// This handles only top-level scalar keys (the metadata Quill writes); nested
/// or multi-line YAML values are read/preserved verbatim but not interpreted.
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

    /// The scalar value of a top-level `key:`, unquoted; `None` if absent.
    fn get(&self, key: &str) -> Option<String> {
        let prefix = format!("{key}:");
        self.raw
            .lines()
            .find_map(|line| line.strip_prefix(&prefix).map(|v| unquote(v.trim())))
    }

    /// Replace the `key:` line in place, or append it if the key is absent.
    fn set(&mut self, key: &str, value: &str) {
        let new_line = format!("{key}: {}\n", quote_scalar(value));
        let prefix = format!("{key}:");
        let mut out = String::with_capacity(self.raw.len() + new_line.len());
        let mut replaced = false;
        for line in self.raw.split_inclusive('\n') {
            let bare = line.strip_suffix('\n').unwrap_or(line);
            if !replaced && bare.strip_prefix(&prefix).is_some() {
                out.push_str(&new_line);
                replaced = true;
            } else {
                out.push_str(line);
            }
        }
        if !replaced {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&new_line);
        }
        self.raw = out;
    }

    /// Drop the `key:` line if present, leaving every other line untouched.
    fn remove(&mut self, key: &str) {
        let prefix = format!("{key}:");
        self.raw = self
            .raw
            .split_inclusive('\n')
            .filter(|line| {
                let bare = line.strip_suffix('\n').unwrap_or(line);
                bare.strip_prefix(&prefix).is_none()
            })
            .collect();
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
}
