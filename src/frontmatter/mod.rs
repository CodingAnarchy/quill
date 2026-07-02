//! YAML front matter at the top of a chapter file: a `---`-delimited block that
//! carries chapter metadata (status, summary, …) above the prose.
//!
//! For now Quill preserves the block **verbatim** — it splits it off so the
//! editor shows only the body, and re-attaches it byte-for-byte on save, so
//! unknown keys and an author's hand formatting survive untouched. Typed access
//! to individual fields lands with the metadata tickets that build on this.

use std::borrow::Cow;

/// Split chapter-file `text` into its front matter and the body beneath it.
///
/// Front matter is recognized only by the conventional fences: the file must
/// begin with a `---` line and have a later `---` line closing the block. The
/// fence newline may be either LF (`---\n`) or CRLF (`---\r\n`), so a chapter
/// imported or edited on Windows splits the same as a native LF file. The
/// returned front matter is the verbatim text *between* the fences (each line
/// including its trailing newline, CRLF and all); `None` when there is no such
/// block, in which case the whole text is the body. A leading `---` with no
/// closing fence (e.g. a Markdown thematic break) is left in the body.
pub fn split(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = strip_open_fence(text) else {
        return (None, text);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if bare(line) == "---" {
            let inner = &rest[..offset];
            let body = &rest[offset + line.len()..];
            return (Some(inner), body);
        }
        offset += line.len();
    }
    // Opening fence but no closing one: not front matter, leave it in the body.
    (None, text)
}

/// Strip the opening `---` fence and its newline, tolerating an optional `\r`
/// (so `---\r\n` is accepted alongside `---\n`); `None` when `text` doesn't
/// open with a fence line.
fn strip_open_fence(text: &str) -> Option<&str> {
    text.strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
}

/// Re-attach a front matter block above `body`, inverting [`split`]. With no
/// front matter the body is returned borrowed (no copy); `inner` is expected to
/// already end in a newline (as [`split`] returns it) so the closing fence sits
/// on its own line. The fences match the block's own line ending — CRLF when the
/// split-off block used CRLF — so a CRLF chapter round-trips byte-for-byte just
/// like an LF one.
pub fn join<'a>(front: Option<&str>, body: &'a str) -> Cow<'a, str> {
    match front {
        Some(inner) => {
            let nl = fence_newline(inner, body);
            Cow::Owned(format!("---{nl}{inner}---{nl}{body}"))
        }
        None => Cow::Borrowed(body),
    }
}

/// The newline the reconstructed fences should use: CRLF when the chapter uses
/// CRLF line endings, else LF, mirroring what [`split`] saw so a conforming file
/// round-trips byte-for-byte (a CRLF chapter's fences come back as CRLF, an LF
/// chapter's stay LF). The body is the authority — it carries the bulk of the
/// chapter's convention — and the block is the fallback for a body-less chapter
/// (a stub with metadata but no prose yet); LF is the final default.
fn fence_newline(inner: &str, body: &str) -> &'static str {
    let crlf = uses_crlf(body)
        .or_else(|| uses_crlf(inner))
        .unwrap_or(false);
    if crlf {
        "\r\n"
    } else {
        "\n"
    }
}

/// Whether `text`'s first line ends in CRLF (`Some(true)`) or LF (`Some(false)`),
/// or `None` when it has no line ending to judge by.
fn uses_crlf(text: &str) -> Option<bool> {
    let line = text.split_inclusive('\n').next()?;
    line.ends_with('\n').then(|| line.ends_with("\r\n"))
}

/// An epigraph: a quotation with an optional attribution, shown above a book or
/// chapter body. Stored as two keys (`epigraph` / `epigraph_by`) in front
/// matter, or two manifest fields for the book. The quotation may be multi-line
/// (verse) — a chapter stores it as a YAML `|` literal block, the book as a TOML
/// `"""` block. The attribution is a single line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Epigraph {
    pub text: String,
    pub attribution: Option<String>,
}

impl Epigraph {
    /// Build an epigraph from a quotation and optional attribution, treating
    /// blank strings as absent. `None` when there's no quotation — an
    /// attribution alone is not an epigraph. Shared by the chapter (front
    /// matter) and book (manifest) accessors.
    pub fn from_parts(text: Option<String>, attribution: Option<String>) -> Option<Epigraph> {
        let text = text.filter(|s| !s.is_empty())?;
        Some(Epigraph {
            text,
            attribution: attribution.filter(|s| !s.is_empty()),
        })
    }
}

