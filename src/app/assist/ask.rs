//! "Ask the manuscript" (#609): a grounded Q&A over the active book's prose,
//! surfaced as jumpable chapter citations, logged per project (#654) so a past
//! lookup and its citations survive a restart and can be re-browsed.

use super::super::*;

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
        })
        .collect();
    if !rows.is_empty() {
        rows.push(Finding {
            summary: String::new(),
            kind: FindingKind::Other,
            chapters: Vec::new(),
            book: None,
            action: None,
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
                });
            }
        }
    }
    out
}

impl App {
    /// Start an "Ask the manuscript" request on a worker thread. The manuscript
    /// is assembled here (on the main thread) and moved into the thread with the
    /// question and a backend; the result returns via `ai_rx`, polled each tick.
    /// A no-op with a status message when AI isn't ready or a request is already
    /// in flight.
    pub(crate) fn ask_ai(&mut self, question: &str) {
        let question = question.trim().to_string();
        if question.is_empty() {
            return;
        }
        // Capture the question so the answer can be filed under it in the session
        // Q&A log when it returns (#609). Set before the spawn so it's recorded
        // even if the request is a no-op (the spawn clears it on a not-ready gate).
        self.pending_qa_question = Some(question.clone());
        self.spawn_ai(
            "⟳ Sending to Claude…",
            move |backend, manuscript, model| {
                Outcome::Answer(
                    backend.complete(&ai::build_qa_request(manuscript, &question, &model)),
                )
            },
        );
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
