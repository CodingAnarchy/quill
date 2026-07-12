//! The manuscript-health dashboard (#827): a consolidated read of the standing
//! revision-triage findings, grouped by kind/pass then chapter with counts, and
//! navigable so the author can jump straight to a finding. Aggregates and
//! displays the findings the analysis passes already surfaced — analysis only,
//! never prose, so it stays within the AI boundary.

use super::*;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mode::{Dashboard, DashboardRow};

/// The kinds shown in the dashboard, in header order — the single, enum-derived
/// [`FindingKind::ALL`] sequence (the continuity/structure passes first, prose/voice
/// last, [`Other`](FindingKind::Other) last), so the overview reads the same run to
/// run *and* every kind is represented. Deriving it from the enum (rather than a
/// hand-maintained list) means a new finding kind can never be silently dropped from
/// the dashboard, the health trend, or the revision-plan export (the #1069-class drift
/// that omitted Accuracy/Relationship/Foreshadow/FactCheck/Pacing here before).
const KIND_ORDER: &[FindingKind] = FindingKind::ALL;

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

/// The rank of a kind in [`KIND_ORDER`] — its fixed precedence, lower is earlier.
/// Used to pick a span's *primary* pass deterministically (#1119) so per-kind
/// attribution can't drift with pass run order. Any recognized kind ranks before
/// the fallback (`KIND_ORDER.len()`), and [`Other`](FindingKind::Other) sits last
/// in `KIND_ORDER`, so it only ever wins a span no analysis pass claimed.
fn kind_rank(k: FindingKind) -> usize {
    KIND_ORDER
        .iter()
        .position(|&o| o == k)
        .unwrap_or(KIND_ORDER.len())
}

