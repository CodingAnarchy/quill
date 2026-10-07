//! AI-feature orchestration for [`App`]: the worker-thread request lifecycle
//! (`spawn_ai`/`poll_ai`), each tracker's analyze/apply pair, the shared
//! story-bible write+conflict+findings flow, and the bible-review / findings key
//! handlers. Split out of `app/mod.rs` to keep that module focused on core
//! editor state and key dispatch (a child module, so it keeps access to `App`'s
//! private fields). See `docs/ai-features.md`.
//!
//! Split by concern into submodules — [`ask`] (the "Ask the manuscript" Q&A and
//! its history), [`reference`] (the thesaurus / name-generator lookups),
//! [`trackers`] (the story-bible analyze/apply passes), [`writers`] (the
//! off-manuscript submission / marketing / social / ad copy), [`recheck`] (the
//! single-finding re-check), [`triage`], and [`workflow`] (workflow generation).
//! The shared infrastructure — the AI-result unwrap/overlay helpers, the finding
//! mapping the apply handlers and the re-check both use, and the navigable-list
//! movement key — lives here, re-exported so callers elsewhere in `app` are
//! unaffected by the split.

use super::worker::bible_tracker;
use super::*;

use crate::ai::naming;
use crate::ai::scaffold;
use crate::ai::style;
use crate::ai::voice;
use crate::mode::AiAction;

mod ask;
mod recheck;
mod reference;
pub(in crate::app) mod retrieve;
mod similar;
mod trackers;
mod triage;
mod verify_idle;
mod workflow;
mod writers;

pub use ask::QaEntry;
pub(in crate::app) use verify_idle::DigestMemo;

/// A one-line status message for one or more failed writes (#153).
pub(super) fn write_failure_message(failures: &[String]) -> String {
    match failures.split_first() {
        Some((first, [])) => format!("Couldn't write {first}"),
        Some((first, rest)) => format!("Couldn't write {} files — first: {first}", rest.len() + 1),
        None => String::new(),
    }
}

/// Map a pass's typed findings into UI [`Finding`]s, attributing each to `book`
/// (`None` for an active-book pass) and to the `action` that produced it (so a
/// single finding can be rechecked by re-running its pass, #608). `describe`
/// yields the one-line summary and the chapters for one item — the single place
/// the analysis applies share, so a change to `Finding` touches here, not seven
/// handlers (#425).
pub(super) fn findings_from<T>(
    items: &[T],
    book: Option<usize>,
    action: AiAction,
    describe: impl Fn(&T) -> (String, Vec<String>),
) -> Vec<Finding> {
    items
        .iter()
        .map(|it| {
            let (summary, chapters) = describe(it);
            Finding {
                summary,
                kind: action.finding_kind(),
                chapters,
                book,
                action: Some(action),
                corroborators: Vec::new(),
            }
        })
        .collect()
}

// --- Per-pass finding descriptions (#425/#608) ------------------------------
//
// The one-line summary + chapters for one typed finding of each pass. Shared by
// the apply handlers (which surface a fresh pass's findings) and the single-
// finding re-check (#608, which re-parses a scoped re-run into the same findings),
// so both produce byte-identical summaries — the finding's content key, which the
// re-check depends on to recognize the finding it just re-ran.

/// The quoted-locator suffix a finding appends so the author can land on the exact
/// span (#973/#1047/#1049/#1050/#293): ` — “{quote}”` for a non-blank, trimmed
/// quote, or `""` when there's nothing to quote. The single place this idiom lives —
/// it was copy-pasted across five `describe_*` handlers.
fn quoted_locator(quote: &str) -> String {
    let quote = quote.trim();
    if quote.is_empty() {
        String::new()
    } else {
        format!(" \u{2014} \u{201c}{quote}\u{201d}")
    }
}

pub(super) fn describe_continuity(f: &continuity::Finding) -> (String, Vec<String>) {
    (format!("{}: {}", f.kind, f.detail), f.chapters.clone())
}

pub(super) fn describe_setting(f: &setting::RuleFinding) -> (String, Vec<String>) {
    (format!("{}: {}", f.rule, f.violation), f.chapters.clone())
}

pub(super) fn describe_plot(f: &plot::ThreadFinding) -> (String, Vec<String>) {
    (format!("{}: {}", f.thread, f.issue), f.chapters.clone())
}

