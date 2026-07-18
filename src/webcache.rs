use anyhow::{Context, Result};
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use std::path::Path;

const DB_PATH: &str = ".rsconstruct/webcache.redb";
const TABLE: TableDefinition<&str, &str> = TableDefinition::new("webcache");

/// An entry in the webcache.
pub struct CacheEntry {
    pub url: String,
    pub size: usize,
}

fn open_db() -> Result<Database> {
    let path = Path::new(DB_PATH);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    Database::create(path)
        .with_context(|| format!("Failed to open webcache database {}", path.display()))
}

/// Fetch URL content, returning cached content if available.
/// On first fetch, the response is stored in the database.
pub fn fetch(url: &str) -> Result<String> {
    let db = open_db()?;

    // Check cache
    {
        let read_txn = db.begin_read()
            .context("Failed to begin read transaction on webcache")?;
        if let Ok(table) = read_txn.open_table(TABLE)
            && let Some(entry) = table.get(url)
                .with_context(|| format!("Failed to read webcache entry for {url}"))?
        {
            return Ok(entry.value().to_string());
        }
    }

    // Fetch from network
    let body = ureq::get(url)
        .call()
        .with_context(|| format!("Failed to fetch {url}"))?
        .body_mut()
        .read_to_string()
        .with_context(|| format!("Failed to read response body from {url}"))?;

    // Store in cache
    let write_txn = db.begin_write()
        .context("Failed to begin write transaction on webcache")?;
    {
        let mut table = write_txn.open_table(TABLE)
            .context("Failed to open webcache table for write")?;
        table.insert(url, body.as_str())
            .with_context(|| format!("Failed to insert webcache entry for {url}"))?;
    }
    write_txn.commit()
        .context("Failed to commit webcache write")?;

    Ok(body)
}

/// Delete all webcache entries. Returns the number of entries removed.
pub fn clear() -> Result<usize> {
    let path = Path::new(DB_PATH);
    if !path.exists() {
        return Ok(0);
    }
    let db = open_db()?;
    let write_txn = db.begin_write()
        .context("Failed to begin write transaction for webcache clear")?;
    let count = {
        let table = write_txn.open_table(TABLE)
            .context("Failed to open webcache table for clear")?;
        table.len()
            .context("Failed to read webcache table length")? as usize
    };
    write_txn.delete_table(TABLE)
        .context("Failed to delete webcache table")?;
    write_txn.commit()
        .context("Failed to commit webcache clear")?;
    Ok(count)
}

/// List all cache entries with URL and size.
pub fn list() -> Result<Vec<CacheEntry>> {
    let path = Path::new(DB_PATH);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let db = open_db()?;
    let read_txn = db.begin_read()
        .context("Failed to begin read transaction on webcache")?;
    let Ok(table) = read_txn.open_table(TABLE) else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    for result in table.iter().context("Failed to iterate webcache entries")? {
        let (key, value) = result.context("Failed to read webcache entry")?;
        entries.push(CacheEntry {
            url: key.value().to_string(),
            size: value.value().len(),
        });
    }
    Ok(entries)
}

/// Return (total_bytes, entry_count) for the webcache.
pub fn stats() -> Result<(u64, usize)> {
    let entries = list()?;
    let total: u64 = entries.iter().map(|e| e.size as u64).sum();
    Ok((total, entries.len()))
}
