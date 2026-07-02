//! Off-manuscript writers — the only prose the AI produces, and never inside the
//! manuscript (#255/#236/#237/#251): the generative brainstorm and starter-draft
//! passes, the submission / marketing / social / ad-creative copy, and the shared
//! [`write_off_manuscript`](App::write_off_manuscript) writer they route through
//! (conflict-reviewed like any bible file, then summarized).

use super::super::bible_review::{Staged, StagedTally};
use super::super::*;

use crate::ai::ad_creative::{self, AdCreative};
use crate::ai::brainstorm::{self, Brainstorm};
use crate::ai::marketing::{self, Marketing};
use crate::ai::social::{self, Social};
use crate::ai::submission::{self, Submission};

impl App {
    /// Generate submission material (synopsis / blurb / query); written to
    /// `submission/` by [`App::poll_ai`].
    pub(crate) fn analyze_submission(&mut self) {
        self.spawn_manuscript_pass(
            "⟳ Drafting submission material…",
            submission::build_request,
            submission::parse,
            Outcome::Submission,
        );
    }

    /// Brainstorm story ideas from a premise (#255) — the first generative pass.
    /// Sends only the premise (no manuscript), so it works on an empty project;
    /// the ideas are written to `bible/brainstorm.md` by [`App::poll_ai`].
    pub(crate) fn brainstorm(&mut self, premise: &str) {
        self.spawn_generative(
            premise,
            "⟳ Brainstorming…",
            |backend, premise, model| {
                let req = brainstorm::build_request(premise, &model);
                Outcome::Brainstorm(
                    backend
                        .complete(&req)
                        .and_then(|r| brainstorm::parse(&r.text)),
                )
            },
        );
    }

    /// Draft a starter character cast from a concept (#256) — generative, no
    /// manuscript. Reuses the extractive pass's `Outcome::Characters` so the
    /// profiles are written to `bible/characters/` by [`apply_characters`].
    pub(crate) fn draft_characters(&mut self, concept: &str) {
        self.spawn_generative(
            concept,
            "⟳ Drafting characters…",
            |backend, concept, model| {
                let req = character::build_concept_request(concept, &model);
                Outcome::Characters(
                    backend
                        .complete(&req)
                        .and_then(|r| character::parse(&r.text)),
                )
            },
        );
    }

    /// Draft a starter setting bible from a concept (#257) — generative, no
    /// manuscript. Reuses the extractive pass's `Outcome::Setting` so the bible is
    /// written to `bible/setting.md` by [`apply_setting`].
    pub(crate) fn draft_setting(&mut self, concept: &str) {
        self.spawn_generative(
            concept,
            "⟳ Drafting setting…",
            |backend, concept, model| {
                let req = setting::build_concept_request(concept, &model);
                Outcome::Setting(backend.complete(&req).and_then(|r| setting::parse(&r.text)))
            },
        );
    }

    /// Apply a brainstorm pass (#255): write the idea notes to `bible/brainstorm.md`,
    /// conflict-reviewed and off-manuscript (never a chapter file, never prose).
    pub(crate) fn apply_brainstorm(&mut self, result: Result<Brainstorm, ai::AiError>) {
        self.overlay = None;
        let Some(ideas) = self.ok_or_report(result) else {
            return;
        };
        let today = date::iso(date::today());
        let path = self.project.root.join("bible").join("brainstorm.md");
        self.apply_bible_with_findings(
            path,
            "brainstorm",
            |notes| brainstorm::render(&ideas, notes, &today),
            Vec::new(),
        );
    }

    /// Generate marketing copy (#236) — blurb variants & taglines; written to
    /// `marketing/` by [`App::poll_ai`].
    pub(crate) fn analyze_marketing(&mut self) {
        self.spawn_manuscript_pass(
            "⟳ Drafting marketing copy…",
            marketing::build_request,
            marketing::parse,
            Outcome::Marketing,
        );
    }

    /// Generate social posts (#237) — per-platform promo copy; written to
    /// `marketing/social/` by [`App::poll_ai`].
    pub(crate) fn analyze_social(&mut self) {
        self.spawn_manuscript_pass(
            "⟳ Drafting social posts…",
            social::build_request,
            social::parse,
            Outcome::Social,
        );
    }

    /// Generate ad creative & copy (#251) — per-platform ad fields written to
    /// `marketing/ads/` by [`App::poll_ai`]. Folds in any existing marketing copy
    /// (#236/#237) so the ads stay on-brand.
    pub(crate) fn analyze_ad_creative(&mut self) {
        let marketing = self.marketing_copy_context();
        self.spawn_manuscript_pass(
            "⟳ Drafting ad creative…",
            move |manuscript, model| ad_creative::build_request(manuscript, &marketing, model),
            ad_creative::parse,
            Outcome::AdCreative,
        );
    }

