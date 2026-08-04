use anyhow::{Context, Result};
use redb::ReadableDatabase;
use redb::ReadableTable;
use redb::TableDefinition;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Buffer size for streaming file reads in `file_checksum`. 64 KB is a
/// good trade-off: large enough to amortize syscall overhead, small
/// enough to fit in L1 cache and bound peak memory regardless of file
/// size.
const HASH_BUF_SIZE: usize = 64 * 1024;

use crate::build_context::BuildContext;

const MTIME_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("mtime_cache");

/// Cached mtime-to-checksum mapping for a single file
#[derive(Serialize, Deserialize)]
struct MtimeEntry {
    mtime_secs: i64,
    mtime_nanos: u32,
    checksum: String,
}

/// Open or get the mtime database from the `BuildContext`.
fn get_mtime_db(ctx: &BuildContext) -> Result<std::sync::MutexGuard<'_, Option<redb::Database>>> {
    let mut guard = ctx.mtime_db.lock().unwrap();
    if guard.is_none() {
        let dir = PathBuf::from(".rsconstruct");
        crate::errors::ctx(fs::create_dir_all(&dir), "Failed to create .rsconstruct directory")?;
        let db = crate::db::open_or_recreate(&dir.join("mtime.redb"), "Mtime cache")?;
        *guard = Some(db);
    }
    Ok(guard)
}

/// Calculate SHA-256 checksum of a file's contents, using the `BuildContext`'s
/// in-memory cache. First call for a given path streams the file through
/// the hasher and caches the result. Subsequent calls return the cached
/// value.
///
/// Streaming (vs `fs::read` + hash) keeps peak memory bounded at
/// `HASH_BUF_SIZE` regardless of file size — a 100 MB binary doesn't
/// allocate a 100 MB Vec.
pub fn file_checksum(ctx: &BuildContext, path: &Path) -> Result<String> {
    let mut guard = ctx.checksum_cache.lock().unwrap();
    let cache = guard.get_or_insert_with(HashMap::new);
    if let Some(cached) = cache.get(path) {
        return Ok(cached.clone());
    }
    let checksum = stream_file_checksum(path)?;
    cache.insert(path.to_path_buf(), checksum.clone());
    Ok(checksum)
}

/// Evict paths from the in-session checksum cache. The executor calls this
/// whenever it writes or restores files (a product's outputs): the
/// in-session cache assumes content is stable across one build run, and
/// these are exactly the writes that break that assumption. Without
/// eviction, the non-mtime path (`file_checksum`) serves the pre-write
/// checksum to every later reader — including the post-execution
/// descriptor-key recompute, which then keys the cache by content that no
/// longer exists. (`fast_checksum` re-hashes on mtime change, so only
/// `mtime_check = false` builds were affected.)
pub fn forget_in_session(ctx: &BuildContext, paths: &[std::path::PathBuf]) {
    let mut guard = ctx.checksum_cache.lock().unwrap();
    if let Some(cache) = guard.as_mut() {
        for path in paths {
            cache.remove(path);
        }
    }
}

