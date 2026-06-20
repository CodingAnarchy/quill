# Spike: Git integration approach (#129)

**Question.** How should Quill talk to Git for the version-control epic (#54) —
shell out to the system `git` CLI, link **git2** (libgit2 bindings), or use
**gix** (gitoxide, pure Rust)?

**Recommendation.** Use **git2** (libgit2), behind a thin internal Git module so
the backend stays swappable. It needs no system `git` install (the author never
has to have or learn Git), has the most mature support for the genuinely hard
part — remote auth (#134) — and produces a compact binary with an ergonomic API.
Its only real cost is build-time C deps (cmake, and OpenSSL for HTTPS), which is
acceptable: Quill already builds C (tree-sitter), and the local-only tickets can
build git2 with `default-features = false` to avoid OpenSSL until remote sync
lands. **gix** is the pure-Rust fallback if those build deps ever block us.

Tested, not assumed — all three did init → commit → status below.

## Why not the CLI

Shelling out to `git` is the least code, but it **requires the author to have
`git` installed** and on `PATH`. The whole point of this epic is that a novelist
never has to grok — or install — Git. So the CLI is the rejected baseline: fine as
a last resort, wrong as the default.

## Options compared

Measured on macOS arm64, Rust 1.94. Binary = a single release binary (lto +
strip) doing init/commit/status; Δ is over the CLI-only baseline (354 KB).

| Dimension | CLI (`git`) | **git2 (libgit2)** | gix (gitoxide) |
|---|---|---|---|
| Requires system `git` | **yes** | no | no |
| Binary size | 354 KB (baseline) | **891 KB** (+0.5 MB) | 3.39 MB (+3 MB) |
| Dependencies | 0 | **108** | 593 |
| Native build deps | none | cmake + cc; OpenSSL (HTTPS) | **none (pure Rust)** |
| Commit / status API | full (shell) | **mature, ergonomic** (`index.add_all` → `write_tree` → `commit`) | works, but **lower-level** (manual blob → tree-editor → commit) |
| Remote push/pull + auth (#134) | full (git's own) | **mature** (libgit2 HTTPS/SSH, credential helpers) | younger / less complete |
| Cross-compilation | n/a | harder (C + OpenSSL) | **easiest** (pure Rust) |

Two findings that overturned assumptions:

- **Pure Rust isn't smaller here.** gix produced a **3.4 MB** binary vs git2's
  **0.89 MB** — gix statically compiles a large tree of `gix-*` crates (593 deps),
  while stripped libgit2 is compact.
- **git2 is the ergonomic one for writes.** A commit is `index.add_all` →
  `write_tree` → `commit`. In gix there's no `add_all`; you write each blob, build
  the tree with the tree editor, then commit — more code per operation (its
  *status*/diff side, by contrast, is excellent).

## POC results

All three ran init → write file → commit `"first"` → modify → status, against a
throwaway temp repo (code in `/tmp/git-spike`):

```
CLI:  commit=0557587 first  dirty=true
git2: commit=1c10601 "first" dirty=true
gix:  commit=9e7a148 "first" dirty=true   ← real pure-Rust commit (blob→tree→commit)
```

So all three are feasible end-to-end, including a pure-Rust gix commit (it needed
the `tree-editor` feature and the blob→tree→commit dance). The differences are
footprint, ergonomics, and remote-auth maturity — not capability for the local
operations.

## Recommendation & rationale

**git2**, because it best fits the epic's constraints:

1. **No system `git`** — satisfies the core goal (authors needn't install or learn
   Git). Both libraries do this; the CLI doesn't.
2. **Remote auth is the hard part, and libgit2 is the most proven at it.** #134
   (push/pull + HTTPS/SSH credentials) is where this epic is riskiest; libgit2 has
   years of real-world auth handling (credential helpers, SSH agents) that gix's
   younger network stack doesn't match yet.
3. **Compact + ergonomic.** ~0.9 MB and 108 deps vs gix's 3.4 MB and 593 deps; and
   commits/status read cleanly.
4. **The build cost is already paid.** git2 needs a C toolchain (cmake/cc) and, for
   HTTPS, OpenSSL — but Quill already compiles C (tree-sitter), so a C toolchain
   is a given. Local-only tickets (#130–#133, #135) can use
   `git2 = { default-features = false }` to drop OpenSSL/SSH entirely and only
   pull them in for the remote ticket (#134).

Pick **gix** instead only if the cmake/OpenSSL build deps or cross-compilation
become a real blocker — accepting a larger binary, more code for writes, and a
less-mature remote story.

## The thin Git seam

Put one small, operation-oriented module between Quill and git2 so the backend is
replaceable and the feature tickets code against a stable surface, not libgit2
directly. Sketch (refine as tickets land):

```rust
// src/vcs.rs — the only place that touches git2.
pub struct Vcs { /* repo root */ }

pub struct Status { pub dirty: bool, pub changed: Vec<PathBuf> }
pub struct CommitMeta { pub id: String, pub message: String, pub when: Date }

impl Vcs {
    pub fn is_repo(root: &Path) -> bool;                 // #132 gating
    pub fn init(root: &Path) -> Result<Vcs, VcsError>;   // #130 (+ writes .gitignore)
    pub fn open(root: &Path) -> Result<Vcs, VcsError>;
    pub fn commit(&self, message: &str) -> Result<String, VcsError>;   // #131: stage tracked + commit
    pub fn status(&self) -> Result<Status, VcsError>;                  // #132 / #135
    pub fn log(&self, limit: usize) -> Result<Vec<CommitMeta>, VcsError>; // #133
    pub fn diff(&self, path: &Path) -> Result<String, VcsError>;       // #135 (reuse the +/- diff renderer)
    pub fn restore(&self, path: &Path) -> Result<(), VcsError>;        // #135 (checkout path from HEAD)
    // Remote (deferred to #134): set_remote / push / pull, with auth.
}
```

`status()` returning the **changed paths** is the same capability the AI
incremental-scoping work (#90) wants as its revision delta — "which chapters
changed since HEAD" — so design it to answer that, with a committed snapshot as
the bible-as-cache's "last built from" marker.

Keep every operation opt-in and non-destructive (the epic's rule): `restore`
confirms before discarding, and remote ops never force-push or auto-resolve.
