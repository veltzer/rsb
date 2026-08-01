//! Dependency cache for storing source file dependencies.
//!
//! Uses a redb key/value store to cache dependency information discovered
//! from source files. This avoids re-scanning files that haven't changed.
//!
//! Cache key: `"<analyzer>\0<source path>"` — analyzer name is part of the
//! key so two analyzers scanning the same file (e.g. a future `python` +
//! `mypy-imports` pair) never overwrite each other's entries. The NUL
//! separator is safe: neither analyzer inames nor paths can contain NUL.
//!
//! Cache value: (`source_checksum`, dependencies)
//!
//! The cache is invalidated when the source file's checksum changes.

use anyhow::{Context, Result};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::build_context::BuildContext;
use crate::checksum::{checksum_fast, ChecksumPath};

const RSBUILD_DIR: &str = ".rsconstruct";
const DEPS_DB_FILE: &str = "deps.redb";

const DEPS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("deps");

/// Cached dependency entry
#[derive(Debug, Serialize, Deserialize)]
struct DepsEntry {
    /// Checksum of the source file when dependencies were scanned
    source_checksum: String,
    /// List of dependency paths (relative to project root)
    dependencies: Vec<String>,
    /// Name of the analyzer that created this entry (e.g., "cpp", "python")
    #[serde(default)]
    analyzer: String,
}

/// Result of a `classify` call — the predict-pass analogue of `DepsCacheStats`.
/// `MtimeHit` + `ContentHit` = a hit that `get` would also report; Miss means
/// `get` would rescan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifyResult {
    /// Cache valid, mtime-cache shortcut applied (no I/O needed).
    MtimeHit,
    /// Cache valid, but mtime was stale so the file was read and re-hashed.
    ContentHit,
    /// Cache invalid or absent.
    Miss,
}

/// Statistics about dependency cache usage.
/// `mtime_hits + content_hits == hits` always holds.
#[derive(Debug, Default, Clone)]
pub struct DepsCacheStats {
    /// Total number of cache hits (`mtime_hits` + `content_hits`).
    pub hits: usize,
    /// Hits where the mtime cache shortcut succeeded — no file I/O was done.
    pub mtime_hits: usize,
    /// Hits where the mtime was stale so the file had to be re-read and
    /// re-hashed, but the content checksum still matched the stored one
    /// (e.g. a touched-but-unchanged file).
    pub content_hits: usize,
    /// Number of cache misses.
    pub misses: usize,
}

/// Dependency cache using redb key/value store
pub struct DepsCache {
    db: Database,
    stats: DepsCacheStats,
}

impl DepsCache {
    /// Open or create the dependency cache
    pub fn open() -> Result<Self> {
        Self::open_in(Path::new(""))
    }

    /// Open the cache with `.rsconstruct` rooted under `base`. Lets tests use
    /// a tempdir without mutating the process-global current directory (which
    /// races the parallel test runner).
    pub fn open_in(base: &Path) -> Result<Self> {
        let rsconstruct_dir = base.join(RSBUILD_DIR);
        let db_path = rsconstruct_dir.join(DEPS_DB_FILE);

        // Ensure .rsconstruct directory exists
        fs::create_dir_all(&rsconstruct_dir)
            .context("Failed to create .rsconstruct directory")?;

        let db = crate::db::open_or_recreate(&db_path, "Dependency cache")?;

        Ok(Self { db, stats: DepsCacheStats::default() })
    }

