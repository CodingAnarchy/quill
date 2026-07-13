<!--
Thanks for contributing to Quill! Please describe the change and link the issue
it closes, then run through the checklist below. See CONTRIBUTING.md for details.
-->

## What this changes

<!-- What does this PR do, and what problem does it solve? Link the issue: Closes #123 -->

## Checklist

- [ ] `cargo fmt --check` is clean (run `cargo fmt` to fix)
- [ ] `cargo test` is green
- [ ] `cargo clippy --all-targets` is clean
- [ ] Ran `cargo perf` if this touches the parse, edit, render, or project-load paths
- [ ] Added or updated tests for the changed behavior
- [ ] Followed the code conventions (imports at top of module, files under ~1,000 lines)
- [ ] If this is an AI feature, it holds the boundary — it tracks, outlines, marks, observes, or looks up, but never drafts or rewrites manuscript prose
