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

## Build & run

```sh
cargo run --release -- manuscript.txt
```

Passing no file opens an untitled buffer.

## Keys

| Key            | Action                          |
| -------------- | ------------------------------- |
| *(typing)*     | insert text                     |
| Arrows         | move cursor                     |
| Home / End     | start / end of line             |
| Ctrl-S         | save                            |
| Ctrl-Q         | quit (prompts if unsaved)       |
| Ctrl-+ / Ctrl- | widen / narrow the text column  |

## Layout

```
src/
  main.rs    terminal setup + event loop
  app.rs     application state, modes, key handling
  buffer.rs  rope-backed document, cursor, editing & movement
  ui.rs      rendering: centered page + status line
```

## Roadmap

### Markdown manuscript format

- Treat the manuscript as Markdown, so emphasis, scene breaks, and headings are
  plain text the author can read anywhere.
- **tree-sitter** ([`tree-sitter-md`](https://github.com/tree-sitter-grammars/tree-sitter-markdown))
  for incremental parsing: syntax-aware highlighting in the focus column and
  scene-level structure *within* a chapter (the chapter tree itself comes from
  the project manifest — see Project structure).
- Soft inline rendering — show `*emphasis*` / `**strong**` styled rather than as
  literal asterisks, while keeping the underlying text Markdown.

### Project structure: a project is a folder of chapter files

- A Quill project is a **directory**: each chapter is its own Markdown file, and
  a root **manifest** records the tree — an optional **series** containing one or
  more **books**, each with **chapters** and an optional **act** grouping. A
  standalone novel is one book. The editor manages these files transparently;
  the author works in chapters, not filenames.
- **Two views, switchable by hotkey.** *Chapter-at-a-time* (default) edits one
  chapter file at a time via the outline pane. *Continuous manuscript* scrolls
  every chapter as one seamless document, with the editor mapping edits back to
  the right files.
- Outline/navigation pane walks the manifest tree; jump between chapters, acts,
  and books, with per-node and rolled-up word counts.

### Metadata & front matter

- Per-node metadata, all human-readable so the manuscript stays portable:
  - **Chapter:** title, POV character, status (draft/revised/final), summary —
    stored as standard YAML front matter at the top of each chapter file.
  - **Book / series:** title, subtitle, author — stored in the project manifest.
- **Epigraphs** on both books and chapters — a quotation plus attribution stored
  in metadata (chapter front matter or the manifest) and rendered above the body,
  distinct from the prose itself.

### Progress reports & writing goals

- Author-set goals in the project manifest: target **words**, **chapters**, and
  `words_per_page` (pages are derived), plus a project **start date**.
- Single-screen status report tracking the manuscript against goals: word /
  page / chapter completion, a projected final length ("expansion ratio") from
  current pace, `xxx` TODO-marker count, and an "unused" (cut) section.
- Daily momentum from an append-only snapshot log: today's word delta with
  additions vs. deletions, the same broken down **per act**, average words/day,
  and "to go" estimates with projected completion dates.
- Full field-by-field spec and formulas in
  [`docs/progress-reports.md`](docs/progress-reports.md).

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
- Per-session and daily word-count goals (per chapter / act / book / project)
- Find & replace
- Undo / redo

### Export to Word / Scrivener / Vellum

- Export the manuscript and its structure to the formats publishing actually
  uses: Word (`.docx`, standard manuscript format), Scrivener (binder
  hierarchy), and Vellum (per-chapter import).
- Carry structural metadata through where supported — chapter titles become
  titles, epigraphs render in place.
- Word is the highest-leverage target: it's the submission standard, and both
  Vellum and Scrivener import `.docx`.

## License

MIT
