//! Analysis report rendering (#150): turns the measured metrics into the
//! report's styled rows. The metrics themselves live in the parent `analysis`
//! module; this is the only half that depends on `ratatui`.

use std::fmt::Write;

use super::*;
use crate::theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// One chapter's analysis inputs: its title, total word count (the rolled-up
/// count, for the pacing section), readability [`Stats`], and the word length of
/// each of its sentences. Gathered by the editor (which owns the manuscript) and
/// handed to [`lines`] to render.
pub struct ChapterAnalysis {
    pub title: String,
    pub words: usize,
    pub stats: Stats,
    pub sentence_lengths: Vec<usize>,
    pub style: StyleFlags,
    pub dialogue: Dialogue,
    pub paragraph_lengths: Vec<usize>,
    /// Lexical variety (MATTR, 0.0..=1.0) — see [`lexical_variety_tokens`].
    pub lexical_variety: f64,
}

/// One part (chapter grouping) for the pacing section: its display name and the
/// 0-based chapter index range it spans (`start..end`, end-exclusive).
pub struct PartSpan {
    pub name: String,
    pub range: std::ops::Range<usize>,
}

/// One of the manuscript's longest sentences, with where it lives, for the
/// run-on list.
pub struct LongSentence {
    pub chapter: String,
    pub words: usize,
    pub text: String,
}

/// A bounded "top-N longest sentences" accumulator, so ranking the longest
/// sentences across the whole manuscript never holds more than N at once and
/// only copies a sentence's text when it actually makes the cut.
pub struct LongestSentences {
    cap: usize,
    items: Vec<LongSentence>, // kept sorted by `words`, descending
}

impl LongestSentences {
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            items: Vec::with_capacity(cap + 1),
        }
    }

    /// Offer a sentence; kept only if it ranks among the longest seen so far.
    pub fn consider(&mut self, chapter: &str, text: &str, words: usize) {
        if self.items.len() < self.cap || words > self.items.last().map_or(0, |l| l.words) {
            let pos = self.items.partition_point(|l| l.words >= words);
            self.items.insert(
                pos,
                LongSentence {
                    chapter: chapter.to_string(),
                    words,
                    text: text.to_string(),
                },
            );
            self.items.truncate(self.cap);
        }
    }

    /// The longest sentences, longest first.
    pub fn into_vec(self) -> Vec<LongSentence> {
        self.items
    }
}

/// Everything the analysis report renders, gathered by the editor. Grouping the
/// inputs keeps [`lines`] to one argument as the epic adds more sections.
pub struct Report<'a> {
    pub title: &'a str,
    pub chapters: &'a [ChapterAnalysis],
    pub longest: &'a [LongSentence],
    /// Chapter groupings for the pacing section (empty when the book has none).
    pub parts: &'a [PartSpan],
    pub run_on_threshold: usize,
    /// Most-used content words across the manuscript, highest first.
    pub top_words: &'a [(String, usize)],
    /// Repeated multi-word phrases across the manuscript, highest first.
    pub top_phrases: &'a [(String, usize)],
    /// Whole-manuscript lexical variety (MATTR over the pooled tokens, not a mean
    /// of the chapters', so it matches the per-chapter measure's definition).
    pub overall_variety: f64,
}

/// The analysis report's rows: a readability table, a sentence-length section
/// (per-chapter statistics, the overall distribution, and the longest
/// sentences), and a chapter-length / pacing section — each with a
/// whole-manuscript total. Computed once when the report is opened (the scan is
/// O(manuscript), so it stays off the per-frame path).
pub fn lines(report: &Report) -> Vec<Line<'static>> {
    let mut out = vec![bold(format!("{} — analysis", report.title)), Line::raw("")];
    out.extend(readability_section(report.chapters));
    out.push(Line::raw(""));
    out.extend(sentence_section(
        report.chapters,
        report.longest,
        report.run_on_threshold,
    ));
    out.push(Line::raw(""));
    out.extend(length_section(report.chapters, report.parts));
    out.push(Line::raw(""));
    out.extend(dialogue_section(report.chapters));
    out.push(Line::raw(""));
    out.extend(style_section(report.chapters, report.top_phrases));
    out.push(Line::raw(""));
    out.extend(vocabulary_section(
        report.chapters,
        report.top_words,
        report.overall_variety,
    ));
    out
}

