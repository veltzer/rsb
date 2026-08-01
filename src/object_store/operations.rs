use anyhow::{Context, Result};

use super::ObjectStore;

impl ObjectStore {
    /// Try to push an object to remote cache (ignores errors).
    /// The remote always stores the original (uncompressed) content so
    /// machines with different local `compression` settings interoperate.
    #[allow(clippy::unnecessary_wraps)] // Result kept for API symmetry with try_fetch_*; future writers may legitimately fail.
    pub(super) fn try_push_object_to_remote(&self, ctx: &crate::build_context::BuildContext, checksum: &str) -> Result<()> {
        let Some(remote) = &self.remote else { return Ok(()) };

        if !self.has_object(checksum) {
            return Ok(());
        }

        let (prefix, rest) = checksum.split_at(super::CHECKSUM_PREFIX_LEN.min(checksum.len()));
        let remote_key = format!("objects/{prefix}/{rest}");

        // Check if already exists remotely (avoid redundant uploads)
        if remote.exists(ctx, &remote_key).unwrap_or(false) {
            return Ok(());
        }

        // Upload (ignore errors - remote cache is best-effort)
        match self.read_object(checksum) {
            Ok(content) => {
                if let Err(e) = remote.upload_bytes(ctx, &remote_key, &content) {
                    eprintln!("Warning: failed to push to remote cache: {e}");
                }
            }
            Err(e) => eprintln!("Warning: failed to read object for remote push: {e}"),
        }

        Ok(())
    }

    /// Whether the object is available locally, fetching it from the remote
    /// cache first if pull is enabled and it is missing.
    ///
    /// This is the read-path entry point for remote pull. Restore decisions
    /// go through it rather than through `has_object` directly, so a
    /// populated remote bucket can actually satisfy a restore — before this,
    /// push worked but every fetch path was dead code, making a configured
    /// remote a write-only feature.
    pub(super) fn ensure_object(&self, ctx: &crate::build_context::BuildContext, checksum: &str) -> bool {
        if self.has_object(checksum) {
            return true;
        }
        if !self.remote_pull {
            return false;
        }
        // A remote miss (or a broken remote) is not an error: it degrades to
        // a local rebuild, which is always correct. A *corrupt* remote object
        // is different — try_fetch_object_from_remote rejects it rather than
        // poisoning the local store, and we report it here so a bad bucket
        // doesn't fail silently forever.
        match self.try_fetch_object_from_remote(ctx, checksum) {
            Ok(fetched) => fetched,
            Err(e) => {
                eprintln!("Warning: failed to fetch from remote cache: {e}");
                false
            }
        }
    }

    /// Whether the object could be obtained — locally, or from the remote
    /// without downloading it now.
    ///
    /// Used by the reporting paths (`--explain`, `product show`, status),
    /// which must agree with what a real build would do but must not have
    /// the side effect of populating the local cache. `ensure_object` is the
    /// build-path counterpart that actually fetches.
    pub(super) fn object_available(&self, ctx: &crate::build_context::BuildContext, checksum: &str) -> bool {
        if self.has_object(checksum) {
            return true;
        }
        if !self.remote_pull {
            return false;
        }
        let Some(remote) = &self.remote else { return false };
        let (prefix, rest) = checksum.split_at(super::CHECKSUM_PREFIX_LEN.min(checksum.len()));
        remote.exists(ctx, &format!("objects/{prefix}/{rest}")).unwrap_or(false)
    }

    /// Try to fetch an object from remote cache
    pub(super) fn try_fetch_object_from_remote(&self, ctx: &crate::build_context::BuildContext, checksum: &str) -> Result<bool> {
        let Some(remote) = &self.remote else { return Ok(false) };

        if self.has_object(checksum) {
            return Ok(true);
        }

        let (prefix, rest) = checksum.split_at(super::CHECKSUM_PREFIX_LEN.min(checksum.len()));
        let remote_key = format!("objects/{prefix}/{rest}");
        let Some(bytes) = remote.download_bytes(ctx, &remote_key)? else {
            return Ok(false);
        };

        // Verify before admitting into the local content-addressed store —
        // a corrupt or malicious remote must not poison it.
        let actual = Self::calculate_checksum_bytes(&bytes);
        if actual != checksum {
            anyhow::bail!(
                "Remote cache object {remote_key} is corrupt: content hashes to {actual}, expected {checksum}"
            );
        }

        // store_object writes atomically and applies the local storage format.
        self.store_object(&bytes)?;
        Ok(true)
    }

