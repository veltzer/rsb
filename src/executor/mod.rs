mod execution;
mod handlers;
mod policy;

pub use policy::{BuildPolicy, IncrementalPolicy, ProductAction};

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use indicatif::ProgressBar;
use parking_lot::Mutex;

use crate::display::DisplayOptions;
use crate::errors;
use crate::color;
use crate::graph::BuildGraph;
use crate::object_store::{ExplainAction, ObjectStore};
use crate::processors::ProcessorMap;
use crate::stats::ProcessStats;

/// Result of the per-item skip/restore pre-check.
enum PreCheckResult {
    /// Item was handled (skipped, restored, or failed restore). Caller should move on.
    Handled,
    /// Item needs execution. Caller should proceed with running the processor.
    NeedsExecution,
}

/// Outcome of a cache restore attempt.
enum RestoreOutcome {
    /// Product was successfully restored from cache.
    Restored,
    /// Restore failed (error already handled/reported).
    Failed,
    /// Product is not restorable; caller should proceed with execution.
    NotRestorable,
}

/// A work item representing a product to be processed in a build level.
struct WorkItem {
    product_id: usize,
    input_checksum: String,
    needs_rebuild: bool,
}

/// Context passed to handler methods for a single product operation.
/// Groups the parameters common across `handle_restore`, `handle_error`, `handle_success`.
struct HandlerContext<'b> {
    product: &'b crate::graph::Product,
    id: usize,
    input_checksum: &'b str,
    proc_name: &'b str,
    keep_going: bool,
    shared: &'b SharedState,
    pb: &'b ProgressBar,
}

/// Prepared work for a single dependency level, split into batch and non-batch items.
struct LevelWork {
    batch_groups: HashMap<String, Vec<WorkItem>>,
    non_batch_items: Vec<WorkItem>,
}

/// Options for configuring an Executor instance.
#[derive(Debug)]
pub struct ExecutorOptions {
    pub parallel: usize,
    pub verbose: bool,
    pub display_opts: DisplayOptions,
    pub batch_size: Option<usize>,
    pub explain: bool,
    pub retry: usize,
}

/// Shared mutable state passed to product processing helpers.
#[derive(Debug)]
struct SharedState {
    stats: Arc<Mutex<HashMap<String, ProcessStats>>>,
    errors: Arc<Mutex<Vec<anyhow::Error>>>,
    failed_products: Arc<Mutex<HashSet<usize>>>,
    failed_messages: Arc<Mutex<Vec<String>>>,
    failed_processors: Arc<Mutex<HashSet<String>>>,
    global_current: Arc<AtomicUsize>,
    global_total: usize,
}

/// Per-product classification recorded by [`classify_products`].
/// `input_checksum` is empty when the checksum could not be computed
/// (which forces Build).
pub struct ClassifiedProduct {
    pub id: usize,
    pub action: ProductAction,
    pub input_checksum: String,
}

/// Result of [`classify_products`]: counts plus per-product actions in
/// topological order.
pub struct Classification {
    pub skip_count: usize,
    pub restore_count: usize,
    pub build_count: usize,
    pub products: Vec<ClassifiedProduct>,
}

/// Pre-build classification: count how many products will be skipped, restored, or built.
/// This is a fast read-only pass (checksums + cache lookups, no mutations).
/// Products are processed in topological order so that dependency changes propagate:
/// if a product will be rebuilt or restored, its dependents are also marked for rebuild.
pub fn classify_products(
    ctx: &crate::build_context::BuildContext,
    policy: &dyn BuildPolicy,
    graph: &BuildGraph,
    order: &[usize],
    object_store: &ObjectStore,
    force: bool,
) -> Classification {
    let mut skip_count = 0;
    let mut restore_count = 0;
    let mut build_count = 0;
    let mut will_change: HashSet<usize> = HashSet::new();
    let mut products: Vec<ClassifiedProduct> = Vec::with_capacity(order.len());

    for &id in order {
        let product = graph.get_product(id).expect(errors::INVALID_PRODUCT_ID);
        let dep_changed = graph.get_dependencies(id).iter().any(|d| will_change.contains(d));

        let Ok(input_checksum) = crate::checksum::combined_input_checksum(ctx, &product.inputs) else {
            build_count += 1;
            will_change.insert(id);
            products.push(ClassifiedProduct { id, action: ProductAction::Build, input_checksum: String::new() });
            continue;
        };

        let action = policy.classify(ctx, product, object_store, &input_checksum, dep_changed, force);
        match action {
            ProductAction::Skip => {
                skip_count += 1;
            }
            ProductAction::Restore => {
                restore_count += 1;
                will_change.insert(id);
            }
            ProductAction::Build => {
                build_count += 1;
                will_change.insert(id);
            }
        }
        products.push(ClassifiedProduct { id, action, input_checksum });
    }

    Classification { skip_count, restore_count, build_count, products }
}

