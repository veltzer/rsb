//! Dependency analyzers for scanning source files and adding dependencies to the build graph.
//!
//! Analyzers are separate from processors - they run after product discovery to add
//! dependency information (like header files for C/C++ or imports for Python).

mod cpp;
mod icpp;
mod markdown;
pub mod python;
mod tera;


use anyhow::Result;
use indicatif::ProgressBar;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use crate::deps_cache::DepsCache;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{format_command, run_command_capture};

/// Trait for dependency analyzers that scan source files and add dependencies to the graph.
///
/// Analyzers run after processors have discovered products. They scan source files
/// to find dependencies (like #include for C/C++ or import for Python) and add
/// them to the appropriate products in the graph.
///
/// Must be Sync + Send for potential parallel execution.
pub trait DepAnalyzer: Sync + Send {
    /// Human-readable description of what this analyzer does.
    fn description(&self) -> &str;

    /// Whether this analyzer is active. Default true; override to respect
    /// the `enabled` field on an analyzer's config struct.
    fn enabled(&self) -> bool { true }

    /// Auto-detect if this analyzer is relevant for the project.
    /// Called with the file index to check for relevant file types.
    fn auto_detect(&self, file_index: &FileIndex) -> bool;

    /// Return the source path this analyzer would scan for the given product,
    /// or None if the product is not relevant. Used by the shared progress bar
    /// in `run_analyzers` to compute an accurate total before scanning starts,
    /// and by `analyze_with_scanner` to filter products inside the analyze loop.
    fn match_product(&self, product: &Product) -> Option<PathBuf>;

    /// Count how many products in the graph this analyzer would scan.
    /// Default impl iterates over products and calls `match_product`; override
    /// only if a cheaper count is available.
    fn count_matches(&self, graph: &BuildGraph) -> usize {
        graph.products().iter().filter(|p| self.match_product(p).is_some()).count()
    }

    /// Return the set of source paths this analyzer would scan. Used by the
    /// pre-scan classify pass to predict cache-hit / rescan counts before any
    /// work runs. Default impl iterates over products and collects each
    /// `match_product` result.
    fn matching_sources(&self, graph: &BuildGraph) -> Vec<PathBuf> {
        graph.products().iter().filter_map(|p| self.match_product(p)).collect()
    }

    /// Analyze dependencies and add them to products in the graph.
    ///
    /// The analyzer should:
    /// 1. Find products it can analyze (via `match_product`)
    /// 2. For each product, scan the primary source file for dependencies
    /// 3. Use `deps_cache` to avoid re-scanning unchanged files
    /// 4. Add discovered dependencies to the product's inputs
    /// 5. Tick `progress` once per product it processed (whether cache hit or miss)
    fn analyze(
        &self,
        ctx: &crate::build_context::BuildContext,
        graph: &mut BuildGraph,
        deps_cache: &mut DepsCache,
        file_index: &FileIndex,
        verbose: bool,
        progress: &ProgressBar,
    ) -> Result<()>;

    /// Recompute the hash pieces this analyzer would contribute for `source`,
    /// without touching the build graph or the deps cache. Used by
    /// `analyzers show files <path> --hash-pieces` to surface the non-content
    /// state (resolved glob sets, embedded shell commands, etc.) that an
    /// analyzer mixes into a product's cache key.
    ///
    /// The default impl returns `None`, meaning "this analyzer does not
    /// contribute hash pieces" (most don't — only Tera does today). Override
    /// when the analyzer's `analyze` populates `ScanResult.hash_pieces`.
    fn scan_hash_pieces(
        &self,
        _ctx: &crate::build_context::BuildContext,
        _source: &Path,
    ) -> Result<Option<Vec<String>>> {
        Ok(None)
    }
}

/// Query pkg-config for include paths from the given packages.
/// Uses `pkg-config --cflags-only-I` and strips the `-I` prefix.
/// Returns an empty list if `packages` is empty or the query fails.
///
/// - `tag`: prefix for log messages (e.g., "cpp" or "icpp")
/// - `packages`: pkg-config package names to query
/// - `verbose`: whether to emit diagnostic messages to stderr
pub fn query_pkg_config_include_paths(ctx: &crate::build_context::BuildContext, tag: &str, packages: &[String], verbose: bool) -> Vec<PathBuf> {
    if packages.is_empty() {
        return Vec::new();
    }

    let mut cmd = Command::new("pkg-config");
    cmd.arg("--cflags-only-I");
    cmd.args(packages);

    if verbose {
        eprintln!("[{}] Querying pkg-config: {}", tag, format_command(&cmd));
    }

    let output = match run_command_capture(ctx, &cmd) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[{tag}] Failed to query pkg-config: {e}");
            return Vec::new();
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("[{}] pkg-config failed: {}", tag, stderr.trim());
        return Vec::new();
    }

    let paths: Vec<PathBuf> = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .filter_map(|flag| flag.strip_prefix("-I").map(PathBuf::from))
        .collect();

    if verbose && !paths.is_empty() {
        eprintln!("[{}] Found {} include paths from pkg-config", tag, paths.len());
    }

    paths
}

