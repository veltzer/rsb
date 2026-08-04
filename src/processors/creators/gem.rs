use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{GemConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, SiblingFilter, run_in_anchor_dir, anchor_display_dir, check_command_output};

pub struct GemProcessor {
    config: GemConfig,
}

impl GemProcessor {
    pub const fn new(config: GemConfig) -> Self {
        Self {
            config,
        }
    }

    /// Run bundle install in the Gemfile's directory
    fn execute_gem(&self, ctx: &crate::build_context::BuildContext, gemfile: &Path) -> Result<()> {
        let subcommand = "install";
        let mut cmd = Command::new(&self.config.standard.command);
        cmd.arg(subcommand);
        cmd.env("GEM_HOME", &self.config.gem_home);
        cmd.env("GEM_PATH", &self.config.gem_home);
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        let output = run_in_anchor_dir(ctx, &mut cmd, gemfile)?;
        check_command_output(&output, format_args!("bundle {} in {}", subcommand, anchor_display_dir(gemfile)))
    }
}

impl Processor for GemProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    // Serialize the FULL config (the trait default covers StandardConfig
    // only), so the extra fields reach config-change detection.
    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean_output_dir(product, &product.processor, verbose)
    }

    fn required_tools(&self) -> Vec<String> {
        vec![self.config.standard.command.clone(), "ruby".to_string()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let Some(files) = crate::processors::scan_or_skip(&self.config.standard, file_index) else {
            return Ok(());
        };

        let hash = Some(output_config_hash(&self.config, <crate::config::GemConfig as crate::config::KnownFields>::checksum_fields()));
        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;

        let siblings = SiblingFilter {
            extensions: &[".gemspec"],
            excludes: &["/.git/", "/out/", "/.rsconstruct/", "/gems/"],
        };

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

            let inputs = crate::processors::build_anchor_inputs(&anchor, &sibling_files, &extra);

            if self.config.cache_output_dir {
                let output_dir = if anchor_dir.as_os_str().is_empty() {
                    PathBuf::from(&self.config.gem_home)
                } else {
                    anchor_dir.join(&self.config.gem_home)
                };
                graph.add_product_with_output_dir(inputs, vec![], instance_name, hash.clone(), output_dir)?;
            } else {
                graph.add_product(inputs, vec![], instance_name, hash.clone())?;
            }
        }

        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.execute_gem(ctx, product.primary_input())
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(GemProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "gem",
        processor_type: crate::processors::ProcessorType::Creator,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::GemConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::GemConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::GemConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::GemConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::GemConfig>,
        keywords: &["ruby", "gem", "package-manager", "rb"],
        description: "Install Ruby dependencies using Bundler",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
