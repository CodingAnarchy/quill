# Contributing to Quill

Thanks for your interest in Quill — a terminal editor optimized for writing
novels. This guide covers building from a checkout, the checks to run before you
open a pull request, and the conventions the codebase holds to. It's the
public-facing companion to the fuller working notes in
[`CLAUDE.md`](CLAUDE.md); if the two ever disagree, `CLAUDE.md` is the source of
truth.

## Building from a checkout

Quill is a from-source Rust tool. You'll need a Rust toolchain — install one via
[rustup](https://rustup.rs) if you don't have it.

You don't need to pick a version. `rust-toolchain.toml` at the repo root pins
the one Quill builds with, and rustup installs it automatically the first time
you run a cargo command here (so the first build may pause to download it). CI
resolves the same file, which means a green check on your machine means a green
check on the runners. If you want to move Rust forward, change `channel` there
and open a PR — nothing upgrades it on its own.

To build and run the editor directly from a checkout:

```sh
cargo run --release -- path/to/book
```

Quill operates on a **project directory**. Pass a path: if the directory has a
`book.toml` manifest it's opened, otherwise a new project is initialized there (a
manifest plus a first chapter). A debug build (`cargo run --`) is fine for most
development — the release flag matters for the performance guards described
below, not for everyday editing.

Quill has optional Cargo features. The one that exists today is `local`, the
on-device AI backend (an in-process quantized model via `candle`); a standard
build is Claude-only. Opt into it with `--features local` on any Cargo command:

```sh
cargo run --release --features local -- path/to/book
```

To install a `quill` command on your `PATH` instead of running from the tree,
`./install.sh` wraps the release build and drops the binary in `~/.local/bin`
(re-run it to upgrade). See the README's **Install** section for the details.

## Checks to run before a pull request

Run these three checks and get them green before opening a PR — they're the same
gate a reviewer will hold your change to:

```sh
cargo fmt --check            # formatting must match rustfmt (run `cargo fmt` to fix)
cargo test                   # unit + render tests — must be green
cargo clippy --all-targets   # must be clean
```

### Performance guards

Some tests are `#[ignore]`d because they measure timing and only make sense in a
release build — in a debug build the unoptimized code is several times slower and
the thresholds false-fail, so they can't run as part of plain `cargo test`. They
back Quill's "typing stays instant" promise, asserting that a keystroke stays
cheap regardless of document size, a reparse fits inside a frame, rendering fits
inside a frame, and even a large or series-sized project opens near-instantly.

**Run them whenever your change touches the parse, the edit path, rendering, or
project load:**

```sh
cargo perf   # alias for: cargo test --release -- --ignored --nocapture
```

The full corpus — every benchmark, its target, and the rationale — is written up
in [`docs/performance.md`](docs/performance.md), the pre-release perf gate. When
in doubt about whether a change is perf-sensitive, run it; it's fast.

## Code conventions

- **Formatting is `rustfmt`'s.** Run `cargo fmt` before committing and keep
  `cargo fmt --check` clean — don't hand-format against the tool. (The one-time
  tree-wide reformat is listed in `.git-blame-ignore-revs`; run `git config
  blame.ignoreRevsFile .git-blame-ignore-revs` once so `git blame` skips it.)
- **`use` imports go at the top of the module**, after the module doc comment —
  not inside functions or partway down the file. Test modules follow the same
  rule: put the test `use`s at the top of the `#[cfg(test)] mod tests` block, not
  inside individual `#[test]` functions.
- **Prefer a `use` import over spelling out a full module path** at the call
  site. Write `use std::fs;` then `fs::write(...)`, not `std::fs::write(...)`
  inline. Disambiguate with a partial path or an `as` alias only when two names
  would otherwise collide.
- **Keep source files under ~1,000 lines.** When a module approaches that,
  extract a cohesive group of types or traits into its own file rather than
  letting one file sprawl. This counts the whole file — a test module that has
  grown large is itself a signal to split the module.

## The AI-assistance boundary

This one is non-negotiable, and it's the most important thing to know if you're
touching an AI feature. **Quill's AI is an assistant, not a co-writer.** It may
*track* (the story bible), *outline* (beat sheets and scene notes), *mark* (`xxx`
TODO notes), *observe* (continuity, consistency, and style *findings*, surfaced
for the author to fix), and *look up* (a thesaurus or name candidates the author
deliberately invokes and chooses among) — but **it never generates, drafts, or
rewrites manuscript prose.** A feature that would draft or reword the author's
text is out of scope: surface the issue, don't write the fix. The only prose the
AI produces is off-manuscript (a synopsis, blurb, or query letter), never inside
the manuscript itself.

If you're adding or changing an AI feature, hold it to that line. The fuller
rationale — including the fine distinction between *reference lookup* and
*co-writing* — is in [`CLAUDE.md`](CLAUDE.md) and
[`docs/ai-features.md`](docs/ai-features.md).

## Filing issues

Bug reports and feature requests both have templates — pick one when you open an
issue. For a bug, the most useful thing you can give us is a clear set of steps
to reproduce it. For a feature, lead with the *problem* — what you're trying to
do and why it's hard today — rather than a specific implementation; Quill's
tickets are written problem-first, so the solution can be designed in the open.

## Pull request expectations

A pull request that's easy to merge:

- Has `cargo fmt --check`, `cargo test`, and `cargo clippy --all-targets` all
  green.
- Includes `cargo perf` results if it touches the parse, edit, render, or
  project-load paths.
- Adds or updates tests for the behavior it changes.
- Follows the code conventions above.
- Explains the change against the problem it solves, so a reviewer can verify it
  does what it intends. Link the issue it closes.

Keep a PR focused on one change — it's easier to review, and easier to revert if
something turns out to be wrong. Thanks for helping make Quill better.
