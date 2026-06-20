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
readability (#67), sentence-length statistics (#68), chapter-length / pacing
(#69), frequency-based style flags (#70), and dialogue balance & paragraph
metrics (#71); a later ticket in the epic (#66) adds vocabulary richness.

## Shared text primitives

Every metric is built from three primitives in `src/analysis.rs`. They are the
reused foundation for the rest of the epic.

### Prose words

Words are counted with Unicode word segmentation (the same tokenizer as
`buffer::count_words`). Markdown markup (`*`, `_`, list bullets, …) is
punctuation to the segmenter, so it never counts as a word; only the prose does.
Front matter is already stripped from a chapter's body before it is measured.

**Heading lines are excluded.** A chapter title (`# The Dark`) or a scene heading
(`## Later`) is structure, not prose — measuring it would glue the title's words
onto the following sentence and skew both the sentence length and the readability
of that chapter. Heading lines are dropped before any metric is computed, so the
analysis measures what a reader actually reads. (This means the analysis word
total can be slightly lower than the progress report's, which counts heading text
toward the manuscript's size; the two reports answer different questions.)

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

Heading lines are excluded (see *Prose words* above), so a chapter title never
counts as part of its first sentence.

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

## Chapter length & pacing

Wildly uneven chapter lengths often signal pacing problems — a scene that should
be split, or two that should be merged. This section makes the shape of the book
visible.

Each chapter is one row: its word count, a bar scaled to the longest chapter, and
a `⚠ short` / `⚠ long` flag for outliers. When the book defines **parts**
(`[[parts]]` in `book.toml`), chapters are grouped under their part with a
subtotal, so act-level balance is visible too.

Unlike the readability and sentence sections, the pacing section uses the
**rolled-up chapter word count** — the same number shown in the chapter list and
the progress report (headings included) — because that is the size the author
thinks in.

A summary line gives the chapter count, **mean**, **median**, **spread**
(standard deviation), and **range** (shortest–longest).

### Outlier flags

A chapter is flagged relative to the rest of the book:

- **short** — fewer than **half** the median chapter's words.
- **long** — more than **double** the median chapter's words.

The fences are median-relative on purpose: a single very long or very short
chapter would drag a mean- or quartile-based threshold toward itself and hide the
very outlier it should catch, whereas the median barely moves. Below four
chapters there is no meaningful "rest" to compare against, so nothing is flagged.

## Dialogue & paragraphs

Dialogue-heavy and narration-heavy chapters read at very different speeds, so the
balance per chapter is a pacing signal: an all-dialogue stretch can feel rushed, a
dialogue-free one can sag into exposition. Paragraph length is the related rhythm
signal — a wall of 200-word paragraphs reads slower than short, broken-up ones.

Per chapter and overall:

- **Dialogue%** — the share of words inside quotation marks. Detection is a
  heuristic: words between double quotes — straight (`"`) or curly (`“ ”`) — count
  as dialogue, the rest as narration. **Single quotes are ignored** because
  they're ambiguous with apostrophes (`don't`, `O'Brien`), and an unbalanced
  quote runs to the end of the chapter. It measures *how much* is dialogue, not
  whether the dialogue is good.
- **Para~ / Median / Longest** — paragraph length in words: the mean, the median,
  and the longest paragraph. A paragraph is a run of non-blank lines; heading
  lines are already excluded (see *Prose words*), so a chapter title is never a
  paragraph. A median well below the mean with a large longest means a few dense
  blocks are dragging the rhythm.

## Style flags (frequency)

Every writer has tics — a word they lean on, too many adverbs, filtering that
distances the reader. This section catches them **statistically**: deterministic
counts the author decides on. It is purely frequency-based; the context-aware,
semantic version (does this *-ly* word actually weaken the sentence?) is the AI
prose/style pass (#79), kept separate to avoid overlap. Everything here is
**descriptive, not prescriptive** — counts and locations, never an auto-edit.

The per-chapter (and overall) table reports, as a percentage of that chapter's
words unless noted:

- **-ly%** — density of `-ly` manner adverbs. Common non-adverbs (`only`,
  `family`, `reply`, …) are excluded so they don't inflate the count.
- **Filter** — count of filtering verbs (`saw`, `felt`, `heard`, `noticed`,
  `realized`, `knew`, …). "She *saw* the door" distances the reader from "the
  door"; cutting the filter often tightens the prose.
- **to-be%** — density of "to be" forms (`is`, `was`, `were`, `been`, …), the
  weak-verb proxy.
- **Passive** — count of estimated passive constructions: a "to be" form followed
  by an `-ed` word ("was locked"). A heuristic, not a parser, so it misses
  irregular participles ("was held") and catches the odd false positive.

### Most-used words

The manuscript's most frequent **content** words — stopwords (`the`, `and`,
`she`, …) and very short words are excluded, so the list surfaces an author's
real lexical tics (a character name, an overused verb) rather than grammar.

### Repeated phrases

Three-word phrases that recur **three or more times** across the manuscript,
highest first. Phrases made entirely of stopwords ("out of the") are skipped.
Phrases are counted within a chapter, never spanning a chapter boundary.

## Reading the report

Each chapter is one row — Ease, F–K, Fog, SMOG — with the whole-manuscript total
beneath. Because the scores are per chapter, **drift** stands out: a chapter that
reads several grades harder (or easier) than the rest is visible at a glance,
which is the point — a YA novel and a literary one target different reading
levels, and a single chapter that jumps is usually worth a look.