    /// The author's existing marketing copy (#236/#237) as on-brand context for
    /// the ad pass — the blurbs/taglines and social posts already written, bodies
    /// only (front matter stripped). Empty when none exist yet.
    pub(crate) fn marketing_copy_context(&self) -> String {
        let root = self.project.root.join("marketing");
        let files = [
            "blurbs.md",
            "taglines.md",
            "social/x.md",
            "social/facebook.md",
            "social/instagram.md",
        ];
        let mut out = String::new();
        for file in files {
            if let Ok(text) = std::fs::read_to_string(root.join(file)) {
                out.push_str(crate::frontmatter::split(&text).1.trim());
                out.push_str("\n\n");
            }
        }
        out
    }

    /// Apply an ad-creative pass (#251): write the per-platform ad copy to
    /// `marketing/ads/`, conflict-reviewed and off-manuscript.
    pub(crate) fn apply_ad_creative(&mut self, result: Result<AdCreative, ai::AiError>) {
        if let Some(creative) = self.ok_or_report(result) {
            self.write_off_manuscript("Ad creative", "marketing/ads", creative.artifacts());
        }
    }

    /// Apply a submission pass: write the synopsis / blurb / query to
    /// `submission/` (conflict-reviewed like any generated file) and summarize.
    pub(crate) fn apply_submission(&mut self, result: Result<Submission, ai::AiError>) {
        if let Some(sub) = self.ok_or_report(result) {
            let artifacts = sub
                .artifacts()
                .into_iter()
                .map(|(k, f, t)| (k, f, t.to_string()))
                .collect();
            self.write_off_manuscript("Submission material", "submission", artifacts);
        }
    }

    /// Apply a marketing-copy pass (#236): write the blurb variants and taglines
    /// to `marketing/`, conflict-reviewed and off-manuscript like the submission
    /// material.
    pub(crate) fn apply_marketing(&mut self, result: Result<Marketing, ai::AiError>) {
        if let Some(copy) = self.ok_or_report(result) {
            self.write_off_manuscript("Marketing copy", "marketing", copy.artifacts());
        }
    }

    /// Apply a social-posts pass (#237): write the per-platform posts under
    /// `marketing/social/`, conflict-reviewed and off-manuscript.
    pub(crate) fn apply_social(&mut self, result: Result<Social, ai::AiError>) {
        if let Some(posts) = self.ok_or_report(result) {
            self.write_off_manuscript("Social posts", "marketing/social", posts.artifacts());
        }
    }

    /// Write a set of off-manuscript artifacts (#236) — `(title, filename, body)`
    /// — under `<subdir>/`, conflict-reviewed via [`stage_bible`](App::stage_bible)
    /// and summarized. The one writer shared by the submission and marketing passes
    /// (and future off-manuscript copy): they differ only in `label`, `subdir`, and
    /// what their `artifacts()` renders.
    fn write_off_manuscript(
        &mut self,
        label: &str,
        subdir: &str,
        artifacts: Vec<(&'static str, &'static str, String)>,
    ) {
        self.overlay = None;
        let today = date::iso(date::today());
        // `Staged::Wrote` carries no path, so capture each successful write's
        // label as we stage, for the clean-success listing below.
        let mut written_paths = Vec::new();
        let tally = StagedTally::from(artifacts.into_iter().map(|(kind, file, text)| {
            let path = self.project.root.join(subdir).join(file);
            let title = format!("title: {kind}\n");
            let staged = self.stage_bible(path, kind, |notes| {
                bible::render(&title, &text, notes, &today)
            });
            if matches!(staged, Staged::Wrote(_)) {
                written_paths.push(format!("{subdir}/{file}"));
            }
            staged
        }));

        let label = label.to_string();
        // Every `Staged::Wrote` pushed one label, so `written` (the tally count)
        // equals `written_paths.len()`; use it so only the clean closure owns the
        // list, leaving the conflict closure free of a competing borrow.
        self.resolve_staged(
            tally,
            |written, pending| format!("Wrote {written} · {pending} need review"),
            |app, _written| {
                if written_paths.is_empty() {
                    app.message = Some(format!("{label}: nothing generated"));
                    return;
                }
                let mut lines = vec![Line::raw(format!("{label} written:")), Line::raw("")];
                lines.extend(
                    written_paths
                        .into_iter()
                        .map(|w| Line::raw(format!("  {w}"))),
                );
                lines.push(Line::raw(""));
                lines.push(Line::raw(
                    "Off-manuscript drafts — edit freely; never part of the novel.",
                ));
                app.show_ai(lines);
            },
        );
    }
}
