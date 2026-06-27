//! Image generation (#238): the backend trait shared by the remote and (later,
//! #240) local image providers, the request shape, and the `[image]` config.
//! Mirrors the text [`Backend`](super::Backend) trait + [`Settings`](super::Settings):
//! opt-in and key-gated, the generated images saved as off-manuscript files the
//! author owns. See `docs/ai-features.md`.

use serde::Deserialize;

use super::AiError;

/// One image to generate: a text prompt and the provider's model id.
pub struct ImageRequest {
    pub prompt: String,
    pub model: String,
}

/// What an image pass produced: the saved files (project-relative paths) and a
/// reason for each one that couldn't be generated or written.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct ImageReport {
    pub saved: Vec<String>,
    pub failed: Vec<String>,
}

/// A generated image: the encoded bytes plus the file extension matching their
/// format, so it's saved as `.jpg`/`.png`/… to match the actual content rather
/// than a guessed extension.
pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    pub ext: String,
}

/// A provider that turns a prompt into an image. Implemented now by the remote
/// [`FalBackend`](super::fal::FalBackend); a local implementation lands in #240.
/// `Send + Sync` so it can run on the worker thread like the text backend.
pub trait ImageBackend: Send + Sync {
    /// Generate an image, returning its bytes and the matching file extension.
    fn generate(&self, req: &ImageRequest) -> Result<GeneratedImage, AiError>;
}

/// The file extension for an image MIME type (e.g. `image/jpeg` → `jpg`),
/// defaulting to `png` for anything unrecognized.
pub fn extension_for(content_type: &str) -> &'static str {
    match content_type.rsplit('/').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "jpeg" | "jpg" => "jpg",
        "webp" => "webp",
        "gif" => "gif",
        _ => "png",
    }
}

/// The fal.ai FLUX model used when none is configured — fast and inexpensive.
pub fn default_model() -> String {
    "fal-ai/flux/schnell".to_string()
}

/// Which image provider to generate with (#240).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ImageBackendKind {
    /// The remote fal.ai API (needs a key; sends the prompt off-machine).
    #[default]
    Remote,
    /// A local Stable-Diffusion model (runs offline; nothing leaves the machine,
    /// no per-image cost). Behind the `local` cargo feature.
    Local,
}

/// Image-generation settings from the `[image]` config section. Off by default;
/// its own opt-in and key, separate from the text `[ai]` settings.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ImageSettings {
    /// Opt-in for image generation.
    pub enabled: bool,
    /// Which provider to use (`remote` or `local`).
    pub backend: ImageBackendKind,
    /// Privacy switch: when true, force the local backend so nothing ever leaves
    /// the machine, regardless of `backend`.
    pub offline: bool,
    /// The remote provider API key (env `FAL_KEY` overrides this, see
    /// [`load`](ImageSettings::load)). Unused by the local backend.
    pub api_key: Option<String>,
    /// The remote provider model id (a fal.ai model, e.g. `fal-ai/flux/schnell`).
    pub model: String,
}

impl Default for ImageSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: ImageBackendKind::Remote,
            offline: false,
            api_key: None,
            model: default_model(),
        }
    }
}

impl ImageSettings {
    /// Load the `[image]` section from the home config file. Best-effort (a
    /// missing/unreadable file yields the defaults, off). `FAL_KEY`, if set, takes
    /// precedence over a key in the file.
    pub fn load() -> Self {
        let mut settings: ImageSettings = crate::config::load_section("image");
        if let Ok(key) = std::env::var("FAL_KEY") {
            if !key.trim().is_empty() {
                settings.api_key = Some(key);
            }
        }
        settings
    }

    /// The configured API key, if any (non-empty).
    pub fn resolved_key(&self) -> Option<&str> {
        super::resolved_key(&self.api_key)
    }

    /// The backend that will actually run, accounting for the privacy switch:
    /// `offline` forces local; otherwise the configured `backend`.
    pub fn backend(&self) -> ImageBackendKind {
        if self.offline {
            ImageBackendKind::Local
        } else {
            self.backend
        }
    }

    /// Whether image generation is usable as configured, or why not — the gate for
    /// the portrait/scene actions. Local needs no key (it loads on first use); the
    /// remote backend needs one. (Whether the local model is *compiled in* is
    /// checked when the backend is built — see `image_backend_spec`.)
    pub fn check_ready(&self) -> Result<(), AiError> {
        if !self.enabled {
            return Err(AiError::NotConfigured(
                "Image generation is off — set `enabled = true` under [image] in \
                 ~/.config/quill/config.toml"
                    .into(),
            ));
        }
        match self.backend() {
            ImageBackendKind::Local => Ok(()),
            ImageBackendKind::Remote if self.resolved_key().is_some() => Ok(()),
            ImageBackendKind::Remote => Err(AiError::NotConfigured(
                "No image API key — set FAL_KEY or `api_key` under [image] in the config \
                 (or set `backend = \"local\"`)"
                    .into(),
            )),
        }
    }
}

