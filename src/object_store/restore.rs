use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use super::{CacheDescriptor, ExplainAction, ObjectStore, RebuildReason};

impl ObjectStore {
    /// Restore outputs from a cache descriptor. Returns Ok(true) if restored.
    pub fn restore_from_descriptor(&self, cache_key: &str, output_paths: &[PathBuf]) -> Result<bool> {
        let Some(descriptor) = self.get_descriptor(cache_key) else { return Ok(false) };
        match descriptor {
            CacheDescriptor::Marker => Ok(true),
            CacheDescriptor::Blob { checksum, mode } => {
                let Some(output_path) = output_paths.first() else { return Ok(true) };
                // Verify content like the Tree path does: a modified or
                // corrupted output must be re-restored, not reported OK.
                if output_path.exists() {
                    if let Ok(existing) = Self::calculate_checksum(output_path)
                        && existing == checksum {
                        return Ok(true);
                    }
                    fs::remove_file(output_path)
                        .with_context(|| format!("Failed to remove stale cached file: {}", output_path.display()))?;
                }
                if !self.has_object(&checksum) {
                    return Ok(false);
                }
                if let Some(parent) = output_path.parent() {
                    fs::create_dir_all(parent)
                        .with_context(|| format!("Failed to create output directory: {}", parent.display()))?;
                }
                self.restore_file(&checksum, output_path, mode)
                    .with_context(|| format!("Failed to restore blob to: {}", output_path.display()))?;
                Ok(true)
            }
            CacheDescriptor::Tree { entries } => {
                for entry in &entries {
                    let file_path = Path::new(&entry.path);
                    if file_path.exists() {
                        if let Ok(existing) = Self::calculate_checksum(file_path)
                            && existing == entry.checksum {
                            continue;
                        }
                        fs::remove_file(file_path)
                            .with_context(|| format!("Failed to remove stale cached file: {}", file_path.display()))?;
                    }
                    if !self.has_object(&entry.checksum) {
                        return Ok(false);
                    }
                    if let Some(parent) = file_path.parent() {
                        fs::create_dir_all(parent)
                            .with_context(|| format!("Failed to create directory for tree restore: {}", parent.display()))?;
                    }
                    self.restore_file(&entry.checksum, file_path, entry.mode)
                        .with_context(|| format!("Failed to restore tree entry: {}", file_path.display()))?;
                }
                Ok(true)
            }
        }
    }

    /// Check if a product needs rebuilding based on its descriptor.
    pub fn needs_rebuild_descriptor(&self, cache_key: &str, output_paths: &[PathBuf]) -> bool {
        let Some(descriptor) = self.get_descriptor(cache_key) else { return true };
        match descriptor {
            CacheDescriptor::Marker => false,
            CacheDescriptor::Blob { checksum, .. } => {
                // First output is content-verified against the descriptor,
                // consistent with the Tree path; extra outputs are
                // existence-checked only (their checksums aren't recorded).
                let content_ok = output_paths.first().is_none_or(|p|
                    Self::calculate_checksum(p).ok().as_ref() == Some(&checksum));
                !content_ok || output_paths.iter().skip(1).any(|p| !p.exists())
            }
            CacheDescriptor::Tree { entries } => {
                entries.iter().any(|e| {
                    let p = Path::new(&e.path);
                    !p.exists() || Self::calculate_checksum(p).ok().as_ref() != Some(&e.checksum)
                })
            }
        }
    }

    /// Check if outputs can be restored from a descriptor.
    pub fn can_restore_descriptor(&self, cache_key: &str) -> bool {
        let Some(descriptor) = self.get_descriptor(cache_key) else { return false };
        match descriptor {
            CacheDescriptor::Marker => true,
            CacheDescriptor::Blob { checksum, .. } => self.has_object(&checksum),
            CacheDescriptor::Tree { entries } => {
                entries.iter().all(|e| self.has_object(&e.checksum))
            }
        }
    }

    /// Explain what action will be taken based on descriptor state.
    pub fn explain_descriptor(&self, descriptor_key: &str, output_paths: &[PathBuf], force: bool) -> ExplainAction {
        if force {
            return ExplainAction::Rebuild(RebuildReason::Force);
        }
        let Some(descriptor) = self.get_descriptor(descriptor_key) else {
            return ExplainAction::Rebuild(RebuildReason::NoCacheEntry);
        };
        match descriptor {
            CacheDescriptor::Marker => ExplainAction::Skip,
            CacheDescriptor::Blob { checksum, .. } => {
                for p in output_paths {
                    if !p.exists() {
                        let display = p.display().to_string();
                        if self.has_object(&checksum) {
                            return ExplainAction::Restore(RebuildReason::OutputMissing(display));
                        }
                        return ExplainAction::Rebuild(RebuildReason::OutputMissing(display));
                    }
                }
                ExplainAction::Skip
            }
            CacheDescriptor::Tree { entries } => {
                for entry in &entries {
                    let p = Path::new(&entry.path);
                    let needs_restore = !p.exists()
                        || Self::calculate_checksum(p).ok().as_ref() != Some(&entry.checksum);
                    if needs_restore {
                        if self.has_object(&entry.checksum) {
                            return ExplainAction::Restore(RebuildReason::OutputMissing(entry.path.clone()));
                        }
                        return ExplainAction::Rebuild(RebuildReason::OutputMissing(entry.path.clone()));
                    }
                }
                ExplainAction::Skip
            }
        }
    }
}
