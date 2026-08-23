//! Cache for HTTP responses (remote JSON schemas fetched by `iyamlschema`).
//!
//! Entries carry a fetch timestamp and expire after
//! `[cache] webcache_ttl_secs`. Before that they had no expiry at all: a URL
//! fetched once was served from disk forever, so an upstream schema change
//! was never picked up and the database only ever grew.

use anyhow::{Context, Result};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const DB_PATH: &str = ".rsconstruct/webcache.redb";
const TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("webcache_v2");

/// An entry in the webcache.
pub struct CacheEntry {
    pub url: String,
    pub size: usize,
    /// Seconds since this entry was fetched.
    pub age_secs: u64,
    pub expired: bool,
}

/// The stored value: the body plus when it was fetched.
#[derive(Serialize, Deserialize)]
struct StoredEntry {
    fetched_at_secs: u64,
    body: String,
}

/// The database handle, opened once per process.
///
/// `fetch` used to call `Database::create` on every single call — reopening
/// the file (and taking its lock) once per URL. redb allows only one open
/// handle, so this also means a fetch could not overlap with any other
/// webcache operation.
static DB: OnceLock<Mutex<Option<Database>>> = OnceLock::new();

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Run `f` with the process-wide database handle, opening it on first use.
fn with_db<T>(f: impl FnOnce(&Database) -> Result<T>) -> Result<T> {
    let cell = DB.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().unwrap();
    if guard.is_none() {
        let path = Path::new(DB_PATH);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }
        let db = Database::create(path)
            .with_context(|| format!("Failed to open webcache database {}", path.display()))?;
        *guard = Some(db);
    }
    let db = guard.as_ref().expect("webcache database just opened");
    f(db)
}

/// Whether the webcache database exists on disk. Read-only callers use this
/// to avoid creating an empty database as a side effect of listing it.
fn db_exists() -> bool {
    Path::new(DB_PATH).exists()
}

/// Look up a still-fresh entry for `url`.
fn get_fresh(url: &str, ttl_secs: u64) -> Result<Option<String>> {
    if ttl_secs == 0 || !db_exists() {
        return Ok(None);
    }
    with_db(|db| {
        let read_txn = db.begin_read()
            .context("Failed to begin read transaction on webcache")?;
        let Ok(table) = read_txn.open_table(TABLE) else {
            return Ok(None);
        };
        let Some(raw) = table.get(url)
            .with_context(|| format!("Failed to read webcache entry for {url}"))?
        else {
            return Ok(None);
        };
        // A malformed entry is treated as a miss and re-fetched, rather than
        // failing the build over a cache we can always rebuild.
        let Ok(entry) = serde_json::from_slice::<StoredEntry>(raw.value()) else {
            return Ok(None);
        };
        let age = now_secs().saturating_sub(entry.fetched_at_secs);
        if age >= ttl_secs {
            return Ok(None);
        }
        Ok(Some(entry.body))
    })
}

/// Fetch URL content, returning cached content when it is still within
/// `ttl_secs` of when it was fetched. A `ttl_secs` of 0 disables the cache
/// entirely (always re-fetch, never store).
pub fn fetch(url: &str, ttl_secs: u64) -> Result<String> {
    if let Some(body) = get_fresh(url, ttl_secs)? {
        return Ok(body);
    }

    // Retried via download::with_retry — see
    // docs/src/internal/download-policy.md.
    let body = crate::download::with_retry(|| {
        ureq::get(url)
            .call()
            .with_context(|| format!("Failed to fetch {url}"))?
            .body_mut()
            .read_to_string()
            .with_context(|| format!("Failed to read response body from {url}"))
    })?;

    if ttl_secs == 0 {
        return Ok(body);
    }

    let stored = serde_json::to_vec(&StoredEntry {
        fetched_at_secs: now_secs(),
        body: body.clone(),
    }).context("Failed to serialize webcache entry")?;

    with_db(|db| {
        let write_txn = db.begin_write()
            .context("Failed to begin write transaction on webcache")?;
        {
            let mut table = write_txn.open_table(TABLE)
                .context("Failed to open webcache table for write")?;
            table.insert(url, stored.as_slice())
                .with_context(|| format!("Failed to insert webcache entry for {url}"))?;
        }
        write_txn.commit()
            .context("Failed to commit webcache write")?;
        Ok(())
    })?;

    Ok(body)
}