/// Declare the [`Status`] enum and its accessors from one table (#435): each
/// variant's `status:` key, status-bar label, and pane badge live on a single
/// line, so adding a status can't leave an accessor — or the `from_key` parse —
/// silently behind. [`Status::next`] cycles in the declaration order below.
macro_rules! status_table {
    ($($variant:ident => $key:literal, $label:literal, $badge:literal);+ $(;)?) => {
        /// A chapter's revision status, cycled by the author and shown in the pane.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Status { $($variant),+ }

        impl Status {
            /// The `status:` value as written in front matter.
            pub fn key(self) -> &'static str {
                match self { $(Status::$variant => $key),+ }
            }

            /// Parse a front-matter `status:` value (case-insensitive).
            pub fn from_key(s: &str) -> Option<Status> {
                match s.trim().to_ascii_lowercase().as_str() {
                    $($key => Some(Status::$variant),)+
                    _ => None,
                }
            }

            /// Human label for the status bar.
            pub fn label(self) -> &'static str {
                match self { $(Status::$variant => $label),+ }
            }

            /// Single-character badge for the chapter pane.
            pub fn badge(self) -> char {
                match self { $(Status::$variant => $badge),+ }
            }

            /// The next status in the cycle (the declaration order below,
            /// wrapping back to the first): draft → revised → final → draft.
            pub fn next(self) -> Status {
                const ALL: &[Status] = &[$(Status::$variant),+];
                let i = ALL.iter().position(|&s| s == self).unwrap_or(0);
                ALL[(i + 1) % ALL.len()]
            }
        }
    };
}

status_table! {
    Draft => "draft", "Draft", 'D';
    Revised => "revised", "Revised", 'R';
    Final => "final", "Final", 'F';
}

/// Declare the [`SceneStatus`] enum and its accessors from one table, mirroring
/// [`status_table`] (#636): each variant's `status:` value, its corkboard label,
/// and a single-character badge on one line. [`SceneStatus::next`] cycles in the
/// declaration order (stub → draft → done → stub), and [`SceneStatus::from_key`]
/// parses the value case-insensitively.
macro_rules! scene_status_table {
    ($($variant:ident => $key:literal, $label:literal, $badge:literal);+ $(;)?) => {
        /// A single scene's drafting status (#636), edited from the corkboard and
        /// stored per scene in the chapter's `scenes:` front matter. Distinct from
        /// the whole-chapter [`Status`]: a chapter holds many scenes, each at its
        /// own stage.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
        pub enum SceneStatus {
            #[default]
            $($variant),+
        }

        impl SceneStatus {
            /// The value as written in the `scenes:` front matter.
            pub fn key(self) -> &'static str {
                match self { $(SceneStatus::$variant => $key),+ }
            }

            /// Parse a scene `status:` value (case-insensitive); `None` when unknown.
            pub fn from_key(s: &str) -> Option<SceneStatus> {
                match s.trim().to_ascii_lowercase().as_str() {
                    $($key => Some(SceneStatus::$variant),)+
                    _ => None,
                }
            }

            /// Human label for the corkboard card.
            pub fn label(self) -> &'static str {
                match self { $(SceneStatus::$variant => $label),+ }
            }

            /// Single-character badge for the card.
            pub fn badge(self) -> char {
                match self { $(SceneStatus::$variant => $badge),+ }
            }

            /// The next status in the cycle (declaration order, wrapping).
            pub fn next(self) -> SceneStatus {
                const ALL: &[SceneStatus] = &[$(SceneStatus::$variant),+];
                let i = ALL.iter().position(|&s| s == self).unwrap_or(0);
                ALL[(i + 1) % ALL.len()]
            }
        }
    };
}

scene_status_table! {
    Stub => "stub", "Stub", '·';
    Draft => "draft", "Draft", '◐';
    Done => "done", "Done", '●';
}

/// One scene's corkboard metadata (#636): a one-line synopsis and a drafting
/// status, aligned positionally with the chapter body's headings in document
/// order. Stored as an ordered `scenes:` sequence in the chapter's front matter,
/// so it stays out of the prose.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneMeta {
    pub synopsis: String,
    pub status: SceneStatus,
}

impl SceneMeta {
    /// Whether this entry carries nothing worth persisting — an empty synopsis at
    /// the default status. A trailing run of these is trimmed before writing so the
    /// block doesn't accumulate empty rows for scenes the author never annotated.
    fn is_blank(&self) -> bool {
        self.synopsis.is_empty() && self.status == SceneStatus::default()
    }
}

/// Declare the [`Role`] enum and its accessors from one table (#435): each
/// variant's `role:` key (plus any accepted alias spellings), label, and
/// ordering band live on a single line, so adding a section role can't parse
/// without a label or sort into the wrong band by a missed `match` arm. A body
/// chapter has no role (`None`), so existing projects — which have no `role:`
/// key — read as plain chapters (#179).
macro_rules! role_table {
    ($($variant:ident => $key:literal $(| $alias:literal)*, $label:literal, $band:ident, $epub_type:literal);+ $(;)?) => {
        /// A project file's **section role** (#179): a front- or back-matter
        /// section type for the non-chapter parts of a book.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Role { $($variant),+ }

        impl Role {
            /// Which ordering band this section sits in (#180).
            pub fn band(self) -> Band {
                match self { $(Role::$variant => Band::$band),+ }
            }

            /// The EPUB 3 structural `epub:type` for this section (#695), so the
            /// EPUB writer can tag the section's `<body>` and its landmark with the
            /// precise semantic (`foreword`, `dedication`, `glossary`, …) rather
            /// than the generic front/back-matter band. `None` for a role with no
            /// distinct term in the EPUB Structural Semantics Vocabulary (the
            /// writer falls back to the band's generic `frontmatter`/`backmatter`);
            /// every `Some` value is a term epubcheck/ACE recognize.
            pub fn epub_type(self) -> Option<&'static str> {
                match self { $(Role::$variant => none_if_empty($epub_type)),+ }
            }

            /// The `role:` value as written in front matter.
            pub fn key(self) -> &'static str {
                match self { $(Role::$variant => $key),+ }
            }

            /// Parse a front-matter `role:` value (case-insensitive). Accepts the
            /// British "acknowledgements" spelling too. `None` for an unknown
            /// value (read as a plain body chapter).
            pub fn from_key(s: &str) -> Option<Role> {
                match s.trim().to_ascii_lowercase().as_str() {
                    $($key $(| $alias)* => Some(Role::$variant),)+
                    _ => None,
                }
            }

            /// Capitalized label for the chapter pane and messages.
            pub fn label(self) -> &'static str {
                match self { $(Role::$variant => $label),+ }
            }
        }
    };
}

