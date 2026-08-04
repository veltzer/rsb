use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{RustSingleFileConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, run_command, check_command_output};

pub struct RustSingleFileProcessor {
    config: RustSingleFileConfig,
}

impl RustSingleFileProcessor {
    pub const fn new(config: RustSingleFileConfig) -> Self {
        Self {
            config,
        }
    }

    fn get_output_path(&self, source: &Path) -> PathBuf {
        let src_dirs = self.config.standard.src_dirs();
        let full_parent = crate::processors::parent_dir_or_empty(source);
        let parent = src_dirs.iter()
            .filter(|d| !d.is_empty())
            .find_map(|d| full_parent.strip_prefix(d).ok())
            .unwrap_or(full_parent);
        let stem = source.file_stem().unwrap_or_default();
        let output_name = format!("{}{}", stem.to_string_lossy(), self.config.output_suffix);
        Path::new(&self.config.standard.output_dir).join(parent).join(output_name)
    }
}

impl Processor for RustSingleFileProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    // Serialize the FULL config (the trait default covers StandardConfig
    // only), so the extra fields reach config-change detection.
    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn required_tools(&self) -> Vec<String> {
        vec![self.config.standard.command.clone()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let files = file_index.scan(&self.config.standard, true);
        if files.is_empty() {
            return Ok(());
        }

        let hash = Some(output_config_hash(&self.config, <crate::config::RustSingleFileConfig as crate::config::KnownFields>::checksum_fields()));
        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;

        for source in &files {
            let output = self.get_output_path(source);

            let mut inputs = Vec::with_capacity(1 + extra.len());
            inputs.push(source.clone());
            inputs.extend_from_slice(&extra);

            graph.add_product(
                inputs,
                vec![output],
                instance_name,
                hash.clone(),
            )?;
        }

        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let source = product.primary_input();
        let output = product.primary_output();

        crate::processors::ensure_output_dir(output)?;

        let mut cmd = Command::new(&self.config.standard.command);
        for flag in &self.config.flags {
            cmd.arg(flag);
        }
        cmd.arg("-o").arg(output).arg(source);

        let out = run_command(ctx, &cmd)?;
        check_command_output(&out, format_args!("rustc {}", source.display()))
    }

}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(RustSingleFileProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "rust_single_file",
        processor_type: crate::processors::ProcessorType::Generator,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::RustSingleFileConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::RustSingleFileConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::RustSingleFileConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::RustSingleFileConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::RustSingleFileConfig>,
        keywords: &["rust", "compiler", "rs", "cargo", "binary", "executable"],
        description: "Compile single-file Rust programs into executables",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
