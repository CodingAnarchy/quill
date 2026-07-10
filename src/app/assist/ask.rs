//! "Ask the manuscript" (#609): a grounded Q&A over the active book's prose,
//! surfaced as jumpable chapter citations, logged per project (#654) so a past
//! lookup and its citations survive a restart and can be re-browsed.

use super::super::*;

use crate::ai::embed::Breadth;
use crate::ai::model_select::local_window;
use crate::ai::retrieval::{self, BibleSeed, LexicalSearch};
use crate::ai::BackendKind;

/// The bible sections the offline retrieval loop seeds with (#856), each `(name,
/// bible-file)` under `bible/`. The compact global index — characters, setting,
/// timeline, plot, theme, outline — that carries the diffuse, holistic structure
/// targeted chapter reads miss. Read on the UI thread and moved into the worker.
const BIBLE_SECTIONS: &[(&str, &str)] = &[
    ("characters", "characters"), // a directory of per-character files
    ("setting", "setting.md"),
    ("timeline", "timeline.md"),
    ("plot", "plot.md"),
    ("theme", "theme.md"),
    ("outline", "outline.md"),
];

/// Whether an offline Q&A pass is too big for the on-device window and so must take
/// the retrieval read-loop (#856) rather than a whole-book completion. `manuscript_len`
/// is the whole active book's character length; `window_tokens` the model's context
/// window. Uses the same `chars / 4 ≈ tokens` upper bound the cost estimate uses, so
/// the decision can't drift from the pre-flight guard that would otherwise refuse
/// (#849). Pure — the seam the app test drives without a backend.
fn retrieval_oversized(manuscript_len: usize, window_tokens: u32) -> bool {
    (manuscript_len / 4) as u64 > u64::from(window_tokens)
}

/// The #849 refusal, kept as the correct fallback when the offline retrieval loop
/// genuinely can't proceed — the resolved local backend exposes no tool-use transport
/// (`Converse`), so no read-loop can run. Actionable, like the router's own refusal.
fn retrieval_unavailable(chapters: usize) -> String {
    format!(
        "This manuscript ({chapters} chapters) is too large for the on-device model's \
         window, and offline reading isn't available on this backend. Turn off `[ai] \
         offline` to use Claude, or narrow the scope (fewer chapters, a single book)."
    )
}

/// Parse an optional retrieval-breadth marker off the front of an Ask question
/// (#931), returning the requested [`Breadth`] and the question with the marker
/// stripped. A question may lead with `!broad ` / `!fine ` (or the aliases
/// `!wide` / `!narrow`) to ground the answer in fewer-larger vs more-smaller
/// passages; anything else — the overwhelming default — is [`Breadth::Normal`] and
/// the question is returned untouched, so default Ask behavior is unchanged.
///
/// The marker is a plain in-question toggle so it needs no new prompt state and
/// stays hermetically testable: the parse is pure. Matching is case-insensitive and
/// only fires on the leading token, so a question that merely *mentions* "broad"
/// isn't affected.
fn parse_breadth(question: &str) -> (Breadth, String) {
    let q = question.trim_start();
    let (marker, rest) = q.split_once(char::is_whitespace).unwrap_or((q, ""));
    let breadth = match marker.to_ascii_lowercase().as_str() {
        "!broad" | "!wide" => Breadth::Broad,
        "!fine" | "!narrow" => Breadth::Fine,
        // No recognized marker → default breadth, question untouched.
        _ => return (Breadth::Normal, question.to_string()),
    };
    (breadth, rest.trim_start().to_string())
}

/// One entry in the "Ask the manuscript" history (#609): the question and its
/// grounded answer (prose + cited chapter titles). Persisted per project (#654)
/// via [`qa_store`](super::super::qa_store) so a past lookup and its citations
/// reload on open — analysis recall within the AI boundary, never manuscript
/// prose.
#[derive(Clone)]
pub struct QaEntry {
    /// The question as asked. Empty when the answer arrived without a recorded
    /// question (e.g. a test that drives `apply_qa_result` directly).
    pub(crate) question: String,
    /// The grounded answer: the prose plus the chapters it cited.
    pub(crate) answer: ai::Answer,
}

/// Turn a Q&A answer's cited chapter titles into jumpable [`Finding`]s (#609) —
/// one per citation, each targeting that chapter so Enter jumps there via the
/// shared findings handler. De-duplicated (a model may cite a chapter twice),
/// order preserved. Empty titles are dropped. `book: None` — Q&A runs over the
/// active book's manuscript.
fn qa_citation_findings(answer: &ai::Answer) -> Vec<Finding> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for title in &answer.citations {
        let title = title.trim();
        if title.is_empty() || seen.contains(&title) {
            continue;
        }
        seen.push(title);
        out.push(Finding {
            summary: format!("cited: {title}"),
            kind: FindingKind::Other, // a Q&A citation isn't an analysis-pass finding (#827)
            chapters: vec![title.to_string()],
            book: None,
            action: None, // a Q&A citation isn't a re-runnable analysis finding (#608)
            corroborators: Vec::new(),
        });
    }
    out
}

