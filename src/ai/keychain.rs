//! OS keychain storage for the AI provider API keys (#530).
//!
//! Quill used to keep the Claude (`[ai] api_key`) and fal.ai (`[image] api_key`)
//! keys verbatim in `~/.config/quill/config.toml`. That file is routinely synced
//! to a dotfiles repo, copied into a backup, or read on a shared machine, so a
//! casual `cat`/`grep` (or an accidental commit) handed over live, billable
//! credentials. We now store the secret in the OS-managed store — the macOS
//! Keychain, the Linux Secret Service, or the Windows Credential Manager — so it
//! never sits in the synced config at all.
//!
//! ## Precedence (see [`super::resolved_key`])
//! A key is resolved env var → keychain → plaintext `api_key`. The env var stays
//! the headless / CI escape hatch (no login session to unlock the store), and the
//! plaintext field keeps existing setups working and gives a migration path: the
//! set-key command writes the keychain and offers to strip the now-redundant
//! plaintext value from the file.
//!
//! ## Threat model
//! This protects against *accidental disclosure* — a key leaking through a synced
//! dotfile, a backup, or a casual read of the config. It does **not** protect
//! against a local attacker who can already run Quill (or any process) as the
//! user: the OS unlocks the store for the logged-in session, so any such process
//! can read it, exactly as the user can.
//!
//! ## Test hermeticity (#531)
//! The real keyring is **never** touched in test builds: every read/write is
//! gated behind `#[cfg(not(test))]` and compiles to a no-op `None`/error under
//! `cfg(test)`, mirroring the config-dir override and the test-build env gate.
//! With no env var and no keychain, [`super::Settings::resolved_key`] returns
//! `None` by default in the suite. The precedence logic itself is pure
//! ([`resolve`]) and unit-tested without any store.

/// The keyring service name under which Quill's keys are filed. Stable — changing
/// it would orphan already-stored keys. Only read by the real-keyring code path,
/// which is compiled out of test builds (#531), so it's dead there.
#[cfg_attr(test, allow(dead_code))]
const SERVICE: &str = "quill";

/// Which provider key a keychain entry holds. The variant maps to the keyring
/// *username*, so the two keys live as separate entries under one service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// The Claude / Anthropic text key (config `[ai] api_key`, env `ANTHROPIC_API_KEY`).
    Anthropic,
    /// The fal.ai image key (config `[image] api_key`, env `FAL_KEY`).
    Fal,
}

impl Key {
    /// The keyring username this key is stored under (also its display label).
    pub fn account(self) -> &'static str {
        match self {
            Key::Anthropic => "anthropic",
            Key::Fal => "fal",
        }
    }
}

/// The single precedence rule, kept pure so it's tested without any real store:
/// env var → keychain → plaintext config. The first non-empty (trimmed) source
/// wins. Each argument is the already-resolved value from that source (`None`
/// when absent). Returns the chosen key, or `None` when every source is empty.
pub fn resolve<'a>(
    env: Option<&'a str>,
    keychain: Option<&'a str>,
    plaintext: Option<&'a str>,
) -> Option<&'a str> {
    [env, keychain, plaintext]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|k| !k.is_empty())
}

/// Read `key` from the OS keychain, or `None` when it isn't stored (or the store
/// is unavailable — a headless Linux box with no Secret Service, say, where the
/// env var is the supported path). Best-effort: any keyring error is treated as
/// "no key", never surfaced, so a missing store degrades to the env / plaintext
/// path rather than breaking AI.
///
/// In test builds this is a no-op returning `None` (#531): the suite never reads
/// the real keychain, so it can't prompt, fail in CI, or pollute the user's store.
pub fn get(key: Key) -> Option<String> {
    #[cfg(test)]
    {
        let _ = key;
        None
    }
    #[cfg(not(test))]
    {
        let entry = keyring::Entry::new(SERVICE, key.account()).ok()?;
        match entry.get_password() {
            Ok(secret) => {
                let secret = secret.trim();
                (!secret.is_empty()).then(|| secret.to_string())
            }
            Err(_) => None,
        }
    }
}

/// Store `secret` for `key` in the OS keychain, replacing any existing value.
/// Returns an error string the caller can surface when the store can't be written
/// (e.g. no Secret Service on a headless Linux box — there the env var is the
/// supported path).
///
/// In test builds this is a no-op that errors (#531): the suite never writes the
/// real keychain. The set-key command is exercised through the pure helpers, not
/// by hitting the store.
pub fn set(key: Key, secret: &str) -> Result<(), String> {
    #[cfg(test)]
    {
        let _ = (key, secret);
        Err("keychain is disabled in test builds".into())
    }
    #[cfg(not(test))]
    {
        let entry = keyring::Entry::new(SERVICE, key.account())
            .map_err(|e| format!("couldn't open the OS keychain: {e}"))?;
        entry
            .set_password(secret)
            .map_err(|e| format!("couldn't save to the OS keychain: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_wins_over_keychain_and_plaintext() {
        assert_eq!(resolve(Some("env"), Some("kc"), Some("plain")), Some("env"));
    }

    #[test]
    fn keychain_wins_over_plaintext_when_no_env() {
        assert_eq!(resolve(None, Some("kc"), Some("plain")), Some("kc"));
    }

    #[test]
    fn plaintext_is_the_last_resort() {
        assert_eq!(resolve(None, None, Some("plain")), Some("plain"));
    }

    #[test]
    fn all_empty_resolves_to_none() {
        assert_eq!(resolve(None, None, None), None);
        // Blank / whitespace-only sources are treated as absent and skipped.
        assert_eq!(resolve(Some("  "), Some(""), Some("\t")), None);
    }

    #[test]
    fn a_blank_higher_source_falls_through_to_the_next() {
        // An empty env var doesn't shadow a real keychain key.
        assert_eq!(resolve(Some(""), Some("kc"), None), Some("kc"));
        // An empty keychain entry falls through to the plaintext key.
        assert_eq!(resolve(None, Some("  "), Some("plain")), Some("plain"));
    }

    #[test]
    fn the_chosen_key_is_trimmed() {
        assert_eq!(resolve(None, Some("  kc  "), None), Some("kc"));
    }

    #[test]
    fn accounts_are_distinct_and_stable() {
        assert_eq!(Key::Anthropic.account(), "anthropic");
        assert_eq!(Key::Fal.account(), "fal");
    }

    #[test]
    fn get_never_touches_the_real_keychain_in_tests() {
        // The whole point of #531: a test build's keychain read is a no-op `None`,
        // so the suite can't prompt, fail in CI, or read the developer's store.
        assert_eq!(get(Key::Anthropic), None);
        assert_eq!(get(Key::Fal), None);
    }

    #[test]
    fn set_is_a_no_op_error_in_tests() {
        assert!(set(Key::Anthropic, "sk-test").is_err());
    }
}
