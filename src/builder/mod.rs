mod add_config;
mod build;
mod clean;
mod fix;
pub mod analyzers;
mod doctor;
mod graph;
mod product;
pub mod processors;
pub mod sloc;
pub mod smart;
pub mod symlink_install;
pub mod tools;

pub use add_config::{add_processor, add_analyzer};

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use anyhow::{Context as _, Result};
use crate::analyzers::DepAnalyzer;
use crate::cli::{BuildPhase, DisplayOptions};
use crate::color;
use crate::config::*;
use crate::deps_cache::DepsCache;
use crate::errors;
use crate::file_index::FileIndex;
use crate::graph::BuildGraph;
use crate::object_store::{ObjectStore, ObjectStoreOptions};
use crate::processors::{LuaProcessor, ProcessorMap, Processor};
use crate::remote_cache;
use crate::tool_lock;

/// Phase timing data collected during graph building.
pub type PhaseTimings = Vec<(String, Duration)>;

/// Controls which graph-building variant to use.
#[derive(Clone, Copy, PartialEq, Eq)]
enum GraphBuildMode {
    /// Full build: discover products, run analyzers, resolve dependencies
    Normal,
    /// Clean: only discover products to find output files (skip expensive analysis)
    ForClean,
}

/// Return the keys of a HashMap sorted alphabetically.
fn sorted_keys<V>(map: &HashMap<String, V>) -> Vec<&String> {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    keys
}

/// Check if phases debug is enabled.
fn phases_debug() -> bool {
    crate::runtime_flags::phases_debug()
}

/// A named point in the build pipeline at which `print_graph_stats` can
/// snapshot the graph. Each variant maps to a specific call site; adding a
/// new snapshot point means adding a variant here and a `print_graph_stats`
/// call at the site — the type checker keeps the two in sync.
///
/// The variant name (`SNAPSHOT_NAME` in SCREAMING_SNAKE_CASE) is printed
/// verbatim next to the counts so the user can grep for a specific point
/// or feed the symbolic name back into future CLI flags (e.g. stop-at).
#[derive(Debug, Clone, Copy)]
pub enum GraphSnapshot {
    /// Before any phase runs — empty graph.
    Start,
    /// After product discovery.
    AfterDiscover,
    /// After analyzer-driven dependency scanning.
    AfterAddDependencies,
    /// After tool-version hashes are folded into config hashes.
    AfterApplyToolHashes,
    /// After graph-wide dependency edge resolution.
    AfterResolve,
    /// After pre-build classify (skip/restore/build decision).
    AfterClassify,
    /// After the build has finished executing all products.
    AfterExecute,
}

impl GraphSnapshot {
    /// Symbolic name shown in graph-stats output. Matches the enum variant
    /// in SCREAMING_SNAKE_CASE.
    const fn name(self) -> &'static str {
        match self {
            GraphSnapshot::Start => "START",
            GraphSnapshot::AfterDiscover => "AFTER_DISCOVER",
            GraphSnapshot::AfterAddDependencies => "AFTER_ADD_DEPENDENCIES",
            GraphSnapshot::AfterApplyToolHashes => "AFTER_APPLY_TOOL_HASHES",
            GraphSnapshot::AfterResolve => "AFTER_RESOLVE",
            GraphSnapshot::AfterClassify => "AFTER_CLASSIFY",
            GraphSnapshot::AfterExecute => "AFTER_EXECUTE",
        }
    }
}

/// Emit a one-line snapshot of the graph's size at a named point.
/// No-op unless `--graph-stats` is set. Output goes to stderr so it can't
/// corrupt stdout piping (e.g., `rsconstruct graph show --format=dot`).
///
/// Three counters per snapshot:
///   - `products`: number of products in the graph.
///   - `edges`: producer→consumer edges (one product's output is another's input).
///   - `inputs`: total input-file references across all products. Headers added
///     by analyzers don't form `edges` (they aren't outputs of any product),
///     but they DO grow `inputs` — diff `inputs` between AFTER_DISCOVER and
///     AFTER_ADD_DEPENDENCIES to see how much the analyzer phase contributed.
pub fn print_graph_stats(snapshot: GraphSnapshot, graph: &BuildGraph) {
    if !crate::runtime_flags::graph_stats() {
        return;
    }
    let products = graph.products().len();
    // Cheap — each product's dependency list is a Vec already held in memory.
    let edges: usize = graph.products()
        .iter()
        .map(|p| graph.get_dependencies(p.id).len())
        .sum();
    let inputs: usize = graph.products()
        .iter()
        .map(|p| p.inputs.len())
        .sum();
    eprintln!(
        "[graph-stats] {:<24}  products={}  edges={}  inputs={}",
        snapshot.name(), products, edges, inputs,
    );
}