/// Delete all webcache entries. Returns the number of entries removed.
pub fn clear() -> Result<usize> {
    if !db_exists() {
        return Ok(0);
    }
    let count = list()?.len();
    with_db(|db| {
        let write_txn = db.begin_write()
            .context("Failed to begin write transaction for webcache clear")?;
        write_txn.delete_table(TABLE)
            .context("Failed to delete webcache table")?;
        write_txn.commit()
            .context("Failed to commit webcache clear")?;
        Ok(())
    })?;
    Ok(count)
}

/// Drop entries older than `ttl_secs`. Returns the number removed.
///
/// Expired entries are skipped on read anyway; this is what actually
/// reclaims their disk space, and is called by `cache trim`.
pub fn prune(ttl_secs: u64) -> Result<usize> {
    if !db_exists() {
        return Ok(0);
    }
    let expired: Vec<String> = list_with_ttl(ttl_secs)?.into_iter()
        .filter(|e| e.expired)
        .map(|e| e.url)
        .collect();
    if expired.is_empty() {
        return Ok(0);
    }
    with_db(|db| {
        let write_txn = db.begin_write()
            .context("Failed to begin write transaction for webcache prune")?;
        {
            let mut table = write_txn.open_table(TABLE)
                .context("Failed to open webcache table for prune")?;
            for url in &expired {
                table.remove(url.as_str())
                    .with_context(|| format!("Failed to remove webcache entry {url}"))?;
            }
        }
        write_txn.commit()
            .context("Failed to commit webcache prune")?;
        Ok(())
    })?;
    Ok(expired.len())
}

/// List all cache entries with URL, size and age.
pub fn list() -> Result<Vec<CacheEntry>> {
    list_with_ttl(default_ttl_for_display())
}

/// The TTL used when listing without a loaded config (e.g. `cache webcache
/// list` outside a project). Only affects the reported `expired` flag.
const fn default_ttl_for_display() -> u64 {
    7 * 24 * 60 * 60
}

/// List entries, marking each expired against `ttl_secs`.
pub fn list_with_ttl(ttl_secs: u64) -> Result<Vec<CacheEntry>> {
    if !db_exists() {
        return Ok(Vec::new());
    }
    let now = now_secs();
    with_db(|db| {
        let read_txn = db.begin_read()
            .context("Failed to begin read transaction on webcache")?;
        let Ok(table) = read_txn.open_table(TABLE) else {
            return Ok(Vec::new());
        };
        let mut entries = Vec::new();
        for result in table.iter().context("Failed to iterate webcache entries")? {
            let (key, value) = result.context("Failed to read webcache entry")?;
            let Ok(stored) = serde_json::from_slice::<StoredEntry>(value.value()) else {
                continue;
            };
            let age_secs = now.saturating_sub(stored.fetched_at_secs);
            entries.push(CacheEntry {
                url: key.value().to_string(),
                size: stored.body.len(),
                age_secs,
                expired: ttl_secs == 0 || age_secs >= ttl_secs,
            });
        }
        Ok(entries)
    })
}

/// Return (`total_bytes`, `entry_count`) for the webcache.
pub fn stats() -> Result<(u64, usize)> {
    let entries = list()?;
    let total: u64 = entries.iter().map(|e| e.size as u64).sum();
    Ok((total, entries.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fetch stored under a TTL of 0 must not be cached at all, and a
    /// lookup with TTL 0 must never report a hit.
    #[test]
    fn ttl_zero_disables_the_cache() {
        assert!(get_fresh("http://example.invalid/x", 0).unwrap().is_none());
    }

    /// Entries are considered fresh strictly *within* the TTL, so an entry
    /// exactly at the boundary is expired rather than served.
    #[test]
    fn expiry_is_at_the_boundary() {
        let entry = StoredEntry { fetched_at_secs: 1000, body: "b".into() };
        let age = 2000u64.saturating_sub(entry.fetched_at_secs);
        assert_eq!(age, 1000);
        assert!(age >= 1000, "an entry exactly at the TTL is expired");
        assert!(age < 1001, "and still fresh just under it");
    }

    /// A clock that moves backwards (NTP correction, VM restore) must not
    /// produce a gigantic age that wraps — `saturating_sub` keeps it at 0,
    /// meaning "fresh", which is the safe direction for a cache.
    #[test]
    fn backwards_clock_does_not_underflow() {
        let fetched_in_the_future = 9_000u64;
        assert_eq!(1_000u64.saturating_sub(fetched_in_the_future), 0);
    }
}
