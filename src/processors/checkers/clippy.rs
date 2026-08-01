use anyhow::Result;
use std::path::Path;
use std::process::Command;

use crate::config::ClippyConfig;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, SiblingFilter, DirectoryProductOpts, discover_directory_products, run_in_anchor_dir, anchor_display_dir, check_command_output};

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
            checksum_fields: <crate::config::ClippyConfig as crate::config::KnownFields>::checksum_fields(),
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
        defconfig_json: crate::registries::default_config_json::<crate::config::ClippyConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::ClippyConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::ClippyConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::ClippyConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::ClippyConfig>,
        keywords: &["rust", "linter", "cargo", "rs"],
        description: "Lint Rust projects using Cargo Clippy",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
