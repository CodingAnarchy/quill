//! Verify open findings on idle (#1053): an opt-in that re-checks only the
//! findings whose chapters changed since they were generated, clearing the ones an
//! edit has resolved — so during a revision pass triage stays honest without the
//! author remembering to re-run.
//!
//! It extends the existing re-check paths (single #608, all #830): the same scoped
//! per-pass re-run over a finding's chapters, the same reconcile (a finding the
//! fresh run no longer produces is cleared, one still flagged is kept), the same
//! `Outcome::RecheckAll` apply. The only thing new here is *what triggers it* and
//! *what it's scoped to*: it runs on the idle tick (never the keystroke path) and
//! is bounded to the open findings whose chapters' content digest changed since the
//! finding was last generated or verified (content-addressed, exactly like the
//! incremental bible cache #90) — no whole-manuscript re-analysis.
//!
//! It stays within the AI boundary: it re-runs analysis on subsets and updates
//! flags; it writes no prose and no bible file. And it never clobbers a finding the
//! author is still working — a finding whose chapter is the one being actively
//! edited is left alone until the edit settles (reconcile, don't overwrite).

use std::fs;
use std::time::{Duration, SystemTime};

use super::super::*;
use super::RecheckRequest;
use crate::mode::AiAction;

/// How recently a file may have been modified and still have its digest memoized.
/// A filesystem's mtime can be coarse (a few ms on Linux, seconds on some), so a
/// second same-size write landing in the same tick as the one we stamped would
/// look unchanged. Refusing to trust a stamp this fresh (the "racy git" rule) means
/// any write after the stamp necessarily moves the mtime.
const RACY_WINDOW: Duration = Duration::from_secs(2);

/// Where a chapter's text currently comes from, as a cheap change stamp: the live
/// buffer's content revision, or the chapter file's size and mtime.
#[derive(PartialEq, Eq)]
enum TextStamp {
    Buffer(u64),
    File(u64, SystemTime),
}

/// A memoized chapter digest for "verify on idle" (#1053): valid while the
/// chapter's [`TextStamp`] is unchanged, so an idle pass re-reads and re-digests
/// only the chapters that actually moved.
pub(in crate::app) struct DigestMemo {
    stamp: TextStamp,
    digest: String,
}

/// One scoped re-check the idle plan will run: the built request plus the pass and
/// book its response is parsed under, so the worker maps the fresh findings back to
/// the verifying set. Mirrors `recheck`'s own grouped-recheck tuple (#830).
type GroupedRecheck = (RecheckRequest, AiAction, Option<usize>);

impl App {
    /// Record the content digest of each finding's chapters as the baseline "verify
    /// on idle" compares against later (#1053). Called when a pass accumulates
    /// findings, so a chapter is baselined at the digest the finding was generated
    /// from; a later idle tick that finds a different digest knows an edit has
    /// touched it. Bounded to just these findings' chapters — never the whole book.
    ///
    /// A no-op unless the opt-in is on, so an author who never enables it pays
    /// nothing (and the baselines map stays empty).
    pub(in crate::app) fn record_verify_baselines(&mut self, findings: &[Finding]) {
        if !self.ai.verify_on_idle {
            return;
        }
        for f in findings {
            self.baseline_finding_chapters(f);
        }
    }

    /// Digest each of a finding's chapters at its current text and store it under
    /// `(book, chapter file)`, so a subsequent edit is detectable as a digest
    /// change. The book is part of the key so a series' same-named chapter files
    /// (root-level, no subdir) don't clobber one another's baselines (#1053).
    fn baseline_finding_chapters(&mut self, finding: &Finding) {
        let book = self.finding_book(finding);
        for file in self.finding_chapter_files(finding) {
            let digest = self.chapter_digest(book, &file);
            self.verify_baselines.insert((book, file), digest);
        }
    }

