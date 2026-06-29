//! Storing AI provider API keys in the OS keychain (#530), driven from the
//! run-action menu. The author enters the key in a prompt; Quill writes it to the
//! macOS Keychain / Linux Secret Service / Windows Credential Manager — never the
//! config file — then offers to strip a now-redundant plaintext `api_key` from
//! the config so the secret stops shadowing the keychain (and stops sitting in a
//! synced dotfile). Reuses the shared prompt/`Pending` machinery; the keychain
//! writes funnel through [`crate::ai::keychain`], which is a no-op in test builds.

use crate::ai::keychain::Key;
use crate::config;
use crate::mode::Pending;

use super::App;

impl App {
    /// Start storing a provider API key (#530): open a prompt for the key value.
    /// The key is written to the OS keychain on Enter, never echoed into config.
    pub(crate) fn start_set_api_key(&mut self, key: Key) {
        let label = format!("{} API key (stored in the OS keychain): ", provider(key));
        self.open_prompt(Pending::SetApiKey(key), label, "", None);
    }

    /// Write the entered key to the OS keychain and report (#530). A blank entry
    /// cancels. On success, if a plaintext `api_key` still sits in the config
    /// section, offer to strip it (it would otherwise remain as a lower-precedence
    /// fallback and keep leaking through a synced dotfile).
    pub(crate) fn store_api_key(&mut self, key: Key, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            self.message = Some("Cancelled — no key entered.".into());
            return;
        }
        match crate::ai::keychain::set(key, value) {
            Ok(()) => {
                if self.has_plaintext_key(key) {
                    let label = format!(
                        "{} key saved to the keychain. Remove the plaintext key from the config? (y/N) ",
                        provider(key)
                    );
                    self.open_prompt(Pending::StripPlaintextKey(key), label, "", None);
                } else {
                    self.message = Some(format!("{} key saved to the OS keychain.", provider(key)));
                }
            }
            // No login session / Secret Service (e.g. headless Linux): the env var
            // is the supported path there, so say so rather than failing silently.
            Err(e) => {
                self.message = Some(format!(
                    "Couldn't save to the keychain: {e}. Set {} instead.",
                    env_var(key)
                ));
            }
        }
    }

    /// Strip the now-redundant plaintext `api_key` from the config section (#530)
    /// and reload the affected settings so the in-memory copy matches the file.
    pub(crate) fn strip_plaintext_key(&mut self, key: Key) {
        match config::remove_value(section(key), "api_key") {
            Ok(true) => {
                self.reload_key_settings(key);
                self.message = Some(format!(
                    "Removed the plaintext {} key from the config.",
                    provider(key)
                ));
            }
            Ok(false) => self.message = Some("No plaintext key to remove.".into()),
            Err(e) => self.message = Some(format!("Couldn't update the config: {e}")),
        }
    }

    /// Whether the config section for `key` carries a non-empty plaintext `api_key`
    /// (the candidate for stripping after a keychain write).
    fn has_plaintext_key(&self, key: Key) -> bool {
        let plaintext = match key {
            Key::Anthropic => &self.ai.api_key,
            Key::Fal => &self.image.api_key,
        };
        plaintext
            .as_deref()
            .map(str::trim)
            .is_some_and(|k| !k.is_empty())
    }

    /// Re-read the settings whose plaintext key just changed, so the running app
    /// reflects the edited config without a restart.
    fn reload_key_settings(&mut self, key: Key) {
        match key {
            Key::Anthropic => self.ai = crate::ai::Settings::load(),
            Key::Fal => self.image = crate::ai::image::ImageSettings::load(),
        }
    }
}

/// The provider's display name for prompts and messages.
fn provider(key: Key) -> &'static str {
    match key {
        Key::Anthropic => "Claude",
        Key::Fal => "Image (fal.ai)",
    }
}

/// The config section that holds this provider's plaintext `api_key`.
fn section(key: Key) -> &'static str {
    match key {
        Key::Anthropic => "ai",
        Key::Fal => "image",
    }
}

/// The environment variable that overrides this provider's key — the headless /
/// CI escape hatch, surfaced when the keychain can't be written.
fn env_var(key: Key) -> &'static str {
    match key {
        Key::Anthropic => "ANTHROPIC_API_KEY",
        Key::Fal => "FAL_KEY",
    }
}
