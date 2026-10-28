//! Looking up a word for a slot the author selected on the phone (#1235). The
//! same deliberate, reference-only lookup the desktop offers (`ai::thesaurus`):
//! the selection and the line it sits in go to Claude, and candidates for that
//! one slot come back. The line is used only to judge fit. Nothing here can
//! return a rewritten sentence, and the author picks the word and places it.
//! Cloud only: the phone build has no on-device model.

use quill_core::ai::{keychain, thesaurus, AiError, Backend, ClaudeBackend, Settings};

use crate::QuillError;

/// One candidate for the selected slot.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct WordCandidate {
    pub text: String,
    pub definition: String,
    pub syllables: Option<u32>,
    pub origin: Option<String>,
    pub rarity: Option<String>,
    pub note: Option<String>,
}

impl From<thesaurus::Word> for WordCandidate {
    fn from(w: thesaurus::Word) -> Self {
        Self {
            text: w.text,
            definition: w.definition,
            syllables: w.syllables,
            origin: w.origin,
            rarity: w.rarity,
            note: w.note,
        }
    }
}

/// Keep `key` as the Claude API key, in the Keychain through the same store
/// the desktop uses, so both resolve the key the same way.
#[uniffi::export]
pub fn set_claude_api_key(key: &str) -> Result<(), QuillError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(QuillError::Lookup {
            message: "Paste your Claude API key first.".into(),
        });
    }
    keychain::set(keychain::Key::Anthropic, key).map_err(|message| QuillError::Lookup { message })
}

/// Whether a Claude API key is available for lookups.
#[uniffi::export]
pub fn has_claude_api_key() -> bool {
    Settings::load().resolved_key().is_some()
}

/// Candidates for `selection` (a word or short phrase), judged against `line`,
/// the line it sits in.
#[uniffi::export]
pub fn look_up_words(selection: &str, line: &str) -> Result<Vec<WordCandidate>, QuillError> {
    let selection = selection.trim();
    if selection.is_empty() || selection.split_whitespace().count() > MAX_WORDS {
        return Err(QuillError::Lookup {
            message: format!(
                "Select a word or a short phrase (up to {MAX_WORDS} words) to look up."
            ),
        });
    }
    let settings = Settings::load();
    let Some(key) = settings.resolved_key() else {
        return Err(QuillError::Lookup {
            message: "Add your Claude API key in Settings to look up words.".into(),
        });
    };
    let request = thesaurus::build_request(selection, line, &settings.model);
    let words = ClaudeBackend::new(key)
        .complete(&request)
        .and_then(|reply| thesaurus::parse(&reply.text))
        .map_err(lookup_error)?;
    Ok(words.into_iter().map(WordCandidate::from).collect())
}

/// The longest selection the lookup treats as a "slot". Anything longer is a
/// passage, and the AI boundary rules out offering a replacement for that.
const MAX_WORDS: usize = 4;

/// An author-facing message for a failed lookup.
fn lookup_error(e: AiError) -> QuillError {
    let message = match e {
        AiError::Network {
            status: Some(401 | 403),
            ..
        } => "Claude didn't accept the API key. Check it in Settings.".to_string(),
        AiError::Network { status: None, .. } => {
            "Can't reach Claude. Check your connection, then try again.".to_string()
        }
        other => format!("The lookup didn't work: {other}"),
    };
    QuillError::Lookup { message }
}

#[cfg(test)]
mod tests;
