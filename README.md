# Quill

A terminal editor optimized for writing novels.

Quill keeps you in the prose. Instead of a full IDE, it gives you a centered,
fixed-width column on a quiet page, a live word count, and a small set of
keystrokes that stay out of the way.

## Features

- **Distraction-free page** — text is centered in a column of a fixed width
  (like a printed page) rather than running edge to edge.
- **Rope-backed buffer** — fast inserts and deletes anywhere in a long
  manuscript, via [`ropey`](https://crates.io/crates/ropey).
- **Unicode-correct cursoring** — movement and deletion work on grapheme
  clusters and account for wide characters.
- **Live stats** — word count and line position in the status bar.
- **Markdown-aware editing** — incremental `tree-sitter` parse drives live
  syntax highlighting, soft-rendered emphasis (markers hidden until you edit a
  span), and an in-chapter outline (Ctrl-O).
- **Project as a folder of chapters** — a `book.toml` manifest plus one
  Markdown file per chapter; create, rename, reorder, and delete chapters from
  the chapter browser (Ctrl-G) without touching the filesystem.
- **Two editing views** — chapter-at-a-time (default) or a continuous
  manuscript (Ctrl-L) that flows across chapter boundaries and writes edits back
  to the right files, while never holding more than ~3 chapters in memory.
- **Rolled-up word counts** — live per-chapter counts and a project total in the
  chapter pane.
- **Chapter metadata** — set each chapter's status (draft / revised / final),
  a one-line summary, and POV character; stored as YAML front matter in the file
  (portable, and your other keys are preserved), with status shown in the pane.
- **Book & series metadata** — title, subtitle, author, and series, stored in
  the `book.toml` manifest and shown as a header above the chapter list.
- **Progress report** — set writing goals (target words, chapters, start date)
  in the manifest and press Ctrl-R for a one-screen report: completion against
  goals, a projected final length from your chapter pace, writing rate, and a
  projected finish date. Includes daily momentum — today's word delta (added vs.
  deleted), broken down **per part** (a configurable grouping of chapters),
  backed by an append-only snapshot log so the day's progress survives a reopen.
  Full spec in [`docs/progress-reports.md`](docs/progress-reports.md).
- **Epigraphs** — a quotation (single- or multi-line / verse) and attribution on
  a chapter (front matter) or the book (manifest), rendered set-apart above the
  body (the book's at the start of the manuscript). The quotation is edited in
  the main editor (Ctrl-S to save, Esc to cancel) and stored readably — a YAML
  `|` block in the chapter file, a TOML `"""` block in `book.toml`.

## Build & run

```sh
cargo run --release -- path/to/book
```

Quill works on a **project directory**. If the directory has a `book.toml`
manifest it's opened; otherwise a new project is initialized there (a manifest
plus a first chapter). With no argument, Quill uses the current directory.

## Project & file format

A project is a directory: a root `book.toml` manifest records the chapter order
and titles, and each chapter's prose lives in its own UTF-8 **Markdown** file.
Both are portable plain text that opens in any editor, so a manuscript stays
yours and travels well in git.

Quill writes each chapter back unchanged except for one normalization: a
non-empty file ends with exactly one trailing newline (the POSIX/git
convention), and an empty file stays empty. Interior bytes, including line
endings, are left exactly as you wrote them, so a conforming file round-trips
byte-for-byte.

## Keys

| Key            | Action                          |
| -------------- | ------------------------------- |
| *(typing)*     | insert text                     |
| Arrows         | move cursor                     |
| Home / End     | start / end of line             |
| Ctrl-S         | save                            |
| Ctrl-Q         | quit (prompts if unsaved)       |
| Ctrl-O         | toggle the chapter pane         |
| Ctrl-R         | progress report (any key closes)|
| Ctrl-L         | toggle continuous manuscript    |
| Ctrl-N / Ctrl-P| next / previous chapter         |
| Ctrl-G         | browse chapters (↑/↓, Enter)    |
| Ctrl-+ / Ctrl- | widen / narrow the text column  |

