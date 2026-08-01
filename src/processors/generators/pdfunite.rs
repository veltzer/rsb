use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{PdfuniteConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, run_command, check_command_output};

use super::find_dirs_with_ext;

pub struct PdfuniteProcessor {
    config: PdfuniteConfig,
}

impl PdfuniteProcessor {
    pub const fn new(config: PdfuniteConfig) -> Self {
        Self {
            config,
        }
    }
}

impl Processor for PdfuniteProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn auto_detect(&self, _file_index: &FileIndex) -> bool {
        let base = Path::new(&self.config.source_dir);
        if !base.exists() {
            return false;
        }
        let ext = self.config.source_ext.strip_prefix('.').unwrap_or(&self.config.source_ext);
        match find_dirs_with_ext(base, ext) {
            Ok(dirs) => !dirs.is_empty(),
            Err(e) => {
                eprintln!("Warning: pdfunite auto-detect scan failed: {e:#}");
                false
            }
        }
    }

    fn required_tools(&self) -> Vec<String> {
        vec![self.config.standard.command.clone()]
    }

    fn discover(&self, graph: &mut BuildGraph, _file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let base = Path::new(&self.config.source_dir);
        if !base.exists() {
            return Ok(());
        }

        let hash = Some(output_config_hash(&self.config, <crate::config::PdfuniteConfig as crate::config::KnownFields>::checksum_fields()));
        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;
        let ext = self.config.source_ext.strip_prefix('.').unwrap_or(&self.config.source_ext);

        let dirs = find_dirs_with_ext(base, ext)?;

        // Compute upstream scan dir once
        let upstream_scan_dir = Path::new(&self.config.source_dir)
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .context("source_dir is empty")?;
        let upstream_scan_dirs = [upstream_scan_dir];

        for dir_path in dirs {
            // Find all source files in this directory
            let mut source_files: Vec<PathBuf> = crate::errors::ctx(fs::read_dir(&dir_path), &format!("Failed to read directory {}", dir_path.display()))?
                .filter_map(std::result::Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == ext))
                .collect();
            if source_files.is_empty() {
                continue;
            }
            source_files.sort();

            let inputs: Vec<PathBuf> = source_files.iter().map(|src| {
                super::output_path(src, &upstream_scan_dirs, &self.config.source_output_dir, "pdf")
            }).chain(extra.iter().cloned()).collect();

            // Mirror the directory structure from source_dir into output_dir,
            // naming each merged PDF after its leaf directory.
            let relative = dir_path.strip_prefix(base).unwrap_or(&dir_path);
            let parent = crate::processors::parent_dir_or_empty(relative);
            let leaf = relative.file_name()
                .with_context(|| format!("Cannot extract leaf directory name from {}", dir_path.display()))?;
            let outputs = vec![
                Path::new(&self.config.standard.output_dir).join(parent).join(format!("{}.pdf", leaf.to_string_lossy())),
            ];

            graph.add_product(inputs, outputs, instance_name, hash.clone())?;
        }

        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let output = product.primary_output();

        crate::processors::ensure_output_dir(output)?;

        // Inputs also carry dep_inputs (extra rebuild triggers); only actual
        // PDFs may be passed to pdfunite as pages.
        let pdf_inputs: Vec<&PathBuf> = product.inputs.iter()
            .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
            .collect();
        if pdf_inputs.is_empty() {
            anyhow::bail!("No PDF inputs found for {}", output.display());
        }

        let mut cmd = Command::new(&self.config.standard.command);
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        // pdfunite takes input files followed by output file
        for input in pdf_inputs {
            cmd.arg(input);
        }
        cmd.arg(output);

        let out = run_command(ctx, &cmd)?;
        check_command_output(&out, format_args!("pdfunite {}", output.display()))
    }

}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(PdfuniteProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "pdfunite",
        processor_type: crate::processors::ProcessorType::Generator,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::PdfuniteConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::PdfuniteConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::PdfuniteConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::PdfuniteConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::PdfuniteConfig>,
        keywords: &["pdf", "merger", "generator"],
        description: "Merge PDFs from subdirectories into course bundles",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
