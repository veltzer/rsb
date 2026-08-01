//! `rsconstruct product show <path>` — print everything that contributes to a
//! single product's cache key: inputs (broken down by source), config_hash,
//! analyzer-contributed hash pieces (live-recomputed), and the resulting
//! descriptor key + cache state.
//!
//! Looking up the product from a path uses two passes: first by output
//! ownership (`graph.path_owner`), then by primary input. This matches the
//! mental model "I'm looking at the file `README.md` and want to know what
//! makes it stale" — the user is unlikely to know the internal product id.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

use crate::color;
use crate::deps_cache::DepsCache;
use crate::graph::{BuildGraph, Product};

use super::Builder;

impl Builder {
    /// Implement `rsconstruct product show <path>`.
    pub fn product_show(&self, ctx: &crate::build_context::BuildContext, path: &str, verbose: bool) -> Result<()> {
        let graph = self.build_graph_for_cache(ctx)?;
        let target = PathBuf::from(path);

        let product = resolve_product(&graph, &target).ok_or_else(|| {
            anyhow::anyhow!(
                "No product owns or consumes '{path}'. Try the output path \
                 (e.g. README.md) or the primary input path.",
            )
        })?;

        let deps_cache = DepsCache::open()?;
        // Group analyzer-attributed inputs by analyzer name. The analyzer
        // attaches deps to the *primary input*, so that's the lookup key.
        let analyzer_entries = deps_cache.get_raw_for_path(product.primary_input());
        let analyzer_inputs: BTreeMap<String, Vec<PathBuf>> = analyzer_entries.iter()
            .map(|(deps, name)| (name.clone(), deps.clone()))
            .collect();

        // Recompute hash pieces live, per analyzer. Only analyzers that
        // contribute pieces (Tera today) return Some.
        let analyzers = self.create_analyzers(false)?;
        let mut hash_pieces: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, analyzer) in &analyzers {
            if let Ok(Some(pieces)) = analyzer.scan_hash_pieces(ctx, product.primary_input())
                && !pieces.is_empty() {
                hash_pieces.insert(name.clone(), pieces);
            }
        }

        // Compute the descriptor key the executor would use.
        let input_checksum = match crate::checksum::combined_input_checksum(ctx, &product.inputs) {
            Ok(c) => c,
            Err(e) => {
                bail!("Failed to compute input checksum: {e}");
            }
        };
        let descriptor_key = product.descriptor_key(&input_checksum);

        // Cache state: what the executor would do with this descriptor right now.
        let cache_state = self.object_store()
            .explain_descriptor(ctx, &descriptor_key, &product.outputs, false);

        if crate::json_output::is_json_mode() {
            print_json(product, &input_checksum, &descriptor_key, &analyzer_inputs, &hash_pieces, &cache_state)?;
        } else {
            print_text(product, &input_checksum, &descriptor_key, &analyzer_inputs, &hash_pieces, &cache_state, verbose);
        }

        Ok(())
    }
}

/// Resolve a path to a single product. Tries output ownership first; if no
/// product owns the path, falls back to products that have it as primary
/// input. Returns None if neither lookup matches.
///
/// Multiple products could legitimately consume the same path as input
/// (e.g. one source feeding three products), so input-fallback only kicks
/// in when output lookup fails AND we'd otherwise have nothing to show —
/// in that case we take the first match and accept the ambiguity rather
/// than refusing.
fn resolve_product<'a>(graph: &'a BuildGraph, path: &Path) -> Option<&'a Product> {
    if let Some(id) = graph.path_owner(path) {
        return graph.get_product(id);
    }
    graph.products().iter().find(|p| p.primary_input() == path)
}