/// Collapse cross-pass agreement into consensus findings (#921): findings that
/// several passes raise for the same span become one, folding every other pass into
/// `corroborators` so the view can weight the finding by agreement. A lone finding
/// passes through unchanged. Pure over the input, so it's directly testable.
///
/// The primary `kind` of a merged span is the highest-precedence pass by
/// [`KIND_ORDER`] (#1119), **not** whichever pass happened to surface the span
/// first. `self.triage` is appended in pass run order, so a first-seen primary made
/// the same cross-pass span tally under different kinds on different runs — the
/// per-kind health-trend line jittered even though the totals were stable. Ranking
/// the primary by the fixed `KIND_ORDER` precedence makes attribution deterministic:
/// the same set of findings always attributes a span to the same kind, whatever
/// order the passes ran. Span first-seen order (which drives the dashboard's row and
/// chapter ordering) is preserved — only *which pass owns the merged span* is made
/// order-independent.
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
    // Normalize each merged span's agreement set (#1119, see the doc comment): the
    // primary `kind` is the highest-precedence pass by `KIND_ORDER`, the corroborators
    // the rest in that same fixed order — so attribution can't drift with pass order.
    for f in &mut merged {
        let mut passes = f.agreement();
        passes.sort_by_key(|&k| kind_rank(k));
        f.kind = passes[0];
        f.corroborators = passes[1..].to_vec();
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

/// How many recorded runs the trend side-file keeps (#1052). Bounded so the file
/// stays tiny and reading it is O(1) in the manuscript's size — a handful of
/// per-kind count tallies, never anything proportional to the prose. Old runs
/// past the cap are dropped oldest-first when a new one is recorded.
const TREND_HISTORY: usize = 20;

/// One recorded health run (#1052): the date it was taken and the open-finding
/// count for each kind that had any, keyed by the kind's stable slug. Small and
/// flat — the count tallies only, never the findings' prose — so a run's record is
/// a few integers regardless of manuscript size.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
struct TrendRun {
    /// ISO date the run was recorded (`YYYY-MM-DD`).
    date: String,
    /// Per-kind open-finding counts, keyed by [`FindingKind::slug`]. Only kinds
    /// with a non-zero count are stored, so an all-clear run is an empty map.
    #[serde(default)]
    counts: std::collections::BTreeMap<String, usize>,
}

impl TrendRun {
    /// The open-finding counts of `findings`, tallied by kind — the run to record.
    /// A pure fold over the (already consensus-collapsed) findings: one increment
    /// per finding, no manuscript access, so recording is independent of project
    /// size (#1052, respecting `tally_is_independent_of_project_size`).
    fn tally(date: String, findings: &[Finding]) -> Self {
        let mut counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for f in findings {
            *counts.entry(f.kind.slug().to_string()).or_default() += 1;
        }
        Self { date, counts }
    }

    fn count(&self, kind: FindingKind) -> usize {
        self.counts.get(kind.slug()).copied().unwrap_or(0)
    }

    fn total(&self) -> usize {
        self.counts.values().sum()
    }
}

/// The durable health-trend side-file (#1052): a bounded log of recorded runs, so
/// the dashboard can show whether counts are falling (revision working) or a
/// category is getting worse — a trend, not just today's snapshot. Persisted as
/// `bible/health-trend.toml`, beside the findings store, and read straight back:
/// rendering the trend touches only these small tallies, never the manuscript.
#[derive(Serialize, Deserialize, Default)]
struct HealthTrend {
    #[serde(default)]
    runs: Vec<TrendRun>,
}

fn trend_path(root: &Path) -> PathBuf {
    root.join("bible").join("health-trend.toml")
}

impl HealthTrend {
    /// Load the trend log, or the empty default when it's missing/unreadable (best
    /// effort — a corrupt file just starts the trend fresh, never blocks the view).
    fn load(root: &Path) -> Self {
        std::fs::read_to_string(trend_path(root))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self, root: &Path) {
        let path = trend_path(root);
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        if let Ok(body) = toml::to_string(self) {
            let _ = crate::atomic::write(&path, body.as_bytes());
        }
    }

    /// The most recent run *before* `run` — the baseline the delta is measured
    /// against. A same-date run already on file is treated as that same run being
    /// re-recorded (see [`record`](Self::record)), so it's not its own baseline.
    fn previous(&self, run: &TrendRun) -> Option<&TrendRun> {
        self.runs.iter().rev().find(|r| r.date != run.date)
    }

    /// Record `run`, returning the prior run it should be compared against (its
    /// baseline) *before* the record lands. Re-recording on a date already on file
    /// replaces that day's tally in place (opening the dashboard twice in one day
    /// keeps one row for the day, updated), so the trend is per-day, not per-open.
    /// The log is capped to [`TREND_HISTORY`] runs, oldest dropped first. Persists.
    fn record(&mut self, root: &Path, run: TrendRun) -> Option<TrendRun> {
        let baseline = self.previous(&run).cloned();
        match self.runs.iter().position(|r| r.date == run.date) {
            Some(i) => self.runs[i] = run,
            None => self.runs.push(run),
        }
        let overflow = self.runs.len().saturating_sub(TREND_HISTORY);
        if overflow > 0 {
            self.runs.drain(0..overflow);
        }
        self.save(root);
        baseline
    }
}

/// The human trend line comparing the current run to its `baseline` (#1052):
/// which kinds fell (revision working), which rose (a category getting worse),
/// and the net total change — a fragment built purely from the two small tallies,
/// no manuscript read. `None` when there's no prior run to compare against (the
/// first recorded run has no trend yet). Kinds are walked in [`KIND_ORDER`] so the
/// line reads the same run to run.
fn trend_line(current: &TrendRun, baseline: &TrendRun) -> String {
    let mut down: Vec<String> = Vec::new();
    let mut up: Vec<String> = Vec::new();
    for &kind in KIND_ORDER {
        let now = current.count(kind);
        let was = baseline.count(kind);
        if now < was {
            down.push(format!("{} down {}", kind.label(), was - now));
        } else if now > was {
            up.push(format!("{} up {}", kind.label(), now - was));
        }
    }
    let net = current.total() as i64 - baseline.total() as i64;
    let direction = match net {
        n if n < 0 => format!("{} fewer findings", -n),
        0 => "no net change".to_string(),
        n => format!("{n} more findings"),
    };
    let mut parts = vec![format!("since {}: {direction}", baseline.date)];
    parts.extend(down);
    parts.extend(up);
    parts.join(" · ")
}

impl App {
    /// Record the current open findings into the health-trend side-file (#1052) and
    /// return the trend line vs the prior recorded run, or `None` on the first run.
    /// Pure aggregation of the count tallies already in `findings` — no manuscript
    /// read, no request — so it's independent of project size and safe to call each
    /// time the dashboard opens.
    fn record_health_trend(&self, findings: &[Finding]) -> Option<String> {
        let run = TrendRun::tally(date::iso(date::today()), findings);
        let mut trend = HealthTrend::load(&self.project.root);
        let baseline = trend.record(&self.project.root, run.clone())?;
        Some(trend_line(&run, &baseline))
    }

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
        // Record this run into the small trend side-file (#1052) and surface the
        // change since the last run, so the point-in-time dashboard reads as a
        // trend — continuity findings falling, a style category rising. Rendered
        // from the persisted tallies alone, never a manuscript re-scan.
        self.message = self.record_health_trend(&findings);
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
        // Fold the trend since the prior run into the snapshot (#1052): recomputed
        // from the already-recorded tallies in the side-file, so the saved health
        // read shows direction of travel, not just today's totals.
        let trend = self.recorded_trend();
        let body = dashboard_plain_text(d, trend.as_deref());
        let name = format!("health-{}", date::iso(date::today()));
        self.write_export(&name, "md", body.as_bytes());
    }

    /// The trend line for the latest recorded run vs the one before it (#1052),
    /// read back from the side-file — the same line [`open_health_dashboard`]
    /// surfaced. `None` when there's no prior run to compare against. Reads only the
    /// small persisted tallies, never the manuscript.
    fn recorded_trend(&self) -> Option<String> {
        let trend = HealthTrend::load(&self.project.root);
        let current = trend.runs.last()?;
        let baseline = trend.previous(current)?;
        Some(trend_line(current, baseline))
    }
}