/// Run each command in `commands` via `sh -c` and collect its stdout (trimmed) as an include path.
/// Commands that fail, produce empty output, or yield non-directory paths are skipped with a warning.
///
/// - `tag`: prefix for log messages (e.g., "cpp" or "icpp")
/// - `commands`: shell command strings to run
/// - `verbose`: whether to emit diagnostic messages to stderr
pub fn run_include_path_commands(ctx: &crate::build_context::BuildContext, tag: &str, commands: &[String], verbose: bool) -> Vec<PathBuf> {
    if commands.is_empty() {
        return Vec::new();
    }

    let mut paths = Vec::new();
    for cmd_str in commands {
        if cmd_str.trim().is_empty() {
            continue;
        }

        // Run via shell to support shell syntax (command substitution, etc.)
        let mut cmd = Command::new("sh");
        cmd.arg("-c");
        cmd.arg(cmd_str);

        if verbose {
            eprintln!("[{tag}] Running include path command: sh -c '{cmd_str}'");
        }

        let output = match run_command_capture(ctx, &cmd) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("[{tag}] Failed to run '{cmd_str}': {e}");
                continue;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("[{}] Command '{}' failed: {}", tag, cmd_str, stderr.trim());
            continue;
        }

        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path_str.is_empty() {
            continue;
        }

        let path = PathBuf::from(&path_str);
        if path.is_dir() {
            if verbose {
                eprintln!("[{}] Added include path from command: {}", tag, path.display());
            }
            paths.push(path);
        } else if verbose {
            eprintln!("[{tag}] Command output is not a directory: {path_str}");
        }
    }

    if verbose && !paths.is_empty() {
        eprintln!("[{}] Found {} include paths from commands", tag, paths.len());
    }

    paths
}

/// Result of scanning a single source file: a list of dependency paths and
/// a list of structured pieces mixed into each affected product's `config_hash`.
///
/// `hash_pieces` is for analyzer state that must invalidate the cache key but
/// is *not* a file content (e.g. the sorted set of paths matching a glob
/// pattern, or the literal text of a shell command embedded in a template).
/// Each piece is a `kind:body` string; the order is determined by the analyzer
/// and must be stable across runs. The pieces are joined with `|` and mixed
/// into the existing `config_hash`, so adding/removing entries from the set or
/// rewording the command flips the key even when no individual input file's
/// content changed.
///
/// The pieces are also surfaced via `rsconstruct analyzers show files <path>
/// --hash-pieces` so users can see exactly what non-content state the analyzer
/// is tracking for a given source.
pub struct ScanResult {
    pub deps: Vec<PathBuf>,
    pub hash_pieces: Vec<String>,
}

