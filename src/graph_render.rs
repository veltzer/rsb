//! Graph renderers — DOT, Mermaid, JSON, text, SVG, HTML.
//!
//! Split out of `graph.rs`, which mixed the core build-graph data model with
//! six output formats. Rendering is a consumer of the graph, not part of it;
//! keeping them together meant every change to either recompiled and re-read
//! the other.

use std::collections::HashSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::errors;
use crate::graph::{BuildGraph, Product};
use crate::processors::names as proc_names;

impl BuildGraph {
    /// Generate a safe node ID from a path
    fn path_node_id(path: &Path) -> String {
        let s = path.display().to_string();
        // Make safe for DOT/Mermaid: replace special chars
        format!("f_{}", s.replace(['.', '-', '/', ' '], "_"))
    }

    /// Generate a node ID for a processor
    fn processor_node_id(product: &Product) -> String {
        format!("proc_{}", product.id)
    }

    /// Get file label (just the filename)
    fn file_label(path: &Path) -> String {
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string()
    }

    /// Format graph as DOT (Graphviz)
    pub fn to_dot(&self) -> String {
        let mut buf = String::new();
        let _ = writeln!(buf, "digraph build_graph {{");
        let _ = writeln!(buf, "    rankdir=LR;");

        // Collect all unique input and output files
        let mut input_files: HashSet<PathBuf> = HashSet::new();
        let mut output_files: HashSet<PathBuf> = HashSet::new();

        for product in &self.products {
            for input in &product.inputs {
                input_files.insert(input.clone());
            }
            for output in &product.outputs {
                output_files.insert(output.clone());
            }
        }

        // Add file nodes (inputs that are not outputs = source files)
        let _ = writeln!(buf, "\n    // Source files");
        for file in &input_files {
            if !output_files.contains(file) {
                let node_id = Self::path_node_id(file);
                let label = Self::file_label(file);
                let _ = writeln!(buf, "    {node_id} [label=\"{label}\" shape=note style=filled fillcolor=white];");
            }
        }

        let _ = writeln!(buf, "\n    // Generated files");
        for file in &output_files {
            let node_id = Self::path_node_id(file);
            let label = Self::file_label(file);
            let color = if input_files.contains(file) { "lightgreen" } else { "lightyellow" };
            let _ = writeln!(buf, "    {node_id} [label=\"{label}\" shape=note style=filled fillcolor={color}];");
        }

        let _ = writeln!(buf, "\n    // Processors");
        for product in &self.products {
            let node_id = Self::processor_node_id(product);
            let color = match product.processor.as_str() {
                proc_names::TERA => "lightblue",
                proc_names::CC_SINGLE_FILE => "lightsalmon",
                _ => "lightgray",
            };
            let _ = writeln!(buf, "    {} [label=\"{}\" shape=box style=filled fillcolor={}];",
                node_id, product.processor, color);
        }

        let _ = writeln!(buf, "\n    // Edges");
        for product in &self.products {
            let proc_id = Self::processor_node_id(product);

            // Input files -> processor
            for input in &product.inputs {
                let input_id = Self::path_node_id(input);
                let _ = writeln!(buf, "    {input_id} -> {proc_id};");
            }

            // Processor -> output files
            for output in &product.outputs {
                let output_id = Self::path_node_id(output);
                let _ = writeln!(buf, "    {proc_id} -> {output_id};");
            }
        }

        let _ = write!(buf, "}}");
        buf
    }

    /// Format graph as Mermaid
    /// Only shows primary source files (first input per product), not headers,
    /// to keep the diagram manageable for large projects.
    pub fn to_mermaid(&self) -> String {
        let mut buf = String::new();
        let _ = writeln!(buf, "graph LR");

        // Collect primary source files (first input only) and output files
        let mut source_files: HashSet<PathBuf> = HashSet::new();
        let mut output_files: HashSet<PathBuf> = HashSet::new();

        for product in &self.products {
            if let Some(first_input) = product.inputs.first() {
                source_files.insert(first_input.clone());
            }
            for output in &product.outputs {
                output_files.insert(output.clone());
            }
        }

        let _ = writeln!(buf, "\n    %% Source files");
        for file in &source_files {
            if !output_files.contains(file) {
                let node_id = Self::path_node_id(file);
                let label = Self::file_label(file);
                let _ = writeln!(buf, "    {node_id}[/\"{label}\"/]");
            }
        }

        let _ = writeln!(buf, "\n    %% Generated files");
        for file in &output_files {
            let node_id = Self::path_node_id(file);
            let label = Self::file_label(file);
            let _ = writeln!(buf, "    {node_id}[/\"{label}\"/]");
        }

        let _ = writeln!(buf, "\n    %% Processors");
        for product in &self.products {
            let node_id = Self::processor_node_id(product);
            let _ = writeln!(buf, "    {}[\"{}\" ]", node_id, product.processor);
        }

        let _ = writeln!(buf, "\n    %% Edges");
        for product in &self.products {
            let proc_id = Self::processor_node_id(product);

            // Only connect primary source file (first input), skip headers
            if let Some(first_input) = product.inputs.first() {
                let input_id = Self::path_node_id(first_input);
                let _ = writeln!(buf, "    {input_id} --> {proc_id}");
            }

            for output in &product.outputs {
                let output_id = Self::path_node_id(output);
                let _ = writeln!(buf, "    {proc_id} --> {output_id}");
            }
        }

        // Add styling
        let tera_procs: Vec<_> = self.products.iter()
            .filter(|p| p.processor == proc_names::TERA)
            .map(Self::processor_node_id)
            .collect();
        let cc_procs: Vec<_> = self.products.iter()
            .filter(|p| p.processor == proc_names::CC_SINGLE_FILE)
            .map(Self::processor_node_id)
            .collect();

        for proc_id in &tera_procs {
            let _ = writeln!(buf, "\n    style {proc_id} fill:#add8e6");
        }
        for proc_id in &cc_procs {
            let _ = writeln!(buf, "\n    style {proc_id} fill:#ffa07a");
        }

        buf.truncate(buf.trim_end().len());
        buf
    }