/// The width the chapter-name column truncates to / pads to, shared by every
/// analysis table so each section's rows and its `Chapter` header line up on the
/// same left edge. The name is truncated to `NAME_MAX` then left-padded to `NAME_W`.
const NAME_MAX: usize = 24;
const NAME_W: usize = 26;

/// The left-aligned first cell of any analysis row: a chapter (or "Whole
/// manuscript") name, truncated and padded to the shared name-column width. Used
/// by every table row and the matching `Chapter` header, so the first column's
/// width lives in exactly one place.
fn name_cell(name: &str) -> String {
    format!("{:<w$}", truncate(name, NAME_MAX), w = NAME_W)
}

/// One numeric column of an analysis table: its header label and width. The same
/// `Col` drives both the header cell and the data cell (see [`header`] /
/// [`data_row`]), so the two can't drift out of alignment when a width changes (#438).
struct Col {
    label: &'static str,
    width: usize,
}

const fn col(label: &'static str, width: usize) -> Col {
    Col { label, width }
}

/// A section's header line: the `Chapter` name column followed by each numeric
/// column's label, right-aligned to that column's width.
fn header(cols: &[Col]) -> Line<'static> {
    let mut s = name_cell("Chapter");
    for c in cols {
        write!(s, "{:>w$}", c.label, w = c.width).unwrap();
    }
    dim(s)
}

/// A data row: the name column followed by the pre-formatted `cells`, each
/// right-aligned to the matching column's width. `cells` lines up with `cols`.
fn data_row(name: &str, cols: &[Col], cells: &[String]) -> String {
    let mut s = name_cell(name);
    for (c, cell) in cols.iter().zip(cells) {
        write!(s, "{:>w$}", cell, w = c.width).unwrap();
    }
    s
}

// Each section's columns — label + width — defined once and shared by the
// section's `header` and its row helper (via `data_row`), in report order.
const READABILITY_COLS: &[Col] = &[col("Ease", 9), col("F–K", 8), col("Fog", 8), col("SMOG", 8)];
const SENTENCE_COLS: &[Col] = &[
    col("Sent", 6),
    col("Mean", 7),
    col("Med", 7),
    col("Min", 6),
    col("Max", 6),
    col("Run-ons", 9),
];
// `Dialogue%` is width 9 (its 9-char label and the data column share one width); the
// header previously padded the label to 10, leaving it one column right of the data
// it titled (#438).
const DIALOGUE_COLS: &[Col] = &[
    col("Dialogue%", 9),
    col("Para~", 9),
    col("Median", 8),
    col("Longest", 9),
];
const STYLE_COLS: &[Col] = &[
    col("-ly%", 7),
    col("Filter", 8),
    col("to-be%", 7),
    col("Passive", 9),
];
const VOCAB_COLS: &[Col] = &[col("Variety", 9), col("Words", 8)];

/// The readability table: one row per chapter plus a whole-manuscript total.
fn readability_section(chapters: &[ChapterAnalysis]) -> Vec<Line<'static>> {
    let mut out = vec![dim("Readability"), header(READABILITY_COLS)];
    let mut total = Stats::default();
    for c in chapters {
        total.add(c.stats);
        out.push(Line::raw(score_text(&c.title, readability(c.stats))));
    }
    out.push(bold(score_text("Whole manuscript", readability(total))));
    out.push(dim(
        "Reading Ease: higher is easier (60–70 ≈ plain English); the rest are grades.",
    ));
    out
}