    /// Try to push a descriptor to remote cache
    #[allow(clippy::unnecessary_wraps)] // Result kept for API symmetry with try_fetch_*.
    pub(super) fn try_push_descriptor_to_remote(&self, ctx: &crate::build_context::BuildContext, descriptor_key: &str, data: &[u8]) -> Result<()> {
        let Some(remote) = &self.remote else { return Ok(()) };

        let remote_key = format!("descriptors/{descriptor_key}");
        if let Err(e) = remote.upload_bytes(ctx, &remote_key, data) {
            eprintln!("Warning: failed to push descriptor to remote cache: {e}");
        }

        Ok(())
    }

    /// Try to fetch a descriptor from remote cache, caching it locally.
    ///
    /// The fetched bytes are parsed before being written: a descriptor that
    /// doesn't deserialize would otherwise land in the local store and
    /// permanently break `cache trim`, which fails closed on a parse error.
    /// Unlike objects, a descriptor is not content-addressed, so parsing is
    /// the only integrity check available.
    pub(super) fn try_fetch_descriptor_from_remote(&self, ctx: &crate::build_context::BuildContext, descriptor_key: &str) -> Result<Option<super::CacheDescriptor>> {
        let Some(remote) = &self.remote else { return Ok(None) };

        let remote_key = format!("descriptors/{descriptor_key}");
        let Some(data) = remote.download_bytes(ctx, &remote_key)? else {
            return Ok(None);
        };

        let descriptor: super::CacheDescriptor = serde_json::from_slice(&data)
            .with_context(|| format!("Remote cache descriptor {remote_key} is malformed"))?;

        // Reuse store_descriptor rather than fs::write so the remote path
        // gets the same temp-then-rename atomicity as the local one.
        self.store_descriptor(descriptor_key, &descriptor)?;
        Ok(Some(descriptor))
    }
}

#[cfg(test)]
mod tests {
    use super::super::ObjectStore;
    use crate::build_context::BuildContext;
    use std::fs;

    /// The whole point of finding 12's remote-cache item: a machine with a
    /// cold local cache must be able to restore from what another machine
    /// pushed. Before this, push worked but every fetch path was dead code,
    /// so a populated bucket could never satisfy a restore.
    #[test]
    fn a_populated_remote_satisfies_a_cold_restore() {
        let tmp = tempfile::TempDir::new().unwrap();
        let remote_dir = tmp.path().join("remote");
        let ctx = BuildContext::new();
        ctx.set_mtime_check(false);
        let key = "0bcd1234";

        let out = tmp.path().join("out.txt");
        fs::write(&out, b"produced bytes").unwrap();

        // Machine A builds and pushes.
        {
            let producer = ObjectStore::new_with_remote(
                &tmp.path().join("a"), "a.redb", &remote_dir,
            );
            producer.store_blob_descriptor(&ctx, key, &out).unwrap();
        }

        // Machine B has an empty local store and no output on disk.
        fs::remove_file(&out).unwrap();
        let consumer = ObjectStore::new_with_remote(
            &tmp.path().join("b"), "b.redb", &remote_dir,
        );

        assert!(consumer.get_descriptor(key).is_none(),
            "consumer must start with a cold local cache");
        assert!(consumer.can_restore_descriptor(&ctx, key),
            "a populated remote must be able to satisfy the restore");
        assert!(consumer.restore_from_descriptor(&ctx, key, std::slice::from_ref(&out)).unwrap());
        assert_eq!(fs::read(&out).unwrap(), b"produced bytes");
    }