/// The answer prose as leading, non-jumpable rows for the citations list (#609):
/// each non-empty line of the answer becomes a `Finding` with no chapters (so
/// Enter on it is a no-op), followed by a blank spacer row. Empty answer → no rows.
fn answer_context_rows(answer: &ai::Answer) -> Vec<Finding> {
    let mut rows: Vec<Finding> = answer
        .answer
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| Finding {
            summary: l.to_string(),
            kind: FindingKind::Other,
            chapters: Vec::new(),
            book: None,
            action: None,
            corroborators: Vec::new(),
        })
        .collect();
    if !rows.is_empty() {
        rows.push(Finding {
            summary: String::new(),
            kind: FindingKind::Other,
            chapters: Vec::new(),
            book: None,
            action: None,
            corroborators: Vec::new(),
        });
    }
    rows
}

/// Build the jumpable Q&A-history findings (#609): every logged ask, newest
/// first, one row per cited chapter (`Q: … → <chapter>`, jumpable), plus a single
/// non-jumpable row for an ask that cited nothing. Lets the whole session's
/// lookups be browsed and re-jumped from one list.
fn qa_history_findings(log: &[QaEntry]) -> Vec<Finding> {
    let mut out = Vec::new();
    for entry in log.iter().rev() {
        let q = entry.question.trim();
        let label = if q.is_empty() {
            "Q".to_string()
        } else {
            format!("Q: {q}")
        };
        let cited: Vec<&String> = entry
            .answer
            .citations
            .iter()
            .filter(|c| !c.trim().is_empty())
            .collect();
        if cited.is_empty() {
            out.push(Finding {
                summary: format!("{label} — (no chapter cited)"),
                kind: FindingKind::Other,
                chapters: Vec::new(),
                book: None,
                action: None,
                corroborators: Vec::new(),
            });
        } else {
            for title in cited {
                let title = title.trim();
                out.push(Finding {
                    summary: format!("{label} → {title}"),
                    kind: FindingKind::Other,
                    chapters: vec![title.to_string()],
                    book: None,
                    action: None,
                    corroborators: Vec::new(),
                });
            }
        }
    }
    out
}

impl App {
    /// Start an "Ask the manuscript" request on a worker thread (#609). The
    /// manuscript is assembled here (on the main thread) and moved into the thread
    /// with the question and a backend; the result returns via `ai_rx`, polled each
    /// tick. A no-op with a status message when AI isn't ready or a request is
    /// already in flight.
    ///
    /// When a local embedding model is available (#831), the worker first *retrieves*
    /// the passages most relevant to the question from the semantic index and sends
    /// only those (with their chapter citations) instead of the whole book — so Ask
    /// scales past one model's context window and cites tighter. If retrieval isn't
    /// available or turns up nothing, it falls back to the whole-book context
    /// unchanged. Either way it's the one `build_qa_request` path, so the answer's
    /// citations stay jumpable exactly as today.
    ///
    /// A question may lead with a breadth marker (`!broad` / `!fine`, #931) to ground
    /// the answer in fewer-larger vs more-smaller passages; the marker is parsed off
    /// here so it never reaches the model or the Q&A log (the logged question is the
    /// plain text), and it only affects the retrieval path — a whole-book fallback is
    /// unchanged, as is the default (no marker) ask.
    pub(crate) fn ask_ai(&mut self, question: &str) {
        // Split off an optional breadth marker (#931); the rest is the real question.
        let (breadth, question) = parse_breadth(question.trim());
        if question.is_empty() {
            return;
        }
        // Offline path only (#856): when the resolved backend is local and the whole
        // book won't fit its window, gather the answer by an agentic read-loop rather
        // than refusing (#849). The cloud path (Claude's 1M window + caching) is
        // unchanged below.
        if self.offline_qa_is_oversized() {
            self.ask_offline_retrieval(&question);
            return;
        }
        // Capture the question so the answer can be filed under it in the session
        // Q&A log when it returns (#609). Set before the spawn so it's recorded
        // even if the request is a no-op (the spawn clears it on a not-ready gate).
        self.pending_qa_question = Some(question.clone());
        // Retrieval inputs, assembled on the UI thread and moved into the worker: the
        // active book's chapters to (re-)index, the index dir, and the embedding model
        // — `None` when none is on disk, which keeps Ask on the whole-book path (#831).
        let embed_model = self.ask_embed_model();
        let chapters = embed_model.is_some().then(|| self.ask_index_chapters());
        let root = self.project.root.clone();
        self.spawn_ai(
            "⟳ Sending to Claude…",
            move |backend, manuscript, model| {
                // Try semantic retrieval first, at the requested breadth; fall back to
                // the whole book (#831, #931).
                let context = match (embed_model, chapters) {
                    (Some(m), Some(chapters)) => {
                        super::retrieve::retrieve_context(&root, &chapters, m, &question, breadth)
                            .unwrap_or(manuscript)
                    }
                    _ => manuscript,
                };
                Outcome::Answer(backend.complete(&ai::build_qa_request(context, &question, &model)))
            },
        );
    }

