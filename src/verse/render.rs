//! The verse report (#1324, #1325): each chapter with a target meter or rhyme
//! scheme, and each fenced verse block (#1337), with its totals and every line
//! with its scansion, its rhyme, and its verdicts. The checking lives in the
//! core; this is the half that depends on `ratatui`.

use std::collections::BTreeMap;
use std::fmt::Write;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::{
    letter_name, summarize, summarize_rhyme, LineScan, Meter, RhymeFault, RhymeKind, RhymeLine,
    RhymeScheme,
};
use crate::theme;

/// One section of the report, gathered by the editor and handed to [`lines`]:
/// a chapter checked against its targets, a fenced verse block (#1337), or lines
/// the author chose to check.
pub struct ChapterVerse {
    pub title: String,
    /// A line to show under the title: what the section was checked against,
    /// or why it couldn't be.
    pub note: Option<String>,
    pub meter: Option<Meter>,
    pub rhyme: Option<RhymeScheme>,
    /// The meter scan, empty without a meter.
    pub scans: Vec<LineScan>,
    /// The rhyme check, empty without a scheme.
    pub rhymes: Vec<RhymeLine>,
}

/// The report's rows. Computed when the report opens, never per frame.
pub fn lines(book: &str, chapters: &[ChapterVerse]) -> Vec<Line<'static>> {
    let mut out = vec![bold(format!("{book}: verse")), Line::raw("")];
    out.push(dim(
        "Each line: its number; for the meter, a verdict (\u{2713} fits, ~ fits with a licence, \u{2717} breaks it),",
    ));
    out.push(dim(
        "its syllables against the pattern, and its reading (\u{2d8} unstressed, / stressed); for the rhyme,",
    ));
    out.push(dim(
        "a verdict and the letter its ending earns (heard/expected when they differ); then the text.",
    ));
    out.push(dim(
        "A word heard wrong, or guessed: put the cursor on it and run \"Set how the word at the cursor sounds\" (Ctrl-K).",
    ));
    for chapter in chapters {
        out.push(Line::raw(""));
        out.extend(chapter_lines(chapter));
    }
    out
}

fn chapter_lines(chapter: &ChapterVerse) -> Vec<Line<'static>> {
    let mut title = chapter.title.clone();
    if let Some(meter) = &chapter.meter {
        let _ = write!(title, " \u{b7} {meter}");
    }
    if let Some(rhyme) = &chapter.rhyme {
        let _ = write!(title, " \u{b7} rhymed {}", rhyme.letters());
        if rhyme.slant {
            title.push_str(" (slant rhymes count)");
        }
    }
    let mut out = vec![bold(title)];
    if let Some(note) = &chapter.note {
        out.push(dim(format!("  {note}")));
    }
    if chapter.meter.is_none() && chapter.rhyme.is_none() {
        return out;
    }
    if chapter.scans.is_empty() && chapter.rhymes.is_empty() {
        out.push(dim("  No lines to check yet."));
        return out;
    }
    let meter = summarize(&chapter.scans);
    let rhyme = summarize_rhyme(&chapter.rhymes);
    if chapter.meter.is_some() {
        let mut totals = format!(
            "  Meter: {} of {} lines fit ({}%)",
            meter.fitting,
            meter.lines,
            percent(meter.fitting, meter.lines)
        );
        if meter.with_licences > 0 {
            let _ = write!(totals, ", {} with a licence", meter.with_licences);
        }
        if let Some((kind, n)) = meter.most_common {
            let _ = write!(totals, " \u{b7} most common fault: {kind} ({n})");
        }
        out.push(Line::raw(totals));
    }
    if chapter.rhyme.is_some() {
        out.push(Line::raw(format!(
            "  Rhyme: {} of {} line endings fit ({}%)",
            rhyme.fitting,
            rhyme.lines,
            percent(rhyme.fitting, rhyme.lines)
        )));
    }
    let stanzas = meter.stanzas.len().max(rhyme.stanzas.len());
    if stanzas > 1 {
        let cells: Vec<String> = (0..stanzas)
            .map(|i| {
                let mut cell = format!("{}:", i + 1);
                if let Some((fit, total)) = meter.stanzas.get(i) {
                    let _ = write!(cell, " meter {fit}/{total}");
                }
                if let Some((fit, total)) = rhyme.stanzas.get(i) {
                    let _ = write!(cell, " rhyme {fit}/{total}");
                }
                cell
            })
            .collect();
        out.push(dim(format!("  Stanzas  {}", cells.join("  "))));
    }
    // The two checks read the same verse lines; join them by line number.
    let mut rows: BTreeMap<usize, (Option<&LineScan>, Option<&RhymeLine>, usize)> = BTreeMap::new();
    for scan in &chapter.scans {
        rows.entry(scan.line).or_insert((None, None, scan.stanza)).0 = Some(scan);
    }
    for rhyme in &chapter.rhymes {
        rows.entry(rhyme.line)
            .or_insert((None, None, rhyme.stanza))
            .1 = Some(rhyme);
    }
    let marks_width = chapter
        .scans
        .iter()
        .map(|l| l.marks.chars().count())
        .max()
        .unwrap_or(0);
    let layout = Layout {
        meter: chapter.meter.is_some(),
        rhyme: chapter.rhyme.is_some(),
        marks_width,
    };
    let mut last_stanza = None;
    for (&line, &(scan, rhyme, stanza)) in &rows {
        if last_stanza.is_some_and(|s| s != stanza) {
            out.push(Line::raw(""));
        }
        last_stanza = Some(stanza);
        out.extend(line_rows(line, scan, rhyme, &layout));
    }
    out
}

