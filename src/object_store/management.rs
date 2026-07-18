use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;

use super::{
    walk_files, CacheDescriptor, CacheListEntry, CacheListOutput, ObjectStore,
    ProcessorCacheStats,
};

impl ObjectStore {
    /// Get cache size in bytes and number of objects (blobs + descriptors)
    pub fn size(&self) -> (u64, usize) {
        let mut total_bytes = 0u64;
        let mut object_count = 0usize;

        for dir in [&self.objects_dir, &self.descriptors_dir] {
            if !dir.exists() {
                continue;
            }
            for path in walk_files(dir) {
                if let Ok(metadata) = fs::metadata(&path) {
                    total_bytes += metadata.len();
                    object_count += 1;
                }
            }
        }

        (total_bytes, object_count)
    }

    /// Trim cache by removing blob objects not referenced by any descriptor.
    pub fn trim(&self) -> Result<(u64, usize)> {
        let mut removed_bytes = 0u64;
        let mut removed_count = 0usize;

        if !self.objects_dir.exists() {
            return Ok((0, 0));
        }

        // Collect all referenced blob checksums from descriptors. An
        // unreadable or unparsable descriptor must abort the trim: skipping it
        // would garbage-collect every blob it references as "unreferenced".
        let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
        if self.descriptors_dir.exists() {
            for path in walk_files(&self.descriptors_dir) {
                let data = fs::read(&path)
                    .with_context(|| format!("Failed to read descriptor during trim: {}", path.display()))?;
                let desc = serde_json::from_slice::<CacheDescriptor>(&data)
                    .with_context(|| format!("Failed to parse descriptor during trim: {} (remove it to proceed)", path.display()))?;
                match desc {
                    CacheDescriptor::Marker => {}
                    CacheDescriptor::Blob { checksum, .. } => {
                        referenced.insert(checksum);
                    }
                    CacheDescriptor::Tree { entries } => {
                        for entry in entries {
                            referenced.insert(entry.checksum);
                        }
                    }
                }
            }
        }

        // Find and remove unreferenced blob objects
        let mut to_remove = Vec::new();
        for path in walk_files(&self.objects_dir) {
            if let (Some(prefix), Some(rest)) = (
                path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()),
                path.file_name().and_then(|n| n.to_str())
            ) {
                // Compressed objects carry a .zst suffix; strip it to recover
                // the checksum. Stray temp files never match a referenced
                // checksum and are collected as garbage here.
                let rest = rest.strip_suffix(".zst").unwrap_or(rest);
                let checksum = format!("{prefix}{rest}");
                if !referenced.contains(&checksum) {
                    if let Ok(metadata) = fs::metadata(&path) {
                        removed_bytes += metadata.len();
                        removed_count += 1;
                    }
                    to_remove.push(path);
                }
            }
        }

        for path in to_remove {
            // Make writable before removing (objects are read-only)
            if let Ok(mut perms) = fs::metadata(&path).map(|m| m.permissions()) {
                perms.set_readonly(false);
                fs::set_permissions(&path, perms)
                    .with_context(|| format!("Failed to make cache object writable: {}", path.display()))?;
            }
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove cache object: {}", path.display()))?;
            if let Some(parent) = path.parent() {
                // Best-effort: remove empty parent dir (fails silently if not empty)
                let _ = fs::remove_dir(parent);
            }
        }

        Ok((removed_bytes, removed_count))
    }

    /// Remove stale descriptor entries whose cache keys are not in the valid set.
    /// Returns the number of entries removed.
    pub fn remove_stale(&self, valid_descriptor_keys: &std::collections::HashSet<String>) -> Result<usize> {
        let mut count = 0;

        if !self.descriptors_dir.exists() {
            return Ok(0);
        }

        for path in walk_files(&self.descriptors_dir) {
            // Reconstruct descriptor key from path
            if let (Some(prefix), Some(rest)) = (
                path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()),
                path.file_name().and_then(|n| n.to_str())
            ) {
                let key = format!("{prefix}{rest}");
                if !valid_descriptor_keys.contains(&key) {
                    if let Ok(mut perms) = fs::metadata(&path).map(|m| m.permissions()) {
                        perms.set_readonly(false);
                        fs::set_permissions(&path, perms)
                            .with_context(|| format!("Failed to make stale descriptor writable: {}", path.display()))?;
                    }
                    fs::remove_file(&path)
                        .with_context(|| format!("Failed to remove stale descriptor: {}", path.display()))?;
                    count += 1;
                    if let Some(parent) = path.parent() {
                        // Best-effort: remove empty parent dir (fails silently if not empty)
                        let _ = fs::remove_dir(parent);
                    }
                }
            }
        }

        Ok(count)
    }

    /// List all cache descriptors
    pub fn list(&self) -> Vec<CacheListEntry> {
        if !self.descriptors_dir.exists() {
            return Vec::new();
        }

        let mut entries: Vec<CacheListEntry> = walk_files(&self.descriptors_dir)
            .into_iter()
            .filter_map(|path| {
                let data = fs::read(&path).ok()?;
                let desc: CacheDescriptor = serde_json::from_slice(&data).ok()?;

                // Reconstruct descriptor key from path
                let prefix = path.parent()?.file_name()?.to_str()?;
                let rest = path.file_name()?.to_str()?;
                let cache_key = format!("{prefix}{rest}");

                let outputs = match desc {
                    CacheDescriptor::Marker => Vec::new(),
                    CacheDescriptor::Blob { ref checksum, .. } => {
                        vec![CacheListOutput { path: "(blob)".to_string(), exists: self.has_object(checksum) }]
                    }
                    CacheDescriptor::Tree { entries } => {
                        entries.iter().map(|e| CacheListOutput {
                            path: e.path.clone(),
                            exists: self.has_object(&e.checksum),
                        }).collect()
                    }
                };

                Some(CacheListEntry { cache_key, outputs })
            })
            .collect();

        entries.sort_by(|a, b| a.cache_key.cmp(&b.cache_key));
        entries
    }

    /// Get per-processor cache statistics.
    /// Extracts processor name by scanning descriptor keys.
    pub fn stats_by_processor(&self) -> BTreeMap<String, ProcessorCacheStats> {
        let mut stats: BTreeMap<String, ProcessorCacheStats> = BTreeMap::new();

        if !self.descriptors_dir.exists() {
            return stats;
        }

        for path in walk_files(&self.descriptors_dir) {
            let Ok(data) = fs::read(&path) else { continue };
            let Ok(desc) = serde_json::from_slice::<CacheDescriptor>(&data) else { continue };

            // We can't extract processor name from a hashed descriptor key.
            // Use "all" as a single bucket for now.
            let processor = "all".to_string();
            let proc_stats = stats.entry(processor).or_default();
            proc_stats.entry_count += 1;

            match desc {
                CacheDescriptor::Marker => {}
                CacheDescriptor::Blob { ref checksum, .. } => {
                    proc_stats.output_count += 1;
                    let obj_path = self.object_path(checksum);
                    if let Ok(metadata) = fs::metadata(&obj_path) {
                        proc_stats.output_bytes += metadata.len();
                    }
                }
                CacheDescriptor::Tree { ref entries } => {
                    proc_stats.output_count += entries.len();
                    for entry in entries {
                        let obj_path = self.object_path(&entry.checksum);
                        if let Ok(metadata) = fs::metadata(&obj_path) {
                            proc_stats.output_bytes += metadata.len();
                        }
                    }
                }
            }
        }

        stats
    }
}
