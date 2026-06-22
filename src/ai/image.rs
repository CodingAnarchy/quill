//! Image generation (#238): the backend trait shared by the remote and (later,
//! #240) local image providers, the request shape, and the `[image]` config.
//! Mirrors the text [`Backend`](super::Backend) trait + [`Settings`](super::Settings):
//! opt-in and key-gated, the generated images saved as off-manuscript files the
//! author owns. See `docs/ai-features.md`.

use serde::Deserialize;

use super::AiError;
use crate::config::config_path;

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

/// A provider that turns a prompt into image bytes. Implemented now by the remote
/// [`FalBackend`](super::fal::FalBackend); a local implementation lands in #240.
/// `Send + Sync` so it can run on the worker thread like the text backend.
pub trait ImageBackend: Send + Sync {
    /// Generate an image, returning the encoded bytes (e.g. PNG/JPEG).
    fn generate(&self, req: &ImageRequest) -> Result<Vec<u8>, AiError>;
}

/// The fal.ai FLUX model used when none is configured — fast and inexpensive.
pub fn default_model() -> String {
    "fal-ai/flux/schnell".to_string()
}

/// Image-generation settings from the `[image]` config section. Off by default;
/// its own opt-in and key, separate from the text `[ai]` settings.
#[derive(Clone, Deserialize)]
#[serde(default)]
pub struct ImageSettings {
    /// Opt-in for image generation.
    pub enabled: bool,
    /// The provider API key (env `FAL_KEY` overrides this, see [`load`](ImageSettings::load)).
    pub api_key: Option<String>,
    /// The provider model id (a fal.ai model, e.g. `fal-ai/flux/schnell`).
    pub model: String,
}

impl Default for ImageSettings {
    fn default() -> Self {
        Self { enabled: false, api_key: None, model: default_model() }
    }
}

impl ImageSettings {
    /// Load the `[image]` section from the home config file. Best-effort (a
    /// missing/unreadable file yields the defaults, off). `FAL_KEY`, if set, takes
    /// precedence over a key in the file.
    pub fn load() -> Self {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct File {
            image: ImageSettings,
        }
        let mut settings = config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| toml::from_str::<File>(&t).ok())
            .map(|f| f.image)
            .unwrap_or_default();
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

    /// Whether image generation is usable as configured, or why not — the gate for
    /// the portrait/scene actions.
    pub fn check_ready(&self) -> Result<(), AiError> {
        if !self.enabled {
            return Err(AiError::NotConfigured(
                "Image generation is off — set `enabled = true` under [image] in \
                 ~/.config/quill/config.toml"
                    .into(),
            ));
        }
        if self.resolved_key().is_none() {
            return Err(AiError::NotConfigured(
                "No image API key — set FAL_KEY or `api_key` under [image] in the config".into(),
            ));
        }
        Ok(())
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
}