    /// Get cached dependencies for a (analyzer, source) pair if the cache is
    /// valid. Returns None if the file has changed or isn't cached.
    /// Updates internal statistics (hits/misses). Every caller hits exactly
    /// one of the two counters — no silent path that leaves both unchanged,
    /// so `hits + misses` always equals the number of `get` calls.
    pub fn get(&mut self, ctx: &BuildContext, analyzer: &str, source: &Path) -> Option<Vec<PathBuf>> {
        let key = key_for(analyzer, source);

        // Any failure to reach the stored entry — DB not yet created, table
        // missing, deserialization error, stat failure — counts as a miss.
        // These paths all mean "we can't trust the cache for this file."
        let Ok(read_txn) = self.db.begin_read() else {
            self.stats.misses += 1;
            return None;
        };
        let Ok(table) = read_txn.open_table(DEPS_TABLE) else {
            self.stats.misses += 1;
            return None;
        };
        let Ok(Some(data)) = table.get(key.as_str()) else {
            self.stats.misses += 1;
            return None;
        };
        let Ok(entry) = serde_json::from_slice::<DepsEntry>(data.value()) else {
            self.stats.misses += 1;
            return None;
        };

        // Verify source file hasn't changed. `checksum_fast` consults the
        // persistent mtime cache so unchanged files skip the full read + hash.
        let Ok((current_checksum, checksum_path)) = checksum_fast(ctx, source) else {
            self.stats.misses += 1;
            return None;
        };
        if entry.source_checksum != current_checksum {
            self.stats.misses += 1;
            return None;
        }

        // Verify all dependencies still exist
        let deps: Vec<PathBuf> = entry.dependencies.iter()
            .map(PathBuf::from)
            .collect();

        for dep in &deps {
            if !dep.exists() {
                self.stats.misses += 1;
                return None;
            }
        }

        self.stats.hits += 1;
        match checksum_path {
            ChecksumPath::MtimeShortcut => self.stats.mtime_hits += 1,
            ChecksumPath::FullRead => self.stats.content_hits += 1,
        }
        Some(deps)
    }

    /// Dry-run of `get`: predict whether this (analyzer, source) pair would
    /// hit the cache, and if so whether the mtime shortcut would apply.
    /// Used by the pre-scan classify pass to count expected hits vs rescans
    /// before the actual scan runs. Identical validity rules to `get`.
    /// Does not touch stats.
    pub fn classify(&self, ctx: &BuildContext, analyzer: &str, source: &Path) -> ClassifyResult {
        let key = key_for(analyzer, source);
        let Ok(read_txn) = self.db.begin_read() else { return ClassifyResult::Miss };
        let Ok(table) = read_txn.open_table(DEPS_TABLE) else { return ClassifyResult::Miss };
        let Ok(Some(data)) = table.get(key.as_str()) else { return ClassifyResult::Miss };
        let Ok(entry) = serde_json::from_slice::<DepsEntry>(data.value()) else { return ClassifyResult::Miss };
        let Ok((current_checksum, checksum_path)) = checksum_fast(ctx, source) else { return ClassifyResult::Miss };
        if entry.source_checksum != current_checksum {
            return ClassifyResult::Miss;
        }
        if !entry.dependencies.iter().all(|d| Path::new(d).exists()) {
            return ClassifyResult::Miss;
        }
        match checksum_path {
            ChecksumPath::MtimeShortcut => ClassifyResult::MtimeHit,
            ChecksumPath::FullRead => ClassifyResult::ContentHit,
        }
    }

    /// Compute the source checksum for use with [`Self::set`]. Call this
    /// BEFORE scanning the file: pairing a checksum taken after the scan with
    /// deps derived from the pre-scan content would let a mid-build edit
    /// poison the cache with a stale dependency list. Uses `checksum_fast` so
    /// the mtime cache is populated alongside — subsequent `get()` calls can
    /// then short-circuit on mtime.
    pub fn source_checksum(ctx: &BuildContext, source: &Path) -> Result<String> {
        let (checksum, _) = checksum_fast(ctx, source)?;
        Ok(checksum)
    }

    /// Store dependencies for a (analyzer, source) pair.
    /// `source_checksum` must come from [`Self::source_checksum`] taken
    /// before the scan that produced `dependencies`.
    pub fn set(&self, analyzer: &str, source: &Path, source_checksum: String, dependencies: &[PathBuf]) -> Result<()> {
        let key = key_for(analyzer, source);

        let entry = DepsEntry {
            source_checksum,
            dependencies: dependencies.iter()
                .map(|p| p.display().to_string())
                .collect(),
            analyzer: analyzer.to_string(),
        };

        let data = serde_json::to_vec(&entry)
            .context("Failed to serialize dependency entry")?;

        let write_txn = self.db.begin_write()
            .context("Failed to begin write transaction")?;
        {
            let mut table = write_txn.open_table(DEPS_TABLE)
                .context("Failed to open deps table")?;
            table.insert(key.as_str(), data.as_slice())
                .context("Failed to write to dependency cache")?;
        }
        write_txn.commit()
            .context("Failed to commit dependency cache write")?;

        Ok(())
    }

