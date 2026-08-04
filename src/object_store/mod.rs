pub mod blobs;
mod checksums;
mod config_diff;
mod descriptors;
mod management;
mod operations;
mod restore;

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use redb::{Database, TableDefinition};

use crate::config::RestoreMethod;
use crate::remote_cache::RemoteCache;

/// Number of hex chars used as the subdirectory prefix for object storage (git-style sharding).
pub const CHECKSUM_PREFIX_LEN: usize = 2;

/// Iteratively collect all files under a directory.
pub fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if let Ok(entries) = fs::read_dir(&current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.is_file() {
                    // Temp files (temp-then-rename in flight, or orphaned by
                    // a crash) are included deliberately: `trim` collects
                    // them as unreferenced garbage — a filter here once made
                    // crash-orphaned temp files permanently unreclaimable by
                    // every management command. redb's exclusive lock means
                    // no second process is mid-write while we walk.
                    result.push(path);
                }
            }
        }
    }
    result
}

/// Reject descriptor entry paths with `..` components — they could write
/// (restore) or delete (stale-output cleanup via `previous_tree_paths`)
/// outside the project root. Absolute paths are additionally rejected at the
/// remote-fetch boundary (`try_fetch_descriptor_from_remote`): production
/// entries are always project-root-relative, so an absolute path can only
/// come from a foreign descriptor.
fn safe_entry_path(raw: &str) -> Result<&Path> {
    let path = Path::new(raw);
    if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        anyhow::bail!("cache descriptor entry path '{raw}' escapes the project root");
    }
    Ok(path)
}

const RSBUILD_DIR: &str = ".rsconstruct";
const OBJECTS_DIR: &str = "objects";
const DESCRIPTORS_DIR: &str = "descriptors";
const DB_FILE: &str = "db.redb";

const CONFIGS_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("processor_configs");

/// Reason why a product needs to be rebuilt.
#[derive(Debug)]
pub enum RebuildReason {
    /// No cache entry exists for this product (new or inputs changed)
    NoCacheEntry,
    /// An output file is missing (and can't be restored from cache)
    OutputMissing(String),
    /// Build was forced with --force flag
    Force,
}

impl std::fmt::Display for RebuildReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCacheEntry => write!(f, "no cache entry"),
            Self::OutputMissing(path) => write!(f, "output missing: {path}"),
            Self::Force => write!(f, "forced"),
        }
    }
}

/// The action the executor will take for a product, with an explanation.
#[derive(Debug)]
pub enum ExplainAction {
    /// Product is up-to-date, will be skipped
    Skip,
    /// Product will be restored from cache
    Restore(RebuildReason),
    /// Product will be rebuilt
    Rebuild(RebuildReason),
}

impl std::fmt::Display for ExplainAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Skip => write!(f, "SKIP (inputs unchanged)"),
            Self::Restore(reason) => write!(f, "RESTORE ({reason})"),
            Self::Rebuild(reason) => write!(f, "BUILD ({reason})"),
        }
    }
}

/// Object store for caching build outputs.
/// Uses git-like object storage: .rsconstruct/objects/[2 chars]/[rest of hash]
/// Index is stored in a redb embedded key/value database at .rsconstruct/db.redb
///
/// Methods are split across submodules:
/// - `blobs.rs` — content-addressed blob read/write/restore
/// - `descriptors.rs` — cache descriptor CRUD and `store_marker/blob/tree`
/// - `restore.rs` — `restore_from_descriptor`, `needs_rebuild`, `can_restore`, explain
/// - `management.rs` — size, trim, `remove_stale`, list, stats
/// - `operations.rs` — remote cache push/fetch
/// - `config_diff.rs` — processor config change tracking
pub struct ObjectStore {
    pub(super) objects_dir: PathBuf,
    pub(super) descriptors_dir: PathBuf,
    pub(super) db: Database,
    pub(super) restore_method: RestoreMethod,
    pub(super) compression: bool,
    pub(super) remote: Option<Box<dyn RemoteCache>>,
    pub(super) remote_push: bool,
    #[allow(dead_code)]
    pub(super) remote_pull: bool,
}

