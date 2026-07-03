//! Sentence segmentation (#404): the self-contained engine that splits prose
//! into sentences, reused across the analysis epic (#66). Extracted from
//! `analysis/mod.rs` to keep that module under the ~1,000-line convention; a
//! pure move, re-exported by the parent, so the analysis output is unchanged.

/// Common abbreviations whose trailing period is not a sentence end. Lowercased,
/// without the period. Covers the titles and Latinisms that show up in prose;
/// the segmenter falls back to the structural rule (a boundary needs whitespace
/// then a capital) for everything else, so this list need not be exhaustive.
const ABBREVIATIONS: &[&str] = &[
    "mr", "mrs", "ms", "dr", "prof", "st", "sr", "jr", "vs", "etc", "rev", "hon", "gen", "col",
    "sgt", "lt", "capt", "messrs", "mt", "no", "vol", "fig",
];

/// Count sentences in prose (see [`sentence_spans`] for the segmentation rules).
pub fn count_sentences(text: &str) -> usize {
    sentence_spans(text).len()
}

/// The sentences in `text`, each trimmed of surrounding whitespace, in order.
/// Empty for empty prose. The basis for sentence-length statistics (#68).
pub fn sentences(text: &str) -> Vec<&str> {
    sentence_spans(text)
        .into_iter()
        .map(|(s, e)| text[s..e].trim())
        .collect()
}

/// Byte spans `(start, end)` of each sentence in `text`. A run of terminators
/// (`.`, `!`, `?`, `…`) ends a sentence when it is followed by the end of the
/// text or by whitespace and then a capital / digit / opening quote — so
/// mid-sentence ellipses ("I... think"), decimals ("3.14"), dialogue tags
/// (`"Stop!" she said.`), and abbreviations ("Dr. Vance") don't over-split.
/// Trailing prose with no terminator is a final sentence.
fn sentence_spans(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let mut spans = Vec::new();
    let mut start: Option<usize> = None; // index into `chars` of the sentence start
    let mut word_pending = false; // saw word content since the last boundary
    let mut i = 0;
    while i < n {
        let (_, c) = chars[i];
        if start.is_none() && !c.is_whitespace() {
            start = Some(i);
        }
        if is_terminator(c) {
            let run = i;
            while i < n && is_terminator(chars[i].1) {
                i += 1;
            }
            // `start` is set whenever `word_pending` is — a word in progress always
            // recorded where it began — so `if let` binds it without a panic path
            // (and leaves `start` untouched when the run isn't a sentence boundary).
            if word_pending && is_boundary(&chars, run, i) {
                if let Some(st) = start.take() {
                    let s = chars[st].0;
                    let e = if i < n { chars[i].0 } else { text.len() };
                    spans.push((s, e));
                    word_pending = false;
                }
            }
        } else {
            if c.is_alphanumeric() {
                word_pending = true;
            }
            i += 1;
        }
    }
    if word_pending {
        // A final fragment with no closing punctuation.
        let s = start.map_or(text.len(), |k| chars[k].0);
        spans.push((s, text.len()));
    }
    spans
}

const fn is_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '…')
}

/// Whether the terminator run `chars[start..end]` actually ends a sentence.
fn is_boundary(chars: &[(usize, char)], start: usize, end: usize) -> bool {
    // A lone '.' between digits is a decimal point, not a full stop.
    if end - start == 1 && chars[start].1 == '.' {
        let prev = start.checked_sub(1).map(|j| chars[j].1);
        let next = chars.get(end).map(|x| x.1);
        if matches!(prev, Some(p) if p.is_ascii_digit())
            && matches!(next, Some(x) if x.is_ascii_digit())
        {
            return false;
        }
        if preceding_is_abbreviation(chars, start) {
            return false;
        }
    }
    // Skip any closing quotes / brackets that trail the terminator.
    let mut j = end;
    while j < chars.len() && matches!(chars[j].1, '"' | '\'' | '”' | '’' | ')' | ']' | '}') {
        j += 1;
    }
    match chars.get(j).map(|x| x.1) {
        None => true, // end of text
        Some(c) if c.is_whitespace() => {
            // A real boundary: the next non-space starts a new sentence (capital,
            // digit, or an opening quote). Lowercase means it ran on.
            match chars[j..].iter().map(|x| x.1).find(|c| !c.is_whitespace()) {
                None | Some('"' | '\'' | '“' | '‘' | '(') => true,
                Some(c) => c.is_uppercase() || c.is_ascii_digit(),
            }
        }
        _ => false, // glued to the next token, e.g. inside "e.g" — not a boundary
    }
}

/// Whether the alphabetic run ending just before `dot` is a known abbreviation.
fn preceding_is_abbreviation(chars: &[(usize, char)], dot: usize) -> bool {
    let mut start = dot;
    while start > 0 && chars[start - 1].1.is_alphabetic() {
        start -= 1;
    }
    if start == dot {
        return false;
    }
    let word: String = chars[start..dot]
        .iter()
        .flat_map(|(_, c)| c.to_lowercase())
        .collect();
    ABBREVIATIONS.contains(&word.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_sentences_handles_terminators() {
        assert_eq!(count_sentences("The night was dark. It was long."), 2);
        assert_eq!(count_sentences("Stop! Who goes there? Nobody."), 3);
        // A trailing fragment with no terminator still counts.
        assert_eq!(count_sentences("Just one clause"), 1);
        assert_eq!(count_sentences(""), 0);
    }

    #[test]
    fn count_sentences_ignores_decimals_abbreviations_and_ellipses() {
        assert_eq!(count_sentences("It cost 3.14 dollars and rose."), 1);
        assert_eq!(count_sentences("Dr. Vance arrived late."), 1);
        assert_eq!(count_sentences("I... think so."), 1); // mid-sentence ellipsis
                                                          // Dialogue: the inner '!' is not a boundary, the trailing '.' is.
        assert_eq!(count_sentences("\"Stop!\" she said."), 1);
    }

    #[test]
    fn sentences_split_and_measure_by_word_length() {
        let s = sentences("The cat sat. The dog ran away fast.");
        assert_eq!(s, vec!["The cat sat.", "The dog ran away fast."]);
        let lengths: Vec<usize> = s.iter().map(|x| crate::buffer::count_words(x)).collect();
        assert_eq!(lengths, vec![3, 5]);
    }
}