/// The sentence-length section: per-chapter statistics, the overall distribution,
/// and the longest sentences (possible run-ons).
fn sentence_section(
    chapters: &[ChapterAnalysis],
    longest: &[LongSentence],
    threshold: usize,
) -> Vec<Line<'static>> {
    let mut out = vec![
        dim(format!(
            "Sentence length (words) · run-on threshold {threshold}"
        )),
        header(SENTENCE_COLS),
    ];
    let mut all = Vec::new();
    for c in chapters {
        all.extend_from_slice(&c.sentence_lengths);
        out.push(Line::raw(sentence_text(
            &c.title,
            &sentence_stats(&c.sentence_lengths, threshold),
        )));
    }
    let overall = sentence_stats(&all, threshold);
    out.push(bold(sentence_text("Whole manuscript", &overall)));

    out.push(Line::raw(""));
    out.push(dim("Distribution (whole manuscript)"));
    out.extend(histogram_lines(&overall));

    if !longest.is_empty() {
        out.push(Line::raw(""));
        out.push(dim("Longest sentences"));
        for s in longest {
            let flag = if s.words > threshold { " ⚠" } else { "" };
            out.push(Line::raw(format!(
                "{:>4}w{flag}  [{}] {}",
                s.words,
                truncate(&s.chapter, 18),
                truncate(&s.text, 64)
            )));
        }
    }
    out
}

/// The chapter-length / pacing section: a per-chapter bar chart of word counts
/// with short/long outliers flagged, grouped by part when the book defines them,
/// plus summary statistics (mean, median, spread). Uses the rolled-up chapter
/// word counts (headings included — the size shown in the chapter list), so it
/// matches what the author sees elsewhere.
fn length_section(chapters: &[ChapterAnalysis], parts: &[PartSpan]) -> Vec<Line<'static>> {
    let words: Vec<usize> = chapters.iter().map(|c| c.words).collect();
    let stats = length_stats(&words);
    let peak = stats.max.max(1); // the longest chapter (≥1 so the bar never divides by zero)
    let mut out = vec![dim("Chapter length (words)")];

    if parts.is_empty() {
        for c in chapters {
            out.push(length_bar_row(&c.title, c.words, peak, &stats, 0));
        }
    } else {
        for p in parts {
            out.push(dim(p.name.clone()));
            let mut subtotal = 0;
            for c in p.range.clone().filter_map(|i| chapters.get(i)) {
                subtotal += c.words;
                out.push(length_bar_row(&c.title, c.words, peak, &stats, 2));
            }
            out.push(dim(format!("  {:<22}{subtotal:>7}", "Subtotal")));
        }
    }

    out.push(bold(format!(
        "Whole manuscript: {} chapters · mean {:.0} · median {:.0} · spread ±{:.0} · range {}–{}",
        stats.count, stats.mean, stats.median, stats.std_dev, stats.min, stats.max
    )));
    out
}

/// One chapter's pacing row: indented (truncated) title, word count, a bar scaled
/// to the longest chapter, and a short/long flag for outliers.
fn length_bar_row(
    title: &str,
    words: usize,
    peak: usize,
    stats: &LengthStats,
    indent: usize,
) -> Line<'static> {
    let avail = 24 - indent; // the indent + name region together pad to 24
    let name = truncate(title, avail);
    let flag = length_flag(words, stats).map_or(String::new(), |f| format!("  ⚠ {f}"));
    Line::raw(format!(
        "{:indent$}{name:<avail$}{words:>7}  {}{flag}",
        "",
        bar(words, peak)
    ))
}

/// The dialogue / paragraph section: per-chapter (and overall) dialogue share and
/// paragraph-length rhythm (mean / median / longest, in words).
fn dialogue_section(chapters: &[ChapterAnalysis]) -> Vec<Line<'static>> {
    let mut out = vec![dim("Dialogue & paragraphs"), header(DIALOGUE_COLS)];
    let mut total_dialogue = Dialogue::default();
    let mut all_paragraphs = Vec::new();
    for c in chapters {
        total_dialogue.add(c.dialogue);
        all_paragraphs.extend_from_slice(&c.paragraph_lengths);
        out.push(Line::raw(dialogue_text(
            &c.title,
            c.dialogue,
            &c.paragraph_lengths,
        )));
    }
    out.push(bold(dialogue_text(
        "Whole manuscript",
        total_dialogue,
        &all_paragraphs,
    )));
    out
}

/// One dialogue/paragraph row: dialogue share and the paragraph mean / median /
/// longest for the span.
fn dialogue_text(name: &str, dialogue: Dialogue, paragraphs: &[usize]) -> String {
    let p = spread(paragraphs);
    data_row(
        name,
        DIALOGUE_COLS,
        &[
            format!("{:.1}", dialogue.dialogue_pct()),
            format!("{:.0}", p.mean),
            format!("{:.0}", p.median),
            p.max.to_string(), // longest paragraph
        ],
    )
}

