use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{StandardConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, SiblingFilter, run_in_anchor_dir, anchor_display_dir, check_command_output};

fn default_cargo() -> String {
    "cargo".into()
}

fn default_cargo_profiles() -> Vec<String> {
    vec!["dev".into(), "release".into()]
}

/// Cargo config. Custom: cargo, profiles, `cache_output_dir`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CargoConfig {
    #[serde(default = "default_cargo")]
    pub cargo: String,
    #[serde(default = "default_cargo_profiles")]
    pub profiles: Vec<String>,
    #[serde(default = "crate::config::default_true")]
    pub cache_output_dir: bool,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for CargoConfig {
    fn default() -> Self {
        Self {
            cargo: "cargo".into(),
            profiles: default_cargo_profiles(),
            cache_output_dir: true,
            standard: StandardConfig::default(),
        }
    }
}

pub struct CargoProcessor {
    config: CargoConfig,
}

impl CargoProcessor {
    pub const fn new(config: CargoConfig) -> Self {
        Self {
            config,
        }
    }

    /// Run cargo build in the Cargo.toml's directory with the given profile
    fn execute_cargo(&self, ctx: &crate::build_context::BuildContext, cargo_toml: &Path, profile: &str) -> Result<()> {
        let subcommand = self.config.standard.require_command(crate::processors::names::CARGO)?;
        let mut cmd = Command::new(&self.config.cargo);
        cmd.arg(subcommand);
        cmd.args(["--profile", profile]);
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        let output = run_in_anchor_dir(ctx, &mut cmd, cargo_toml)?;
        check_command_output(&output, format_args!("cargo {} --profile {} in {}", subcommand, profile, anchor_display_dir(cargo_toml)))
    }
}

impl Processor for CargoProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean_output_dir(product, &product.processor, verbose)
    }

    fn required_tools(&self) -> Vec<String> {
        vec![self.config.cargo.clone()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let Some(files) = crate::processors::scan_or_skip(&self.config.standard, file_index) else {
            return Ok(());
        };

        let siblings = SiblingFilter {
            extensions: &[".rs", ".toml"],
            excludes: &["/.git/", "/target/", "/.rsconstruct/"],
        };
        let hash = Some(output_config_hash(&self.config, &crate::config::checksum_fields_of(instance_name)));
        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;

        for anchor in files {
            let anchor_dir = anchor.parent().map(std::path::Path::to_path_buf).unwrap_or_default();

            let sibling_files = file_index.query(
                &anchor_dir,
                siblings.extensions,
                siblings.excludes,
                &[],
                &[],
                &[],
            );

            let base_inputs = crate::processors::build_anchor_inputs(&anchor, &sibling_files, &extra);

            for profile in &self.config.profiles {
                let inputs = base_inputs.clone();
                if self.config.cache_output_dir {
                    let output_dir = if anchor_dir.as_os_str().is_empty() {
                        PathBuf::from("target")
                    } else {
                        anchor_dir.join("target")
                    };
                    graph.add_product_with_output_dir_and_variant(
                        inputs,
                        vec![],
                        instance_name,
                        hash.clone(),
                        output_dir,
                        Some(profile),
                    )?;
                } else {
                    graph.add_product_with_variant(
                        inputs,
                        vec![],
                        instance_name,
                        hash.clone(),
                        Some(profile),
                    )?;
                }
            }
        }

        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let profile = product.variant.as_deref().unwrap_or("dev");
        self.execute_cargo(ctx, product.primary_input(), profile)
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(CargoProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "cargo",
        processor_type: crate::processors::ProcessorType::Creator,
        create: plugin_create,
        fields: &[
            crate::config::FieldSpec { name: "cargo", ty: crate::config::FieldType::String,
                affects_output: true, required: false,
                doc: "Path to the cargo executable" },
            crate::config::FieldSpec { name: "profiles", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Build profiles to run (e.g. dev, release)" },
            crate::config::FieldSpec { name: "cache_output_dir", ty: crate::config::FieldType::Bool,
                affects_output: false, required: false,
                doc: "Cache the entire output directory as a unit" },
        ],
        omit_standard_fields: &["formats", "dep_auto", "output_dir"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &["Cargo.toml"], src_exclude_dirs: &[] }),
        defaults: Some(crate::config::ProcessorDefaults { command: "build", ..crate::config::ProcessorDefaults::EMPTY }),
        defconfig_json: crate::registries::default_config_json::<CargoConfig>,
        keywords: &["rust", "builder", "cargo", "rs", "package-manager"],
        description: "Build Rust projects using Cargo",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
