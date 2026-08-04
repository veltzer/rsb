use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cache_key::{CacheKey, Component as KeyComponent};
use crate::display::{DisplayOptions, InputDisplay, OutputDisplay, PathFormat};
use crate::errors;

/// A single build product with concrete inputs and outputs.
/// All paths are relative to project root.
#[derive(Debug, Clone)]
pub struct Product {
    /// Input files (relative paths)
    pub inputs: Vec<PathBuf>,
    /// Output files (relative paths)
    pub outputs: Vec<PathBuf>,
    /// Which processor handles this product
    pub processor: String,
    /// Unique identifier for this product
    pub id: usize,
    /// Every piece of non-input state that contributes to this product's
    /// cache key: processor config, analyzer pieces, tool versions, variant.
    /// See `src/cache_key.rs` — all contributors go through `CacheKey::push`.
    pub cache_key: CacheKey,
    /// Optional variant/profile name (e.g., compiler profile name).
    /// Also mixed into `cache_key`; kept here for display.
    pub variant: Option<String>,
    /// Output directories for creators / creators (relative to project root).
    /// When non-empty, the executor caches/restores these directories instead of individual output files.
    pub output_dirs: Vec<Arc<PathBuf>>,
}

impl Product {
    pub fn new(inputs: Vec<PathBuf>, outputs: Vec<PathBuf>, processor: &str, id: usize, config_hash: Option<String>) -> Self {
        Self {
            inputs,
            outputs,
            processor: processor.to_string(),
            id,
            cache_key: CacheKey::from_config_hash(config_hash),
            variant: None,
            output_dirs: Vec::new(),
        }
    }

    /// Create a new product with a variant/profile name
    pub fn with_variant(inputs: Vec<PathBuf>, outputs: Vec<PathBuf>, processor: &str, id: usize, config_hash: Option<String>, variant: &str) -> Self {
        let mut cache_key = CacheKey::from_config_hash(config_hash);
        cache_key.push(KeyComponent::Variant, variant);
        Self {
            inputs,
            outputs,
            processor: processor.to_string(),
            id,
            cache_key,
            variant: Some(variant.to_string()),
            output_dirs: Vec::new(),
        }
    }

    /// Return the primary (first) input file for this product.
    /// Panics if the product has no inputs (a programming error — every product must have at least one).
    pub fn primary_input(&self) -> &Path {
        self.inputs.first().expect(errors::EMPTY_PRODUCT_INPUTS)
    }

    /// Return the primary (first) output file for this product.
    /// Panics if the product has no outputs (a programming error — every generator product must have at least one).
    pub fn primary_output(&self) -> &Path {
        self.outputs.first().expect(errors::EMPTY_PRODUCT_OUTPUTS)
    }

    /// Whether this product has output directories to cache.
    pub const fn has_output_dirs(&self) -> bool {
        !self.output_dirs.is_empty()
    }

    /// Mix an analyzer-supplied piece into the product's cache key.
    /// Used by analyzers that need to contribute non-content state (e.g. the
    /// sorted set of paths matching a glob pattern) into the key.
    pub fn extend_config_hash(&mut self, piece: &str) {
        self.cache_key.push(KeyComponent::Analyzer, piece);
    }

    /// Compute the content-addressed descriptor key for this product.
    /// Composition lives in `CacheKey` — see `src/cache_key.rs`.
    pub fn descriptor_key(&self, input_checksum: &str) -> String {
        self.cache_key.descriptor_key(&self.processor, input_checksum)
    }

    /// Format a path according to the given format
    fn format_path(path: &Path, format: PathFormat) -> String {
        match format {
            PathFormat::Basename => {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string()
            }
            PathFormat::Path => path.display().to_string(),
        }
    }

    /// Display name for logging with the given display options.
    /// All paths are already relative to project root.
    pub fn display(&self, opts: DisplayOptions) -> String {
        // For checkers (empty outputs), display the input file instead
        if self.outputs.is_empty() {
            return self.inputs.first().map_or_else(|| "?".to_string(), |p| Self::format_path(p, opts.path_format));
        }

        // Format output part
        let output_part = match opts.output {
            OutputDisplay::None => String::new(),
            OutputDisplay::Basename => {
                let names: Vec<_> = self.outputs.iter()
                    .map(|p| Self::format_path(p, PathFormat::Basename))
                    .collect();
                names.join(", ")
            }
            OutputDisplay::Path => {
                let paths: Vec<_> = self.outputs.iter()
                    .map(|p| Self::format_path(p, PathFormat::Path))
                    .collect();
                paths.join(", ")
            }
        };

        // Format input part
        let input_part = match opts.input {
            InputDisplay::None => None,
            InputDisplay::Source => {
                self.inputs.first()
                    .map(|p| Self::format_path(p, opts.path_format))
            }
            InputDisplay::All => {
                let inputs: Vec<_> = self.inputs.iter()
                    .map(|p| Self::format_path(p, opts.path_format))
                    .collect();
                if inputs.is_empty() {
                    None
                } else {
                    Some(inputs.join(", "))
                }
            }
        };

        // Combine output and input parts
        match (output_part.is_empty(), input_part) {
            (true, None) => "?".to_string(),
            (true, Some(inp)) => inp,
            (false, None) => output_part,
            (false, Some(inp)) => format!("{output_part} <- {inp}"),
        }
    }

}

/// Per-process integer ID assigned to a path by `PathInterner`.
/// IDs are only meaningful within a single `BuildGraph` — never persisted
/// to disk or logs. See docs/src/internal/path-interning.md.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
struct PathId(u32);

/// Interns `PathBuf`s into small integer IDs for use as `HashMap` keys in
/// `BuildGraph`'s hot lookup tables. Hashing and comparing `u32` is one
/// instruction each, versus walking every component of a path.
#[derive(Default)]
struct PathInterner {
    to_id: HashMap<PathBuf, PathId>,
}

impl PathInterner {
    /// Return the id for `path`, inserting if new.
    fn intern(&mut self, path: &Path) -> PathId {
        if let Some(&id) = self.to_id.get(path) {
            return id;
        }
        let id = PathId(self.to_id.len() as u32);
        self.to_id.insert(path.to_path_buf(), id);
        id
    }

