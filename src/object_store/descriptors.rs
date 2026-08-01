use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use super::{CacheDescriptor, ObjectStore, TreeEntry, CHECKSUM_PREFIX_LEN, walk_files};

impl ObjectStore {
    pub(super) fn descriptor_path(&self, descriptor_key: &str) -> PathBuf {
        let (prefix, rest) = descriptor_key.split_at(CHECKSUM_PREFIX_LEN.min(descriptor_key.len()));
        self.descriptors_dir.join(prefix).join(rest)
    }

    /// Store a cache descriptor for a cache key.
    pub(super) fn store_descriptor(&self, cache_key: &str, descriptor: &CacheDescriptor) -> Result<()> {
        let path = self.descriptor_path(cache_key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .context("Failed to create descriptor directory")?;
        }
        let data = serde_json::to_vec(descriptor)
            .context("Failed to serialize cache descriptor")?;
        // Make writable if the file already exists (it was set read-only by a
        // previous store). This races with concurrent writers that also toggle
        // permissions, so if the first write attempt fails with PermissionDenied
        // we retry once after forcing writable.
        if let Err(first_err) = fs::write(&path, &data) {
            if first_err.kind() == std::io::ErrorKind::PermissionDenied {
                let _ = crate::platform::set_permissions_mode(&path, 0o644);
                fs::write(&path, &data)
                    .with_context(|| format!("Failed to write cache descriptor (retry): {}", path.display()))?;
            } else {
                return Err(first_err).with_context(|| format!("Failed to write cache descriptor: {}", path.display()));
            }
        }
        if let Ok(meta) = fs::metadata(&path) {
            let mut perms = meta.permissions();
            perms.set_readonly(true);
            let _ = fs::set_permissions(&path, perms);
        }
        Ok(())
    }

    /// Read a cache descriptor for a cache key. Returns None if not found.
    pub(super) fn get_descriptor(&self, cache_key: &str) -> Option<CacheDescriptor> {
        let path = self.descriptor_path(cache_key);
        let data = fs::read(&path).ok()?;
        serde_json::from_slice(&data).ok()
    }

