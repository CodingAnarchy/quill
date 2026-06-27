//! Image generation (#238/#239/#247): character portraits and scene
//! illustration — prompt building, the worker spawn, and result application.
//! Extracted from `assist.rs` (#315).

use std::sync::{Arc, Mutex};

use ratatui::text::Line;

use crate::ai::fal::FalBackend;
use crate::ai::image::{self, ImageBackend, ImageReport, ImageRequest};
use crate::ai::scene_brief;
use crate::ai::{self, bible, Outcome};
use crate::mode::{Command, Mode, Palette, PaletteItem, Pending, PortraitChoice};

use super::worker::BackendSpec;
use super::App;

/// Condense prose for an image prompt (#239): drop markdown heading lines,
/// collapse runs of whitespace, and cap at `max_chars` (on a char boundary), so a
/// whole chapter becomes a compact scene description the image model can use.
pub(super) fn condense(text: &str, max_chars: usize) -> String {
    let joined = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    joined.chars().take(max_chars).collect()
}

/// The active chapter's input for a scene illustration (#247): identity, the full
/// text (to distill from), the setting bible, and a prose-excerpt fallback prompt.
pub(super) struct SceneInput {
    slug: String,
    title: String,
    text: String,
    setting: String,
    fallback_prompt: String,
}

/// Build the scene image prompt (#247): distill a visual brief via the text
/// backend when one is available, else use the prose-excerpt fallback. Runs on the
/// worker thread, so any distill failure (or no text backend) degrades quietly to
/// the fallback — the verified #239 behavior.
pub(super) fn scene_prompt_for(
    text_spec: Option<BackendSpec>,
    scene: &SceneInput,
    book: &str,
    model: &str,
) -> String {
    if let Some(spec) = text_spec {
        if let Ok(backend) = spec.build() {
            let req = scene_brief::build_request(scene.text.clone(), model);
            if let Ok(resp) = backend.complete(&req) {
                if !resp.text.trim().is_empty() {
                    return image::scene_prompt(book, &scene.title, &resp.text, &scene.setting);
                }
            }
        }
    }
    scene.fallback_prompt.clone()
}

