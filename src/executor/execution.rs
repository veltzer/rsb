use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use indicatif::ProgressBar;
use parking_lot::{Condvar, Mutex};
use std::thread;
use std::time::Instant;

use crate::color;
use crate::errors;
use crate::graph::{BuildGraph, Product};
use crate::json_output;
use crate::object_store::ObjectStore;
use crate::processors::{BuildStats, ProductTiming};
use crate::progress;

use super::{Classification, Executor, HandlerContext, LevelWork, PreCheckResult, RestoreOutcome, SharedState, WorkItem};

/// Compute the effective `max_jobs` for a processor instance. The config
/// field is capped by the plugin's static `max_jobs_cap`; `None` on either
/// side means no limit on that side.
fn effective_max_jobs(name: &str, proc: &dyn crate::processors::Processor) -> Option<usize> {
    let plugin = crate::registries::processor::find_plugin(name);
    let user_set = proc.scan_config().max_jobs;
    let static_cap = plugin.and_then(|p| p.max_jobs_cap);
    match (user_set, static_cap) {
        (Some(c), Some(m)) => Some(c.min(m)),
        (Some(c), None)    => Some(c),
        (None, Some(m))    => Some(m),
        (None, None)       => None,
    }
}

/// Compute the effective `supports_batch` for a processor instance: the
/// plugin's static capability flag must be true, AND the user config must
/// request batching.
fn effective_supports_batch(name: &str, proc: &dyn crate::processors::Processor) -> bool {
    let plugin_ok = crate::registries::processor::find_plugin(name).is_some_and(|p| p.supports_batch);
    plugin_ok && proc.scan_config().batch
}

/// Chunk size for one batch group. `0` means no limit — the whole group in
/// one chunk. Batching disabled (`batch_size: None`) never forms batch
/// groups (see [`should_batch`]), so this only sees explicit sizes. Never
/// returns 0 — `chunks(0)` panics.
fn batch_chunk_size(batch_size: usize, n_items: usize) -> usize {
    if batch_size == 0 { n_items.max(1) } else { batch_size }
}

/// Whether a processor's items take the batch path. Batching requires an
/// explicit batch size (`batch_size: None` disables it entirely), a
/// processor that supports it, and more than one item actually rebuilding.
const fn should_batch(batching_enabled: bool, supports_batch: bool, rebuild_count: usize) -> bool {
    batching_enabled && supports_batch && rebuild_count > 1
}

/// A simple counting semaphore for limiting per-processor concurrency.
struct Semaphore {
    state: Mutex<usize>,
    condvar: Condvar,
    max_permits: usize,
}

impl Semaphore {
    const fn new(max_permits: usize) -> Self {
        Self {
            state: Mutex::new(0),
            condvar: Condvar::new(),
            max_permits,
        }
    }

    fn acquire(&self) {
        let mut active = self.state.lock();
        while *active >= self.max_permits {
            self.condvar.wait(&mut active);
        }
        *active += 1;
    }

    fn release(&self) {
        let mut active = self.state.lock();
        *active -= 1;
        drop(active);
        self.condvar.notify_one();
    }
}

/// Remove existing output files before executing a product.
///
/// Cache objects are stored read-only to prevent corruption via hardlinks.
/// When a restored hardlink exists and a rebuild is needed, the tool cannot
/// overwrite the read-only file. Removing outputs first ensures the tool
/// can create fresh files.
///
/// For products with `output_dirs` (Creators), we do NOT wipe entire directories
/// because other processors may contribute files to the same directory. Instead,
/// we remove just the files recorded in this product's last tree descriptor
/// (falling back to creating empty dirs if there is no prior tree).
pub(super) fn remove_stale_outputs(
    product: &Product,
    object_store: &ObjectStore,
    input_checksum: &str,
) -> anyhow::Result<()> {
    if !product.output_dirs.is_empty() {
        // Creator-style: remove only files we owned in the previous run.
        // Files that belong to other processors (sharing the same directory)
        // are left alone and restored/rebuilt independently.
        let cache_key = product.descriptor_key(input_checksum);
        for file in object_store.previous_tree_paths(&cache_key) {
            if file.exists() {
                fs::remove_file(&file)
                    .with_context(|| format!("Failed to remove stale output: {}", file.display()))?;
            }
        }
        // Ensure output dirs still exist (the tool may assume they do)
        for output_dir in &product.output_dirs {
            fs::create_dir_all(output_dir.as_ref())
                .with_context(|| format!("Failed to create output directory: {}", output_dir.display()))?;
        }
    }
    for output in &product.outputs {
        if output.exists() {
            fs::remove_file(output)
                .with_context(|| format!("Failed to remove stale output: {}", output.display()))?;
        }
    }
    Ok(())
}

