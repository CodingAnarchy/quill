//! The manuscript-health dashboard (#827): a consolidated read of the standing
//! revision-triage findings, grouped by kind/pass then chapter with counts, and
//! navigable so the author can jump straight to a finding. Aggregates and
//! displays the findings the analysis passes already surfaced — analysis only,
//! never prose, so it stays within the AI boundary.

use super::*;

use crate::mode::{Dashboard, DashboardRow};

/// The kinds shown in the dashboard, in header order — a stable, meaningful
/// sequence (the continuity/structure passes first, prose/voice last), so the
/// overview reads the same run to run. [`FindingKind::Other`] is intentionally
/// last: it collects the non-pass findings (Q&A citations) that rarely reach
/// triage but shouldn't be dropped if they do.
const KIND_ORDER: &[FindingKind] = &[
    FindingKind::Continuity,
    FindingKind::Setting,
    FindingKind::Plot,
    FindingKind::Theme,
    FindingKind::Scaffold,
    FindingKind::Naming,
    FindingKind::Style,
    FindingKind::Editorial,
    FindingKind::Voice,
    FindingKind::Echo,
    FindingKind::Other,
];

/// The chapter bucket a finding sorts under in the dashboard: its first (jump-
/// target) chapter, or a placeholder for a finding with no chapter.
const NO_CHAPTER: &str = "(no chapter)";

fn chapter_bucket(f: &Finding) -> String {
    f.chapters
        .first()
        .map_or_else(|| NO_CHAPTER.to_string(), Clone::clone)
}

/// The *span* two findings share when they're the same issue seen by different
/// passes (#921): the summary + chapters + book, ignoring the kind. Findings that
/// agree on this — one dangling thread flagged by both continuity and plot — are
/// merged into a single row weighted by that agreement.
fn span_of(f: &Finding) -> (String, Vec<String>, Option<usize>) {
    (f.summary.clone(), f.chapters.clone(), f.book)
}

/// Collapse cross-pass agreement into consensus findings (#921): findings that
/// several passes raise for the same span become one, keeping the first pass's
/// `kind` as the primary lens and folding every other pass into `corroborators`,
/// so the view can weight the finding by agreement. First-seen order is preserved
/// (the primary is whichever pass surfaced the span first). A lone finding passes
/// through unchanged. Pure over the input, so it's directly testable.
pub(super) fn consensus(findings: &[Finding]) -> Vec<Finding> {
    let mut merged: Vec<Finding> = Vec::new();
    for f in findings {
        let span = span_of(f);
        if let Some(existing) = merged.iter_mut().find(|m| span_of(m) == span) {
            existing.corroborate(f);
        } else {
            merged.push(f.clone());
        }
    }
    merged
}

/// Build the dashboard's flat, ordered row list (#827) from a set of open
/// findings: for each kind that has any, a kind header with its count, then —
/// under it — a chapter sub-header (with count) and that chapter's findings, in
/// first-seen order. Pure over the input, so it's directly testable.
pub(super) fn dashboard_rows(findings: &[Finding]) -> Vec<DashboardRow> {
    let mut rows = Vec::new();
    for &kind in KIND_ORDER {
        let of_kind: Vec<&Finding> = findings.iter().filter(|f| f.kind == kind).collect();
        if of_kind.is_empty() {
            continue;
        }
        rows.push(DashboardRow::Kind {
            label: kind.label(),
            count: of_kind.len(),
        });
        // Chapters in first-seen order within this kind, each with its findings.
        let mut chapters: Vec<String> = Vec::new();
        for f in &of_kind {
            let bucket = chapter_bucket(f);
            if !chapters.contains(&bucket) {
                chapters.push(bucket);
            }
        }
        for chapter in chapters {
            let in_chapter: Vec<&Finding> = of_kind
                .iter()
                .copied()
                .filter(|f| chapter_bucket(f) == chapter)
                .collect();
            rows.push(DashboardRow::Chapter {
                title: chapter,
                count: in_chapter.len(),
            });
            for f in in_chapter {
                rows.push(DashboardRow::Finding(f.clone()));
            }
        }
    }
    rows
}