/// Unlink the on-disk outputs of every product classified as Build or Restore.
///
/// Called once between classify and execute so that any "to-be-rebuilt" output
/// is guaranteed to be gone from disk by the time execution starts. If a
/// processor then fails (or its upstream fails and it is skipped), the stale
/// version cannot remain on disk — there is nothing to confuse the user into
/// thinking the build succeeded.
///
/// Uses the same per-product unlink logic as the pre-execute cleanup
/// ([`execution::remove_stale_outputs`]), so Creator-style products with
/// shared `output_dirs` only remove files they previously owned.
pub fn unlink_pending_outputs(
    graph: &BuildGraph,
    object_store: &ObjectStore,
    classification: &Classification,
) -> Result<()> {
    for c in &classification.products {
        if matches!(c.action, ProductAction::Skip) {
            continue;
        }
        let product = graph.get_product(c.id).expect(errors::INVALID_PRODUCT_ID);
        execution::remove_stale_outputs(product, object_store, &c.input_checksum)?;
    }
    Ok(())
}

/// Executor handles running products through their processors
/// It respects dependency order and can parallelize independent products
pub struct Executor<'a> {
    processors: &'a ProcessorMap,
    build_ctx: &'a crate::build_context::BuildContext,
    policy: &'a dyn BuildPolicy,
    parallel: usize,
    verbose: bool,
    display_opts: DisplayOptions,
    batch_size: Option<usize>,
    explain: bool,
    retry: usize,
}

impl<'a> Executor<'a> {
    pub fn new(
        processors: &'a ProcessorMap,
        build_ctx: &'a crate::build_context::BuildContext,
        policy: &'a dyn BuildPolicy,
        opts: ExecutorOptions,
    ) -> Self {
        Self {
            processors,
            build_ctx,
            policy,
            parallel: opts.parallel,
            // Verbose progress lines are human output: under --json (or
            // --quiet) they would corrupt the machine-readable stream, so
            // verbosity is forced off there rather than gated at each of
            // the half-dozen println! sites.
            verbose: opts.verbose && crate::json_output::human_output_enabled(),
            display_opts: opts.display_opts,
            batch_size: opts.batch_size,
            explain: opts.explain,
            retry: opts.retry,
        }
    }

    /// Check if the build was interrupted (Ctrl+C).
    fn is_interrupted(&self) -> bool {
        self.build_ctx.is_interrupted()
    }

    /// Display a product with the current display options.
    fn product_display(&self, product: &crate::graph::Product) -> String {
        product.display(self.display_opts)
    }

    /// Increment the global product counter only (no progress bar advancement).
    fn inc_global(shared: &SharedState) {
        shared.global_current.fetch_add(1, Ordering::SeqCst);
    }

    /// Increment both the progress bar and the global product counter.
    fn inc_progress(pb: &ProgressBar, shared: &SharedState) {
        Self::inc_global(shared);
        pb.inc(1);
    }

    /// Print an explain line for a product showing what action will be taken and why.
    fn print_explain(&self, product: &crate::graph::Product, action: &ExplainAction) {
        let styled = match action {
            ExplainAction::Skip => color::dim("SKIP"),
            ExplainAction::Restore(_) => color::cyan("RESTORE"),
            ExplainAction::Rebuild(_) => color::yellow("BUILD"),
        };
        crate::output::info(&format!("[{}] {} {} ({})", product.processor,
            styled,
            self.product_display(product),
            action));
    }

    /// Clean all products.
    /// Returns a map of processor name → number of files removed.
    pub fn clean(&self, graph: &BuildGraph, verbose: bool) -> Result<HashMap<String, usize>> {
        let mut stats: HashMap<String, usize> = HashMap::new();
        for product in graph.products() {
            // Nothing else in this serial loop observes the flag; without
            // this check a clean over a large graph cannot be Ctrl+C'd.
            if self.is_interrupted() {
                return Err(crate::exit_code::interrupted());
            }
            if let Some(processor) = self.processors.get(&product.processor) {
                let count = processor.clean(product, verbose)?;
                if count > 0 {
                    *stats.entry(product.processor.clone()).or_default() += count;
                }
            }
        }
        Ok(stats)
    }
}