role_table! {
    // Front matter (sorts before the body), in conventional reading order. The
    // final column is the EPUB Structural Semantics term (#695); an empty string
    // means "no distinct term — use the band's generic frontmatter/backmatter".
    Dedication => "dedication", "Dedication", Front, "dedication";
    Foreword => "foreword", "Foreword", Front, "foreword";
    Preface => "preface", "Preface", Front, "preface";
    Introduction => "introduction", "Introduction", Front, "introduction";
    Prologue => "prologue", "Prologue", Front, "prologue";
    // Back matter (sorts after the body), in conventional reading order.
    Epilogue => "epilogue", "Epilogue", Back, "epilogue";
    Afterword => "afterword", "Afterword", Back, "afterword";
    Acknowledgments => "acknowledgments" | "acknowledgements", "Acknowledgments", Back, "acknowledgments";
    Appendix => "appendix", "Appendix", Back, "appendix";
    Glossary => "glossary", "Glossary", Back, "glossary";
    // "also by" and "about the author" have no distinct EPUB term — they keep the
    // generic backmatter semantic.
    AlsoBy => "also-by" | "also by the author", "Also by the Author", Back, "";
    AboutTheAuthor => "about-the-author" | "about the author", "About the Author", Back, "";
    Colophon => "colophon", "Colophon", Back, "colophon";
}

/// `None` for an empty string, else `Some(s)` — lets a [`role_table!`] row leave
/// the EPUB-type column blank to mean "no distinct term, use the band default".
const fn none_if_empty(s: &'static str) -> Option<&'static str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// The three ordering bands of a manuscript (#180): front matter sorts before
/// the body, which sorts before back matter. Declared in that order so the
/// derived `Ord` sorts chapters into reading order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Band {
    Front,
    Body,
    Back,
}

impl Band {
    /// The band a file belongs to: a role's band, or `Body` when it has no role.
    pub fn of(role: Option<Role>) -> Band {
        match role {
            Some(r) => r.band(),
            None => Band::Body,
        }
    }
}

/// A chapter's front matter, held as the verbatim block between the `---`
/// fences. Reads and writes are **surgical**: a write replaces (or appends) only
/// the one key's entry, so unknown keys and an author's hand formatting are left
/// exactly as they were — preserving the round-trip guarantee from the
/// foundation while still giving typed access to the fields Quill manages.
///
/// Scope: top-level keys whose value is a scalar or a single-level literal
/// (`|`) block (the metadata Quill writes — e.g. a multi-line epigraph). Nested
/// mappings and sequences are preserved verbatim but not interpreted; if a
/// future field needs those, this is where a real parse-edit-reserialize layer
/// would go.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrontMatter {
    raw: String,
}

impl FrontMatter {
    /// Wrap the verbatim block text (between the fences) returned by [`split`].
    pub fn from_raw(inner: &str) -> Self {
        Self {
            raw: inner.to_string(),
        }
    }

    /// The verbatim block, for [`join`].
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Whether the block has no content (no keys to write).
    pub fn is_empty(&self) -> bool {
        self.raw.trim().is_empty()
    }

    /// The value of a top-level `key:`. A plain scalar is unquoted; a `|`/`>`
    /// literal block is de-indented and its lines joined with `\n`. `None` if
    /// absent.
    fn get(&self, key: &str) -> Option<String> {
        let prefix = format!("{key}:");
        let mut lines = self.raw.lines();
        let value = lines.find_map(|line| line.strip_prefix(&prefix))?;
        let value = value.trim();
        if value.starts_with(['|', '>']) {
            // Literal block: gather the indented / blank continuation lines.
            let mut out: Vec<String> = Vec::new();
            for line in lines {
                if line.is_empty() {
                    out.push(String::new());
                } else if line.starts_with(' ') {
                    out.push(line.trim_start().to_string());
                } else {
                    break; // a column-0 line ends the block
                }
            }
            while out.last().is_some_and(|l| l.is_empty()) {
                out.pop(); // a `|` block clips trailing blank lines
            }
            Some(out.join("\n"))
        } else {
            Some(unquote(value))
        }
    }