    /// Return the list of file paths recorded in the product's last tree descriptor.
    pub fn previous_tree_paths(&self, cache_key: &str) -> Vec<PathBuf> {
        match self.get_descriptor(cache_key) {
            Some(CacheDescriptor::Tree { entries }) => {
                entries.into_iter().map(|e| PathBuf::from(e.path)).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Store a marker descriptor (checker passed).
    pub fn store_marker(&self, cache_key: &str) -> Result<()> {
        self.store_descriptor(cache_key, &CacheDescriptor::Marker)
    }

    /// Store a blob descriptor (generator produced a single output).
    pub fn store_blob_descriptor(&self, ctx: &crate::build_context::BuildContext, cache_key: &str, output_path: &Path) -> Result<bool> {
        let content = fs::read(output_path)
            .with_context(|| format!("Failed to read output: {}", output_path.display()))?;
        let checksum = self.store_object(&content)?;
        let mode = fs::metadata(output_path).ok()
            .and_then(|m| crate::platform::get_mode(&m));

        let changed = match self.get_descriptor(cache_key) {
            Some(CacheDescriptor::Blob { checksum: prev, .. }) => prev != checksum,
            _ => true,
        };

        if self.remote_push {
            self.try_push_object_to_remote(ctx, &checksum)?;
        }

        self.store_descriptor(cache_key, &CacheDescriptor::Blob {
            checksum,
            mode,
        })?;

        Ok(changed)
    }

    /// Store a tree descriptor (creator produced multiple outputs).
    pub fn store_tree_descriptor(
        &self,
        ctx: &crate::build_context::BuildContext,
        cache_key: &str,
        output_dirs: &[std::sync::Arc<PathBuf>],
        output_files: &[PathBuf],
        is_foreign: &dyn Fn(&Path) -> bool,
    ) -> Result<bool> {
        let prev = self.get_descriptor(cache_key);
        let mut entries = Vec::new();

        for dir in output_dirs {
            let dir: &Path = dir;
            anyhow::ensure!(dir.exists() && dir.is_dir(),
                "Expected output directory not produced: {}", dir.display());
            for file_path in walk_files(dir) {
                if is_foreign(&file_path) {
                    continue;
                }
                let content = fs::read(&file_path)
                    .with_context(|| format!("Failed to read: {}", file_path.display()))?;
                let checksum = self.store_object(&content)?;
                let mode = fs::metadata(&file_path).ok()
                    .and_then(|m| crate::platform::get_mode(&m));
                if self.remote_push {
                    self.try_push_object_to_remote(ctx, &checksum)?;
                }
                entries.push(TreeEntry {
                    path: file_path.display().to_string(),
                    checksum,
                    mode,
                });
            }
        }

        for file_path in output_files {
            anyhow::ensure!(file_path.exists(),
                "Expected output file not produced: {}", file_path.display());
            let content = fs::read(file_path)
                .with_context(|| format!("Failed to read: {}", file_path.display()))?;
            let checksum = self.store_object(&content)?;
            let mode = fs::metadata(file_path).ok()
                .and_then(|m| crate::platform::get_mode(&m));
            if self.remote_push {
                self.try_push_object_to_remote(ctx, &checksum)?;
            }
            entries.push(TreeEntry {
                path: Self::path_string(file_path),
                checksum,
                mode,
            });
        }

        // Canonical order: walk_files yields filesystem-dependent read_dir
        // order, so an order shift between runs must not read as a change.
        entries.sort_by(|a, b| a.path.cmp(&b.path));

        let changed = match prev {
            Some(CacheDescriptor::Tree { entries: ref prev_entries }) => {
                entries.len() != prev_entries.len()
                    || entries.iter().zip(prev_entries.iter()).any(|(a, b)| a.checksum != b.checksum || a.path != b.path)
            }
            _ => true,
        };

        self.store_descriptor(cache_key, &CacheDescriptor::Tree { entries })?;
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_context::BuildContext;

    /// Descriptor files are sharded by the key's first two characters; a
    /// degenerately short key must not panic.
    #[test]
    fn descriptor_path_shards_and_survives_short_keys() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ObjectStore::new_in(tmp.path());

        let path = store.descriptor_path("abcdef");
        assert!(path.ends_with(Path::new("ab").join("cdef")),
            "expected ab/cdef sharding, got {}", path.display());

        // One-char key: must not panic on split_at.
        let short = store.descriptor_path("a");
        assert!(short.starts_with(tmp.path()));
    }

    /// The entries are sorted before comparison precisely so that
    /// filesystem-dependent read_dir order can't read as a change; content
    /// changes still must.
    #[test]
    fn tree_change_detection_ignores_order_but_sees_content() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ObjectStore::new_in(tmp.path());
        let ctx = BuildContext::new();
        let key = "abba7777";

        let outdir = tmp.path().join("out");
        fs::create_dir_all(&outdir).unwrap();
        fs::write(outdir.join("a.txt"), b"one").unwrap();
        fs::write(outdir.join("b.txt"), b"two").unwrap();
        let dirs = [std::sync::Arc::new(outdir.clone())];

        assert!(store.store_tree_descriptor(&ctx, key, &dirs, &[], &|_| false).unwrap(),
            "first store is always a change");
        assert!(!store.store_tree_descriptor(&ctx, key, &dirs, &[], &|_| false).unwrap(),
            "identical re-store must not read as a change");

        fs::write(outdir.join("a.txt"), b"changed").unwrap();
        assert!(store.store_tree_descriptor(&ctx, key, &dirs, &[], &|_| false).unwrap(),
            "content change must be detected");
    }

    /// Descriptors are stored read-only; a second store over the same key
    /// must take the PermissionDenied retry path and still succeed.
    #[test]
    fn descriptor_overwrite_survives_read_only_previous() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ObjectStore::new_in(tmp.path());

        store.store_marker("cdcd1212").unwrap();
        store.store_marker("cdcd1212").unwrap();
        assert!(matches!(store.get_descriptor("cdcd1212"), Some(CacheDescriptor::Marker)));
    }
}
