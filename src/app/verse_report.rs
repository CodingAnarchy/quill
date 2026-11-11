//! The verse report (#1324, #1325) and the verse it covers: each chapter with a
//! target meter or rhyme scheme, each fenced verse block inside a chapter
//! (#1337), and, on request, lines the author picks and names targets for
//! (#1337). Like the rest of the verse tools, it only reports; the text is
//! never changed.

use super::*;

use crate::verse::lexicon::Lexicon;
use crate::verse::{shift_lines, verse_blocks, ChapterVerse, Targets, VerseBlock};

impl App {
    /// Open the verse report overlay: every chapter with a target meter or rhyme
    /// scheme, and every fenced verse block, checked line by line. With nothing
    /// to check anywhere, say how to set a target rather than open an empty
    /// report. Reads each chapter once, like the analysis report.
    pub(crate) fn open_verse_report(&mut self) {
        let active = self.project.active_index();
        let lexicon = self.project.lexicon();
        let mut sections = Vec::new();
        for i in 0..self.chapter_count() {
            let chapter = self.chapter_targets(i);
            let text = self.chapter_text_with_active(i, active);
            let title = self.project.chapter_title(i).unwrap_or("Untitled");
            if !chapter.is_empty() {
                sections.push(section(
                    title.to_string(),
                    &text,
                    chapter.clone(),
                    0,
                    lexicon,
                ));
            }
            if text.contains("```") {
                for block in verse_blocks(&text) {
                    sections.push(block_section(title, &text, &block, &chapter, lexicon));
                }
            }
        }
        if sections.is_empty() {
            self.message = Some(
                "Nothing to check: set a target meter or rhyme scheme with V or H in the chapter browser (Ctrl-G), fence a poem with ```verse, or select lines and run \"Check the selected lines as verse\"."
                    .into(),
            );
            return;
        }
        let lines = verse::lines(&self.project.manifest.title, &sections);
        self.overlay = Some(Overlay::report("verse", lines));
    }

    /// The chapter's own targets, from its front matter.
    pub(super) fn chapter_targets(&self, index: usize) -> Targets {
        Targets {
            meter: self.project.chapter_meter(index).cloned(),
            rhyme: self.project.chapter_rhyme(index).cloned(),
        }
    }

    /// Ask what to check the selected lines against: the Visual selection the
    /// action menu was opened over, else the stanza at the cursor. Offers the
    /// targets used last, else the chapter's.
    pub(crate) fn prompt_verse_check(&mut self) {
        let selection = self.palette_selection.take();
        let buffer = self.edit_buffer();
        let lines = match selection {
            Some((s, e)) => Some((
                buffer.line_of_byte(s),
                buffer.line_of_byte(e.saturating_sub(1).max(s)),
            )),
            None => stanza_at(buffer, buffer.line_of_byte(buffer.cursor_byte())),
        };
        let Some((first, last)) = lines else {
            self.message = Some(
                "Select the lines to check (v, or drag), or put the cursor in a stanza.".into(),
            );
            return;
        };
        let start = buffer.line_start_byte(first);
        let end = buffer.line_start_byte(last) + buffer.line(last).len();
        let count = last - first + 1;
        let prefill = if self.verse_check_last.is_empty() {
            self.chapter_targets(self.chapter_at_byte(start).0).key()
        } else {
            self.verse_check_last.clone()
        };
        self.verse_check = Some((start, end));
        let label = format!(
            "Check {count} line{} as (a meter, a rhyme scheme, or both, e.g. iambic tetrameter abab): ",
            if count == 1 { "" } else { "s" }
        );
        self.open_prompt(Pending::CheckVerse, label, &prefill, None);
    }

