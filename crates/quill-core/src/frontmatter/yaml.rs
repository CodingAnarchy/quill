//! The surgical YAML key/scalar/block layer under [`FrontMatter`]. Reads and
//! writes touch only the one key's entry — replacing (or appending) a scalar
//! line or a `|` literal block in place — so unknown keys and an author's hand
//! formatting round-trip untouched (#872). Duplicate keys collapse to one
//! canonical entry so [`FrontMatter::get`] and an external YAML parser agree.
//!
//! Scope: top-level keys whose value is a scalar or a single-level literal (`|`)
//! block. Nested mappings and sequences are preserved verbatim but not
//! interpreted; the `scenes:` sequence is the one exception, spliced by span
//! like a block ([`FrontMatter::replace_scenes`]).

use super::block::bare;
use super::FrontMatter;

/// Indent for the lines of a `|` literal block scalar.
pub(super) const BLOCK_INDENT: &str = "  ";

impl FrontMatter {
    /// The value of a top-level `key:`. A plain scalar is unquoted; a `|`/`>`
    /// literal block is de-indented and its lines joined with `\n`. `None` if
    /// absent.
    pub(super) fn get(&self, key: &str) -> Option<String> {
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
            while out.last().is_some_and(std::string::String::is_empty) {
                out.pop(); // a `|` block clips trailing blank lines
            }
            Some(out.join("\n"))
        } else {
            Some(unquote(value))
        }
    }

    /// Write `key`: a single line for a scalar, a `|` literal block for a
    /// multi-line value. Replaces the key's existing entry in place (or appends).
    ///
    /// A value carrying any line break — LF, a bare CR (an old-Mac paste), or
    /// CRLF — takes the `|` block path: a lone CR left on the scalar path would
    /// have `quote_scalar` emit a literal carriage return inside `"…"`, which is
    /// not a valid one-line YAML scalar and which the block reader then mangles.
    /// [`split_lines`] treats CR, LF, and CRLF each as one break, so a `\r\n`
    /// paste yields one line per visual line rather than a spurious blank one.
    pub(super) fn set(&mut self, key: &str, value: &str) {
        let entry = if value.contains(['\n', '\r']) {
            let mut block = format!("{key}: |\n");
            for line in split_lines(value) {
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
    pub(super) fn remove(&mut self, key: &str) {
        self.replace_key(key, None);
    }

    /// Splice `key`'s full entry — a scalar line, or a `|` block's `key:` line
    /// plus its indented/blank continuation — replacing it with `replacement`
    /// (or removing it when `None`); appends when the key is absent and a
    /// replacement is given.
    ///
    /// Duplicates collapse to one canonical entry: the *first* occurrence is
    /// replaced (or removed), and every later occurrence of the key — with its
    /// own block span — is dropped. A hand-edited/pasted/imported block with the
    /// same key twice would otherwise leave a stale second entry that a real YAML
    /// parser (or the export path) reads as the *last* value, contradicting the
    /// author's change; a "clear" would leave a ghost that resurrects on the next
    /// [`get`](Self::get), which reads the first. Collapsing keeps `get` and an
    /// external parser in agreement.
    fn replace_key(&mut self, key: &str, replacement: Option<&str>) {
        let prefix = format!("{key}:");
        let segments: Vec<&str> = self.raw.split_inclusive('\n').collect();
        let mut out = String::with_capacity(self.raw.len());
        let mut i = 0;
        let mut done = false;
        while i < segments.len() {
            let seg = segments[i];
            if let Some(value) = bare(seg).strip_prefix(&prefix) {
                // A match: skip this entry's whole span. The first occurrence
                // takes the replacement (once); later duplicates are just dropped.
                let mut end = i + 1;
                if value.trim().starts_with(['|', '>']) {
                    while end < segments.len() && is_block_continuation(segments[end]) {
                        end += 1;
                    }
                }
                if !done {
                    if let Some(rep) = replacement {
                        out.push_str(rep);
                    }
                    done = true;
                }
                i = end;
                continue;
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
    pub(super) fn put_text(&mut self, key: &str, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            self.remove(key);
        } else {
            self.set(key, value);
        }
    }

    /// Splice the `scenes:` sequence's full span — its `scenes:` line plus every
    /// indented/blank continuation — replacing it with `replacement` (or removing
    /// it when `None`); appends when the key is absent and a replacement is given.
    /// The sequence's continuation is indented like a `|` block, so the span rule
    /// is the same as [`replace_key`](Self::replace_key)'s block case — including
    /// its duplicate collapse: the first `scenes:` block is replaced, any later
    /// one is dropped, so `get`/`scenes` and an external parser agree.
    pub(super) fn replace_scenes(&mut self, replacement: Option<&str>) {
        let segments: Vec<&str> = self.raw.split_inclusive('\n').collect();
        let mut out = String::with_capacity(self.raw.len());
        let mut i = 0;
        let mut done = false;
        while i < segments.len() {
            let seg = segments[i];
            if bare(seg).trim_end() == "scenes:" {
                let mut end = i + 1;
                while end < segments.len() && is_block_continuation(segments[end]) {
                    end += 1;
                }
                if !done {
                    if let Some(rep) = replacement {
                        out.push_str(rep);
                    }
                    done = true;
                }
                i = end;
                continue;
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
}

/// Split a value into its visual lines, treating CR, LF, and CRLF each as a
/// single break (so a `\r\n` value doesn't produce a spurious empty line
/// between the CR and LF, and an old-Mac lone `\r` still breaks). Interior
/// blank lines survive as empty entries. Used when writing a multi-line value
/// as a `|` block, so no block line carries an interior carriage return.
pub(super) fn split_lines(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                out.push(&value[start..i]);
                i += 1;
                start = i;
            }
            b'\r' => {
                out.push(&value[start..i]);
                i += 1;
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1; // consume the LF of a CRLF pair as part of one break
                }
                start = i;
            }
            _ => i += 1,
        }
    }
    out.push(&value[start..]);
    out
}

/// Whether `segment` continues a block scalar: an indented line, or a blank
/// line (a stanza break within verse). A column-0 non-empty line ends the block.
fn is_block_continuation(segment: &str) -> bool {
    let line = bare(segment);
    line.is_empty() || line.starts_with(' ')
}

/// Strip surrounding YAML quotes from a scalar value, undoing the escaping
/// [`quote_scalar`] applies. Plain (unquoted) values are returned as-is.
pub(super) fn unquote(value: &str) -> String {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        let mut out = String::with_capacity(value.len());
        let mut chars = value[1..value.len() - 1].chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some('"') => out.push('"'),
                    // An escaped backslash, or a trailing lone backslash at end of
                    // input, both yield a single literal backslash.
                    Some('\\') | None => out.push('\\'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
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
pub(super) fn quote_scalar(value: &str) -> String {
    let needs_quoting = value.is_empty()
        || value.starts_with(char::is_whitespace)
        || value.ends_with(char::is_whitespace)
        // A value opening with a `|`/`>` block-scalar indicator would be read
        // back by `get` as a literal/folded block header, not the inline text;
        // quoting keeps it a plain scalar that round-trips verbatim.
        || value.starts_with(['|', '>'])
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
    use super::FrontMatter;

    #[test]
    fn block_scalar_indicator_start_round_trips() {
        // A value opening with a YAML block-scalar indicator (`>` folded, `|`
        // literal) must be quoted so `get` reads it back as inline text, not as a
        // block header that swallows following lines. Model: the colon-quoting
        // round-trip test in `mod.rs`.
        for value in ["> as she said", "| pipe first"] {
            let mut fm = FrontMatter::default();
            fm.set("summary", value);
            assert_eq!(
                fm.get("summary").as_deref(),
                Some(value),
                "value starting with a block-scalar indicator must round-trip verbatim",
            );
        }
    }
}
