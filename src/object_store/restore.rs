use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use super::{CacheDescriptor, ExplainAction, ObjectStore, RebuildReason};

impl ObjectStore {
    /// Restore outputs from a cache descriptor. Returns Ok(true) if restored.
    pub fn restore_from_descriptor(&self, cache_key: &str, output_paths: &[PathBuf]) -> Result<bool> {
        let Some(descriptor) = self.get_descriptor(cache_key) else { return Ok(false) };
        match descriptor {
            CacheDescriptor::Marker => Ok(true),
            CacheDescriptor::Blob { checksum, mode } => {
                let Some(output_path) = output_paths.first() else { return Ok(true) };
                // Verify content like the Tree path does: a modified or
                // corrupted output must be re-restored, not reported OK.
                if output_path.exists() {
                    if let Ok(existing) = Self::calculate_checksum(output_path)
                        && existing == checksum {
                        return Ok(true);
                    }
                    fs::remove_file(output_path)
                        .with_context(|| format!("Failed to remove stale cached file: {}", output_path.display()))?;
                }
                if !self.has_object(&checksum) {
                    return Ok(false);
                }
                if let Some(parent) = output_path.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("Failed to create output directory: {}", parent.display()))?;
                }
                self.restore_file(&checksum, output_path, mode)
                    .with_context(|| format!("Failed to restore blob to: {}", output_path.display()))?;
                Ok(true)
            }
            CacheDescriptor::Tree { entries } => {
                for entry in &entries {
                    let file_path = Path::new(&entry.path);
                    if file_path.exists() {
                        if let Ok(existing) = Self::calculate_checksum(file_path)
                            && existing == entry.checksum {
                            continue;
                        }
                        fs::remove_file(file_path)
                            .with_context(|| format!("Failed to remove stale cached file: {}", file_path.display()))?;
                    }
                    if !self.has_object(&entry.checksum) {
                        return Ok(false);
                    }
                    if let Some(parent) = file_path.parent() {
                        fs::create_dir_all(parent)
                            .with_context(|| format!("Failed to create directory for tree restore: {}", parent.display()))?;
                    }
                    self.restore_file(&entry.checksum, file_path, entry.mode)
                        .with_context(|| format!("Failed to restore tree entry: {}", file_path.display()))?;
                }
                Ok(true)
            }
        }
    }

    /// Check if a product needs rebuilding based on its descriptor.
    pub fn needs_rebuild_descriptor(&self, cache_key: &str, output_paths: &[PathBuf]) -> bool {
        let Some(descriptor) = self.get_descriptor(cache_key) else { return true };
        match descriptor {
            CacheDescriptor::Marker => false,
            CacheDescriptor::Blob { checksum, .. } => {
                // First output is content-verified against the descriptor,
                // consistent with the Tree path; extra outputs are
                // existence-checked only (their checksums aren't recorded).
                let content_ok = output_paths.first().is_none_or(|p|
                    Self::calculate_checksum(p).ok().as_ref() == Some(&checksum));
                !content_ok || output_paths.iter().skip(1).any(|p| !p.exists())
            }
            CacheDescriptor::Tree { entries } => {
                entries.iter().any(|e| {
                    let p = Path::new(&e.path);
                    !p.exists() || Self::calculate_checksum(p).ok().as_ref() != Some(&e.checksum)
                })
            }
        }
    }

    /// Check if outputs can be restored from a descriptor.
    pub fn can_restore_descriptor(&self, cache_key: &str) -> bool {
        let Some(descriptor) = self.get_descriptor(cache_key) else { return false };
        match descriptor {
            CacheDescriptor::Marker => true,
            CacheDescriptor::Blob { checksum, .. } => self.has_object(&checksum),
            CacheDescriptor::Tree { entries } => {
                entries.iter().all(|e| self.has_object(&e.checksum))
            }
        }
    }

    /// Explain what action will be taken based on descriptor state.
    pub fn explain_descriptor(&self, descriptor_key: &str, output_paths: &[PathBuf], force: bool) -> ExplainAction {
        if force {
            return ExplainAction::Rebuild(RebuildReason::Force);
        }
        let Some(descriptor) = self.get_descriptor(descriptor_key) else {
            return ExplainAction::Rebuild(RebuildReason::NoCacheEntry);
        };
        match descriptor {
            CacheDescriptor::Marker => ExplainAction::Skip,
            CacheDescriptor::Blob { checksum, .. } => {
                // Same tiered verification as needs_rebuild_descriptor: the
                // first output is content-verified (its checksum is the
                // descriptor), extras are existence-checked only — --explain
                // must never disagree with what the build would do.
                for (i, p) in output_paths.iter().enumerate() {
                    let needs_restore = if i == 0 {
                        !p.exists() || Self::calculate_checksum(p).ok().as_ref() != Some(&checksum)
                    } else {
                        !p.exists()
                    };
                    if needs_restore {
                        let display = p.display().to_string();
                        if self.has_object(&checksum) {
                            return ExplainAction::Restore(RebuildReason::OutputMissing(display));
                        }
                        return ExplainAction::Rebuild(RebuildReason::OutputMissing(display));
                    }
                }
                ExplainAction::Skip
            }
            CacheDescriptor::Tree { entries } => {
                for entry in &entries {
                    let p = Path::new(&entry.path);
                    let needs_restore = !p.exists()
                        || Self::calculate_checksum(p).ok().as_ref() != Some(&entry.checksum);
                    if needs_restore {
                        if self.has_object(&entry.checksum) {
                            return ExplainAction::Restore(RebuildReason::OutputMissing(entry.path.clone()));
                        }
                        return ExplainAction::Rebuild(RebuildReason::OutputMissing(entry.path.clone()));
                    }
                }
                ExplainAction::Skip
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_context::BuildContext;

    /// Marker descriptors are the checker fast-path: a stored PASS must
    /// never trigger a rebuild, and a missing descriptor always must.
    #[test]
    fn marker_never_rebuilds_missing_descriptor_always_does() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ObjectStore::new_in(tmp.path());

        assert!(store.needs_rebuild_descriptor("absent99", &[]));
        assert!(!store.can_restore_descriptor("absent99"));

        store.store_marker("cafe1234").unwrap();
        assert!(!store.needs_rebuild_descriptor("cafe1234", &[]));
        assert!(store.can_restore_descriptor("cafe1234"));
    }

    /// Blob verification is tiered: the first output is content-verified
    /// (its checksum is the descriptor), the remaining outputs are
    /// existence-checked only — their checksums were never recorded.
    #[test]
    fn blob_rebuild_verification_is_tiered() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ObjectStore::new_in(tmp.path());
        let ctx = BuildContext::new();
        let key = "beef5678";

        let first = tmp.path().join("first.out");
        let second = tmp.path().join("second.out");
        fs::write(&first, b"primary").unwrap();
        store.store_blob_descriptor(&ctx, key, &first).unwrap();

        let outputs = vec![first.clone(), second.clone()];
        assert!(store.needs_rebuild_descriptor(key, &outputs),
            "second output missing → rebuild");

        fs::write(&second, b"anything at all").unwrap();
        assert!(!store.needs_rebuild_descriptor(key, &outputs),
            "extra outputs are existence-checked only");

        fs::write(&first, b"tampered").unwrap();
        assert!(store.needs_rebuild_descriptor(key, &outputs),
            "first output is content-verified");
    }

    /// A corrupted output must be re-materialized from the cache, not
    /// reported OK; a descriptor whose object is gone must report false so
    /// the caller falls back to building.
    #[test]
    fn restore_replaces_corrupted_output_and_reports_missing_object() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ObjectStore::new_in(tmp.path());
        let ctx = BuildContext::new();
        let key = "feed9abc";

        let out = tmp.path().join("out.txt");
        fs::write(&out, b"cached bytes").unwrap();
        store.store_blob_descriptor(&ctx, key, &out).unwrap();

        fs::write(&out, b"corrupted").unwrap();
        assert!(store.restore_from_descriptor(key, &[out.clone()]).unwrap());
        assert_eq!(fs::read(&out).unwrap(), b"cached bytes",
            "restore must replace corrupted content with the cached bytes");

        // Remove the object behind the descriptor: restore must decline.
        let checksum = ObjectStore::calculate_checksum(&out).unwrap();
        fs::remove_file(store.object_path(&checksum)).unwrap();
        fs::remove_file(&out).unwrap();
        assert!(!store.restore_from_descriptor(key, &[out.clone()]).unwrap(),
            "no object → cannot restore, caller must build");
    }

    /// Tree descriptors content-verify every entry — corrupting any one
    /// file flags a rebuild, and restore puts the recorded bytes back.
    #[test]
    fn tree_verifies_and_restores_every_entry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ObjectStore::new_in(tmp.path());
        let ctx = BuildContext::new();
        let key = "dead4321";

        let outdir = tmp.path().join("outdir");
        fs::create_dir_all(&outdir).unwrap();
        fs::write(outdir.join("a.txt"), b"alpha").unwrap();
        fs::write(outdir.join("b.txt"), b"beta").unwrap();
        let dirs = [std::sync::Arc::new(outdir.clone())];
        store.store_tree_descriptor(&ctx, key, &dirs, &[], &|_| false).unwrap();

        assert!(!store.needs_rebuild_descriptor(key, &[]));

        fs::write(outdir.join("b.txt"), b"tampered").unwrap();
        assert!(store.needs_rebuild_descriptor(key, &[]),
            "any corrupted tree entry must flag a rebuild");

        assert!(store.restore_from_descriptor(key, &[]).unwrap());
        assert_eq!(fs::read(outdir.join("b.txt")).unwrap(), b"beta");
        assert!(!store.needs_rebuild_descriptor(key, &[]));
    }
}