/// Labels for the four product states used by dry_run and status.
struct ProductStatusLabels<'a> {
    current: (Cow<'a, str>, &'static str),
    restorable: (Cow<'a, str>, &'static str),
    stale: (Cow<'a, str>, &'static str),
    new: (Cow<'a, str>, &'static str),
}

/// Options for `print_product_status`.
struct StatusPrintOptions<'a> {
    force: bool,
    labels: &'a ProductStatusLabels<'a>,
    explain: bool,
    display_opts: DisplayOptions,
    verbose: bool,
    all_processor_names: &'a [&'a str],
    native_processors: &'a std::collections::HashSet<&'a str>,
}

/// Create a processor from a type name and TOML config value.
/// Returns None for unknown types (Lua plugins handled separately).
pub fn create_processor_for_instance(
    type_name: &str,
    config_toml: &toml::Value,
) -> anyhow::Result<Option<Box<dyn Processor>>> {
    if let Some(entry) = find_registry_entry(type_name) {
        let mut resolved = config_toml.clone();
        let mut prov = crate::config::ProvenanceMap::new();
        crate::registries::apply_all_defaults(entry.name, &mut resolved, &mut prov);
        return (entry.create)(&resolved).map(Some);
    }
    Ok(None)
}

/// Create all builtin processors with default configs.
pub fn create_all_default_processors() -> Result<ProcessorMap> {
    let mut processors: ProcessorMap = HashMap::new();
    for entry in registry_entries() {
        let mut empty_toml = toml::Value::Table(toml::map::Map::new());
        let mut prov = crate::config::ProvenanceMap::new();
        crate::registries::apply_all_defaults(entry.name, &mut empty_toml, &mut prov);
        let proc = (entry.create)(&empty_toml)
            .with_context(|| format!("Failed to create processor '{}' with default config", entry.name))?;
        processors.insert(entry.name.to_string(), proc);
    }
    Ok(processors)
}

pub struct Builder {
    object_store: ObjectStore,
    config: Config,
    file_index: FileIndex,
}

impl Builder {
    /// Apply config-derived settings to the BuildContext. Call this once after
    /// creating the Builder, before any build/status operations. This bridges
    /// config values that affect BuildContext state (e.g. mtime_check) without
    /// requiring Builder::new() to take &BuildContext.
    pub fn apply_config_to_context(&self, ctx: &crate::build_context::BuildContext) {
        if !self.config.cache.mtime_check {
            ctx.set_mtime_check(false);
        }
    }

    pub fn new() -> Result<Self> {
        Self::new_with_overrides(&[], &[])
    }

    /// Construct a Builder, applying CLI `--iset`/`--pset` config overrides
    /// after the rsconstruct.toml is loaded.
    pub fn new_with_overrides(iset: &[String], pset: &[String]) -> Result<Self> {
        Config::require_config()?;
        let mut config = Config::load()?;
        config.apply_overrides(iset, pset)?;

        // Resolve auto restore method based on environment
        let restore_method = config.cache.restore_method.resolve();

        // Validate: compression and hardlink restore are incompatible
        if config.cache.compression && restore_method == crate::config::RestoreMethod::Hardlink {
            anyhow::bail!("Cannot use cache compression with hardlink restore method. \
                Set restore_method = \"copy\" or disable compression.");
        }

        // Create remote cache backend if configured
        let remote_backend = match &config.cache.remote {
            Some(url) => Some(remote_cache::create_backend(url)?),
            None => None,
        };

        // Note: config.cache.mtime_check is applied by the caller via
        // ctx.set_mtime_check() — Builder::new() doesn't have access to
        // BuildContext. The CLI --no-mtime-cache flag is also applied by
        // the caller in main.rs.
        let object_store = ObjectStore::new(ObjectStoreOptions {
            restore_method,
            compression: config.cache.compression,
            remote: remote_backend,
            remote_push: config.cache.remote_push,
            remote_pull: config.cache.remote_pull,
        })?;
        let file_index = FileIndex::build()?;

        Ok(Self {
            object_store,
            config,
            file_index,
        })
    }