/// Build the image prompt for a character portrait (#238) from the bible profile:
/// the character's name plus their description (physical / voice), framed as a
/// portrait. Kept pure for testing.
pub fn portrait_prompt(name: &str, description: &str) -> String {
    let desc = description.trim();
    let mut prompt = format!("Character portrait of {name}, a person from a novel.");
    if !desc.is_empty() {
        prompt.push(' ');
        prompt.push_str(desc);
    }
    prompt.push_str(" Head-and-shoulders, detailed, book-cover illustration style.");
    prompt
}

/// Build the image prompt for a key-scene illustration (#239) from the active
/// chapter: the book + chapter titles, a scene excerpt, and the setting bible for
/// visual consistency. Kept pure for testing.
pub fn scene_prompt(book: &str, chapter: &str, excerpt: &str, setting: &str) -> String {
    let mut prompt = format!("An illustration of a scene from the novel \"{book}\", in the chapter \"{chapter}\".");
    let excerpt = excerpt.trim();
    if !excerpt.is_empty() {
        prompt.push_str(" Scene: ");
        prompt.push_str(excerpt);
    }
    let setting = setting.trim();
    if !setting.is_empty() {
        prompt.push_str(" Setting, for visual consistency: ");
        prompt.push_str(setting);
    }
    prompt.push_str(
        " Cinematic, atmospheric, book-cover illustration style; no text or lettering in the image.",
    );
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_ready_requires_enabled_and_a_key() {
        let off = ImageSettings::default();
        assert!(off.check_ready().is_err());
        let no_key = ImageSettings { enabled: true, ..Default::default() };
        assert!(no_key.check_ready().unwrap_err().to_string().contains("key"));
        let ready = ImageSettings { enabled: true, api_key: Some("k".into()), ..Default::default() };
        assert!(ready.check_ready().is_ok());
    }

    #[test]
    fn local_backend_needs_no_key() {
        let s = ImageSettings { enabled: true, backend: ImageBackendKind::Local, ..Default::default() };
        assert!(s.check_ready().is_ok(), "local needs no key");
    }

    #[test]
    fn offline_forces_local_and_needs_no_key() {
        let s = ImageSettings { enabled: true, offline: true, ..Default::default() };
        assert_eq!(s.backend(), ImageBackendKind::Local);
        assert!(s.check_ready().is_ok());
    }

    #[test]
    fn resolved_key_trims_and_rejects_blank() {
        let blank = ImageSettings { api_key: Some("   ".into()), ..Default::default() };
        assert!(blank.resolved_key().is_none());
        let set = ImageSettings { api_key: Some(" k ".into()), ..Default::default() };
        assert_eq!(set.resolved_key(), Some("k"));
    }

    #[test]
    fn portrait_prompt_includes_name_and_description() {
        let p = portrait_prompt("Mara", "green eyes, clipped speech");
        assert!(p.contains("Mara"));
        assert!(p.contains("green eyes"));
        assert!(p.contains("portrait"));
    }

    #[test]
    fn portrait_prompt_tolerates_an_empty_description() {
        let p = portrait_prompt("Mara", "  ");
        assert!(p.contains("Mara") && p.contains("portrait"));
    }

    #[test]
    fn scene_prompt_includes_titles_excerpt_and_setting() {
        let p = scene_prompt("Red State", "The Find", "Mara opens the locket", "a coastal town");
        assert!(p.contains("Red State") && p.contains("The Find"));
        assert!(p.contains("Mara opens the locket"));
        assert!(p.contains("coastal town"));
    }

    #[test]
    fn scene_prompt_omits_empty_sections() {
        let p = scene_prompt("Book", "Ch", "  ", "");
        assert!(p.contains("Book") && p.contains("illustration"));
        assert!(!p.contains("Scene:") && !p.contains("Setting"));
    }

    #[test]
    fn extension_maps_mime_types() {
        assert_eq!(extension_for("image/jpeg"), "jpg");
        assert_eq!(extension_for("image/png"), "png");
        assert_eq!(extension_for("image/webp"), "webp");
        assert_eq!(extension_for("application/octet-stream"), "png"); // fallback
    }
}
