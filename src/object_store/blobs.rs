use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{ObjectStore, CHECKSUM_PREFIX_LEN};
use crate::checksum;
use crate::config::RestoreMethod;

/// Distinguishes concurrent temp files within one process; combined with the
/// pid so concurrent rsconstruct invocations never collide either.
static NEXT_TMP_ID: AtomicU64 = AtomicU64::new(0);

impl ObjectStore {
    /// Calculate SHA-256 checksum of a file by reading its contents directly.
    /// Does not use the BuildContext's in-memory cache — intended for one-off
    /// integrity checks (e.g. restore verification), not hot-path input hashing.
    pub fn calculate_checksum(file_path: &Path) -> Result<String> {
        let contents = fs::read(file_path)
            .with_context(|| format!("Failed to read file for checksum: {}", file_path.display()))?;
        Ok(Self::calculate_checksum_bytes(&contents))
    }

    /// Calculate SHA-256 checksum of bytes
    pub fn calculate_checksum_bytes(data: &[u8]) -> String {
        checksum::bytes_checksum(data)
    }

    /// Get object path for a checksum (e.g., .rsconstruct/objects/ab/cdef123...)
    pub(super) fn object_path(&self, checksum: &str) -> PathBuf {
        let (prefix, rest) = checksum.split_at(CHECKSUM_PREFIX_LEN.min(checksum.len()));
        self.objects_dir.join(prefix).join(rest)
    }

    /// Path of the zstd-compressed variant of an object. The `.zst` suffix
    /// records the storage format in the filename, so readers key off the
    /// object's actual format rather than the current `compression` setting —
    /// toggling the setting must never misread existing objects.
    pub(super) fn compressed_object_path(&self, checksum: &str) -> PathBuf {
        let mut path = self.object_path(checksum).into_os_string();
        path.push(".zst");
        PathBuf::from(path)
    }

    /// Store content in object store, returns checksum.
    /// The checksum is always computed on the **original** (uncompressed) content
    /// so cache keys remain stable regardless of compression setting.
    /// Objects are made read-only to prevent accidental modification via hardlinks.
    ///
    /// The blob is written to a unique temp file and renamed into place: a
    /// crash can never leave a truncated object at the content-addressed path,
    /// and concurrent stores of identical content don't collide.
    pub(super) fn store_object(&self, content: &[u8]) -> Result<String> {
        let checksum = Self::calculate_checksum_bytes(content);
        if self.has_object(&checksum) {
            return Ok(checksum);
        }

        let object_path = if self.compression {
            self.compressed_object_path(&checksum)
        } else {
            self.object_path(&checksum)
        };
        let parent = object_path.parent()
            .context("Object path has no parent directory")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create object directory: {}", parent.display()))?;

        let blob = if self.compression {
            zstd::encode_all(content, 0)
                .context("Failed to zstd-compress object")?
        } else {
            content.to_vec()
        };

        let tmp_path = parent.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            NEXT_TMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&tmp_path, &blob)
            .with_context(|| format!("Failed to write object temp file: {}", tmp_path.display()))?;
        let mut perms = fs::metadata(&tmp_path)
            .with_context(|| format!("Failed to read object temp file metadata: {}", tmp_path.display()))?
            .permissions();
        perms.set_readonly(true);
        fs::set_permissions(&tmp_path, perms)
            .with_context(|| format!("Failed to set object temp file read-only: {}", tmp_path.display()))?;

        if let Err(e) = fs::rename(&tmp_path, &object_path) {
            // A concurrent store may have placed the identical object first;
            // that's success. Otherwise clean up the temp file and fail.
            let mut writable = fs::metadata(&tmp_path).map(|m| m.permissions());
            if let Ok(ref mut perms) = writable {
                perms.set_readonly(false);
                let _ = fs::set_permissions(&tmp_path, perms.clone());
            }
            let _ = fs::remove_file(&tmp_path);
            if !object_path.exists() {
                return Err(e).with_context(|| format!("Failed to move object into place: {}", object_path.display()));
            }
        }

