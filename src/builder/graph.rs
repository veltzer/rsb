use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use crate::cli::{GraphAction, GraphFormat, GraphViewer};
use crate::color;
use crate::json_output;
use crate::processors::log_command;
use super::Builder;

impl Builder {
    /// Dispatch graph subcommands
    pub fn graph(&self, ctx: &crate::build_context::BuildContext, action: GraphAction) -> Result<()> {
        match action {
            GraphAction::Show { format } => self.print_graph(ctx, format),
            GraphAction::View { viewer } => self.view_graph(ctx, viewer),
            GraphAction::Stats => self.graph_stats(ctx),
            GraphAction::Unreferenced { extensions, rm } => self.graph_unreferenced(ctx, extensions, rm),
            GraphAction::LookupFwd { files } => self.graph_lookup_fwd(ctx, files),
            GraphAction::LookupRev { files } => self.graph_lookup_rev(ctx, files),
        }
    }

    /// Print the dependency graph in the specified format
    fn print_graph(&self, ctx: &crate::build_context::BuildContext, format: GraphFormat) -> Result<()> {
        let graph = self.build_graph(ctx)?;

        // Output in the requested format
        let output = match format {
            GraphFormat::Dot => graph.to_dot(),
            GraphFormat::Mermaid => graph.to_mermaid(),
            GraphFormat::Json => graph.to_json(),
            GraphFormat::Text => graph.to_text(),
            GraphFormat::Svg => graph.to_svg(ctx)?,
        };

        println!("{output}");
        Ok(())
    }

    /// View the dependency graph in a viewer
    fn view_graph(&self, ctx: &crate::build_context::BuildContext, viewer: GraphViewer) -> Result<()> {
        use std::process::Command;

        let graph = self.build_graph(ctx)?;

        // Create temp file
        let temp_dir = std::env::temp_dir();

        match viewer {
            GraphViewer::Mermaid => {
                let html_path = temp_dir.join("rsconstruct_graph.html");
                let html_content = graph.to_html();
                fs::write(&html_path, html_content)
                    .with_context(|| format!("Failed to write HTML file: {}", html_path.display()))?;

                // Open in browser
                self.open_file(&html_path)?;
                println!("Opened graph in browser: {}", html_path.display());
            }
            GraphViewer::Svg => {
                // Check if dot is available
                let mut dot_check_cmd = Command::new("dot");
                dot_check_cmd.arg("-V");
                let dot_check = crate::processors::run_command_capture(ctx, &dot_check_cmd);
                if dot_check.map_or(true, |o| !o.status.success()) {
                    anyhow::bail!("Graphviz 'dot' command not found. Install Graphviz or use --view=mermaid");
                }

                let dot_path = temp_dir.join("rsconstruct_graph.dot");
                let svg_path = temp_dir.join("rsconstruct_graph.svg");

                // Write DOT file
                let dot_content = graph.to_dot();
                fs::write(&dot_path, dot_content)
                    .with_context(|| format!("Failed to write DOT file: {}", dot_path.display()))?;

                // Convert to SVG
                let mut dot_cmd = Command::new("dot");
                dot_cmd.arg("-Tsvg").arg(&dot_path).arg("-o").arg(&svg_path);
                let output = crate::processors::run_command_capture(ctx, &dot_cmd)
                    .context("Failed to run dot command")?;

                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    anyhow::bail!("dot command failed: {stderr}");
                }

                // Open SVG
                self.open_file(&svg_path)?;
                println!("Opened graph: {}", svg_path.display());
            }
        }

