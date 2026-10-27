//! Typed chapter metadata over the raw block: the `Epigraph`/`SceneMeta` value
//! types and the [`FrontMatter`] accessors that read and write the fields Quill
//! manages (status, summary, pov, goal, role, epigraph, scenes). Each accessor
//! is surgical — it goes through the [`super::yaml`] layer, so a write touches
//! only its own key and leaves an author's other keys and formatting untouched.
//!
//! `Epigraph` and `SceneMeta` are re-exported from [`super`], so callers still
//! use `frontmatter::{Epigraph, SceneMeta}` unchanged.

use super::block::bare;
use super::yaml::quote_scalar;
use super::yaml::unquote;
use super::yaml::BLOCK_INDENT;
use super::FrontMatter;
use super::Role;
use super::SceneStatus;
use super::Status;

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
    pub fn from_parts(text: Option<String>, attribution: Option<String>) -> Option<Self> {
        let text = text.filter(|s| !s.is_empty())?;
        Some(Self {
            text,
            attribution: attribution.filter(|s| !s.is_empty()),
        })
    }
}

/// One scene's corkboard metadata (#636): a one-line synopsis and a drafting
/// status, aligned positionally with the chapter body's headings in document
/// order. Stored as an ordered `scenes:` sequence in the chapter's front matter,
/// so it stays out of the prose.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SceneMeta {
    pub synopsis: String,
    pub status: SceneStatus,
    /// The scene's label when this entry was last written (#1240), written only
    /// for a short story. A story's scenes appear and vanish as the author types
    /// scene breaks, so its metadata is matched to scenes by this anchor first and
    /// by position only as a fallback. Empty for a novel chapter's entries.
    pub anchor: String,
}

impl SceneMeta {
    /// Whether this entry carries nothing worth persisting — an empty synopsis at
    /// the default status. A trailing run of these is trimmed before writing so the
    /// block doesn't accumulate empty rows for scenes the author never annotated.
    fn is_blank(&self) -> bool {
        self.synopsis.is_empty() && self.status == SceneStatus::default()
    }
}

impl FrontMatter {
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
        let mut segments = self.as_str().split_inclusive('\n');
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
                let mut scene = SceneMeta::default();
                apply_scene_field(&mut scene, rest);
                out.push(scene);
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
            if !scene.anchor.is_empty() {
                block.push_str(BLOCK_INDENT);
                block.push_str("  anchor: ");
                block.push_str(&quote_scalar(&scene.anchor));
                block.push('\n');
            }
        }
        self.replace_scenes(Some(&block));
    }
}

/// Parse one `key: value` pair from a scene sequence item into `scene` (#636).
/// Recognizes `synopsis` (unquoted) and `status` (parsed, unknown ignored);
/// anything else is left alone, so an author's extra keys don't crash the read.
fn apply_scene_field(scene: &mut SceneMeta, pair: &str) {
    if let Some(v) = pair.strip_prefix("synopsis:") {
        scene.synopsis = unquote(v.trim());
    } else if let Some(v) = pair.strip_prefix("anchor:") {
        scene.anchor = unquote(v.trim());
    } else if let Some(v) = pair.strip_prefix("status:") {
        if let Some(s) = SceneStatus::from_key(v) {
            scene.status = s;
        }
    }
}