/// Text-mode pretty printer. Sections in this order:
///   header, outputs, inputs, hash pieces, cache state.
/// Each section is independently skippable when it has no content (e.g. a
/// product with no analyzer-attributed inputs).
fn print_text(
    product: &Product,
    input_checksum: &str,
    descriptor_key: &str,
    analyzer_inputs: &BTreeMap<String, Vec<PathBuf>>,
    hash_pieces: &BTreeMap<String, Vec<String>>,
    cache_state: &crate::object_store::ExplainAction,
    verbose: bool,
) {
    println!("{} {}", color::dim("processor:"), color::cyan(&product.processor));
    if let Some(v) = &product.variant {
        println!("{} {}", color::dim("variant:"), v);
    }

    println!("{}", color::dim("outputs:"));
    if product.outputs.is_empty() {
        println!("  {}", color::dim("(checker, no outputs)"));
    } else {
        for o in &product.outputs {
            println!("  {}", o.display());
        }
    }

    println!("{}", color::dim("inputs:"));
    let primary = product.primary_input();
    let analyzer_added: std::collections::HashSet<&PathBuf> = analyzer_inputs.values()
        .flatten()
        .collect();
    println!("  {} {}", color::cyan("primary"), primary.display());
    let mut other_count = 0;
    for inp in product.inputs.iter().skip(1) {
        if analyzer_added.contains(inp) {
            continue;
        }
        if other_count == 0 {
            println!("  {}", color::cyan("configured"));
        }
        println!("    {}", inp.display());
        other_count += 1;
    }
    for (analyzer, deps) in analyzer_inputs {
        if deps.is_empty() {
            continue;
        }
        println!("  {} [{}]", color::cyan("analyzer"), analyzer);
        for d in deps {
            println!("    {}", d.display());
        }
    }

    // Every non-input contributor to the cache key, in the order the build
    // contributed them. A changed descriptor_key is always attributable to
    // exactly one of these lines (or to input_checksum below).
    if product.cache_key.is_empty() {
        println!("{} {}", color::dim("cache_key:"), color::dim("(inputs only)"));
    } else {
        println!(
            "{} {}",
            color::dim("cache_key:"),
            product.cache_key.digest().unwrap_or_default(),
        );
        for (component, value) in product.cache_key.components() {
            // Analyzer components carry the raw contributed piece, which can
            // be a multi-line file list. Summarize it here — the hash_pieces
            // section below renders the same content in full (under
            // --verbose), and dumping it twice would bury the other
            // components.
            if value.contains('\n') {
                let lines = value.lines().count();
                println!(
                    "  {} {}",
                    color::cyan(component.tag()),
                    color::dim(&format!("({lines} lines, see hash_pieces below)")),
                );
            } else {
                println!("  {} {}", color::cyan(component.tag()), value);
            }
        }
    }

    if hash_pieces.is_empty() {
        println!("{} {}", color::dim("hash_pieces:"), color::dim("(none)"));
    } else {
        println!("{}", color::dim("hash_pieces:"));
        for (analyzer, pieces) in hash_pieces {
            println!("  [{analyzer}]");
            for piece in pieces {
                let (kind, body) = piece.split_once(':').unwrap_or((piece.as_str(), ""));
                if !verbose && kind.ends_with("_resolved") {
                    let n = if body.is_empty() { 0 } else { body.lines().count() };
                    println!(
                        "    {} {}",
                        color::cyan(kind),
                        color::dim(&format!("({n} files, pass --verbose to list)")),
                    );
                } else if body.contains('\n') {
                    println!("    {}", color::cyan(kind));
                    for line in body.lines() {
                        println!("      {line}");
                    }
                } else {
                    println!("    {} {}", color::cyan(kind), body);
                }
            }
        }
    }

    println!("{} {}", color::dim("input_checksum:"), input_checksum);
    println!("{} {}", color::dim("descriptor_key:"), descriptor_key);
    println!("{} {}", color::dim("cache_state:"), cache_state);
}

/// JSON-mode printer. Emits a single object with stable field names so it can
/// be diffed across runs (e.g. before/after committing a new file).
fn print_json(
    product: &Product,
    input_checksum: &str,
    descriptor_key: &str,
    analyzer_inputs: &BTreeMap<String, Vec<PathBuf>>,
    hash_pieces: &BTreeMap<String, Vec<String>>,
    cache_state: &crate::object_store::ExplainAction,
) -> Result<()> {
    let primary = product.primary_input();
    let analyzer_added: std::collections::HashSet<&PathBuf> = analyzer_inputs.values()
        .flatten()
        .collect();
    let configured: Vec<String> = product.inputs.iter().skip(1)
        .filter(|p| !analyzer_added.contains(*p))
        .map(|p| p.display().to_string())
        .collect();
    let analyzer_inputs_json: serde_json::Map<String, serde_json::Value> = analyzer_inputs.iter()
        .map(|(k, v)| (
            k.clone(),
            serde_json::Value::Array(v.iter().map(|p| serde_json::Value::String(p.display().to_string())).collect()),
        ))
        .collect();
    let hash_pieces_json: serde_json::Map<String, serde_json::Value> = hash_pieces.iter()
        .map(|(k, v)| (
            k.clone(),
            serde_json::Value::Array(v.iter().map(|s| serde_json::Value::String(s.clone())).collect()),
        ))
        .collect();

    let cache_state_str = cache_state.to_string();

    let out = serde_json::json!({
        "processor": product.processor,
        "variant": product.variant,
        "outputs": product.outputs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "inputs": {
            "primary": primary.display().to_string(),
            "configured": configured,
            "analyzer": analyzer_inputs_json,
        },
        "cache_key": product.cache_key.digest(),
        "cache_key_components": product.cache_key.components().iter()
            .map(|(c, v)| serde_json::json!({"component": c.tag(), "value": v}))
            .collect::<Vec<_>>(),
        "hash_pieces": hash_pieces_json,
        "input_checksum": input_checksum,
        "descriptor_key": descriptor_key,
        "cache_state": cache_state_str,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
