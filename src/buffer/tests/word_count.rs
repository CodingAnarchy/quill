//! Word counting: Unicode segmentation via `count_words`, the cache that follows
//! edits and undo (#151), and the incremental count maintained on the edit path
//! (#868) — pinned byte-for-byte to a whole-document recount across multibyte,
//! CJK, punctuation, whitespace runs, and long randomized edit sequences.

use super::*;

#[test]
fn count_words_uses_unicode_segmentation() {
    assert_eq!(count_words(""), 0);
    assert_eq!(count_words("   \n  "), 0);
    assert_eq!(count_words("one two three"), 3);
    assert_eq!(count_words("hello, world!"), 2);
    assert_eq!(count_words("it's a tidy plan"), 4); // contraction stays one word
                                                    // Unicode segmentation splits hyphenated compounds.
    assert_eq!(count_words("well-made"), 2);
}

#[test]
fn word_count_is_correct_across_rope_chunk_boundaries() {
    // A document large enough to span multiple rope chunks. A word split
    // across a chunk boundary must not be counted twice.
    let path = temp_path("wordcount.md");
    write(&path, &"word ".repeat(5000));

    let buf = Buffer::from_path(&path).unwrap();
    assert_eq!(buf.word_count(), 5000);

    fs::remove_file(&path).ok();
}

#[test]
fn word_count_cache_updates_on_edit_and_undo() {
    let mut buf = Buffer::from_text("one two");
    assert_eq!(buf.word_count(), 2);

    // Append a third word; the cached count reflects the edit (#151).
    buf.cursor = Cursor { line: 0, col: 7 };
    for c in " three".chars() {
        buf.insert_char(c);
    }
    assert_eq!(buf.word_count(), 3);

    // Delete it back; the count follows.
    buf.seal_undo_group();
    for _ in 0..6 {
        buf.backspace();
    }
    assert_eq!(buf.word_count(), 2);

    // Undo restores both the text and the count.
    buf.undo();
    assert_eq!(buf.word_count(), 3);
}

// --- Incremental word count (#868) -------------------------------------
//
// The buffer maintains `word_count()` incrementally on the edit path instead of
// recomputing over `rope.to_string()` each frame. These tests pin the count to
// the byte-for-byte-identical whole-document `count_words` across tricky inputs
// (multibyte, CJK, punctuation, whitespace runs, emphasis markers), across the
// cold rebuild path (empty cache), and across long edit sequences that insert
// and delete mid-word and at line boundaries.

/// Inputs whose segmentation is easy to get subtly wrong.
const WORD_COUNT_CASES: &[&str] = &[
    "",
    " ",
    "   \n  ",
    "\n\n\n",
    "one two three",
    "hello, world!",
    "it's a tidy plan",
    "well-made",
    "line one\nline two\nline three",
    "trailing space \n and leading",
    "a\n\n\nb",
    "multi   space   run",
    "end\n",
    "\n\nstart",
    "日本語 のテスト\nもう一行",
    "café résumé\nnaïve",
    "*dark* and **long** indeed",
    "CJK混在mixed英語text\nnext行",
    "numbers 123 456\n789 done",
    "emoji test line\nsecond line here",
    "tab\tseparated\twords",
    "The night was *dark* and the road was **long** indeed.\nA second sentence.\n",
];

#[test]
fn cold_word_count_matches_whole_document_segmentation() {
    // The from-scratch rebuild (empty cache) must equal counting over the whole
    // materialized string — no chunk-boundary double-count, no lost words.
    for &case in WORD_COUNT_CASES {
        let buf = Buffer::from_text(case);
        assert_eq!(
            buf.word_count(),
            count_words(case),
            "cold count mismatch for {case:?}"
        );
    }
}

#[test]
fn cold_word_count_matches_across_rope_chunks_with_multibyte() {
    // Force many rope chunks with multibyte prose so a naive per-chunk count
    // would split words or graphemes at a boundary. The line-buffered cold path
    // must still equal the whole-document count.
    let unit = "café 日本語 word résumé\n";
    let text = unit.repeat(4000);
    let buf = Buffer::from_text(&text);
    assert_eq!(buf.word_count(), count_words(&text));
}