    /// Get cache statistics (hits and misses)
    pub const fn stats(&self) -> &DepsCacheStats {
        &self.stats
    }

    /// Collect all entries from the database as (analyzer, `source_path`, `DepsEntry`) triples.
    /// Returns an empty Vec on any error (missing table, etc.).
    /// Entries with malformed keys (no NUL separator) are skipped — that would
    /// be a pre-key-format-change entry from an older build, effectively invalid.
    fn collect_entries(&self) -> Vec<(String, PathBuf, DepsEntry)> {
        let Ok(read_txn) = self.db.begin_read() else { return Vec::new() };
        let Ok(table) = read_txn.open_table(DEPS_TABLE) else { return Vec::new() };
        let Ok(iter) = table.iter() else { return Vec::new() };
        iter.filter_map(|item| {
            let (key, value) = item.ok()?;
            let (analyzer, source) = parse_key(key.value())?;
            let entry: DepsEntry = serde_json::from_slice(value.value()).ok()?;
            Some((analyzer, source, entry))
        })
        .collect()
    }

    /// Get all raw cached entries for a given source path, across every
    /// analyzer that has scanned it. Each returned tuple is (dependencies,
    /// `analyzer_name`). Returns an empty Vec if the source has no entries.
    /// Used by `analyzers show files <path>`, where the user gives a path and
    /// expects to see every analyzer's view of it.
    pub fn get_raw_for_path(&self, source: &Path) -> Vec<(Vec<PathBuf>, String)> {
        self.collect_entries()
            .into_iter()
            .filter(|(_a, s, _e)| s == source)
            .map(|(analyzer, _s, entry)| {
                let deps = entry.dependencies.iter().map(PathBuf::from).collect();
                (deps, analyzer)
            })
            .collect()
    }

    /// List all cached source files and their dependencies.
    /// Returns tuples of (`source_path`, dependencies, `analyzer_name`).
    pub fn list_all(&self) -> Vec<(PathBuf, Vec<PathBuf>, String)> {
        self.collect_entries()
            .into_iter()
            .map(|(analyzer, source, entry)| {
                let deps: Vec<PathBuf> = entry.dependencies.iter().map(PathBuf::from).collect();
                (source, deps, analyzer)
            })
            .collect()
    }

    /// Get statistics about cached dependencies by analyzer.
    /// Returns a map of `analyzer_name` -> (`file_count`, `total_dep_count`).
    pub fn stats_by_analyzer(&self) -> std::collections::HashMap<String, (usize, usize)> {
        let mut stats: std::collections::HashMap<String, (usize, usize)> = std::collections::HashMap::new();
        for (analyzer, _source, entry) in self.collect_entries() {
            let name = if analyzer.is_empty() { "unknown".to_string() } else { analyzer };
            let (files, deps) = stats.entry(name).or_insert((0, 0));
            *files += 1;
            *deps += entry.dependencies.len();
        }
        stats
    }

    /// List cached source files and their dependencies filtered by analyzer names.
    /// Returns tuples of (`source_path`, dependencies, `analyzer_name`).
    pub fn list_by_analyzers(&self, analyzers: &[String]) -> Vec<(PathBuf, Vec<PathBuf>, String)> {
        self.collect_entries()
            .into_iter()
            .filter_map(|(analyzer, source, entry)| {
                if !analyzers.contains(&analyzer) {
                    return None;
                }
                let deps: Vec<PathBuf> = entry.dependencies.iter().map(PathBuf::from).collect();
                Some((source, deps, analyzer))
            })
            .collect()
    }

    /// Remove all cached entries created by a specific analyzer.
    /// Returns the number of entries removed.
    pub fn remove_by_analyzer(&self, analyzer: &str) -> Result<usize> {
        // Collect raw keys to remove by re-encoding (analyzer, source) → key.
        let keys_to_remove: Vec<String> = self.collect_entries()
            .into_iter()
            .filter_map(|(a, source, _entry)| {
                if a == analyzer { Some(key_for(&a, &source)) } else { None }
            })
            .collect();

        let mut removed = 0;
        if !keys_to_remove.is_empty() {
            let write_txn = self.db.begin_write()
                .context("Failed to begin write transaction")?;
            {
                let mut table = write_txn.open_table(DEPS_TABLE)
                    .context("Failed to open deps table")?;
                for key in &keys_to_remove {
                    if table.remove(key.as_str()).is_ok() {
                        removed += 1;
                    }
                }
            }
            write_txn.commit()
                .context("Failed to commit dependency cache removal")?;
        }

        Ok(removed)
    }
}

