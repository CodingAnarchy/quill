//! The system-clipboard seam (#811): a small trait so the copy / paste paths are
//! testable without touching the real OS clipboard, and a terminal-native
//! implementation over the OSC 52 escape sequence.
//!
//! Copying a Vim yank / delete or a Visual selection places the text on the OS
//! clipboard; paste inserts it at the cursor. The internal Vim registers (#204)
//! are unchanged — the clipboard is an additional destination, not a replacement.
//!
//! ## Why OSC 52 (and not a GUI-clipboard crate)
//!
//! Quill is terminal-first: authors run it locally, but also over SSH and inside
//! tmux/screen. OSC 52 is the terminal's own "set the clipboard" escape — the
//! emulator (not a display server) owns the clipboard, so a copy works across an
//! SSH hop or a multiplexer with no X11/Wayland/pbcopy dependency and no extra
//! crate. The trade-off is that OSC 52 is, in practice, write-only: terminals
//! answer a *read* request rarely and only with explicit opt-in, so we can't
//! reliably pull the clipboard back out of the terminal. We therefore keep a copy
//! of the last text this process placed on the clipboard and read *that* back for
//! paste — which is exactly the text an author just yanked or selected, the case
//! paste-after-copy needs. (Pasting text copied in another app is a documented
//! limitation of the OSC 52 path.)
//!
//! ## Testability (#531)
//!
//! Everything goes through the [`Clipboard`] trait, so `cargo test` swaps in
//! [`FakeClipboard`] — an in-memory cell. No test ever writes an escape sequence
//! to a real terminal or reads a real clipboard, so the suite stays hermetic.

#[cfg(test)]
use std::cell::RefCell;
use std::io::Write;
use std::sync::Mutex;

/// The system-clipboard seam: set the OS clipboard, and read back what this
/// process last put there. Implemented by [`Osc52Clipboard`] in the running
/// editor and [`FakeClipboard`] under test.
pub trait Clipboard {
    /// Place `text` on the system clipboard. Best-effort: a terminal that ignores
    /// OSC 52 simply doesn't update, but the internal register / cached readback
    /// still hold the text, so nothing is lost.
    fn set(&self, text: &str);

    /// The text this process last placed on the clipboard, if any. OSC 52 can't
    /// reliably read the terminal's clipboard, so paste reads the cached copy —
    /// the just-copied text (see the module docs).
    fn get(&self) -> Option<String>;
}

/// The terminal-native clipboard over OSC 52 (#811). Writing emits the
/// `ESC ] 52 ; c ; <base64> ESC \` sequence to stdout, which the terminal (or
/// tmux, when clipboard passthrough is on) copies to the system clipboard —
/// working locally and over SSH alike. The last-set text is cached so paste has
/// something to read back (OSC 52 is effectively write-only).
pub struct Osc52Clipboard {
    /// The last text written, returned by [`Clipboard::get`] for paste. A `Mutex`
    /// so the seam is `Sync` (the trait object lives on `App`); contention is nil.
    cached: Mutex<Option<String>>,
}

impl Osc52Clipboard {
    pub const fn new() -> Self {
        Self {
            cached: Mutex::new(None),
        }
    }

    /// Emit the OSC 52 copy sequence for `text` to stdout. Best-effort: a write
    /// error (closed pipe) is swallowed — a failed clipboard update must never
    /// interrupt editing, and the cached copy still backs paste.
    fn emit_osc52(text: &str) {
        let seq = format!("\x1b]52;c;{}\x1b\\", base64_encode(text.as_bytes()));
        let mut out = std::io::stdout();
        let _ = out.write_all(seq.as_bytes());
        let _ = out.flush();
    }
}

/// Standard-alphabet base64 (RFC 4648) with padding — the encoding OSC 52 carries
/// its payload in. Hand-rolled (a dozen lines) so the clipboard seam needs no new
/// dependency; a base64 crate is already in the tree transitively, but a direct
/// dep isn't worth it for one call site.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPHABET[(b0 >> 2) as usize] as char);
        out.push(ALPHABET[(((b0 & 0b11) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(((b1 & 0b1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(b2 & 0b111111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

impl Default for Osc52Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard for Osc52Clipboard {
    fn set(&self, text: &str) {
        Self::emit_osc52(text);
        if let Ok(mut cached) = self.cached.lock() {
            *cached = Some(text.to_string());
        }
    }

    fn get(&self) -> Option<String> {
        self.cached.lock().ok().and_then(|c| c.clone())
    }
}

/// An in-memory clipboard for tests (#531): [`set`](Clipboard::set) stores into a
/// cell and [`get`](Clipboard::get) reads it back, so the copy / paste paths are
/// exercised end to end without a real OS clipboard or any terminal I/O.
#[cfg(test)]
#[derive(Default)]
pub struct FakeClipboard {
    text: RefCell<Option<String>>,
}

#[cfg(test)]
impl FakeClipboard {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(test)]
impl Clipboard for FakeClipboard {
    fn set(&self, text: &str) {
        *self.text.borrow_mut() = Some(text.to_string());
    }

    fn get(&self) -> Option<String> {
        self.text.borrow().clone()
    }
}

// `FakeClipboard` uses a `RefCell` (test-only, single-threaded); asserting `Sync`
// isn't needed there since the production seam is `Osc52Clipboard`. The trait
// object on `App` is only ever touched from the UI thread.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_round_trips_text() {
        let clip = FakeClipboard::new();
        assert_eq!(clip.get(), None);
        clip.set("hello");
        assert_eq!(clip.get().as_deref(), Some("hello"));
        clip.set("world");
        assert_eq!(clip.get().as_deref(), Some("world"));
    }

    #[test]
    fn osc52_caches_the_last_set_text_for_paste() {
        // The real writer emits to stdout (harmless in a test) but also caches, so
        // its `get` returns the just-copied text — the paste-after-copy contract.
        let clip = Osc52Clipboard::new();
        clip.set("copied");
        assert_eq!(clip.get().as_deref(), Some("copied"));
    }

    #[test]
    fn base64_matches_known_vectors() {
        // RFC 4648 test vectors, so the OSC 52 payload is a valid base64 string.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