/// Check if any dependency of a product has failed
pub fn has_failed_dependency(graph: &BuildGraph, id: usize, failed: &HashSet<usize>) -> bool {
    for &dep_id in graph.get_dependencies(id) {
        if failed.contains(&dep_id) {
            return true;
        }
    }
    false
}

/// Compute levels of products that can be executed in parallel
/// Products in the same level have no dependencies on each other
pub fn compute_parallel_levels(graph: &BuildGraph, order: &[usize]) -> Vec<Vec<usize>> {
    let mut levels: Vec<Vec<usize>> = Vec::new();
    let mut product_level: HashMap<usize, usize> = HashMap::new();

    for &id in order {
        // Find the maximum level of all dependencies
        let max_dep_level = graph.get_dependencies(id)
            .iter()
            .filter_map(|&dep_id| product_level.get(&dep_id))
            .max()
            .copied()
            .unwrap_or(0);

        // This product goes in the next level after its dependencies
        let my_level = if graph.get_dependencies(id).is_empty() {
            0
        } else {
            max_dep_level + 1
        };

        product_level.insert(id, my_level);

        // Ensure we have enough levels
        while levels.len() <= my_level {
            levels.push(Vec::new());
        }
        levels[my_level].push(id);
    }

    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A diamond top → {left, right} → bottom must schedule as three levels
    /// with left and right side by side; an independent node always lands in
    /// level 0.
    #[test]
    fn parallel_levels_diamond() {
        let mut graph = BuildGraph::new();
        let top = graph.add_product(vec!["a.src".into()], vec!["a.o".into()], "cc", None).unwrap();
        let left = graph.add_product(vec!["a.o".into()], vec!["b.o".into()], "cc", None).unwrap();
        let right = graph.add_product(vec!["a.o".into()], vec!["c.o".into()], "cc", None).unwrap();
        let bottom = graph.add_product(vec!["b.o".into(), "c.o".into()], vec!["d.o".into()], "cc", None).unwrap();
        let lone = graph.add_product(vec!["x.src".into()], vec!["x.o".into()], "cc", None).unwrap();
        graph.resolve_dependencies();
        let order = graph.topological_sort().unwrap();

        let levels = compute_parallel_levels(&graph, &order);

        // Level membership is order-independent; compare as sorted sets.
        let sorted = |mut ids: Vec<usize>| {
            ids.sort_unstable();
            ids
        };

        assert_eq!(levels.len(), 3, "diamond plus a free node is three levels: {levels:?}");
        assert_eq!(sorted(levels[0].clone()), sorted(vec![top, lone]));
        assert_eq!(sorted(levels[1].clone()), sorted(vec![left, right]));
        assert_eq!(levels[2], vec![bottom]);
    }

    /// Every product must appear in exactly one level — a dropped product
    /// would silently never build.
    #[test]
    fn parallel_levels_cover_all_products() {
        let mut g = BuildGraph::new();
        g.add_product(vec!["a.src".into()], vec!["a.o".into()], "cc", None).unwrap();
        g.add_product(vec!["a.o".into()], vec!["b.o".into()], "cc", None).unwrap();
        g.add_product(vec!["free.src".into()], vec!["free.o".into()], "cc", None).unwrap();
        g.resolve_dependencies();
        let order = g.topological_sort().unwrap();

        let levels = compute_parallel_levels(&g, &order);
        let mut all: Vec<usize> = levels.into_iter().flatten().collect();
        all.sort_unstable();
        assert_eq!(all, vec![0, 1, 2]);
    }

    /// Only direct dependencies count as failed here — transitive failure
    /// propagation happens level by level as each product is marked failed.
    #[test]
    fn failed_dependency_is_direct_only() {
        let mut g = BuildGraph::new();
        let a = g.add_product(vec!["a.src".into()], vec!["a.o".into()], "cc", None).unwrap();
        let b = g.add_product(vec!["a.o".into()], vec!["b.o".into()], "cc", None).unwrap();
        let c = g.add_product(vec!["b.o".into()], vec!["c.o".into()], "cc", None).unwrap();
        g.resolve_dependencies();

        let failed: HashSet<usize> = [a].into();
        assert!(has_failed_dependency(&g, b, &failed), "b directly depends on failed a");
        assert!(!has_failed_dependency(&g, c, &failed),
            "c depends on a only through b; direct check must not see it");
        assert!(!has_failed_dependency(&g, a, &failed), "a has no dependencies");
    }
}