    /// Whether an "Ask the manuscript" request must take the offline retrieval loop
    /// (#856): the resolved backend is local (an `offline` pin or a `local` backend)
    /// AND the whole active book exceeds the on-device model's context window. When
    /// false, Ask uses its normal path (cloud whole-book, or a local book that fits).
    /// A pure size/backend check on data already on hand — no filesystem read, so it's
    /// off the keystroke path.
    fn offline_qa_is_oversized(&self) -> bool {
        self.ai.backend() == BackendKind::Local
            && retrieval_oversized(
                self.manuscript_text().len(),
                local_window(&self.ai.local_models),
            )
    }

    /// Run the offline bible-grounded retrieval loop for `question` (#856): assemble
    /// the corpus (the active book's chapters) and the compact bible seed on the UI
    /// thread, then drive the read-loop off the worker thread through the local
    /// backend's [`Converse`](crate::ai::tool_loop::Converse) transport. The result is
    /// wrapped as the same [`Outcome::Answer`] the whole-book path returns, so the Q&A
    /// log, citations, and jump handling are all unchanged (one Q&A path). If the
    /// backend has no tool-use transport, fall back to the #849 refusal — the correct
    /// fallback when the loop genuinely can't proceed.
    fn ask_offline_retrieval(&mut self, question: &str) {
        let question = question.to_string();
        self.pending_qa_question = Some(question.clone());
        let chapters = self.ask_index_chapters();
        let bible = self.bible_seed();
        let max_turns = self.ai.retrieval_max_turns;
        let window = local_window(&self.ai.local_models);
        self.spawn_outcome("⟳ Reading the manuscript on-device…", move |backend| {
            let Some(converse) = backend.as_converse() else {
                return Outcome::Answer(Err(ai::AiError::NotConfigured(retrieval_unavailable(
                    chapters.len(),
                ))));
            };
            let search = LexicalSearch::new(chapters);
            let result =
                retrieval::run_retrieval(converse, &search, &bible, &question, max_turns, window)
                    .map(|answer| retrieval::answer_response(&answer));
            Outcome::Answer(result)
        });
    }

    /// The compact story-bible seed for the retrieval loop (#856): the existing bible
    /// files under `bible/` (characters, setting, timeline, plot, theme, outline) as
    /// named sections — the global index the loop opens with and [`READ_BIBLE`] serves.
    /// Empty sections are dropped; an empty seed makes the loop search-only and its
    /// answer discloses the weaker coverage. Reads the bible files on the UI thread,
    /// then moves the seed into the worker.
    ///
    /// [`READ_BIBLE`]: crate::ai::retrieval::READ_BIBLE
    fn bible_seed(&self) -> BibleSeed {
        let root = &self.project.root;
        let sections = BIBLE_SECTIONS
            .iter()
            .map(|&(name, rel)| {
                let text = if rel.contains('.') {
                    bible::raw_file(root, rel)
                } else {
                    bible::raw_dir(root, rel)
                };
                (name.to_string(), text)
            })
            .collect();
        BibleSeed::new(sections)
    }

    /// Apply a completed Q&A result (#609): parse the grounded answer, record it in
    /// the session Q&A log (under the question it answered, for later recall), then
    /// surface it — as a jumpable citations list when it cites chapters (reusing the
    /// [`Finding`] jump), or as the plain answer overlay when it cites none. Errors
    /// set a status message. Shared by the async poll path and tests (which drive a
    /// mock backend synchronously).
    pub(crate) fn apply_qa_result(&mut self, result: Result<ai::Response, ai::AiError>) {
        let question = self.pending_qa_question.take().unwrap_or_default();
        let Some(resp) = self.ok_or_report(result) else {
            return;
        };
        let answer = ai::parse_answer(&resp.text);
        // Record the ask before rendering, so the log always reflects it even if the
        // author dismisses the overlay immediately (#609). Append to the durable
        // store too, so the lookup and its citations survive a restart (#654).
        let entry = QaEntry {
            question,
            answer: answer.clone(),
        };
        self.qa_store.append(&self.project.root, &entry);
        self.qa_log.push(entry);
        self.present_qa(&answer, &resp.usage);
    }

