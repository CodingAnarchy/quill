//! The motif / character co-occurrence report (#922): a combinatorial read over
//! the character (#74) and theme / motif (#828) bibles, cross-referencing the
//! chapters each entry already appears in to surface co-occurring pairs (which
//! motifs/characters cluster together, and how tightly) and notable absences (an
//! entry conspicuously missing from a run of chapters). It reads facts the
//! trackers hold and reports the pattern — no request, no new model, no prose —
//! staying within the AI boundary (observe, never write). Reuses the health
//! dashboard's overlay machinery: a flat, navigable row list, jumpable to the
//! relevant chapter, saveable to `export/`.

use super::*;

use crate::ai::cooccurrence::{self, Absence, Entry, EntryKind, Pairing, Report};
use crate::mode::{CoOccurrence, CoOccurrenceRow};

impl App {
    /// Open the co-occurrence report (#922): read the character and theme bibles'
    /// chapter-appearance data, compute the co-occurring pairs and notable
    /// absences, and show them as a navigable, jumpable overlay. A read of the
    /// standing bible data — no request, no fresh pass. Reports when there's no
    /// bible yet, or when the read finds nothing worth surfacing.
    pub(crate) fn open_cooccurrence(&mut self) {
        self.overlay = None;
        let entries = self.bible_entries();
        if entries.is_empty() {
            self.message = Some(
                "No character or theme bible yet — run an AI tracking pass (Ctrl-A) first.".into(),
            );
            return;
        }
        let order = self.chapter_order();
        let report = cooccurrence::analyze(&entries, &order);
        if report.is_empty() {
            self.message =
                Some("No co-occurrence patterns or notable absences in the bible yet.".into());
            return;
        }
        let rows = cooccurrence_rows(&report);
        let selected = rows.iter().position(|r| r.chapter().is_some()).unwrap_or(0);
        self.mode = Mode::CoOccurrence(Box::new(CoOccurrence { rows, selected }));
    }

    /// The tracked bible entries (#922): every character (arc-beat chapters) and
    /// motif (appearance chapters) read back from the on-disk bibles, as the common
    /// [`Entry`] shape the analysis works over. Empty entries (no chapters) are kept
    /// out — they can neither co-occur nor gap.
    fn bible_entries(&self) -> Vec<Entry> {
        let root = &self.project.root;
        let mut entries = Vec::new();
        for a in bible::character_appearances(root) {
            let entry = Entry::new(a.name, EntryKind::Character, &a.chapters);
            if !entry.chapters.is_empty() {
                entries.push(entry);
            }
        }
        for a in bible::motif_appearances(root) {
            let entry = Entry::new(a.name, EntryKind::Motif, &a.chapters);
            if !entry.chapters.is_empty() {
                entries.push(entry);
            }
        }
        entries
    }

    /// The active book's chapter titles in manuscript order — the spine the notable
    /// absences are measured against.
    fn chapter_order(&self) -> Vec<String> {
        (0..self.chapter_count())
            .filter_map(|i| self.project.chapter_title(i).map(str::to_string))
            .collect()
    }

    /// Handle a key while the co-occurrence report is open (#922): the arrows move
    /// between jumpable rows (headers are skipped), Enter jumps to the selected
    /// row's chapter, `s` saves the report to `export/`, Esc closes.
    pub(crate) fn on_key_cooccurrence(&mut self, key: KeyEvent) {
        let Mode::CoOccurrence(c) = &self.mode else {
            return;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                let s = self.cooccurrence_step(false);
                self.set_cooccurrence_selected(s);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let s = self.cooccurrence_step(true);
                self.set_cooccurrence_selected(s);
            }
            KeyCode::Enter => {
                let chapter = c.chapter_at(c.selected).map(str::to_string);
                self.mode = self.editing_mode();
                if let Some(chapter) = chapter {
                    self.jump_to_chapter(&chapter);
                }
            }
            KeyCode::Char('s') => self.save_cooccurrence(),
            KeyCode::Esc => self.mode = Mode::Insert,
            _ => {}
        }
    }

    /// The next selectable (jumpable) row in `forward` direction, skipping headers;
    /// clamps at the ends.
    fn cooccurrence_step(&self, forward: bool) -> usize {
        let Mode::CoOccurrence(c) = &self.mode else {
            return 0;
        };
        let stops = c.jump_rows();
        stops.iter().position(|&r| r == c.selected).map_or_else(
            || stops.first().copied().unwrap_or(c.selected),
            |pos| {
                let next = if forward {
                    (pos + 1).min(stops.len().saturating_sub(1))
                } else {
                    pos.saturating_sub(1)
                };
                stops.get(next).copied().unwrap_or(c.selected)
            },
        )
    }

    fn set_cooccurrence_selected(&mut self, row: usize) {
        if let Mode::CoOccurrence(c) = &mut self.mode {
            c.selected = row;
        }
    }

    /// Jump the editor to a chapter by title (#922) — activate it in the active
    /// book, reporting when the chapter can't be found.
    fn jump_to_chapter(&mut self, title: &str) {
        if let Some(i) =
            (0..self.chapter_count()).find(|&i| self.project.chapter_title(i) == Some(title))
        {
            self.activate_chapter(i);
        } else {
            self.message = Some(format!("Chapter '{title}' not found"));
        }
    }

    /// Write the report's rendered text to `export/co-occurrence-<date>.md` (#922),
    /// reusing the export path the other report snapshots use.
    fn save_cooccurrence(&mut self) {
        let Mode::CoOccurrence(c) = &self.mode else {
            return;
        };
        let body = cooccurrence_plain_text(&c.rows);
        let name = format!("co-occurrence-{}", date::iso(date::today()));
        self.write_export(&name, "md", body.as_bytes());
    }
}