    /// Create processors from declared instances in the config.
    /// Only processors declared in `[processor.TYPE]` or `[processor.TYPE.NAME]` are created.
    pub fn create_processors(&self) -> Result<ProcessorMap> {
        let cfg = &self.config.processor;
        let mut processors: ProcessorMap = HashMap::new();

        for inst in &cfg.instances {
            match create_processor_for_instance(&inst.type_name, &inst.config_toml) {
                Ok(Some(proc)) => {
                    processors.insert(inst.instance_name.clone(), proc);
                }
                Ok(None) => {
                    anyhow::bail!("Unknown processor type: '{}'", inst.type_name);
                }
                Err(e) => {
                    return Err(e.context(format!(
                        "Failed to create processor instance '{}'", inst.instance_name
                    )));
                }
            }
        }

        // Lua plugin processors
        let lua_plugins = LuaProcessor::discover_plugins(
            &self.config.plugins.dir,
            &cfg.extra,
        )?;
        for (name, proc) in lua_plugins {
            if processors.contains_key(&name) {
                anyhow::bail!("Lua plugin '{name}' conflicts with processor instance");
            }
            processors.insert(name, Box::new(proc));
        }

        Ok(processors)
    }

    /// Detect and display config changes for each processor.
    /// Shows colored diffs when processor configuration has changed since last build.
    fn detect_config_changes(&self, processors: &ProcessorMap, show_all: bool) {
        // Don't show config diffs in JSON or quiet mode
        if crate::json_output::is_json_mode() || crate::runtime_flags::quiet() {
            return;
        }

        for name in sorted_keys(processors) {
            let processor = processors.get(name).expect(errors::PROCESSOR_NOT_IN_MAP);

            // Skip processors that don't provide config JSON
            let Some(config_json) = processor.config_json() else { continue };

            // Keep only checksum-affecting fields for change detection (unless show_all).
            // Each processor declares which config fields affect its output;
            // changes to other fields (src_dirs, batch, max_jobs, etc.)
            // should not trigger config change detection by default.
            let config_json = if show_all {
                config_json
            } else {
                Self::filter_checksum_fields(name, &config_json)
            };

            // Store the config and check if it changed
            if let Ok(Some(old_json)) = self.object_store.store_processor_config(name, &config_json) {
                // Config changed - show diff
                if let Some(diff) = ObjectStore::diff_configs(&old_json, &config_json) {
                    println!("{}",
                        color::yellow(&format!("Config changed for [{name}]:")));
                    println!("{diff}");
                }
            }
        }
    }

    /// Filter a config JSON string to only include checksum-affecting fields.
    /// Uses the processor's `checksum_fields()` declaration to determine which
    /// fields matter for build output.
    fn filter_checksum_fields(processor_name: &str, json: &str) -> String {
        let checksum_fields = ProcessorConfig::checksum_fields_for(processor_name);
        let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
            return json.to_string();
        };
        let Some(obj) = value.as_object() else {
            return json.to_string();
        };
        match checksum_fields {
            Some(fields) => {
                let filtered: serde_json::Map<String, serde_json::Value> = obj.iter()
                    .filter(|(k, _)| fields.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                serde_json::to_string(&filtered).unwrap_or_else(|_| json.to_string())
            }
            // Lua plugins: no checksum_fields declaration, use full config
            None => json.to_string(),
        }
    }

    /// Create dependency analyzers for each declared instance in `[analyzer.*]`.
    /// Only analyzers that appear in the config and have `enabled = true` are
    /// instantiated. Setting `enabled = false` lets a user disable an analyzer
    /// without removing its `[analyzer.X]` section.
    pub(crate) fn create_analyzers(&self, verbose: bool) -> Result<HashMap<String, Box<dyn DepAnalyzer>>> {
        let mut analyzers: HashMap<String, Box<dyn DepAnalyzer>> = HashMap::new();
        for inst in &self.config.analyzer.instances {
            let plugin = crate::registries::find_analyzer_plugin(&inst.type_name)
                .ok_or_else(|| anyhow::anyhow!("Unknown analyzer type '{}'", inst.type_name))?;
            let analyzer = (plugin.create)(&inst.instance_name, &inst.config_toml, verbose)?;
            if !analyzer.enabled() {
                continue;
            }
            analyzers.insert(inst.instance_name.clone(), analyzer);
        }
        Ok(analyzers)
    }