/// Per-processor progress counters shared across non-batch threads.
struct ProgressCounters {
    total_per_processor: Arc<HashMap<String, usize>>,
    current_per_processor: Arc<Mutex<HashMap<String, usize>>>,
}

/// Common context shared by both batch and non-batch processing threads within a level.
struct LevelContext<'b> {
    graph: &'b BuildGraph,
    object_store: &'b ObjectStore,
    force: bool,
    keep_going: bool,
    timings: bool,
    shared: &'b SharedState,
    pb: &'b ProgressBar,
    build_start: Instant,
}

impl Executor<'_> {
    /// Execute all products in the graph that need rebuilding.
    ///
    /// `classification` is the result of an earlier [`classify_products`]
    /// pass. The executor reuses the per-product `input_checksum` values
    /// recorded there instead of recomputing them, which matters when
    /// outputs of one product are inputs of another and were unlinked
    /// between classify and execute (see [`unlink_pending_outputs`]).
    pub fn execute(
        &self,
        graph: &BuildGraph,
        object_store: &ObjectStore,
        force: bool,
        timings: bool,
        keep_going: bool,
        classification: &Classification,
    ) -> Result<BuildStats> {
        let build_start = Instant::now();
        let order = graph.topological_sort()?;

        // Emit JSON build start event
        json_output::emit_build_start(order.len());

        let result = self.execute_parallel(graph, &order, object_store, force, timings, keep_going, classification);

        match result {
            Ok(mut stats) => {
                stats.total_duration = build_start.elapsed();

                // Emit JSON build summary
                json_output::emit_build_summary(
                    stats.total_processed() + stats.total_skipped() + stats.total_restored() + stats.failed_count,
                    stats.total_processed(),
                    stats.failed_count,
                    stats.total_skipped(),
                    stats.total_restored(),
                    stats.total_duration,
                    &stats.failed_messages,
                );

                Ok(stats)
            }
            Err(e) => {
                // Emit JSON build summary even on failure
                let duration = build_start.elapsed();
                json_output::emit_build_summary(
                    order.len(),
                    0,
                    1,
                    0,
                    0,
                    duration,
                    &[e.to_string()],
                );
                Err(e)
            }
        }
    }

    /// Execute products in parallel where dependencies allow.
    /// Within each level, batch-supporting processors with multiple items
    /// are grouped and executed via `execute_batch()` in a single thread.
    fn execute_parallel(
        &self,
        graph: &BuildGraph,
        order: &[usize],
        object_store: &ObjectStore,
        force: bool,
        timings: bool,
        keep_going: bool,
        classification: &Classification,
    ) -> Result<BuildStats> {
        let build_start = Instant::now();
        // Group products into levels that can run in parallel
        let levels = super::compute_parallel_levels(graph, order);

        // Count total products per processor for progress display
        let mut total_per_processor: HashMap<String, usize> = HashMap::new();
        for &product_id in order {
            let product = graph.get_product(product_id).expect(errors::INVALID_PRODUCT_ID);
            *total_per_processor.entry(product.processor.clone()).or_insert(0) += 1;
        }
        let counters = ProgressCounters {
            total_per_processor: Arc::new(total_per_processor),
            current_per_processor: Arc::new(Mutex::new(HashMap::new())),
        };
        let global_total = order.len();

        // Use the caller-provided classification for progress bar sizing.
        let work_count = classification.restore_count + classification.build_count;

        // Create progress bar sized to actual work (excludes instant skips)
        let pb = progress::create_bar(
            work_count as u64,
            self.verbose || json_output::is_json_mode() || crate::runtime_flags::quiet(),
        );

        let shared = SharedState {
            stats: Arc::new(Mutex::new(HashMap::new())),
            errors: Arc::new(Mutex::new(Vec::new())),
            failed_products: Arc::new(Mutex::new(HashSet::new())),
            failed_messages: Arc::new(Mutex::new(Vec::new())),
            failed_details: Arc::new(Mutex::new(Vec::new())),
            failed_processors: Arc::new(Mutex::new(HashSet::new())),
            unchanged_products: Arc::new(Mutex::new(HashSet::new())),
            global_current: Arc::new(AtomicUsize::new(0)),
            global_total,
        };

        // Build per-processor semaphores for max_jobs limits
        let semaphores: HashMap<String, Arc<Semaphore>> = self.processors.iter()
            .filter_map(|(name, proc)| {
                effective_max_jobs(name, proc.as_ref()).map(|max| (name.clone(), Arc::new(Semaphore::new(max))))
            })
            .collect();

        for level in levels {
            // Check for Ctrl+C before starting next level
            if self.is_interrupted() {
                break;
            }

            let LevelWork { batch_groups, non_batch_items } = self.prepare_level_work(
                graph, &level, object_store, force, keep_going, &shared,
            );

            let lctx = LevelContext {
                graph, object_store, force, keep_going, timings,
                shared: &shared, pb: &pb, build_start,
            };

            // Process this level in parallel using thread pool
            thread::scope(|s| {
                let lctx_ref = &lctx;
                let semaphores_ref = &semaphores;

                // Spawn one thread per batch group
                for (proc_name, items) in &batch_groups {
                    s.spawn(move || {
                        self.process_batch_group(proc_name, items, lctx_ref);
                    });
                }

                // Spawn threads for non-batch items (chunked across threads)
                if !non_batch_items.is_empty() {
                    let chunk_size = non_batch_items.len().div_ceil(self.parallel.max(1));

                    for chunk in non_batch_items.chunks(chunk_size.max(1)) {
                        let total_ref = Arc::clone(&counters.total_per_processor);
                        let current_ref = Arc::clone(&counters.current_per_processor);

                        s.spawn(move || {
                            self.process_non_batch_chunk(
                                chunk, lctx_ref, &total_ref, &current_ref,
                                semaphores_ref,
                            );
                        });
                    }
                }
            });

            // If interrupted, stop processing further levels
            if self.is_interrupted() {
                crate::output::info(&color::yellow("Interrupted, saving progress..."));
                break;
            }

            // In non-keep-going mode, stop after level with errors
            if !keep_going && !shared.errors.lock().is_empty() {
                break;
            }
        }

        pb.finish_and_clear();
        Self::collect_build_stats(shared, keep_going, self.is_interrupted())
    }

    /// Pre-check a work item: handle explain, skip-if-unchanged, and cache restore.
    ///
    /// Returns `Handled` if the item was fully processed (skip/restore/failed),
    /// or `NeedsExecution` if the caller should proceed to execute the product.
    fn try_skip_or_restore(
        &self,
        item: &WorkItem,
        proc_name: &str,
        lctx: &LevelContext,
        emit_fail_event: bool,
    ) -> PreCheckResult {
        let product = lctx.graph.get_product(item.product_id).expect(errors::INVALID_PRODUCT_ID);

        if self.explain {
            let action = self.policy.explain(self.build_ctx, product, lctx.object_store, &item.input_checksum, lctx.force);
            self.print_explain(product, &action);
        }

        if !item.needs_rebuild {
            self.handle_skip(product, lctx.shared);
            return PreCheckResult::Handled;
        }

        let ctx = HandlerContext {
            product, id: item.product_id, input_checksum: &item.input_checksum,
            proc_name, keep_going: lctx.keep_going, shared: lctx.shared, pb: lctx.pb,
        };
        match self.handle_restore(&ctx, lctx.object_store, lctx.force, emit_fail_event) {
            RestoreOutcome::Restored | RestoreOutcome::Failed => PreCheckResult::Handled,
            RestoreOutcome::NotRestorable => PreCheckResult::NeedsExecution,
        }
    }

    /// Process a single batch group within a thread.
    fn process_batch_group(
        &self,
        proc_name: &str,
        items: &[WorkItem],
        lctx: &LevelContext,
    ) {
        if self.is_interrupted() {
            return;
        }

        let Some(processor) = self.processors.get(proc_name) else { return };

        // Handle skip/restore for items that don't need rebuild
        let mut to_execute: Vec<&WorkItem> = Vec::new();
        for item in items {
            match self.try_skip_or_restore(item, proc_name, lctx, false) {
                PreCheckResult::Handled => continue,
                PreCheckResult::NeedsExecution => to_execute.push(item),
            }
        }

        if to_execute.is_empty() || self.is_interrupted() {
            return;
        }

        let proc_total = items.len();
        let mut proc_current = items.len() - to_execute.len();

        let chunk_size = batch_chunk_size(
            self.batch_size.expect("batch groups only form when batching is enabled"),
            to_execute.len(),
        );

        // Process in chunks
        for chunk in to_execute.chunks(chunk_size) {
            if self.is_interrupted()
                || (!lctx.keep_going && !lctx.shared.errors.lock().is_empty())
            {
                break;
            }

            // Execute batch chunk
            let product_refs: Vec<&crate::graph::Product> = chunk.iter()
                .map(|item| lctx.graph.get_product(item.product_id).expect(errors::INVALID_PRODUCT_ID))
                .collect();

            proc_current += chunk.len();
            if self.verbose {
                let display = product_refs.iter()
                    .map(|p| self.product_display(p))
                    .collect::<Vec<_>>()
                    .join(", ");
                let gc = lctx.shared.global_current.load(Ordering::SeqCst);
                crate::output::info(&format!("[{}] ({}/{}) ({}/{}) {} {} files: {}",
                    proc_name,
                    gc + 1, lctx.shared.global_total,
                    proc_current, proc_total,
                    color::green("Processing batch:"),
                    product_refs.len(),
                    display));
            } else {
                lctx.pb.set_message(format!("[{}] batch {} files", proc_name, product_refs.len()));
            }

            // Outputs were already unlinked at classify time; just announce starts.
            for p in &product_refs {
                json_output::emit_product_start(&self.product_display(p), &p.processor);
            }
            let batch_start = Instant::now();
            crate::processors::set_declared_tools(Some(processor.required_tools()));
            let results = processor.execute_batch(self.build_ctx, &product_refs);
            crate::processors::set_declared_tools(None);
            let batch_duration = batch_start.elapsed();

            // Validate batch returned correct number of results
            assert_eq!(results.len(), chunk.len(),
                "execute_batch returned {} results for {} products (processor: {})",
                results.len(), chunk.len(), proc_name);

            // Process per-product results
            for (item, result) in chunk.iter().zip(results) {
                let product = lctx.graph.get_product(item.product_id).expect(errors::INVALID_PRODUCT_ID);

                let ctx = HandlerContext {
                    product, id: item.product_id, input_checksum: &item.input_checksum,
                    proc_name, keep_going: lctx.keep_going, shared: lctx.shared, pb: lctx.pb,
                };
                match result {
                    Ok(()) => {
                        // handle_success returns false if cache_outputs failed
                        self.handle_success(&ctx, lctx.object_store, lctx.graph, None);
                    }
                    Err(e) => {
                        self.handle_error(&ctx, e, None);
                    }
                }
                Self::inc_progress(lctx.pb, lctx.shared);
            }

            // Record batch timing for this chunk
            if lctx.timings {
                let timing = ProductTiming {
                    display: format!("batch ({} files)", product_refs.len()),
                    processor: proc_name.to_string(),
                    duration: batch_duration,
                    start_offset: Some(batch_start.duration_since(lctx.build_start)),
                };
                let mut stats = lctx.shared.stats.lock();
                let proc_stats = stats
                    .entry(proc_name.to_string())
                    .or_default();
                proc_stats.duration += batch_duration;
                proc_stats.product_timings.push(timing);
            }
        }
    }

    /// Process a chunk of non-batch work items within a thread.
    fn process_non_batch_chunk(
        &self,
        chunk: &[WorkItem],
        lctx: &LevelContext,
        total_per_processor: &HashMap<String, usize>,
        current_per_processor: &Mutex<HashMap<String, usize>>,
        semaphores: &HashMap<String, Arc<Semaphore>>,
    ) {
        for item in chunk {
            // Stop if interrupted or if there's an error (non-keep-going mode)
            if self.is_interrupted()
                || (!lctx.keep_going && !lctx.shared.errors.lock().is_empty())
            {
                break;
            }

            let product = lctx.graph.get_product(item.product_id).expect(errors::INVALID_PRODUCT_ID);

            if matches!(self.try_skip_or_restore(item, &product.processor, lctx, true), PreCheckResult::Handled) {
                continue;
            }

            // Acquire per-processor semaphore permit if max_jobs is set
            let semaphore = semaphores.get(&product.processor);
            if let Some(sem) = semaphore {
                sem.acquire();
            }

            if let Some(processor) = self.processors.get(&product.processor) {
                let ctx = HandlerContext {
                    product, id: item.product_id, input_checksum: &item.input_checksum,
                    proc_name: &product.processor, keep_going: lctx.keep_going,
                    shared: lctx.shared, pb: lctx.pb,
                };

                // Update progress counter
                let current = {
                    let mut current_guard = current_per_processor.lock();
                    let c = current_guard.entry(product.processor.clone()).or_insert(0);
                    *c += 1;
                    *c
                };
                let total = total_per_processor.get(&product.processor).copied()
                    .expect(errors::PROCESSOR_NOT_IN_TOTALS);

                if self.verbose {
                    let variant_tag = product.variant.as_ref()
                        .map(|v| format!(":{v}"))
                        .unwrap_or_default();
                    let gc = lctx.shared.global_current.load(Ordering::SeqCst) + 1;
                    crate::output::info(&format!("[{}{}] ({}/{}) ({}/{}) {} {}", product.processor, variant_tag,
                        gc, lctx.shared.global_total,
                        current, total,
                        color::green("Processing:"),
                        self.product_display(product)));
                } else {
                    let variant_tag = product.variant.as_ref()
                        .map(|v| format!(":{v}"))
                        .unwrap_or_default();
                    lctx.pb.set_message(format!("[{}{}] {}", product.processor, variant_tag, self.product_display(product)));
                }

                json_output::emit_product_start(&self.product_display(product), &product.processor);
                let product_start = Instant::now();
                let mut last_error = None;
                let max_attempts = 1 + self.retry;
                crate::processors::set_declared_tools(Some(processor.required_tools()));
                for attempt in 1..=max_attempts {
                    match processor.execute(self.build_ctx, product) {
                        Ok(()) => {
                            let duration = product_start.elapsed();
                            last_error = None; // Clear before handle_success to avoid double-failure if caching fails

                            if !self.handle_success(&ctx, lctx.object_store, lctx.graph, Some(duration)) {
                                // cache_outputs failed and error was handled
                                break;
                            }

                            // Mark as flaky if it passed on a retry
                            if attempt > 1 {
                                let mut stats = lctx.shared.stats.lock();
                                let proc_stats = stats
                                    .entry(product.processor.clone())
                                    .or_default();
                                proc_stats.flaky += 1;
                                crate::output::info(&format!("[{}] {} {} (passed on attempt {})",
                                    product.processor,
                                    color::yellow("FLAKY:"),
                                    self.product_display(product),
                                    attempt));
                            }

                            // Record per-product duration (non-batch only)
                            {
                                let mut stats = lctx.shared.stats.lock();
                                let proc_stats = stats
                                    .entry(product.processor.clone())
                                    .or_default();
                                proc_stats.duration += duration;
                                if lctx.timings {
                                    proc_stats.product_timings.push(ProductTiming {
                                        display: self.product_display(product),
                                        processor: product.processor.clone(),
                                        duration,
                                        start_offset: Some(product_start.duration_since(lctx.build_start)),
                                    });
                                }
                            }
                            break;
                        }
                        Err(e) => {
                            if attempt < max_attempts {
                                crate::output::info(&format!("[{}] {} {} (attempt {}/{}, retrying...)",
                                    product.processor,
                                    color::yellow("Retry:"),
                                    self.product_display(product),
                                    attempt, max_attempts));
                                last_error = Some(e);
                            } else {
                                let duration = product_start.elapsed();
                                self.handle_error(&ctx, e, Some(duration));
                                last_error = None; // error already handled
                            }
                        }
                    }
                }
                // Safety: if we broke out of retry loop with unhandled error, handle it
                if let Some(e) = last_error {
                    let duration = product_start.elapsed();
                    self.handle_error(&ctx, e, Some(duration));
                }
                crate::processors::set_declared_tools(None);
                Self::inc_progress(lctx.pb, lctx.shared);
            }

            // Release per-processor semaphore permit
            if let Some(sem) = semaphore {
                sem.release();
            }
        }
    }

    /// Collect final build stats from shared state after all levels complete.
    fn collect_build_stats(shared: SharedState, keep_going: bool, interrupted: bool) -> Result<BuildStats> {
        let final_stats = Arc::try_unwrap(shared.stats)
            .map_err(|_| anyhow::anyhow!("internal error: outstanding Arc reference to stats"))?
            .into_inner();
        let mut stats = BuildStats::default();
        for (_, proc_stats) in final_stats {
            stats.add(proc_stats);
        }

        let final_failed = Arc::try_unwrap(shared.failed_products)
            .map_err(|_| anyhow::anyhow!("internal error: outstanding Arc reference to failed products"))?
            .into_inner();
        let final_msgs = Arc::try_unwrap(shared.failed_messages)
            .map_err(|_| anyhow::anyhow!("internal error: outstanding Arc reference to failed messages"))?
            .into_inner();
        let final_details = Arc::try_unwrap(shared.failed_details)
            .map_err(|_| anyhow::anyhow!("internal error: outstanding Arc reference to failed details"))?
            .into_inner();
        stats.failed_count = final_failed.len();
        stats.failed_messages = final_msgs;
        stats.failed_details = final_details;

        // In non-keep-going mode, return the first error after giving
        // independent products a chance to execute and be cached
        if !keep_going && !interrupted {
            let errs = Arc::try_unwrap(shared.errors)
                .map_err(|_| anyhow::anyhow!("internal error: outstanding Arc reference to errors"))?
                .into_inner();
            if let Some(first_err) = errs.into_iter().next() {
                return Err(first_err);
            }
        }

        Ok(stats)
    }

    /// Prepare work items for a single parallel level.
    ///
    /// Skips products with failed dependencies, recomputes per-product input
    /// checksums from current on-disk state, and separates items into batch
    /// groups vs non-batch items.
    pub(super) fn prepare_level_work(
        &self,
        graph: &BuildGraph,
        level: &[usize],
        object_store: &ObjectStore,
        force: bool,
        keep_going: bool,
        shared: &SharedState,
    ) -> LevelWork {
        let mut work_items: Vec<WorkItem> = Vec::new();

        // First pass: identify products with failed dependencies
        let mut skipped_ids: HashSet<usize> = HashSet::new();
        {
            let failed_guard = shared.failed_products.lock();
            for &id in level {
                if super::has_failed_dependency(graph, id, &failed_guard) {
                    let product = graph.get_product(id).expect(errors::INVALID_PRODUCT_ID);
                    crate::output::detail(self.verbose, &format!("[{}] {} {}", product.processor,
                        color::yellow("Skipping (dependency failed):"),
                        self.product_display(product)));
                    skipped_ids.insert(id);
                }
            }
        }
        if !skipped_ids.is_empty() {
            let mut failed_guard = shared.failed_products.lock();
            for id in &skipped_ids {
                failed_guard.insert(*id);
            }
        }

        // Second pass: determine work items for non-skipped products
        {
            let fp_guard = shared.failed_processors.lock();
            for &id in level {
                if skipped_ids.contains(&id) {
                    continue;
                }

                let product = graph.get_product(id).expect(errors::INVALID_PRODUCT_ID);

                // In non-keep-going mode, silently skip products from a
                // processor that failed in a previous level
                if !keep_going && fp_guard.contains(&product.processor) {
                    shared.failed_products.lock().insert(id);
                    continue;
                }
                // Recompute the input checksum here (per-level, in topological
                // order) rather than reusing the classify-time value. By the
                // time we reach this level, every upstream level has completed,
                // so reading inputs from disk gives the post-upstream content
                // that the cache descriptor must be keyed by — critical for
                // products whose primary inputs are produced by an upstream
                // (e.g., ipdfunite consuming marp's PDF). The classify-time
                // value can carry MISSING: for inputs that didn't exist yet,
                // which would mint a cache key the next classify can never match.
                //
                // For products whose own outputs appear as inputs (e.g., a
                // tera template with dep_inputs listing its own output): the
                // input is currently MISSING here because unlink_pending_outputs
                // removed it. That's fine: `needs_rebuild_descriptor` won't
                // find a cache entry under the MISSING-keyed descriptor (no
                // prior build cached under that key), so we build. After build,
                // handle_success recomputes from the post-execution state and
                // caches under the real-content key — which is what next
                // classify will look up.
                let input_checksum = match crate::checksum::combined_input_checksum(self.build_ctx, &product.inputs) {
                    Ok(cs) => cs,
                    Err(e) => {
                        if keep_going {
                            let msg = format!("[{}] {}: {}", product.processor, self.product_display(product), e);
                            // stderr: errors must survive --json, and must not
                            // corrupt the JSON event stream on stdout.
                            crate::output::error(&msg);
                            shared.failed_products.lock().insert(id);
                            shared.failed_messages.lock().push(msg);
                        } else {
                            shared.failed_products.lock().insert(id);
                            shared.errors.lock().push(e);
                        }
                        continue;
                    }
                };

                let desc_key = product.descriptor_key(&input_checksum);
                let needs_rebuild = force || object_store.needs_rebuild_descriptor(self.build_ctx, &desc_key, &product.outputs);
                work_items.push(WorkItem { product_id: id, input_checksum, needs_rebuild });
            }
        }

        // Separate work items into batch groups and non-batch items.
        // Batch groups: processor supports batch AND has >1 item that needs rebuild.
        let mut batch_groups: HashMap<String, Vec<WorkItem>> = HashMap::new();
        let mut non_batch_items: Vec<WorkItem> = Vec::new();

        // Group all items by processor name
        let mut by_processor: HashMap<String, Vec<WorkItem>> = HashMap::new();
        for item in work_items {
            let product = graph.get_product(item.product_id).expect(errors::INVALID_PRODUCT_ID);
            by_processor.entry(product.processor.clone()).or_default().push(item);
        }

        // Separate into batch vs non-batch
        // batch_size: None = disable batching, Some(0) = no limit, Some(n) = max n items
        let batching_enabled = self.batch_size.is_some();
        for (proc_name, items) in by_processor {
            let processor = self.processors.get(&proc_name);
            let supports_batch = processor.is_some_and(|p| effective_supports_batch(&proc_name, p.as_ref()));
            // Count items that actually need rebuild (not just cache-skip)
            let rebuild_count = items.iter().filter(|item| item.needs_rebuild).count();

            if should_batch(batching_enabled, supports_batch, rebuild_count) {
                batch_groups.insert(proc_name, items);
            } else {
                non_batch_items.extend(items);
            }
        }

        LevelWork { batch_groups, non_batch_items }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0` means the whole group in one chunk; an explicit size wins.
    #[test]
    fn batch_chunk_size_decision_table() {
        assert_eq!(batch_chunk_size(0, 7), 7, "0 means no limit");
        assert_eq!(batch_chunk_size(3, 7), 3, "explicit size wins");
        assert_eq!(batch_chunk_size(3, 2), 3, "size larger than group is harmless");
    }

    /// `chunks(0)` panics; the sizing function must never return 0 even for
    /// degenerate inputs.
    #[test]
    fn batch_chunk_size_never_zero() {
        assert_eq!(batch_chunk_size(0, 0), 1);
    }

    /// Batching needs all three conditions; in particular a single rebuilding
    /// item must go down the non-batch path, and `batch_size: None`
    /// (`--batch-size -1`) disables batching no matter what the processor
    /// supports.
    #[test]
    fn should_batch_requires_all_conditions() {
        assert!(should_batch(true, true, 2));
        assert!(!should_batch(true, true, 1), "one rebuilding item is not a batch");
        assert!(!should_batch(true, true, 0));
        assert!(!should_batch(true, false, 5), "processor must support batching");
        assert!(!should_batch(false, true, 5), "batch_size None disables batching entirely");
    }
}
