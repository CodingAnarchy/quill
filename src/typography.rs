//! Smart typography (#296): map the punctuation an author types to its
//! manuscript form — straight quotes to curly (`"`/`'` → `“”`/`‘’`), `--` to an
//! em-dash (`—`), and `...` to an ellipsis (`…`). A pure per-insert transform the
//! edit path applies when `[editor] smart_typography` is on (the default).
//!
//! This substitutes punctuation the author is already typing — never prose — so
//! it stays within the AI-assistance boundary: Quill isn't wording anything, just
//! orienting the marks. When the setting is off, nothing here runs and the buffer
//! holds exactly the ASCII the author typed.

/// Opening / closing single quotes. The closing single quote doubles as the
/// apostrophe (so contractions like `don’t` get the right glyph).
const LSQUO: char = '\u{2018}'; // ‘
const RSQUO: char = '\u{2019}'; // ’
/// Opening / closing double quotes.
const LDQUO: char = '\u{201C}'; // “
const RDQUO: char = '\u{201D}'; // ”
const EM_DASH: char = '\u{2014}'; // —
const ELLIPSIS: char = '\u{2026}'; // …

/// Decide a smart-typography substitution for the character `c` the author just
/// typed, given `before` — the current line's text up to the cursor.
///
/// Returns `Some((delete, insert))`: remove `delete` characters immediately
/// before the cursor, then insert the single character `insert`. `None` means
/// insert `c` unchanged. So `"` → `(0, “/”)`, the second `-` of `--` →
/// `(1, —)` (the first hyphen is consumed), and the third `.` of `...` →
/// `(2, …)`.
pub fn substitute(c: char, before: &str) -> Option<(usize, char)> {
    match c {
        '"' => Some((0, if opens(before) { LDQUO } else { RDQUO })),
        '\'' => Some((0, if opens(before) { LSQUO } else { RSQUO })),
        '-' if before.ends_with('-') => Some((1, EM_DASH)),
        '.' if before.ends_with("..") => Some((2, ELLIPSIS)),
        _ => None,
    }
}

/// Whether a quote at this position should be the opening form: true at the start
/// of a line, after whitespace, or after an opening bracket / opening quote —
/// false otherwise, so a quote or apostrophe after a word closes (and contraction
/// apostrophes become `’`).
fn opens(before: &str) -> bool {
    match before.chars().next_back() {
        None => true,
        Some(p) => p.is_whitespace() || matches!(p, '(' | '[' | '{' | LSQUO | LDQUO),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_quote_opens_then_closes_from_context() {
        assert_eq!(substitute('"', ""), Some((0, '“'))); // line start
        assert_eq!(substitute('"', "she said "), Some((0, '“'))); // after space
        assert_eq!(substitute('"', "“hello"), Some((0, '”'))); // after a word
    }

    #[test]
    fn double_quote_after_open_bracket_or_quote_opens() {
        assert_eq!(substitute('"', "("), Some((0, '“')));
        assert_eq!(substitute('"', "\u{201C}"), Some((0, '“'))); // after “ -> still opening
    }

    #[test]
    fn single_quote_handles_contractions_and_quotes() {
        assert_eq!(substitute('\'', "don"), Some((0, '’'))); // contraction
        assert_eq!(substitute('\'', "it"), Some((0, '’')));
        assert_eq!(substitute('\'', "said "), Some((0, '‘'))); // opening single quote
        assert_eq!(substitute('\'', ""), Some((0, '‘')));
    }

    #[test]
    fn double_hyphen_becomes_em_dash() {
        assert_eq!(substitute('-', "wait-"), Some((1, '—'))); // second hyphen
        assert_eq!(substitute('-', "wait"), None); // first hyphen is literal
        assert_eq!(substitute('-', "well-being is "), None); // single hyphen in a word
    }

    #[test]
    fn triple_dot_becomes_ellipsis() {
        assert_eq!(substitute('.', "wait.."), Some((2, '…'))); // third dot
        assert_eq!(substitute('.', "wait."), None); // second dot is literal
        assert_eq!(substitute('.', "wait"), None); // first dot is literal
    }

    #[test]
    fn ordinary_characters_pass_through() {
        assert_eq!(substitute('a', "hello"), None);
        assert_eq!(substitute(' ', "hello"), None);
        assert_eq!(substitute('\n', "hello"), None);
    }
}