    /// Return the id for `path` if it has been interned, without inserting.
    /// Used by read-only lookups so we don't create spurious entries.
    fn get(&self, path: &Path) -> Option<PathId> {
        self.to_id.get(path).copied()
    }

    fn clear(&mut self) {
        self.to_id.clear();
    }
}

/// Build graph with dependency resolution
#[derive(Default)]
pub struct BuildGraph {
    /// `pub(crate)` for `graph_render`, which reads the graph to draw it.
    /// The renderers used to live in this file and touched these directly;
    /// widening to crate visibility is what let them move out without
    /// inventing an accessor per field for a read-only consumer.
    pub(crate) products: Vec<Product>,
    /// In-memory path interner backing the PathId-keyed maps below.
    /// Never persisted. See docs/src/internal/path-interning.md.
    interner: PathInterner,
    /// Map from output path (interned) to the single product id that produces it.
    /// One path has at most one owner by construction (output-conflict check).
    output_to_product: HashMap<PathId, usize>,
    /// Map from input path (interned) to every product id that consumes it.
    /// One path may feed many products (e.g. a shared header).
    input_to_products: HashMap<PathId, Vec<usize>>,
    /// Dedup index for checker products (outputs empty): maps
    /// (processor, `primary_input_id`, variant) → product id. Replaces an O(N)
    /// linear scan that dominated `status` wall time on large projects.
    checker_dedup: HashMap<(String, PathId, Option<String>), usize>,
    /// Adjacency list: product id -> list of product ids that depend on it
    dependents: Vec<Vec<usize>>,
    /// Reverse: product id -> list of product ids it depends on
    pub(crate) dependencies: Vec<Vec<usize>>,
}