pub(super) fn describe_theme(f: &ai_theme::MotifFinding) -> (String, Vec<String>) {
    (format!("{}: {}", f.motif, f.issue), f.chapters.clone())
}

pub(super) fn describe_scaffold(g: &scaffold::Gap) -> (String, Vec<String>) {
    (format!("gap: {}", g.detail), vec![g.chapter.clone()])
}

pub(super) fn describe_naming(f: &naming::NameFinding) -> (String, Vec<String>) {
    let summary = if f.variants.is_empty() {
        f.name.clone()
    } else {
        format!("{} — also: {}", f.name, f.variants.join(", "))
    };
    (summary, f.chapters.clone())
}

pub(super) fn describe_style(f: &style::StyleFinding) -> (String, Vec<String>) {
    (format!("{}: {}", f.kind, f.detail), f.chapters.clone())
}

pub(super) fn describe_editorial(n: &ai::editorial::Note) -> (String, Vec<String>) {
    let at = quoted_locator(n.location.as_deref().unwrap_or_default());
    (
        format!("{}: {}{at}", n.focus, n.concern),
        n.chapters.clone(),
    )
}

pub(super) fn describe_accuracy(f: &ai::accuracy::AccuracyFinding) -> (String, Vec<String>) {
    // The flagged passage (when quoted) then why it may be off for the named concern
    // — an observation the author acts on, never a fix.
    (
        format!("{}{}", f.detail, quoted_locator(&f.passage)),
        f.chapters.clone(),
    )
}

pub(super) fn describe_relationship(
    f: &ai::relationship::TrajectoryFinding,
) -> (String, Vec<String>) {
    // The inconsistency (kind + note), naming what drifts in the pair's arc — an
    // observation the author acts on, never a rewrite of the relationship.
    (f.summarize(), f.chapters.clone())
}

pub(super) fn describe_foreshadow(f: &ai::foreshadow::UnpaidSetup) -> (String, Vec<String>) {
    // The what-was-planted-and-why-it's-unpaid note, then the setup passage (when
    // quoted) — an observation the author acts on, never a drafted payoff.
    (
        format!("{}{}", f.detail, quoted_locator(&f.setup)),
        f.chapters.clone(),
    )
}

pub(super) fn describe_factcheck(f: &ai::factcheck::FactCheckFinding) -> (String, Vec<String>) {
    // Cite the contradicted research note, the contradiction, then the flagged
    // passage (when quoted) — an observation the author acts on, never a fix.
    let at = quoted_locator(&f.passage);
    let note = f.note.trim();
    let summary = if note.is_empty() {
        format!("{}{at}", f.detail)
    } else {
        // Reuse the notebook's own citation phrasing so a finding reads the same as
        // a bible entry that grounds against the note (#932).
        format!("{}: {}{at}", ai::bible::cite_research(note), f.detail)
    };
    (summary, f.chapters.clone())
}

pub(super) fn describe_pacing(o: &ai::pacing::PacingObservation) -> (String, Vec<String>) {
    // The plain-language rhythm reading, located to the chapters it spans — an
    // observation the author acts on, never a suggested cut / addition / reorder / reword.
    (o.observation.clone(), o.chapters.clone())
}

pub(super) fn describe_voice(f: &voice::VoiceFinding) -> (String, Vec<String>) {
    // Attribute to the speaker (kind + character), then quote the specific spoken
    // line for a line-level dialogue finding (#1050) so the author can land on the
    // exact line — the line is quoted for locating, never a rewrite.
    let head = if f.character.is_empty() {
        format!("{}: {}", f.kind, f.detail)
    } else {
        format!("{} ({}): {}", f.kind, f.character, f.detail)
    };
    (
        format!("{head}{}", quoted_locator(&f.line)),
        f.chapters.clone(),
    )
}

/// The single fn signature that turns a pass's raw JSON response into the UI
/// findings it produces, attributed to an `action` + `book`. The one place a
/// response becomes findings for the analysis-only passes: the interactive worker
/// (#563), the single-finding re-check (#608), the re-check-all batch (#830), and
/// the on-demand scoped pass (#1054) all run the *same* fn, so a re-run's findings
/// share the original's content key by construction.
pub(super) type FindingsParser = fn(&str, AiAction, Option<usize>) -> Vec<Finding>;

/// Bundle a built re-check request with the parser that turns its response into
/// the UI findings the pass now produces (#608), so both move into the worker
/// closure together.
pub(super) struct RecheckRequest {
    pub(super) request: ai::Request,
    pub(super) to_findings: FindingsParser,
}

