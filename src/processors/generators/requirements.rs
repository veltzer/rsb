//! Requirements generator — produces a `requirements.txt` from Python imports.
//!
//! Scans every `.py` file in the project, collects the top-level import names,
//! filters out local modules (resolve to project files) and stdlib, maps each
//! remaining import name to its PyPI distribution name, and writes the sorted
//! result to `requirements.txt`.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::analyzers::python::scan_python_imports;
use crate::config::{RequirementsConfig, output_config_hash, resolve_extra_inputs, KnownFields};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, ensure_output_dir};

use super::python_distribution_map;
use super::python_stdlib;

pub struct RequirementsProcessor {
    config: RequirementsConfig,
}

impl RequirementsProcessor {
    pub const fn new(config: RequirementsConfig) -> Self {
        Self {
            config,
        }
    }

    /// Map an import name to a distribution name: user config wins over the
    /// built-in curated table, which in turn wins over identity.
    fn distribution_for(&self, import_name: &str) -> String {
        if let Some(mapped) = self.config.mapping.get(import_name) {
            return mapped.clone();
        }
        python_distribution_map::resolve_distribution(import_name).to_string()
    }
}

impl Processor for RequirementsProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &Product, verbose: bool) -> Result<usize> {
        crate::processors::ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        !file_index.scan(&self.config.standard, false).is_empty()
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let files = file_index.scan(&self.config.standard, true);
        if files.is_empty() {
            return Ok(());
        }

        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;
        let mut inputs = Vec::with_capacity(files.len() + extra.len());
        inputs.extend(files);
        inputs.extend_from_slice(&extra);

        let output = PathBuf::from(&self.config.output);
        graph.add_product(
            inputs,
            vec![output],
            instance_name,
            Some(output_config_hash(&self.config, RequirementsConfig::checksum_fields())),
        )?;
        Ok(())
    }

    fn execute(&self, _ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let output_path = product.primary_output();
        ensure_output_dir(output_path)?;

        // The file index is not available inside execute(). Build a local set
        // of input .py files to recognize local imports that resolve to
        // another product input.
        let local_py: HashSet<&Path> = product.inputs.iter()
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("py"))
            .map(std::path::PathBuf::as_path)
            .collect();

        let exclude: HashSet<&str> = self.config.exclude.iter()
            .map(std::string::String::as_str)
            .collect();

        // Preserve first-seen order for the non-sorted case; BTreeSet gives
        // sorted order for free when requested.
        let mut first_seen: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for input in &product.inputs {
            if input.extension().and_then(|e| e.to_str()) != Some("py") {
                continue;
            }
            let modules = scan_python_imports(input)
                .with_context(|| format!("Failed to scan imports in {}", input.display()))?;
            for module in modules {
                let top = module.split('.').next().unwrap_or(&module);
                if top.is_empty() {
                    continue;
                }
                if exclude.contains(top) {
                    continue;
                }
                if python_stdlib::is_stdlib(top) {
                    continue;
                }
                if is_local(input, top, &local_py, &self.config.python_paths) {
                    continue;
                }
                let dist = self.distribution_for(top);
                if seen.insert(dist.clone()) {
                    first_seen.push(dist);
                }
            }
        }

        // Distributions explicitly listed in `extra` are appended after the
        // import-derived set. They bypass exclude and stdlib filters because
        // they were declared by the user on purpose. Order: import-derived
        // first, then extras (only matters when sorted=false).
        for dist in &self.config.extra {
            if seen.insert(dist.clone()) {
                first_seen.push(dist.clone());
            }
        }

        let entries: Vec<String> = if self.config.sorted {
            let set: BTreeSet<String> = first_seen.into_iter().collect();
            set.into_iter().collect()
        } else {
            first_seen
        };

        let mut file = fs::File::create(output_path)
            .with_context(|| format!("Failed to create {}", output_path.display()))?;
        if self.config.header {
            writeln!(file, "# Generated by rsconstruct — do not edit by hand")
                .with_context(|| format!("Failed to write header to {}", output_path.display()))?;
        }
        for entry in &entries {
            writeln!(file, "{entry}")
                .with_context(|| format!("Failed to write entry to {}", output_path.display()))?;
        }

        Ok(())
    }
}

/// Check whether an import from `source` resolves to a file that's part of
/// the project's Python input set. The importer's own directory and the
/// project root are always checked. Additional roots come from the
/// processor's `python_paths` config — directories the user declares as
/// being on `sys.path` at runtime (e.g. via PYTHONPATH or `sys.path.insert`).
fn is_local(
    source: &Path,
    module: &str,
    local_py: &HashSet<&Path>,
    python_paths: &[String],
) -> bool {
    let module_path = module.replace('.', "/");
    let source_dir = source.parent().unwrap_or(Path::new("."));

    let mut roots: Vec<PathBuf> = Vec::with_capacity(2 + python_paths.len());
    roots.push(source_dir.to_path_buf());
    roots.push(PathBuf::from("."));
    for p in python_paths {
        roots.push(PathBuf::from(p));
    }

    for root in &roots {
        let candidates = [
            root.join(format!("{module_path}.py")),
            root.join(&module_path).join("__init__.py"),
        ];
        for candidate in &candidates {
            if local_py.contains(candidate.as_path()) {
                return true;
            }
            if candidate.is_file() {
                return true;
            }
        }
    }
    false
}

fn plugin_create(toml: &toml::Value) -> Result<Box<dyn Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(RequirementsProcessor::new(cfg)))
}

inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "requirements",
        processor_type: crate::processors::ProcessorType::Generator,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<RequirementsConfig>,
        known_fields: crate::registries::typed_known_fields::<RequirementsConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<RequirementsConfig>,
        must_fields: crate::registries::typed_must_fields::<RequirementsConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<RequirementsConfig>,
        keywords: &["python", "pip", "requirements", "dependencies", "generator", "py"],
        description: "Generate requirements.txt from Python import statements",
        is_native: true,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
