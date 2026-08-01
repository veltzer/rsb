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
    ///
    /// Written temp-then-rename, the same discipline as blobs: a reader never
    /// observes a partially-written descriptor, and rename replaces the
    /// previous read-only file without the chmod dance (which raced with
    /// concurrent writers and could leave a torn descriptor — permanently
    /// breaking `cache trim`, which fails closed on a parse error).
    pub(super) fn store_descriptor(&self, cache_key: &str, descriptor: &CacheDescriptor) -> Result<()> {
        let path = self.descriptor_path(cache_key);
        let parent = path.parent()
            .with_context(|| format!("Descriptor path has no parent: {}", path.display()))?;
        fs::create_dir_all(parent)
            .context("Failed to create descriptor directory")?;
        let data = serde_json::to_vec(descriptor)
            .context("Failed to serialize cache descriptor")?;

        let tmp_path = parent.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            super::blobs::NEXT_TMP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::write(&tmp_path, &data)
            .with_context(|| format!("Failed to write descriptor temp file: {}", tmp_path.display()))?;
        let mut perms = fs::metadata(&tmp_path)
            .with_context(|| format!("Failed to read descriptor temp file metadata: {}", tmp_path.display()))?
            .permissions();
        perms.set_readonly(true);
        let _ = fs::set_permissions(&tmp_path, perms);

        if let Err(e) = fs::rename(&tmp_path, &path) {
            let mut writable = fs::metadata(&tmp_path).map(|m| m.permissions());
            if let Ok(ref mut perms) = writable {
                perms.set_readonly(false);
                let _ = fs::set_permissions(&tmp_path, perms.clone());
            }
            let _ = fs::remove_file(&tmp_path);
            return Err(e).with_context(|| format!("Failed to move descriptor into place: {}", path.display()));
        }
        Ok(())
    }

    /// Read a cache descriptor for a cache key from the local store.
    /// Returns None if not found. Read paths that can legitimately consult
    /// the remote cache should use `get_descriptor_pulling` instead.
    pub(super) fn get_descriptor(&self, cache_key: &str) -> Option<CacheDescriptor> {
        let path = self.descriptor_path(cache_key);
        let data = fs::read(&path).ok()?;
        serde_json::from_slice(&data).ok()
    }

    /// Read a cache descriptor, falling back to the remote cache on a local
    /// miss when pull is enabled.
    ///
    /// Without this, a populated remote bucket could never satisfy a
    /// restore: every descriptor lookup was local-only, so a machine with a
    /// cold local cache always rebuilt regardless of what the remote held.
    pub(super) fn get_descriptor_pulling(
        &self,
        ctx: &crate::build_context::BuildContext,
        cache_key: &str,
    ) -> Option<CacheDescriptor> {
        if let Some(descriptor) = self.get_descriptor(cache_key) {
            return Some(descriptor);
        }
        if !self.remote_pull {
            return None;
        }
        // A remote miss degrades to a rebuild, which is always correct; only
        // a malformed descriptor is worth reporting, since it means the
        // bucket itself is bad.
        match self.try_fetch_descriptor_from_remote(ctx, cache_key) {
            Ok(descriptor) => descriptor,
            Err(e) => {
                crate::output::warn(&format!("failed to fetch descriptor from remote cache: {e}"));
                None
            }
        }
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

    /// Store a descriptor locally and, when remote push is on, publish it so
    /// another machine's pull can find it. Objects are pushed by their own
    /// call sites (they must exist before the descriptor that names them).
    fn store_and_push_descriptor(
        &self,
        ctx: &crate::build_context::BuildContext,
        cache_key: &str,
        descriptor: &CacheDescriptor,
    ) -> Result<()> {
        self.store_descriptor(cache_key, descriptor)?;
        if self.remote_push {
            let data = serde_json::to_vec(descriptor)
                .context("Failed to serialize cache descriptor for remote push")?;
            self.try_push_descriptor_to_remote(ctx, cache_key, &data)?;
        }
        Ok(())
    }

    /// Store a marker descriptor (checker passed), publishing it to the
    /// remote cache when push is enabled — the same contract as
    /// `store_blob_descriptor` and `store_tree_descriptor`.
    pub fn store_marker(&self, ctx: &crate::build_context::BuildContext, cache_key: &str) -> Result<()> {
        self.store_and_push_descriptor(ctx, cache_key, &CacheDescriptor::Marker)
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

        self.store_and_push_descriptor(ctx, cache_key, &CacheDescriptor::Blob {
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

        self.store_and_push_descriptor(ctx, cache_key, &CacheDescriptor::Tree { entries })?;
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
    /// filesystem-dependent `read_dir` order can't read as a change; content
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
    /// must take the `PermissionDenied` retry path and still succeed.
    #[test]
    fn descriptor_overwrite_survives_read_only_previous() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ObjectStore::new_in(tmp.path());
        let ctx = BuildContext::new();

        store.store_marker(&ctx, "cdcd1212").unwrap();
        store.store_marker(&ctx, "cdcd1212").unwrap();
        assert!(matches!(store.get_descriptor("cdcd1212"), Some(CacheDescriptor::Marker)));
    }
}
