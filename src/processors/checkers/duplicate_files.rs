use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;

use crate::config::{DuplicateFilesConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};

pub struct DuplicateFilesProcessor {
    config: DuplicateFilesConfig,
}

impl DuplicateFilesProcessor {
    pub const fn new(config: DuplicateFilesConfig) -> Self {
        Self { config }
    }

    fn check_files(&self, files: &[&Path]) -> Result<()> {
        let mut checksums: HashMap<Vec<u8>, Vec<&Path>> = HashMap::new();

        for &file in files {
            let bytes = crate::errors::ctx(std::fs::read(file), &format!("Failed to read {}", file.display()))?;
            let hash = Sha256::digest(&bytes).to_vec();
            checksums.entry(hash).or_default().push(file);
        }

        let mut duplicates: Vec<String> = Vec::new();
        for paths in checksums.values() {
            if paths.len() > 1 {
                let names: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                duplicates.push(format!("  {}", names.join(", ")));
            }
        }

        if duplicates.is_empty() {
            Ok(())
        } else {
            duplicates.sort();
            anyhow::bail!(
                "{} set(s) of duplicate files found:\n{}",
                duplicates.len(),
                duplicates.join("\n"),
            )
        }
    }
}

impl crate::processors::Processor for DuplicateFilesProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    fn required_tools(&self) -> Vec<String> {
        Vec::new()
    }

    /// Duplicate detection is a whole-set property: a single product spanning
    /// every scanned file. Per-file products would be chunked and cache-skipped
    /// by the executor, so pairs of duplicates could never be compared.
    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let scan = &self.config.standard;
        let mut inputs = file_index.scan(scan, true);
        if inputs.is_empty() {
            return Ok(());
        }
        let mut all_dep_inputs = scan.dep_inputs.clone();
        for ai in &scan.dep_auto {
            all_dep_inputs.extend(crate::processors::config_file_inputs(ai));
        }
        inputs.extend(resolve_extra_inputs(&all_dep_inputs)?);
        let hash = Some(output_config_hash(
            &self.config,
            &crate::config::checksum_fields_of(instance_name),
        ));
        graph.add_product(inputs, vec![], instance_name, hash)?;
        Ok(())
    }

    fn execute(&self, _ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let files: Vec<&Path> = product.inputs.iter().map(std::path::PathBuf::as_path).collect();
        self.check_files(&files)
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(DuplicateFilesProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "duplicate_files",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::DuplicateFilesConfig>,
        fields: &[],
        omit_standard_fields: &[],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".py", ".rs", ".js", ".ts", ".c", ".cc", ".h", ".hh", ".java", ".rb", ".go", ".sh", ".md", ".yaml", ".yml", ".json", ".toml", ".xml", ".html", ".css"], src_exclude_dirs: &[] }),
        defaults: None,
        keywords: &["checker", "duplicates", "files"],
        description: "Detect duplicate files by content (SHA-256)",
        is_native: true,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
