//! The one error type the Swift side sees. Each case carries a message written
//! for the author (what went wrong and what to do), not a debugging string.

use std::fmt;
use std::io;

/// Why a call into the core failed.
#[derive(Debug, uniffi::Error)]
pub enum QuillError {
    /// The folder or file isn't there.
    NotFound { message: String },
    /// The project is there but can't be read as a Quill project (a damaged or
    /// newer `book.toml`, say).
    Invalid { message: String },
    /// Reading or writing failed for another reason (permissions, a full disk).
    Io { message: String },
    /// A clone, pull, or sync couldn't finish: no network, a rejected token, or
    /// a remote that isn't a Quill project. The message says which.
    Remote { message: String },
    /// A word lookup couldn't run: no API key, no network, or a rejected key.
    Lookup { message: String },
}

impl fmt::Display for QuillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (Self::NotFound { message }
        | Self::Invalid { message }
        | Self::Io { message }
        | Self::Remote { message }
        | Self::Lookup { message }) = self;
        f.write_str(message)
    }
}

impl std::error::Error for QuillError {}

impl From<io::Error> for QuillError {
    fn from(e: io::Error) -> Self {
        let message = e.to_string();
        match e.kind() {
            io::ErrorKind::NotFound => Self::NotFound { message },
            io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput => Self::Invalid { message },
            _ => Self::Io { message },
        }
    }
}
