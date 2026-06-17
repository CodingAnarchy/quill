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
  for incremental parsing: syntax-aware highlighting in the focus column and a
  reliable document outline without regex heuristics.
- Soft inline rendering — show `*emphasis*` / `**strong**` styled rather than as
  literal asterisks, while keeping the underlying text Markdown.

### Project structure: series (optional) → book → chapter

- Model a manuscript as a tree: an optional **series** containing one or more
  **books**, each containing **chapters**, with an optional **act** grouping
  between book and chapter. A standalone novel is just one book.
- Drive the divisions from the document itself via tree-sitter heading levels
  (e.g. `#` book / series, `##` chapter), so structure and prose stay in one
  file and never drift out of sync. A multi-file project (one file per chapter,
  manifest at the root) is a later option.
- Outline/navigation pane that walks the tree; jump between chapters and books,
  with per-node and rolled-up word counts.

### Metadata & front matter

- Lightweight per-node metadata via YAML front matter (and/or fenced blocks),
  parsed alongside the tree-sitter outline:
  - **Chapter:** title, POV character, status (draft/revised/final), summary.
  - **Book / series:** title, subtitle, author.
- **Epigraphs** on both books and chapters — a quotation plus attribution stored
  in metadata and rendered above the body, distinct from the prose itself.
- Keep all metadata human-readable in the file so a manuscript remains portable
  plain text.

### Progress reports & writing goals

- Author-set goals in project front matter: target **words**, **chapters**, and
  `words_per_page` (pages are derived), plus a project **start date**.
- Single-screen status report tracking the manuscript against goals: word /
  page / chapter completion, a projected final length ("expansion ratio") from
  current pace, `xxx` TODO-marker count, and an "unused" (cut) section.
- Daily momentum from an append-only snapshot log: today's word delta with
  additions vs. deletions, the same broken down **per act**, average words/day,
  and "to go" estimates with projected completion dates.
- Full field-by-field spec and formulas in
  [`docs/progress-reports.md`](docs/progress-reports.md).

### Editing & workflow

- Word-wrap soft-wrapped paragraphs to the focus column
- Per-session and daily word-count goals (per chapter / act / book / project)
- Find & replace
- Undo / redo

## License

MIT
