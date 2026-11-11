//! The verse report (#1324): each chapter with a target meter, its totals, and
//! every line with its scansion and verdict. The scanning lives in the core; this
//! is the half that depends on `ratatui`.

use std::fmt::Write;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::{summarize, LineScan, Meter};
use crate::theme;

/// One chapter's scan, gathered by the editor and handed to [`lines`].
pub struct ChapterVerse {
    pub title: String,
    pub meter: Meter,
    pub lines: Vec<LineScan>,
}

/// The report's rows. Computed when the report opens, never per frame.
pub fn lines(book: &str, chapters: &[ChapterVerse]) -> Vec<Line<'static>> {
    let mut out = vec![bold(format!("{book}: verse")), Line::raw("")];
    out.push(dim(
        "Each line: its number, a verdict (\u{2713} fits, ~ fits with a licence, \u{2717} breaks the meter),",
    ));
    out.push(dim(
        "its syllables against the pattern, its reading (\u{2d8} unstressed, / stressed), and the text.",
    ));
    for chapter in chapters {
        out.push(Line::raw(""));
        out.extend(chapter_lines(chapter));
    }
    out
}

fn chapter_lines(chapter: &ChapterVerse) -> Vec<Line<'static>> {
    let summary = summarize(&chapter.lines);
    let mut out = vec![bold(format!("{} \u{b7} {}", chapter.title, chapter.meter))];
    if summary.lines == 0 {
        out.push(dim("  No lines to scan yet."));
        return out;
    }
    let mut totals = format!(
        "  {} of {} lines fit ({}%)",
        summary.fitting,
        summary.lines,
        summary.fitting * 100 / summary.lines
    );
    if summary.with_licences > 0 {
        let _ = write!(totals, ", {} with a licence", summary.with_licences);
    }
    if let Some((kind, n)) = summary.most_common {
        let _ = write!(totals, " \u{b7} most common fault: {kind} ({n})");
    }
    out.push(Line::raw(totals));
    if summary.stanzas.len() > 1 {
        let stanzas: Vec<String> = summary
            .stanzas
            .iter()
            .enumerate()
            .map(|(i, (fit, total))| format!("{}: {fit}/{total}", i + 1))
            .collect();
        out.push(dim(format!("  Stanzas  {}", stanzas.join("  "))));
    }
    let marks_width = chapter
        .lines
        .iter()
        .map(|l| l.marks.chars().count())
        .max()
        .unwrap_or(0);
    let mut stanza = None;
    for line in &chapter.lines {
        if stanza.is_some_and(|s| s != line.stanza) {
            out.push(Line::raw(""));
        }
        stanza = Some(line.stanza);
        out.extend(line_rows(line, marks_width));
    }
    out
}

/// A scanned line's row, and a note under it naming any licence, fault, or
/// guessed word.
fn line_rows(line: &LineScan, marks_width: usize) -> Vec<Line<'static>> {
    let t = theme::current();
    let (glyph, color) = if !line.fits() {
        ('\u{2717}', t.removed)
    } else if line.variations.is_empty() {
        ('\u{2713}', t.added)
    } else {
        ('~', t.revised)
    };
    let row = Line::from(vec![
        Span::styled(
            format!("  {:>4} ", line.line + 1),
            Style::default().fg(t.dim),
        ),
        Span::styled(glyph.to_string(), Style::default().fg(color)),
        Span::styled(
            format!(" {:>2}/{:<2} ", line.syllables, line.expected),
            Style::default().fg(t.dim),
        ),
        Span::raw(format!("{:<marks_width$}  ", line.marks)),
        Span::raw(line.text.trim().to_string()),
    ]);
    let mut notes: Vec<String> = line
        .deviations
        .iter()
        .map(ToString::to_string)
        .chain(line.variations.iter().map(ToString::to_string))
        .collect();
    let guessed: Vec<&str> = line.guessed().collect();
    if !guessed.is_empty() {
        notes.push(format!("guessed: {}", guessed.join(", ")));
    }
    let mut out = vec![row];
    if !notes.is_empty() {
        out.push(Line::from(Span::styled(
            format!("{:>16}{}", "", notes.join("; ")),
            Style::default().fg(color),
        )));
    }
    out
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