    /// Run all declared dependency analyzers on the graph.
    ///
    /// Three-stage output matching the build phase's shape:
    ///   1. Forward total — how many files would be scanned if nothing is cached.
    ///   2. Pre-scan classify breakdown — how many are expected to hit the cache
    ///      vs need rescanning, based on a cheap dry-run of the validity check
    ///      (mtime shortcut where possible, full read + hash when mtime changed).
    ///   3. Post-scan summary — what actually happened.
    ///
    /// The pre-scan classify pass is observable separately (a place the user
    /// can stop and inspect state). It does the same work the scan would do
    /// anyway, but reads the result instead of acting on it — and the
    /// in-memory checksum cache in `checksum.rs` dedupes work across the two
    /// passes, so nothing is read+hashed twice.
    fn run_analyzers(&self, ctx: &crate::build_context::BuildContext, graph: &mut BuildGraph, verbose: bool) -> Result<()> {
        let analyzers = self.create_analyzers(verbose)?;
        let mut deps_cache = DepsCache::open()?;

        // Only run analyzers that auto-detect relevant files in the project.
        let active_analyzers: Vec<&String> = sorted_keys(&analyzers).into_iter()
            .filter(|name| analyzers[*name].auto_detect(&self.file_index))
            .collect();

        if active_analyzers.is_empty() {
            return Ok(());
        }

        // Per-product reference count: every matching product becomes one
        // reference, even when many products share the same source. Used
        // to drive the progress bar (which ticks once per product) and to
        // surface fan-out in the user-facing total.
        let product_refs: usize = active_analyzers.iter()
            .map(|name| analyzers[*name].count_matches(graph))
            .sum();

        // Unique source count: dedup (analyzer, source) pairs. This matches
        // what the cache and the scanner actually see — one cache key and one
        // file read per pair, regardless of how many products reference it.
        let mut unique_pairs: std::collections::HashSet<(&str, PathBuf)> = std::collections::HashSet::new();
        for name in &active_analyzers {
            for source in analyzers[*name].matching_sources(graph) {
                unique_pairs.insert((name.as_str(), source));
            }
        }
        let unique_count = unique_pairs.len();

        let suppress = crate::json_output::is_json_mode() || crate::runtime_flags::quiet();

        // Stage 1: announce the work in unique-source units, with the fan-out
        // shown in parentheses when products outnumber unique sources.
        if unique_count > 0 && !suppress {
            if product_refs > unique_count {
                println!(
                    "[deps] {unique_count} files to check (consumed by {product_refs} products)",
                );
            } else {
                println!("[deps] {unique_count} files to check");
            }
        }

        // Stage 2: pre-scan classify. Walk the deduped (analyzer, source) set
        // and classify each pair exactly once. Splits the hit count into mtime
        // vs checksum so the user can see how much of the cache work was
        // I/O-free (mtime matched) vs had to re-hash the file (mtime stale but
        // content unchanged, e.g. a touched file).
        let mut mtime_hits: usize = 0;
        let mut content_hits: usize = 0;
        let mut misses: usize = 0;
        for (name, source) in &unique_pairs {
            match deps_cache.classify(ctx, name, source) {
                crate::deps_cache::ClassifyResult::MtimeHit => mtime_hits += 1,
                crate::deps_cache::ClassifyResult::ContentHit => content_hits += 1,
                crate::deps_cache::ClassifyResult::Miss => misses += 1,
            }
        }
        if unique_count > 0 && !suppress {
            let cached = mtime_hits + content_hits;
            println!(
                "[deps] {misses} to rescan ({cached} cached: {mtime_hits} mtime, {content_hits} checksum)",
            );
        }

        // Stage 3: actual scan. Progress bar still ticks per-product since
        // `analyze_with_scanner` ticks once per consuming product (so the bar
        // matches what users expect from the build graph).
        let hidden = verbose || crate::json_output::is_json_mode() || crate::runtime_flags::quiet();
        let pb = crate::progress::create_bar(product_refs as u64, hidden);
        for name in &active_analyzers {
            analyzers[*name].analyze(ctx, graph, &mut deps_cache, &self.file_index, verbose, &pb)?;
        }
        pb.finish_and_clear();

        let stats = deps_cache.stats();
        if unique_count > 0 && !suppress {
            println!(
                "[deps] summary: {} rescanned ({} cache hits: {} mtime, {} checksum)",
                stats.misses, stats.hits, stats.mtime_hits, stats.content_hits,
            );
        }

        Ok(())
    }