    /// Write `key`: a single line for a scalar, a `|` literal block for a
    /// multi-line value. Replaces the key's existing entry in place (or appends).
    fn set(&mut self, key: &str, value: &str) {
        let entry = if value.contains('\n') {
            let mut block = format!("{key}: |\n");
            for line in value.split('\n') {
                if line.is_empty() {
                    block.push('\n');
                } else {
                    block.push_str(BLOCK_INDENT);
                    block.push_str(line);
                    block.push('\n');
                }
            }
            block
        } else {
            format!("{key}: {}\n", quote_scalar(value))
        };
        self.replace_key(key, Some(&entry));
    }

    /// Drop `key`'s entry (the `key:` line and, for a block, its continuation),
    /// leaving every other key untouched.
    fn remove(&mut self, key: &str) {
        self.replace_key(key, None);
    }

    /// Splice `key`'s full entry — a scalar line, or a `|` block's `key:` line
    /// plus its indented/blank continuation — replacing it with `replacement`
    /// (or removing it when `None`); appends when the key is absent and a
    /// replacement is given.
    fn replace_key(&mut self, key: &str, replacement: Option<&str>) {
        let prefix = format!("{key}:");
        let segments: Vec<&str> = self.raw.split_inclusive('\n').collect();
        let mut out = String::with_capacity(self.raw.len());
        let mut i = 0;
        let mut done = false;
        while i < segments.len() {
            let seg = segments[i];
            if !done {
                if let Some(value) = bare(seg).strip_prefix(&prefix) {
                    // Found it: skip this entry's whole span.
                    let mut end = i + 1;
                    if value.trim().starts_with(['|', '>']) {
                        while end < segments.len() && is_block_continuation(segments[end]) {
                            end += 1;
                        }
                    }
                    if let Some(rep) = replacement {
                        out.push_str(rep);
                    }
                    i = end;
                    done = true;
                    continue;
                }
            }
            out.push_str(seg);
            i += 1;
        }
        if !done {
            if let Some(rep) = replacement {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(rep);
            }
        }
        self.raw = out;
    }

