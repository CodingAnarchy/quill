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
# The key is NOT kept here — run "Set Claude API key" from the Ctrl-K menu to
# store it in the OS keychain (see "Where the API key lives" below). A legacy
# plaintext `api_key = "sk-ant-..."` still works but is the lowest precedence.

[image]                          # image generation (#238) — its own opt-in & key
enabled = true                   # off by default
backend = "remote"               # "remote" (fal.ai) or "local" (on-device SD, #240)
offline = false                  # true forces the local backend
model = "fal-ai/flux/schnell"    # a fal.ai model id (remote backend)
# As with Claude: run "Set image API key" from the Ctrl-K menu to store it in the
# OS keychain; FAL_KEY (env) overrides; a legacy plaintext `api_key` is last.
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

**Where the API key lives (#530).** Quill no longer keeps the key in
`config.toml`, where it would ride along into a synced dotfile repo, a backup, or
a casual `cat` of the file. The key is resolved by precedence:

1. **Environment variable** — `ANTHROPIC_API_KEY` (text) / `FAL_KEY` (image). The
   headless / CI escape hatch: a server or CI box has no login session to unlock
   the OS keychain, so the env var covers it.
2. **OS keychain** — the macOS Keychain, the Linux Secret Service (libsecret), or
   the Windows Credential Manager. Set it from inside Quill with **Set Claude API
   key** / **Set image API key** in the `Ctrl-K` run-action menu: you type the
   key once, Quill writes it to the OS store, and it never touches the config
   file. This is the recommended path.
3. **Plaintext `api_key`** in `config.toml` — still read, so existing setups keep
   working, but lowest precedence. To migrate: run the set-key command (which
   stores the key in the keychain) and accept its offer to strip the now-redundant
   plaintext line from your config.

On a headless Linux box with no Secret Service daemon, the keychain write fails
and Quill tells you to use the env var instead — that's the supported path there.

**Threat model.** Keychain storage protects against *accidental disclosure*: a
key leaking through a dotfile sync, a backup, or someone glancing at your config.
It does **not** protect against a local attacker who can already run Quill (or any
process) as you — the OS unlocks the store for your logged-in session, so anything
running as you can read it, exactly as you can. The goal is to stop the key from
sitting in a file that travels off the machine, not to defend a compromised login.

**Backends:** the default `claude` backend sends
the manuscript to the Anthropic API (needs a key); the `local` backend runs an
on-device Qwen with candle (#83/#124) — no key, nothing leaves the machine — and
`offline = true` forces local regardless of `backend`. The local backend is a
build-time opt-in: a standard build is Claude-only, and selecting `local` without
`--features local` tells you to rebuild. Both satisfy the same `quill-ai` trait,
so every feature works against either; the local backend's structured output is
guaranteed by constrained decoding, and it is free (no cost confirm).

**Switching backends at runtime (#371):** besides the config setting, **Switch
AI backend** in the `Ctrl-K` run-action menu flips the active backend between
Claude and local without editing `config.toml` or restarting — its menu line
shows which one is active right now (`… · now: Claude (cloud)`). The switch is
**session-only**: the next AI request uses the new backend (the local model
loads off the UI thread on first use, reusing its cached slot), and a restart
reverts to the configured `backend`. `offline = true` stays authoritative — it
pins local, so the switch reports the pin instead of overriding it — and
selecting local in a build without `--features local` gives the same rebuild
hint as the static setting rather than appearing to switch and then failing.

**Local model set (#377):** by default the local backend loads the single
hardware-recommended Qwen. Listing models in `[ai] local_models` instead makes a
*set* available, and the router picks the best of them per request — the smallest
for a quick lookup (lowest latency), the largest for a substantial pass (best
quality):

```toml
[ai]
local_models = ["1.5B", "7B"]   # ids from the model table; empty ⇒ the auto pick
```

Each model is downloaded on first use (cached and resumable) and then reused;
several can be cached at once and switched between with no reload. An empty list
keeps the prior behavior (one auto-selected model).

You needn't edit the config by hand: **Local AI models** in the `Ctrl-K` menu
opens an in-editor manager (#379) that lists the catalog — each model with its
size, RAM need, context window, a one-line description, and whether it's on disk
(`✓ downloaded`). **Enter / Space** toggles a model in your set (written back to
`[ai] local_models`); **`d`** downloads one now (off the UI thread, the editor
stays responsive); **`x`** removes a downloaded model to reclaim disk, after a
confirm (#382). A model you add but don't explicitly download is still fetched on
first use, as before.

**Context windows and whole-book passes (#378):** every model carries its real
context window, and the router routes a pass on-device only when a downloaded
model can hold it. The standard Qwen2.5 models hold ~32k tokens, so a big
whole-manuscript or series pass would still fall back to the cloud — *unless* you
download an **extended-context** variant. The catalog includes `7B-1M`
(Qwen2.5-7B-Instruct-1M, a ~1,010,000-token window), so a `prefer-local` or
`cost-ceiling` policy can run whole-book continuity, setting, and character
passes entirely on-device — free, private, offline — instead of paying the cloud.
The cloud fallback fires (with its notice) only when no downloaded model's window
fits the pass. Long context is memory-hungry (the KV cache), so the extended
models carry a higher RAM floor; the in-editor manager (#379) will surface that
when choosing.

**Hybrid routing (#372):** instead of one static backend, `[ai] routing` can pick
local vs remote *per action*, turning the cost estimator from a warning into a
decision. The policy is a pure function of (estimate, the action's local
capability, preference), so it's predictable and testable:

```toml
[ai]
routing = "configured"   # configured (default) | prefer-local | prefer-cloud | cost-ceiling | ask
```

- `configured` — always the session backend; the router is off (the historical
  behavior, so an unset config is unchanged).
- `prefer-local` — run on-device whenever the local backend is available and the
  pass fits its context budget (~24k tokens); larger passes fall back to Claude
  with a notice.
- `prefer-cloud` — always Claude (with the usual over-threshold confirm).
- `cost-ceiling` — Claude until the estimate exceeds `cost_threshold`, then route
  on-device instead of paying (falling back to Claude only when the pass is too
  big for local).
- `ask` — when the estimate is over the threshold, offer the choice at the
  confirm: **y** spend on Claude · **l** run locally · anything else cancels. The
  label compares them (`~$0.34 on claude-opus-4-8 — or free on-device`).

The decision is always visible: the confirm shows the comparison up front, and
the in-flight status tag names the backend a pass is actually running on
(`⟳ Claude` / `⟳ local`). The hard controls still win — `offline = true` keeps
everything local, and a per-action choice at the `ask` confirm overrides the
policy. A pass the local model can't hold is never silently routed to it; it
falls back to the cloud with a notice. Routing only changes *where* an action
runs — never whether it can touch manuscript prose.

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
- **Import notes** (#283, #288) — point Quill at an existing Markdown notes file
  (or a folder of them) and it preserves the source under `bible/sources/`, then
  runs one *extractive* pass over the combined text to generate/update whatever
  the notes contain: character profiles, the setting bible (incl. magic/tech
  systems, factions, world history as categories), the story timeline, and a beat
  outline — each in the project's format, merging via the usual generated markers
  + preserved Notes, with all hand-edit conflicts surfaced together in the bible
  review. Sections the notes don't cover are omitted, not fabricated. Available
  (for source preservation) even with AI off. Off-manuscript, never prose. Run it
  from the Ctrl-K menu ("Import notes → story bible").
- **Ask the manuscript** (#73) — a grounded question, answered with chapter
  citations in an overlay.
- **Word search** (#290) — an in-context thesaurus: name a word or phrase (the
  word under the cursor is prefilled) plus optional plain-language constraints
  (count, closeness, syllable count, language of origin, rarity); the current
  line is sent as *context for fit only*. Returns candidate words/short phrases
  with brief definitions in the overlay. A **reference tool, not a co-writer** —
  it returns only candidates for the requested slot and never rewrites the line
  or any other prose. Sends just the word + line (no manuscript), so it's cheap
  and skips the cost confirm.
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
- **Editorial read** (#293) — a holistic editor's read across three lenses —
  **structural** (pacing, stakes, a sagging middle), **continuity**
  (contradictions across chapters), and **line** (grammar, usage, typos,
  awkwardness) — surfaced as located, plain-language flags in the revision-triage
  list, each tagged by lens (and line issues quoting the span). It *only points
  out* issues: never a fix, a corrected sentence, or replacement prose — the
  author decides what to change. The analysis category of the assistant-not-
  co-writer rule, taken to its sharpest: advice you accept or ignore, not prose to
  adopt.
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

### Series bible (#336)

A multi-book series shares **one** bible across its books, but the whole series
at once won't fit a model's context window. So each persistent-bible pass is
**book-scoped** (#339): it sends only that book's chapters (a first build, or just
its changed chapters via the incremental scoping above) plus the existing shared
bible to *revise* — never another book's prose. A book's "full" pass on a series
revises the shared bible rather than rebuilding it, so folding one book in never
discards the others.

**Refresh series bible** (#340, the Ctrl-K menu, multi-book projects only) drives
this across the whole series: it runs each book's character, timeline, and setting
pass in reading order — one book per AI call — so each book builds on the bible
the earlier books wrote. Only books with changes run; the status line shows
per-book progress and **Esc** cancels cleanly after the in-flight pass. A
standalone novel uses the per-book passes directly (the action is unavailable).
The fast active-book passes (Ctrl-A) stay as they were. *Trade-off:* a deleted
chapter in a series book leaves a stale bible entry until a full rebuild — the
cost of not re-scanning every book on each refresh.

**Cross-book findings** (#341) fall out of the same mechanism. The timeline and
setting passes already report contradictions against the bible they're given;
during a series refresh that bible is the *accumulated* one, carrying the state
the earlier books established. So when book three's pass runs, it checks book
three's chapters against a timeline that already records book one's events — and a
character who died in book one but reappears in book three, or a world rule book
two set and book four breaks, is flagged even though no single request ever holds
more than one book's prose. The revise prompts say so explicitly: the bible
records material *not shown* in this request, and contradictions with it count.

These findings accumulate into the **same revision-triage list** (the Ctrl-K
"Revision triage" action) rather than popping an overlay after every book — the
refresh's summary line reports how many landed. Each carries the book it was found
in (chapter titles repeat across books, so a bare title is ambiguous): triage
shows it as `[Book Two · Dusk]`, dedup is per-book, and pressing Enter (or `i` to
mark an `xxx`) switches to that book before jumping. A standalone novel's findings
are unchanged — they target the active book, as before.

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

## The assistant pane (#307/#392)

Every feature above is reachable from the AI palette (`Ctrl-A`), but the
**assistant pane** (`Ctrl-W a`, or from the `Ctrl-K` run-action menu — it's a
registered action like the rest, #399; docked beside the prose) is the
conversational front door: the author types a request in plain language and the assistant runs
the editor's registered actions to carry it out, keeping the conversation across
turns. It drives the tool-use loop (#306) one turn at a time from the UI thread —
network turns run off-thread so typing never freezes; tool execution runs on the
UI thread where the editor state lives.

The assistant can run the **mechanical** actions (save, export, the reports,
spelling navigation, view toggles) and the **analysis passes** (continuity,
characters, setting, scaffold, naming, style, voice, editorial). A pass runs as a
normal background request and its findings open in the editor exactly as from the
palette; the pane reports that it ran and the conversation continues.

It can also run the actions that take an **argument** (#390): ask the manuscript a
question, brainstorm from a premise, or draft a character / setting bible from a
concept. Each tool's input schema is derived from the action itself (the single
source from #306), so the model supplies the argument and the action runs with it
instead of opening a prompt the assistant couldn't answer; a Q&A answer comes back
in the conversation. (The thesaurus and image passes stay out — they need cursor
context the assistant has no way to give.) Because a
pass costs money, the same cost gate applies: when the estimate exceeds the
threshold the assistant asks for a `y` / skip confirmation **in the conversation**
(not a modal, which would fight the pane for keys).

A conversational *loop* is many calls, not one, so it's cost-controlled too (#309).
The pane shows the run's running spend in its title (`~$0.12`), and a ceiling
estimate — the manuscript counted at the cached-prefix rate, not re-billed each
turn — accumulates across turns. The first turn always runs; once a run's
projected spend crosses the `[ai] cost_threshold`, the assistant asks `y` /
stop **before each further turn**, so an agentic loop can't run away into a
surprise bill. Esc cancels a run at any point, stopping further calls. On the
local backend every turn is free, so the loop is never gated. The interactive AI actions
that need their own typed input (ask, brainstorm, the drafts, the thesaurus, the
image passes) and the off-manuscript copy generators aren't offered to the
assistant.

The assistant runs on **either backend** (#310): it uses the session backend, so
switching to the local model (the `Ctrl-K` backend switch, or an `offline` pin)
makes the assistant work fully offline, with no change to the pane. Claude uses
its native tool-calling; the local Qwen has none, so each local turn is one
**constrained choice** — a JSON `{action, message}` over the registry-derived tool
names plus `respond`, which constrained decoding (`src/ai/constrain.rs`)
guarantees is valid and on-list. The model makes one choice per turn (it can run a
tool, see the result, then choose again), so a small model never has to juggle
several calls at once. Tool-use quality scales with the chosen model; a
whole-manuscript request wants an extended-context model (#378). The
prose-boundary guardrail holds identically offline — the local model can only
choose a vetted tool name (none writes prose), and the same execution-layer
allowlist re-checks it.

### The prose boundary, enforced (#308)

A conversational pane invites the one request Quill must never fulfil — "rewrite
this paragraph," "make this punchier," "draft the next scene." The boundary above
holds here as a **two-layer, tested contract**, not a prompt nicety:

1. **Refuse and redirect (prompt).** The system prompt tells the assistant it
   cannot write or change prose and has no tool that does; asked to draft,
   rewrite, tighten, expand, or reword, it refuses and offers what it *can* do
   instead — flag continuity / style / voice issues for the author to fix, run an
   analysis pass, or open a report.
2. **Allowlist at the execution layer (code).** Every tool call is checked against
   the registry-derived allowlist before it runs; a call to anything off-list —
   or to a tool the model simply invented — is blocked in code, regardless of what
   the model emits. The allowlist contains **no prose-writing action**, so the
   assistant has no capability to edit chapter prose. It's an allowlist, not a
   denylist: a newly added action is excluded until it's deliberately vetted in.

Tests feed prose-generation requests and off-list / invented tool calls and
assert nothing is written to the chapter and no off-list tool runs — so the
riskiest surface is a guarantee, not a hope.

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