/// Stream a file through SHA-256 with a fixed-size buffer. The buffer is
/// heap-allocated to keep stack usage trivial — a single allocation per
/// hashed file is negligible next to the I/O.
fn stream_file_checksum(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("Failed to open file for checksum: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUF_SIZE];
    loop {
        let n = file.read(&mut buf)
            .with_context(|| format!("Failed to read file for checksum: {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// A dirty mtime-cache entry waiting to be flushed: (path key, entry).
type DirtyMtimeEntry = (String, MtimeEntry);

/// Get checksum using mtime pre-check to avoid re-reading unchanged files.
/// Returns the checksum, which path was taken, and optionally a dirty mtime
/// entry to flush (absent for cache hits and for recently-modified files,
/// which are deliberately not cached — see below).
fn fast_checksum(ctx: &BuildContext, path: &Path) -> Result<(String, ChecksumPath, Option<DirtyMtimeEntry>)> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("Failed to stat file: {}", path.display()))?;
    let mtime = metadata.modified()
        .with_context(|| format!("Failed to get mtime: {}", path.display()))?;
    let duration = mtime.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let mtime_secs = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
    let mtime_nanos = duration.subsec_nanos();

    let path_str = path.display().to_string();

    // Check mtime cache in DB
    let db_guard = get_mtime_db(ctx)?;
    let cached = if let Some(ref db) = *db_guard {
        let read_txn = crate::errors::ctx(db.begin_read(), "Failed to begin read transaction for mtime cache")?;
        match read_txn.open_table(MTIME_TABLE) {
            Ok(table) => {
                table.get(path_str.as_str()).ok()
                    .flatten()
                    .and_then(|data| serde_json::from_slice::<MtimeEntry>(data.value()).ok())
            }
            Err(_) => None,
        }
    } else {
        None
    };
    drop(db_guard);

    if let Some(ref entry) = cached
        && entry.mtime_secs == mtime_secs && entry.mtime_nanos == mtime_nanos
    {
        let mut guard = ctx.checksum_cache.lock().unwrap();
        let cache = guard.get_or_insert_with(HashMap::new);
        cache.insert(path.to_path_buf(), entry.checksum.clone());
        return Ok((entry.checksum.clone(), ChecksumPath::MtimeShortcut, None));
    }

    // mtime changed or no cache entry — recompute checksum from disk.
    //
    // We bypass `file_checksum` (which would consult `ctx.checksum_cache`)
    // because the in-session cache assumes file content is stable across a
    // single build run. That assumption is wrong when an upstream product
    // writes a file mid-build: a stale entry from a pre-rebuild read would
    // be returned here even though mtime told us the file changed. Hash
    // the bytes directly, then refresh the in-session cache.
    let checksum = stream_file_checksum(path)?;
    let mut guard = ctx.checksum_cache.lock().unwrap();
    let cache = guard.get_or_insert_with(HashMap::new);
    cache.insert(path.to_path_buf(), checksum.clone());
    drop(guard);
    // Don't persist an entry for a file modified moments ago: on coarse-
    // timestamp filesystems a write landing in the same mtime tick after we
    // hashed would leave a permanently valid-looking (mtime, checksum) pair
    // for stale content. The next run simply re-hashes such files.
    let recently_modified = SystemTime::now()
        .duration_since(mtime)
        .is_ok_and(|age| age.as_secs() < 2);
    if recently_modified {
        return Ok((checksum, ChecksumPath::FullRead, None));
    }

    let new_entry = MtimeEntry {
        mtime_secs,
        mtime_nanos,
        checksum: checksum.clone(),
    };

    Ok((checksum, ChecksumPath::FullRead, Some((path_str, new_entry))))
}

/// Flush a batch of dirty mtime entries in a single write transaction.
fn flush_mtime_entries(ctx: &BuildContext, dirty: Vec<(String, MtimeEntry)>) -> Result<()> {
    if dirty.is_empty() {
        return Ok(());
    }
    let db_guard = get_mtime_db(ctx)?;
    let db = crate::errors::ctx_opt(db_guard.as_ref(), "Mtime database not available")?;
    let write_txn = crate::errors::ctx(db.begin_write(), "Failed to begin write transaction for mtime cache")?;
    {
        let mut table = write_txn.open_table(MTIME_TABLE)
            .context("Failed to open mtime cache table")?;
        for (path_str, entry) in &dirty {
            let value = serde_json::to_vec(entry)
                .context("Failed to serialize mtime entry")?;
            table.insert(path_str.as_str(), value.as_slice())
                .context("Failed to insert mtime entry")?;
        }
    }
    crate::errors::ctx(write_txn.commit(), "Failed to commit mtime cache entries")?;
    Ok(())
}

/// Hash a list of individual checksums into a single combined SHA-256 checksum.
/// Each element is length-prefixed: a plain `:` join would be ambiguous
/// because `MISSING:{path}` entries embed arbitrary paths that may themselves
/// contain `:`.
fn hash_checksums(checksums: &[String]) -> String {
    let mut hasher = Sha256::new();
    for c in checksums {
        hasher.update((c.len() as u64).to_le_bytes());
        hasher.update(c.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Hash a sequence of string parts with length prefixes, so no part can
/// impersonate a boundary between parts — the same injection-proofing as
/// `hash_checksums`, exposed for the other key-composition sites
/// (`CacheKey::digest`/`descriptor_key`, tool identity hashing). Those sites
/// used to join user-influenced values (instance names, file paths) with
/// bare separators, letting crafted values realign key boundaries.
pub fn hash_parts(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for p in parts {
        hasher.update((p.len() as u64).to_le_bytes());
        hasher.update(p.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// Outcome of a `checksum_fast` call, surfacing whether the mtime cache
/// succeeded in avoiding a file read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumPath {
    MtimeShortcut,
    FullRead,
}

/// Drop mtime-cache entries whose file no longer exists. Returns the number
/// removed.
///
/// The cache is keyed by path and nothing ever removed an entry, so deleting
/// or renaming a file left its row behind forever — the database grew
/// monotonically with the history of every path the project ever had.
/// Called by `rsconstruct cache trim`.
pub fn prune_mtime_cache(ctx: &BuildContext) -> Result<usize> {
    if !PathBuf::from(".rsconstruct").join("mtime.redb").exists() {
        return Ok(0);
    }

    let stale: Vec<String> = {
        let db_guard = get_mtime_db(ctx)?;
        let Some(db) = db_guard.as_ref() else { return Ok(0) };
        let read_txn = crate::errors::ctx(db.begin_read(), "Failed to begin read transaction for mtime prune")?;
        let Ok(table) = read_txn.open_table(MTIME_TABLE) else {
            return Ok(0);
        };
        let mut stale = Vec::new();
        for entry in table.iter().context("Failed to iterate mtime cache")? {
            let (key, _) = entry.context("Failed to read mtime cache entry")?;
            let path_str = key.value();
            if !Path::new(path_str).exists() {
                stale.push(path_str.to_string());
            }
        }
        stale
    };

    if stale.is_empty() {
        return Ok(0);
    }

    let db_guard = get_mtime_db(ctx)?;
    let db = crate::errors::ctx_opt(db_guard.as_ref(), "Mtime database not available")?;
    let write_txn = crate::errors::ctx(db.begin_write(), "Failed to begin write transaction for mtime prune")?;
    {
        let mut table = write_txn.open_table(MTIME_TABLE)
            .context("Failed to open mtime cache table for prune")?;
        for path_str in &stale {
            table.remove(path_str.as_str())
                .with_context(|| format!("Failed to remove mtime entry for {path_str}"))?;
        }
    }
    crate::errors::ctx(write_txn.commit(), "Failed to commit mtime cache prune")?;
    Ok(stale.len())
}

/// Compute a file's checksum, consulting the persistent mtime cache first.
pub fn checksum_fast(ctx: &BuildContext, path: &Path) -> Result<(String, ChecksumPath)> {
    if !ctx.mtime_enabled.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok((file_checksum(ctx, path)?, ChecksumPath::FullRead));
    }
    let (checksum, path_taken, dirty) = fast_checksum(ctx, path)?;
    if let Some(entry) = dirty {
        flush_mtime_entries(ctx, vec![entry])?;
    }
    Ok((checksum, path_taken))
}

/// Checksum an **output** file, using the persistent mtime cache but never
/// the in-session cache.
///
/// The in-session cache (`file_checksum`) assumes content is stable for the
/// whole run — true for inputs, false for outputs, which are rewritten by
/// the very build doing the checking. Output verification therefore takes
/// the mtime shortcut when available and otherwise re-hashes from disk.
pub fn checksum_output(ctx: &BuildContext, path: &Path) -> Result<(String, ChecksumPath)> {
    if !ctx.mtime_enabled.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok((stream_file_checksum(path)?, ChecksumPath::FullRead));
    }
    let (checksum, path_taken, dirty) = fast_checksum(ctx, path)?;
    if let Some(entry) = dirty {
        flush_mtime_entries(ctx, vec![entry])?;
    }
    Ok((checksum, path_taken))
}

/// Get the combined input checksum for a list of input files, using mtime
/// pre-check to avoid re-reading unchanged files across builds.
pub fn combined_input_checksum(ctx: &BuildContext, inputs: &[PathBuf]) -> Result<String> {
    let mtime_enabled = ctx.mtime_enabled.load(std::sync::atomic::Ordering::Relaxed);

    let mut checksums = Vec::with_capacity(inputs.len());
    let mut dirty_entries = Vec::new();

    for input in inputs {
        if input.exists() {
            if mtime_enabled {
                let (checksum, _, dirty) = fast_checksum(ctx, input)?;
                checksums.push(checksum);
                if let Some(entry) = dirty {
                    dirty_entries.push(entry);
                }
            } else {
                checksums.push(file_checksum(ctx, input)?);
            }
        } else {
            checksums.push(format!("MISSING:{}", input.display()));
        }
    }

    if mtime_enabled {
        flush_mtime_entries(ctx, dirty_entries)?;
    }

    Ok(hash_checksums(&checksums))
}

/// Calculate SHA-256 checksum of a byte slice. Not cached — pure function.
pub fn bytes_checksum(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Length-prefixing exists precisely so element boundaries can't be
    /// forged: ["a:b", "c"] and ["a", "b:c"] concatenate identically under
    /// a plain `:` join.
    #[test]
    fn hash_checksums_is_injection_proof() {
        let joined_left = hash_checksums(&["a:b".to_string(), "c".to_string()]);
        let joined_right = hash_checksums(&["a".to_string(), "b:c".to_string()]);
        assert_ne!(joined_left, joined_right);

        assert_ne!(hash_checksums(&[]), hash_checksums(&[String::new()]),
            "no elements and one empty element must differ");
        assert_eq!(hash_checksums(&["x".to_string()]), hash_checksums(&["x".to_string()]),
            "must be deterministic");
    }

    /// The streaming hasher must agree with the one-shot hasher at every
    /// buffer boundary — off-by-one in the read loop shows up exactly there.
    #[test]
    fn stream_checksum_matches_bytes_checksum_at_buffer_boundaries() {
        let tmp = tempfile::TempDir::new().unwrap();
        for size in [0, HASH_BUF_SIZE - 1, HASH_BUF_SIZE, HASH_BUF_SIZE + 1] {
            let data = vec![0xABu8; size];
            let path = tmp.path().join(format!("f{size}"));
            fs::write(&path, &data).unwrap();
            assert_eq!(stream_file_checksum(&path).unwrap(), bytes_checksum(&data),
                "stream and one-shot checksums diverge at {size} bytes");
        }
    }

    /// Missing inputs are part of the combined key (a vanished input must
    /// change it), and the combination is positional, not a set.
    #[test]
    fn combined_input_checksum_missing_and_order_semantics() {
        let ctx = BuildContext::new();
        ctx.set_mtime_check(false);
        let tmp = tempfile::TempDir::new().unwrap();

        let empty = tmp.path().join("empty.txt");
        fs::write(&empty, b"").unwrap();
        let missing = tmp.path().join("missing.txt");

        assert_ne!(
            combined_input_checksum(&ctx, std::slice::from_ref(&empty)).unwrap(),
            combined_input_checksum(&ctx, std::slice::from_ref(&missing)).unwrap(),
            "a missing input must not hash like an empty one"
        );

        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        fs::write(&a, b"aaa").unwrap();
        fs::write(&b, b"bbb").unwrap();
        assert_ne!(
            combined_input_checksum(&ctx, &[a.clone(), b.clone()]).unwrap(),
            combined_input_checksum(&ctx, &[b, a]).unwrap(),
            "input order is part of the key"
        );
    }

    /// The in-session cache serves repeat reads within one build run; the
    /// executor evicts a product's outputs via `forget_in_session` whenever
    /// it writes or restores them. Eviction is what keeps `file_checksum`
    /// correct when mtime checking is off — without it, a mid-build rewrite
    /// would be invisible for the rest of the run.
    #[test]
    fn file_checksum_cache_evicts_on_forget() {
        let ctx = BuildContext::new();
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("f.txt");

        fs::write(&path, b"one").unwrap();
        let first = file_checksum(&ctx, &path).unwrap();
        fs::write(&path, b"two").unwrap();
        assert_eq!(file_checksum(&ctx, &path).unwrap(), first,
            "un-evicted reads serve the in-session cached checksum");

        forget_in_session(&ctx, std::slice::from_ref(&path));
        let second = file_checksum(&ctx, &path).unwrap();
        assert_ne!(second, first, "eviction must expose the new content");

        let fresh_ctx = BuildContext::new();
        assert_eq!(file_checksum(&fresh_ctx, &path).unwrap(), second,
            "post-eviction value must match a fresh context's view");
    }
}