/// The parser for `action`'s findings pass (#563): parse the response with the
/// pass's own parser and map it to UI findings with the same `describe_*` the apply
/// handler uses, so every path that surfaces this pass's findings — a full pass, a
/// re-check (#608), a scoped run (#1054) — produces byte-identical summaries (the
/// finding's content key the re-check depends on). A parse error yields no findings;
/// for a re-check that reads as "the finding is gone" (a later full pass re-surfaces
/// it if it truly persists). `Some` for every action that surfaces re-runnable
/// findings — including the arg-taking passes (accuracy #973, relationship #1046),
/// which have their own parser arms here. `None` only for an action that surfaces no
/// re-runnable findings at all: the local views, the generative helpers, and the
/// off-manuscript copy generators.
///
/// Having a parser here does NOT mean a pass can be *scoped* and re-run: whether a
/// scoped re-check exists is gated separately in [`recheck_request`], whose `match`
/// returns `None` for the arg-taking passes (their author-chosen argument — the
/// accuracy concern, the relationship pair — isn't stored per finding, so a scoped
/// re-run can't reconstruct it). `findings_parser` still parses their responses on
/// the full-pass path; it's `recheck_request` that withholds the scoped re-run.
///
/// [`recheck_request`]: App::recheck_request
///
/// Kept a `match` (not a descriptor fn-pointer field) because each arm's parse +
/// `describe_*` are the per-pass content, and threading those fn pointers up into
/// the `crate::mode` descriptor would invert the crate's `mode → app` dependency.
pub(super) fn findings_parser(action: AiAction) -> Option<FindingsParser> {
    Some(match action {
        AiAction::Continuity => |text, a, book| {
            continuity::parse(text)
                .map(|c| findings_from(&c.findings, book, a, describe_continuity))
                .unwrap_or_default()
        },
        AiAction::Setting => |text, a, book| {
            setting::parse(text)
                .map(|s| findings_from(&s.findings, book, a, describe_setting))
                .unwrap_or_default()
        },
        AiAction::Characters => |text, a, book| {
            // A character re-check surfaces no findings of its own (the pass writes
            // profiles, not flags), so a clean re-parse clears the finding.
            let _ = (text, a, book);
            Vec::new()
        },
        AiAction::Scaffold => |text, a, book| {
            scaffold::parse(text)
                .map(|s| findings_from(&s.gaps, book, a, describe_scaffold))
                .unwrap_or_default()
        },
        AiAction::Naming => |text, a, book| {
            naming::parse(text)
                .map(|n| findings_from(&n.findings, book, a, describe_naming))
                .unwrap_or_default()
        },
        AiAction::Style => |text, a, book| {
            style::parse(text)
                .map(|s| findings_from(&s.findings, book, a, describe_style))
                .unwrap_or_default()
        },
        AiAction::Editorial => |text, a, book| {
            ai::editorial::parse(text)
                .map(|notes| findings_from(&notes, book, a, describe_editorial))
                .unwrap_or_default()
        },
        AiAction::Voice => |text, a, book| {
            voice::parse(text)
                .map(|v| findings_from(&v.findings, book, a, describe_voice))
                .unwrap_or_default()
        },
        AiAction::Foreshadow => |text, a, book| {
            ai::foreshadow::parse(text)
                .map(|f| findings_from(&f.findings, book, a, describe_foreshadow))
                .unwrap_or_default()
        },
        AiAction::FactCheck => |text, a, book| {
            ai::factcheck::parse(text)
                .map(|f| findings_from(&f.findings, book, a, describe_factcheck))
                .unwrap_or_default()
        },
        AiAction::Pacing => |text, a, book| {
            ai::pacing::parse(text)
                .map(|p| findings_from(&p.observations, book, a, describe_pacing))
                .unwrap_or_default()
        },
        AiAction::Accuracy => |text, a, book| {
            ai::accuracy::parse(text)
                .map(|acc| findings_from(&acc.findings, book, a, describe_accuracy))
                .unwrap_or_default()
        },
        AiAction::Relationship => |text, a, book| {
            ai::relationship::parse(text)
                .map(|t| findings_from(&t.findings, book, a, describe_relationship))
                .unwrap_or_default()
        },
        AiAction::Plot => |text, a, book| {
            plot::parse(text)
                .map(|p| findings_from(&p.findings, book, a, describe_plot))
                .unwrap_or_default()
        },
        AiAction::Theme => |text, a, book| {
            ai_theme::parse(text)
                .map(|t| findings_from(&t.findings, book, a, describe_theme))
                .unwrap_or_default()
        },
        // No re-runnable findings: the local views, generative helpers, image/thesaurus
        // passes, and off-manuscript copy generators.
        _ => return None,
    })
}

