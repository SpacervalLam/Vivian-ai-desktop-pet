//! Content-addressed, immutable assets. Currently used only for bundled fonts.
//! The caller keeps its original relative URL; physical blocks can be shared.

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

static PUBLISH: Mutex<()> = Mutex::new(());

/// Store one object per SHA-256 and atomically materialize its note-local alias.
/// Only immutable application-owned assets belong here, never editable documents.
pub fn materialize(root: &Path, target: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let _guard = PUBLISH.lock();
    materialize_locked(root, target, bytes)
}

fn materialize_locked(root: &Path, target: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let digest = format!("{:x}", Sha256::digest(bytes));
    let object = root.join(&digest[..2]).join(digest);
    // Verify existing objects too. Repair by replacing their directory entry,
    // which cannot alter older aliases that still reference a different inode.
    if std::fs::read(&object).ok().as_deref() != Some(bytes) {
        super::fs::atomic_write(&object, bytes)?;
    }
    super::fs::link_or_copy_atomic(&object, target)?;
    Ok(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_assets_reuse_content_and_preserve_independent_font_versions() {
        let dir = tempfile::tempdir().unwrap();
        let objects = dir.path().join("objects");
        let first = dir.path().join("note-a/fonts/font.woff2");
        let second = dir.path().join("note-b/fonts/font.woff2");
        let old = materialize(&objects, &first, b"font v1").unwrap();
        assert_eq!(old, materialize(&objects, &second, b"font v1").unwrap());
        // Updating one note must replace its alias, never modify shared blocks.
        let new = materialize(&objects, &first, b"font v2").unwrap();
        assert_ne!(old, new);
        assert_eq!(std::fs::read(&second).unwrap(), b"font v1");
        assert_eq!(std::fs::read(&first).unwrap(), b"font v2");
        std::fs::remove_file(&first).unwrap();
        assert_eq!(std::fs::read(old).unwrap(), b"font v1");
    }
}