    /// The idle hook (#1053): re-check the open findings whose chapters changed
    /// since they were baselined, scoped to only those chapters. A no-op — and
    /// cheap — unless the opt-in is on, an AI request isn't already in flight, and
    /// there are open findings sitting in triage.
    ///
    /// Runs on the idle tick, never the keystroke path, so it can't make typing
    /// depend on document size. The work it does is bounded to the open findings'
    /// changed chapters: it digests only those chapters (never the whole
    /// manuscript) and re-runs a pass only over the ones that actually changed.
    pub(in crate::app) fn verify_findings_on_idle(&mut self) {
        if !self.ai.verify_on_idle || self.ai_busy() || self.triage.is_empty() {
            return;
        }
        let (requests, verifying) = self.verify_on_idle_plan();
        if requests.is_empty() || verifying.is_empty() {
            return;
        }
        self.spawn_outcome("⟳ Verifying changed findings…", move |backend| {
            let mut fresh = Vec::new();
            for (req, action, book) in requests {
                match backend.complete(&req.request) {
                    Ok(r) => fresh.extend((req.to_findings)(&r.text, action, book)),
                    Err(e) => return Outcome::RecheckAll(Err(e)),
                }
            }
            Outcome::RecheckAll(Ok(fresh))
        });
        // Everything below runs only if the spawn actually took — `spawn_outcome`
        // returns early *without* spawning when a request is already in flight or the
        // backend can't be built (unconfigured, offline pin with no local model, a
        // transient error), in which case `ai_busy()` is still false. Gating on it is
        // the source of truth for "the pass will run":
        //
        //  - Re-baseline the verifying chapters at their current digest ONLY on a
        //    successful spawn. Doing it unconditionally (before the spawn, as it
        //    used to) silently dropped the re-check when the backend gated out: the
        //    baseline advanced to the current digest, so the next idle tick saw no
        //    change and never re-verified the stale finding until the author edited
        //    that chapter *again* — an edit that resolved a finding never cleared it.
        //    Re-baselining here means a gated-out pass leaves the baseline untouched,
        //    so the finding is re-checked once the backend is ready. On success it
        //    re-baselines exactly once, so a second idle tick before the result lands
        //    doesn't queue the same re-check (the reconcile that lands later is the
        //    source of truth for whether the finding survives).
        //  - Reconcile exactly these on completion — the same apply path a manual
        //    re-check-all uses (#830) — leaving no stale pending re-check on a gate.
        if self.ai_busy() {
            for finding in &verifying {
                self.baseline_finding_chapters(finding);
            }
            self.pending_recheck_all = Some(verifying);
        }
    }

    /// Plan the idle verification (#1053): the scoped requests to run and the
    /// findings they verify — but only over the findings whose chapters *changed*
    /// since they were baselined, and never a finding on the chapter the author is
    /// actively editing. Groups by (pass, book, chapters) so several stale findings
    /// sharing a pass + chapters cost one re-run, mirroring [`recheck_all_plan`].
    ///
    /// A finding with no re-runnable pass (`action: None` — a Q&A citation or one
    /// restored from the durable store) or no locatable chapter is skipped: it stays
    /// in triage untouched, since nothing scoped can be re-run for it.
    fn verify_on_idle_plan(&self) -> (Vec<GroupedRecheck>, Vec<Finding>) {
        let active = self.project.active_file().to_string();
        let editing_active = self.edit_dirty();
        let mut requests = Vec::new();
        let mut seen: Vec<(AiAction, Option<usize>, Vec<String>)> = Vec::new();
        let mut verifying = Vec::new();
        for finding in &self.triage {
            let Some(action) = finding.action else {
                continue; // restored / non-pass finding: nothing scoped to re-run
            };
            let files = self.finding_chapter_files(finding);
            if files.is_empty() {
                continue; // no locatable chapter to scope to
            }
            // Reconcile, don't overwrite: never re-check a finding whose chapter is
            // the one the author is mid-edit on (unsaved changes). Its digest is in
            // flux; verifying now could clobber a finding they're still working.
            if editing_active && files.iter().any(|f| f == &active) {
                continue;
            }
            if !self.finding_chapters_changed(&files, self.finding_book(finding)) {
                continue; // nothing this finding speaks for changed → leave it be
            }
            verifying.push(finding.clone());
            let group = (action, finding.book, files.clone());
            if seen.contains(&group) {
                continue; // this pass + chapters is already queued
            }
            if let Some(req) = self.recheck_request(action, finding.book, &files) {
                requests.push((req, action, finding.book));
                seen.push(group);
            }
        }
        (requests, verifying)
    }