    /// Build the dependency graph using provided processors
    fn build_graph_with_processors(&self, ctx: &crate::build_context::BuildContext, processors: &ProcessorMap) -> Result<BuildGraph> {
        let (graph, _) = self.build_graph_with_processors_impl(ctx, processors, GraphBuildMode::Normal, BuildPhase::Build, None, false)?;
        Ok(graph)
    }

    /// Build the dependency graph with optional early stopping
    fn build_graph_with_processors_and_phase(&self, ctx: &crate::build_context::BuildContext, processors: &ProcessorMap, stop_after: BuildPhase, processor_filter: Option<&[String]>, verbose: bool) -> Result<(BuildGraph, PhaseTimings)> {
        self.build_graph_with_processors_impl(ctx, processors, GraphBuildMode::Normal, stop_after, processor_filter, verbose)
    }

    /// Build the dependency graph for clean (skip expensive dependency scanning)
    fn build_graph_for_clean_with_processors(&self, ctx: &crate::build_context::BuildContext, processors: &ProcessorMap) -> Result<BuildGraph> {
        let (graph, _) = self.build_graph_with_processors_impl(ctx, processors, GraphBuildMode::ForClean, BuildPhase::Build, None, false)?;
        Ok(graph)
    }

    /// Return the set of processor type names whose files are detected in the project.
    /// Uses default configs for all builtin processors to check auto_detect.
    pub fn detected_processors(&self) -> Result<std::collections::HashSet<String>> {
        let processors = create_all_default_processors()?;
        let mut detected = std::collections::HashSet::new();
        for (name, proc) in &processors {
            if proc.auto_detect(&self.file_index) {
                detected.insert(name.clone());
            }
        }
        Ok(detected)
    }

    /// Return the set of processor type names whose files are detected AND whose
    /// required tools are all installed.
    pub fn detected_and_available_processors(&self) -> Result<std::collections::HashSet<String>> {
        let processors = create_all_default_processors()?;
        let mut available = std::collections::HashSet::new();
        for (name, proc) in &processors {
            if !proc.auto_detect(&self.file_index) {
                continue;
            }
            let tools = proc.required_tools();
            if tools.iter().all(|t| which::which(t).is_ok()) {
                available.insert(name.clone());
            }
        }
        Ok(available)
    }

    /// Return the set of configured processor instance names that have 0 products
    /// (i.e., don't match any files).
    pub fn no_file_processors(&self, ctx: &crate::build_context::BuildContext) -> Result<Vec<String>> {
        let processors = self.create_processors()?;
        let graph = self.build_graph_with_processors(ctx, &processors)?;

        let mut has_products: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for product in graph.products() {
            has_products.insert(product.processor.as_str());
        }

        let mut empty: Vec<String> = sorted_keys(&processors)
            .into_iter()
            .filter(|name| !has_products.contains(name.as_str()))
            .cloned()
            .collect();
        empty.sort();
        Ok(empty)
    }

    /// Check whether a processor should run. In the instance-based model,
    /// all declared processors are active (existence in config = enabled).
    fn is_processor_active(&self, _name: &str, _processor: &dyn Processor) -> bool {
        true
    }