        Ok(())
    }

    /// Show graph statistics (products, processors, dependencies)
    fn graph_stats(&self, ctx: &crate::build_context::BuildContext) -> Result<()> {
        let graph = self.build_graph(ctx)?;
        let products = graph.products();

        // Aggregate per-processor stats
        let mut per_processor: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
        let mut total_edges = 0usize;

        for product in products {
            let entry = per_processor.entry(product.processor.as_str()).or_insert((0, 0, 0));
            entry.0 += 1; // product count
            entry.1 += product.inputs.len();
            entry.2 += product.outputs.len();
            total_edges += graph.get_dependencies(product.id).len();
        }

        if json_output::is_json_mode() {
            let stats: Vec<serde_json::Value> = per_processor.iter().map(|(proc, (count, inputs, outputs))| {
                serde_json::json!({
                    "processor": proc,
                    "products": count,
                    "inputs": inputs,
                    "outputs": outputs,
                })
            }).collect();
            let json = serde_json::json!({
                "processors": stats,
                "total_products": products.len(),
                "total_edges": total_edges,
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
        } else {
            for (proc, (count, inputs, outputs)) in &per_processor {
                println!("{}: {} products, {} inputs, {} outputs",
                    color::bold(proc), count, inputs, outputs);
            }
            println!();
            println!("{}: {} products, {} dependency edges",
                color::bold("Total"), products.len(), total_edges);
        }

        Ok(())
    }

    /// List files on disk not referenced by any product input (primary or dependency).
    fn graph_unreferenced(&self, ctx: &crate::build_context::BuildContext, extensions: Vec<String>, rm: bool) -> Result<()> {
        let graph = self.build_graph(ctx)?;

        // Collect every file that appears in any product's inputs
        let referenced: HashSet<PathBuf> = graph.products()
            .iter()
            .flat_map(|p| p.inputs.iter().cloned())
            .collect();

        // Normalise extensions: ensure they start with '.'
        let exts: Vec<String> = extensions.iter()
            .map(|e| if e.starts_with('.') { e.clone() } else { format!(".{e}") })
            .collect();

        // Walk the project directory for matching files
        let mut unreferenced: Vec<PathBuf> = Vec::new();
        collect_unreferenced(std::path::Path::new("."), &exts, &referenced, &mut unreferenced)?;

        unreferenced.sort();

        for path in &unreferenced {
            println!("{}", path.display());
        }

        if rm {
            for path in &unreferenced {
                fs::remove_file(path)
                    .with_context(|| format!("Failed to delete {}", path.display()))?;
            }
        }

        Ok(())
    }

    /// Open a file with the configured viewer or the system default application
    pub(super) fn open_file(&self, path: &std::path::Path) -> Result<()> {
        use std::process::Command;

        let cmd = if let Some(ref viewer) = self.config.graph.viewer {
            viewer.as_str()
        } else {
            "xdg-open"
        };

        let mut open_cmd = Command::new(cmd);
        open_cmd.arg(path);
        log_command(&open_cmd);
        // Deliberately NOT routed through run_command: this launches a
        // detached viewer (browser, image viewer) that must outlive
        // rsconstruct. The central runner sets kill_on_drop and waits for
        // the child, both of which are exactly wrong here.
        open_cmd
            .spawn()
            .with_context(|| format!("Failed to open file with {cmd}"))?;

        Ok(())
    }

    /// Forward lookup: for each given file, find products where it appears in `inputs`.
    /// Shows: the queried file → each consuming product's processor + outputs.
    ///
    /// Reports per file. Files that are not inputs to any product are listed as such.
    fn graph_lookup_fwd(&self, ctx: &crate::build_context::BuildContext, files: Vec<String>) -> Result<()> {
        let graph = self.build_graph(ctx)?;
        let queries = normalize_query_paths(&files);

        if json_output::is_json_mode() {
            #[derive(serde::Serialize)]
            struct Consumer<'a> {
                processor: &'a str,
                outputs: Vec<String>,
            }
            #[derive(serde::Serialize)]
            struct Entry<'a> {
                file: String,
                consumers: Vec<Consumer<'a>>,
            }
            let entries: Vec<Entry> = queries.iter().map(|q| {
                let consumers: Vec<Consumer> = graph.products_consuming(q).iter()
                    .map(|&id| &graph.products()[id])
                    .map(|p| Consumer {
                        processor: &p.processor,
                        outputs: p.outputs.iter().map(|o| o.display().to_string()).collect(),
                    })
                    .collect();
                Entry { file: q.display().to_string(), consumers }
            }).collect();
            println!("{}", serde_json::to_string_pretty(&entries)?);
            return Ok(());
        }

        for query in &queries {
            println!("{}", query.display());
            let consumers = graph.products_consuming(query);
            if consumers.is_empty() {
                println!("  (not consumed by any product)");
            } else {
                for &id in consumers {
                    let product = &graph.products()[id];
                    if product.outputs.is_empty() {
                        println!("  [{}] (no outputs — checker)", product.processor);
                    } else {
                        let outs: Vec<String> = product.outputs.iter()
                            .map(|o| o.display().to_string()).collect();
                        println!("  [{}] -> {}", product.processor, outs.join(", "));
                    }
                }
            }
        }
        Ok(())
    }

    /// Reverse lookup: for each given file, find the single product where it appears
    /// in `outputs`. Shows: the queried file → the producing processor + its inputs.
    ///
    /// By construction every declared output belongs to exactly one product
    /// (enforced at graph-build time via the output-conflict check).
    fn graph_lookup_rev(&self, ctx: &crate::build_context::BuildContext, files: Vec<String>) -> Result<()> {
        let graph = self.build_graph(ctx)?;
        let queries = normalize_query_paths(&files);

        if json_output::is_json_mode() {
            #[derive(serde::Serialize)]
            struct Producer<'a> {
                processor: &'a str,
                inputs: Vec<String>,
            }
            #[derive(serde::Serialize)]
            struct Entry<'a> {
                file: String,
                producer: Option<Producer<'a>>,
            }
            let entries: Vec<Entry> = queries.iter().map(|q| {
                let producer = graph.path_owner(q).map(|id| {
                    let p = &graph.products()[id];
                    Producer {
                        processor: &p.processor,
                        inputs: p.inputs.iter().map(|i| i.display().to_string()).collect(),
                    }
                });
                Entry { file: q.display().to_string(), producer }
            }).collect();
            println!("{}", serde_json::to_string_pretty(&entries)?);
            return Ok(());
        }

        for query in &queries {
            println!("{}", query.display());
            match graph.path_owner(query) {
                Some(id) => {
                    let p = &graph.products()[id];
                    if p.inputs.is_empty() {
                        println!("  [{}] (no inputs declared)", p.processor);
                    } else {
                        let ins: Vec<String> = p.inputs.iter()
                            .map(|i| i.display().to_string()).collect();
                        println!("  [{}] <- {}", p.processor, ins.join(", "));
                    }
                }
                None => println!("  (not produced by any product)"),
            }
        }
        Ok(())
    }
}

/// Normalize user-supplied paths: strip a leading `./` so queries match graph paths
/// which are stored without that prefix.
fn normalize_query_paths(files: &[String]) -> Vec<PathBuf> {
    files.iter().map(|s| {
        let p = Path::new(s);
        p.strip_prefix("./").unwrap_or(p).to_path_buf()
    }).collect()
}

/// Recursively collect files whose extension matches `exts` and are not in `referenced`.
fn collect_unreferenced(
    dir: &Path,
    exts: &[String],
    referenced: &HashSet<PathBuf>,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("Failed to read dir {}", dir.display()))? {
        let entry = entry.with_context(|| format!("Failed to read entry in {}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            // Skip hidden directories and common non-project dirs
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "target" {
                continue;
            }
            collect_unreferenced(&path, exts, referenced, out)?;
        } else if path.is_file()
            && let Some(ext) = path.extension().and_then(|e| e.to_str())
            && exts.contains(&format!(".{ext}"))
        {
            // Normalise to a path without leading "./"
            let clean = path.strip_prefix("./").unwrap_or(&path).to_path_buf();
            if !referenced.contains(&clean) {
                out.push(clean);
            }
        }
    }
    Ok(())
}
