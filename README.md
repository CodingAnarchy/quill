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

- Word-wrap soft-wrapped paragraphs to the focus column
- Chapter/scene navigation and a manuscript outline
- Per-session and daily word-count goals
- Find & replace
- Undo / redo

## License

MIT