/// The display line for a co-occurring pair: the two entries (each with its kind),
/// how tightly they cluster (as a percentage), and the shared chapters.
fn pair_line(p: &Pairing) -> String {
    let pct = (p.tightness * 100.0).round() as u32;
    format!(
        "{} ({}) + {} ({}) — {}% shared: {}",
        p.a,
        p.a_kind.label(),
        p.b,
        p.b_kind.label(),
        pct,
        p.shared.join(", ")
    )
}

/// The display line for a notable absence: the entry, its kind, and the run of
/// chapters it's conspicuously missing from.
fn absence_line(a: &Absence) -> String {
    format!(
        "{} ({}) — absent from: {}",
        a.name,
        a.kind.label(),
        a.missing.join(", ")
    )
}

/// Build the flat, ordered row list from a report (#922): a "Co-occurring" section
/// of pairs (each jumping to its first shared chapter) then a "Notable absences"
/// section (each jumping to its first missing chapter). Pure over the input, so
/// it's directly testable. Empty sections are omitted.
fn cooccurrence_rows(report: &Report) -> Vec<CoOccurrenceRow> {
    let mut rows = Vec::new();
    if !report.pairings.is_empty() {
        rows.push(CoOccurrenceRow::Header("Co-occurring"));
        for p in &report.pairings {
            // A reported pair always shares at least one chapter (the jump target).
            if let Some(chapter) = p.shared.first() {
                rows.push(CoOccurrenceRow::Pair {
                    line: pair_line(p),
                    chapter: chapter.clone(),
                });
            }
        }
    }
    if !report.absences.is_empty() {
        rows.push(CoOccurrenceRow::Header("Notable absences"));
        for a in &report.absences {
            if let Some(chapter) = a.missing.first() {
                rows.push(CoOccurrenceRow::Absence {
                    line: absence_line(a),
                    chapter: chapter.clone(),
                });
            }
        }
    }
    rows
}

/// The report as plain Markdown — the same rows the overlay shows, for a saved
/// snapshot (#922): a section header is a `##` heading, each pair / absence an
/// indented bullet.
fn cooccurrence_plain_text(rows: &[CoOccurrenceRow]) -> String {
    let mut out = vec![
        "# Motif & character co-occurrence".to_string(),
        String::new(),
    ];
    for row in rows {
        match row {
            CoOccurrenceRow::Header(label) => out.push(format!("## {label}")),
            CoOccurrenceRow::Pair { line, .. } | CoOccurrenceRow::Absence { line, .. } => {
                out.push(format!("- {line}"));
            }
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(titles: &[&str]) -> Vec<String> {
        titles.iter().map(|s| (*s).to_string()).collect()
    }

    /// A report over sample bible data: two motifs always paired, plus a character
    /// with a conspicuous mid-book gap.
    fn sample_report() -> Report {
        let order = ch(&["One", "Two", "Three", "Four", "Five"]);
        let entries = vec![
            Entry::new("Mirrors", EntryKind::Motif, &ch(&["One", "Two"])),
            Entry::new("Locket", EntryKind::Motif, &ch(&["One", "Two"])),
            Entry::new("Ada", EntryKind::Character, &ch(&["One", "Five"])),
        ];
        cooccurrence::analyze(&entries, &order)
    }

    #[test]
    fn rows_list_pairs_then_absences_with_jump_targets() {
        let rows = cooccurrence_rows(&sample_report());
        // The pair section leads, then the absence section.
        let headers: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                CoOccurrenceRow::Header(h) => Some(*h),
                _ => None,
            })
            .collect();
        assert_eq!(headers, vec!["Co-occurring", "Notable absences"]);

        // The Mirrors+Locket pair is present, jumps to their first shared chapter.
        let pair = rows.iter().find_map(|r| match r {
            CoOccurrenceRow::Pair { line, chapter } => Some((line.clone(), chapter.clone())),
            _ => None,
        });
        let (line, chapter) = pair.expect("a co-occurring pair");
        assert!(line.contains("Mirrors"));
        assert!(line.contains("Locket"));
        assert!(line.contains("100%"), "always-paired reads 100%: {line}");
        assert_eq!(chapter, "One");

        // Ada's mid-book gap (Two–Four) is the notable absence; it jumps to "Two".
        let absence = rows.iter().find_map(|r| match r {
            CoOccurrenceRow::Absence { line, chapter } => Some((line.clone(), chapter.clone())),
            _ => None,
        });
        let (line, chapter) = absence.expect("a notable absence");
        assert!(line.contains("Ada"));
        assert!(line.contains("Two"));
        assert_eq!(chapter, "Two", "jumps to the first missing chapter");
    }

    #[test]
    fn plain_text_snapshot_has_both_sections() {
        let text = cooccurrence_plain_text(&cooccurrence_rows(&sample_report()));
        assert!(text.starts_with("# Motif & character co-occurrence"));
        assert!(text.contains("## Co-occurring"));
        assert!(text.contains("## Notable absences"));
        assert!(text.contains("Mirrors"));
        assert!(text.contains("Ada"));
    }

    #[test]
    fn empty_report_yields_no_rows() {
        assert!(cooccurrence_rows(&Report::default()).is_empty());
    }

    #[test]
    fn only_jumpable_rows_are_selectable() {
        let rows = cooccurrence_rows(&sample_report());
        let state = CoOccurrence { rows, selected: 0 };
        // Every stop is a Pair/Absence row (never a header).
        for &i in &state.jump_rows() {
            assert!(state.chapter_at(i).is_some());
        }
    }
}
