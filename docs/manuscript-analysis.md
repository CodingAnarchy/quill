# Manuscript analysis (local editing metrics)

Quill computes editing metrics from the manuscript **locally** — deterministic,
instant, and fully offline. Nothing leaves the machine; there is no API cost.
These checks complement the AI feature set (`docs/ai-features.md`, which does the
semantic work) and the progress report (`docs/progress-reports.md`, which tracks
goals against word counts).

The analysis report opens with **Ctrl-t** ("text" analysis) and is read-only —
any key closes it. It is computed on demand, not per frame: opening it scans
every chapter once (the active chapter live from the buffer, the rest from
disk), so typing stays instant on a large manuscript (the perf guards in
`CLAUDE.md` apply).

This document specifies each metric and its formula. The slices so far are
readability (#67) and sentence-length statistics (#68); later tickets in the epic
(#66) add paragraph statistics, chapter-length distribution, style flags,
dialogue balance, and vocabulary.

## Shared text primitives

Every metric is built from three primitives in `src/analysis.rs`. They are the
reused foundation for the rest of the epic.

### Prose words

Words are counted with Unicode word segmentation — the same basis as
`buffer::count_words` and the progress report — so both reports agree on how big
a chapter is. Markdown markup (`#`, `*`, `_`, list bullets, …) is punctuation to
the segmenter, so it never counts as a word; only the prose does. Front matter is
already stripped from a chapter's body before it is measured. Heading *text*
(e.g. "Chapter One") is included, matching the progress report's word count.

### Sentences

A sentence ends at a run of terminators (`.`, `!`, `?`, `…`) **only** when the
run is followed by the end of the text, or by whitespace and then a capital
letter, digit, or opening quote. That structural rule, plus a short
abbreviation list, keeps these from over-counting:

- **Decimals** — `3.14` is one token, not two sentences.
- **Abbreviations** — `Dr. Vance`, `Mr.`, `etc.`, `vol.` … (see `ABBREVIATIONS`).
- **Mid-sentence ellipses** — `I... think so.` is one sentence (the `...` is
  followed by a lowercase word).
- **Dialogue** — `"Stop!" she said.` is one sentence (the inner `!` is glued to a
  closing quote and a lowercase tag; the trailing `.` is the real boundary).

Prose that ends without a terminator still counts as one final sentence, so the
words/sentence ratio is never inflated by a missing full stop.

### Syllables

A per-word heuristic: count vowel groups (`a e i o u y`), drop a silent trailing
`e` (but keep the beat in a `-le` ending like "table"), and never return zero for
a word that has letters. Words of three letters or fewer are treated as one
syllable. This is an estimate — it misses some irregular words — which is fine
for an aggregate score over thousands of words. A word with **three or more**
syllables is "complex" (polysyllabic), the input to Gunning Fog and SMOG.

## Readability scores

All four are reported **per chapter** and for the **whole manuscript**. The
whole-manuscript figure sums the raw counts (words, sentences, syllables, complex
words) across all chapters and scores that total — not an average of the
per-chapter scores, which would distort short chapters.

Let, for the span being scored:

- `W` = words, `S` = sentences, `Y` = syllables, `C` = complex (≥3-syllable) words
- `ASL` = `W / S` (average sentence length, in words)
- `ASW` = `Y / W` (average syllables per word)

A span with no words or no sentences scores a flat 0 (no divide-by-zero).

### Flesch Reading Ease

```
206.835 − 1.015 × ASL − 84.6 × ASW
```

Higher is easier. Roughly: 90–100 very easy (5th grade), 60–70 plain English,
30–50 difficult, below 30 very hard. This is the one score where higher means
*more* accessible; the other three are grade levels where higher means *harder*.

### Flesch–Kincaid grade level

```
0.39 × ASL + 11.8 × ASW − 15.59
```

A U.S. school grade. A value of 8.0 means an eighth-grader could follow it.

### Gunning Fog index

```
0.4 × (ASL + 100 × (C / W))
```

A U.S. grade. Weights long sentences and the proportion of complex words.

### SMOG grade

```
1.0430 × √(C × 30 / S) + 3.1291
```

A U.S. grade estimating the years of education needed to understand the text on
first reading. SMOG is designed for samples of ~30 sentences; on very short
chapters it is noisier than the others.

## Sentence-length statistics

Sentence-length variety is a core craft lever: long unbroken sentences and
monotonous rhythm are common revision targets. The report measures each
sentence's length in **words** (the same Unicode word basis as everything else)
and reports, per chapter and for the whole manuscript:

- **Sent** — the sentence count.
- **Mean / Median** — average and middle sentence length. A median well below the
  mean means a few very long sentences are pulling the average up.
- **Min / Max** — the shortest and longest sentence.
- **Run-ons** — how many sentences are longer than the run-on threshold.

The first sentence of a chapter includes its heading text (consistent with the
word-count basis above), so a heading adds a few words to that one sentence.

### Distribution

A histogram of the whole manuscript bucketed by length: `<10`, `10–19`, `20–29`,
`30–39`, `40+` words. Each bar is scaled to the busiest bucket. A healthy
narrative usually has a wide spread; a tall single bucket signals monotonous
rhythm.

### Longest sentences

The five longest sentences in the manuscript, each with its word count and the
chapter it lives in, so the worst offenders are easy to find. Any sentence over
the run-on threshold is marked with `⚠`.

### Run-on threshold

The length above which a sentence is flagged is configurable in `book.toml`:

```toml
title = "My Novel"
run_on_threshold = 40   # words; default 40
```

Set it to match your genre — terse thrillers run shorter than literary fiction.

## Reading the report

Each chapter is one row — Ease, F–K, Fog, SMOG — with the whole-manuscript total
beneath. Because the scores are per chapter, **drift** stands out: a chapter that
reads several grades harder (or easier) than the rest is visible at a glance,
which is the point — a YA novel and a literary one target different reading
levels, and a single chapter that jumps is usually worth a look.