/// Every public edit keeps the incremental count byte-identical to a fresh
/// whole-document recount of the buffer's current text.
fn assert_count_tracks(buf: &Buffer) {
    let mut whole = String::new();
    buf.text_into(&mut whole);
    assert_eq!(
        buf.word_count(),
        count_words(&whole),
        "incremental count drifted for {whole:?}"
    );
}

#[test]
fn word_count_tracks_edits_mid_word_and_at_boundaries() {
    let mut buf = Buffer::from_text("alpha beta gamma\ndelta epsilon\n");
    assert_count_tracks(&buf);

    // Insert in the middle of a word.
    buf.cursor = Cursor { line: 0, col: 3 };
    for c in "XYZ".chars() {
        buf.insert_char(c);
    }
    assert_count_tracks(&buf);

    // Insert a newline mid-word, splitting one word into two.
    buf.cursor = Cursor { line: 0, col: 2 };
    buf.insert_char('\n');
    assert_count_tracks(&buf);

    // Type a multi-line, whitespace-heavy run.
    for c in "  new   words \n across  lines ".chars() {
        buf.insert_char(c);
    }
    assert_count_tracks(&buf);

    // Delete back across the inserted boundary, re-merging split words.
    buf.seal_undo_group();
    for _ in 0..40 {
        buf.backspace();
        assert_count_tracks(&buf);
    }

    // Undo/redo swap the whole rope: the count still resolves correctly.
    buf.undo();
    assert_count_tracks(&buf);
    buf.redo();
    assert_count_tracks(&buf);
}

#[test]
fn word_count_tracks_replace_and_multibyte_edits() {
    let mut buf = Buffer::from_text("The café was 日本 quiet.\nSecond line.\n");
    assert_count_tracks(&buf);

    // Replace a byte range spanning multibyte text (find-and-replace path).
    let start = buf.text().find("café").unwrap();
    let end = start + "café".len();
    buf.replace_byte_range(start, end, "restaurant kitchen");
    assert_count_tracks(&buf);

    // Replace a range that spans a newline, collapsing two lines into one.
    let text = buf.text();
    let nl = text.find('\n').unwrap();
    buf.replace_byte_range(nl, nl + 1, " ");
    assert_count_tracks(&buf);

    // replace_text swaps the whole document; the cold path recomputes.
    buf.replace_text("完全に new 内容 here\nwith 複数 lines\n");
    assert_count_tracks(&buf);
}

#[test]
fn word_count_matches_after_randomized_edit_sequences() {
    // A deterministic pseudo-random walk of inserts and deletes over a
    // multibyte, whitespace-heavy alphabet. After every edit the incremental
    // count must equal a fresh whole-document recount — this is the correctness
    // bar for #868 (counts stay byte-for-byte identical to the old behavior).
    let alphabet: &[&str] = &[
        "a", "b", " ", "\n", "!", "-", "'", "é", "日", "本", "  ", "word", ", ",
    ];
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };

    let mut buf = Buffer::from_text("seed text\nover two lines\n");
    for _ in 0..3000 {
        if next() % 3 == 0 && buf.text().chars().count() > 1 {
            // Delete a run at a pseudo-random cursor position.
            let chars = buf.text().chars().count();
            let line = next() % buf.line_count();
            buf.cursor = Cursor {
                line,
                col: next() % 20,
            };
            for _ in 0..(1 + next() % 3).min(chars) {
                buf.backspace();
            }
        } else {
            let line = next() % buf.line_count();
            buf.cursor = Cursor {
                line,
                col: next() % 20,
            };
            let frag = alphabet[next() % alphabet.len()];
            for c in frag.chars() {
                buf.insert_char(c);
            }
        }
        assert_count_tracks(&buf);
    }
}
