# Quill — working notes for Claude

Quill is a terminal editor optimized for writing novels (Rust, `ratatui` +
`crossterm`, rope-backed via `ropey`, Markdown parsed incrementally with
`tree-sitter-md`). See `README.md` for the product roadmap and `docs/` for the
feature specs. Storage model: one chapter per file; a buffer holds one chapter
(single-chapter view) or at most ~3 chapters (continuous view), never the whole
novel.

Chapter files may begin with a `---`-fenced YAML front matter block. Quill
splits it off when loading (`src/frontmatter.rs`), keeps it out of the prose
buffer so the editor shows only the body, and re-attaches it verbatim on save.
Titles stay in the manifest; front matter carries the rest of the chapter
metadata (the #3 epic builds typed access on this).

The continuous-manuscript view (`Ctrl-l`) concatenates a window of up to ~3
adjacent chapters into one editable buffer (`src/manuscript.rs`), paging
chapters in/out around the cursor and writing edits back to each chapter's file.
Chapter boundaries are fixed seams (editing never merges chapters); the centered
title dividers between chapters are render-only.

## Code conventions

- **`use` imports go at the top of the module**, after the module doc comment —
  not inside functions or partway down the file. Test modules follow the same
  rule: put the test `use`s at the top of the `#[cfg(test)] mod tests` block,
  not inside individual `#[test]` fns.
- **Prefer a `use` import over spelling out a full module path** at the call
  site. Write `use std::fs;` then `fs::write(...)`, or `use crate::buffer::count_words;`
  then `count_words(...)` — not `std::fs::write(...)` / `crate::buffer::count_words(...)`
  inline. (Disambiguate with a partial path or `as` alias only when two names
  would otherwise collide.)

## Standard checks

Run these before opening a PR:

```sh
cargo test            # unit + render tests (must be green)
cargo clippy --all-targets   # must be clean
```

## Performance guards (ignored tests)

Some tests are `#[ignore]`d because they measure timing and only make sense in
release. **Run them whenever you touch the parse, the edit path, or rendering**
— they back the "typing stays instant" promise.

```sh
cargo test --release -- --ignored --nocapture
```

What they assert:

- `keystroke_is_decoupled_from_document_size` (`buffer.rs`) — a keystroke only
  *records* the edit (no parse), so it stays cheap (~tens of µs) even on a
  75k-word buffer.
- `reparse_is_fast_at_window_size` (`buffer.rs`) — a full reparse on a
  ~3-chapter window (the realistic worst case) is under one 16ms frame.
- `render_stays_within_a_frame` (`ui.rs`) — a full `draw()` (large chapter +
  a many-chapter outline pane) stays under one frame. The status bar's live
  word count is O(document) per draw; the pane reads cached per-chapter
  metadata, never the files. If rendering ever re-reads files or does
  per-chapter work proportional to the novel, this catches it.
- `open_is_fast_for_a_large_novel` (`project.rs`) — opening reads and parses
  every chapter file once (word count + front matter); even a 150-chapter
  novel opens in a few ms.

If you make per-keystroke work depend on document size, the first guard will
catch it. Keep parsing off the keystroke path (record on edit, reparse on idle
in the `main.rs` event loop). Keep per-frame rendering off the filesystem —
cache file-derived data (word counts, front-matter metadata) on the manifest
at open, like `ChapterEntry` does.

## Live TUI verification (tmux)

Unit tests cover render logic via `ratatui::TestBackend`, but for visual
features (syntax highlighting, soft-rendered emphasis, the outline pane) it's
worth driving the real binary in a PTY and capturing the screen. We use `tmux`
(install once with `brew install tmux`).

`capture-pane -p` gives plain text (proves layout and marker concealment);
`capture-pane -e` keeps the ANSI escapes (proves styling: italic `^[[3m`, bold
`^[[1m`, heading color `^[[38;5;6m`).

```sh
cargo build
# Quill operates on a project directory; seed one with a manifest + chapter.
rm -rf /tmp/quill-sample && mkdir -p /tmp/quill-sample
printf 'title = "Sample"\n\n[[chapters]]\ntitle = "One"\nfile = "one.md"\n' > /tmp/quill-sample/book.toml
printf '# Title\n\nThe night was *dark* and **long**.\n' > /tmp/quill-sample/one.md

tmux kill-session -t quill 2>/dev/null
tmux new-session -d -s quill -x 90 -y 16 "target/debug/quill /tmp/quill-sample"

# Bounded wait for first paint (no foreground `sleep`; poll until content shows).
i=0; until tmux capture-pane -p -t quill 2>/dev/null | grep -q "Title"; do
  i=$((i+1)); [ "$i" -gt 200 ] && { echo TIMEOUT; break; }
done

tmux capture-pane -p -t quill            # readable screenshot
tmux capture-pane -e -p -t quill | sed 's/\x1b/^[/g'   # with styling escapes

# Drive input, then re-capture to verify interactive behavior:
tmux send-keys -t quill C-o              # e.g. toggle the outline
for _ in $(seq 1 6); do tmux send-keys -t quill Down; done
i=0; until tmux capture-pane -p -t quill | grep -q OUTLINE; do
  i=$((i+1)); [ "$i" -gt 200 ] && { echo TIMEOUT; break; }
done
tmux capture-pane -p -t quill

tmux kill-session -t quill               # always clean up
```

Notes:
- Use the bounded `until` poll to wait for paint — `sleep` is blocked in this
  harness, and the editor renders within a few milliseconds anyway.
- Keys: `C-o` toggles the outline, `C-l` toggles the continuous view, `C-s`
  saves, `C-q` quits, `C-n`/`C-p` step chapters (single view), arrows move.
- Always `tmux kill-session -t quill` when done so sessions don't linger.