/// The style-flags section: a per-chapter (and overall) table of `-ly` adverb,
/// filter-word, weak-verb, and passive density, plus the manuscript's most
/// repeated phrases. Descriptive only — counts, never edits.
fn style_section(
    chapters: &[ChapterAnalysis],
    top_phrases: &[(String, usize)],
) -> Vec<Line<'static>> {
    let mut out = vec![dim("Style flags (frequency)"), header(STYLE_COLS)];
    let mut total = StyleFlags::default();
    for c in chapters {
        total.add(c.style);
        out.push(Line::raw(style_text(&c.title, &c.style)));
    }
    out.push(bold(style_text("Whole manuscript", &total)));
    out.extend(count_list("Repeated phrases", top_phrases));
    out
}

/// The vocabulary section: lexical variety (MATTR ×100, so higher = more varied)
/// per chapter and overall, plus the manuscript's most-used content words. The
/// overall variety is measured over the pooled manuscript tokens (see
/// [`Report::overall_variety`]), the same definition as the per-chapter rows.
fn vocabulary_section(
    chapters: &[ChapterAnalysis],
    top_words: &[(String, usize)],
    overall_variety: f64,
) -> Vec<Line<'static>> {
    let mut out = vec![dim("Vocabulary"), header(VOCAB_COLS)];
    let mut words = 0usize;
    for c in chapters {
        words += c.stats.words;
        out.push(Line::raw(vocab_text(
            &c.title,
            c.lexical_variety,
            c.stats.words,
        )));
    }
    out.push(bold(vocab_text("Whole manuscript", overall_variety, words)));
    out.extend(count_list("Most-used words", top_words));
    out
}

/// One vocabulary row: lexical variety scaled to 0–100 (unique words per 100-word
/// window) and the word count it was measured over.
fn vocab_text(name: &str, variety: f64, words: usize) -> String {
    data_row(
        name,
        VOCAB_COLS,
        &[format!("{:.1}", variety * 100.0), words.to_string()],
    )
}

/// A titled `count×  item` list (one per line, so long lists scroll rather than
/// clip), or nothing when empty. Shared by the most-used-words and repeated-phrase
/// lists.
fn count_list(title: &str, items: &[(String, usize)]) -> Vec<Line<'static>> {
    if items.is_empty() {
        return Vec::new();
    }
    let mut out = vec![Line::raw(""), dim(title.to_string())];
    out.extend(
        items
            .iter()
            .map(|(item, count)| Line::raw(format!("{count:>4}×  {item}"))),
    );
    out
}

/// One style-flags row: densities as a percent of the chapter's words.
fn style_text(name: &str, f: &StyleFlags) -> String {
    let pct = |n: usize| {
        if f.words == 0 {
            0.0
        } else {
            n as f64 * 100.0 / f.words as f64
        }
    };
    data_row(
        name,
        STYLE_COLS,
        &[
            format!("{:.1}", pct(f.adverbs_ly)),
            f.filter_words.to_string(),
            format!("{:.1}", pct(f.weak_verbs)),
            f.passive.to_string(),
        ],
    )
}

/// A horizontal bar per length bucket, scaled to the busiest bucket.
fn histogram_lines(stats: &SentenceStats) -> Vec<Line<'static>> {
    let peak = stats.histogram.iter().copied().max().unwrap_or(0).max(1);
    BUCKET_LABELS
        .iter()
        .zip(stats.histogram)
        .map(|(label, count)| Line::raw(format!("{label:>6} {} {count}", bar(count, peak))))
        .collect()
}

/// Width of the bar-chart bars, in cells.
const BAR_WIDTH: usize = 24;

/// A horizontal bar of `value` scaled to `peak` (the longest bar is full width).
fn bar(value: usize, peak: usize) -> String {
    "█".repeat(value * BAR_WIDTH / peak)
}

/// One readability row: a (truncated) name and the four scores to one decimal.
fn score_text(name: &str, r: Readability) -> String {
    data_row(
        name,
        READABILITY_COLS,
        &[
            format!("{:.1}", r.flesch_reading_ease),
            format!("{:.1}", r.flesch_kincaid_grade),
            format!("{:.1}", r.gunning_fog),
            format!("{:.1}", r.smog),
        ],
    )
}