In the chapter browser (Ctrl-G): **a** add a chapter, **r** rename, **d** delete
(moved to `unused/`), **J / K** move the selection down / up, **s** cycle status
(draft → revised → final), **m** edit the one-line summary, **v** set the POV
character, **g** epigraph (quotation, then attribution). Book/series metadata
(shown in the pane header): **t** book title, **b** subtitle, **w** author,
**e** series, **G** book epigraph.

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

The Markdown manuscript format, the project model, chapter/book metadata, and the
progress report have **shipped** — see the Features section above. The sections
below are forward-looking.

### Project structure: multi-book series & tree navigation

Today a project is a flat, ordered list of chapter files with an optional
`parts` overlay (named, inclusive chapter ranges). Still ahead:

- **Series support** — more than one **book** under a single project, with the
  manifest recording a series → book → part → chapter tree rather than today's
  single-book chapter list.
- **Tree navigation** — an outline/navigation pane that walks the full tree and
  jumps between books, parts, and chapters, with rolled-up word counts at each
  level.

### AI assistance (Claude)

Opt-in, off by default — a draft only leaves the machine when the author enables
it and supplies a key. Talks to the Claude Messages API directly (no official
Rust SDK), defaulting to `claude-opus-4-8`, using structured outputs, prompt
caching (the manuscript is the ideal cache prefix), and the Batch API. Features
read/write a human-readable **story bible** kept in the project.

- **Character tracking** — AI-summarized profiles with per-chapter traits,
  relationships, and arc, plus drift/contradiction flags.
- **Timeline & continuity** — extract and order events; flag contradictions
  (a character in two places, an object used after it's destroyed).
- **Scaffolding** — generate outlines/beat sheets from a premise or partial
  draft and insert `xxx` TODO markers at gaps (feeding the progress report).
- **Setting & worldbuilding** — extract a setting bible (geography,
  magic/tech systems, factions) and flag prose that breaks established rules.
- **Also planned** — naming/term consistency, prose & style checks, "ask the
  manuscript" Q&A, a selection rewrite assistant, synopsis/blurb generation,
  and a local-model backend for fully offline use.
- Full design, model choices, and privacy model in
  [`docs/ai-features.md`](docs/ai-features.md).

### Editing & workflow

- Word-wrap soft-wrapped paragraphs to the focus column
- Per-session and daily word-count goals (per chapter / part / book / project)
- Find & replace
- Undo / redo

### Vim-style keybindings & modes

- **Opt-in** modal editing for authors with Vim muscle memory; the default
  modeless editing is unchanged when it's off.
- Normal / Insert / Visual modes with a mode indicator; motions (`hjkl`,
  `w b e`, `0 ^ $`, `gg`/`G`) and counts.
- Operators with motions and text objects (`d`/`c`/`y` + `iw`/`aw`/…),
  `x`, `p`/`P`, undo/redo, registers.
- Search (`/`, `n`/`N`) and a command line (`:w`, `:q`, `:wq`); later,
  configurable keybindings.

### Export to Word / Scrivener / Vellum

- Export the manuscript and its structure to the formats publishing actually
  uses: Word (`.docx`, standard manuscript format), Scrivener (binder
  hierarchy), and Vellum (per-chapter import).
- Carry structural metadata through where supported — chapter titles become
  titles, epigraphs render in place.
- Word is the highest-leverage target: it's the submission standard, and both
  Vellum and Scrivener import `.docx`.

### Hardening: performance & stability

Cross-cutting passes scheduled *after* the skeleton and feature set are built
out. Small wins happen inline during feature work; these are the deliberate,
focused passes before Quill is a daily driver.

- **Performance** — stay instant on real manuscripts: typing, scrolling,
  navigation, and save remain snappy from a short story up to a multi-book
  series, backed by a benchmark suite with documented targets.
- **Stability** — never lose an author's words: atomic saves, autosave and
  crash recovery, panic-free error handling, and a fuzzed, property-tested data
  path.

## License

MIT
