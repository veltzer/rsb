use std::path::PathBuf;
use std::process::Command;
use anyhow::Result;

use serde::{Deserialize, Serialize};

use crate::config::{StandardConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{ProcessorBase, Processor,
    run_in_anchor_dir, anchor_display_dir, check_command_output};

/// Config for Creator processors — run a command and cache declared outputs.
#[derive(Debug, Deserialize, Serialize, Clone)]
/// Creator processor config.
/// Custom fields: `output_dirs`, `output_files`.
/// Unused `StandardConfig` fields: formats, `output_dir`.
#[derive(Default)]
pub struct CreatorConfig {
    /// Directories to cache after the command runs.
    #[serde(default)]
    pub output_dirs: Vec<String>,
    /// Individual files to cache after the command runs.
    #[serde(default)]
    pub output_files: Vec<String>,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

/// A data-driven processor that runs a command and caches declared outputs.
///
/// Unlike generators (1 input → 1 output) or checkers (validate only),
/// a Creator runs a command that may produce any combination of files and
/// directories. The user declares what outputs to cache in the config.
///
/// Example config:
/// ```toml
/// [processor.creator.pip]
/// command = "pip"
/// args = ["install", "-r", "requirements.txt"]
/// output_dirs = [".venv"]
/// src_extensions = ["requirements.txt"]
/// ```
pub struct CreatorProcessor {
    config: CreatorConfig,
}

impl CreatorProcessor {
    pub const fn new(config: CreatorConfig) -> Self {
        Self {
            config,
        }
    }
}

impl Processor for CreatorProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    fn config_json(&self) -> Option<String> {
        ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &Product, verbose: bool) -> Result<usize> {
        ProcessorBase::clean_output_dir(product, &product.processor, verbose)
    }

    fn required_tools(&self) -> Vec<String> {
        if self.config.standard.command.is_empty() {
            Vec::new()
        } else {
            vec![self.config.standard.command.clone()]
        }
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let Some(files) = crate::processors::scan_or_skip(&self.config.standard, file_index) else {
            return Ok(());
        };

        let hash = Some(output_config_hash(&self.config, &crate::config::checksum_fields_of(instance_name)));
        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;

        for anchor in files {
            let anchor_dir = anchor.parent().map(std::path::Path::to_path_buf).unwrap_or_default();

            let mut inputs = Vec::with_capacity(1 + extra.len());
            inputs.push(anchor.clone());
            inputs.extend_from_slice(&extra);

            let resolve = |rel: &str| -> PathBuf {
                if anchor_dir.as_os_str().is_empty() {
                    PathBuf::from(rel)
                } else {
                    anchor_dir.join(rel)
                }
            };

            let output_files: Vec<PathBuf> = self.config.output_files.iter()
                .map(|f| resolve(f))
                .collect();

            let output_dirs: Vec<PathBuf> = self.config.output_dirs.iter()
                .map(|d| resolve(d))
                .collect();

            if output_dirs.is_empty() {
                graph.add_product(inputs, output_files, instance_name, hash.clone())?;
            } else {
                graph.add_product_with_output_dirs_and_variant(
                    inputs, output_files, instance_name, hash.clone(), output_dirs, None,
                )?;
            }
        }

        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let anchor = product.primary_input();
        let command = self.config.standard.require_command("creator")?;
        let mut cmd = Command::new(command);
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        let output = run_in_anchor_dir(ctx, &mut cmd, anchor)?;
        check_command_output(&output, format_args!("{} in {}", command, anchor_display_dir(anchor)))
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(CreatorProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "creator",
        processor_type: crate::processors::ProcessorType::Creator,
        create: plugin_create,
        fields: &[
            crate::config::FieldSpec { name: "output_dirs", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Directories to cache after the command runs" },
            crate::config::FieldSpec { name: "output_files", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Individual files to cache after the command runs" },
        ],
        omit_standard_fields: &["formats", "output_dir"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[], src_exclude_dirs: &[] }),
        defaults: None,
        defconfig_json: crate::registries::default_config_json::<CreatorConfig>,
        keywords: &["builder", "creator", "generic"],
        description: "Run a command and cache declared outputs",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
