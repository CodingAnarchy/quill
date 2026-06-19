# AI features (Claude integration)

Quill can call [Claude](https://www.anthropic.com/) to help an author *track* and
*scaffold* a manuscript — turning a long, evolving draft into a queryable,
consistency-checked story bible. AI is **opt-in and off by default**: a
novelist's unpublished work only leaves the machine when the author enables it
and provides an API key.

## Configuration

AI is configured in a per-user file **outside the project** —
`$XDG_CONFIG_HOME/quill/config.toml` (else `~/.config/quill/config.toml`):

```toml
[ai]
enabled = true                 # opt-in; off by default
offline = false                # true disables all network calls
model = "claude-opus-4-8"      # or claude-haiku-4-5 for cheap passes
# api_key = "sk-ant-..."       # or set ANTHROPIC_API_KEY (env wins)
```

The API key comes from `ANTHROPIC_API_KEY` (preferred) or the `api_key` field —
never from the project directory. With AI enabled, **Ctrl-A** asks a question
about the manuscript and the grounded answer (with chapter citations) opens in an
overlay. The status line shows a persistent `AI` tag when enabled and `⟳ Claude`
while a request is in flight. *(Shipped in #73: the `quill-ai` backend trait, the
Claude HTTP backend, and "Ask the manuscript" Q&A. The request runs on a worker
thread so the editor never blocks.)*

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

- AI summarization builds a profile per character: physical traits, voice,
  relationships, goals, and a **per-chapter arc** (state and change over the
  book), attributed via the tree-sitter chapter/scene outline.
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

- Generate an **outline / beat sheet** from a premise, logline, or partial
  draft; expand a one-line chapter summary into proposed scenes.
- Insert **`xxx` TODO markers** (the same token the progress report counts) at
  gaps — "`xxx` motivation unclear here", "`xxx` needs a transition" — so
  scaffolding feeds the existing stats loop.
- Suggest next-scene options or "what's missing" against the outline, always as
  drafts the author accepts or rejects; nothing is written without confirmation.

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
  repeated-phrase detection, per-POV voice drift, pacing/quiet-vs-action
  heatmap.
- **"Ask the manuscript"** — natural-language Q&A grounded in the cached
  manuscript ("where do I first mention the locket?", "summarize Mara's
  relationship with her brother").
- **Selection assistant** — rewrite/tighten/expand a highlighted passage, or
  brainstorm in a side panel, with edits staged as diffs.
- **Synopsis & query material** — generate a synopsis, back-cover blurb, or
  query-letter draft from the finished manuscript.
- **Revision triage** — turn continuity/character/setting findings into a
  prioritized, dismissible to-do list (feeds the `xxx` and report tooling).

## Open questions

- **Cost/visibility** — show estimated token cost before a full-manuscript pass;
  let the author scope analyses to a chapter/act/book.
- **Trust & provenance** — every AI-written bible entry needs a clear source
  ("generated from ch. 1–12, edited by you") so authors can trust or correct it.
- **Local model backend** — define the `quill-ai` trait so a future on-device
  model can satisfy the privacy-first authors who won't send drafts anywhere.
- **Determinism** — analyses should be re-runnable and diffable; pin model
  versions in the bible so a re-run's changes are attributable.