    /// Fixed-point product discovery loop.
    /// Runs discovery for all active processors, then injects declared outputs
    /// as virtual files so downstream processors can discover products for files
    /// that don't exist on disk yet. Repeats until no new products are found.
    fn discover_products(
        &self,
        graph: &mut BuildGraph,
        processors: &ProcessorMap,
        active: &[impl AsRef<str>],
        for_clean: bool,
    ) -> Result<()> {
        let mut file_index = self.file_index.clone();
        let debug = phases_debug();

        for pass in 0..10 {
            let before = graph.products().len();
            for name in active {
                let name = name.as_ref();
                if !processors[name].scan_config().enabled {
                    continue;
                }
                if for_clean {
                    processors[name].discover_for_clean(graph, &file_index, name)?;
                } else {
                    processors[name].discover(graph, &file_index, name)?;
                }
            }
            let after = graph.products().len();
            if after == before {
                break;
            }
            if pass + 1 >= 10 {
                break;
            }
            let outputs: Vec<PathBuf> = graph.products()[before..after]
                .iter()
                .flat_map(|p| p.outputs.iter().cloned())
                .collect();
            let added = file_index.add_virtual_files(&outputs);
            if added == 0 {
                break;
            }
            if debug {
                eprintln!("{}", color::dim(&format!(
                    "    discover pass {}: {} new products, {} virtual files added",
                    pass + 1, after - before, added
                )));
            }
        }

        // After the fixed-point loop has settled, every src_dirs entry must
        // be backed by something — either a real directory on disk or virtual
        // files injected by an upstream processor's declared outputs. A dir
        // that is neither is a typo and must fail. Skip when src_files is set
        // (file-list mode bypasses src_dirs).
        let mut missing: Vec<String> = Vec::new();
        for name in active {
            let name = name.as_ref();
            let scan = processors[name].scan_config();
            if !scan.enabled {
                continue;
            }
            if !scan.src_files().is_empty() {
                continue;
            }
            for dir in scan.src_dirs() {
                if dir.is_empty() || dir == "." {
                    continue;
                }
                if std::path::Path::new(dir).is_dir() {
                    continue;
                }
                let prefix = std::path::PathBuf::from(dir);
                let covered_by_virtual = file_index.files().iter().any(|f| f.starts_with(&prefix));
                if !covered_by_virtual {
                    missing.push(format!("[{name}]: src_dirs entry '{dir}' does not exist or is not a directory"));
                }
            }
        }
        if !missing.is_empty() {
            anyhow::bail!("Invalid config:\n{}", missing.join("\n"));
        }
        Ok(())
    }

    /// Build the dependency graph using provided processors
    /// processor_filter: if Some, only run processors in this list (in addition to enabled check)
    fn build_graph_with_processors_impl(&self, ctx: &crate::build_context::BuildContext, processors: &ProcessorMap, mode: GraphBuildMode, stop_after: BuildPhase, processor_filter: Option<&[String]>, verbose: bool) -> Result<(BuildGraph, PhaseTimings)> {
        if phases_debug() {
            eprintln!("{}", color::bold("Phase: Building dependency graph..."));
        }
        let mut graph = BuildGraph::new();
        let mut phase_timings = PhaseTimings::new();
        print_graph_stats(GraphSnapshot::Start, &graph);

        // Collect which processors should run
        let active_processors: Vec<&String> = sorted_keys(processors).into_iter()
            .filter(|name| {
                if let Some(filter) = processor_filter
                    && !filter.iter().any(|f| f == *name) {
                        return false;
                    }
                self.is_processor_active(name, processors[*name].as_ref())
            })
            .collect();

        // Phase 1: Discover products
        if phases_debug() {
            eprintln!("{}", color::dim("  Phase: discover"));
        }
        let t = Instant::now();
        self.discover_products(&mut graph, processors, &active_processors, mode == GraphBuildMode::ForClean)?;
        phase_timings.push(("discover".to_string(), t.elapsed()));
        print_graph_stats(GraphSnapshot::AfterDiscover, &graph);

        if stop_after == BuildPhase::Discover {
            return Ok((graph, phase_timings));
        }

        // Phase 2: Run dependency analyzers (only for regular builds, not clean)
        if mode == GraphBuildMode::Normal {
            if phases_debug() {
                eprintln!("{}", color::dim("  Phase: add_dependencies"));
            }
            let t = Instant::now();
            self.run_analyzers(ctx, &mut graph, verbose)?;
            phase_timings.push(("add_dependencies".to_string(), t.elapsed()));
            print_graph_stats(GraphSnapshot::AfterAddDependencies, &graph);
        }

        if stop_after == BuildPhase::AddDependencies {
            return Ok((graph, phase_timings));
        }

        // Phase 3: Apply tool version hashes
        if phases_debug() {
            eprintln!("{}", color::dim("  Phase: apply_tool_version_hashes"));
        }
        let t = Instant::now();
        let tool_hashes = tool_lock::processor_tool_hashes(
            processors,
            &|_name| true, // All declared processors are active
        )?;
        if !tool_hashes.is_empty() {
            graph.apply_tool_version_hashes(&tool_hashes);
        }
        phase_timings.push(("tool_version_hashes".to_string(), t.elapsed()));
        print_graph_stats(GraphSnapshot::AfterApplyToolHashes, &graph);

        // Phase 4: Resolve dependencies
        if phases_debug() {
            eprintln!("{}", color::dim("  Phase: resolve_dependencies"));
        }
        let t = Instant::now();
        graph.resolve_dependencies();
        phase_timings.push(("resolve".to_string(), t.elapsed()));
        print_graph_stats(GraphSnapshot::AfterResolve, &graph);

        // Phase 5: Validate graph
        let t = Instant::now();
        let validation_errors = graph.validate(&self.config.graph);
        if !validation_errors.is_empty() {
            anyhow::bail!("Graph validation failed:\n{}", validation_errors.join("\n"));
        }
        phase_timings.push(("validate".to_string(), t.elapsed()));

        // Note: BuildPhase::Resolve and BuildPhase::Build both complete the graph
        Ok((graph, phase_timings))
    }

