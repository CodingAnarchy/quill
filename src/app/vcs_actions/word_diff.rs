//! Word-level intra-line diff for the history / milestone compare view (#1013).
//!
//! The unified patch [`diff_lines`](super::diff_lines) feeds is line-granular: a
//! one-word revision lights the whole `-`/`+` pair red and green, hiding *which*
//! words changed — exactly the level a novelist revises at. This module diffs a
//! removed line against the added line that replaced it at word granularity, so
//! the render can dim the words the two share and emphasize only the words that
//! differ.
//!
//! The diff is a classic LCS over word tokens (whitespace is kept as its own
//! token so spacing and punctuation survive a round-trip), turned into a run of
//! [`Seg`]s tagging each stretch of the old and new line as shared or changed.
//! It's computed lazily, when the diff overlay is rendered from a gesture — never
//! on the keystroke path — so the quadratic LCS over a single line pair is fine.

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

/// A tokenized run of one line: a *word* (a maximal run of non-whitespace) or a
/// *gap* (a maximal run of whitespace). Splitting on the boundary — rather than
/// on bytes — keeps the diff aligned to words while still preserving the exact
/// spacing and punctuation of both lines when they're reassembled.
fn tokenize(line: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        let ws = rest.starts_with(char::is_whitespace);
        // Take the maximal run whose whitespace-ness matches the first char, so a
        // word and the gap after it become two adjacent tokens.
        let end = rest
            .char_indices()
            .find(|(_, c)| c.is_whitespace() != ws)
            .map_or(rest.len(), |(i, _)| i);
        tokens.push(&rest[..end]);
        rest = &rest[end..];
    }
    tokens
}

/// Whether a token is shared between the two lines (dimmed) or unique to one of
/// them (emphasized). `Same` runs are the LCS; `Diff` runs are the insertions and
/// deletions on either side of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Kind {
    Same,
    Diff,
}

/// A styled stretch of one line: its text plus whether it's shared or changed.
/// A removed line and its added counterpart each render as a sequence of these,
/// so the shared words read as quiet context and the changed words stand out.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Seg {
    pub text: String,
    pub kind: Kind,
}

