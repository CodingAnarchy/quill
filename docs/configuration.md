# Configuration reference

Quill keeps its global settings in a single TOML file in your home config
directory:

```
$XDG_CONFIG_HOME/quill/config.toml    # or, if that's unset:
~/.config/quill/config.toml
```

This is the **home config** — the settings shared across every project you open.
It's plain TOML, organized into sections: `[ai]`, `[image]`, `[editor]` (with an
`[editor.keys]` sub-table), `[theme]`, `[projects]`, `[snippets]`,
`[workflows]`/`[triggers]`, and `[general]`. Every section is optional; a key you
leave out takes its built-in default, and an empty or absent file means Quill
runs entirely on defaults (AI off, the dark theme, no snippets, and so on).

This document is the one place that lists every home-config setting: what it
does, its default, its valid values, and an example. For the AI feature designs
behind `[ai]`/`[image]` see [`ai-features.md`](ai-features.md); for the
getting-started narrative see the in-app guide ([`guide.md`](guide.md), opened
with F1).

## The three ways to change a setting

1. **Hand-edit `config.toml`.** Open the file in any editor and add or change a
   key. This is the only way to reach everything below — some sections (`[ai]`,
   `[image]`, `[snippets]`, `[workflows]`/`[triggers]`) have no in-app editor and
   are configured here. Quill re-reads the file on the next launch (or, for the
   `[editor]`, `[theme]`, and `[editor.keys]` knobs, live when you use the Settings
   menu below).