/// The dashboard as plain Markdown text — the same rows the overlay shows, for a
/// saved snapshot (#827), with the trend line (#1052) under the title when there's
/// a prior run to compare against. A kind header is a `##` heading with its count,
/// a chapter a `-` bullet with its count, and each finding an indented bullet.
fn dashboard_plain_text(d: &Dashboard, trend: Option<&str>) -> String {
    let mut out = vec![format!("# Manuscript health — {} open findings", d.total)];
    if let Some(trend) = trend {
        out.push(String::new());
        out.push(format!("_Trend {trend}_"));
    }
    out.push(String::new());
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

/// A finding's revision-plan confidence (#1055/#921): how many passes agree on it.
/// A lone finding scores 1; a span two or three passes independently raised scores
/// higher and sorts to the top of its chapter, since cross-pass agreement is the
/// strongest signal the issue is real. Pure over the (already merged) finding.
fn confidence(f: &Finding) -> usize {
    f.agreement().len()
}

/// Build the chapter-ordered, confidence-ranked revision plan (#1055) from a set of
/// open findings: chapters in first-seen order (the order the author works the
/// manuscript), and within each chapter the findings sorted by confidence (#921)
/// high-to-low, ties broken by [`KIND_ORDER`] then summary so the plan is stable
/// run to run. Reformats the findings the passes already produced — it authors no
/// prose. Pure over the input, so it's directly testable. Empty input yields the
/// clean "nothing to plan" document.
pub(super) fn revision_plan_text(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return [
            "# Revision plan",
            "",
            "Nothing to plan — no open findings. Run a continuity / setting / style \
             pass, or you're all clear.",
        ]
        .join("\n");
    }

    // Chapters in first-seen order — the top-to-bottom order the author reads.
    let mut chapters: Vec<String> = Vec::new();
    for f in findings {
        let bucket = chapter_bucket(f);
        if !chapters.contains(&bucket) {
            chapters.push(bucket);
        }
    }

    let total = findings.len();
    let mut out = format!(
        "# Revision plan — {total} open finding{}\n\nWork this top-to-bottom: \
         chapters in reading order, highest-confidence findings first.\n",
        if total == 1 { "" } else { "s" }
    );

    for chapter in chapters {
        let mut in_chapter: Vec<&Finding> = findings
            .iter()
            .filter(|f| chapter_bucket(f) == chapter)
            .collect();
        // Highest confidence first; ties by kind order, then summary — a total,
        // deterministic order so the plan reads the same every export.
        in_chapter.sort_by(|a, b| {
            confidence(b)
                .cmp(&confidence(a))
                .then_with(|| kind_rank(a.kind).cmp(&kind_rank(b.kind)))
                .then_with(|| a.summary.cmp(&b.summary))
        });

        let _ = write!(out, "\n## {chapter} ({} to address)\n\n", in_chapter.len());
        for f in in_chapter {
            let conf = confidence(f);
            let stars = "★".repeat(conf);
            let signal = f
                .agreement_signal()
                .map(|s| format!(" — {s}"))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "- [ ] {stars} [{}] {}{signal}",
                f.kind.label(),
                f.summary
            );
        }
    }
    out
}

impl App {
    /// Export the open findings as a chapter-ordered, confidence-ranked revision
    /// plan (#1055): `export/revision-plan-<date>.md`, an author-workable to-do that
    /// folds in cross-pass confidence (#921) and the durable store's TODO/dismissal
    /// state (dismissed and resolved findings are already gone from `triage`, so the
    /// plan lists only what's still open). It reformats findings already produced —
    /// an off-manuscript artifact like the synopsis, authoring no prose. With no
    /// open findings it writes the clean "nothing to plan" document.
    pub(crate) fn export_revision_plan(&mut self) {
        // Collapse cross-pass agreement first (#921), so a span several passes flag
        // is one ranked entry weighted by that agreement, not per-pass duplicates.
        let findings = consensus(&self.triage);
        let body = revision_plan_text(&findings);
        let name = format!("revision-plan-{}", date::iso(date::today()));
        self.write_export(&name, "md", body.as_bytes());
    }

    /// Record a health run and write the trend snapshot to `export/health-<date>.md`
    /// (#1052) without opening the dashboard — the run-action entry point for the
    /// across-drafts trend. Records the current counts into the side-file, then
    /// writes the same grouped snapshot [`save_dashboard`] does, headed by the trend
    /// line since the prior run. Pure local aggregation of the count tallies — no
    /// request, no manuscript re-scan — so it's independent of project size.
    pub(crate) fn export_health_trend(&mut self) {
        if self.triage.is_empty() {
            self.message = Some("No findings yet — run a continuity / setting / style pass".into());
            return;
        }
        let findings = consensus(&self.triage);
        let total = findings.len();
        let rows = dashboard_rows(&findings);
        let trend = self.record_health_trend(&findings);
        let snapshot = Dashboard {
            rows,
            selected: 0,
            total,
        };
        let body = dashboard_plain_text(&snapshot, trend.as_deref());
        let name = format!("health-{}", date::iso(date::today()));
        self.write_export(&name, "md", body.as_bytes());
    }
}