/// Shared helper for analyzer `analyze()` implementations.
///
/// Iterates over products in the graph, filters them using `match_product`, checks the
/// dependency cache, scans dependencies using `scan_deps` on cache miss, caches results,
/// and adds discovered dependencies to product inputs. Shows a progress bar and cache stats.
///
/// - `match_product`: given a product, returns `Some(source_path)` if the product is relevant
/// - `scan_deps`: given a source path, returns the list of dependency paths
pub fn analyze_with_scanner<F, G>(
    ctx: &crate::build_context::BuildContext,
    graph: &mut BuildGraph,
    deps_cache: &mut DepsCache,
    analyzer_name: &str,
    match_product: F,
    scan_deps: G,
    progress: &ProgressBar,
) -> Result<()>
where
    F: Fn(&crate::graph::Product) -> Option<PathBuf>,
    G: Fn(&Path) -> Result<Vec<PathBuf>>,
{
    // Group product IDs by source path so each unique source is scanned once,
    // then fan the resulting deps out to every product that referenced it.
    let mut by_source: std::collections::BTreeMap<PathBuf, Vec<usize>> = std::collections::BTreeMap::new();
    for p in graph.products() {
        if let Some(source) = match_product(p) {
            by_source.entry(source).or_default().push(p.id);
        }
    }

    if by_source.is_empty() {
        return Ok(());
    }

    for (source, product_ids) in &by_source {
        progress.set_message(format!("[{}] {}", analyzer_name, source.display()));

        // Try to get cached dependencies, otherwise scan. The checksum is
        // taken before the scan so a mid-scan edit can't pair the new
        // content's checksum with the old content's dependencies.
        let deps = if let Some(cached) = deps_cache.get(ctx, analyzer_name, source) {
            cached
        } else {
            let source_checksum = DepsCache::source_checksum(ctx, source)?;
            let scanned = scan_deps(source)?;
            if let Err(e) = deps_cache.set(analyzer_name, source, source_checksum, &scanned) {
                crate::output::warn(&format!("failed to cache dependencies for {}: {}", source.display(), e));
            }
            scanned
        };

        // Fan deps out to every product that has this source as primary input
        if !deps.is_empty() {
            for &id in product_ids {
                if let Some(product) = graph.get_product_mut(id) {
                    let existing: HashSet<&PathBuf> = product.inputs.iter().collect();
                    let new_deps: Vec<PathBuf> = deps.iter()
                        .filter(|dep| !existing.contains(dep))
                        .cloned()
                        .collect();
                    product.inputs.extend(new_deps);
                }
            }
        }

        // Tick once per product so the progress total still matches the pre-scan count
        progress.inc(product_ids.len() as u64);
    }

    Ok(())
}

/// Like `analyze_with_scanner` but the scanner returns a [`ScanResult`] that
/// can also contribute to each affected product's `config_hash`. Used by
/// analyzers whose dependencies aren't only the contents of files (e.g., the
/// Tera analyzer must also account for the *set* of paths matching a glob).
///
/// Cache: the path list is cached per source like in `analyze_with_scanner`,
/// but the `hash_pieces` are **not** cached — they're recomputed on every
/// analyzer run. That's intentional. Tera analysis is cheap, and the pieces
/// often depend on filesystem state (the glob set) that the per-source
/// content cache cannot represent.
pub fn analyze_with_full_scanner<F, G>(
    ctx: &crate::build_context::BuildContext,
    graph: &mut BuildGraph,
    deps_cache: &DepsCache,
    analyzer_name: &str,
    match_product: F,
    scan: G,
    progress: &ProgressBar,
) -> Result<()>
where
    F: Fn(&crate::graph::Product) -> Option<PathBuf>,
    G: Fn(&Path) -> Result<ScanResult>,
{
    let mut by_source: std::collections::BTreeMap<PathBuf, Vec<usize>> = std::collections::BTreeMap::new();
    for p in graph.products() {
        if let Some(source) = match_product(p) {
            by_source.entry(source).or_default().push(p.id);
        }
    }

    if by_source.is_empty() {
        return Ok(());
    }

    for (source, product_ids) in &by_source {
        progress.set_message(format!("[{}] {}", analyzer_name, source.display()));

        let source_checksum = DepsCache::source_checksum(ctx, source)?;
        let result = scan(source)?;

        // Persist the dep list to the cache so commands like
        // `analyzers show` can report what was discovered. The
        // hash_pieces are intentionally NOT cached — they depend on
        // filesystem state (glob results) that must be recomputed on
        // every run. The checksum is taken before the scan (see set()).
        if let Err(e) = deps_cache.set(analyzer_name, source, source_checksum, &result.deps) {
            eprintln!("Warning: failed to cache dependencies for {}: {}", source.display(), e);
        }

        let joined_pieces = if result.hash_pieces.is_empty() {
            None
        } else {
            Some(result.hash_pieces.join("|"))
        };
        for &id in product_ids {
            if let Some(product) = graph.get_product_mut(id) {
                if !result.deps.is_empty() {
                    let existing: HashSet<&PathBuf> = product.inputs.iter().collect();
                    let new_deps: Vec<PathBuf> = result.deps.iter()
                        .filter(|dep| !existing.contains(dep))
                        .cloned()
                        .collect();
                    product.inputs.extend(new_deps);
                }
                if let Some(ref piece) = joined_pieces {
                    product.extend_config_hash(piece);
                }
            }
        }

        progress.inc(product_ids.len() as u64);
    }

    Ok(())
}
