# Quill

A terminal editor optimized for writing novels.

Quill keeps you in the prose. Instead of a full IDE, it gives you a centered,
fixed-width column on a quiet page, a live word count, and a small set of
keystrokes that stay out of the way.

## Features

- **Distraction-free page** — text is centered in a column of a fixed width
  (like a printed page) rather than running edge to edge. Two optional modes go
  further: **typewriter scrolling** pins the line you're writing to the centre of
  the page (the text scrolls under it, instead of the cursor drifting to the
  bottom), and **focus mode** dims every paragraph but the one under the cursor.
  Both default off; toggle them from the command palette (Ctrl-K) or set them in
  `[editor]` config.
- **Rope-backed buffer** — fast inserts and deletes anywhere in a long
  manuscript, via [`ropey`](https://crates.io/crates/ropey).
- **Unicode-correct cursoring** — movement and deletion work on grapheme
  clusters and account for wide characters.
- **Live stats** — word count and line position in the status bar.
- **Markdown-aware editing** — incremental `tree-sitter` parse drives live
  syntax highlighting, soft-rendered emphasis (markers hidden until you edit a
  span), and an in-chapter outline (Ctrl-O). A scene break shows as a centered
  typeset divider (revealed to its raw marks only with the cursor on the line),
  with the paragraph after a break or a code block set flush-left.
- **Preformatted blocks** — fence a block in ` ``` ` to set it apart verbatim:
  a log readout, transmission, letter, or any text whose line breaks and spacing
  should be preserved. It renders monospaced and unstyled in the editor and
  carries through to every export. (Smart typography still applies — these blocks
  hold prose, not source code.)
- **Spell-checking** — misspelled words are underlined as you write (a bundled
  en_US dictionary, checked off the keystroke path so typing stays instant);
  accept invented names into a per-project dictionary, sweep the flags from
  the command palette or with `]s` / `[s` in Vim mode, and pick an offline
  correction from a suggestions list with `z=` / `Alt-=`. See
  [Spell-checking](#spell-checking).
- **Project as a folder of chapters** — a `book.toml` manifest plus one
  Markdown file per chapter; create, rename, reorder, and delete chapters from
  the chapter browser (Ctrl-G) without touching the filesystem.
- **Two editing views** — chapter-at-a-time (default) or a continuous
  manuscript (Ctrl-L) that flows across chapter boundaries and writes edits back
  to the right files, while never holding more than ~3 chapters in memory. As you
  flow through it, faint part and front/back-matter section headers mark each
  structural transition (render-only, never in the buffer), so you always know
  which part you're in.
- **Corkboard** — an index-card overview of a chapter's scenes (one card per
  heading), run from the command palette (Ctrl-K). Navigate the cards, jump the
  cursor to a scene, reorder scenes (heading and prose move together), cycle a
  scene's status, and write a one-line synopsis — all kept in front matter, out
  of the prose. A manuscript-wide corkboard spans every chapter of the book.
- **Split panes** — keep a reference beside the prose with Vim-style windows
  (Ctrl-W): split the editor, open another chapter / a story-bible entry / a
  docked report in a second pane, switch focus, resize, and close. Panes collapse
  gracefully on a narrow terminal. Full details in [`docs/windows.md`](docs/windows.md).
- **Rolled-up word counts** — live per-chapter counts and a project total in the
  chapter pane.
- **Inline annotations** — pin a margin-style note to a span of text ("check
  this date"); it's marked while editing, stays on its phrase as you revise, and
  never appears in a final export. Each note is a thread you can reply to and
  resolve, and a marked-up review-copy `.docx` round-trips: export it, and an
  editor's returned Word comments import back as annotations on the right spans.
  Stored in a side file, listed and navigable.
- **Chapter metadata** — set each chapter's status (draft / revised / final),
  a one-line summary, and POV character; stored as YAML front matter in the file
  (portable, and your other keys are preserved), with status shown in the pane.
- **Book & series metadata** — title, subtitle, author, and series, stored in
  the `book.toml` manifest and shown as a header above the chapter list.
- **Publishing metadata & copyright page** — ISBN, publisher, publication date,
  language (BCP-47), and a rights/copyright line, set via "Book details" and
  stored in `book.toml`. They flow into the EPUB OPF (`dc:identifier` from the
  ISBN, `dc:publisher`/`dc:date`/`dc:rights`, and `dc:language`/`xml:lang` from
  the language), and a copyright page renders as front matter in EPUB, PDF, and
  Word exports.
- **Multi-book series & tree navigation** — a project can hold a whole series
  (a `[[books]]` array, each book with its own parts and chapters) or a single
  standalone novel, unchanged. **Switch book** (Ctrl-K) moves the editor, pane,
  and reports between books; the **Project tree** navigator (Ctrl-K) walks the
  series → book → part → chapter tree and jumps anywhere; word counts roll up at
  every level and the progress report leads with a series total.
- **Progress report** — set writing goals (target words, chapters, start date)
  in the manifest and press Ctrl-R for a one-screen report: completion against
  goals, a projected final length from your chapter pace, writing rate, and a
  projected finish date. Includes daily momentum — today's word delta (added vs.
  deleted), broken down **per part** (a configurable grouping of chapters),
  backed by an append-only snapshot log so the day's progress survives a reopen.
  Set a **daily word-count target** (`daily` under `[goals]`) and the report
  shows today's net words against it — met, or how many remain. Set a **deadline
  date** and the report projects your finish date and the daily pace it implies.
  Full spec in
  [`docs/progress-reports.md`](docs/progress-reports.md).
- **Writing analytics** — a **month calendar heatmap** of daily activity plus
  **per-session records** (words, elapsed time, words-per-minute), alongside the
  writing-streak history, all read from Quill's append-only day/session log.
- **Named milestones** — mark a point in the project's history by name, then
  **restore the whole project** to it or **diff** the current manuscript against
  it — a coarse, whole-project checkpoint above the per-chapter save/recovery.
- **Writing sprints** — start a timed session (**Start a writing sprint** in the
  Ctrl-K menu; pick a length in minutes) and the status bar shows a live countdown
  and the words written since you began — the word-war / Pomodoro ritual a lot of
  writers build momentum with. When the time's up (or you **Stop the writing
  sprint**), Quill reports the words written, the time, and your words-per-minute.
  It's non-modal — the timer lives in the status bar, typing is never interrupted —
  and the count uses the same word accounting as the daily momentum line.
- **Epigraphs** — a quotation (single- or multi-line / verse) and attribution on
  a chapter (front matter) or the book (manifest), rendered set-apart above the
  body (the book's at the start of the manuscript). The quotation is edited in
  the main editor (Ctrl-S to save, Esc to cancel) and stored readably — a YAML
  `|` block in the chapter file, a TOML `"""` block in `book.toml`.
- **Book sections** — mark a file as front matter (dedication/foreword/preface/
  introduction/prologue) or back matter (epilogue/afterword/acknowledgments/
  appendix/glossary/also-by/about-the-author/colophon) with **S** in the chapter
  browser; sections sort around the body, are grouped in the pane, and are kept
  out of the body word-count goal.
- **Export** — **Ctrl-E** opens an export menu. *Markdown* compiles the whole
  manuscript to a single `.md` file under `export/` (chapters in reading order,
  parts and titles as headings, epigraphs set apart), ready to hand off or
  convert. *Word* writes a `.docx` in standard manuscript format (title block,
  each chapter on a fresh page, double-spaced Times New Roman, `*`/`**` emphasis
  as italic/bold). *Scrivener* writes a `.scriv` project bundle — a "Manuscript"
  binder with front matter and chapters as documents, parts as folders, and each
  chapter's prose as RTF (emphasis carried through) — that opens in Scrivener.
  *Vellum* writes a `.docx` tuned for [Vellum](https://vellum.pub): chapter and
  part titles are styled **Heading 1**, which is where Vellum splits chapters on
  import (in Vellum: *File → Import Word File…* and pick the `.vellum.docx`), so
  chapters arrive as separate, correctly-ordered elements with emphasis intact.
  *EPUB* writes a finished `.epub` ebook — the standard distribution format for
  Apple Books, Kobo, and Kindle — with a navigable table of contents built from
  the part/chapter structure, book metadata (title, subtitle, author, series),
  epigraphs set apart, and `*`/`**` emphasis as italic/bold; no second paid tool
  or Mac required, and it validates against [epubcheck](https://www.w3.org/publishing/epubcheck/).
  *PDF* writes a paginated `.pdf` — the universal fixed-layout proof for beta
  readers, submissions, or a print interior — with a title page, each chapter on
  a fresh page, page numbers, a navigable bookmark outline of the part/chapter
  structure, epigraphs set apart, and `*`/`**` emphasis as italic/bold. It embeds
  its fonts (Liberation Serif as the body, a DejaVu Sans fallback, and DejaVu Sans
  Mono for fenced preformatted blocks) and renders the full Unicode they cover —
  Latin, Greek, Cyrillic, smart punctuation, and decorative ornaments all show. Chinese and other CJK need an author-supplied CJK
  font (`[style].body_font`); glyphs no embedded font has are reported and skipped.
  Each font is subset to just the glyphs the book uses, so files stay small even
  with a large Unicode or CJK source font.
  *PDF (print interior)* writes a print-on-demand book block from the same
  content: a chosen trim size, mirrored inside/outside margins with a binding
  gutter, running heads (book title on the left page, author on the right), page
  numbers at the outer corner, chapters starting on a right-hand (recto) page,
  and the body fonts **embedded** — what services like KDP and IngramSpark
  require. Set the trim from the **Set export style…** menu (below) or in a
  `[print]` table in `book.toml` (`trim = "6x9"`; also `5x8`, `5.5x8.5`; default
  `6x9`). Current limitations: those three trim
  sizes only, no bleed, and no spine artwork (the cover is a full-page page 0, not
  a wrap-around spread); Latin-1 text as above. The default 6×9 interior (0.875″ inside, 0.5″ outside,
  0.75″ top/bottom margins) clears KDP's published minimums; validate the file in
  your service's preview before ordering a proof.
  For a multi-book **series**, the menu offers each format twice — the whole
  series (books in reading order, each delimited per format: Markdown/Word book
  headings, Scrivener book folders, EPUB book sections in the table of contents,
  PDF book divider pages) or just the active book. The designed exports — EPUB and
  PDF — can be lightly styled (opt-in) via a
  `[style]` table in `book.toml`: `scene_break` chooses the divider glyph between
  scenes (e.g. `scene_break = "* * *"`), and `body_font` / `heading_font` point at
  font files (paths relative to the project, `.ttf`/`.otf`/`.woff`/`.woff2`) that
  are embedded into the export and applied (only embed fonts you're licensed to
  distribute). You don't have to find and license those files yourself: the
  run-action menu (`Ctrl-k`) has **Browse & download fonts…** (`fonts:browse`),
  which lists a curated set of open-licensed (SIL OFL) families — name, style, and
  a sample — and, when you pick one, downloads it and sets it as your `body_font`,
  `heading_font`, or both, with no manual path editing. Downloads land in the
  project's `fonts/` directory as `fonts/<family>.ttf`, with the family's license
  saved beside it as `fonts/<family>.LICENSE.txt` so the license travels with the
  font; re-applying a font you've already downloaded is instant and offline.
  Network access happens only on that explicit download — everything else stays
  offline — and the chosen font embeds in the EPUB and print PDF exactly as a
  hand-specified one does. Bringing your own font still works: just point
  `body_font` / `heading_font` at a file path as before. `heading_align`
  (`left`/`center`/`right`/`justify`) sets the
  chapter-heading alignment and `drop_cap = true` opens each chapter's first
  paragraph with a drop cap; `title_ornament` / `part_ornament` place a decorative
  glyph on the title page and each part divider. For artwork instead of glyphs,
  `scene_break_image` / `chapter_heading_image` / `title_image` point at image
  files (paths relative to the project, `.png`/`.jpg`/`.gif`/`.svg`/`.webp`)
  embedded into the EPUB (the scene-break image replaces its glyph). `cover_image`
  sets the **book cover** — every storefront requires one: it embeds in the EPUB as
  the `cover-image` item (the first spine page, plus the EPUB 2 `<meta name="cover">`
  legacy hint older readers use) and renders as a full-page page 0 in the print PDF
  (raster formats only there — an SVG cover is reported and skipped). For
  accessibility (the EU Accessibility Act, Apple/Kobo), give each image a
  description via its `*_alt` key — `cover_image_alt`, `scene_break_image_alt`,
  `chapter_heading_image_alt`, `title_image_alt` — and the EPUB emits it as the
  image's `alt` text. A described image reads as content; an image left without
  an `*_alt` is declared *decorative* (empty `alt` + `role="presentation"`),
  which is the right convention for an ornamental divider. The cover always
  carries a description — `cover_image_alt`, or the book title as a fallback,
  never a bare or generic alt. To start
  from a coordinated look instead of setting each key, `preset` selects a
  built-in bundle of these settings, and any explicit `[style]` key you also set
  overrides just that part of the preset. The library:
  - `classic` — `* * *` scene breaks, drop caps, `❦` title/part ornaments
  - `modern` — left-aligned headings, `•` scene breaks
  - `literary` — `⁂` (asterism) scene breaks and drop caps, no ornaments
  - `romance` — `❀` scene breaks, drop caps, `❦` ornaments
  - `thriller` — left-aligned headings, em-dash (`—`) scene breaks, spare
  - `fantasy` — `❖` scene breaks, drop caps, `⚜` (fleur-de-lis) ornaments
  - `minimalist` — left-aligned headings, a single centered mid-dot (`·`)

  You don't have to hand-edit `book.toml` to choose any of this. The export menu
  (`Ctrl-e`) leads with **Set export style…** (also the `style:set` run-action),
  a menu of the common knobs — each showing its current value — where you pick a
  preset, set the scene-break glyph, heading alignment, drop caps, the
  title/part ornaments, and the cover-image path, and choose the print trim size
  from its allowed set. Each
  choice is spliced straight into the `[style]` / `[print]` tables, leaving the
  rest of the manifest (other sections, comments, formatting) byte-for-byte, and
  takes effect on the next export and in the preview with no restart. Hand-editing
  the manifest still works and round-trips; setting the style from the app is for a
  standalone novel (a multi-book series sets `[style]` / `[print]` per book by
  hand). With no `[style]`, the output is exactly the plain default. Both designed formats
  read `[style]`; the plain formats (Markdown, Word, Scrivener, Vellum) ignore it.
  The PDF honors the fonts, scene-break glyph (including decorative ornaments like
  `❦`/`⚜`, via the fallback font), heading alignment, drop caps, presets, the
  full-page cover, and raster
  image ornaments (PNG/JPEG/GIF/WebP, transparency included). One PDF-only gap is
  reported when configured: `.svg` image ornaments (the PDF takes raster only); a
  glyph no embedded font covers is skipped (a scene break falls back to `#`). To see a look before committing to a full export, the export
  menu (`Ctrl-e`) opens with **Preview style…**, which renders a fixed sample book
  (title page, a chapter opening, a scene break, a part divider) through your
  current `[style]` and opens it in your system EPUB reader — the same output a
  real export produces, so fonts, images, drop caps, and ornaments all show. It
  writes only a temp file and never touches your manuscript.

- **AI assistance (opt-in)** — off by default; a draft only leaves the machine
  when you enable AI and supply a key. Turning it on is **in-app** (#602): press
  `Ctrl-A` or reach for any AI action and Quill offers a `y`/`N` prompt to enable
  it — confirming writes `[ai] enabled = true` and chains straight into setting
  your key (or run **Enable AI assistance** from `Ctrl-K`); declining leaves AI
  off. Your **API key is kept in the OS keychain,
  not in plaintext** (#530): run **Set Claude API key** / **Set image API key**
  from the `Ctrl-K` menu and Quill stores it in the macOS Keychain / Linux Secret
  Service / Windows Credential Manager, so a synced dotfile or backup of your
  config never carries the key. `ANTHROPIC_API_KEY` / `FAL_KEY` still override
  (the headless / CI path), and a legacy plaintext `api_key` still works with an
  offer to migrate it into the keychain. Quill's AI is an **assistant, not a
  co-writer** — it tracks, outlines, marks, observes, and looks up, but never
  drafts or rewrites your prose. Talking to the Claude API (prompt caching, the
  Batch API, structured outputs), it keeps a human-readable **story bible** in
  the project and offers: character profiles & arcs; a timeline with continuity
  findings; a setting / worldbuilding bible with rule-break findings; a
  **plot-thread / arc tracker** that surfaces dangling threads; a **theme /
  motif tracker** (a fifth incremental bible) that follows a book's themes and
  where they recur; outline /
  beat-sheet scaffolding (with `xxx` gap markers); name & term consistency; a
  **name generator**; prose & style and editorial reads (findings, never fixes);
  an in-context **thesaurus** (candidate words for a slot you pick); notes import
  into the bible; grounded "ask the manuscript" Q&A with jumpable citations,
  retrieved through an **on-device semantic index** so the right chapters are
  cited without a cloud round-trip; and
  off-manuscript artifacts (synopsis, blurb, query letter, marketing / social
  copy). Findings surface in a **manuscript-health dashboard** — a one-screen
  overview grouped by kind (continuity, consistency, style, …); you can **export
  the findings list** to a file and **re-check every open finding** in one pass.
  A finding corroborated across several passes is marked as **consensus**, and
  **dismissing** one persists against its anchored span, so it stays dismissed
  when you re-run. The same on-device semantic index powers analysis beyond Ask:
  **find similar scenes** by meaning (jump to the scenes closest to a passage or
  query), **echo / near-duplicate detection** (passages that read too much like
  each other, surfaced as findings), and a **motif & character co-occurrence
  report** (which motifs and characters recur, and where they appear together).
  The incremental continuity and setting passes also pull their context from that
  index — the passages that actually bear on the check — rather than a fixed
  window. An author-written **research notebook** in the project can be cited by
  the bibles, so a tracked fact traces back to a source you control. Run several
  passes as a **durable, resumable queue** with per-pass progress that survives a
  restart, and — opt-in — kick off a **background bible update** that rebuilds
  the bibles from the full manuscript through Claude's asynchronous Message
  Batches API (about 50% cheaper), returning immediately and reconciling the
  finished result on a later launch or idle tick (`[ai] background-batch-update`,
  off by default).
  Your hand-edits to
  the bible are ground truth on the next pass — **Browse story bible**
  (`bible:browse`, from the `Ctrl-K` menu) lists the files under `bible/`
  (characters, timeline, setting, outline) and opens the selected one in an
  editable pane that saves back to disk, so you can read and correct the AI's
  tracking without leaving Quill. Findings collect in a
  navigable **revision triage** list, and for a multi-book series the bible and
  its findings span every book. It runs against Claude or an on-device local
  model: **switch backends from the `Ctrl-K` menu** for the session, or set a
  **routing policy** (`[ai] routing`) that picks per action — keeping cheap,
  frequent work on-device and reserving the paid cloud model for the passes whose
  size needs it, surfacing the choice (cloud $ vs. free local) when it matters.
  With several **Qwen3** models available (choose them in the **Local AI models**
  manager, `Ctrl-K`, or via `[ai] local_models`), the router also picks the
  right-sized one per request — a small model for a quick lookup, a larger one
  for a heavy pass — and the manager is memory-aware, so it only offers models
  that fit the machine. Full design, model choices, and privacy model in
  [`docs/ai-features.md`](docs/ai-features.md).

- **Assistant pane** — open a conversational assistant docked beside the prose
  (**Ctrl-W a**, or from the **Ctrl-K** menu) and ask in plain language: "save and export to Markdown",
  "open the progress report", "now check this for continuity", "ask the
  manuscript who the villain is". It carries out the request by running the
  editor's own actions — the analysis passes (continuity, characters, setting,
  style, naming, scaffold, voice, editorial) whose findings open in the editor,
  and the ones that take an argument (a question to ask, a premise to brainstorm,
  a concept to draft a bible from) — and shows what it ran. The
  conversation persists across turns so a follow-up has the earlier context. It
  runs off the editor thread — typing never freezes — and **Esc** cancels a run in
  flight; **Ctrl-W c** closes the pane. It's cost-aware: the pane shows the run's
  running spend, a costly pass asks for a quick **y / skip** confirmation, and a
  multi-step run that would pass your threshold asks before continuing rather than
  spending silently. It works on **either backend** — switch to the
  local model (or pin `offline`) and the assistant runs fully on-device. Like the
  rest of Quill's AI it's an assistant, not a co-writer (it won't draft or reword
  your prose — it has no tool that can), and with AI off the pane explains that
  rather than failing silently.

- **Run action** — **Ctrl-K** opens one menu listing every invokable action —
  save, each export, the progress and analysis reports, and the AI passes — and
  runs the one you pick through the same path its own shortcut uses. The menu is
  decluttered: actions that simply don't apply in the current context are hidden
  rather than listed as unavailable, and it **remembers group collapse state**
  between sessions (a group you collapse stays collapsed, and reopens collapsed).
  It's the single seam author workflows build on to chain actions by name.

- **Workflows** — chain the actions you run together into one named routine.
  Define them in `config.toml` and run one with **Ctrl-X**:

  ```toml
  [workflows]
  finish = ["save", "report:progress", "export:docx"]
  revise = ["save", "ai:continuity", "ai:style", "triage"]
  ```

  Each step is an action name (the tokens shown in the Ctrl-K menu, e.g. `save`,
  `export:docx`, `report:progress`, `triage`, `ai:continuity`) or the name of
  another workflow, expanded in place. Steps run in order; the run stops at the
  first step that can't run and tells you which and why (the earlier steps'
  effects stand). An unknown step or a workflow that would loop back on itself is
  reported, not run.

  AI passes can be steps too. They run off the editor thread, one at a time, so a
  workflow can refresh a bible and flag continuity without freezing the editor;
  the status bar shows the current step and **Esc** cancels the run (cleanly,
  after the step in flight). AI steps respect your `[ai]` settings: with AI off
  they're skipped and noted rather than failing the run, and a step over the cost
  threshold asks to confirm first (declining cancels the run). The interactive
  "ask the manuscript" action needs a typed question, so it can't be a step.

  Beyond running one by hand (Ctrl-X), a workflow can fire from a **trigger**:

  ```toml
  [triggers]
  on_save = "autosave"      # run this workflow after every save

  [triggers.keys]           # Vim Normal-mode key → workflow
  Q = "revise"
  ```

  `on_save` runs a workflow after each explicit save (Ctrl-S, or `:w` in Vim
  mode); the save isn't blocked (async steps continue in the background), and a
  workflow's own `save` step won't re-trigger it. `[triggers.keys]` binds a Vim
  Normal-mode key to a workflow (Vim mode, #27); a bound key overrides its
  built-in Normal command and any remap of that key, your choice. A trigger that
  fires while a workflow is already running is ignored, and a trigger naming an
  unknown workflow (or a bad key) is reported at startup, not silently kept.

  Don't want to learn the tokens? Pick **✎ New workflow from a description…** in
  the Ctrl-X menu and describe the routine in plain words ("save, refresh the
  character bible, flag continuity, then export to Word"). The AI (when enabled)
  drafts an ordered workflow from the available actions, tells you about anything
  it couldn't map, and shows the proposal for you to name and confirm — nothing is
  written to `config.toml` until you do, and the rest of your config is left as-is.

- **Snippets** — fixed text you type often (a scene break, a tricky proper
  name, a stock phrase) defined once in `config.toml` and inserted verbatim:

  ```toml
  [snippets]
  scene = "* * *"
  heroine = "Aelindra Voss"
  ```

  Each entry maps a name to its literal expansion. Insert one from **Ctrl-K →
  Insert snippet…**, which opens a picker of your snippets by name; Enter drops
  the exact text at the cursor (no transformation — smart typography is bypassed)
  as a single undo. It works in both the chapter and continuous views. This is
  pure mechanical convenience: the author defines the text, Quill only inserts
  it. An empty or absent `[snippets]` table changes nothing; a non-string entry
  is reported at startup and skipped, like the rest of the config.

  A snippet may include a single caret marker `$0` to set where the cursor
  lands after insertion — the marker is consumed, never inserted:

  ```toml
  [snippets]
  quote = "“$0”"          # cursor drops between the quotes
  letter = "Dear $0,\n\nSincerely,\n"
  ```

  Only the first `$0` is the caret (any later one inserts literally); a snippet
  with no marker leaves the cursor at the end, as before. To insert a literal
  `$0`, escape it as `$$0`.

- **Configure by description** — the same idea for *settings*: in the `Ctrl-K`
  command palette, run **Configure by description** and say what you want — "turn
  on Vim mode," "narrow the text column," "give me a warm dark theme," or "add
  Aerendil and Vashti to the dictionary so they stop getting flagged." The AI
  (when enabled) maps it onto concrete changes — `[editor]` options (Vim,
  smart typography, column width), `[theme]` colors, and words for the per-project
  spell-check dictionary — and shows the exact changes for you to confirm.
  **Nothing is written until you confirm** (declining changes nothing); on
  confirm, settings are written to `config.toml` (the rest of the file left
  intact) and dictionary words to the project's `dictionary.txt`, all applied
  immediately. Anything that doesn't map to a real setting is reported back, not
  invented, and the AI only ever touches configuration — never your manuscript.

## Install

The quickest way to get a `quill` command on your `PATH`:

```sh
./install.sh
```

It builds a release binary with Cargo and installs it to `~/.local/bin` (no
`sudo`, nothing in system paths), then tells you if that directory isn't on your
`PATH` yet. Re-run it to upgrade in place. Override the location with
`BIN_DIR=/usr/local/bin ./install.sh` (or `PREFIX=…`).

Quill has optional Cargo features (currently `local`, the on-device AI backend).
Run with no arguments and the script lists them with descriptions and lets you
pick; or choose up front and skip the prompt with `./install.sh --features local`
(or `--features ""` for none). A piped/non-interactive run builds with no
optional features unless you pass them.

Building requires a Rust toolchain ([rustup](https://rustup.rs)) — Quill is a
from-source tool, and the script wraps the Cargo build rather than replacing it.

## Build & run

To run from the checkout without installing, or to develop:

```sh
cargo run --release -- path/to/book
```

Quill works on a **project directory**. Pass a path and, if the directory has a
`book.toml` manifest it's opened; otherwise a new project is initialized there (a
manifest plus a first chapter).

With **no argument**, Quill doesn't scatter a project into the current folder. If
you launch from inside an existing project it opens that one. Otherwise — when you
have books to return to — it offers a **chooser**: create a new project, or open
an existing one. *Open existing* lists your recently-opened projects together with
every project under your **projects home** (`[projects] home`, see
[Projects](#projects-switch-without-relaunching)); pick one and it opens. *Create
new* asks what to call the book and files it in your projects home — so `quill`
from anywhere starts a tidily-filed book rather than one wherever the shell
happens to be. With no projects home configured it falls back to a `Quill` folder
in your home directory and tells you so (set `[projects] home` to choose where
books live).

When you have nothing to open yet — no recent projects and no projects home — the
chooser would be empty, so Quill skips it and goes straight to naming a new book.

If you always return to the same book, set `open_last = true` under `[projects]`
and a bare `quill` reopens your most-recently-opened project directly, skipping
the chooser. It applies only to a no-argument launch from outside a project; an
explicit path and in-project launches are unaffected, and with no recent project
to reopen it falls back to the usual chooser (or new-book prompt).

## Project & file format

A project is a directory: a root `book.toml` manifest records the chapter order
and titles, and each chapter's prose lives in its own UTF-8 **Markdown** file.
Both are portable plain text that opens in any editor, so a manuscript stays
yours and travels well in git.

A project usually holds one book — the manifest's `title`, optional `[[parts]]`,
and `[[chapters]]` describe it directly. A project can also hold a **series of
books**: the manifest grows a `[[books]]` array, each book carrying its own
`title`, `[[parts]]`, and `[[chapters]]`, with `title`/`author` at the top level
naming the series. A standalone novel's `book.toml` has no `books` table and is
read and written exactly as before — adding a series never changes a single-book
file. When a project does hold several books, Quill opens the first one; switch
between them with the **Switch book** command, or walk the whole series →
book → part → chapter tree with the **Project tree** navigator (both in the
run-action menu, `Ctrl-K`).

Quill writes each chapter back unchanged except for one normalization: a
non-empty file ends with exactly one trailing newline (the POSIX/git
convention), and an empty file stays empty. Interior bytes, including line
endings, are left exactly as you wrote them, so a conforming file round-trips
byte-for-byte. (One opt-in exception: with **smart typography** on — the
default — the punctuation you *type* is substituted at the keystroke, so curly
quotes, em-dashes, and ellipses are what land in the file. A chapter Quill
isn't editing is never touched; turn the setting off for plain-ASCII input. See
[Smart typography](#smart-typography).)

## Keys

| Key            | Action                          |
| -------------- | ------------------------------- |
| *(typing)*     | insert text                     |
| Arrows         | move cursor                     |
| Mouse          | click to position the cursor; drag to select (a Visual selection); scroll-wheel to scroll; click a row in a focused list overlay to select it |
| Paste          | terminal paste (Ctrl-Shift-V / Cmd-V / middle-click) inserts the system clipboard at the cursor as one undoable edit |
| Home / End     | start / end of line             |
| F1             | open the built-in user's guide  |
| Ctrl-Z / Ctrl-Y| undo / redo                     |
| Ctrl-F         | find (then `r` to replace)      |
| Ctrl-S         | save                            |
| Ctrl-Q / Ctrl-C| quit (prompts if unsaved)       |
| Ctrl-O         | toggle the chapter / outline pane (press again to focus it, then Enter jumps to the selected heading) |
| Ctrl-R         | progress report (any key closes)|
| Ctrl-T         | analysis report (any key closes)|
| Ctrl-L         | toggle continuous manuscript    |
| Ctrl-A         | AI command palette              |
| Ctrl-E         | export menu: Markdown, Word (`.docx`), Scrivener (`.scriv`), Vellum (`.docx`), EPUB (`.epub`), or PDF (`.pdf`, screen and print-interior) → `export/` |
| Ctrl-K         | run action: one menu for save, export, the reports, and the AI passes (unavailable ones say why) |
| Ctrl-X         | run workflow: pick one of your defined action sequences and run it |
| Ctrl-B         | browse projects: switch to a recently-opened project, or press `b` to browse the filesystem for any project (saves the current first) |
| Ctrl-N / Ctrl-P| next / previous chapter         |
| Ctrl-G         | browse chapters (↑/↓, Enter; `/` filter / quick-jump by title·status·POV; `o` open in a pane, `f` reference files) |
| Ctrl-W then …  | window panes: `s`/`v` split, `a` assistant pane, `w` switch pane, `</>`/`+-` resize, `=` even up, `c`/`q` close (with a report/AI answer open, `s`/`v` docks it beside the editor) — see [`docs/windows.md`](docs/windows.md) |
| Ctrl-+ / Ctrl- | widen / narrow the text column  |

### Projects (switch without relaunching)

**Ctrl-B** opens the project browser so you can move between books without
quitting. It lists your recently-opened projects (newest first); Enter switches
to one in place, saving the current project first (and confirming if there are
unsaved changes). Press **`b`** to browse the filesystem for a project that
isn't in the list yet, or **`n`** to start a brand-new project — type its path
(prefilled with a sensible location) and Quill creates the folder, manifest, and
first chapter, then opens it.

To get a complete, always-current library view, point Quill at the folder where
you keep your books in `~/.config/quill/config.toml`:

```toml
[projects]
home = "~/Writing"
```

Every project under that folder then appears in the browser, ready to open —
no need to have opened it before. Without it, the browser just shows recents.

Every setting in `config.toml` — this `[projects]` table, `[editor]`, `[theme]`,
`[ai]`, `[image]`, `[snippets]`, and `[workflows]`/`[triggers]` — with its
default, valid values, and an example, is documented in one place:
[`docs/configuration.md`](docs/configuration.md).

Renamed your book or series and want the folder to match? **Rename project
folder** (in the Ctrl-K menu) moves the open project's directory without
quitting: it's prefilled with the book title, so accepting it makes the folder
match the title in one keystroke (or type a different name). The project stays
open and keeps saving to the new location, and the recent-projects list follows.
It refuses if a folder of that name already exists, and leaves everything in
place. Renaming the folder is separate from the book **title** (set with **t** in
the chapter browser) — moving the folder never changes the title, and vice versa.

Already have a folder of notes? **New project from a folder of notes** (in the
Ctrl-K menu) creates a project and imports every Markdown file in the folder into
its story bible in one step — preserving the originals.

Already have a **manuscript**? **Import a manuscript → new project** (in the
Ctrl-K menu) turns an existing **Markdown (`.md`)**, **Word (`.docx`)**,
**OpenDocument Text (`.odt`)**, or
**Scrivener (`.scriv`)** manuscript into a chaptered Quill project: point it at
the file (or the `.scriv` bundle), give the new project a path, and Quill splits
it into chapters and carries the chapter titles into the manifest. A Markdown file
splits on headings — `##` when there's a single `#` title (Quill's own export
shape), or `#` when each chapter is a top-level heading — with the book title /
author from a YAML front-matter block or the title page. A Word file splits on
**Heading 1** paragraphs (the level Quill's own Word/Vellum export uses for
chapter titles), converts bold/italic runs to `**`/`*`, and reads the title /
author from the document's properties or a title-block preamble. A Scrivener
bundle follows its binder's reading order (each text document a chapter, its
binder title the chapter title; folders flatten into their documents) and converts
RTF bold/italic to `**`/`*`, with the title from the bundle name. Emphasis is kept
in every case, and the original is only read from, never moved or changed — the
inverse of Quill's Markdown, Word, and Scrivener *exports*, closing the round
trip.

When you create a project, a short setup wizard offers to scaffold it — writing
goals, a starter cast and setting (with AI, when enabled), and a first note.
Every step is skippable and the whole thing is dismissible (Esc); turn it off
entirely with `setup_wizard = false` under `[projects]`.

On a first run, a dismissible **onboarding / quick-start card** points at the
core keys and the next steps to take, so a new project isn't a blank page with
no signposts.

Want an offline backup — no git, no remote, no account? **Archive project…** (in
the Ctrl-K menu, or `archive`) writes a single timestamped `.zip` of the whole
project: the manifest, every chapter, the story bible, your annotations, and the
custom dictionary. The prompt is prefilled with `<project>-<date-time>.zip` (e.g.
`My-Novel-2026-06-30-142530.zip`) in your projects folder, so a fresh backup is
usually one keystroke — edit the path to save it to a USB stick, a synced folder,
anywhere. The active chapter is saved first so the archive reflects what's on
screen, and editor scratch is left out (the `.quill/` recovery state, the
`unused/` folder of removed chapters, `export/`, and OS cruft), mirroring what
git ignores. To bring one back, **Restore project from archive…** (or
`archive:restore`) unpacks a `.zip` into a new folder and opens it — it refuses
to overwrite an existing project, so a stray path can't clobber your work.

For backups you don't have to remember, an opt-in `[backup]` table automates two
kinds. A project under version control that points at a remote (Ctrl-V → **Set
remote**) can push to it on a cadence in the background — an always-current
off-site copy without a manual push, with the status bar reporting the last
success or a failure to retry. Independently, Quill can keep **scheduled local
archives** — the same `.zip`, written on a cadence to
`~/.local/share/quill/backups/<project>/`, keeping the N most recent and pruning
the rest, and warning you at open if the newest copy has gone stale. Both are off
by default; the keys (`enabled`, `interval_mins`, `local_enabled`,
`local_interval_hours`, `local_keep`, `stale_warn_hours`) are in
[`docs/configuration.md`](docs/configuration.md).

Those local archives are browsable and restorable **in-app**: **Browse backups…**
(in the Ctrl-K menu, or `backup:browse`) lists the archives Quill has kept for the
project, and restoring one is a keystroke away. When you open a project and a
newer local backup exists on disk, Quill offers to restore it, so a recovered copy
is never more than a confirmation away.

### Smart typography

A finished manuscript wants typographic punctuation, so Quill produces it as you
type: straight quotes become correctly-oriented curly quotes (`“…”` `‘…’`, the
direction chosen from what precedes them, so contraction apostrophes come out
right too), `--` becomes an em-dash (`—`), and `...` becomes an ellipsis (`…`).
This is mechanical punctuation, not prose — Quill orients the marks you're
already typing, it never words anything.

It's **on by default**. The substitution happens at the keystroke, so the curly
characters are what's saved to disk and what every export (Word, Scrivener,
Vellum, Markdown) carries through. Only the punctuation you actually type is
changed; a chapter Quill isn't editing is never rewritten. Prefer plain ASCII
(and an exact byte-for-byte round-trip of what you type)? Turn it off in
`~/.config/quill/config.toml`:

```toml
[editor]
smart_typography = false
```

### Typewriter scrolling and focus mode

Two optional aids for staying in the current line. **Typewriter scrolling** keeps
the cursor's line pinned to the vertical centre of the page — the text scrolls
under it as you write, instead of the cursor walking to the bottom as a chapter
grows. **Focus mode** dims every paragraph but the one under the cursor (the
paragraph is the run of non-blank lines around it), so the passage you're shaping
stands out. Both default off.

Toggle either one at any time from the command palette (Ctrl-K → "Typewriter
scrolling" / "Focus mode"), or set your resting preference in
`~/.config/quill/config.toml`:

```toml
[editor]
typewriter = true
focus_mode = true
```

### Autosave (opt-in)

The status bar carries a save-state marker beside the chapter name: **✓** when
the buffer matches what's on disk, **●** when you have unsaved changes. So you
can always tell at a glance whether your work is persisted.

Autosave keeps that marker green for you. Turn it on by giving it an idle
interval, in seconds, in `~/.config/quill/config.toml`:

```toml
[editor]
autosave_interval_secs = 30
```

With it set, Quill saves the active chapter after that many seconds with no
input — never mid-keystroke, only once you pause. It goes through the same
atomic, front-matter-preserving path as Ctrl-S, so your cursor, viewport, and
chapter metadata are untouched; it just writes the file and flips the marker
back to ✓. It works in both the single-chapter and continuous views.

One deliberate difference from a manual save: an autosave does **not** fire your
`[triggers] on_save` workflow. A workflow that, say, exports or runs an AI pass
shouldn't churn every thirty seconds in the background — `on_save` stays tied to
explicit saves (Ctrl-S / `:w`). If an autosave can't write (a disk error), the
buffer simply stays dirty (the ● shows it) and the next manual save surfaces the
error.

The default is `0`, which turns autosave off — with it off, the editor behaves
exactly as it always has, saving only when you ask.

### Color theme

Quill ships two built-in themes — `dark` (the default) and `light` — selected in
`~/.config/quill/config.toml`:

```toml
[theme]
name = "light"
```

With no `[theme]` set, Quill uses `dark`, which is its original palette — so an
unconfigured editor looks exactly as it always has. Any individual color can be
overridden on top of the chosen theme; a value is an ANSI color name (`cyan`,
`darkgray`, …), a hex string (`#1e1e24` or `#abc`), an `r,g,b` triple, or a
0–255 palette index:

```toml
[theme]
name = "dark"
heading = "#7aa2f7"   # headings and accents
dim = "gray"          # secondary text, hints, dividers
status_bg = "16,16,20" # the status bar background
match_current = "yellow"
```

The themeable fields are `heading`, `on_accent`, `dim`, `muted`,
`match_current`, `match_other`, `status_bg`, `added`, `removed`, `revised`, and
`mode_insert` / `mode_normal` / `mode_visual`. The theme drives the whole editor
consistently — prose, syntax highlighting, headings, status bar, panes, and
overlays. An unknown theme name, an unknown field, or an unparseable color is
reported in the status line at startup and falls back to the default for that
piece, rather than failing to start.

### Vim mode (opt-in)

Authors with Vim muscle memory can turn on modal editing. It's off by default —
with it off, editing is exactly as described above. Enable it in
`~/.config/quill/config.toml`:

```toml
[editor]
vim = true
```

With it on, the editor rests in **Normal** mode (shown in the status bar): keys
are commands, not text. `i`/`a` (and `I`/`A`/`o`/`O`) enter **Insert** mode to
type, and `Esc` returns to Normal. All the `Ctrl-` shortcuts above keep working
in both modes.

Normal-mode motions:

- **Characters / lines** — `h` `j` `k` `l` (and the arrows); `0` line start,
  `^` first non-blank, `$` line end.
- **Words** — `w` / `b` next / previous word start, `e` word end.
- **Paragraphs** — `}` / `{` jump to the next / previous paragraph (the blank
  line between them).
- **Document** — `gg` first line, `G` last line, `{count}G` (or `{count}gg`) a
  specific line.
- **Find on the line** — `f{char}` / `t{char}` jump to / just before the next
  occurrence, `F` / `T` backward, and `;` / `,` repeat it forward / back.
- **Counts** — a number repeats a motion: `5j`, `3w`, `2fx`.

Normal-mode editing:

- **Operators** — `d` (delete) and `c` (change) compose with any motion
  (`dw`, `d$`, `c3w`, `dG`, `d}`) and with text objects (`diw`/`daw`, `ci"`,
  `di(`, and the prose objects `dip`/`dap` paragraph, `cis`/`das` sentence);
  `dd`/`cc` act on whole lines, `D`/`C` to end of line, with counts (`2dd`).
  `c` leaves you in Insert.
- **Delete a character** — `x` (with a count, `3x`).
- **Yank & put** — `y` with a motion or text object (`yw`, `yi"`), `yy` (or `Y`)
  for the line; `p` puts after the cursor / line, `P` before; counts apply
  (`3p`). Yanks and deletes share a register, so `dd`…`p` moves a line, and
  `"a`–`"z` are named registers (`"ayy`, `"ap`).
- **Undo / redo** — `u` undoes; `Ctrl-y` redoes (sharing Quill's undo history).

Visual mode:

- **`v`** starts a characterwise selection, **`V`** a linewise one; motions
  extend it (it's highlighted as you go), `o` swaps the active end, `Esc`
  cancels.
- **`d`** / **`c`** / **`y`** act on the selection (delete / change / yank), and
  **`p`** replaces it with the register.
- **Drag to select** — pressing and dragging the left mouse button enters Visual
  mode anchored at the press point and extends the selection as you drag
  (correct across the centered margin, soft-wrap, the paragraph indent, and
  fenced-code rows); releasing leaves the selection active. A plain click (no
  drag) still just positions the cursor.

System clipboard:

- A **yank / delete** and a **Visual copy** also place the text on the system
  clipboard, so you can paste it into another app — over SSH and tmux too (Quill
  writes the clipboard with the OSC 52 terminal escape). The internal Vim
  registers keep working unchanged.
- **Paste** with your terminal's paste (Ctrl-Shift-V / Cmd-V / middle-click):
  the system clipboard is inserted at the cursor as one undoable edit (replacing
  a Visual selection, if one is active). In Vim mode, **`"+p`** / **`"*p`** paste
  the system clipboard explicitly.

Search:

- **`/`** searches forward, **`?`** backward — type the query on the bottom line
  (matches highlight as you type), Enter jumps to the match, Esc cancels and
  returns the cursor.
- **`n`** repeats in the search direction, **`N`** the opposite (wrapping at the
  document ends with a notice); **`*`** searches for the word under the cursor.
- **`]s`** / **`[s`** jump to the next / previous misspelled word (the spelling
  sweep), wrapping at the ends.
- **`z=`** opens a pick-list of spelling suggestions for the flagged word at the
  cursor; Enter replaces the word with the one you choose.

Command line:

- **`:`** opens a command line on the bottom row (Enter runs, Esc cancels):
  **`:w`** saves, **`:q`** quits (prompting on unsaved changes), **`:wq`** / **`:x`**
  save then quit, **`:q!`** quits discarding changes.

Vim editing commands run in the single-chapter view; in the continuous view
(Ctrl-L) they're held back, like undo.

#### Remapping keys

Bind a key to an action under `[editor.keys]` (`action = "key"`). Unmapped keys
keep their defaults; an invalid entry (unknown action, or a value that isn't a
single key) is reported at startup and ignored, leaving the default in place.

```toml
[editor.keys]
# Colemak-style home-row motions (the defaults still work too):
left = "n"
down = "e"
up = "i"
right = "o"
```

The action names: `left` `down` `up` `right`; `word_forward` `word_backward`
`word_end`; `line_start` `first_non_blank` `line_end` `paragraph_forward`
`paragraph_backward` `last_line`; `find_forward`
`find_backward` `till_forward` `till_backward` `repeat_find`
`repeat_find_reverse`; `delete` `change` `yank`; `delete_char`
`delete_to_line_end` `change_to_line_end`; `put_after` `put_before` `undo`;
`insert` `append` `insert_line_start` `append_line_end` `open_below` `open_above`;
`visual` `visual_line`; `search_forward` `search_backward` `search_next`
`search_prev` `search_word`; `ex_command`.

In the chapter browser (Ctrl-G): **a** add a chapter, **r** rename, **d** delete
(moved to `unused/`), **J / K** move the selection down / up, **s** cycle status
(draft → revised → final), **m** edit the one-line summary, **v** set the POV
character, **l** set a word-count goal, **S** set a section role (blank = body),
**g** epigraph (quotation, then attribution). Book/series metadata
(shown in the pane header): **t** book title, **b** subtitle, **w** author,
**e** series, **G** book epigraph.

**Type-to-filter / quick-jump** — in a long novel or a series, press **/** to
filter the list as you type: the browser shows only chapters whose title, status,
or POV contains the query (case-insensitive), and the selection snaps to the
first match. **Enter** opens the highlighted chapter — so `/ m e e t Enter` jumps
straight there. The bare-letter commands above would otherwise be typed text, so
filtering lives behind `/`: while typing, **Esc** leaves the query *in place* (the
commands now act on the narrowed list), and a second **Esc** clears the filter
and restores the full list. The filter reads only cached metadata, so it stays
instant even at series scale.

A file can be marked as a non-chapter **section** (#53) with **S**: front matter
(`dedication`, `foreword`, `preface`, `introduction`, `prologue`) or back matter
(`epilogue`, `afterword`, `acknowledgments`, `appendix`, `glossary`, `also-by`,
`about-the-author`, `colophon`); blank clears it back to a body chapter. The role shows as a dim tag in the pane. Front matter sorts before
the body and back matter after it, reordering (J / K) stays within a band, and
the progress report counts only body chapters toward the chapter goal. (Pane
grouping and separate front/back word counts build on this in later work.)

To write with your notes open, the browser opens files in a side pane: **o**
opens the selected chapter beside the editor (compare two chapters), and **f**
switches to the reference-file browser — the project's story bible (`bible/`) and
loose notes, **grouped** so a large set is easy to scan — where **Enter** opens
the chosen file in a side pane. A reference pane is a normal editor: **Ctrl-W w**
focuses it (or click it) to scroll or edit, drag the **divider** to resize it,
**Ctrl-S** saves it to its own file, and **Ctrl-W c** (or Vim `:q`) closes it.

In find (Ctrl-F): type to search the current chapter (case-insensitive,
matches highlighted), **n / N** jump to the next / previous match, **r** start
replacing the query — type the replacement, then **y** replace this match,
**n** skip, **a** replace all, **Esc** finish. Find & replace is scoped to the
active chapter.

**Find across manuscript** (the `Ctrl-K` command palette) is the project-wide
counterpart: it searches *every* chapter and lists the matches grouped by
chapter, each with a line of context. **↑ / ↓** move, **Enter** jumps to the
match in its chapter, **r** starts a replace — type the replacement, then **y**
replace the selected match, **n** skip, or **a** replace across the whole
manuscript (confirmed first, since a cross-file replace isn't a single undo). A
manuscript-wide replace rewrites each affected chapter file on disk, leaving
front matter untouched, and reports how many occurrences changed and in how many
chapters. Chapters are read one at a time, so a long series stays responsive.
The single most common rename — a character or place, everywhere — is one pass.
Project-wide find takes a regular expression, at parity with chapter find, so a
pattern (not just a literal) can sweep the whole manuscript.

### Terminal keyboard support

On terminals that support the **enhanced ("Kitty") keyboard protocol** — kitty,
foot, WezTerm, Ghostty, recent iTerm2, and others — Quill turns it on at startup
(the `DISAMBIGUATE_ESCAPE_CODES` mode) so combinations the legacy encoding can't
tell apart — `Tab` vs `Ctrl-I`, `Enter` vs `Ctrl-M`, and various `Ctrl`/`Shift`
combos — arrive as distinct key events. It's detected automatically and restored
on exit (including after a crash). On a terminal without support, Quill behaves
exactly as before — nothing is enabled and no escape sequences are emitted.

### Spell-checking

Misspelled words are **underlined** as you write, checked against a bundled
English dictionary. It's fully offline — no network, no API key — and runs on
the idle tick, never on the keystroke path, so typing stays instant. Like the
rest of Quill's analysis it only *flags*: it never changes your prose. Numbers
and `xxx` markers aren't flagged. (The dictionary is SCOWL-derived en_US,
bundled under `assets/dictionary/` with its license.)

Fiction is full of invented names, so you can **accept a flagged word into a
per-project dictionary**: put the cursor on the underlined word and run **Add
word to dictionary** from the `Ctrl-K` command palette. It clears immediately and
is never flagged again in this project. Accepted words are stored, one per line,
in a plain-text `dictionary.txt` at the project root — diffable and carried in
git with the manuscript.

To work through them, **Next misspelling** / **Previous misspelling** (in the
`Ctrl-K` palette) jump the cursor between flagged words, wrapping around the ends;
a word you've since fixed or accepted is skipped, and when none are left it says
so. In Vim mode the standard **`]s`** / **`[s`** do the same, so a spelling sweep
is a quick repeated keystroke: jump to a flag, fix it (or accept the name), jump
to the next.

When a flag is a genuine typo, you don't have to retype it: land on the word and
press **`z=`** (Vim mode) or **`Alt-=`** (default mode) to open a pick-list of
spelling **suggestions** drawn from the same bundled dictionary — fully offline,
no network or API key. Arrow keys move, Enter replaces the word with the one you
choose, Esc cancels. Like the thesaurus and the name generator, Quill only offers
the candidates and places the one you pick; it never rewrites your prose.

### Annotations

Revision is a conversation with yourself, and an `xxx` marker is too blunt for it
— it sits in the prose and would export as literal text if you missed it. An
**annotation** is a margin-style note tied to a span of text ("check this date,"
"is she still angry here?"): visible while you edit, never in the output.

To leave one, select the text (Vim Visual mode, then **`m`**) or put the cursor
on a word, then run **Annotate selection** from the `Ctrl-K` palette; type the
note and press Enter. The annotated span gets a tinted background. Notes stay on
their phrase as you edit around them — type before a note and it slides along;
delete the text it's pinned to and the note goes with it.

**Review annotations** (also in the palette) opens the chapter's notes as a list:
**↑↓** to move, **Enter** to jump to one, **`d`** to delete it, **Esc** to close.
An annotation is a thread, not a one-off: **reply** to one to add to the
conversation, and **resolve** it when it's handled — the note stays on record but
drops out of the way, so editorial back-and-forth lives on the annotation itself.
When a manuscript comes back marked up by several reviewers, the list is built
for triage: **sort** the notes, **filter** by reviewer or by status, and **jump
to the next open one**, so a stack of comments is workable at a glance.

Annotations also make the round trip through an editor. Export a **review copy**
(the marked-up `.docx` from the `Ctrl-K` palette) and each annotation becomes a
Word comment anchored to its phrase; when the editor sends it back, **Import
review copy** reads their comments straight back in as inline annotations on the
exact spans they marked — so an editor's markup arrives as live notes, not a
document to reconcile by hand.

Annotations live in an `annotations.toml` side file at the project root, keyed by
chapter — the chapter `.md` files stay clean, so **no final export** (Markdown,
Word, Scrivener, Vellum, EPUB) ever carries them. They work in both the
single-chapter and continuous (`Ctrl-L`) views — in the continuous view a note is
marked and anchored across the chapter window and saved back to the chapter that
owns it.

## Layout

```
src/
  main.rs        terminal setup + event loop
  app/           application state + key handling (mod.rs), unit tests
  mode.rs        editor modes: Insert / Prompt / Nav / EditMeta
  buffer.rs      rope-backed document, cursor, editing & movement
  project.rs     project model: manifest + per-chapter files
  manuscript.rs  continuous view: a window of chapters as one buffer
  syntax.rs      incremental tree-sitter Markdown parse
  ui.rs          rendering: panes, centered page + status line
```

## Roadmap

Quill is a working editor today — everything in **Features** above has shipped,
including the multi-book series model and tree navigation, Vim mode, export to
Word / Scrivener / Vellum, the AI assistance surface and its series-spanning
story bible, and the performance and stability hardening (atomic saves, crash
recovery, foreseeable-error handling, and a property-tested data path, backed by
the benchmarked `cargo perf` suite in [`docs/performance.md`](docs/performance.md)).

The last of the planned AI work has now shipped — the **fully-offline
local-model backend** (an in-process quantized model via
[`candle`](https://github.com/huggingface/candle), Metal on Apple Silicon, so
nothing leaves the machine), the **runtime backend switch**, and **hybrid
routing** that picks local vs cloud per action by cost and capability (see
Features). The local backend is a build-time opt-in (`--features local`); a
standard build is Claude-only. Nothing else is actively scheduled.

The **AI boundary is fixed, not a roadmap item**: Quill's AI is an *assistant,
not a co-writer*. It tracks, outlines, marks, observes, and looks up — it never
drafts, rewrites, or suggests manuscript prose. (An earlier "selection rewrite
assistant" idea was retired on exactly this principle; off-manuscript artifacts
like a synopsis or query letter are the only prose the AI produces.) The full
design and privacy model live in [`docs/ai-features.md`](docs/ai-features.md).

## License

MIT
