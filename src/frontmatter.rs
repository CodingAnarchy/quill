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
}