/// Build the composite cache key for an (analyzer, source) pair. NUL is used
/// as the separator because neither analyzer inames nor filesystem paths can
/// contain NUL bytes, so there's no possible ambiguity.
fn key_for(analyzer: &str, path: &Path) -> String {
    let mut s = String::with_capacity(analyzer.len() + 1 + path.as_os_str().len());
    s.push_str(analyzer);
    s.push('\0');
    s.push_str(&path.display().to_string());
    s
}

/// Split a composite key back into (analyzer, source path). Returns None if
/// the key predates the composite format (no NUL separator) or is otherwise
/// malformed — such entries are treated as stale and ignored.
fn parse_key(key: &str) -> Option<(String, PathBuf)> {
    let (analyzer, path) = key.split_once('\0')?;
    Some((analyzer.to_string(), PathBuf::from(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_and_parse_roundtrip() {
        let key = key_for("python", Path::new("src/foo/bar.py"));
        assert_eq!(key, "python\0src/foo/bar.py");
        let (analyzer, path) = parse_key(&key).expect("must parse");
        assert_eq!(analyzer, "python");
        assert_eq!(path, PathBuf::from("src/foo/bar.py"));
    }

    #[test]
    fn keys_differ_by_analyzer() {
        // The whole point of the composite key: two analyzers scanning the
        // same file must produce distinct cache entries.
        let k1 = key_for("python", Path::new("foo.py"));
        let k2 = key_for("mypy", Path::new("foo.py"));
        assert_ne!(k1, k2);
    }

    #[test]
    fn keys_differ_by_instance_name() {
        // Multi-instance analyzers (e.g. cpp.kernel vs cpp.userspace) must
        // also produce distinct keys.
        let k1 = key_for("cpp.kernel", Path::new("foo.c"));
        let k2 = key_for("cpp.userspace", Path::new("foo.c"));
        assert_ne!(k1, k2);
    }

    #[test]
    fn parse_rejects_key_without_separator() {
        // Pre-composite-format entries (just a bare path) must not parse —
        // they're treated as stale and dropped from listings.
        assert!(parse_key("just/a/path.py").is_none());
    }

    #[test]
    fn parse_handles_path_with_colons() {
        // The separator is NUL specifically because paths can contain every
        // other punctuation character. A path with colons must parse cleanly.
        let key = key_for("cpp", Path::new("src/a:b.c"));
        let (analyzer, path) = parse_key(&key).unwrap();
        assert_eq!(analyzer, "cpp");
        assert_eq!(path, PathBuf::from("src/a:b.c"));
    }

    /// Regression guard: every `get` call must increment exactly one counter.
    /// The earlier implementation used `.ok()?` on `begin_read` and
    /// `open_table`, which silently returned None without counting — so on a
    /// fresh DB the first call was statistically invisible and the predict
    /// pass's numbers wouldn't match the summary's. Calling `get` against a
    /// nonexistent source file (in a fresh tempdir, no cache yet) must count
    /// as a miss.
    #[test]
    fn get_counts_miss_even_when_db_is_fresh() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Root the cache in the tempdir directly — mutating the process-wide
        // current dir would race other tests in the parallel runner.
        let ctx = crate::build_context::BuildContext::new();
        let mut cache = DepsCache::open_in(tmp.path()).expect("open fresh cache");
        let nonexistent = tmp.path().join("does_not_exist.py");
        let result = cache.get(&ctx, "python", &nonexistent);

        assert!(result.is_none(), "missing entry must return None");
        let stats = cache.stats();
        assert_eq!(stats.hits + stats.misses, 1,
            "exactly one of hits/misses must advance per get call (hits={}, misses={})",
            stats.hits, stats.misses);
        assert_eq!(stats.misses, 1, "missing entry counts as a miss");
    }
}