    /// Set a free-text field, removing the key when the value is blank so the
    /// block doesn't accumulate empty entries.
    fn put_text(&mut self, key: &str, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            self.remove(key);
        } else {
            self.set(key, value);
        }
    }

    pub fn status(&self) -> Option<Status> {
        self.get("status").as_deref().and_then(Status::from_key)
    }

    pub fn set_status(&mut self, status: Status) {
        self.set("status", status.key());
    }

    pub fn summary(&self) -> Option<String> {
        self.get("summary").filter(|s| !s.is_empty())
    }

    pub fn set_summary(&mut self, summary: &str) {
        self.put_text("summary", summary);
    }

    pub fn pov(&self) -> Option<String> {
        self.get("pov").filter(|s| !s.is_empty())
    }

    pub fn set_pov(&mut self, pov: &str) {
        self.put_text("pov", pov);
    }

    /// The chapter's word-count goal, if set (#146). A non-positive or
    /// unparseable value reads as "no goal".
    pub fn goal(&self) -> Option<usize> {
        self.get("goal")
            .and_then(|v| v.trim().parse().ok())
            .filter(|&n| n > 0)
    }

    /// Set the chapter's word-count goal; `None` clears it.
    pub fn set_goal(&mut self, goal: Option<usize>) {
        match goal {
            Some(n) => self.set("goal", &n.to_string()),
            None => self.remove("goal"),
        }
    }

    /// The file's section role (#179), if set; `None` is a plain body chapter.
    pub fn role(&self) -> Option<Role> {
        self.get("role").as_deref().and_then(Role::from_key)
    }

    /// Set the section role; `None` clears it back to a body chapter.
    pub fn set_role(&mut self, role: Option<Role>) {
        match role {
            Some(r) => self.set("role", r.key()),
            None => self.remove("role"),
        }
    }

    /// The chapter epigraph, if a quotation is set. The attribution is optional.
    pub fn epigraph(&self) -> Option<Epigraph> {
        Epigraph::from_parts(self.get("epigraph"), self.get("epigraph_by"))
    }

    pub fn set_epigraph_text(&mut self, text: &str) {
        self.put_text("epigraph", text);
    }

    pub fn set_epigraph_by(&mut self, attribution: &str) {
        self.put_text("epigraph_by", attribution);
    }

    /// The per-scene corkboard metadata (#636): the ordered `scenes:` sequence,
    /// each entry a synopsis + status, aligned positionally with the chapter
    /// body's headings. Empty when there is no `scenes:` block, so an existing
    /// chapter (which has none) reads as having no scene metadata. Entries with an
    /// unknown status fall back to the default; a malformed block reads as far as
    /// it parses rather than erroring.
    pub fn scenes(&self) -> Vec<SceneMeta> {
        let mut segments = self.raw.split_inclusive('\n');
        // Advance to the `scenes:` line; the entries are its indented block.
        if !segments.any(|seg| bare(seg).trim_end() == "scenes:") {
            return Vec::new();
        }
        let mut out: Vec<SceneMeta> = Vec::new();
        for seg in segments.by_ref() {
            let line = bare(seg);
            if line.is_empty() {
                continue;
            }
            if !line.starts_with(' ') {
                break; // a column-0 line ends the block
            }
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("- ") {
                // A new sequence item; its first inline `key: value` (if any).
                out.push(SceneMeta::default());
                apply_scene_field(out.last_mut().unwrap(), rest);
            } else if let Some(scene) = out.last_mut() {
                // A continuation key of the current item (`  status: …`).
                apply_scene_field(scene, trimmed);
            }
        }
        out
    }

    /// Write the ordered `scenes:` sequence (#636), replacing any existing block.
    /// A trailing run of blank entries (empty synopsis, default status) is dropped
    /// so the block doesn't grow rows for scenes the author never annotated; when
    /// nothing is left the whole key is removed, keeping an un-annotated chapter's
    /// front matter clean.
    pub fn set_scenes(&mut self, scenes: &[SceneMeta]) {
        let mut end = scenes.len();
        while end > 0 && scenes[end - 1].is_blank() {
            end -= 1;
        }
        let scenes = &scenes[..end];
        if scenes.is_empty() {
            self.replace_scenes(None);
            return;
        }
        let mut block = String::from("scenes:\n");
        for scene in scenes {
            block.push_str(BLOCK_INDENT);
            block.push_str("- synopsis: ");
            block.push_str(&quote_scalar(&scene.synopsis));
            block.push('\n');
            block.push_str(BLOCK_INDENT);
            block.push_str("  status: ");
            block.push_str(scene.status.key());
            block.push('\n');
        }
        self.replace_scenes(Some(&block));
    }

    /// Splice the `scenes:` sequence's full span — its `scenes:` line plus every
    /// indented/blank continuation — replacing it with `replacement` (or removing
    /// it when `None`); appends when the key is absent and a replacement is given.
    /// The sequence's continuation is indented like a `|` block, so the span rule
    /// is the same as [`replace_key`](Self::replace_key)'s block case.
    fn replace_scenes(&mut self, replacement: Option<&str>) {
        let segments: Vec<&str> = self.raw.split_inclusive('\n').collect();
        let mut out = String::with_capacity(self.raw.len());
        let mut i = 0;
        let mut done = false;
        while i < segments.len() {
            let seg = segments[i];
            if !done && bare(seg).trim_end() == "scenes:" {
                let mut end = i + 1;
                while end < segments.len() && is_block_continuation(segments[end]) {
                    end += 1;
                }
                if let Some(rep) = replacement {
                    out.push_str(rep);
                }
                i = end;
                done = true;
                continue;
            }
            out.push_str(seg);
            i += 1;
        }
        if !done {
            if let Some(rep) = replacement {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(rep);
            }
        }
        self.raw = out;
    }
}

/// Parse one `key: value` pair from a scene sequence item into `scene` (#636).
/// Recognizes `synopsis` (unquoted) and `status` (parsed, unknown ignored);
/// anything else is left alone, so an author's extra keys don't crash the read.
fn apply_scene_field(scene: &mut SceneMeta, pair: &str) {
    if let Some(v) = pair.strip_prefix("synopsis:") {
        scene.synopsis = unquote(v.trim());
    } else if let Some(v) = pair.strip_prefix("status:") {
        if let Some(s) = SceneStatus::from_key(v) {
            scene.status = s;
        }
    }
}

/// Indent for the lines of a `|` literal block scalar.
const BLOCK_INDENT: &str = "  ";

/// A line without its trailing newline (LF or CRLF), so a `---\r\n` fence line
/// compares equal to `---` just like a `---\n` one.
fn bare(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// Whether `segment` continues a block scalar: an indented line, or a blank
/// line (a stanza break within verse). A column-0 non-empty line ends the block.
fn is_block_continuation(segment: &str) -> bool {
    let line = bare(segment);
    line.is_empty() || line.starts_with(' ')
}

/// Strip surrounding YAML quotes from a scalar value, undoing the escaping
/// [`quote_scalar`] applies. Plain (unquoted) values are returned as-is.
fn unquote(value: &str) -> String {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        let mut out = String::with_capacity(value.len());
        let mut chars = value[1..value.len() - 1].chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                },
                _ => out.push(c),
            }
        }
        out
    } else if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].replace("''", "'")
    } else {
        value.to_string()
    }
}