/// A cache descriptor stored in the object store at the cache key path.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum CacheDescriptor {
    #[serde(rename = "marker")]
    Marker,
    #[serde(rename = "blob")]
    Blob {
        checksum: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<u32>,
    },
    #[serde(rename = "tree")]
    Tree {
        entries: Vec<TreeEntry>,
    },
}

/// A single file entry in a tree descriptor.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TreeEntry {
    pub(super) path: String,
    pub(super) checksum: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) mode: Option<u32>,
}

/// Per-processor cache statistics
#[derive(Debug, Default, Serialize)]
pub struct ProcessorCacheStats {
    pub entry_count: usize,
    pub output_count: usize,
    pub output_bytes: u64,
}

/// Information about a cache entry for display
#[derive(Debug, Serialize)]
pub struct CacheListEntry {
    pub cache_key: String,
    pub outputs: Vec<CacheListOutput>,
}

/// Information about a single output in a cache list entry
#[derive(Debug, Serialize)]
pub struct CacheListOutput {
    pub path: String,
    pub exists: bool,
}

/// Options for configuring an `ObjectStore` instance.
pub struct ObjectStoreOptions {
    pub restore_method: RestoreMethod,
    pub compression: bool,
    pub remote: Option<Box<dyn RemoteCache>>,
    pub remote_push: bool,
    pub remote_pull: bool,
}

impl ObjectStore {
    pub fn new(opts: ObjectStoreOptions) -> Result<Self> {
        let rsconstruct_dir = PathBuf::from(RSBUILD_DIR);
        let objects_dir = rsconstruct_dir.join(OBJECTS_DIR);
        let db_path = rsconstruct_dir.join(DB_FILE);

        fs::create_dir_all(&rsconstruct_dir)
            .context("Failed to create .rsconstruct directory")?;

        let db = crate::db::open_or_recreate(&db_path, "Cache database")?;
        let descriptors_dir = rsconstruct_dir.join(DESCRIPTORS_DIR);

        Ok(Self {
            objects_dir,
            descriptors_dir,
            db,
            restore_method: opts.restore_method,
            compression: opts.compression,
            remote: opts.remote,
            remote_push: opts.remote_push,
            remote_pull: opts.remote_pull,
        })
    }

    /// Test-only store rooted at `dir` with its own db file — production
    /// `new()` hardcodes the CWD-relative `.rsconstruct` and cannot be
    /// pointed at a tempdir. The redb file allows only one open handle, so
    /// tests that build several stores over the same dir must give each its
    /// own `db_name`.
    #[cfg(test)]
    pub(crate) fn new_at(dir: &Path, db_name: &str) -> Self {
        Self {
            objects_dir: dir.join(OBJECTS_DIR),
            descriptors_dir: dir.join(DESCRIPTORS_DIR),
            db: crate::db::open_or_recreate(&dir.join(db_name), "test cache database").unwrap(),
            restore_method: RestoreMethod::Hardlink,
            compression: false,
            remote: None,
            remote_push: false,
            remote_pull: false,
        }
    }

    /// Test-only store rooted at `dir` with the default db file name.
    #[cfg(test)]
    pub(crate) fn new_in(dir: &Path) -> Self {
        Self::new_at(dir, "db.redb")
    }

    /// Test-only store with a file:// remote and push+pull enabled, for
    /// exercising the remote round-trip without an S3 bucket.
    #[cfg(test)]
    pub(crate) fn new_with_remote(dir: &Path, db_name: &str, remote_dir: &Path) -> Self {
        fs::create_dir_all(dir).unwrap();
        let mut store = Self::new_at(dir, db_name);
        store.remote = Some(Box::new(
            crate::remote_cache::FileBackend::new(&format!("file://{}", remote_dir.display())).unwrap(),
        ));
        store.remote_push = true;
        store.remote_pull = true;
        store
    }
}