    /// Check the prompted lines against the targets typed, and show the result
    /// in the verse report overlay, numbered as lines of their chapter.
    pub(crate) fn apply_verse_check(&mut self, text: &str) {
        let Some((start, end)) = self.verse_check.take() else {
            return;
        };
        let Some(targets) = Targets::parse(text).filter(|t| !t.is_empty()) else {
            self.message = Some(format!(
                "Name a meter, a rhyme scheme, or both ({METER_EXAMPLES}; {RHYME_EXAMPLES})."
            ));
            return;
        };
        self.verse_check_last = targets.key();
        let (index, chapter_start) = self.chapter_at_byte(start);
        let buffer = self.edit_buffer();
        let end = end.min(buffer.len_bytes());
        let start = start.min(end);
        let passage = buffer.byte_slice(start, end);
        let first = buffer.line_of_byte(start) - buffer.line_of_byte(chapter_start);
        let count = passage.lines().count().max(1);
        let chapter = self.project.chapter_title(index).unwrap_or("Untitled");
        let title = if count == 1 {
            format!("{chapter} \u{b7} line {}", first + 1)
        } else {
            format!(
                "{chapter} \u{b7} lines {}\u{2013}{}",
                first + 1,
                first + count
            )
        };
        let checked = section(title, &passage, targets, first, self.project.lexicon());
        let lines = verse::lines(&self.project.manifest.title, &[checked]);
        self.overlay = Some(Overlay::report("verse", lines));
    }

    /// The chapter holding buffer byte `byte`, and where that chapter starts in
    /// the buffer: the continuous view's segment, else the active chapter.
    fn chapter_at_byte(&self, byte: usize) -> (usize, usize) {
        self.manuscript
            .as_ref()
            .and_then(|m| {
                let spans = m.segment_spans();
                spans
                    .iter()
                    .find(|&&(_, s, e)| s <= byte && byte < e)
                    .or_else(|| spans.last())
                    .map(|&(i, s, _)| (i, s))
            })
            .unwrap_or_else(|| (self.project.active_index(), 0))
    }
}

/// The lines of the stanza around line `at`: the run of non-blank lines that
/// holds it. `None` on a blank line.
fn stanza_at(buffer: &Buffer, at: usize) -> Option<(usize, usize)> {
    let blank = |l: usize| buffer.line(l).trim().is_empty();
    if blank(at) {
        return None;
    }
    let mut first = at;
    while first > 0 && !blank(first - 1) {
        first -= 1;
    }
    let mut last = at;
    while last + 1 < buffer.line_count() && !blank(last + 1) {
        last += 1;
    }
    Some((first, last))
}

/// A report section: `text` checked against `targets`, its lines numbered from
/// `first_line`.
fn section(
    title: String,
    text: &str,
    targets: Targets,
    first_line: usize,
    lexicon: &Lexicon,
) -> ChapterVerse {
    let mut scans = targets
        .meter
        .as_ref()
        .map(|m| verse::scan_meter_with(text, m, lexicon))
        .unwrap_or_default();
    let mut rhymes = targets
        .rhyme
        .as_ref()
        .map(|r| verse::check_rhyme_with(text, r, lexicon))
        .unwrap_or_default();
    shift_lines(&mut scans, &mut rhymes, first_line);
    ChapterVerse {
        title,
        meter: targets.meter,
        rhyme: targets.rhyme,
        scans,
        rhymes,
        note: None,
    }
}

/// A fenced verse block's section: checked against its own targets, or the
/// chapter's when it names none, with a note when it has nothing to check.
fn block_section(
    chapter_title: &str,
    text: &str,
    block: &VerseBlock,
    chapter: &Targets,
    lexicon: &Lexicon,
) -> ChapterVerse {
    let title = format!(
        "{chapter_title} \u{b7} verse from line {}",
        block.first_line + 1
    );
    let inner = text.get(block.inner.clone()).unwrap_or("");
    let mut out = section(
        title,
        inner,
        block.targets_within(chapter),
        block.first_line,
        lexicon,
    );
    out.note = match &block.targets {
        None => Some(format!(
            "Couldn't read \"{}\" as a meter or rhyme scheme ({METER_EXAMPLES}; {RHYME_EXAMPLES}).",
            block.written
        )),
        Some(own) if own.is_empty() && chapter.is_empty() => Some(
            "No target to check against: name one on the fence line, e.g. ```verse iambic tetrameter abab."
                .into(),
        ),
        Some(own) if own.is_empty() => Some("Checked against the chapter's targets.".into()),
        Some(_) => None,
    };
    out
}