/// One sentence-length row: a (truncated) name and the length statistics.
fn sentence_text(name: &str, s: &SentenceStats) -> String {
    data_row(
        name,
        SENTENCE_COLS,
        &[
            s.count.to_string(),
            format!("{:.1}", s.mean),
            format!("{:.1}", s.median),
            s.min.to_string(),
            s.max.to_string(),
            s.run_ons.to_string(),
        ],
    )
}

/// A bold line and a dim (dark-gray) line, the two row styles the report uses.
fn bold(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(
        text.into(),
        Style::default().add_modifier(Modifier::BOLD),
    ))
}

fn dim(text: impl Into<String>) -> Line<'static> {
    Line::from(Span::styled(
        text.into(),
        Style::default().fg(theme::current().dim),
    ))
}

/// Truncate to `max` characters with an ellipsis, counting by character so a
/// multibyte name isn't split mid-codepoint.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lexical variety of a raw string (tokenize-then-measure), for tests.
    fn variety(text: &str) -> f64 {
        let mut tokens = Vec::new();
        tally_tokens(text, &mut tokens);
        lexical_variety_tokens(&tokens)
    }

    fn chapter(title: &str, prose_text: &str, words: usize) -> ChapterAnalysis {
        ChapterAnalysis {
            title: title.to_string(),
            words,
            stats: stats(prose_text),
            sentence_lengths: sentences(prose_text)
                .iter()
                .map(|s| crate::buffer::count_words(s))
                .collect(),
            style: style_flags(prose_text, &WordLists::new()),
            dialogue: dialogue_split(prose_text),
            paragraph_lengths: paragraph_lengths(prose_text),
            lexical_variety: variety(prose_text),
        }
    }

    #[test]
    fn lines_render_all_sections_with_totals() {
        let chapters = vec![
            chapter("One", "The cat sat on the mat.", 1200),
            chapter("Two", "The dog ran fast across the yard.", 3400),
        ];
        let longest = vec![LongSentence {
            chapter: "Two".to_string(),
            words: 7,
            text: "The dog ran fast across the yard.".to_string(),
        }];
        let report = Report {
            title: "My Book",
            chapters: &chapters,
            longest: &longest,
            parts: &[],
            run_on_threshold: 40,
            top_words: &[("locket".to_string(), 9)],
            top_phrases: &[("the dog ran".to_string(), 4)],
            overall_variety: 0.7,
        };
        let rows = lines(&report);
        let text: String = rows
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("My Book — analysis"));
        assert!(text.contains("Readability"));
        assert!(text.contains("Sentence length"));
        assert!(text.contains("Distribution"));
        assert!(text.contains("Longest sentences"));
        assert!(text.contains("Chapter length"));
        assert!(text.contains("Dialogue & paragraphs"));
        assert!(text.contains("Style flags"));
        assert!(text.contains("Vocabulary"));
        assert!(text.contains("Most-used words") && text.contains("locket"));
        assert!(text.contains("Repeated phrases") && text.contains("the dog ran"));
        assert!(text.contains("One") && text.contains("Two"));
        assert_eq!(text.matches("Whole manuscript").count(), 6); // one per section
    }

    #[test]
    fn lines_group_chapter_length_by_part() {
        let chapters = vec![
            chapter("One", "A.", 1000),
            chapter("Two", "B.", 1100),
            chapter("Three", "C.", 900),
        ];
        let parts = vec![
            PartSpan {
                name: "Act I".to_string(),
                range: 0..1,
            },
            PartSpan {
                name: "Act II".to_string(),
                range: 1..3,
            },
        ];
        let report = Report {
            title: "Book",
            chapters: &chapters,
            longest: &[],
            parts: &parts,
            run_on_threshold: 40,
            top_words: &[],
            top_phrases: &[],
            overall_variety: 0.0,
        };
        let text: String = lines(&report)
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Act I") && text.contains("Act II"));
        assert!(text.contains("Subtotal"));
    }

    #[test]
    fn truncate_keeps_multibyte_names_intact() {
        let long = "Café ".repeat(10);
        let t = truncate(&long, 24);
        assert!(t.chars().count() <= 24);
        assert!(t.ends_with('…'));
    }
}
