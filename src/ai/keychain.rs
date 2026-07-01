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
//! ([`resolve_lazy`]) and unit-tested without any store.

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

/// Resolve a key by precedence — env var → OS keychain → plaintext config, the
/// first non-empty (trimmed) source winning — reading the keychain **lazily**:
/// `keychain` is only consulted when the env var doesn't already supply a key.
/// This keeps the documented escape hatch real — a configured `ANTHROPIC_API_KEY`
/// / `FAL_KEY` resolves without ever touching (or prompting for) the OS keychain,
/// which matters for headless / CI runs with no session to unlock the store and
/// for a freshly-built dev binary, whose changed code signature makes macOS
/// re-prompt on keychain access and would otherwise hang a non-interactive run.
/// `keychain` is a thunk so the read is genuinely skipped, not just ignored; kept
/// pure (the store is injected) so the precedence is unit-tested without a store.
pub fn resolve_lazy(
    env: Option<&str>,
    keychain: impl FnOnce() -> Option<String>,
    plaintext: Option<&str>,
) -> Option<String> {
    if let Some(k) = env.map(str::trim).filter(|k| !k.is_empty()) {
        return Some(k.to_string());
    }
    if let Some(k) = keychain()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
    {
        return Some(k);
    }
    plaintext
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
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

    // The keychain source is injected as a thunk, so precedence + trimming are
    // tested without any real store (and the thunk lets us assert it's skipped).
    #[test]
    fn env_wins_over_keychain_and_plaintext() {
        let k = resolve_lazy(Some("env"), || Some("kc".into()), Some("plain"));
        assert_eq!(k.as_deref(), Some("env"));
    }

    #[test]
    fn keychain_wins_over_plaintext_when_no_env() {
        let k = resolve_lazy(None, || Some("kc".into()), Some("plain"));
        assert_eq!(k.as_deref(), Some("kc"));
    }

    #[test]
    fn plaintext_is_the_last_resort() {
        let k = resolve_lazy(None, || None, Some("plain"));
        assert_eq!(k.as_deref(), Some("plain"));
    }

    #[test]
    fn all_empty_resolves_to_none() {
        assert_eq!(resolve_lazy(None, || None, None), None);
        // Blank / whitespace-only sources are treated as absent and skipped.
        assert_eq!(
            resolve_lazy(Some("  "), || Some("".into()), Some("\t")),
            None
        );
    }

    #[test]
    fn a_blank_higher_source_falls_through_to_the_next() {
        // An empty env var doesn't shadow a real keychain key.
        let via_kc = resolve_lazy(Some(""), || Some("kc".into()), None);
        assert_eq!(via_kc.as_deref(), Some("kc"));
        // An empty keychain entry falls through to the plaintext key.
        let via_plain = resolve_lazy(None, || Some("  ".into()), Some("plain"));
        assert_eq!(via_plain.as_deref(), Some("plain"));
    }

    #[test]
    fn the_chosen_key_is_trimmed() {
        let k = resolve_lazy(None, || Some("  kc  ".into()), None);
        assert_eq!(k.as_deref(), Some("kc"));
    }

    #[test]
    fn resolve_lazy_skips_the_keychain_when_env_is_set() {
        // The escape hatch: a configured env key must resolve WITHOUT reading the
        // keychain, so headless/CI runs and freshly-built dev binaries never prompt
        // for keychain access. The thunk panics if consulted.
        let key = resolve_lazy(
            Some("env-key"),
            || panic!("keychain must not be read"),
            None,
        );
        assert_eq!(key.as_deref(), Some("env-key"));
    }

    #[test]
    fn resolve_lazy_reads_the_keychain_only_when_env_is_absent() {
        let mut reads = 0;
        let key = resolve_lazy(
            None,
            || {
                reads += 1;
                Some("kc".into())
            },
            Some("plain"),
        );
        assert_eq!(key.as_deref(), Some("kc"));
        assert_eq!(reads, 1, "the keychain is consulted exactly once");
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
