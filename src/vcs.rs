//! Version control (#54), the single seam between Quill and Git. Everything that
//! touches `git2` (libgit2) lives here, so the backend stays swappable (the spike
//! #129 chose git2 over the CLI and gitoxide; see `docs/git-integration-spike.md`).
//!
//! Version control is opt-in: a project is only a repository once the author runs
//! [`init`]. This first slice (#130) covers detecting and initializing a
//! repository with a sensible `.gitignore`; commit / status / log / diff land in
//! the later tickets through this same module.

use std::fs;
use std::path::Path;

/// Editor state and scratch that should never be committed — Quill's own
/// metadata and OS cruft. Everything else (chapter `.md` files, `book.toml`, the
/// `bible/` and `submission/` artifacts) is tracked.
const GITIGNORE: &str = "\
# Quill editor state — not part of the manuscript
/.quill/
/unused/

# OS / editor cruft
.DS_Store
*.swp
*.tmp
";

/// Whether `root` is already the top of a Git repository.
pub fn is_repo(root: &Path) -> bool {
    root.join(".git").exists()
}

/// Initialize a Git repository at `root` and write a default `.gitignore` (only
/// if one isn't already present, so a hand-tuned file is never clobbered).
/// Idempotent: re-initializing an existing repository is safe.
pub fn init(root: &Path) -> Result<(), String> {
    git2::Repository::init(root).map_err(|e| format!("git init: {e}"))?;
    let gitignore = root.join(".gitignore");
    if !gitignore.exists() {
        fs::write(&gitignore, GITIGNORE).map_err(|e| format!("write .gitignore: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("quill-vcs-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn init_creates_a_repo_and_gitignore() {
        let dir = temp_dir("init");
        assert!(!is_repo(&dir));
        init(&dir).unwrap();
        assert!(is_repo(&dir), "a .git directory should exist");
        let ignore = fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(ignore.contains("/.quill/") && ignore.contains("/unused/"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn init_is_idempotent_and_preserves_an_existing_gitignore() {
        let dir = temp_dir("idempotent");
        fs::write(dir.join(".gitignore"), "# hand-tuned\n/custom/\n").unwrap();
        init(&dir).unwrap();
        init(&dir).unwrap(); // second time is harmless
        assert!(is_repo(&dir));
        // The author's existing .gitignore is left untouched.
        assert_eq!(fs::read_to_string(dir.join(".gitignore")).unwrap(), "# hand-tuned\n/custom/\n");
        fs::remove_dir_all(&dir).ok();
    }
}