        Ok(checksum)
    }

    /// Check if an object exists in the store, in either storage format.
    pub(super) fn has_object(&self, checksum: &str) -> bool {
        self.object_path(checksum).exists() || self.compressed_object_path(checksum).exists()
    }

    /// Restore a file from the object store, applying the stored file mode.
    ///
    /// The restore path is chosen by the object's actual on-disk format, not
    /// the current `compression` setting — a compressed object is always
    /// decompressed into a fresh file. A hardlinked output shares the
    /// read-only object inode, so no mode is ever applied through a hardlink —
    /// chmodding the link would make the cache object itself writable and any
    /// later in-place write would silently corrupt it. Files whose stored mode
    /// needs exec bits are restored by copy so the mode can be applied
    /// faithfully.
    pub(super) fn restore_file(&self, checksum: &str, output_path: &Path, mode: Option<u32>) -> Result<()> {
        let object_path = self.object_path(checksum);

        if !object_path.exists() {
            // Only the compressed variant exists: decompress into the output.
            let content = self.read_object(checksum)
                .with_context(|| format!("Failed to read cached object: {checksum}"))?;
            fs::write(output_path, &content)
                .with_context(|| format!("Failed to write decompressed output: {}", output_path.display()))?;
            crate::platform::set_permissions_mode(output_path, mode.unwrap_or(0o644))
                .with_context(|| format!("Failed to set permissions on restored file: {}", output_path.display()))?;
            return Ok(());
        }

        let needs_exec = mode.is_some_and(|m| m & 0o111 != 0);
        match self.restore_method {
            RestoreMethod::Hardlink if !needs_exec => {
                fs::hard_link(&object_path, output_path)
                    .with_context(|| format!("Failed to hard link from cache: {checksum}. If on a cross-filesystem setup, set restore_method = \"copy\" in rsconstruct.toml."))?;
            }
            RestoreMethod::Hardlink | RestoreMethod::Copy => {
                fs::copy(&object_path, output_path)
                    .with_context(|| format!("Failed to copy from cache: {checksum}"))?;
                crate::platform::set_permissions_mode(output_path, mode.unwrap_or(0o644))
                    .with_context(|| format!("Failed to set permissions on restored file: {}", output_path.display()))?;
            }
            RestoreMethod::Auto => unreachable!("Auto should be resolved before use"),
        }

        Ok(())
    }

    /// Read an object's original content, decompressing if the object is
    /// stored compressed. The format is detected from which variant exists on
    /// disk, so objects written under either `compression` setting stay
    /// readable.
    pub(crate) fn read_object(&self, checksum: &str) -> Result<Vec<u8>> {
        let plain_path = self.object_path(checksum);
        if plain_path.exists() {
            return fs::read(&plain_path)
                .with_context(|| format!("Failed to read object: {checksum}"));
        }
        let compressed_path = self.compressed_object_path(checksum);
        let raw = fs::read(&compressed_path)
            .with_context(|| format!("Failed to read object: {checksum}"))?;
        zstd::decode_all(raw.as_slice())
            .with_context(|| format!("Failed to decompress object: {checksum}"))
    }

    /// Convert path to string for storage. Paths are already relative.
    pub(super) fn path_string(path: &Path) -> String {
        path.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_db(dir: &Path, db_name: &str) -> ObjectStore {
        ObjectStore::new_at(dir, db_name)
    }

    fn store_in(dir: &Path) -> ObjectStore {
        ObjectStore::new_in(dir)
    }

    /// A hardlink restore shares the object's inode: applying the stored mode
    /// through the link would make the cache object writable and let later
    /// in-place writes silently corrupt it.
    #[test]
    fn hardlink_restore_keeps_object_read_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = store_in(tmp.path());
        let checksum = store.store_object(b"cached content").unwrap();
        let out = tmp.path().join("out.txt");
        store.restore_file(&checksum, &out, Some(0o644)).unwrap();

        let obj_meta = fs::metadata(store.object_path(&checksum)).unwrap();
        assert!(obj_meta.permissions().readonly(),
            "cache object must stay read-only after a hardlink restore");
        if let Some(mode) = crate::platform::get_mode(&fs::metadata(&out).unwrap()) {
            assert_eq!(mode & 0o222, 0,
                "hardlink-restored output shares the object inode and must stay read-only");
        }
    }

    /// Objects written under one `compression` setting must stay readable and
    /// restorable after the setting is toggled: the storage format lives in
    /// the object filename, not in the config.
    #[test]
    fn compression_toggle_roundtrip() {
        let tmp = tempfile::TempDir::new().unwrap();

        let compressed_store = ObjectStore { compression: true, ..store_with_db(tmp.path(), "db1.redb") };
        let checksum = compressed_store.store_object(b"compressed at store time").unwrap();
        assert!(compressed_store.compressed_object_path(&checksum).exists());

        // Same on-disk store, compression now off
        let plain_store = store_with_db(tmp.path(), "db2.redb");
        assert!(plain_store.has_object(&checksum), "toggled store must still see the object");
        assert_eq!(plain_store.read_object(&checksum).unwrap(), b"compressed at store time",
            "read must decompress based on the object's actual format");
        let out = tmp.path().join("restored.txt");
        plain_store.restore_file(&checksum, &out, Some(0o644)).unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"compressed at store time",
            "restore must never emit raw zstd bytes as file content");

        // And the reverse direction: plain object read with compression on
        let checksum2 = plain_store.store_object(b"plain at store time").unwrap();
        assert_eq!(compressed_store.read_object(&checksum2).unwrap(), b"plain at store time");
    }

    /// Executable outputs cannot be restored by hardlink (the mode would have
    /// to be applied to the shared inode) — they must fall back to copy.
    #[test]
    fn exec_mode_restores_via_copy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = store_in(tmp.path());
        let checksum = store.store_object(b"#!/bin/sh\n").unwrap();
        let out = tmp.path().join("script.sh");
        store.restore_file(&checksum, &out, Some(0o755)).unwrap();

        assert!(fs::metadata(store.object_path(&checksum)).unwrap().permissions().readonly(),
            "cache object must stay read-only after an exec-mode restore");
        if let Some(mode) = crate::platform::get_mode(&fs::metadata(&out).unwrap()) {
            assert_ne!(mode & 0o111, 0, "restored script must be executable");
            assert_ne!(mode & 0o200, 0, "copy-restored file must be writable");
        }
    }
}