fn percent(part: usize, whole: usize) -> usize {
    (part * 100).checked_div(whole).unwrap_or(0)
}

/// Which columns a chapter's rows carry.
struct Layout {
    meter: bool,
    rhyme: bool,
    marks_width: usize,
}

/// A line's row, and a note under it naming any licence, fault, unusual rhyme,
/// or guessed word.
fn line_rows(
    number: usize,
    scan: Option<&LineScan>,
    rhyme: Option<&RhymeLine>,
    layout: &Layout,
) -> Vec<Line<'static>> {
    let t = theme::current();
    let dim_style = Style::default().fg(t.dim);
    let mut spans = vec![Span::styled(format!("  {:>4} ", number + 1), dim_style)];
    let mut notes: Vec<String> = Vec::new();
    let mut guessed: Vec<String> = Vec::new();
    let mut worst: Option<Color> = None;
    let mut text = String::new();
    if layout.meter {
        match scan {
            Some(scan) => {
                let (glyph, color) = if !scan.fits() {
                    ('\u{2717}', t.removed)
                } else if scan.variations.is_empty() {
                    ('\u{2713}', t.added)
                } else {
                    ('~', t.revised)
                };
                worst = Some(color);
                spans.push(Span::styled(glyph.to_string(), Style::default().fg(color)));
                spans.push(Span::styled(
                    format!(" {:>2}/{:<2} ", scan.syllables, scan.expected),
                    dim_style,
                ));
                spans.push(Span::raw(format!(
                    "{:<width$}  ",
                    scan.marks,
                    width = layout.marks_width
                )));
                notes.extend(scan.deviations.iter().map(ToString::to_string));
                notes.extend(scan.variations.iter().map(ToString::to_string));
                guessed.extend(scan.guessed().map(str::to_string));
                text = scan.text.trim().to_string();
            }
            None => spans.push(Span::raw(format!(
                "{:width$}",
                "",
                width = layout.marks_width + 9
            ))),
        }
    }
    if layout.rhyme {
        match rhyme {
            Some(rhyme) => {
                let (glyph, color) = if rhyme.fits() {
                    ('\u{2713}', t.added)
                } else {
                    ('\u{2717}', t.removed)
                };
                if !rhyme.fits() || worst.is_none() {
                    worst = Some(color);
                }
                let mut letter = letter_name(rhyme.heard);
                if rhyme.fault.is_some() {
                    let _ = write!(letter, "/{}", letter_name(rhyme.expected));
                }
                spans.push(Span::styled(glyph.to_string(), Style::default().fg(color)));
                spans.push(Span::raw(format!(" {letter:<5} ")));
                notes.extend(rhyme_note(rhyme));
                if rhyme.guessed && !guessed.contains(&rhyme.word) {
                    guessed.push(rhyme.word.clone());
                }
            }
            None => spans.push(Span::raw(" ".repeat(8))),
        }
    }
    if text.is_empty() {
        if let Some(rhyme) = rhyme {
            text = rhyme_text(rhyme);
        }
    }
    spans.push(Span::raw(text));
    if !guessed.is_empty() {
        notes.push(format!("guessed: {}", guessed.join(", ")));
    }
    let mut out = vec![Line::from(spans)];
    if !notes.is_empty() {
        out.push(Line::from(Span::styled(
            format!("{:>16}{}", "", notes.join("; ")),
            Style::default().fg(worst.unwrap_or(t.dim)),
        )));
    }
    out
}

/// The line's text, for a chapter with no meter scan to carry it.
fn rhyme_text(rhyme: &RhymeLine) -> String {
    rhyme.text.trim().to_string()
}

/// What to say about a line's rhyme: its fault, or how it rhymes when that's
/// anything but perfect.
fn rhyme_note(rhyme: &RhymeLine) -> Option<String> {
    match (rhyme.fault, rhyme.partner) {
        (Some(RhymeFault::Missing { with, kind }), _) => Some(format!(
            "\"{}\" doesn't rhyme with line {} ({kind})",
            rhyme.word,
            with + 1
        )),
        (Some(RhymeFault::Unwanted { with }), _) => Some(format!(
            "\"{}\" rhymes with line {}, where the scheme wants a new sound",
            rhyme.word,
            with + 1
        )),
        (None, Some((with, kind))) if kind != RhymeKind::Perfect => {
            Some(format!("{kind} with line {}", with + 1))
        }
        _ => None,
    }
}

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
