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

This document specifies each metric and its formula. The report covers
readability (#67), sentence-length statistics (#68), chapter-length / pacing
(#69), frequency-based style flags (#70), dialogue balance & paragraph metrics
(#71), and vocabulary richness (#72).

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

### Repeated phrases

Three-word phrases that recur **three or more times** across the manuscript,
highest first. Phrases made entirely of stopwords ("out of the") are skipped.
Phrases are counted within a chapter, never spanning a chapter boundary.

## Vocabulary

Lexical variety reveals reliance on a narrow vocabulary, and the most-used-words
list is a fast way to spot unintended echoes.

- **Variety** — a length-normalized lexical-variety score, 0–100 (higher = more
  varied). It's a **moving-average type–token ratio (MATTR)**: the average
  fraction of distinct words across every 100-word window, scaled ×100 (so "72"
  means ~72 distinct words per 100-word window). A plain type–token ratio falls
  just because a chapter is longer; MATTR doesn't, so chapters are comparable.
  Chapters shorter than the window use the plain ratio over the whole text. The
  whole-manuscript figure is the word-weighted mean of the chapters'.
- **Words** — the word count the variety was measured over (context for short
  chapters, where the score is noisier).

### Most-used words

The manuscript's most frequent **content** words — stopwords (`the`, `and`,
`she`, …) and very short words are excluded, so the list surfaces an author's
real lexical tics (a character name, an overused verb) rather than grammar.

## Customizing the word lists (#119)

The stopword list (excluded from **most-used words**) and the filter-word list
(counted in **style flags**) are baked-in defaults that encode editorial
judgement — and the defaults won't fit every manuscript. A character named
"Said" is silently dropped from the most-used list; a sci-fi draft leans on
invented terms a generic list never anticipated; you may not consider "know" a
filtering verb. A project tunes both lists from an `[analysis]` table in
`book.toml`:

```toml
[analysis]
stopwords_add = ["said", "only"]        # also exclude these from most-used words
stopwords_remove = ["back", "got"]      # let these surface in most-used words
filter_words_add = ["gazed", "stared"]  # also count these as filtering verbs
filter_words_remove = ["know", "knew"]  # stop counting these as filtering verbs
```

The lists **adjust** the defaults rather than replacing them, so you only state
the deltas — `*_add` extends, `*_remove` drops a built-in entry (an add wins a
conflict). Matching is case-insensitive. With the table absent (or its lists
empty) the built-in defaults apply, unchanged. The same effective lists drive
both the style-flags section and the most-used-words list, so the report stays
consistent. (The `-ly` non-adverb exceptions aren't configurable.)

## Verse: meter (#1324)

A chapter or short story written in verse can carry a **target meter** in its
front matter (`meter: iambic pentameter`), set with **V** in the chapter browser.
The **Verse report** (`report:verse`) scans every line of each such chapter
against it (and checks its rhyme scheme, below, when it has one). Prose chapters, with no target, never appear in it. Like the rest of
this document it's offline and algorithmic: it reports, and never offers a word
or line that would fit.

**The target.** A foot (iambic, trochaic, anapestic, dactylic, amphibrachic,
spondaic) and a length (monometer through octameter). Lengths separated by `/`
alternate down a stanza (`iambic tetrameter/trimeter`). Named forms: blank verse
and heroic couplets (iambic pentameter), alexandrines (iambic hexameter), common
meter (alternating 4 and 3 feet; also "ballad meter"), long meter (4), and short
meter (3, 3, 4, 3). A leading `strict` counts the licences below as faults.

**Lines and stanzas.** A line is a source line. Blank lines, headings, scene
breaks, and fenced code end a stanza and aren't scanned.

**How a word sounds.** Syllables and stress come from the CMU Pronouncing
Dictionary (about 134,000 words, BSD licence, bundled in
`assets/pronouncing/`). The author's own pronunciations come first (see
below). A possessive the dictionary lacks is its base word plus the `s`, a
syllable after a hiss (*Rose's*). Any other missing word gets the spelling-based
syllable estimate above with its stress left open, and is listed as a guess
under its line.

**Reading a line.** Words don't scan the same in every line, so Quill reads each
line the way a reader resolves it. A polysyllable's primary stress is fixed, and
so is an unstressed syllable next to it. Everything else can go either way:
monosyllables, function words (*the*, *of*, *into*), secondary stresses, and an
unstressed syllable two away from the main stress (the last syllable of
*temperate* commonly takes a beat). Those leaning syllables still lean (a content
word towards a beat, a function word away from one), and reading against the
lean costs a little.

Each line is matched against templates built from the target: the strict
pattern, plus the **licences** verse in that meter commonly takes.

- Iambic: a **feminine ending** (an extra offbeat at the end), a **headless
  line** (no first offbeat), and an **inverted first foot** (opening on a
  trochee).
- Anapestic: a headless line, a **substituted foot** (one offbeat of a foot left
  out), and a feminine ending.
- Trochaic: a **catalectic ending** (the last offbeat dropped) and **anacrusis**
  (an extra offbeat before the first beat).
- Dactylic: catalectic (one or both final offbeats dropped), a substituted foot,
  and anacrusis.
- Amphibrachic: headless and catalectic.

A small dynamic program chooses each word's pronunciation (where the dictionary
has more than one, like *record*) and each syllable's place, scoring a fixed
stress on the wrong beat at 3, a licence at 2, and a syllable read against its
lean at 1. The cheapest reading wins. A line **fits** when that reading puts no
fixed stress on the wrong beat. Weighing faults rather than forbidding them
matters: a line of small words can nearly always be forced into the pattern by
straining every one of them and stacking licences, and the honest reading with
one foot inverted is what a reader hears.

**What the report says.** Each line shows its number, a verdict (✓ fits, ~ fits
with a licence, ✗ breaks the meter), its syllables against the strict pattern,
its reading (`˘` unstressed, `/` stressed, a space between feet), and the text.
Under it come the faults, the licences, and any guessed words. A fault is one
of:

- **N extra syllables** or **N syllables short**: no reading of the meter, licences
  included, holds that many syllables. The count is against the strict pattern.
- **Foot N inverted**: a foot whose fixed stresses read it backwards (a trochee
  where an iamb belongs).
- **Stress out of place in foot N**: a fixed stress on an offbeat, or an offbeat
  syllable on a beat, that isn't a simple inversion.

Each chapter opens with how many lines fit (and how many of those take a
licence), each stanza's count when there's more than one, and the most common
fault.

**Where it falls short.** The dictionary says a word the way it's said today,
in American English, on its own. It doesn't know an archaic stress (Shakespeare's
*some-TIME*), an elision a poet intends (*th'*, *heav'n* aside, which are spelled
out), or how a word shifts its stress in a phrase. Monosyllables are read as the
line needs, so a line made only of them can fit almost anything. The report is a
reader's ear for where to look, not a ruling.

## Verse: in-editor scansion (#1326)

**F3** (or `view:scansion`) shows the same checks while the author writes. For
each verse line of a chapter with a target, a gutter left of the prose column
shows the meter verdict (✓, `~` for a licence, ✗), the reading, the syllable
count, and the rhyme letter (red when the ending breaks the scheme). In the
text, faults are red and underlined: a word with a fixed stress on the wrong
beat, a whole line whose syllable count no reading holds, a rhyme word that
breaks the scheme, and, under `strict`, a licence's word. An accepted licence's
word (the last word of a feminine or catalectic ending, the first of a headless
or inverted line or anacrusis) is in the "revised" color instead. A substituted
foot has no single word to mark, so only its gutter verdict shows it.

`]m` / `[m` (and `verse:next` / `verse:prev`) step between lines off the meter
or the scheme, wrapping in the continuous view and crossing chapters in the
single view. The marks are computed on the idle tick over the edit buffer (each
chapter in the continuous view against its own targets), cached by revision,
targets, and pronunciations, and dropped on an edit until the next idle pass,
like the other revision aids. The toggle is remembered per project by a file in
the `.quill/` state folder. When the terminal has no room left of the prose, the
gutter isn't drawn; the in-text marks still are.

## Verse: the author's pronunciations (#1328)

The dictionary is a reader with an American accent and no context. When it hears
a word wrong (a character's name, an invented word, Shakespeare's *some-TIME*),
the author tells it once, for the whole project, in **`pronunciations.txt`** at
the project root:

```text
# word = stress, one digit a syllable (1 stressed, 2 secondary, 0 unstressed)
Hermione = 0100 rhymes with Tony
sometime = 01
```

"Set how the word at the cursor sounds" (`verse:pronounce`) writes one line,
prefilled with how Quill hears the word now; a blank entry removes it. "Edit
pronunciations" (`verse:pronunciations`) opens the file in a pane; closing it
reloads the file and names any line that didn't parse (that word falls back to
the dictionary). Matching ignores case. An entry's stress is read like a
dictionary word's (its primary stress fixed, and so on). `rhymes with` lends the
word another word's sound for the rhyme check; without it the rhyme check
compares the word by its spelling. The file is plain text, so it diffs and
travels in git with the manuscript.

