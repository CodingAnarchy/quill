# Compatibility & stability

Your manuscript is the thing that matters, and it lives in files on your disk — a `book.toml` manifest and a Markdown file per chapter, all of it plain text you own. This document is Quill's promise about what stays stable once **1.0** ships, so you can trust the format with a book you'll be writing for years.

Quill follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (as the [changelog](CHANGELOG.md) states). Under SemVer, the leading `1` is the compatibility contract: everything below is a promise about the whole **1.x** line — every 1.1, 1.2, and patch release in between — and only a future **2.0** could break it, deliberately and with a migration path.

## What 1.0 guarantees

**Your project opens in every 1.x.** A project directory written by any 1.x release of Quill opens correctly in every other 1.x release — earlier or later. You can upgrade Quill without touching your manuscript, and a book started on one machine's version opens on another's. The format doesn't shift under you across the 1.x line.

Concretely, the whole on-disk project is covered:

- **The manifest** (`book.toml`) — the chapter order and titles, book and series metadata (title, subtitle, author, series, publishing details), writing goals, parts, and the export `[style]`/`[print]` tables. A standalone novel's flat manifest and its shape are frozen for 1.x.
- **The series manifest** — a multi-book project's `[[books]]` array, each book carrying its own parts and chapters under a series root. Same guarantee: the series shape written by any 1.x reads in every 1.x. A standalone novel that carries only a `series` label is never rewritten into the multi-book shape behind your back.
- **The story bible** — the human-readable Markdown files under `bible/` (characters, timeline, setting, outline, and the rest). They're plain Markdown, they stay plain Markdown, and your hand-edits are ground truth.
- **Chapter front matter** — the `---`-fenced YAML block at the top of a chapter file (status, summary, POV, goal, section role, epigraph, scene metadata). Quill splits it off to show you only the prose and re-attaches it **verbatim** on save, so keys Quill doesn't manage — and your own hand formatting — survive byte-for-byte.
- **Configuration** — the keys in your home `config.toml` (`[editor]`, `[theme]`, `[projects]`, `[ai]`, `[image]`, `[snippets]`, `[workflows]`/`[triggers]`, and their per-project overrides), documented in [`docs/configuration.md`](docs/configuration.md).

**Config keys are stable.** A setting documented for 1.0 keeps its name, its section, its meaning, and its default across all of 1.x. A config file you write today keeps working, unchanged, on every later 1.x. New settings may be *added* — the default of any new key is always the pre-existing behavior, so adding one never changes how your editor already behaves — but an existing key is never renamed, moved, or repurposed within 1.x.

**The command line is stable.** The invocation and flags — `quill [PATH]`, `quill --series`, `-h`/`--help`, `-V`/`--version` — keep their names and meaning across 1.x. New flags may be added; existing ones aren't removed or given a new meaning.

## How the format holds together

These aren't aspirations — the mechanism is in the code, and it's what makes the promise above enforceable rather than a hope.

**Manifests carry a format version.** Both the standalone `book.toml` and the series manifest stamp a `format_version` on save. It's bumped **only** for a change an older Quill genuinely couldn't read correctly — never for adding a field. A manifest written before the version existed simply reads as the current version, so existing projects are unaffected.

**Additive changes don't break anything.** New fields are added as optional keys. An older Quill reading a newer project's manifest just ignores a field it's never heard of — it's not an error, and the fields it *does* understand load intact. This is why routine feature work across 1.x never bumps the format version: it's additive by construction. (One consequence to know: because the manifest is a typed structure, an *older* Quill that re-saves a project drops a *newer* version's additive field it can't model. Chapter front matter is the exception — it's preserved verbatim, unknown keys and all.)

**An older format migrates forward on open.** When a genuinely breaking change does eventually land — the one case additive evolution can't cover — the version bumps and a migration step translates the older layout up to the current one the moment you open the project, one version at a time. You open your book; Quill brings the format current; you keep writing. There's no manual conversion step and no separate tool.

**A newer format is refused, not silently downgraded.** If you open a project written by a *newer* Quill than the one you're running — a `format_version` this build doesn't know — Quill stops with a clear message naming both versions and telling you to upgrade, rather than loading a lossy view and stripping the future fields on the next save. Your work is never quietly damaged by an older build; the honest failure is the safe one.

Together these are the escape valve: the format can still evolve when it truly must, but never by breaking the manuscript you already have. A future **2.0** — should it ever come — would use exactly this migration path to carry your 1.x project forward.
