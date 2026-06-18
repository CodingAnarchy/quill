# Quill — working notes for Claude

Quill is a terminal editor optimized for writing novels (Rust, `ratatui` +
`crossterm`, rope-backed via `ropey`, Markdown parsed incrementally with
`tree-sitter-md`). See `README.md` for the product roadmap and `docs/` for the
feature specs. Storage model: one chapter per file; a buffer holds one chapter
(single-chapter view) or at most ~3 chapters (continuous view), never the whole
novel.

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

What they assert (in `src/buffer.rs`):

- `keystroke_is_decoupled_from_document_size` — a keystroke only *records* the
  edit (no parse), so it stays cheap (~tens of µs) even on a 75k-word buffer.
- `reparse_is_fast_at_window_size` — a full reparse on a ~3-chapter window
  (the realistic worst case) is under one 16ms frame.

If you make per-keystroke work depend on document size, the first guard will
catch it. Keep parsing off the keystroke path (record on edit, reparse on idle
in the `main.rs` event loop).

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
- Keys: `C-o` toggles the outline, `C-s` saves, `C-q` quits, arrows move.
- Always `tmux kill-session -t quill` when done so sessions don't linger.
