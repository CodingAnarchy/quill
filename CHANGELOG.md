# Changelog

All notable changes to Quill are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-06-29

The first version bump since the initial `0.1.0`, collecting the user-facing
features that have landed since.

### Added

- **Export to EPUB** with a `[style]` styling seam: scene-break/title/part
  ornaments, body and heading font selection, chapter-heading treatments, drop
  caps, and image ornaments.
- **Export to PDF**, including a print-ready interior for print-on-demand,
  `[style]` support, image ornaments, and full-Unicode text via embedded Type0
  fonts.
- **Export style presets** — a preset library plus a pre-export style preview.
- **Import a Word (.docx) manuscript** into a project.
- **Import a Scrivener (.scriv) project** into a project.
- **Author-defined text snippets** for fixed insertions.
- **Rename a project's folder** from within Quill.
- **Bare launch** names a new project in the projects home rather than the
  current working directory.

### Changed

- Tree-wide `rustfmt` adoption with documented conventions and a blame-ignore
  revision list.
- A round of tech-debt cleanups: de-duplicated AI pass machinery, single-source
  struct/enum/theme/config tables, shared serializers, and faster history
  persistence.

[0.2.0]: https://github.com/CodingAnarchy/quill/releases/tag/v0.2.0