    /// With pull disabled the remote is invisible to the read path, even
    /// though the bytes are sitting right there.
    #[test]
    fn pull_disabled_ignores_a_populated_remote() {
        let tmp = tempfile::TempDir::new().unwrap();
        let remote_dir = tmp.path().join("remote");
        let ctx = BuildContext::new();
        ctx.set_mtime_check(false);
        let key = "0bcd5678";

        let out = tmp.path().join("out.txt");
        fs::write(&out, b"produced bytes").unwrap();
        {
            let producer = ObjectStore::new_with_remote(
                &tmp.path().join("a"), "a.redb", &remote_dir,
            );
            producer.store_blob_descriptor(&ctx, key, &out).unwrap();
        }
        fs::remove_file(&out).unwrap();

        let mut consumer = ObjectStore::new_with_remote(
            &tmp.path().join("b"), "b.redb", &remote_dir,
        );
        consumer.remote_pull = false;
        assert!(!consumer.can_restore_descriptor(&ctx, key));
        assert!(!consumer.restore_from_descriptor(&ctx, key, std::slice::from_ref(&out)).unwrap());
    }

    /// Tree descriptors must round-trip too — every entry's object has to
    /// come down, not just the first.
    #[test]
    fn remote_pull_restores_every_tree_entry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let remote_dir = tmp.path().join("remote");
        let ctx = BuildContext::new();
        ctx.set_mtime_check(false);
        let key = "0bcd9abc";

        let outdir = tmp.path().join("outdir");
        fs::create_dir_all(&outdir).unwrap();
        fs::write(outdir.join("a.txt"), b"alpha").unwrap();
        fs::write(outdir.join("b.txt"), b"beta").unwrap();
        let dirs = [std::sync::Arc::new(outdir.clone())];
        {
            let producer = ObjectStore::new_with_remote(
                &tmp.path().join("a"), "a.redb", &remote_dir,
            );
            producer.store_tree_descriptor(&ctx, key, &dirs, &[], &|_| false).unwrap();
        }

        fs::remove_dir_all(&outdir).unwrap();
        let consumer = ObjectStore::new_with_remote(
            &tmp.path().join("b"), "b.redb", &remote_dir,
        );
        assert!(consumer.restore_from_descriptor(&ctx, key, &[]).unwrap());
        assert_eq!(fs::read(outdir.join("a.txt")).unwrap(), b"alpha");
        assert_eq!(fs::read(outdir.join("b.txt")).unwrap(), b"beta");
    }

    /// A corrupt remote must never poison the local content-addressed store.
    #[test]
    fn corrupt_remote_object_is_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let remote_dir = tmp.path().join("remote");
        let ctx = BuildContext::new();
        ctx.set_mtime_check(false);
        let key = "0bcddead";

        let out = tmp.path().join("out.txt");
        fs::write(&out, b"produced bytes").unwrap();
        let checksum = {
            let producer = ObjectStore::new_with_remote(
                &tmp.path().join("a"), "a.redb", &remote_dir,
            );
            producer.store_blob_descriptor(&ctx, key, &out).unwrap();
            ObjectStore::calculate_checksum_bytes(b"produced bytes")
        };

        // Tamper with the remote copy of the object. Objects are stored
        // read-only, so replace the file rather than writing over it.
        let (prefix, rest) = checksum.split_at(super::super::CHECKSUM_PREFIX_LEN);
        let remote_object = remote_dir.join("objects").join(prefix).join(rest);
        fs::remove_file(&remote_object).unwrap();
        fs::write(&remote_object, b"tampered bytes").unwrap();

        fs::remove_file(&out).unwrap();
        let consumer = ObjectStore::new_with_remote(
            &tmp.path().join("b"), "b.redb", &remote_dir,
        );
        assert!(!consumer.restore_from_descriptor(&ctx, key, std::slice::from_ref(&out)).unwrap(),
            "a corrupt remote object must not be admitted; caller falls back to building");
        assert!(!consumer.has_object(&checksum),
            "the corrupt bytes must not land in the local store");
    }
}
