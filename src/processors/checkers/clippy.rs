use anyhow::Result;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::StandardConfig;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, SiblingFilter, DirectoryProductOpts, discover_directory_products, run_in_anchor_dir, anchor_display_dir, check_command_output};

fn default_cargo() -> String {
    "cargo".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
/// Clippy config. Custom: cargo.
pub struct ClippyConfig {
    #[serde(default = "default_cargo")]
    pub cargo: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for ClippyConfig {
    fn default() -> Self {
        Self {
            cargo: "cargo".into(),
            standard: StandardConfig::default(),
        }
    }
}

pub struct ClippyProcessor {
    config: ClippyConfig,
}

impl ClippyProcessor {
    pub const fn new(config: ClippyConfig) -> Self {
        Self {
            config,
        }
    }

    /// Run cargo clippy in the Cargo.toml's directory
    fn execute_clippy(&self, ctx: &crate::build_context::BuildContext, cargo_toml: &Path) -> Result<()> {
        let subcommand = self.config.standard.require_command(crate::processors::names::CLIPPY)?;
        let mut cmd = Command::new(&self.config.cargo);
        cmd.arg(subcommand);
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        let output = run_in_anchor_dir(ctx, &mut cmd, cargo_toml)?;
        check_command_output(&output, format_args!("cargo {} in {}", subcommand, anchor_display_dir(cargo_toml)))
    }
}

impl Processor for ClippyProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn required_tools(&self) -> Vec<String> {
        vec![self.config.cargo.clone()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        discover_directory_products(graph, DirectoryProductOpts {
            scan: &self.config.standard,
            file_index,
            dep_inputs: &self.config.standard.dep_inputs,
            cfg_hash: &self.config,
            checksum_fields: crate::config::checksum_fields_of(instance_name),
            siblings: &SiblingFilter {
                extensions: &[".rs", ".toml"],
                excludes: &["/.git/", "/target/", "/.rsconstruct/"],
            },
            processor_name: instance_name,
            output_dir_name: None,
        })
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.execute_clippy(ctx, product.primary_input())
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(ClippyProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "clippy",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        fields: &[
            crate::config::FieldSpec { name: "cargo", ty: crate::config::FieldType::String,
                affects_output: true, required: false,
                doc: "Path to the cargo executable" },
        ],
        omit_standard_fields: &["formats", "output_dir"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &["Cargo.toml"], src_exclude_dirs: &[] }),
        defaults: Some(crate::config::ProcessorDefaults { command: "clippy", ..crate::config::ProcessorDefaults::EMPTY }),
        defconfig_json: crate::registries::default_config_json::<ClippyConfig>,
        keywords: &["rust", "linter", "cargo", "rs"],
        description: "Lint Rust projects using Cargo Clippy",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
