//! `rsconstruct cache` command handlers.
//!
//! Split out of `main.rs`'s `run()`, where this was a 136-line inline match
//! arm — by far the largest, and the specific example finding 6 names. The
//! command bodies live here; `run()` keeps only the dispatch.

use anyhow::Result;
use std::fs;

use crate::builder::Builder;
use crate::cli::CacheAction;
use crate::{checksum, color, json_output, webcache};

/// Handle every `rsconstruct cache <action>` subcommand.
pub fn handle(ctx: &crate::build_context::BuildContext, action: CacheAction) -> Result<()> {
        match action {
            CacheAction::Clear => {
                // Delete .rsconstruct directory directly — must work even if the
                // database is corrupted and Builder::new() would fail.
                let rsconstruct_dir = std::path::Path::new(".rsconstruct");
                if rsconstruct_dir.exists() {
                    crate::errors::ctx(fs::remove_dir_all(rsconstruct_dir), "Failed to remove .rsconstruct directory")?;
                }
                println!("Cache cleared.");
            }
            CacheAction::Size => {
                let builder = Builder::new()?;
                let (bytes, count) = builder.object_store().size();
                if json_output::is_json_mode() {
                    let out = serde_json::json!({ "bytes": bytes, "objects": count });
                    println!("{}", serde_json::to_string_pretty(&out)?);
                } else {
                    println!("Cache size: {} ({} objects)", humansize::format_size(bytes, humansize::BINARY), count);
                }
            }
            CacheAction::Trim => {
                let builder = Builder::new()?;
                builder.apply_config_to_context(ctx);
                let (bytes, count) = builder.object_store().trim()?;
                // The object store is not the only thing that grows
                // without bound: mtime rows for deleted files and expired
                // HTTP responses accumulate forever otherwise.
                let mtime_pruned = checksum::prune_mtime_cache(ctx)?;
                let web_pruned = webcache::prune(ctx.webcache_ttl_secs())?;
                if json_output::is_json_mode() {
                    let out = serde_json::json!({
                        "removed_bytes": bytes,
                        "removed_objects": count,
                        "removed_mtime_entries": mtime_pruned,
                        "removed_webcache_entries": web_pruned,
                    });
                    println!("{}", serde_json::to_string_pretty(&out)?);
                } else {
                    println!("Removed {bytes} bytes ({count} unreferenced objects)");
                    println!("Removed {mtime_pruned} mtime entries for deleted files");
                    println!("Removed {web_pruned} expired webcache entries");
                }
            }
            CacheAction::RemoveStale => {
                let builder = Builder::new()?;
                let valid_keys = builder.valid_cache_keys(ctx)?;
                let stale_count = builder.object_store().remove_stale(&valid_keys)?;
                let (bytes, trim_count) = builder.object_store().trim()?;
                if json_output::is_json_mode() {
                    let out = serde_json::json!({
                        "stale_index_entries_removed": stale_count,
                        "orphaned_bytes_removed": bytes,
                        "orphaned_objects_removed": trim_count,
                    });
                    println!("{}", serde_json::to_string_pretty(&out)?);
                } else {
                    println!("Removed {stale_count} stale index entries");
                    if trim_count > 0 {
                        println!("Removed {bytes} bytes ({trim_count} orphaned objects)");
                    }
                }
            }
            CacheAction::List => {
                let builder = Builder::new()?;
                let entries = builder.object_store().list();
                println!("{}", serde_json::to_string_pretty(&entries)?);
            }
            CacheAction::Stats => {
                let builder = Builder::new()?;
                let stats = builder.object_store().stats_by_processor();

                if stats.is_empty() {
                    println!("Cache is empty.");
                } else if json_output::is_json_mode() {
                    println!("{}", serde_json::to_string_pretty(&stats)?);
                } else {
                    let mut total_entries = 0usize;
                    let mut total_outputs = 0usize;
                    let mut total_bytes = 0u64;
                    for (processor, proc_stats) in &stats {
                        total_entries += proc_stats.entry_count;
                        total_outputs += proc_stats.output_count;
                        total_bytes += proc_stats.output_bytes;
                        println!("{}: {} entries, {} outputs, {}",
                            color::bold(processor),
                            proc_stats.entry_count,
                            proc_stats.output_count,
                            humansize::format_size(proc_stats.output_bytes, humansize::BINARY));
                    }
                    println!();
                    println!("{}: {} entries, {} outputs, {}",
                        color::bold("Total"),
                        total_entries,
                        total_outputs,
                        humansize::format_size(total_bytes, humansize::BINARY));
                }
            }
            CacheAction::Stale => {
                let builder = Builder::new()?;
                let valid_keys = builder.valid_cache_keys(ctx)?;
                let entries = builder.object_store().list();
                let mut current_count = 0usize;
                let mut stale_count = 0usize;
                if json_output::is_json_mode() {
                    let mut rows = Vec::with_capacity(entries.len());
                    for entry in &entries {
                        let is_current = valid_keys.contains(&entry.cache_key);
                        if is_current { current_count += 1; } else { stale_count += 1; }
                        rows.push(serde_json::json!({
                            "cache_key": entry.cache_key,
                            "status": if is_current { "current" } else { "stale" },
                        }));
                    }
                    let out = serde_json::json!({
                        "entries": rows,
                        "summary": { "current": current_count, "stale": stale_count },
                    });
                    println!("{}", serde_json::to_string_pretty(&out)?);
                } else {
                    for entry in &entries {
                        if valid_keys.contains(&entry.cache_key) {
                            println!("{} {}", color::green("current"), entry.cache_key);
                            current_count += 1;
                        } else {
                            println!("{} {}", color::yellow("stale"), entry.cache_key);
                            stale_count += 1;
                        }
                    }
                    println!();
                    println!("{}: {} current, {} stale",
                        color::bold("Summary"), current_count, stale_count);
                }
            }
        }
    Ok(())
}
