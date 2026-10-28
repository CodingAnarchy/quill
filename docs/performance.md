# Performance: benchmarks, corpus, and targets

Quill must stay instant on real manuscripts — typing, scrolling, chapter
navigation, and saving snappy from a short story up to a multi-book series, not
just on toy files (epic #14). Responsiveness is hard to retrofit and easy to
regress, so this is the measured gate that catches a slowdown before an author
hits it mid-draft. You can't defend a target you don't measure.

## Running the suite

```sh
cargo perf   # alias for: cargo test --release -- --ignored --nocapture --test-threads=1
```

The benchmarks are `#[ignore]`d so they don't run under plain `cargo test`:
they measure wall-clock time, which is only meaningful in a release build (a
debug build is 3–18× slower and the thresholds false-fail). They also run
**single-threaded** (`--test-threads=1`): several guards build and read
series-sized projects (thousands of files), and running them concurrently with
the CPU-bound timing benchmarks (reparse, render, keystroke) would starve those
of a core and false-fail their sub-frame thresholds. Run `cargo perf`
**before every release**, and whenever you touch the parse, the edit path,
rendering, or project load. `--nocapture` prints each measured time next to its
target.

`cargo perf` is **hermetic and deterministic** (#1022): it needs no network and
no machine isolation, so a green run means performance is green — full stop. Two
things make that true. Single-threaded execution (above) means a parallel build
or several busy worktrees can't starve a timing guard into a false fail, so you
never have to re-run to shake off a contention flake. And the `--ignored` sweep
no longer picks up the network catalog checks: it runs offline without a
`--skip` flag.

### Thresholds widen on CI (shared-runner headroom)

The budgets are tuned for a real machine. A GitHub-hosted runner is a shared
2-vCPU VM that runs this CPU-bound work ~2.5–3× slower (the continuous-view
window reparse measured ~7 ms locally vs ~20 ms on CI), so a threshold that's
honest on a dev box false-fails there. Every wall-clock guard therefore compares
against `perf_budget_ms` / `perf_budget_us` (in `crates/quill-core/src/testutil.rs`), which triple
the budget when the `CI` env var is set — GitHub Actions sets it automatically.
So the informational `perf` CI job (`.github/workflows/ci.yml`) only reddens on a
gross (>3×) regression, while **`cargo perf` on real hardware, where `CI` is
unset, keeps the exact tuned budgets** and remains the strict, authoritative
pre-release gate. Scale-independence guards (a ratio of a large run to a small
one, e.g. `word_motion_is_decoupled_from_document_size`) already cancel the
hardware factor and deliberately don't use the slack.

### The catalog check is separate (network, run deliberately)

Two `#[ignore]`d tests — `catalog_gguf_files_exist_on_huggingface` and
`embed_catalog_files_exist_on_huggingface` (#726, in `crates/quill-core/src/ai/model_select.rs`) —
HEAD each `MODELS`/`EMBED_MODELS` row's weights URL on HuggingFace to catch a
catalog that names a file a user's download would 404 on. They reach the network,
so they are *not* part of the offline perf gate. They no-op unless the
`QUILL_CATALOG_CHECK` env guard is set; the `cargo perf` sweep doesn't set it, so
those tests pass trivially and the gate stays offline. Run them deliberately —
before a release, or whenever a row's repo/filename changes — with the dedicated
alias plus the guard:

```sh
QUILL_CATALOG_CHECK=1 cargo perf-catalog   # runs *only* the two HuggingFace catalog checks
```

(The alias filters to the two tests by name; a cargo alias can't set env itself,
hence the inline `QUILL_CATALOG_CHECK=1` prefix. Without the guard the checks
skip and report why.)

Each benchmark lives next to the code it guards (so a regression points at the
cause) and asserts against a written-down budget — it *fails* if exceeded,
rather than just reporting. This file is the index; the budgets and rationale
live here so a contributor can defend or update a number.

## The corpus

Quill never holds the whole novel in a buffer — one chapter in the
single-chapter view, at most ~3 (a window around the edit point) in the
continuous view — so per-buffer cost is bounded to the editing window. The
benchmarks exercise the three shapes that matter:

- **A long single chapter** — the largest buffer the single-chapter view holds.
- **A ~3-chapter window** — the continuous view's realistic worst case.
- **A series-sized project** — many chapter *files* (only a window is ever
  loaded), for open and navigation cost as the project grows.

Corpora are generated deterministically in-test, so the suite needs no fixtures.

## The benchmarks

| Benchmark | Location | Corpus | Budget | What it guards |
| --- | --- | --- | --- | --- |
| `keystroke_is_decoupled_from_document_size` | `buffer.rs` | 150-chapter buffer of blank-line-separated paragraphs (~110k words) | < 1 ms / keystroke | A keystroke only marks the parse stale (the reparse is deferred to the settled pass, and there is no tree bookkeeping per edit), so its cost is in the edit, not the document size. (Typically ~10 µs; ~1.6 ms before the per-block reparse, when every edit fixed up one inline tree per paragraph.) |
| `reparse_is_fast_at_window_size` | `buffer.rs` | ~3-chapter window of realistic paragraphs (~13k words) | keystroke reparse < 2 ms (best of 5); unclosed-fence reparse < 16 ms | The reparse after a keystroke sends only the edited block through tree-sitter — every other block's parse is cached by its text (`crates/quill-core/src/syntax/segment.rs`) — so it is ~0.2 ms here (it was ~20 ms when the whole window was re-parsed). The worst case, opening a fence near the top so the rest of the window turns to code, re-parses the rest once (~10 ms, the cost of a cold parse) and must stay under a frame. Best-of-5 because a single sample catches one-off noise (scheduling, CPU ramp); noise only ever *adds* time. |
| `render_stays_within_a_frame` | `ui/tests/perf.rs` | ~83k-word chapter (emphasis on every line) + 150-chapter outline pane, 100×60; 300 spell flags, 600 proximity repeats, 200 style flags, 201 annotations; then a find with 7,500 matches | prose and search passes < 4 ms / frame (~0.5 ms measured); folded / modal / scrolled / assistant passes < 16 ms | A full `draw()` of unfolded, wrapped, styled prose from mid-chapter stays well under a frame. The body renderer narrows each whole-buffer range list (conceal markers, matches, flags, annotations) to the row by binary search; scanning those lists per character cost ~45 ms on this corpus, so the tight budget catches that regression. The status bar's word count is O(document); the pane reads cached per-chapter metadata, never files. Catches rendering that re-reads files or does per-chapter work proportional to the novel. The folded pass reads the fold view as one sorted line range per folded section, answered by binary search; it used to insert every hidden line into a `HashSet` each frame (~0.85 ms of the ~1 ms folded frame; the view now builds in ~30 µs). |
| `outline_pane_is_flat_in_chapter_count` | `ui/tests/perf.rs` | small chapter (40 headings) beside a 150- vs a 1500-chapter outline, active chapter 90% down the list | pane at 1500 chapters < 1.5× its 150-chapter time + 20 µs (~1.2× measured, ~45 µs); full 1500-chapter frame < 4 ms (~0.15 ms) | The outline pane walks an unstyled row model to place its scroll and styles only the visible rows, so a series-sized outline costs about what a novel's does. It used to format a styled row for every chapter each frame (and never scrolled): ~66 µs → ~190 µs from 150 to 1500 chapters. The active-chapter index (an O(chapters) scan) is resolved once per `draw()` and threaded to the panes that need it. |
| `open_is_fast_for_a_large_novel` | `project.rs` | 150 chapters on disk | < 250 ms | Opening reads and parses each chapter once (word count + front matter) and caches it; catches a regression to per-chapter re-reads or O(n²) work at open. (Typically a few ms.) |
| `open_is_fast_for_a_series_sized_project` | `project.rs` | 1500 chapters, one book | < 500 ms | Open scales linearly with the project (the per-chapter cache read is the only proportional work, and it's bounded). (Typically ~25–60 ms.) |
| `open_is_fast_for_a_multi_book_series` | `project.rs` | 1500 chapters across 6 books (the `[[books]]` format, #328) | < 500 ms | Opening the multi-book manifest runs the books-array parse, the active-book hoist, and (since #331) one read per chapter across every book to cache word counts for series rollups — the same total file work as a single book of the same size, so it stays linear. (Typically ~20 ms.) |
| `open_is_fast_for_a_large_collection` | `project/tests/collection.rs` | a collection linking 50 short stories of ~5k words (#1244) | < 50 ms | Opening a collection reads each linked story once — its manifest and its story file, without opening or locking the story project — and caches its title, words, and status on the link, so the story list never touches the disk per frame. (Typically ~9 ms.) |
| `startup_is_fast_for_a_series` | `app/tests/launch.rs` | 1500 chapters across 6 books, `[quill.editor]` override in `book.toml`, last position on a later chapter | < 1.5× a bare open + 10 ms (median of 5), and < 500 ms | The real launch `main` runs (`App::launch`: open, every config section, the remembered chapter, the recovery / damage-restore / pass-queue offers, onboarding) costs about one project open. Catches opening the project twice (the damage check used to re-open it and re-read every chapter), parsing the series-sized `book.toml` once per config section instead of once per launch, or loading chapter 0 before switching to the remembered one. A ratio against a bare open on the same machine, so it holds on any hardware. (Typically ~39 ms vs a ~38 ms open; ~86 ms before.) |
| `navigation_is_fast_in_a_series_sized_project` | `app/tests/chapters_counts_and_perf.rs` | 600 chapters on disk, every third with annotations | < 10 ms / switch, and zero `annotations.toml` writes | Switching chapters reads/parses one chapter file — never work proportional to the project. A clean switch changes no note, so it must not rewrite the annotations side file (a durable write, two fsyncs — several ms on macOS); the guard counts those writes rather than timing them, since one write per annotated chapter averages to too little per switch for the time budget to catch. (Typically ~0.5 ms.) |
| `switching_books_is_fast_in_a_series` | `project.rs` | 2 books × 750 chapters (#329) | < 500 ms | Switching the active book re-reads the target book's chapters once (the bounded work `open` does for one book) — never the whole series. The re-read is deliberate: `open` already cached every book (#331), but a switch should reflect chapters edited outside Quill since open. (Typically ~5–10 ms.) |
| `tree_rows_are_fast_for_a_series` | `app/tests/books.rs` | 4 books × 200 chapters, all expanded (#330) | < 16 ms | Building the tree navigator's visible rows reads only cached per-chapter metadata (titles, roles), never files — so even fully expanded it stays within a frame. (Typically ~70 µs.) |
| `tally_is_independent_of_project_size` | `app/tests/chapters.rs` | 2000 chapters on disk | < 30 µs / idle tick | The daily word tally touches only the chapters that changed (the live active one, plus any whose cache moved), not all of them. A regression to the per-chapter walk would be ~50–100 µs here. (Typically ~6 µs.) |
| `keystroke_with_settled_pass_stays_within_a_frame` | `app/tests/chapters_counts_and_perf.rs` | ~6k-word chapter on disk, dirty | < 4 ms / keystroke | A keystroke as the event loop pays for it: `on_key` + the per-batch settled pass (reparse, fold prune, annotation/spell-flag re-anchoring) + the throttled recovery tick. The heavy idle work — spell re-judging, revision passes, tally, the recovery capture (whose fsync'd write runs on a background thread), sealing the undo group — is debounced to a ~200 ms pause in input and must stay off this path; running it per batch (the old behavior, ~16 ms here) trips the budget, as would re-parsing the whole chapter per batch. (Typically ~0.2 ms on macOS since the per-block reparse; ~10 ms before it, almost all of it re-parsing the whole chapter.) |
| `the_typing_and_idle_path_never_writes_on_the_ui_thread` | `app/tests/recovery_writer.rs` (plain `cargo test`, not `--ignored`) | two-chapter project, single and continuous view, three typing rounds with the recovery throttle expired between them | zero atomic writes on the UI thread | The crash-recovery sidecar write is durable (temp, `F_FULLFSYNC`, rename, directory `fsync`): ~10 ms on an idle disk, but under contention each step stalled independently — p50 15–99 ms, max ~7 s — and keys queued in the pty and echoed in a burst. The capture now hands an O(1) rope snapshot to a background writer (`crates/quill-core/src/recovery/writer.rs`, latest-wins per project). The guard counts atomic writes on the calling thread (the `atomic::write_count` seam) across `on_key`, `on_input_settled`, `tick_recovery`, and `on_quiet`, so moving the write back onto the UI thread fails it regardless of disk speed. Explicit saves and autosave are a separate path. |
| `save_is_independent_of_project_size` | `app/tests/chapters.rs` | 150 vs 1500 chapters on disk (#867) | < 500 µs at 1500, and flat vs 150 | A save (Ctrl-S / autosave / chapter-switch flush) records a snapshot whose TODO count and project word total read maintained caches — the TODO count per chapter and a running word total — never a per-chapter disk sweep. The roll-up cost must stay flat from 150 to 1500 chapters; a regression to the old per-chapter `read_to_string` + TODO scan would make the 1500-chapter case ~10× the 150-chapter one (milliseconds). (Typically ~3–7 µs.) |
| `a_keystroke_is_fast_in_a_long_chapter` | `crates/quill-ffi/src/session/tests.rs` | a 10k+-word chapter of styled paragraphs (#1232) | < 1 ms / keystroke | The iPhone editor's per-keystroke work through the FFI: apply the text view's UTF-16 edit to the buffer, return the styled spans for the edited paragraph (an incremental reparse), and read the live word count. UTF-16 positions convert through the rope in O(log n), and styles are asked for the paragraph, never the chapter, so typing stays smooth however long the chapter grows. (Typically ~0.1 ms.) |

Budgets are deliberately generous headroom over the typical measured time: they
catch *regressions* (a gross slowdown), not micro-fluctuations. When an
optimization lands (#218–#220), tighten the relevant budget toward the new
typical so it keeps defending the win.

## The invariants these protect

- **Parsing is off the keystroke path** — mark the parse stale on keypress,
  reparse once per input batch (the `main.rs` event loop). The keystroke guard
  enforces this.
- **A reparse is proportional to the edit** — the document is cut into blocks at
  blank lines and each block's parse is cached by its text, so only changed
  blocks go through tree-sitter (`crates/quill-core/src/syntax/segment.rs`; the property tests in
  `crates/quill-core/src/syntax/tests/incremental.rs` hold the result identical to a whole-document
  parse). The window reparse guard enforces this.
- **Heavy idle work waits for a pause** — per input batch the loop runs only
  `on_input_settled` (the reparse plus re-anchoring the byte-offset caches the
  renderer reads), draws, and runs the rest (`on_quiet`) once input has been
  quiet for `IDLE_QUIET`. The recovery sidecar is rewritten only when the text
  changed, at most once per `RECOVERY_MIN_INTERVAL`. The settled-keystroke guard
  enforces this.
- **No durable write on the typing path** — the recovery capture hands a
  snapshot to a background writer and returns; the UI thread never waits on a
  `fsync`. The UI-thread write guard enforces this.
- **Rendering never touches the filesystem** — file-derived data (word counts,
  front-matter metadata) is cached on `ChapterEntry` at open and kept current as
  the author edits. The render and navigation guards enforce this.
- **Work stays incremental** — editing one chapter doesn't reprocess the whole
  project (parse, word counts, AI caches). The navigation guard, and the
  word-count work in #220, enforce this.
- **Memory is bounded to the editing window** — the project holds one small
  `ChapterEntry` per chapter (title, file, cached count, front-matter metadata),
  never a chapter's body text, and the editor holds one live `Buffer` (the active
  chapter) — at most ~3 in the continuous view. So a series-sized project's
  footprint grows with the *number* of chapters (small metadata), not with the
  manuscript's word count. Opening or editing one chapter reads/parses only that
  chapter's file, never the others'.