    /// Build the dependency graph, optionally filtering to a single processor.
    /// If `include_all` is true, skip enabled/auto-detect checks.
    pub fn build_graph_filtered(
        &self,
        ctx: &crate::build_context::BuildContext,
        filter_name: Option<&str>,
        include_all: bool,
    ) -> Result<BuildGraph> {
        let processors = self.create_processors()?;
        let mut graph = BuildGraph::new();

        // Collect active processors
        let active_processors: Vec<&String> = sorted_keys(&processors).into_iter()
            .filter(|name| {
                if let Some(filter) = filter_name
                    && name.as_str() != filter {
                        return false;
                    }
                include_all || self.is_processor_active(name, processors[*name].as_ref())
            })
            .collect();

        // Phase 1: Discover products (fixed-point loop for cross-processor deps)
        self.discover_products(&mut graph, &processors, &active_processors, false)?;

        // Phase 2: Run dependency analyzers
        self.run_analyzers(ctx, &mut graph, false)?;

        graph.resolve_dependencies();

        let validation_errors = graph.validate(&self.config.graph);
        if !validation_errors.is_empty() {
            anyhow::bail!("Graph validation failed:\n{}", validation_errors.join("\n"));
        }

        Ok(graph)
    }

    /// Build the dependency graph (creates processors internally)
    fn build_graph(&self, ctx: &crate::build_context::BuildContext) -> Result<BuildGraph> {
        let processors = self.create_processors()?;
        self.build_graph_with_processors(ctx, &processors)
    }

    /// Build the dependency graph for cache operations (public).
    pub fn build_graph_for_cache(&self, ctx: &crate::build_context::BuildContext) -> Result<BuildGraph> {
        self.build_graph(ctx)
    }

    /// Compute the set of valid cache keys from the current build graph.
    pub fn valid_cache_keys(&self, ctx: &crate::build_context::BuildContext) -> Result<std::collections::HashSet<String>> {
        let graph = self.build_graph_for_cache(ctx)?;
        Ok(graph.products().iter().map(super::graph::Product::cache_key).collect())
    }

    /// Get a reference to the object store.
    pub const fn object_store(&self) -> &ObjectStore {
        &self.object_store
    }

    /// The configured global output directory (default "out").
    pub fn output_dir(&self) -> &str {
        &self.config.build.output_dir
    }

    /// Return directories that should be watched for file changes.
    /// Derived from processor scan configs plus standard project files.
    pub fn watch_paths(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = Vec::new();

        // Always watch rsconstruct.toml
        let config_path = PathBuf::from("rsconstruct.toml");
        if config_path.exists() {
            paths.push(config_path);
        }

        // Add scan directories from all processor configs
        for dir in self.config.processor.src_dirs() {
            let full = PathBuf::from(&dir);
            if full.exists() {
                paths.push(full);
            }
        }

        // Processors with empty default_src_dirs scan the project root — watch common
        // top-level files/dirs that wouldn't be covered by src_dirs above.
        for name in &["pyproject.toml", "config"] {
            let p = PathBuf::from(name);
            if p.exists() {
                paths.push(p);
            }
        }

        // Plugins directory
        let plugins = PathBuf::from(&self.config.plugins.dir);
        if plugins.exists() {
            paths.push(plugins);
        }

        paths.sort();
        paths.dedup();
        paths
    }
}