/// Render a scalar for writing, double-quoting (and escaping) it only when a
/// plain YAML scalar wouldn't round-trip the value.
fn quote_scalar(value: &str) -> String {
    let needs_quoting = value.is_empty()
        || value.starts_with(char::is_whitespace)
        || value.ends_with(char::is_whitespace)
        || value.contains(['"', '\'', ':', '#', '\n', '\r', '\t']);
    if !needs_quoting {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_front_matter_leaves_text_as_the_body() {
        let text = "# Chapter\n\nProse here.\n";
        let (front, body) = split(text);
        assert_eq!(front, None);
        assert_eq!(body, text);
    }

    #[test]
    fn splits_a_fenced_block_from_the_body() {
        let text = "---\nstatus: draft\npov: Mara\n---\n# Chapter\n\nProse.\n";
        let (front, body) = split(text);
        assert_eq!(front, Some("status: draft\npov: Mara\n"));
        assert_eq!(body, "# Chapter\n\nProse.\n");
    }

    #[test]
    fn split_then_join_round_trips() {
        let text = "---\nstatus: draft\n---\nThe body.\n";
        let (front, body) = split(text);
        assert_eq!(&*join(front, body), text);
    }

    #[test]
    fn splits_a_crlf_fenced_block_and_round_trips() {
        // A chapter imported or edited on Windows: CRLF fences and CRLF inside
        // the block. It must split (YAML off, prose in the body) and re-join
        // byte-for-byte, never leaking the YAML into the prose.
        let text = "---\r\nstatus: draft\r\npov: Mara\r\n---\r\n# Chapter\r\n\r\nProse.\r\n";
        let (front, body) = split(text);
        assert_eq!(front, Some("status: draft\r\npov: Mara\r\n"));
        assert_eq!(body, "# Chapter\r\n\r\nProse.\r\n");
        // The YAML block does not leak into the body.
        assert!(!body.contains("status:"));
        // Save reproduces the original chapter exactly, CRLF fences and all.
        assert_eq!(&*join(front, body), text);
    }

    #[test]
    fn an_empty_crlf_block_round_trips() {
        // With no inner lines the fence newline is read from the body.
        let text = "---\r\n---\r\nBody only.\r\n";
        let (front, body) = split(text);
        assert_eq!(front, Some(""));
        assert_eq!(body, "Body only.\r\n");
        assert_eq!(&*join(front, body), text);
    }

    #[test]
    fn an_empty_block_round_trips() {
        let text = "---\n---\nBody only.\n";
        let (front, body) = split(text);
        assert_eq!(front, Some(""));
        assert_eq!(body, "Body only.\n");
        assert_eq!(&*join(front, body), text);
    }

    #[test]
    fn a_leading_rule_without_a_closing_fence_is_body() {
        // A Markdown thematic break at the top is not front matter.
        let text = "---\njust a horizontal rule, no closing fence\n";
        let (front, body) = split(text);
        assert_eq!(front, None);
        assert_eq!(body, text);
    }

    #[test]
    fn join_none_is_the_body_unchanged() {
        assert_eq!(&*join(None, "Body.\n"), "Body.\n");
    }

    #[test]
    fn set_replaces_a_key_in_place_and_leaves_others_untouched() {
        let mut fm = FrontMatter::from_raw("status: draft\npov: Mara\ncustom: keep me\n");
        fm.set_status(Status::Final);
        // The status line changed; the other keys (incl. an unknown one) are intact.
        assert_eq!(fm.status(), Some(Status::Final));
        assert_eq!(fm.pov().as_deref(), Some("Mara"));
        assert!(fm.as_str().contains("custom: keep me\n"));
        assert_eq!(fm.as_str(), "status: final\npov: Mara\ncustom: keep me\n");
    }

    #[test]
    fn set_appends_a_missing_key() {
        let mut fm = FrontMatter::default();
        fm.set_status(Status::Draft);
        fm.set_pov("Jonah");
        assert_eq!(fm.as_str(), "status: draft\npov: Jonah\n");
    }

    #[test]
    fn summary_round_trips_through_quoting() {
        // A summary with a colon must be quoted to stay valid, and read back whole.
        let mut fm = FrontMatter::default();
        fm.set_summary("The reckoning: part one");
        assert!(fm
            .as_str()
            .contains(r#"summary: "The reckoning: part one""#));
        assert_eq!(fm.summary().as_deref(), Some("The reckoning: part one"));
    }

    #[test]
    fn blank_text_removes_the_key() {
        let mut fm = FrontMatter::from_raw("summary: old\nstatus: draft\n");
        fm.set_summary("   ");
        assert_eq!(fm.summary(), None);
        assert!(!fm.as_str().contains("summary"));
        // Other keys survive the removal.
        assert_eq!(fm.status(), Some(Status::Draft));
    }

    #[test]
    fn role_round_trips_and_clears() {
        let mut fm = FrontMatter::from_raw("status: draft\n");
        assert_eq!(fm.role(), None); // absent → body chapter
        fm.set_role(Some(Role::Prologue));
        assert!(fm.as_str().contains("role: prologue\n"));
        assert_eq!(fm.role(), Some(Role::Prologue));
        assert_eq!(fm.status(), Some(Status::Draft)); // sibling untouched
                                                      // Clearing drops the key (back to a body chapter).
        fm.set_role(None);
        assert_eq!(fm.role(), None);
        assert!(!fm.as_str().contains("role"));
    }

    #[test]
    fn role_keys_parse_case_insensitively_with_the_british_alias() {
        assert_eq!(Role::from_key("Prologue"), Some(Role::Prologue));
        assert_eq!(Role::from_key("  EPILOGUE "), Some(Role::Epilogue));
        assert_eq!(
            Role::from_key("acknowledgements"),
            Some(Role::Acknowledgments)
        );
        assert_eq!(
            Role::from_key("acknowledgments"),
            Some(Role::Acknowledgments)
        );
        assert_eq!(Role::from_key("chapter"), None); // unknown → body
        assert_eq!(Role::Appendix.key(), "appendix");
        assert_eq!(Role::Appendix.label(), "Appendix");
    }

    #[test]
    fn roles_map_to_their_epub_structural_terms() {
        // Each role reaching the EPUB writer (#695) carries its precise EPUB
        // Structural Semantics term; the two roles with no distinct term fall back
        // to `None` so the writer keeps the generic front/back-matter band.
        assert_eq!(Role::Dedication.epub_type(), Some("dedication"));
        assert_eq!(Role::Foreword.epub_type(), Some("foreword"));
        assert_eq!(Role::Glossary.epub_type(), Some("glossary"));
        assert_eq!(Role::Epilogue.epub_type(), Some("epilogue"));
        assert_eq!(Role::Colophon.epub_type(), Some("colophon"));
        // No distinct EPUB term for these → the band default is used.
        assert_eq!(Role::AlsoBy.epub_type(), None);
        assert_eq!(Role::AboutTheAuthor.epub_type(), None);
    }

    #[test]
    fn front_and_back_matter_roles_parse_into_the_right_band() {
        // Dedication is a front-matter page; the "also by" / "about the author" /
        // colophon pages are back matter (#605). Each parses (with its longhand
        // aliases and case-insensitively) and reports the conventional band.
        assert_eq!(Role::from_key("Dedication"), Some(Role::Dedication));
        assert_eq!(Role::Dedication.band(), Band::Front);

        assert_eq!(Role::from_key("also-by"), Some(Role::AlsoBy));
        assert_eq!(Role::from_key("Also By The Author"), Some(Role::AlsoBy));
        assert_eq!(Role::AlsoBy.band(), Band::Back);

        assert_eq!(
            Role::from_key("about-the-author"),
            Some(Role::AboutTheAuthor)
        );
        assert_eq!(
            Role::from_key(" About the Author "),
            Some(Role::AboutTheAuthor)
        );
        assert_eq!(Role::AboutTheAuthor.band(), Band::Back);

        assert_eq!(Role::from_key("COLOPHON"), Some(Role::Colophon));
        assert_eq!(Role::Colophon.band(), Band::Back);

        // Keys and labels round-trip for the manifest and the pane.
        assert_eq!(Role::Dedication.key(), "dedication");
        assert_eq!(Role::AboutTheAuthor.label(), "About the Author");
    }

    #[test]
    fn new_roles_sort_into_reading_order_across_bands() {
        // The band a file lands in is what orders it in the compiled book: front
        // matter (dedication) before the body, back matter (also-by, about-the-
        // author, colophon) after it. A stable sort by band preserves the
        // author's within-band order, so the section-assembly picks these up.
        let roles = [
            Some(Role::AboutTheAuthor),
            Some(Role::Dedication),
            None, // a plain body chapter
            Some(Role::Colophon),
            Some(Role::AlsoBy),
        ];
        let mut order: Vec<usize> = (0..roles.len()).collect();
        order.sort_by_key(|&i| Band::of(roles[i]));
        // Dedication (front) first, body next, then the back matter in the
        // author's declared order.
        assert_eq!(order, vec![1, 2, 0, 3, 4]);
    }

    #[test]
    fn status_cycles() {
        assert_eq!(Status::Draft.next(), Status::Revised);
        assert_eq!(Status::Revised.next(), Status::Final);
        assert_eq!(Status::Final.next(), Status::Draft);
    }

    #[test]
    fn multiline_value_round_trips_as_a_literal_block() {
        let mut fm = FrontMatter::from_raw("status: draft\n");
        fm.set_epigraph_text("Line one\nLine two\nLine three");
        // Written as a `|` literal block, indented, other keys intact.
        assert_eq!(
            fm.as_str(),
            "status: draft\nepigraph: |\n  Line one\n  Line two\n  Line three\n"
        );
        assert_eq!(
            fm.epigraph().unwrap().text,
            "Line one\nLine two\nLine three"
        );
        assert_eq!(fm.status(), Some(Status::Draft));
    }

    #[test]
    fn a_block_preserves_interior_blank_lines() {
        let mut fm = FrontMatter::default();
        fm.set_epigraph_text("Roses are red\n\nViolets are blue");
        fm.set_epigraph_by("Anon"); // a key after the block stays separate
        let ep = fm.epigraph().unwrap();
        assert_eq!(ep.text, "Roses are red\n\nViolets are blue");
        assert_eq!(ep.attribution.as_deref(), Some("Anon"));
    }

    #[test]
    fn replacing_a_block_with_a_scalar_drops_the_old_continuation() {
        let mut fm = FrontMatter::default();
        fm.set_epigraph_text("Line one\nLine two");
        fm.set_status(Status::Final); // a key written after the block
                                      // Shrink the block back to a single line.
        fm.set_epigraph_text("Just one line");
        assert_eq!(fm.epigraph().unwrap().text, "Just one line");
        assert!(fm.as_str().contains("epigraph: Just one line\n"));
        assert!(!fm.as_str().contains("Line two"));
        assert_eq!(fm.status(), Some(Status::Final));
    }

    #[test]
    fn removing_a_block_drops_its_whole_span() {
        let mut fm = FrontMatter::from_raw("status: draft\n");
        fm.set_epigraph_text("A\nB\nC");
        fm.set_epigraph_text(""); // clear
        assert!(fm.epigraph().is_none());
        assert!(!fm.as_str().contains("epigraph"));
        assert_eq!(fm.status(), Some(Status::Draft)); // sibling untouched
    }

    #[test]
    fn epigraph_round_trips_quotation_and_attribution() {
        let mut fm = FrontMatter::default();
        fm.set_epigraph_text("Call me Ishmael.");
        fm.set_epigraph_by("Melville");
        let ep = fm.epigraph().unwrap();
        assert_eq!(ep.text, "Call me Ishmael.");
        assert_eq!(ep.attribution.as_deref(), Some("Melville"));
    }

    #[test]
    fn scenes_round_trip_through_an_ordered_sequence() {
        // Per-scene synopsis + status write as an ordered block and read back in
        // order, aligned positionally with the body headings (#636).
        let mut fm = FrontMatter::from_raw("status: draft\n");
        fm.set_scenes(&[
            SceneMeta {
                synopsis: "The arrival: rain".into(),
                status: SceneStatus::Done,
            },
            SceneMeta {
                synopsis: "The meeting".into(),
                status: SceneStatus::Stub,
            },
        ]);
        let read = fm.scenes();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].synopsis, "The arrival: rain");
        assert_eq!(read[0].status, SceneStatus::Done);
        assert_eq!(read[1].synopsis, "The meeting");
        assert_eq!(read[1].status, SceneStatus::Stub);
        // The sibling chapter key survives the sequence write.
        assert_eq!(fm.status(), Some(Status::Draft));
    }

    #[test]
    fn a_chapter_with_no_scenes_key_reads_as_empty() {
        let fm = FrontMatter::from_raw("status: draft\npov: Mara\n");
        assert!(fm.scenes().is_empty());
    }

    #[test]
    fn set_scenes_replaces_the_block_and_trims_trailing_blanks() {
        let mut fm = FrontMatter::default();
        fm.set_scenes(&[
            SceneMeta {
                synopsis: "One".into(),
                status: SceneStatus::Draft,
            },
            SceneMeta {
                synopsis: "Two".into(),
                status: SceneStatus::Draft,
            },
        ]);
        fm.set_status(Status::Final); // a key written after the block
                                      // Rewrite with a trailing blank entry: it's dropped, the sibling stays.
        fm.set_scenes(&[
            SceneMeta {
                synopsis: "Only one now".into(),
                status: SceneStatus::Done,
            },
            SceneMeta::default(),
        ]);
        let read = fm.scenes();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].synopsis, "Only one now");
        assert!(!fm.as_str().contains("Two"));
        assert_eq!(fm.status(), Some(Status::Final));
    }

    #[test]
    fn clearing_scenes_removes_the_whole_block() {
        let mut fm = FrontMatter::from_raw("status: draft\n");
        fm.set_scenes(&[SceneMeta {
            synopsis: "A".into(),
            status: SceneStatus::Draft,
        }]);
        // All-blank entries clear the block back out.
        fm.set_scenes(&[SceneMeta::default(), SceneMeta::default()]);
        assert!(fm.scenes().is_empty());
        assert!(!fm.as_str().contains("scenes"));
        assert_eq!(fm.status(), Some(Status::Draft));
    }

    #[test]
    fn scene_status_cycles_and_parses() {
        assert_eq!(SceneStatus::Stub.next(), SceneStatus::Draft);
        assert_eq!(SceneStatus::Draft.next(), SceneStatus::Done);
        assert_eq!(SceneStatus::Done.next(), SceneStatus::Stub);
        assert_eq!(SceneStatus::from_key("DONE"), Some(SceneStatus::Done));
        assert_eq!(SceneStatus::from_key("nonsense"), None);
        assert_eq!(SceneStatus::default(), SceneStatus::Stub);
    }

    #[test]
    fn epigraph_without_a_quotation_is_none_and_clearing_removes_keys() {
        let mut fm = FrontMatter::default();
        // Attribution alone is not an epigraph.
        fm.set_epigraph_by("Nobody");
        assert!(fm.epigraph().is_none());

        fm.set_epigraph_text("A line.");
        assert!(fm.epigraph().is_some());
        // Clearing the quotation drops the epigraph; keys are removed.
        fm.set_epigraph_text("");
        assert!(fm.epigraph().is_none());
        assert!(!fm.as_str().contains("epigraph:"));
    }
}
