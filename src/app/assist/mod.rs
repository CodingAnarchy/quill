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
mod trackers;
mod triage;
mod workflow;
mod writers;

pub use ask::QaEntry;

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
                chapters,
                book,
                action: Some(action),
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
    let at = n
        .location
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|q| format!(" \u{2014} \u{201c}{q}\u{201d}"))
        .unwrap_or_default();
    (
        format!("{}: {}{at}", n.focus, n.concern),
        n.chapters.clone(),
    )
}

pub(super) fn describe_voice(f: &voice::VoiceFinding) -> (String, Vec<String>) {
    let summary = if f.character.is_empty() {
        format!("{}: {}", f.kind, f.detail)
    } else {
        format!("{} ({}): {}", f.kind, f.character, f.detail)
    };
    (summary, f.chapters.clone())
}

/// Bundle a built re-check request with the parser that turns its response into
/// the UI findings the pass now produces (#608), so both move into the worker
/// closure together.
pub(super) struct RecheckRequest {
    pub(super) request: ai::Request,
    pub(super) to_findings: fn(&str, AiAction, Option<usize>) -> Vec<Finding>,
}

/// The parser for a re-check of `action` (#608): parse the response with the
/// pass's own parser and map it to UI findings with the same `describe_*` the
/// apply handler uses, so a re-run's findings share the original's content key. A
/// parse error yields no findings — the re-check then treats the flagged finding
/// as gone; that's acceptable (a later full pass re-surfaces it if it truly
/// persists). Attributes each to `action` and `book`.
pub(super) fn recheck_parser(
    action: AiAction,
) -> fn(&str, AiAction, Option<usize>) -> Vec<Finding> {
    match action {
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
        // Non-finding actions never reach here (`recheck_request` returns `None`).
        _ => |_, _, _| Vec::new(),
    }
}

impl App {
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
