use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::StandardConfig;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{
    ProcessorBase, Processor,
    run_command, check_command_output, ensure_output_dir,
};
use crate::config::output_config_hash;

/// Explicit config. Custom: inputs, `input_globs`, `output_files`, `output_dirs`.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct ExplicitConfig {
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub input_globs: Vec<String>,
    #[serde(default)]
    pub output_files: Vec<String>,
    #[serde(default)]
    pub output_dirs: Vec<String>,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

pub struct ExplicitProcessor {
    config: ExplicitConfig,
}

impl ExplicitProcessor {
    pub const fn new(config: ExplicitConfig) -> Self {
        Self {
            config,
        }
    }

    /// Resolve literal inputs. Unlike `dep_inputs`, missing files are silently
    /// skipped — they may be virtual files from upstream generators that only
    /// appear after fixed-point discovery injects them into the `FileIndex`.
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
        // glob). A bad pattern is an error — silently resolving to zero
        // inputs would hide a config typo behind missing dependencies.
        //
        // Matching runs against the FILE INDEX ONLY (real files plus virtual
        // files from upstream generators). A previous version also globbed
        // the raw filesystem and unioned the two — half-honoring the user's
        // `.gitignore`/`.rsconstructignore`: a pattern like `**/*.json`
        // silently swept node_modules/ into the input set.
        for pattern in &self.config.input_globs {
            let pat = glob::Pattern::new(pattern)
                .with_context(|| format!("Invalid input_globs pattern: {pattern}"))?;
            let mut glob_results: Vec<PathBuf> = file_index.files().iter()
                .filter(|file| pat.matches(&file.to_string_lossy()))
                .cloned()
                .collect();
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
        let mut tools = if self.config.standard.command.is_empty() {
            Vec::new()
        } else {
            vec![self.config.standard.command.clone()]
        };
        // `command` here is frequently a wrapper script, so the tool it shells
        // out to is only visible if the config names it.
        tools.extend(self.config.standard.required_tools.iter().cloned());
        tools
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
        let hash = Some(output_config_hash(&self.config, &crate::config::checksum_fields_of(instance_name)));

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
        fields: &[
            crate::config::FieldSpec { name: "command", ty: crate::config::FieldType::String,
                affects_output: true, required: true,
                doc: "Command to run to produce the outputs" },
            crate::config::FieldSpec { name: "inputs", ty: crate::config::FieldType::StringArray,
                affects_output: false, required: false,
                doc: "Explicit list of input files" },
            crate::config::FieldSpec { name: "input_globs", ty: crate::config::FieldType::StringArray,
                affects_output: false, required: false,
                doc: "Glob patterns for input files" },
            crate::config::FieldSpec { name: "output_files", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Output files produced by the command" },
            crate::config::FieldSpec { name: "output_dirs", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Output directories produced by the command" },
        ],
        omit_standard_fields: &["formats", "dep_inputs", "dep_auto", "output_dir", "batch", "max_jobs"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[], src_exclude_dirs: &[] }),
        defaults: Some(crate::config::ProcessorDefaults::EMPTY),
        defconfig_json: crate::registries::default_config_json::<ExplicitConfig>,
        keywords: &["explicit", "command", "custom", "script"],
        description: "Run a command with explicitly declared inputs and outputs",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
