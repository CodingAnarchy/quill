//! Splitting a chapter file's `---`-fenced front-matter block off the body and
//! re-attaching it byte-for-byte on save (#872). Quill preserves the block
//! **verbatim**: the fences and their line endings (LF or CRLF) round-trip so an
//! author's hand formatting and any unknown keys survive a load/save untouched.
//!
//! Re-exported from [`super`], so callers still use `frontmatter::{split, join}`
//! unchanged.

use std::borrow::Cow;

/// Split chapter-file `text` into its front matter and the body beneath it.
///
/// Front matter is recognized only by the conventional fences: the file must
/// begin with a `---` line and have a later `---` line closing the block. The
/// fence newline may be either LF (`---\n`) or CRLF (`---\r\n`), so a chapter
/// imported or edited on Windows splits the same as a native LF file. The
/// returned front matter is the verbatim text *between* the fences (each line
/// including its trailing newline, CRLF and all); `None` when there is no such
/// block, in which case the whole text is the body. A leading `---` with no
/// closing fence (e.g. a Markdown thematic break) is left in the body.
pub fn split(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = strip_open_fence(text) else {
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

/// Strip the opening `---` fence and its newline, tolerating an optional `\r`
/// (so `---\r\n` is accepted alongside `---\n`); `None` when `text` doesn't
/// open with a fence line.
fn strip_open_fence(text: &str) -> Option<&str> {
    text.strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
}

/// Re-attach a front matter block above `body`, inverting [`split`]. With no
/// front matter the body is returned borrowed (no copy); `inner` is expected to
/// already end in a newline (as [`split`] returns it) so the closing fence sits
/// on its own line. The fences match the block's own line ending — CRLF when the
/// split-off block used CRLF — so a CRLF chapter round-trips byte-for-byte just
/// like an LF one.
pub fn join<'a>(front: Option<&str>, body: &'a str) -> Cow<'a, str> {
    front.map_or_else(
        || Cow::Borrowed(body),
        |inner| {
            let nl = fence_newline(inner, body);
            Cow::Owned(format!("---{nl}{inner}---{nl}{body}"))
        },
    )
}

/// The newline the reconstructed fences should use: CRLF when the chapter uses
/// CRLF line endings, else LF, mirroring what [`split`] saw so a conforming file
/// round-trips byte-for-byte (a CRLF chapter's fences come back as CRLF, an LF
/// chapter's stay LF). The body is the authority — it carries the bulk of the
/// chapter's convention — and the block is the fallback for a body-less chapter
/// (a stub with metadata but no prose yet); LF is the final default.
fn fence_newline(inner: &str, body: &str) -> &'static str {
    let crlf = uses_crlf(body)
        .or_else(|| uses_crlf(inner))
        .unwrap_or(false);
    if crlf {
        "\r\n"
    } else {
        "\n"
    }
}

/// Whether `text`'s first line ends in CRLF (`Some(true)`) or LF (`Some(false)`),
/// or `None` when it has no line ending to judge by.
fn uses_crlf(text: &str) -> Option<bool> {
    let line = text.split_inclusive('\n').next()?;
    line.ends_with('\n').then(|| line.ends_with("\r\n"))
}

/// A line without its trailing newline (LF or CRLF), so a `---\r\n` fence line
/// compares equal to `---` just like a `---\n` one.
pub(super) fn bare(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}
