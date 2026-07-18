use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

use crate::config::ExplicitConfig;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{
    ProcessorBase, Processor,
    run_command, check_command_output, ensure_output_dir,
};
use crate::config::output_config_hash;

pub struct ExplicitProcessor {
    config: ExplicitConfig,
}

impl ExplicitProcessor {
    pub const fn new(config: ExplicitConfig) -> Self {
        Self {
            config,
        }
    }

    /// Resolve literal inputs. Unlike dep_inputs, missing files are silently
    /// skipped — they may be virtual files from upstream generators that only
    /// appear after fixed-point discovery injects them into the FileIndex.
    fn resolve_inputs(&self, file_index: &FileIndex) -> Result<Vec<PathBuf>> {
        let mut resolved = Vec::new();
        // Literal inputs (in config order), only include files that exist
        // or are known to the file index (virtual files from upstream generators)
        for p in &self.config.inputs {
            let path = PathBuf::from(p);
            if path.exists() || file_index.contains(&path) {
                resolved.push(path);
            }
        }
        // Glob inputs (one glob at a time in config order, sorted within each
        // glob). A bad pattern or unreadable match is an error — silently
        // resolving to zero inputs would hide a config typo behind missing
        // dependencies.
        for pattern in &self.config.input_globs {
            let mut glob_results: Vec<PathBuf> = Vec::new();
            // Match against real files on disk
            let entries = glob::glob(pattern)
                .with_context(|| format!("Invalid input_globs pattern: {pattern}"))?;
            for entry in entries {
                let entry = entry
                    .with_context(|| format!("Failed to read glob match for pattern: {pattern}"))?;
                if entry.is_file() {
                    glob_results.push(entry);
                }
            }
            // Also match against virtual files in the file index
            let pat = glob::Pattern::new(pattern)
                .with_context(|| format!("Invalid input_globs pattern: {pattern}"))?;
            for file in file_index.files() {
                if pat.matches(&file.to_string_lossy()) && !glob_results.contains(file) {
                    glob_results.push(file.clone());
                }
            }
            glob_results.sort();
            glob_results.dedup();
            resolved.extend(glob_results);
        }
        Ok(resolved)
    }
}

impl Processor for ExplicitProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn auto_detect(&self, _file_index: &FileIndex) -> bool {
        // Only active if a command is configured and outputs are declared.
        !self.config.standard.command.is_empty()
            && (!self.config.output_files.is_empty() || !self.config.output_dirs.is_empty())
    }

    fn required_tools(&self) -> Vec<String> {
        if self.config.standard.command.is_empty() {
            Vec::new()
        } else {
            vec![self.config.standard.command.clone()]
        }
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        if self.config.output_files.is_empty() && self.config.output_dirs.is_empty() {
            return Ok(());
        }

        let inputs = self.resolve_inputs(file_index)?;
        if inputs.is_empty() && self.config.inputs.is_empty() && self.config.input_globs.is_empty() {
            return Ok(());
        }

        let output_files: Vec<PathBuf> = self.config.output_files.iter().map(PathBuf::from).collect();
        let output_dirs: Vec<PathBuf> = self.config.output_dirs.iter().map(PathBuf::from).collect();
        let hash = Some(output_config_hash(&self.config, <crate::config::ExplicitConfig as crate::config::KnownFields>::checksum_fields()));

        if output_dirs.is_empty() {
            graph.add_product(inputs, output_files, instance_name, hash)?;
        } else {
            graph.add_product_with_output_dirs_and_variant(
                inputs, output_files, instance_name, hash, output_dirs, None,
            )?;
        }

        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        // Ensure output file directories exist
        for output in &product.outputs {
            ensure_output_dir(output)?;
        }

        let command = self.config.standard.require_command(crate::processors::names::EXPLICIT)?;
        let mut cmd = Command::new(command);
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        cmd.arg("--inputs");
        for input in &product.inputs {
            cmd.arg(input);
        }
        if !self.config.output_files.is_empty() {
            cmd.arg("--output-files");
            for f in &self.config.output_files {
                cmd.arg(f);
            }
        }
        if !self.config.output_dirs.is_empty() {
            cmd.arg("--output-dirs");
            for d in &self.config.output_dirs {
                cmd.arg(d);
            }
        }

        let out = run_command(ctx, &cmd)?;
        check_command_output(
            &out,
            format_args!("{} ({} inputs)",
                command,
                product.inputs.len(),
            ),
        )
    }

    fn clean(&self, product: &Product, verbose: bool) -> Result<usize> {
        let file_count = ProcessorBase::clean(product, &product.processor, verbose)?;
        let dir_count = crate::processors::clean_output_dir(product, &product.processor, verbose)?;
        Ok(file_count + dir_count)
    }

    fn config_json(&self) -> Option<String> {
        ProcessorBase::config_json(&self.config)
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(ExplicitProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "explicit",
        processor_type: crate::processors::ProcessorType::Explicit,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::ExplicitConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::ExplicitConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::ExplicitConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::ExplicitConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::ExplicitConfig>,
        keywords: &["explicit", "command", "custom", "script"],
        description: "Run a command with explicitly declared inputs and outputs",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