    /// Whether any of `files` (a finding's chapters, in book `book`) has a current
    /// content digest different from its recorded baseline (#1053) — i.e. an edit
    /// touched it since the finding was generated. A chapter with no baseline (a
    /// finding restored before the opt-in was on, say) counts as unchanged, so a
    /// verification never fires on a chapter we can't prove moved.
    fn finding_chapters_changed(&self, files: &[String], book: usize) -> bool {
        files.iter().any(|file| {
            // Look up the baseline under the *book-scoped* key, so a series' same-
            // named files can't cross-contaminate (#1053).
            let Some(baseline) = self.verify_baselines.get(&(book, file.clone())) else {
                return false;
            };
            &self.chapter_digest(book, file) != baseline
        })
    }

    /// The content digest of chapter `file` in book `book`, as
    /// [`book_chapter_text`](Self::book_chapter_text) reads it — memoized against
    /// the text's [`TextStamp`], so an idle pass where nothing changed costs a
    /// `stat` per chapter rather than a read and a hash.
    fn chapter_digest(&self, book: usize, file: &str) -> String {
        let stamp = self.chapter_text_stamp(book, file);
        let key = (book, file.to_string());
        if let Some(stamp) = &stamp {
            if let Some(memo) = self.verify_digests.borrow().get(&key) {
                if &memo.stamp == stamp {
                    return memo.digest.clone();
                }
            }
        }
        let digest = ai::incremental::digest(&self.book_chapter_text(book, file));
        if let Some(stamp) = stamp {
            let memo = DigestMemo {
                stamp,
                digest: digest.clone(),
            };
            self.verify_digests.borrow_mut().insert(key, memo);
        }
        digest
    }

    /// The change stamp for the text [`book_chapter_text`](Self::book_chapter_text)
    /// would return: the live buffer's revision when that's where the text comes
    /// from, otherwise the file's size and mtime. `None` when it can't be stamped
    /// reliably — unreadable metadata, or an mtime inside [`RACY_WINDOW`] — so the
    /// caller digests afresh rather than trust it.
    fn chapter_text_stamp(&self, book: usize, file: &str) -> Option<TextStamp> {
        if book == self.project.active_book() {
            let index = self.project.index_of_file(file)?;
            if index == self.project.active_index() && self.focus_shows_active_chapter() {
                return Some(TextStamp::Buffer(self.buffer.revision()));
            }
        }
        let meta = fs::metadata(self.project.root.join(file)).ok()?;
        let modified = meta.modified().ok()?;
        let age = SystemTime::now().duration_since(modified).ok()?;
        (age >= RACY_WINDOW).then_some(TextStamp::File(meta.len(), modified))
    }

    /// The idle-verification plan's scoped requests (#1053), for tests to assert the
    /// changed-chapter scoping and grouping without spawning a worker or a live
    /// backend. Mirrors [`recheck_all_requests`](Self::recheck_all_requests).
    #[cfg(test)]
    pub(crate) fn verify_on_idle_requests(&self) -> Vec<ai::Request> {
        self.verify_on_idle_plan()
            .0
            .into_iter()
            .map(|(r, _, _)| r.request)
            .collect()
    }

    /// The findings an idle verification would re-check right now (#1053) — the
    /// stale, re-runnable, not-actively-edited ones — for tests to assert exactly
    /// which findings a change marks for re-verify, without a worker or a backend.
    #[cfg(test)]
    pub(crate) fn verify_on_idle_targets(&self) -> Vec<Finding> {
        self.verify_on_idle_plan().1
    }

    /// Re-baseline a finding's chapters at their current digest (#1053), exposing
    /// exactly the step [`verify_findings_on_idle`](Self::verify_findings_on_idle)
    /// performs on a *successful* spawn — for tests to assert the success-path
    /// re-baseline contract without spawning a live worker (#531).
    #[cfg(test)]
    pub(crate) fn baseline_finding_chapters_for_test(&mut self, finding: &Finding) {
        self.baseline_finding_chapters(finding);
    }
}