/// The length of the longest common subsequence table for `old` and `new`,
/// tokenized. Standard dynamic-programming LCS; the table lets [`segments`]
/// backtrack the actual matched tokens, not just the length.
fn lcs_table(old: &[&str], new: &[&str]) -> Vec<Vec<usize>> {
    let mut table = vec![vec![0usize; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            table[i][j] = if old[i] == new[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    table
}

/// Split `old` and `new` into shared / changed segments by backtracking the LCS
/// table. Returns `(old_segs, new_segs)`: the removed line's segments and the
/// added line's, each a run of [`Seg`]s where consecutive tokens of the same
/// [`Kind`] are coalesced so a changed clause is one span, not one span per word.
pub(super) fn segments(old_line: &str, new_line: &str) -> (Vec<Seg>, Vec<Seg>) {
    let old = tokenize(old_line);
    let new = tokenize(new_line);
    let table = lcs_table(&old, &new);

    let mut old_segs: Vec<Seg> = Vec::new();
    let mut new_segs: Vec<Seg> = Vec::new();
    let mut i = 0;
    let mut j = 0;
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            push(&mut old_segs, old[i], Kind::Same);
            push(&mut new_segs, new[j], Kind::Same);
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            push(&mut old_segs, old[i], Kind::Diff);
            i += 1;
        } else {
            push(&mut new_segs, new[j], Kind::Diff);
            j += 1;
        }
    }
    // Trailing tail on either side is pure insertion / deletion.
    while i < old.len() {
        push(&mut old_segs, old[i], Kind::Diff);
        i += 1;
    }
    while j < new.len() {
        push(&mut new_segs, new[j], Kind::Diff);
        j += 1;
    }
    (old_segs, new_segs)
}

/// Append `token` to `segs`, merging into the trailing segment when it shares the
/// same [`Kind`] so a multi-word change reads as a single highlighted span.
fn push(segs: &mut Vec<Seg>, token: &str, kind: Kind) {
    match segs.last_mut() {
        Some(last) if last.kind == kind => last.text.push_str(token),
        _ => segs.push(Seg {
            text: token.to_string(),
            kind,
        }),
    }
}

/// Whether a word diff is worth showing for this line pair. When the two lines
/// share nothing (a full rewrite), the intra-line diff is just the whole line in
/// both colors, so the line-level coloring is clearer — fall back to it. `false`
/// means "render as plain removed / added lines".
pub(super) fn is_useful(old_segs: &[Seg], new_segs: &[Seg]) -> bool {
    let shared = |segs: &[Seg]| {
        segs.iter()
            .filter(|s| s.kind == Kind::Same)
            .map(|s| s.text.trim().len())
            .sum::<usize>()
    };
    // Only meaningful once *some* non-whitespace word is shared between the lines;
    // otherwise the pair is an unrelated rewrite and word-diffing adds only noise.
    shared(old_segs) > 0 && shared(new_segs) > 0
}

/// Render one side's segments as styled spans. `sign` is the diff marker (`'-'`
/// or `'+'`), colored in `changed` (the added / removed theme color). Shared
/// words are dimmed (`context`) so they read as quiet, unchanged context; the
/// words unique to this side are shown bold in `changed` so the actual revision
/// stands out.
pub(super) fn spans(
    sign: char,
    segs: &[Seg],
    changed: ratatui::style::Color,
    context: ratatui::style::Color,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(sign.to_string(), Style::default().fg(changed))];
    for seg in segs {
        let style = match seg.kind {
            Kind::Same => Style::default().fg(context),
            Kind::Diff => Style::default().fg(changed).add_modifier(Modifier::BOLD),
        };
        spans.push(Span::styled(seg.text.clone(), style));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::{is_useful, segments, spans, tokenize, Kind};
    use ratatui::style::{Color, Modifier};

    fn changed_text(segs: &[super::Seg]) -> String {
        segs.iter()
            .filter(|s| s.kind == Kind::Diff)
            .map(|s| s.text.as_str())
            .collect()
    }

    fn same_text(segs: &[super::Seg]) -> String {
        segs.iter()
            .filter(|s| s.kind == Kind::Same)
            .map(|s| s.text.as_str())
            .collect()
    }

    #[test]
    fn tokenize_keeps_words_and_gaps_separate() {
        assert_eq!(
            tokenize("the  dark night"),
            vec!["the", "  ", "dark", " ", "night"]
        );
        assert_eq!(tokenize(""), Vec::<&str>::new());
    }

    #[test]
    fn round_trips_each_side_exactly() {
        // The segments must reassemble to the original lines — spacing and
        // punctuation intact — or the rendered diff would silently drop text.
        let old = "The night was dark, and cold.";
        let new = "The night was dark and long.";
        let (o, n) = segments(old, new);
        let joined = |segs: &[super::Seg]| segs.iter().map(|s| s.text.as_str()).collect::<String>();
        assert_eq!(joined(&o), old);
        assert_eq!(joined(&n), new);
    }

    #[test]
    fn single_word_change_marks_only_that_word() {
        // "dark" -> "bright": only the one adjective differs; everything else is
        // shared context on both sides.
        let (o, n) = segments("the night was dark", "the night was bright");
        assert_eq!(changed_text(&o), "dark");
        assert_eq!(changed_text(&n), "bright");
        assert_eq!(same_text(&o).trim(), "the night was");
        assert_eq!(same_text(&n).trim(), "the night was");
    }

    #[test]
    fn inserted_clause_marks_only_the_insertion() {
        // The added line inserts a clause; the shared words stay unchanged and the
        // new clause is the only thing flagged on the new side. Nothing is flagged
        // as removed on the old side.
        let old = "She left the room.";
        let new = "She left the quiet, shadowed room.";
        let (o, n) = segments(old, new);
        assert_eq!(changed_text(&o), "", "nothing removed");
        assert!(changed_text(&n).contains("quiet"));
        assert!(changed_text(&n).contains("shadowed"));
        assert_eq!(same_text(&o), old);
    }

    #[test]
    fn punctuation_only_change_is_isolated() {
        // A comma becoming a period is a one-token change; the words around it are
        // untouched context, so the reader sees exactly the punctuation move.
        let (o, n) = segments("Wait, he said", "Wait. he said");
        assert_eq!(changed_text(&o), "Wait,");
        assert_eq!(changed_text(&n), "Wait.");
        assert_eq!(same_text(&o).trim(), "he said");
    }

    #[test]
    fn unrelated_rewrite_is_not_useful() {
        // No shared word: the pair is a full rewrite, so the caller should fall
        // back to plain line coloring rather than a noisy all-changed word diff.
        let (o, n) = segments("alpha beta gamma", "one two three");
        assert!(!is_useful(&o, &n));
    }

    #[test]
    fn shared_words_make_it_useful() {
        let (o, n) = segments("the night was dark", "the night was bright");
        assert!(is_useful(&o, &n));
    }

    #[test]
    fn spans_dim_shared_and_bold_changed() {
        let (_, n) = segments("the night was dark", "the night was bright");
        let rendered = spans('+', &n, Color::Green, Color::DarkGray);
        // Leading sign, then the shared context (dim, no bold) and the changed word
        // (green + bold).
        assert_eq!(rendered[0].content, "+");
        let bright = rendered
            .iter()
            .find(|s| s.content.contains("bright"))
            .expect("changed span present");
        assert!(bright.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(bright.style.fg, Some(Color::Green));
        let context = rendered
            .iter()
            .find(|s| s.content.contains("night"))
            .expect("shared span present");
        assert!(!context.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(context.style.fg, Some(Color::DarkGray));
    }
}