impl BuildGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// If `new_inputs` is a superset of the existing product's inputs, replace
    /// them and update the `input_to_products` index. This happens during
    /// fixed-point discovery when a later pass resolves more virtual files
    /// (e.g. globs that matched nothing on pass 0 now match upstream outputs).
    /// If the inputs are identical or not a superset, this is a no-op.
    /// Returns true if the inputs were accepted (identical or superset),
    /// false if the new inputs are not a superset of the existing ones.
    fn try_update_inputs(&mut self, product_id: usize, new_inputs: Vec<PathBuf>) -> bool {
        let existing = &self.products[product_id];
        if existing.inputs == new_inputs {
            return true;
        }
        let new_set: HashSet<&PathBuf> = new_inputs.iter().collect();
        if !existing.inputs.iter().all(|i| new_set.contains(i)) {
            return false;
        }
        let old_set: HashSet<&PathBuf> = existing.inputs.iter().collect();
        // Collect index updates before mutating products, to satisfy the borrow checker.
        let to_remove: Vec<PathBuf> = existing.inputs.iter()
            .filter(|p| !new_set.contains(p))
            .cloned()
            .collect();
        let to_add: Vec<PathBuf> = new_inputs.iter()
            .filter(|p| !old_set.contains(p))
            .cloned()
            .collect();
        self.products[product_id].inputs = new_inputs;
        for path in &to_remove {
            if let Some(path_id) = self.interner.get(path)
                && let Some(ids) = self.input_to_products.get_mut(&path_id) {
                    ids.retain(|&x| x != product_id);
                }
        }
        for path in &to_add {
            let path_id = self.interner.intern(path);
            self.input_to_products.entry(path_id).or_default().push(product_id);
        }
        true
    }

    /// Add a product to the graph.
    /// Returns an error if any output path is already claimed by another product.
    pub fn add_product(&mut self, inputs: Vec<PathBuf>, outputs: Vec<PathBuf>, processor: &str, config_hash: Option<String>) -> Result<usize> {
        self.add_product_with_variant(inputs, outputs, processor, config_hash, None)
    }

    /// Add a product to the graph with an optional variant/profile name.
    /// Returns an error if any output path is already claimed by another product.
    pub fn add_product_with_variant(&mut self, inputs: Vec<PathBuf>, outputs: Vec<PathBuf>, processor: &str, config_hash: Option<String>, variant: Option<&str>) -> Result<usize> {
        let id = self.products.len();

        // During fixed-point discovery, processors re-run and may re-declare
        // products that already exist. Detect and deduplicate these cases.

        // Checkers and explicit processors have no outputs, so the output-based
        // dedup below won't catch them. Deduplicate by matching on processor name,
        // primary input, and variant. If the new inputs are a superset (e.g. globs
        // resolved more files in a later fixed-point pass), update the product's
        // inputs so dependency resolution sees the full set.
        if outputs.is_empty() && !inputs.is_empty()
            && let Some(primary_id) = self.interner.get(&inputs[0]) {
                let key = (processor.to_string(), primary_id, variant.map(str::to_string));
                if let Some(&existing_id) = self.checker_dedup.get(&key) {
                    // A non-superset re-declaration is a real disagreement about
                    // what this product consumes, and it used to be swallowed:
                    // `try_update_inputs`'s `false` was discarded, so the second
                    // declaration's inputs were silently dropped and the checker
                    // ran against the first set. The generator path a few lines
                    // below hard-errors on the equivalent conflict; matching that
                    // here removes the weaker of two identity schemes.
                    let attempted = inputs.clone();
                    if !self.try_update_inputs(existing_id, inputs) {
                        let existing = self.products.get(existing_id)
                            .expect(crate::errors::INVALID_PRODUCT_ID);
                        return Err(crate::exit_code::RsconstructError::new(
                            crate::exit_code::RsconstructExitCode::GraphError,
                            format!(
                                "Input conflict: [{}] declared product for {} twice with \
                                 incompatible inputs ({:?} then {:?}). A re-declaration may \
                                 only add inputs, not change them.",
                                processor,
                                existing.primary_input().display(),
                                existing.inputs,
                                attempted,
                            ),
                        ).into());
                    }
                    return Ok(existing_id);
                }
            }

        // For generators: check output conflicts and deduplicate re-declarations.
        for output in &outputs {
            let Some(output_id) = self.interner.get(output) else { continue };
            if let Some(&existing_id) = self.output_to_product.get(&output_id) {
                let existing = self.products.get(existing_id).expect(crate::errors::INVALID_PRODUCT_ID);
                let same_processor = existing.processor == processor;
                let same_outputs = existing.outputs == outputs;
                let existing_proc_name = existing.processor.clone();
                // Same processor re-declaring the same outputs: update inputs if
                // they grew (virtual files from upstream generators were added).
                if same_processor && same_outputs
                    && self.try_update_inputs(existing_id, inputs)
                {
                    return Ok(existing_id);
                }
                return Err(crate::exit_code::RsconstructError::new(
                    crate::exit_code::RsconstructExitCode::GraphError,
                    format!(
                        "Output conflict: {} is produced by both [{}] and [{}]",
                        output.display(),
                        existing_proc_name,
                        processor,
                    ),
                ).into());
            }
        }

        let product = match variant {
            Some(v) => Product::with_variant(inputs, outputs, processor, id, config_hash, v),
            None => Product::new(inputs, outputs, processor, id, config_hash),
        };
        self.register_product(product);

        Ok(id)
    }

    /// Register a product into every lookup index and push it onto the
    /// parallel `products` / `dependents` / `dependencies` vectors.
    ///
    /// The one place that knows what "adding a product to the graph" means.
    /// `add_product_with_variant` and `filter_by_targets` used to each carry
    /// their own copy of this — two sites that had to evolve in lockstep,
    /// with `Product.id == index` upheld by hand in both. The product's `id`
    /// is assigned here from the vector length, so the invariant holds by
    /// construction rather than by convention.
    fn register_product(&mut self, mut product: Product) {
        let id = self.products.len();
        product.id = id;

        for output in &product.outputs {
            let output_id = self.interner.intern(output);
            self.output_to_product.insert(output_id, id);
        }

        for input in &product.inputs {
            let input_id = self.interner.intern(input);
            self.input_to_products.entry(input_id).or_default().push(id);
        }

        // For checker products, populate the dedup index so a future re-declaration
        // with the same (processor, primary_input, variant) returns this id.
        if product.outputs.is_empty() && !product.inputs.is_empty() {
            let primary_id = self.interner.intern(&product.inputs[0]);
            let key = (product.processor.clone(), primary_id, product.variant.clone());
            self.checker_dedup.insert(key, id);
        }

        self.products.push(product);
        self.dependents.push(Vec::new());
        self.dependencies.push(Vec::new());
    }

    /// Add a product with an output directory for creator caching.
    /// The `output_dir` is the directory whose contents will be cached/restored as a whole.
    pub fn add_product_with_output_dir(&mut self, inputs: Vec<PathBuf>, outputs: Vec<PathBuf>, processor: &str, config_hash: Option<String>, output_dir: PathBuf) -> Result<usize> {
        self.add_product_with_output_dirs_and_variant(inputs, outputs, processor, config_hash, vec![output_dir], None)
    }

    /// Add a product with an output directory and an optional variant/profile name.
    pub fn add_product_with_output_dir_and_variant(&mut self, inputs: Vec<PathBuf>, outputs: Vec<PathBuf>, processor: &str, config_hash: Option<String>, output_dir: PathBuf, variant: Option<&str>) -> Result<usize> {
        self.add_product_with_output_dirs_and_variant(inputs, outputs, processor, config_hash, vec![output_dir], variant)
    }

    /// Add a product with multiple output directories and an optional variant/profile name.
    pub fn add_product_with_output_dirs_and_variant(&mut self, inputs: Vec<PathBuf>, outputs: Vec<PathBuf>, processor: &str, config_hash: Option<String>, output_dirs: Vec<PathBuf>, variant: Option<&str>) -> Result<usize> {
        let id = self.add_product_with_variant(inputs, outputs, processor, config_hash, variant)?;
        self.products[id].output_dirs = output_dirs.into_iter().map(Arc::new).collect();
        Ok(id)
    }

    /// Incorporate tool version hashes into product cache keys.
    /// For each product whose processor has an entry in the map, the tool
    /// version hash becomes a `ToolVersion` component of its cache key, so
    /// that upgrading a tool invalidates everything that tool produced.
    pub fn apply_tool_version_hashes(&mut self, processor_tool_hashes: &HashMap<String, String>) {
        for product in &mut self.products {
            if let Some(tool_hash) = processor_tool_hashes.get(&product.processor) {
                product.cache_key.push(KeyComponent::ToolVersion, tool_hash.clone());
            }
        }
    }

    /// Resolve dependencies between products
    pub fn resolve_dependencies(&mut self) {
        // Collect edges first to avoid borrow conflict with self.products
        let edges: Vec<(usize, usize)> = self.products.iter()
            .flat_map(|product| {
                product.inputs.iter().filter_map(|input| {
                    let input_id = self.interner.get(input)?;
                    self.output_to_product.get(&input_id)
                        .copied()
                        .filter(|&producer_id| producer_id != product.id)
                        .map(|producer_id| (producer_id, product.id))
                })
            })
            .collect();

        for (producer_id, consumer_id) in edges {
            self.dependents.get_mut(producer_id).expect(crate::errors::INVALID_PRODUCT_ID).push(consumer_id);
            self.dependencies.get_mut(consumer_id).expect(crate::errors::INVALID_PRODUCT_ID).push(producer_id);
        }
    }

    /// Topological sort - returns product ids in execution order
    /// Returns error if there's a cycle
    pub fn topological_sort(&self) -> Result<Vec<usize>> {
        let mut in_degree: Vec<usize> = self.dependencies.iter()
            .map(std::vec::Vec::len)
            .collect();

        // Start with products that have no dependencies (BTreeSet keeps sorted order)
        let mut queue: BTreeSet<usize> = in_degree.iter()
            .enumerate()
            .filter(|&(_, deg)| *deg == 0)
            .map(|(id, _)| id)
            .collect();

        let mut result = Vec::with_capacity(self.products.len());

        while let Some(id) = queue.pop_first() {
            result.push(id);

            // Reduce in-degree of dependents
            for &dep_id in self.dependents.get(id).expect(crate::errors::INVALID_PRODUCT_ID) {
                in_degree[dep_id] = in_degree[dep_id].saturating_sub(1);
                if in_degree[dep_id] == 0 {
                    queue.insert(dep_id);
                }
            }
        }

        if result.len() != self.products.len() {
            return Err(crate::exit_code::RsconstructError::new(
                crate::exit_code::RsconstructExitCode::GraphError,
                "Cycle detected in build graph",
            ).into());
        }

        Ok(result)
    }

    /// Get a product by id
    pub fn get_product(&self, id: usize) -> Option<&Product> {
        self.products.get(id)
    }

    /// Get all products
    pub fn products(&self) -> &[Product] {
        &self.products
    }

    /// Remove products that don't match the predicate.
    ///
    /// Rebuilds every index and re-links dependencies, so the graph is fully
    /// consistent afterwards — `id == index`, the lookup maps, and the
    /// adjacency lists all hold. This used to be a bare `Vec::retain` guarded
    /// only by a doc-comment saying the result was "only suitable for
    /// read-only iteration": every product after the first removed one had an
    /// `id` that no longer matched its index, so any subsequent `get_product`,
    /// `path_owner`, or dependency lookup silently returned the wrong product.
    /// Both callers happened to only iterate, so nothing was broken — but a
    /// comment was the only thing standing between the next caller and silent
    /// corruption, which is exactly the "identity by convention" this finding
    /// is about.
    pub fn retain_products(&mut self, f: impl Fn(&Product) -> bool) {
        let keep: HashSet<usize> = self.products.iter()
            .filter(|p| f(p))
            .map(|p| p.id)
            .collect();
        self.rebuild_retaining(&keep);
    }

    /// Rebuild the graph from scratch keeping only the products whose ids are
    /// in `keep`, reassigning ids and re-linking dependencies.
    ///
    /// The single place that knows how to drop products without corrupting
    /// the graph — shared by `retain_products` and `filter_by_targets`, which
    /// previously each open-coded it (and only one of them did it correctly).
    fn rebuild_retaining(&mut self, keep: &HashSet<usize>) {
        let old_products = std::mem::take(&mut self.products);
        self.interner.clear();
        self.output_to_product.clear();
        self.input_to_products.clear();
        self.checker_dedup.clear();
        self.dependents.clear();
        self.dependencies.clear();

        for product in old_products {
            if keep.contains(&product.id) {
                // Same registration path as add_product — including the id
                // reassignment, which register_product derives from the
                // vector length rather than trusting the caller.
                self.register_product(product);
            }
        }

        // The rebuild assigned new ids and cleared all edges; re-link the
        // surviving products so execution order and failure propagation hold.
        self.resolve_dependencies();
    }

    /// Return the id of the product that declares `path` as one of its outputs,
    /// or None if no product owns it. O(1) average — backed by a hashmap index.
    ///
    /// Used by Creators caching a shared output directory: any path owned by a
    /// different product must be excluded from this Creator's tree so restore
    /// never clobbers another processor's file.
    pub fn path_owner(&self, path: &Path) -> Option<usize> {
        let id = self.interner.get(path)?;
        self.output_to_product.get(&id).copied()
    }

    /// Return every product id that lists `path` as an input. O(1) average — backed
    /// by a hashmap index. Returns an empty slice if the path is not an input to
    /// any product.
    pub fn products_consuming(&self, path: &Path) -> &[usize] {
        match self.interner.get(path) {
            Some(id) => self.input_to_products.get(&id).map_or(&[], Vec::as_slice),
            None => &[],
        }
    }

    /// Get dependencies of a product (products that must be built before this one)
    pub fn get_dependencies(&self, id: usize) -> &[usize] {
        self.dependencies.get(id).expect(crate::errors::INVALID_PRODUCT_ID)
    }

    /// Get processor-level dependencies: returns a map from processor name
    /// to the set of processor names it depends on.
    pub fn processor_dependencies(&self) -> BTreeMap<String, BTreeSet<String>> {
        let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for product in &self.products {
            deps.entry(product.processor.clone()).or_default();
            for &dep_id in self.dependencies.get(product.id).expect(crate::errors::INVALID_PRODUCT_ID) {
                let dep_proc = &self.products[dep_id].processor;
                if dep_proc != &product.processor {
                    deps.entry(product.processor.clone()).or_default().insert(dep_proc.clone());
                }
            }
        }
        deps
    }

    /// Get mutable access to a product by id
    pub fn get_product_mut(&mut self, id: usize) -> Option<&mut Product> {
        self.products.get_mut(id)
    }

    /// Filter the graph to only include products whose input files match any of the target patterns.
    /// Uses glob matching. Products not matching any pattern are removed.
    pub fn filter_by_targets(&mut self, patterns: &[String]) -> anyhow::Result<()> {
        let compiled: Vec<glob::Pattern> = patterns.iter()
            .map(|p| glob::Pattern::new(p)
                .with_context(|| format!("Invalid glob pattern: {p}")))
            .collect::<anyhow::Result<_>>()?;
        if compiled.is_empty() {
            return Ok(());
        }

        // Collect IDs to keep
        let mut keep: HashSet<usize> = self.products.iter()
            .filter(|product| {
                product.inputs.iter().any(|input| {
                    let input_str = input.display().to_string();
                    compiled.iter().any(|pat| pat.matches(&input_str))
                })
            })
            .map(|p| p.id)
            .collect();

        // Close over upstream producers: a kept consumer's inputs may be
        // produced by products that match no pattern themselves. Dropping
        // the producer would leave the consumer building against a missing
        // or stale input. Dependencies are already resolved at this point
        // (the builder filters after graph construction).
        let mut worklist: Vec<usize> = keep.iter().copied().collect();
        while let Some(id) = worklist.pop() {
            for &dep_id in self.get_dependencies(id) {
                if keep.insert(dep_id) {
                    worklist.push(dep_id);
                }
            }
        }

        // Ids are indices, so products can't simply be removed — the graph is
        // rebuilt from the survivors.
        self.rebuild_retaining(&keep);
        Ok(())
    }

    /// Run configurable validation checks on the fully-built graph.
    /// Returns a list of error messages. The caller decides whether to
    /// bail or warn based on the config.
    pub fn validate(&self, config: &crate::config::GraphConfig) -> Vec<String> {
        let mut errors = Vec::new();

        // Check 1: reject products with no input files
        if config.validate_empty_inputs {
            for product in &self.products {
                if product.inputs.is_empty() {
                    errors.push(format!(
                        "[{}] product {} has no input files",
                        product.processor,
                        product.display(crate::cli::DisplayOptions::minimal()),
                    ));
                }
            }
        }

        // Check 2: internal graph consistency. The old form of this check
        // (dep ids within bounds) was unreachable by construction — every
        // edge comes from a live index in `output_to_product`. The invariants
        // that CAN break under a bad retain/rebuild are id/index agreement
        // and the parallel-vector lengths, which is exactly the corruption
        // class `retain_products` had historically.
        if config.validate_dep_references {
            for (index, product) in self.products.iter().enumerate() {
                if product.id != index {
                    errors.push(format!(
                        "[{}] product at index {index} carries id {} — ids must equal indices after retain/rebuild",
                        product.processor, product.id,
                    ));
                }
            }
            if self.dependencies.len() != self.products.len() {
                errors.push(format!(
                    "dependency table has {} rows for {} products",
                    self.dependencies.len(), self.products.len(),
                ));
            }
            for (id, deps) in self.dependencies.iter().enumerate() {
                for &dep_id in deps {
                    if dep_id >= self.products.len() {
                        let product = &self.products[id];
                        errors.push(format!(
                            "[{}] product {} has dependency on non-existent product id {}",
                            product.processor,
                            product.display(crate::cli::DisplayOptions::minimal()),
                            dep_id,
                        ));
                    }
                }
            }
        }

        // Check 3: detect duplicate inputs within same processor
        if config.validate_duplicate_inputs {
            let mut seen: HashMap<(&str, &Path), usize> = HashMap::new();
            for product in &self.products {
                for input in &product.inputs {
                    let key = (product.processor.as_str(), input.as_path());
                    if let Some(first_id) = seen.get(&key) {
                        errors.push(format!(
                            "[{}] input {} appears in both product {} and product {}",
                            product.processor,
                            input.display(),
                            first_id,
                            product.id,
                        ));
                    } else {
                        seen.insert(key, product.id);
                    }
                }
            }
        }

        // Check 4: early cycle detection
        if config.validate_early_cycles
            && let Err(e) = self.topological_sort() {
                errors.push(format!("{e}"));
            }

        errors
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_product_assigns_incrementing_ids() {
        let mut g = BuildGraph::new();
        let id0 = g.add_product(vec!["a.c".into()], vec!["a.o".into()], "cc", None).unwrap();
        let id1 = g.add_product(vec!["b.c".into()], vec!["b.o".into()], "cc", None).unwrap();
        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(g.products().len(), 2);
    }

    #[test]
    fn output_conflict_is_detected() {
        let mut g = BuildGraph::new();
        g.add_product(vec!["a.c".into()], vec!["out.o".into()], "cc", None).unwrap();
        let result = g.add_product(vec!["b.c".into()], vec!["out.o".into()], "cc", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Output conflict"));
    }

    #[test]
    fn topological_sort_no_dependencies() {
        let mut g = BuildGraph::new();
        g.add_product(vec!["c.c".into()], vec![], "check", None).unwrap();
        g.add_product(vec!["b.c".into()], vec![], "check", None).unwrap();
        g.add_product(vec!["a.c".into()], vec![], "check", None).unwrap();
        g.resolve_dependencies();
        let order = g.topological_sort().unwrap();
        // All products have no dependencies, order should contain all ids
        assert_eq!(order.len(), 3);
        let mut sorted = order;
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2]);
    }

    #[test]
    fn topological_sort_respects_dependencies() {
        let mut g = BuildGraph::new();
        // Product 0: generates lib.o
        g.add_product(vec!["lib.c".into()], vec!["lib.o".into()], "cc", None).unwrap();
        // Product 1: consumes lib.o (depends on product 0)
        g.add_product(vec!["main.c".into(), "lib.o".into()], vec!["main".into()], "cc", None).unwrap();
        g.resolve_dependencies();
        let order = g.topological_sort().unwrap();
        assert_eq!(order.len(), 2);
        // lib.o producer (0) must come before consumer (1)
        let pos0 = order.iter().position(|&id| id == 0).unwrap();
        let pos1 = order.iter().position(|&id| id == 1).unwrap();
        assert!(pos0 < pos1);
    }

    #[test]
    fn topological_sort_chain() {
        let mut g = BuildGraph::new();
        // A -> B -> C chain
        g.add_product(vec!["a.c".into()], vec!["a.o".into()], "cc", None).unwrap();
        g.add_product(vec!["a.o".into()], vec!["b.o".into()], "link", None).unwrap();
        g.add_product(vec!["b.o".into()], vec!["c.out".into()], "link", None).unwrap();
        g.resolve_dependencies();
        let order = g.topological_sort().unwrap();
        assert_eq!(order, vec![0, 1, 2]);
    }

    #[test]
    fn cycle_detection() {
        let mut g = BuildGraph::new();
        // Create a cycle: 0 produces a.o, 1 produces b.o, but each consumes the other
        g.add_product(vec!["b.o".into()], vec!["a.o".into()], "cc", None).unwrap();
        g.add_product(vec!["a.o".into()], vec!["b.o".into()], "cc", None).unwrap();
        g.resolve_dependencies();
        let result = g.topological_sort();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cycle"));
    }

    #[test]
    fn resolve_dependencies_links_products() {
        let mut g = BuildGraph::new();
        g.add_product(vec!["src.c".into()], vec!["obj.o".into()], "cc", None).unwrap();
        g.add_product(vec!["obj.o".into()], vec!["app".into()], "link", None).unwrap();
        g.resolve_dependencies();
        // Product 1 depends on product 0
        assert_eq!(g.get_dependencies(1), &[0]);
        // Product 0 has no dependencies
        assert!(g.get_dependencies(0).is_empty());
    }

    #[test]
    fn descriptor_key_differs_per_format_variant() {
        // Regression test: products with the same input but different output
        // formats (e.g., pandoc producing pdf, html, docx from the same .md
        // file) must have different descriptor keys, or they overwrite each
        // other's cache entries. The descriptor key is path-free, so the
        // outputs themselves can't distinguish them — multi-format discovery
        // passes the format as the variant component (see
        // discover_multi_format), and that variant is what separates them.
        let p_pdf = Product::with_variant(
            vec!["doc.md".into()], vec!["out/doc.pdf".into()], "pandoc", 0, Some("h".into()), "pdf");
        let p_html = Product::with_variant(
            vec!["doc.md".into()], vec!["out/doc.html".into()], "pandoc", 0, Some("h".into()), "html");
        let p_docx = Product::with_variant(
            vec!["doc.md".into()], vec!["out/doc.docx".into()], "pandoc", 0, Some("h".into()), "docx");

        assert_ne!(p_pdf.descriptor_key("chk"), p_html.descriptor_key("chk"),
            "PDF and HTML products must have different descriptor keys");
        assert_ne!(p_html.descriptor_key("chk"), p_docx.descriptor_key("chk"),
            "HTML and DOCX products must have different descriptor keys");
        assert_ne!(p_pdf.descriptor_key("chk"), p_docx.descriptor_key("chk"),
            "PDF and DOCX products must have different descriptor keys");
    }

    #[test]
    fn descriptor_key_includes_config_hash() {
        let p1 = Product::new(vec!["a.c".into()], vec![], "cc", 0, None);
        let p2 = Product::new(vec!["a.c".into()], vec![], "cc", 0, Some("abc123".into()));
        assert_ne!(p1.descriptor_key("chk"), p2.descriptor_key("chk"));
    }

    #[test]
    fn apply_tool_version_hashes() {
        let mut g = BuildGraph::new();
        g.add_product(vec!["a.c".into()], vec![], "cc", Some("cfg1".into())).unwrap();
        g.add_product(vec!["b.py".into()], vec![], "ruff", None).unwrap();
        let before_cc = g.get_product(0).unwrap().descriptor_key("chk");
        let before_ruff = g.get_product(1).unwrap().descriptor_key("chk");

        let mut hashes = HashMap::new();
        hashes.insert("cc".into(), "toolv1".into());
        g.apply_tool_version_hashes(&hashes);

        // The cc product keeps its config component and gains a tool component.
        let cc_key = &g.get_product(0).unwrap().cache_key;
        let components: Vec<_> = cc_key.components().iter()
            .map(|(c, v)| (c.tag(), v.as_str()))
            .collect();
        assert_eq!(components, vec![("config", "cfg1"), ("tool", "toolv1")]);
        assert_ne!(before_cc, g.get_product(0).unwrap().descriptor_key("chk"),
            "a tool version change must invalidate the descriptor key");

        // The ruff product has no tool hash mapping and is untouched.
        assert!(g.get_product(1).unwrap().cache_key.is_empty());
        assert_eq!(before_ruff, g.get_product(1).unwrap().descriptor_key("chk"));
    }

    #[test]
    fn variant_is_a_cache_key_component() {
        // Variants used to be spliced into descriptor_key separately from
        // config_hash; they are now a normal component, so a variant change
        // is visible in `product show` like every other contributor.
        let plain = Product::new(vec!["a.c".into()], vec![], "cc", 0, Some("cfg".into()));
        let debug = Product::with_variant(vec!["a.c".into()], vec![], "cc", 0, Some("cfg".into()), "debug");
        let release = Product::with_variant(vec!["a.c".into()], vec![], "cc", 0, Some("cfg".into()), "release");
        assert_ne!(plain.descriptor_key("chk"), debug.descriptor_key("chk"));
        assert_ne!(debug.descriptor_key("chk"), release.descriptor_key("chk"));
        assert_eq!(
            debug.cache_key.components().iter().map(|(c, _)| c.tag()).collect::<Vec<_>>(),
            vec!["config", "variant"],
        );
    }

    #[test]
    fn analyzer_pieces_accumulate_distinctly() {
        // extend_config_hash used to rehash into an opaque string; each piece
        // is now its own component, so contributions stay attributable.
        let mut p = Product::new(vec!["a.tera".into()], vec![], "tera", 0, None);
        let empty = p.descriptor_key("chk");
        p.extend_config_hash("glob:one");
        let one = p.descriptor_key("chk");
        p.extend_config_hash("glob:two");
        let two = p.descriptor_key("chk");
        assert_ne!(empty, one);
        assert_ne!(one, two);
        assert_eq!(p.cache_key.components().len(), 2);
    }

    #[test]
    fn empty_graph_sorts_ok() {
        let g = BuildGraph::new();
        let order = g.topological_sort().unwrap();
        assert!(order.is_empty());
    }

    /// Simulate the fixed-point discovery bug: a product with no outputs
    /// (like explicit processors with `output_dirs`) is first discovered with
    /// only literal inputs (globs match nothing on pass 0). On pass 1,
    /// virtual files from upstream generators are available and the product
    /// is re-declared with expanded inputs. The dedup must update the inputs
    /// so dependency resolution creates edges to the upstream producers.
    #[test]
    fn checker_dedup_updates_inputs_on_superset() {
        let mut g = BuildGraph::new();

        // Pass 0: upstream generator declares output _site/page.html
        let gen_id = g.add_product(
            vec!["src/page.md".into()],
            vec!["_site/page.html".into()],
            "pandoc",
            None,
        ).unwrap();

        // Pass 0: explicit processor discovered with only literal inputs
        // (input_globs matched nothing because _site/ files don't exist yet)
        let explicit_id = g.add_product(
            vec!["resources/index.html".into()],
            vec![],
            "explicit.build_site",
            None,
        ).unwrap();
        assert_ne!(gen_id, explicit_id);

        // Pass 1: explicit processor re-discovered with expanded inputs
        // (virtual files from pandoc now visible to input_globs)
        let redeclared_id = g.add_product(
            vec!["resources/index.html".into(), "_site/page.html".into()],
            vec![],
            "explicit.build_site",
            None,
        ).unwrap();

        // Dedup should return the same product id
        assert_eq!(redeclared_id, explicit_id);
        // Only 2 products in the graph (not 3)
        assert_eq!(g.products().len(), 2);

        // Inputs must be updated to the expanded set
        let product = g.get_product(explicit_id).unwrap();
        assert_eq!(product.inputs.len(), 2);
        assert_eq!(product.inputs[0], PathBuf::from("resources/index.html"));
        assert_eq!(product.inputs[1], PathBuf::from("_site/page.html"));

        // Dependency resolution must now link explicit -> pandoc
        g.resolve_dependencies();
        assert_eq!(g.get_dependencies(explicit_id), &[gen_id]);

        // Topological sort must place pandoc before explicit
        let order = g.topological_sort().unwrap();
        let gen_pos = order.iter().position(|&id| id == gen_id).unwrap();
        let explicit_pos = order.iter().position(|&id| id == explicit_id).unwrap();
        assert!(gen_pos < explicit_pos,
            "pandoc (pos {gen_pos}) must run before explicit (pos {explicit_pos})");
    }

    /// `filter_by_targets` rebuilds the graph with new ids; the edges between
    /// surviving products must be re-resolved so a producer still runs
    /// `retain_products` must leave the graph fully consistent, not just
    /// iterable. It used to be a bare `Vec::retain`, which left every
    /// surviving product's `id` pointing at the wrong index as soon as an
    /// earlier product was dropped — so `get_product(p.id)` returned a
    /// different product than `p`, and the lookup indexes still referenced
    /// removed ids.
    #[test]
    fn retain_products_rebuilds_indexes() {
        let mut g = BuildGraph::new();
        g.add_product(vec!["a.py".into()], vec![], "ruff", None).unwrap();
        g.add_product(vec!["b.md".into()], vec!["b.html".into()], "pandoc", None).unwrap();
        g.add_product(vec!["c.py".into()], vec![], "ruff", None).unwrap();

        // Drop the FIRST product, so every survivor's id must shift.
        g.retain_products(|p| p.processor != "ruff" || p.primary_input() != Path::new("a.py"));

        assert_eq!(g.products().len(), 2);
        for (idx, product) in g.products().iter().enumerate() {
            assert_eq!(product.id, idx, "id must equal index after retain");
            let fetched = g.get_product(product.id).expect("id must resolve");
            assert_eq!(fetched.inputs, product.inputs, "get_product(id) must return that product");
            // Dependency adjacency must be sized for the new id space.
            assert!(g.get_dependencies(product.id).is_empty() || product.id < g.products().len());
        }
        // Output ownership must point at the survivor's new id, not the old one.
        let owner = g.path_owner(Path::new("b.html")).expect("output owner must survive");
        assert_eq!(g.products()[owner].primary_input(), Path::new("b.md"));
        // The dropped product's output must no longer be owned.
        assert!(g.path_owner(Path::new("a.py")).is_none());
    }

    /// Target filtering must keep a kept consumer's upstream producer, so the
    /// producer still builds before its consumer under `build -t <pattern>`.
    #[test]
    fn filter_by_targets_preserves_dependencies() {
        let mut g = BuildGraph::new();
        g.add_product(vec!["other.txt".into()], vec![], "check", None).unwrap();
        let producer = g.add_product(
            vec!["a.md".into()],
            vec!["out.html".into()],
            "pandoc",
            None,
        ).unwrap();
        let consumer = g.add_product(
            vec!["out.html".into()],
            vec!["final.pdf".into()],
            "chromium",
            None,
        ).unwrap();
        g.resolve_dependencies();
        assert_eq!(g.get_dependencies(consumer), &[producer]);

        // Filter keeps the producer/consumer pair, drops the checker
        g.filter_by_targets(&["a.md".to_string(), "out.html".to_string()]).unwrap();
        assert_eq!(g.products().len(), 2);

        // Ids were reassigned; the consumer must still depend on the producer
        let new_producer = g.products().iter()
            .find(|p| p.processor == "pandoc").unwrap().id;
        let new_consumer = g.products().iter()
            .find(|p| p.processor == "chromium").unwrap().id;
        assert_eq!(g.get_dependencies(new_consumer), &[new_producer]);

        let order = g.topological_sort().unwrap();
        let prod_pos = order.iter().position(|&id| id == new_producer).unwrap();
        let cons_pos = order.iter().position(|&id| id == new_consumer).unwrap();
        assert!(prod_pos < cons_pos,
            "producer (pos {prod_pos}) must run before consumer (pos {cons_pos})");
    }

    /// `filter_by_targets` rebuilds the graph from scratch, and used to carry
    /// its own copy of `add_product`'s index registration — two sites that had
    /// to evolve in lockstep. Both now go through `register_product`, so a
    /// filtered graph must be indistinguishable from one built directly:
    /// every lookup index, the `id == index` invariant, and per-product state
    /// like `output_dirs` all survive.
    #[test]
    fn filtering_preserves_every_index_and_product_field() {
        let mut g = BuildGraph::new();
        g.add_product(vec!["drop.txt".into()], vec![], "check", None).unwrap();
        g.add_product_with_output_dir(
            vec!["keep.rs".into()],
            vec!["keep.bin".into()],
            "cargo",
            Some("cfg".into()),
            PathBuf::from("target/debug"),
        ).unwrap();
        // A checker on the kept input, to exercise the dedup index.
        g.add_product(vec!["keep.rs".into()], vec![], "clippy", None).unwrap();
        g.resolve_dependencies();

        g.filter_by_targets(&["keep.rs".to_string()]).unwrap();
        assert_eq!(g.products().len(), 2, "only the two keep.rs products survive");

        // id == index holds by construction after the rebuild.
        for (i, p) in g.products().iter().enumerate() {
            assert_eq!(p.id, i, "product {i} has id {} after filtering", p.id);
        }

        // Output ownership index survived and points at the right product.
        let owner = g.path_owner(Path::new("keep.bin")).expect("output must still be owned");
        assert_eq!(g.get_product(owner).unwrap().processor, "cargo");

        // Per-product state that lives outside the constructor args survived
        // the move through the rebuild.
        let cargo = g.products().iter().find(|p| p.processor == "cargo").unwrap();
        assert_eq!(cargo.output_dirs.len(), 1);
        assert_eq!(cargo.output_dirs[0].as_ref(), &PathBuf::from("target/debug"));
        assert!(cargo.cache_key.digest().is_some(), "config hash must survive");

        // The checker dedup index was rebuilt: re-declaring the same checker
        // returns the existing id instead of adding a duplicate.
        let before = g.products().len();
        let dup = g.add_product(vec!["keep.rs".into()], vec![], "clippy", None).unwrap();
        assert_eq!(g.products().len(), before, "re-declared checker must dedup");
        assert_eq!(g.get_product(dup).unwrap().processor, "clippy");
    }

    /// Targeting only a consumer must transitively keep its producer —
    /// otherwise the consumer builds against a missing or stale input.
    #[test]
    fn filter_by_targets_closes_over_producers() {
        let mut g = BuildGraph::new();
        g.add_product(vec!["other.txt".into()], vec![], "check", None).unwrap();
        g.add_product(vec!["a.md".into()], vec!["out.html".into()], "pandoc", None).unwrap();
        g.add_product(vec!["out.html".into()], vec!["final.pdf".into()], "chromium", None).unwrap();
        g.resolve_dependencies();

        // Only the consumer's input matches; the producer must be pulled in.
        g.filter_by_targets(&["out.html".to_string()]).unwrap();
        assert_eq!(g.products().len(), 2, "producer must be kept transitively");

        let new_producer = g.products().iter().find(|p| p.processor == "pandoc").unwrap().id;
        let new_consumer = g.products().iter().find(|p| p.processor == "chromium").unwrap().id;
        assert_eq!(g.get_dependencies(new_consumer), &[new_producer]);
    }

    /// The closure is transitive through chains, and walks producers only —
    /// targeting the head of a chain must not pull in its consumers.
    #[test]
    fn filter_by_targets_closure_is_transitive_and_upstream_only() {
        fn chain() -> BuildGraph {
            let mut g = BuildGraph::new();
            g.add_product(vec!["a.src".into()], vec!["a.mid".into()], "gen1", None).unwrap();
            g.add_product(vec!["a.mid".into()], vec!["a.out".into()], "gen2", None).unwrap();
            g.add_product(vec!["a.out".into()], vec!["a.final".into()], "gen3", None).unwrap();
            g.resolve_dependencies();
            g
        }

        // Targeting the tail keeps the whole upstream chain.
        let mut tail = chain();
        tail.filter_by_targets(&["a.out".to_string()]).unwrap();
        assert_eq!(tail.products().len(), 3, "whole upstream chain must survive");

        // Targeting the head keeps only the head — no downstream pull-in.
        let mut head = chain();
        head.filter_by_targets(&["a.src".to_string()]).unwrap();
        assert_eq!(head.products().len(), 1, "consumers must not be pulled in");
        assert_eq!(head.products()[0].processor, "gen1");
    }

    /// When a no-output product is re-declared with the same inputs,
    /// dedup should return the existing id without modification.
    #[test]
    fn checker_dedup_identical_redeclaration() {
        let mut g = BuildGraph::new();
        let id1 = g.add_product(
            vec!["a.py".into(), "b.py".into()],
            vec![],
            "ruff",
            None,
        ).unwrap();
        let id2 = g.add_product(
            vec!["a.py".into(), "b.py".into()],
            vec![],
            "ruff",
            None,
        ).unwrap();
        assert_eq!(id1, id2);
        assert_eq!(g.products().len(), 1);
        assert_eq!(g.get_product(id1).unwrap().inputs.len(), 2);
    }

    /// A checker re-declaring the same product with inputs that are NOT a
    /// superset is a genuine disagreement and must hard-error, exactly as the
    /// generator path does for conflicting outputs. It used to be silently
    /// ignored — `try_update_inputs`'s `false` was discarded, so the second
    /// declaration's inputs vanished and the checker ran on the first set.
    #[test]
    fn checker_dedup_non_superset_is_an_error() {
        let mut g = BuildGraph::new();
        g.add_product(
            vec!["a.py".into(), "b.py".into()],
            vec![],
            "ruff",
            None,
        ).unwrap();
        // Same processor + primary input, but drops b.py and adds c.py —
        // not a superset, so the two declarations genuinely disagree.
        let err = g.add_product(
            vec!["a.py".into(), "c.py".into()],
            vec![],
            "ruff",
            None,
        ).expect_err("non-superset checker re-declaration must be rejected");
        let msg = format!("{err}");
        assert!(msg.contains("Input conflict"), "unexpected message: {msg}");
        assert!(msg.contains("ruff"), "message should name the processor: {msg}");
    }

    /// When a no-output product is re-declared with inputs that are NOT a
    /// superset (different primary input), it should create a new product.
    #[test]
    fn checker_dedup_different_primary_input_creates_new() {
        let mut g = BuildGraph::new();
        let id1 = g.add_product(
            vec!["a.py".into()],
            vec![],
            "ruff",
            None,
        ).unwrap();
        let id2 = g.add_product(
            vec!["b.py".into()],
            vec![],
            "ruff",
            None,
        ).unwrap();
        assert_ne!(id1, id2);
        assert_eq!(g.products().len(), 2);
    }

    /// Generator dedup: same processor re-declaring the same outputs with
    /// expanded inputs should update the product (not conflict).
    #[test]
    fn generator_dedup_updates_inputs_on_superset() {
        let mut g = BuildGraph::new();
        let id1 = g.add_product(
            vec!["a.md".into()],
            vec!["out/a.html".into()],
            "pandoc",
            None,
        ).unwrap();
        // Re-declare with a superset of inputs (e.g. dep_inputs resolved more files)
        let id2 = g.add_product(
            vec!["a.md".into(), "style.css".into()],
            vec!["out/a.html".into()],
            "pandoc",
            None,
        ).unwrap();
        assert_eq!(id1, id2);
        assert_eq!(g.products().len(), 1);
        assert_eq!(g.get_product(id1).unwrap().inputs.len(), 2);
    }

    /// Generator dedup: same processor, same outputs, but non-superset inputs
    /// must produce an output conflict error.
    #[test]
    fn generator_dedup_non_superset_is_conflict() {
        let mut g = BuildGraph::new();
        g.add_product(
            vec!["a.c".into()],
            vec!["out.o".into()],
            "cc",
            None,
        ).unwrap();
        let result = g.add_product(
            vec!["b.c".into()],
            vec!["out.o".into()],
            "cc",
            None,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Output conflict"));
    }
}