    /// Surface a grounded Q&A answer (#609). When it cites chapters, open a single
    /// jumpable [`Mode::Findings`] list — the answer prose as leading, non-jumpable
    /// context rows, then one jumpable entry per citation, so Enter jumps to the
    /// cited chapter exactly like a triage finding. When it cites none, fall back to
    /// the plain answer overlay. The citations never enter durable triage: they're
    /// retrieval for this ask, not standing revision findings.
    pub(super) fn present_qa(&mut self, answer: &ai::Answer, usage: &ai::Usage) {
        self.overlay = None;
        let citations = qa_citation_findings(answer);
        if citations.is_empty() {
            self.show_ai(ai::answer_to_lines(answer, usage));
            return;
        }
        // Lead with the answer prose as context rows (empty `chapters` → not a jump
        // target, a no-op on Enter), then the jumpable citations.
        let mut items: Vec<Finding> = answer_context_rows(answer);
        items.extend(citations);
        // Select the first jumpable citation so Enter jumps straight there.
        let selected = items
            .iter()
            .position(|f| !f.chapters.is_empty())
            .unwrap_or(0);
        self.mode = Mode::Findings(Box::new(Findings {
            items,
            selected,
            ..Default::default()
        }));
    }

    /// Browse the session's "Ask the manuscript" history (#609): every past ask's
    /// citations, newest first, as one jumpable [`Mode::Findings`] list — each row
    /// labels the question and the chapter it cited, so the author can re-jump any
    /// earlier lookup without re-asking (and without a fresh request). An ask that
    /// cited no chapter still shows, as a non-jumpable row. Empty log → a status.
    pub(crate) fn open_qa_history(&mut self) {
        self.overlay = None;
        if self.qa_log.is_empty() {
            self.message = Some("No questions asked yet — run \"Ask the manuscript\"".into());
            return;
        }
        let items = qa_history_findings(&self.qa_log);
        self.mode = Mode::Findings(Box::new(Findings {
            items,
            ..Default::default()
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_breadth, retrieval_oversized, Breadth};
    use crate::ai::model_select::STANDARD_CONTEXT_TOKENS;

    #[test]
    fn oversized_is_a_pure_window_check() {
        // A book that fits the window takes the normal path; one that exceeds it
        // (chars/4 tokens > window) takes the read-loop (#856), matching the #849
        // pre-flight guard's own chars/4 estimate so the decision can't drift.
        let window = STANDARD_CONTEXT_TOKENS; // 40960 tokens
        assert!(
            !retrieval_oversized(4 * window as usize, window),
            "a book that exactly fills the window is not oversized"
        );
        assert!(
            retrieval_oversized(4 * window as usize + 4, window),
            "a book past the window takes the retrieval loop"
        );
        assert!(!retrieval_oversized(0, window), "an empty book fits");
    }

    #[test]
    fn no_marker_is_normal_breadth_and_untouched_question() {
        // The default (no marker) ask is unchanged (#931): Normal breadth, verbatim.
        let (b, q) = parse_breadth("what is the arc of the locket?");
        assert_eq!(b, Breadth::Normal);
        assert_eq!(q, "what is the arc of the locket?");
    }

    #[test]
    fn broad_and_fine_markers_are_parsed_and_stripped() {
        let (b, q) = parse_breadth("!broad what is the arc of the locket?");
        assert_eq!(b, Breadth::Broad);
        assert_eq!(q, "what is the arc of the locket?", "marker stripped");

        let (b, q) = parse_breadth("!fine what exact word did she use?");
        assert_eq!(b, Breadth::Fine);
        assert_eq!(q, "what exact word did she use?");

        // Aliases and case-insensitivity.
        assert_eq!(parse_breadth("!WIDE q").0, Breadth::Broad);
        assert_eq!(parse_breadth("!Narrow q").0, Breadth::Fine);
    }

    #[test]
    fn a_mentioned_word_is_not_a_marker() {
        // Only a leading `!marker` toggles breadth; a question that merely mentions
        // "broad" is a plain Normal ask, untouched.
        let (b, q) = parse_breadth("is the broad daylight scene consistent?");
        assert_eq!(b, Breadth::Normal);
        assert_eq!(q, "is the broad daylight scene consistent?");
        // A bare word without the `!` prefix isn't a marker either.
        assert_eq!(parse_breadth("broad q").0, Breadth::Normal);
    }
}