impl App {
    /// Open the manuscript-health dashboard (#827): the standing triage findings
    /// grouped by kind/pass then chapter with counts, navigable and jumpable. A
    /// read of the accumulated triage state — no request, no fresh pass. When
    /// there are no findings yet it just reports, like the flat triage list.
    pub(crate) fn open_health_dashboard(&mut self) {
        self.overlay = None;
        if self.triage.is_empty() {
            self.message = Some("No findings yet — run a continuity / setting / style pass".into());
            return;
        }
        // Collapse cross-pass agreement first (#921): a span several passes flag is
        // one finding weighted by consensus, so the dashboard count and rows reflect
        // distinct issues, not per-pass duplicates.
        let findings = consensus(&self.triage);
        let total = findings.len();
        let rows = dashboard_rows(&findings);
        let selected = rows.iter().position(|r| r.finding().is_some()).unwrap_or(0);
        self.mode = Mode::Dashboard(Box::new(Dashboard {
            rows,
            selected,
            total,
        }));
    }

    /// Handle a key while the health dashboard is open (#827): the arrows move
    /// between finding rows (headers are skipped), Enter jumps to the selected
    /// finding's chapter, `s` saves the overview to `export/`, Esc closes.
    pub(crate) fn on_key_dashboard(&mut self, key: KeyEvent) {
        let Mode::Dashboard(d) = &self.mode else {
            return;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                let s = self.dashboard_step(false);
                self.set_dashboard_selected(s);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let s = self.dashboard_step(true);
                self.set_dashboard_selected(s);
            }
            KeyCode::Enter => {
                let item = d.finding_at(d.selected).cloned();
                self.mode = self.editing_mode();
                if let Some(item) = item {
                    self.jump_to_finding(&item);
                }
            }
            KeyCode::Char('s') => self.save_dashboard(),
            KeyCode::Esc => self.mode = Mode::Insert,
            _ => {}
        }
    }

    /// The next selectable (finding) row in `forward` direction, skipping the
    /// header rows; clamps at the ends. Returns the current selection when there's
    /// nowhere to move.
    fn dashboard_step(&self, forward: bool) -> usize {
        let Mode::Dashboard(d) = &self.mode else {
            return 0;
        };
        let stops = d.finding_rows();
        stops.iter().position(|&r| r == d.selected).map_or_else(
            // Selection isn't on a finding row (shouldn't happen): snap to the first.
            || stops.first().copied().unwrap_or(d.selected),
            |pos| {
                let next = if forward {
                    (pos + 1).min(stops.len().saturating_sub(1))
                } else {
                    pos.saturating_sub(1)
                };
                stops.get(next).copied().unwrap_or(d.selected)
            },
        )
    }

    fn set_dashboard_selected(&mut self, row: usize) {
        if let Mode::Dashboard(d) = &mut self.mode {
            d.selected = row;
        }
    }

    /// Write the dashboard's rendered overview to `export/health-<date>.md` (#827),
    /// reusing the same export path the report snapshots (#601) use, so a health
    /// read can be filed alongside the progress / analysis reports.
    fn save_dashboard(&mut self) {
        let Mode::Dashboard(d) = &self.mode else {
            return;
        };
        let body = dashboard_plain_text(d);
        let name = format!("health-{}", date::iso(date::today()));
        self.write_export(&name, "md", body.as_bytes());
    }
}

/// The dashboard as plain Markdown text — the same rows the overlay shows, for a
/// saved snapshot (#827). A kind header is a `##` heading with its count, a
/// chapter a `-` bullet with its count, and each finding an indented bullet.
fn dashboard_plain_text(d: &Dashboard) -> String {
    let mut out = vec![
        format!("# Manuscript health — {} open findings", d.total),
        String::new(),
    ];
    for row in &d.rows {
        match row {
            DashboardRow::Kind { label, count } => {
                out.push(format!("## {label} ({count})"));
            }
            DashboardRow::Chapter { title, count } => {
                out.push(format!("- {title} ({count})"));
            }
            DashboardRow::Finding(f) => {
                let signal = f
                    .agreement_signal()
                    .map(|s| format!(" ({s})"))
                    .unwrap_or_default();
                out.push(format!("    - {}{signal}", f.summary));
            }
        }
    }
    out.join("\n")
}