/// Write image bytes to `path`, creating parent directories (#238). Runs on the
/// worker thread, so it returns a plain message rather than using `App`. Removes
/// any existing sibling with the same stem but a different extension first, so
/// regenerating in a different format replaces rather than accumulates (e.g. a
/// later `.jpg` doesn't leave a stale `.png` behind).
pub(super) fn save_image(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    if let (Some(parent), Some(stem)) = (path.parent(), path.file_stem()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.file_stem() == Some(stem) && p != path {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
    }
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

/// Shown when the local *image* backend is selected but wasn't compiled in (#240).
pub(super) const LOCAL_IMAGE_UNAVAILABLE: &str =
    "local image backend isn't in this build — rebuild Quill with `--features local`, or set `backend = \"remote\"` under [image]";

/// A resolved image-backend choice, built on the worker thread (#238/#240) so a
/// heavy local model load can't block the UI — mirroring [`BackendSpec`].
pub(super) enum ImageBackendSpec {
    /// The remote fal.ai backend, with its API key.
    Remote(String),
    /// The local Stable-Diffusion backend, sharing `App`'s cache slot so the
    /// loaded model is reused across requests.
    Local(Arc<Mutex<Option<Arc<dyn ImageBackend>>>>),
}

impl ImageBackendSpec {
    /// Build (or fetch the cached) image backend. Runs on the worker thread.
    fn build(self) -> Result<Arc<dyn ImageBackend>, ai::AiError> {
        match self {
            ImageBackendSpec::Remote(key) => Ok(Arc::new(FalBackend::new(key))),
            ImageBackendSpec::Local(slot) => build_local_image(slot),
        }
    }
}

/// Load the local image model once and cache it in the shared slot (#240).
#[cfg(feature = "local")]
pub(super) fn build_local_image(
    slot: Arc<Mutex<Option<Arc<dyn ImageBackend>>>>,
) -> Result<Arc<dyn ImageBackend>, ai::AiError> {
    let mut guard = slot
        .lock()
        .map_err(|_| ai::AiError::Internal("image model lock poisoned".into()))?;
    if let Some(backend) = guard.as_ref() {
        return Ok(backend.clone());
    }
    let backend: Arc<dyn ImageBackend> = Arc::new(ai::local_image::LocalImageBackend::load()?);
    *guard = Some(backend.clone());
    Ok(backend)
}

#[cfg(not(feature = "local"))]
pub(super) fn build_local_image(
    _slot: Arc<Mutex<Option<Arc<dyn ImageBackend>>>>,
) -> Result<Arc<dyn ImageBackend>, ai::AiError> {
    Err(ai::AiError::NotConfigured(LOCAL_IMAGE_UNAVAILABLE.into()))
}

impl App {
    /// Begin a character-portrait pass (#238): gate on image readiness and the
    /// character bible, then open the picker so the author chooses which
    /// character(s) to generate (#244).
    pub(crate) fn start_portrait(&mut self) {
        if let Err(e) = self.image_backend_spec() {
            self.message = Some(e.to_string());
            return;
        }
        let targets = self.character_portrait_targets();
        if targets.is_empty() {
            self.message = Some("Run the character pass first to build the character bible".into());
            return;
        }
        // Lead with "all", then one item per character. The choice carries the
        // index; the targets are recomputed (stable order) when it's confirmed.
        let mut items = vec![PaletteItem {
            label: format!("All characters ({})", targets.len()),
            desc: "a portrait for every character".into(),
            command: Command::Portrait(PortraitChoice::All),
        }];
        items.extend(
            targets
                .iter()
                .enumerate()
                .map(|(i, (_, name, _))| PaletteItem {
                    label: name.clone(),
                    desc: "a portrait for this character".into(),
                    command: Command::Portrait(PortraitChoice::One(i)),
                }),
        );
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Character portrait",
            items,
            selected: 0,
        }));
    }

    /// Confirm the picked portrait selection (#244) before spending: stash the
    /// chosen targets and open the cost confirm, whose count reflects the choice.
    pub(crate) fn confirm_portraits(&mut self, choice: PortraitChoice) {
        let targets = self.character_portrait_targets();
        let selected = match choice {
            PortraitChoice::All => targets,
            PortraitChoice::One(i) => targets
                .into_iter()
                .nth(i)
                .map(|t| vec![t])
                .unwrap_or_default(),
        };
        if selected.is_empty() {
            return;
        }
        let n = selected.len();
        let plural = if n == 1 { "" } else { "s" };
        let label = format!(
            "Generate {n} character portrait{plural} via fal.ai ({}) — each image is billed. Run? (y/N)",
            self.image.model,
        );
        self.pending_portraits = Some(selected);
        self.open_prompt(Pending::ConfirmPortraits, label, "", None);
    }

    /// Generate the picked character portraits off the UI thread (#238/#244),
    /// saved under `marketing/images/characters/`. The result is applied by
    /// [`poll_ai`](App::poll_ai). A no-op if the selection was cleared.
    pub(crate) fn generate_portraits(&mut self) {
        if let Some(targets) = self.pending_portraits.take() {
            self.spawn_image_job(
                "marketing/images/characters",
                "⟳ Generating portraits…",
                targets,
            );
        }
    }

    /// Run an image job off the UI thread (#238/#239): generate one image per
    /// `(slug, label, prompt)` target via the image backend and save each under
    /// `<subdir>/<slug>.<ext>`, reporting saved files and per-target failures. The
    /// shared worker behind the portrait and scene passes. A no-op for no targets;
    /// the busy/key guards mirror the text spawn path.
    pub(super) fn spawn_image_job(
        &mut self,
        subdir: &'static str,
        message: &str,
        targets: Vec<(String, String, String)>,
    ) {
        if targets.is_empty() {
            return;
        }
        if self.ai_task.is_busy() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        // Resolve the backend choice on the UI thread (gating + fail-fast for an
        // uncompiled local model); the heavy local load happens on the worker.
        let spec = match self.image_backend_spec() {
            Ok(spec) => spec,
            Err(e) => {
                self.message = Some(e.to_string());
                return;
            }
        };
        let model = self.image.model.clone();
        let dir = self.project.root.join(subdir);
        self.ai_task.spawn(move || {
            let backend = match spec.build() {
                Ok(b) => b,
                Err(e) => return Outcome::Image(Err(e)),
            };
            let mut report = ImageReport::default();
            for (slug, label, prompt) in targets {
                match backend.generate(&ImageRequest {
                    prompt,
                    model: model.clone(),
                }) {
                    Ok(img) => {
                        let file = format!("{slug}.{}", img.ext);
                        match save_image(&dir.join(&file), &img.bytes) {
                            Ok(()) => report.saved.push(format!("{subdir}/{file}")),
                            Err(e) => report.failed.push(format!("{label}: {e}")),
                        }
                    }
                    Err(e) => report.failed.push(format!("{label}: {e}")),
                }
            }
            Outcome::Image(Ok(report))
        });
        self.message = Some(message.to_string());
    }

    /// Resolve the configured image backend (#238/#240) for building on the worker:
    /// gate on readiness, supply the remote key, or fail fast when the local model
    /// is selected but not compiled in.
    pub(super) fn image_backend_spec(&self) -> Result<ImageBackendSpec, ai::AiError> {
        self.image.check_ready()?;
        Ok(match self.image.backend() {
            ai::image::ImageBackendKind::Remote => {
                let key = self
                    .image
                    .resolved_key()
                    .ok_or_else(|| ai::AiError::NotConfigured("no image API key".into()))?;
                ImageBackendSpec::Remote(key.to_string())
            }
            ai::image::ImageBackendKind::Local => {
                if cfg!(not(feature = "local")) {
                    return Err(ai::AiError::NotConfigured(LOCAL_IMAGE_UNAVAILABLE.into()));
                }
                ImageBackendSpec::Local(self.image_backend.clone())
            }
        })
    }

    /// The portrait targets — `(slug, name, prompt)` per character in the bible,
    /// the prompt built from that character's profile. Empty when no bible exists.
    pub(super) fn character_portrait_targets(&self) -> Vec<(String, String, String)> {
        let dir = self.project.root.join("bible").join("characters");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut files: Vec<std::path::PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("md"))
            .collect();
        files.sort();
        let mut out = Vec::new();
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let slug = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("character")
                .to_string();
            let name = bible::front_field(&text, "name")
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| slug.clone());
            let description = crate::frontmatter::split(&text).1;
            out.push((
                slug,
                name.clone(),
                image::portrait_prompt(&name, description),
            ));
        }
        out
    }

    /// Begin a scene-illustration pass (#239): gate on image readiness and the
    /// active chapter having text, then confirm before spending.
    pub(crate) fn start_scene(&mut self) {
        if let Err(e) = self.image_backend_spec() {
            self.message = Some(e.to_string());
            return;
        }
        let Some(scene) = self.active_scene() else {
            self.message = Some("The active chapter has no text to illustrate".into());
            return;
        };
        let label = format!(
            "Illustrate \"{}\" via fal.ai ({}) — the image is billed. Run? (y/N)",
            scene.title, self.image.model,
        );
        self.open_prompt(Pending::ConfirmScene, label, "", None);
    }

    /// Generate a scene illustration of the active chapter off the UI thread
    /// (#239/#247), saved under `marketing/images/scenes/`. When the text AI is
    /// ready it first distills a visual brief of the chapter's pivotal moment
    /// (#247); otherwise it falls back to the prose-excerpt prompt. Applied by
    /// [`poll_ai`](App::poll_ai).
    pub(crate) fn generate_scene(&mut self) {
        if self.ai_task.is_busy() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        let Some(scene) = self.active_scene() else {
            return;
        };
        let image_spec = match self.image_backend_spec() {
            Ok(spec) => spec,
            Err(e) => {
                self.message = Some(e.to_string());
                return;
            }
        };
        // Optional distill step: the text backend, if it's configured. None → the
        // worker uses the prose-excerpt fallback prompt.
        let text_spec = self.backend_spec().ok();
        let book = self.project.manifest.title.clone();
        let text_model = self.ai.model.clone();
        let image_model = self.image.model.clone();
        let subdir = "marketing/images/scenes";
        let dir = self.project.root.join(subdir);
        self.ai_task.spawn(move || {
            let backend = match image_spec.build() {
                Ok(b) => b,
                Err(e) => return Outcome::Image(Err(e)),
            };
            let prompt = scene_prompt_for(text_spec, &scene, &book, &text_model);
            let mut report = ImageReport::default();
            match backend.generate(&ImageRequest {
                prompt,
                model: image_model,
            }) {
                Ok(img) => {
                    let file = format!("{}.{}", scene.slug, img.ext);
                    match save_image(&dir.join(&file), &img.bytes) {
                        Ok(()) => report.saved.push(format!("{subdir}/{file}")),
                        Err(e) => report.failed.push(format!("{}: {e}", scene.title)),
                    }
                }
                Err(e) => report.failed.push(format!("{}: {e}", scene.title)),
            }
            Outcome::Image(Ok(report))
        });
        self.message = Some("⟳ Illustrating the scene…".into());
    }

    /// The active chapter's scene input (#247): its slug, title, full text, the
    /// setting bible, and a prose-excerpt fallback prompt. `None` when the chapter
    /// has no prose.
    pub(super) fn active_scene(&self) -> Option<SceneInput> {
        let idx = self.project.active_index();
        let text = self.chapter_text(idx);
        let excerpt = condense(&text, 600);
        if excerpt.is_empty() {
            return None;
        }
        let title = self
            .project
            .chapter_title(idx)
            .unwrap_or("Scene")
            .to_string();
        let slug = std::path::Path::new(self.project.chapter_file(idx))
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("scene")
            .to_string();
        let setting = condense(
            crate::frontmatter::split(&bible::raw_file(&self.project.root, "setting.md")).1,
            400,
        );
        let fallback_prompt =
            image::scene_prompt(&self.project.manifest.title, &title, &excerpt, &setting);
        Some(SceneInput {
            slug,
            title,
            text,
            setting,
            fallback_prompt,
        })
    }

    /// Apply an image pass (#238/#239): summarize the saved images and any
    /// failures. Shared by the portrait and scene passes (both `Outcome::Image`).
    pub(crate) fn apply_images(&mut self, result: Result<ImageReport, ai::AiError>) {
        self.overlay = None;
        let report = match result {
            Ok(r) => r,
            Err(e) => {
                self.message = Some(format!("AI: {e}"));
                return;
            }
        };
        if report.saved.is_empty() && report.failed.is_empty() {
            self.message = Some("No images generated".into());
            return;
        }
        let mut lines = Vec::new();
        if !report.saved.is_empty() {
            lines.push(Line::raw("Images generated:"));
            lines.push(Line::raw(""));
            lines.extend(report.saved.iter().map(|s| Line::raw(format!("  {s}"))));
            lines.push(Line::raw(""));
            lines.push(Line::raw(
                "Off-manuscript image files — yours to use and regenerate.",
            ));
        }
        if !report.failed.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::raw(format!("{} failed:", report.failed.len())));
            lines.extend(report.failed.iter().map(|f| Line::raw(format!("  {f}"))));
        }
        self.show_ai(lines);
    }
}