impl App {
    /// Spawn an analysis-only findings pass (#563): `build` makes the request from
    /// the manuscript + model (capturing any author-supplied argument — the accuracy
    /// concern, the relationship pair, the fact-check notes, the voice profiles — so
    /// the per-pass content stays in the closure), the response is mapped to UI
    /// findings by the pass's shared [`findings_parser`], and the result is wrapped in
    /// the one generic [`Outcome::Findings`] tagged with `action`. The single spawn
    /// path every findings pass (scaffold, naming, style, editorial, voice, accuracy,
    /// relationship, foreshadow, factcheck, pacing) shares — [`apply_findings`] then
    /// surfaces them identically. A no-op with a status when AI isn't ready or a
    /// request is in flight (like every spawn).
    ///
    /// [`apply_findings`]: App::apply_findings
    pub(super) fn spawn_findings_pass(
        &mut self,
        action: AiAction,
        message: &str,
        build: impl FnOnce(String, &str) -> ai::Request + Send + 'static,
    ) {
        // The active-book analysis passes attribute to the active book (`None`), like
        // every findings pass before this; only the series-bible refresh carries a book.
        let to_findings = findings_parser(action);
        self.spawn_ai(message, move |backend, manuscript, model| {
            let findings = backend
                .complete(&build(manuscript, &model))
                .map(|r| to_findings.map_or_else(Vec::new, |parse| parse(&r.text, action, None)));
            Outcome::Findings(findings)
        });
    }

    /// Apply a completed analysis-only findings pass (#563): surface the findings the
    /// worker already mapped from the pass's typed result. The one generic apply the
    /// [`Outcome::Findings`] dispatch arm calls for every findings pass — the bespoke
    /// per-pass `apply_*` handlers it replaced only differed in the typed result they
    /// unwrapped (which the worker now does) and the `describe_*` they mapped through
    /// (which [`findings_parser`] now holds, attributing each finding to its action).
    pub(super) fn apply_findings(&mut self, result: Result<Vec<Finding>, ai::AiError>) {
        let Some(findings) = self.ok_or_report(result) else {
            return;
        };
        self.show_findings(findings);
    }

    /// Run one persistent-bible pass (#339) for `action` scoped to `book` — the
    /// active-book passes call it with the active book; the series-bible refresh
    /// (#340) drives it per book. The tracker carries its own request builders and
    /// parse, so the single `bible_tracker` lookup is both the gate and the
    /// dispatch (#510).
    pub(crate) fn run_book_bible_pass(&mut self, action: AiAction, book: usize, force: bool) {
        if let Some(t) = bible_tracker(action) {
            self.run_bible_tracker(t, book, force);
        }
    }

    /// Show an AI result — a Q&A answer or a pass summary — as the full-screen
    /// overlay, scrolled to the top. The single place an AI result becomes an
    /// overlay.
    pub(super) fn show_ai(&mut self, lines: Vec<Line<'static>>) {
        self.overlay = Some(Overlay::ai(lines));
    }

    /// Unwrap an AI result, or set the standard `AI: <error>` status and return
    /// `None` — the shared error arm of the `apply_*`/`analyze_*` handlers (#315).
    pub(super) fn ok_or_report<T>(&mut self, result: Result<T, ai::AiError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                None
            }
        }
    }

    /// Whether an AI request is currently in flight (for the status indicator).
    pub const fn ai_busy(&self) -> bool {
        self.ai_task.is_busy()
    }
}

/// The new selection for a navigable list given a key, or `None` if the key
/// isn't a movement key. Shared by the palette, findings, and reference-file
/// list handlers.
pub(super) fn list_move(selected: usize, len: usize, code: KeyCode) -> Option<usize> {
    match code {
        KeyCode::Up | KeyCode::Char('k') => Some(selected.saturating_sub(1)),
        KeyCode::Down | KeyCode::Char('j') => Some((selected + 1).min(len.saturating_sub(1))),
        _ => None,
    }
}