2. **The in-app Settings menu** (`settings` in the Ctrl-K menu; #592). A plain,
   offline, value-showing picker for the everyday `[editor]` and `[theme]` knobs
   — theme, column width, smart typography, Vim mode, autosave interval,
   typewriter and focus defaults, paragraph indent — plus the per-project
   `[analysis]` stopwords. It also carries two sub-menus (#1109): **Theme colors…**,
   which overrides any individual `[theme]` color field (a color value entered in
   the same syntax as the file; a blank entry resets the field to the base theme's
   value), and **Key bindings…**, which lists the Vim `[editor.keys]` bindings and
   remaps any of them (one key; a blank entry resets to the default). Each change is
   spliced into `config.toml` (preserving the rest of the file) and applied to the
   running editor with no restart.
3. **`:set …`** — the same everyday knobs from the command line, plus, with AI
   on, **"Configure by description"**: you describe the change in plain language
   ("a warm dark theme", "narrow the column") and the AI maps it onto concrete
   `[editor]`/`[theme]` keys, which you confirm before anything is written.

The Settings menu and describe-by-description paths only ever write the
`[editor]` (including `[editor.keys]`) and `[theme]` sections (and, for
describe-by-description, words to the project dictionary). Everything else is
hand-edited.

## Global vs. per-project settings

This file holds the **global** settings. A single book can override any of them
for itself in its `book.toml` manifest, under a `[quill]` table whose sub-tables
mirror this file's sections (`[quill.editor]`, `[quill.theme]`,
`[quill.snippets]`, `[quill.workflows]`, `[quill.projects]`). Precedence is
per-field: a key set under `[quill]` wins, one it leaves unset falls back to the
global value, and one neither sets falls back to Quill's built-in default. Your
AI keys and other private settings stay in the global file, never in a committed
`book.toml`.

Separately, `book.toml` also carries tables that are **only** per-project — they
have no global equivalent because they describe one book:

- `[goals]` — a `daily` word-count target and a `deadline` date (see the README's
  progress-reports section and [`progress-reports.md`](progress-reports.md)).
- `[analysis]` — analysis tuning such as `stopwords_add` (editable in-app via the
  Settings menu's "Analysis stopwords" row).
- `[print]` and `[style]` — print trim size and typographic design for the
  designed exports (documented in the README's export section).

Those tables live with the book and travel with it; the sections below are the
global file only.

---

## `[ai]` — Claude and local AI

AI is **opt-in and off by default**: an unpublished manuscript only leaves the
machine when you turn AI on and provide a key. The full design and privacy model
are in [`ai-features.md`](ai-features.md).

| Key | Default | Valid values | What it does |
| --- | --- | --- | --- |
| `enabled` | `false` | `true` / `false` | Master switch. With it off, no AI feature is reachable. |
| `backend` | `"claude"` | `"claude"` / `"local"` | Which provider runs AI passes: the Claude HTTP API (sends the manuscript off-machine, needs a key) or a local on-device model (offline, nothing leaves the machine). |
| `offline` | `false` | `true` / `false` | Privacy switch: when `true`, force the local backend regardless of `backend`, so nothing is ever sent off-machine. A full-book scan too big for the on-device window (40960 tokens) is folded chapter-by-chapter for the bible passes, or — where folding can't apply — refused with a clear "too large for the local model" message rather than failing (#849); see `docs/ai-features.md`. |
| `routing` | `"configured"` | `"configured"` / `"prefer-local"` / `"prefer-cloud"` | Hybrid routing policy (#372): `configured` always uses `backend`; `prefer-local` uses the local model whenever it's downloaded and big enough for the pass; `prefer-cloud` prefers Claude. |
| `model` | `"claude-opus-4-8"` | a Claude model id | The model id sent to the Claude backend. The local backend picks its own. |
| `local_models` | `[]` | a list of catalog model ids, e.g. `["1.7B", "8B"]` | The local models you want available; the router picks the best of these per request. Empty means the single hardware-recommended model. Usually managed from the in-app **model manager** (Ctrl-K → model manager) rather than by hand. |
| `embed_model` | `""` (the default embedder) | an embedding model id, e.g. `"minilm"` | The on-device embedding model for grounded-Ask semantic retrieval (#831). Only used by a `local`-feature build; when its weights aren't on disk, Ask falls back to sending the whole book. |
| `api_key` | *(unset)* | a Claude API key string | Your Claude API key. The `ANTHROPIC_API_KEY` environment variable and the OS keychain both take precedence over this, so you rarely need to store the key in plaintext here. |
| `cost_threshold` | `0.20` | a dollar amount (float) | Confirm before any Claude pass whose estimated cost exceeds this many dollars. Cheaper passes — and the free local backend — run without a prompt. |
| `refresh_after_changes` | `5` | a whole number | Rebuild the story bible from the whole manuscript (instead of an incremental revise) once this many chapters have changed since its last pass. `0` makes every pass a full rebuild. |
| `background-batch-update` | `false` | `true` / `false` | Opt in to the **background bible update** (#950): a "Queue a background bible update" run-action that rebuilds every story bible through Claude's asynchronous Message Batches API — **half the per-token price**, but a processing window of up to ~24h. It submits and returns immediately (never blocking writing), persists the batch ids to `bible/.batch.toml`, and applies the results on a later launch or idle tick, so it survives restarts. A pass whose chapters changed while the batch was in flight is skipped as stale (re-queue to refresh) rather than overwriting your edited bible. Needs the Claude (cloud) backend; the interactive refreshes stay synchronous. |

```toml
[ai]
enabled = true
backend = "claude"
model = "claude-opus-4-8"
cost_threshold = 0.50
refresh_after_changes = 10
background-batch-update = true   # opt in to the async, half-price batch update (#950)
```

The easiest way to turn AI on and set a key is in-app (the run-action menu's
"Turn AI on" / "Set API key"), which writes `enabled = true` and stores the key
in your OS keychain — see [`ai-features.md`](ai-features.md).

## `[image]` — image generation

Image generation (character portraits, scene illustrations) is its own opt-in
with its own key, separate from the text `[ai]` settings. Off by default; the
generated images are saved as files you own.

| Key | Default | Valid values | What it does |
| --- | --- | --- | --- |
| `enabled` | `false` | `true` / `false` | Opt-in for image generation. |
| `backend` | `"remote"` | `"remote"` / `"local"` | Which provider generates: the remote fal.ai API (needs a key; sends the prompt off-machine) or a local Stable-Diffusion model (offline, no per-image cost; behind the `local` build feature). |
| `offline` | `false` | `true` / `false` | Privacy switch: when `true`, force the local backend regardless of `backend`. |
| `model` | `"fal-ai/flux/schnell"` | a fal.ai model id | The remote model to generate with. Unused by the local backend. |
| `api_key` | *(unset)* | a fal.ai API key string | The remote provider key. The `FAL_KEY` environment variable and the OS keychain take precedence, so you rarely need it in plaintext here. |

```toml
[image]
enabled = true
backend = "remote"
model = "fal-ai/flux/schnell"
```

## `[editor]` — editing behavior

Editor behavior. The defaults keep Quill's modeless editing unchanged (with smart
typography on). Every key here is also editable from the in-app Settings menu
except the `[editor.keys]` remaps, which are hand-edited.

| Key | Default | Valid values | What it does |
| --- | --- | --- | --- |
| `vim` | `false` | `true` / `false` | Opt-in Vim-style modal editing (#27). Off by default the editor behaves exactly as it always has. |
| `smart_typography` | `true` | `true` / `false` | As you type, straight quotes become curly (`“” ‘’`), `--` becomes an em-dash (`—`), and `...` an ellipsis (`…`) (#296). Set `false` for plain ASCII punctuation and an exact byte-for-byte round-trip of what you type. |
| `auto_pair` | `false` | `true` / `false` | Auto-close quotes and brackets as you type (#1007): typing an opener (`"`, `(`, `[`, or an opening curly quote) inserts its matching closer and drops the cursor between the pair; typing the closer when it's already at the cursor skips past it instead of duplicating. Off by default. Composes with `smart_typography` — with both on, a `"` becomes a correctly-oriented curly-quote pair. |
| `column_width` | `72` | `32`–`160` (characters) | The centered text column's starting width (#303). The runtime `Ctrl-+` / `Ctrl--` adjustment still applies on top. |
| `typewriter` | `false` | `true` / `false` | Typewriter scrolling (#301): keep the cursor's line pinned at the vertical centre of the page, the text scrolling under it. |
| `focus_mode` | `false` | `true` / `false` | Focus mode (#301): dim every paragraph but the one under the cursor. |
| `paragraph_indent` | `false` | `true` / `false` | Visually indent the first line of each prose paragraph in the editor (#722), matching the exports. Display-only — no spaces enter the buffer or the saved file. |
| `autosave_interval_secs` | `0` (off) | `0`–`3600` (seconds) | Idle-timer autosave (#409): after this many seconds with no input, the active chapter is saved automatically through the same atomic, front-matter-preserving path as Ctrl-S (without firing on-save workflows). `0` turns autosave off. |

```toml
[editor]
vim = false
smart_typography = true
column_width = 90
autosave_interval_secs = 30
```

### `[editor.keys]` — Vim key remaps

When Vim mode is on, the `[editor.keys]` table remaps Normal-mode actions to the
keys you prefer (#208): each entry is `action = "key"`, where the action is a
built-in name (e.g. `down`, `up`, `word_forward`) and the value is the key that
should trigger it. The keymap is validated when the editor builds it — an unknown
action is reported in the status line, not silently dropped. Absent by default
(the standard Vim bindings apply).

You can view and remap these bindings in-app from the Settings menu's **Key
bindings…** sub-menu (#1109) — it lists every remappable action with its current
key; picking one prompts for a single key (blank resets it to the default) and
applies the change live.

```toml
[editor.keys]
down = "n"
up = "e"
```

## `[theme]` — colors

Quill ships two built-in themes selected by `name`, plus per-field color
overrides layered on top (#300). With no `[theme]` set, Quill uses `dark` — its
original palette — so an unconfigured editor looks exactly as it always has.

| Key | Default | Valid values | What it does |
| --- | --- | --- | --- |
| `name` | `"dark"` | `"dark"` / `"light"` | The base color theme. |

Any individual color can be overridden on top of the chosen theme. A color value
is an ANSI color name (`cyan`, `darkgray`, …), a hex string (`#1e1e24` or
`#abc`), an `r,g,b` triple (`16,16,20`), or a `0`–`255` palette index. The
overridable color fields (all optional, each defaulting to the base theme's
value):

| Field | Controls |
| --- | --- |
| `heading` | Headings and accents (active rows, banner backgrounds) |
| `on_accent` | Text on an accent / match / mode-chip background |
| `dim` | Secondary text and chrome (hints, dividers, attributions) |
| `muted` | Muted prose set off from the body (epigraphs, placeholders) |
| `code` | Preformatted (fenced code) blocks |
| `code_bg` | The fenced-code-block background box |
| `match_current` | The current search match's background |
| `match_other` | Other (non-current) search matches' background |
| `status_bg` | The status bar's background |
| `added` | Additions: diff `+`, a positive day's net, the `Final` status |
| `removed` | Removals: diff `-`, a negative day's net |
| `revised` | The `Revised` chapter status |
| `mode_insert` | The Insert-mode status indicator |
| `mode_normal` | The Normal-mode status indicator |
| `mode_visual` | The Visual-mode status indicator |
| `annotation` | An inline author annotation's highlighted span |

```toml
[theme]
name = "dark"
heading = "#7aa2f7"      # headings and accents
dim = "gray"             # secondary text, hints, dividers
status_bg = "16,16,20"   # the status bar background
match_current = "yellow"
```

Any of these color fields can also be overridden in-app from the Settings menu's
**Theme colors…** sub-menu (#1109) — it lists every field with its current value;
picking one prompts for a color in the same syntax as this file (blank resets it to
the base theme's value) and repaints the editor live.

An unknown theme name, an unknown field, or an unparseable color is reported in
the status line at startup and falls back to the default for that piece, rather
than failing to start.

## `[projects]` — the project browser

Where the project browser looks for books, and how a bare `quill` launch behaves
(#272).

| Key | Default | Valid values | What it does |
| --- | --- | --- | --- |
| `home` | *(unset)* | a folder path; `~` expands to the home directory | A folder holding your projects (e.g. `~/Writing`); the browser lists every project under it. Unset the browser shows only recently-opened projects. |
| `setup_wizard` | `true` | `true` / `false` | Whether a newly-created project offers the setup wizard (#282). Set `false` to never be prompted. |
| `open_last` | `false` | `true` / `false` | Whether a bare `quill` launch (no path, not inside a project) reopens the most-recent project directly instead of showing the create/open chooser (#613). |

```toml
[projects]
home = "~/Writing"
open_last = true
```

## `[snippets]` — text snippets

Author-defined text snippets (#411): reusable bits of text (a scene-break glyph,
a legal name, a stock phrase) inserted verbatim from the snippet picker. Each
entry is `name = "literal text"`; the picker lists them sorted by name.

The text may contain a single caret marker `$0` (#823): on insertion the marker
is consumed and the cursor lands where it was, so a template can position the
cursor mid-snippet. Only the first `$0` is a caret; escape a literal `$0` as
`$$0`. A non-string entry is reported and skipped, not a parse failure.

```toml
[snippets]
scene = "* * *"
copyright = "© 2026 Your Name. All rights reserved."
quote = "“$0”"
```

## `[workflows]` and `[triggers]` — author workflows

A **workflow** is a named, ordered sequence of registry actions run as a single
step (#226, #228). Each step token is either an action name (e.g. `export:docx`)
or the name of another workflow, expanded in place when the workflow runs. Run one
with **Ctrl-X**. Workflows are validated at load: an unknown or interactive step,
or a cycle, is reported.

```toml
[workflows]
publish = ["save", "export:docx", "export:epub"]
nightly = ["bible:update", "analysis:report"]
```

**Triggers** fire a workflow automatically beyond the run-workflow menu (#228).
Entries naming an unknown workflow (or a bad key) are dropped and reported at
load, never silently kept.

| Key | Default | Valid values | What it does |
| --- | --- | --- | --- |
| `on_save` | *(unset)* | a defined workflow name | The workflow run after each manual save (Ctrl-S / `:w`). Idle autosave deliberately does **not** fire it. |
| `keys` | *(none)* | single-character key → workflow name | Vim Normal-mode keys bound to workflows. |

```toml
[triggers]
on_save = "publish"

[triggers.keys]
p = "publish"
```

## `[general]` — app-level flags

App-level flags that aren't editor behavior, a theme, or a project setting (#837).
Managed by Quill itself — you won't normally edit this by hand.

| Key | Default | Valid values | What it does |
| --- | --- | --- | --- |
| `onboarding_shown` | `false` | `true` / `false` | Whether the first-run quick-start card has been shown and dismissed. Set to `true` the moment it's dismissed, so it never reappears; per-install, so a new project doesn't re-trigger onboarding. |

---

## A worked example

Turn on idle autosave every 30 seconds, list your books under a projects home,
and switch to the light theme — all by hand-editing
`~/.config/quill/config.toml`:

```toml
[editor]
autosave_interval_secs = 30

[projects]
home = "~/Writing"

[theme]
name = "light"
```

The same three changes can be made without touching the file: open the Settings
menu (`settings` in the Ctrl-K menu), pick **Autosave interval** and **Theme**,
and set `[projects] home` by hand (the projects home has no in-app editor).

---

## On-disk format stability (across 1.x)

Quill's on-disk formats — the `book.toml` project manifest, the home
`config.toml`, and each chapter's YAML front matter — carry a compatibility
promise: **a project written by any 1.x version opens in every other 1.x
version.** A newer version can add fields without breaking an older one, and an
older version won't corrupt a project a newer version has touched.

**Unknown keys never fail a load.** Every config section and the manifest ignore
keys they don't recognize, so a field a *newer* Quill added is silently skipped by
an *older* one rather than erroring. (No struct uses `deny_unknown_fields`; this
is a tested guarantee, not an accident.)

**What happens to an unknown key on re-save** depends on the file:

| Format | Unknown keys on re-save | Why |
| --- | --- | --- |
| Chapter front matter | **Preserved** verbatim | The `---` block is kept byte-for-byte and edited surgically — one key at a time — so an unknown key and your hand formatting survive untouched. |
| `book.toml` manifest | **Dropped** | The manifest is a typed structure re-serialized whole on save, so a key this version doesn't model is not written back. |
| `config.toml` | **Dropped** on rewrite | Config is typed like the manifest; Quill rarely rewrites the home config, but when it does an unmodeled key is not carried. |

The practical consequence of the *drop* rows: if a newer Quill adds a manifest or
config field and you then open and **save** that project in an older Quill, the
newer field is lost (its default takes over). Reading alone never loses anything;
only a save from the older version does. Front matter has no such caveat — its
unknown keys always round-trip.

**A version + migration escape valve.** For the rare change that genuinely *can't*
be additive, `book.toml` may carry a `format_version` scalar. It's absent on every
existing project (which reads as the current version, and stays byte-stable — Quill
won't add the key to a project that never had it). A future breaking format change
bumps the version and adds a migration step that upgrades an older file forward on
open, so the format can evolve without a hard break.
