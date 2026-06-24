# AI features (Claude integration)

Quill can call [Claude](https://www.anthropic.com/) to help an author *track* and
*scaffold* a manuscript — turning a long, evolving draft into a queryable,
consistency-checked story bible. AI is **opt-in and off by default**: a
novelist's unpublished work only leaves the machine when the author enables it
and provides an API key.

## Principle: assistant, not co-writer

Quill's AI **never generates or suggests manuscript prose** — no drafting,
rewriting, tightening, expanding, or larger-edit suggestions. It is limited to
**structure** (outlines, beat sheets, scene notes), **tracking** (the story
bible), **markers** (`xxx` TODOs), **analysis** (flags and findings the author
fixes themselves), and **reference lookup** (a thesaurus / word search the author
deliberately invokes for a specific word or phrase, returning candidate words the
author chooses among — it may read the surrounding line to judge fit, but returns
only options for the requested slot and never rewrites the sentence or any other
prose). The prose is the author's. Off-manuscript artifacts like a synopsis,
query letter, or marketing blurb are the only prose the AI produces, and never
inside the manuscript.

## Configuration

AI is configured in a per-user file **outside the project** —
`$XDG_CONFIG_HOME/quill/config.toml` (else `~/.config/quill/config.toml`):

```toml
[ai]
enabled = true                 # opt-in; off by default
backend = "claude"             # "claude" (API) or "local" (on-device Qwen via candle)
offline = false                # true forces the local backend (nothing leaves the machine)
model = "claude-opus-4-8"      # Claude model; the local backend picks its own
cost_threshold = 0.20          # confirm before a Claude pass estimated above this (USD)
refresh_after_changes = 5      # rebuild a bible fully once this many chapters change (else revise incrementally)
# api_key = "sk-ant-..."       # or set ANTHROPIC_API_KEY (env wins); Claude only

[image]                          # image generation (#238) — its own opt-in & key
enabled = true                   # off by default
backend = "remote"               # "remote" (fal.ai) or "local" (on-device SD, #240)
offline = false                  # true forces the local backend
model = "fal-ai/flux/schnell"    # a fal.ai model id (remote backend)
# api_key = "..."                # or set FAL_KEY (env wins); remote backend only
```

Image generation (#238) is a separate opt-in with its own `[image]` section and
key (`FAL_KEY` or `api_key`), since it uses a different provider (fal.ai) and is
billed per image. Its opt-in and key are separate from the text `[ai]` settings;
the portrait action is gated on image readiness and reachable from the run-action
menu (Ctrl-K) even with text AI off (the AI palette itself opens only when text AI
is on).

**Image backends** (#240): the default `remote` backend calls fal.ai (needs a key,
billed per image). The `local` backend runs an on-device Stable-Diffusion model
with candle — no key, no per-image cost, nothing leaves the machine — and
`offline = true` forces it. Like the text local backend, it's a build-time opt-in:
a standard build is remote-only, and selecting `local` without `--features local`
tells you to rebuild. Both satisfy the same image-backend trait, so portraits and
scenes work against either; the local model loads once on first use (off the UI
thread) and downloads its weights via `hf-hub` (cached outside the project).

The API key comes from `ANTHROPIC_API_KEY` (preferred) or the `api_key` field —
never from the project directory. **Backends:** the default `claude` backend sends
the manuscript to the Anthropic API (needs a key); the `local` backend runs an
on-device Qwen with candle (#83/#124) — no key, nothing leaves the machine — and
`offline = true` forces local regardless of `backend`. The local backend is a
build-time opt-in: a standard build is Claude-only, and selecting `local` without
`--features local` tells you to rebuild. Both satisfy the same `quill-ai` trait,
so every feature works against either; the local backend's structured output is
guaranteed by constrained decoding, and it is free (no cost confirm).

With AI enabled, **Ctrl-A** opens the **AI command palette** (#98) — one entry
point listing every AI action; arrow to one and Enter runs it. (When AI is off,
Ctrl-A explains how to enable it.) The status line shows a persistent `AI` tag
when enabled and `⟳ Claude` while a request is in flight; requests run on a worker
thread so the editor never blocks (the first local request also loads the model
there, so it can't freeze the UI).

The palette actions:

- **Brainstorm ideas** (#255) — the first *generative* pass: from a premise or
  open prompt (not the manuscript), brainstorm premise variations, "what if"
  questions, themes, and central conflicts, written as editable notes to
  `bible/brainstorm.md`. Works at chapter zero (no manuscript needed), off by
  default, off-manuscript and conflict-reviewed; idea notes, never prose scenes.
- **Draft characters** (#256) — *generative*: from a concept/premise (not the
  manuscript), invent a starter cast and write it to `bible/characters/` in the
  same #74 format (marked AI-generated, editable). Works at chapter zero; the
  drafted cast then feeds the extractive character pass and reports once prose
  exists. Scaffolding the author reshapes, never manuscript prose.
- **Draft setting** (#257) — *generative*: from a concept, invent a starter
  setting / worldbuilding bible to `bible/setting.md` in the same #77 format
  (categorized entries + gazetteer, marked AI-generated, editable). Works at
  chapter zero and later feeds the setting rule-consistency checks. Scaffolding,
  never prose.
- **Import notes** (#283) — point Quill at an existing Markdown notes file (or a
  folder of them) and it preserves the source under `bible/sources/`, then runs
  one *extractive* pass over the combined text to generate/update the character
  and setting bibles in the project's format — merging via the usual generated
  markers + preserved Notes, with hand-edit conflicts surfaced in the bible
  review. Available (for source preservation) even with AI off. Off-manuscript,
  never prose. Run it from the Ctrl-K menu ("Import notes → story bible").
- **Ask the manuscript** (#73) — a grounded question, answered with chapter
  citations in an overlay.
- **Character bible** (#74) — a profile per character (traits, arc, drift) to
  `bible/characters/<slug>.md`.
- **Timeline & continuity** (#75) — an ordered timeline (`bible/timeline.md`) plus
  contradiction findings.
- **Setting & worldbuilding** (#77) — a categorized setting bible
  (`bible/setting.md`) + a place/term gazetteer + rule-violation findings.
- **Scaffold / outline** (#76) — a structural beat outline (`bible/outline.md`) +
  structural-gap findings (no prose).
- **Name & term consistency** (#78) — a cheap Haiku scan for spelling variants,
  checked against the gazetteer + character names.
- **Prose & style** (#79) — *semantic* findings (POV / tense consistency, voice
  drift, context-aware repeated phrasing); analysis only, no rewrites. Frequency
  metrics (crutch words, etc.) are the local-analysis report's job (#70).
- **Character voice** (#183) — checks the manuscript against the character
  bible's **voice profiles** (the character pass now captures speech patterns,
  vocabulary/register, verbal tics, and sentence rhythm per character, editable
  like the rest of the bible). Flags dialogue that reads out of character for its
  speaker and POV/narration whose voice has drifted or isn't distinct from another
  POV — each finding naming the character and chapters. Analysis only, grounded in
  the bible, never a rewrite; needs the character pass to have run first. Distinct
  from prose & style: this is character-grounded with dialogue attribution.
- **Submission material** (#81) — a synopsis, back-cover blurb, and query-letter
  draft written to `submission/`. Produces prose, but **off-manuscript**
  (submission/marketing artifacts, never in the novel), so it stays within the
  assistant-not-co-writer rule. Each is a conflict-reviewed bible-format file the
  author edits freely.
- **Marketing copy** (#236) — several back-cover blurb variants (different hooks,
  spoiler-free) and one-line taglines for promoting the finished book, written to
  `marketing/` (`blurbs.md`, `taglines.md`). Off-manuscript and conflict-reviewed
  like the submission material; the author edits and regenerates freely. The first
  of the indie-marketing features (epic #104).
- **Social posts** (#237) — ready-to-post promo copy shaped to each platform's
  conventions (X / Facebook / Instagram), written to `marketing/social/` (one file
  per platform). Off-manuscript and conflict-reviewed; edit and regenerate freely.
- **Ad creative** (#251) — platform-ready ad copy (headlines, primary text /
  descriptions, suggested targeting/keywords, and an ad-image brief) shaped to
  Amazon Ads, Meta, and X, written to `marketing/ads/` (one file per platform).
  Drafts from the manuscript and folds in your existing marketing copy (#236/#237)
  to stay on-brand. Off-manuscript and conflict-reviewed; paste into any ad
  manager. (The ad image is a brief — render one with the image features or supply
  your own; account/API publishing is epic #105.)
- **Character portraits** (#238, #244) — portrait images of the bible characters,
  built from their profiles and generated via the image backend, saved to
  `marketing/images/characters/`. Off-manuscript files the author owns and
  regenerates. A picker lets you generate one chosen character or the whole cast;
  the cost confirm reflects the count (each image is billed). Its own `[image]`
  opt-in + key (separate from the text `[ai]` settings). Needs the character pass
  to have run first.
- **Scene illustration** (#239, #247) — an illustration of the active chapter, via
  the same image backend, saved to `marketing/images/scenes/`. When the text AI is
  configured, a read-only pass first distills a **visual brief** of the chapter's
  most pivotal, depictable moment (setting, subjects, composition, mood) and uses
  that as the image prompt — off-manuscript, never the author's prose; otherwise it
  falls back to a prose excerpt + the setting bible. Confirms before generating
  (billed per image).
- **Revision triage** (#82) — every finding surfaced this session (continuity,
  setting, style, naming, structural gaps), aggregated into one de-duplicated
  list in the findings overlay. Local only — no request. Marking a finding as an
  `xxx` TODO is the durable outcome (it lands in the chapter and feeds the
  report's TODO count); dismissals are in-session.
- **Build a workflow from a description** (#232) — from the Ctrl-X workflows
  menu, describe a routine in plain words; the model maps it onto the available
  action tokens (constrained to that closed set, so it can't invent a step) and
  reports anything it couldn't map. The proposal is shown to name and confirm
  before the `[workflows]` entry is written to `config.toml` (preserving the rest
  of the file). Configures tooling, not prose — within the assistant boundary. No
  manuscript is sent, so the request is cheap.

**Cost confirm** (#82): before any whole-manuscript pass, Quill estimates the
token/dollar cost from the manuscript size and the effective model. When the
estimate exceeds `cost_threshold`, it shows a `~12k tokens · ~$0.34 on
claude-opus-4-8 — run? (y/N)` confirm; cheaper passes run straight through. The
estimate is a local upper bound (no `count_tokens` round-trip). Ask and Revision
triage skip it — they're interactive or local. See `src/ai/cost.rs`.

Each bible file has a regenerated block (`<!-- generated:start/end -->`) and an
author-owned `<!-- notes:start/end -->` block that survives regeneration; a
`digest:` detects hand-edits so a pass shows a diff instead of overwriting. The
findings overlay lets the author jump to a finding's chapter, mark it as an `xxx`
TODO, or dismiss it. The shared bible machinery lives in `src/ai/bible.rs`.

### Incremental scoping (#90)

Authors edit a chapter at a time, so re-sending the whole novel on every pass is
almost all wasted work. The persistent-bible passes (character, setting,
timeline) scope themselves to what changed:

- Each pass records a `chapter file → content digest` map of the manuscript it
  was built from, in `bible/.state.toml` (one entry per tracker). The digest is
  content-addressed, so a chapter counts as changed exactly when its text
  differs — no version control or commit timing involved.
- The next run hashes the current chapters and diffs against that record:
  - **nothing changed** → the bible is already current, so no request is sent
    (and the cost confirm never fires);
  - **a few chapters changed** → an *incremental* pass feeds the existing bible
    plus only the changed chapters and asks the model to revise it;
  - **many changed** (≥ `refresh_after_changes`), a chapter was removed, or
    there's no prior record → a full whole-manuscript rebuild.
- The cost estimate reflects what will actually be sent, so an incremental update
  is correctly cheap rather than warning at whole-novel size.

The findings-only passes (naming, style, scaffold) have no persistent cache to
revise, so they always run a full pass. The scoping logic lives in
`src/ai/incremental.rs`. The state file is a derivable cache (gitignored): if it's
absent — a fresh clone, or hand-deleted — the next pass rebuilds in full.

**Force a full rebuild on demand** (#176): in the AI palette (Ctrl-A), press
**R** instead of Enter to rebuild the selected bible from the whole manuscript
regardless of the delta — the recovery path for the case an incremental revise
can't cover (an incremental pass updates and adds bible entries but doesn't delete
a profile file whose character vanishes only within the incremental window).

**Batch API** (#176): every full rebuild — automatic past the threshold or forced
with R — runs through the Claude [Message Batches API](https://docs.anthropic.com/en/api/creating-message-batches)
at half the per-token price. It trades latency for cost (the request runs on the
worker thread and may take a few minutes), but the editor stays fully usable
while it processes; the status line shows `(batch — may take a few minutes)`.
Incremental passes stay on the normal request (they're small and want low
latency), and the local backend has no batch path (it runs the request normally).

## Integration shape

- **No official Anthropic SDK for Rust**, so Quill talks to the Messages API
  (`POST /v1/messages`) directly over HTTPS (`reqwest`), or via a community
  crate if one proves stable. A thin `quill-ai` module wraps requests, retries,
  and streaming.
- **Model:** default to **`claude-opus-4-8`** for analysis and reasoning
  (1M-token context — a whole novel fits in one request); use
  **`claude-haiku-4-5`** for cheap, high-volume mechanical passes (e.g.
  name-variant scans). Model is configurable.
- **Structured outputs** (`output_config.format` with a JSON Schema) so every
  tracking feature returns typed data Quill can store and diff — not prose to
  re-parse. The same schemas define the on-disk story-bible format.
- **Prompt caching** — the manuscript is large and changes slowly, the ideal
  cache prefix. Cache the manuscript (or the unchanged chapters) so repeated
  analyses cost ~10% of the first call. Re-analyze only chapters the
  tree-sitter parse marks dirty.
- **Batch API** (50% cost) for whole-manuscript sweeps that aren't latency
  sensitive — e.g. regenerating every character profile after a big revision.
- **Adaptive thinking** (`thinking: {type: "adaptive"}`) for the
  reasoning-heavy features (continuity, arc analysis); plain requests for
  extraction.
- **Privacy:** explicit enable, key stored outside the project, a clear
  indicator when text is about to be sent, and an offline mode that disables all
  network features. Account for a future local-model backend behind the same
  `quill-ai` trait.

## The story bible

AI features read from and write to a **story bible** — derived artifacts kept in
the project as human-readable Markdown/YAML (e.g. `bible/characters/*.md`,
`bible/timeline.yaml`, `bible/setting.md`), versioned alongside the manuscript.
Generated content is clearly marked and never overwrites the prose. The author
can edit any bible entry; edits are treated as ground truth on the next pass.

## Features

### 1. Character tracking (traits & arcs)

- AI summarization builds a profile per character: physical traits, an actionable
  **voice profile** (speech patterns, vocabulary/register, verbal tics, sentence
  rhythm — see Character voice, #183), relationships, goals, and a **per-chapter
  arc** (state and change over the book), attributed via the tree-sitter
  chapter/scene outline.
- Flags drift the author may not intend (a trait stated in ch. 3 contradicted in
  ch. 20) and surfaces "last seen" / "screen time" per character.
- Incremental: editing one chapter re-summarizes that chapter and updates the
  rolled-up arc, not the whole book.

### 2. Timeline & continuity

- Extracts dated/relative events into a timeline and orders them, separating
  story order from narrative order.
- **Continuity checks** look for contradictions — a character in two places at
  once, an age/season/eye-color that doesn't add up, an object used after it was
  destroyed — and report them with the chapters involved (rendered like the
  continuity findings in the progress report).
- Adaptive thinking does the cross-referencing reasoning; findings are
  structured so the author can dismiss or jump to each.

### 3. Scaffolding (outlines & TODO notations)

- Generate an **outline / beat sheet** from the draft: per chapter, the key
  story beats as short structural **notes** (never prose). Written to
  `bible/outline.md` (#76).
- Surface **structural gaps** ("a chapter lacking a clear goal", "a missing
  transition", "a setup with no payoff") as findings, and let the author turn any
  finding into an **`xxx` TODO marker** in its chapter (the same token the
  progress report counts) — a note, never prose.
- Per the assistant-not-co-writer principle, scaffolding produces structure and
  notes only; it never drafts, expands, or suggests manuscript prose.

### 4. Setting & worldbuilding

- Extracts a **setting bible**: geography/locations, the rules of any
  magic/tech system, factions, cultures, and timeline of world history.
- **Rule-consistency checks** — flags where the prose violates an established
  rule the author (or a prior pass) recorded ("magic requires a focus, but ch.
  14 casts without one").
- Keeps a place/term gazetteer used by naming-consistency (below).

### Additional features worth adding

- **Naming & term consistency** — detect spelling variants of character/place
  names and inconsistent invented terms (cheap Haiku pass over the entity list).
- **Prose & style** — POV/tense consistency per scene, crutch-word and
  repeated-phrase detection, per-POV voice drift, pacing/quiet-vs-action heatmap.
  *Analysis only* — flags and observations the author acts on, never a rewrite.
- **"Ask the manuscript"** — natural-language Q&A grounded in the cached
  manuscript ("where do I first mention the locket?", "summarize Mara's
  relationship with her brother").
- **Synopsis & query material** — generate a synopsis, back-cover blurb, or
  query-letter draft. *Off-manuscript submission material only* — never prose
  written into the book.
- **Revision triage** — turn continuity/character/setting findings into a
  prioritized, dismissible to-do list (feeds the `xxx` and report tooling).

(The rewrite/tighten/expand "selection assistant" was dropped: editing the
author's prose is out of scope per the assistant-not-co-writer principle above.)

## Open questions

- **Cost/visibility** — the pre-pass cost confirm (#82), incremental scoping
  (#90), and the on-demand full rebuild + Batch API (#176) all ship; see above.
- **Trust & provenance** — every AI-written bible entry needs a clear source
  ("generated from ch. 1–12, edited by you") so authors can trust or correct it.
- **Local model backend** — define the `quill-ai` trait so a future on-device
  model can satisfy the privacy-first authors who won't send drafts anywhere.
- **Determinism** — analyses should be re-runnable and diffable; pin model
  versions in the bible so a re-run's changes are attributable.