Some spellings carry their own pronunciation and need no entry:

- a **grave accent** sounds a syllable that's usually silent: *belovèd* is three
  syllables, the dictionary's *beloved* with its `-ed` sounded;
- other accents are read past (*naïve* is *naive*);
- **`'d`** is a silent `-ed` (*dimm'd* is *dimmed*);
- an **elision** (*heav'n*, *o'er*, *ev'ry*, *wand'ring*, *th'*, *'tis*) counts
  its syllables from the letters as written, and isn't listed as a guess. An
  apostrophe before or after a word counts as part of it only when it touches
  the word and no other letter, so a quotation mark around a word doesn't cost
  it its dictionary entry.

Quill only changes how it hears the word. The text is never touched.

## Verse: rhyme scheme (#1325)

A verse chapter can also carry a **target rhyme scheme** (`rhyme: ABAB CDCD EFEF
GG`), set with **H** in the chapter browser. The Verse report then checks each
line's ending against it, beside the meter when both are set.

**The scheme.** Letters, case-insensitive, with spaces between groups; `X` marks
a line that needn't rhyme. **One group** (`ABCB`, `AA`) repeats down the lines
of each stanza with fresh sounds each time, so `ABCB` checks every ballad
quatrain and `AA` every couplet. **Several groups** (`ABAB CDCD EFEF GG`) run
through the poem's lines in order, across stanza breaks, and start over with
fresh sounds if the poem runs past them. Named forms: shakespearean (or english)
sonnet, petrarchan (or italian) sonnet (`ABBAABBA CDECDE`), spenserian sonnet,
couplets, ballad stanza, limerick, rhyme royal, ottava rima, villanelle (its
refrains aren't checked as refrains), and terza rima, whose tercets interlock
(ABA BCB CDC …, a closing line rhyming with the last tercet's middle). A leading
`slant` lets slant rhymes count.

**How endings are heard.** The end word is the line's last word with a letter in
it. Its rhyming part is its sound from the last stressed vowel to the end, from
the same pronouncing dictionary as the meter (every pronunciation is tried). Two
endings are:

- **identical**: the same word, or the same sound throughout (*rite*, *right*);
- **perfect**: the same rhyming part (*day*, *May*);
- **eye**: spelled alike from the last vowel on, with a silent final *e* looked
  past, but said differently (*love*, *move*);
- **slant**: the stressed vowel matches or the closing consonants match, but not
  both (*shape*, *keep*);
- otherwise no rhyme.

A word the dictionary lacks is compared by its spelled ending and listed as a
guess. Identical and perfect rhymes count, slant ones count only under `slant`,
and an eye rhyme never does.

**What the report says.** Each line gets the letter its ending earns: the first
ending of a block is A, a later ending that rhymes with an earlier one takes its
letter, and anything else takes the next letter. Lines are flagged when:

- the scheme pairs it with an earlier line and it rhymes with none of them; the
  note names the first and how the two do sound (often a slant or eye rhyme);
- the scheme wants a new sound and it rhymes perfectly with an earlier line of
  another letter.

A fitting line's pairing is named when it isn't perfect. Each chapter shows how
many line endings fit, and each stanza's count beside the meter's.

**Where it falls short.** The dictionary's American pronunciation decides, so a
rhyme that holds in another accent (or held in Shakespeare's) can read as eye or
slant. Shakespeare's *temperate* and *date* rhyme only by wrenching the last
syllable onto the beat, and the report calls them an eye rhyme. Only the last
word is heard, so a feminine rhyme on a stressed word plus an unstressed one
(*knowing it*, *showing it*) is judged on its final word alone.

## Reading the report

Each chapter is one row — Ease, F–K, Fog, SMOG — with the whole-manuscript total
beneath. Because the scores are per chapter, **drift** stands out: a chapter that
reads several grades harder (or easier) than the rest is visible at a glance,
which is the point — a YA novel and a literary one target different reading
levels, and a single chapter that jumps is usually worth a look.