    /// Format graph as JSON
    pub fn to_json(&self) -> String {
        let nodes: Vec<serde_json::Value> = self.products.iter()
            .map(|product| {
                let inputs: Vec<String> = product.inputs.iter()
                    .map(|p| p.display().to_string())
                    .collect();
                let outputs: Vec<String> = product.outputs.iter()
                    .map(|p| p.display().to_string())
                    .collect();
                serde_json::json!({
                    "id": product.id,
                    "processor": product.processor,
                    "inputs": inputs,
                    "outputs": outputs,
                    "depends_on": self.dependencies.get(product.id).expect(errors::INVALID_PRODUCT_ID),
                })
            })
            .collect();

        let root = serde_json::json!({ "products": nodes });
        serde_json::to_string_pretty(&root).expect(errors::JSON_SERIALIZE)
    }

    /// Format graph as plain text
    pub fn to_text(&self) -> String {
        let mut buf = String::new();
        let _ = writeln!(buf, "Build Dependency Graph");
        let _ = writeln!(buf, "======================");

        // Get topological order
        let Ok(order) = self.topological_sort() else {
            let _ = writeln!(buf, "Error: Cycle detected in graph");
            buf.truncate(buf.trim_end().len());
            return buf;
        };

        for id in order {
            let product = self.products.get(id).expect(errors::INVALID_PRODUCT_ID);
            let inputs: Vec<_> = product.inputs.iter()
                .filter_map(|p| p.file_name())
                .filter_map(|n| n.to_str())
                .collect();
            let outputs: Vec<_> = product.outputs.iter()
                .filter_map(|p| p.file_name())
                .filter_map(|n| n.to_str())
                .collect();

            let _ = writeln!(buf, "[{}] {} -> {}",
                product.processor,
                inputs.join(", "),
                outputs.join(", "));

            // Show dependencies
            let deps = self.dependencies.get(product.id).expect(errors::INVALID_PRODUCT_ID);
            if !deps.is_empty() {
                let dep_names: Vec<_> = deps.iter()
                    .map(|&d| {
                        let dep = self.products.get(d).expect(errors::INVALID_PRODUCT_ID);
                        let out: Vec<_> = dep.outputs.iter()
                            .filter_map(|p| p.file_name())
                            .filter_map(|n| n.to_str())
                            .collect();
                        out.join(", ")
                    })
                    .collect();
                let _ = writeln!(buf, "    depends on: {}", dep_names.join(", "));
            }
        }

        if self.products.is_empty() {
            let _ = writeln!(buf, "(empty graph)");
        }

        buf.truncate(buf.trim_end().len());
        buf
    }

    /// Generate SVG by piping DOT through the `dot` command.
    pub fn to_svg(&self, ctx: &crate::build_context::BuildContext) -> Result<String> {
        crate::processors::dot_to_svg(ctx, &self.to_dot())
    }

    /// Generate a self-contained HTML file with Mermaid diagram
    pub fn to_html(&self) -> String {
        let mermaid_content = self.to_mermaid();
        format!(r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>RSConstruct Build Graph</title>
    <script src="https://cdn.jsdelivr.net/npm/mermaid/dist/mermaid.min.js"></script>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            margin: 40px;
            background: #f5f5f5;
        }}
        h1 {{
            color: #333;
        }}
        .mermaid {{
            background: white;
            padding: 20px;
            border-radius: 8px;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
        }}
    </style>
</head>
<body>
    <h1>RSConstruct Build Graph</h1>
    <div class="mermaid">
{mermaid_content}
    </div>
    <script>
        mermaid.initialize({{ startOnLoad: true, theme: 'default', maxTextSize: 500000 }});
    </script>
</body>
</html>
"#)
    }
}
