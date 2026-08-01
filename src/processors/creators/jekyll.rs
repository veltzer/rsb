use anyhow::Result;
use std::path::Path;
use std::process::Command;

use crate::config::JekyllConfig;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, SiblingFilter, DirectoryProductOpts, discover_directory_products, run_in_anchor_dir, anchor_display_dir, check_command_output};

pub struct JekyllProcessor {
    config: JekyllConfig,
}

impl JekyllProcessor {
    pub const fn new(config: JekyllConfig) -> Self {
        Self {
            config,
        }
    }

    const fn should_process(&self) -> bool {
        true
    }

    fn execute_jekyll(&self, ctx: &crate::build_context::BuildContext, config_yml: &Path) -> Result<()> {
        let command = self.config.standard.require_command("jekyll")?;
        let mut cmd = Command::new(command);
        cmd.arg("build");
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        let output = run_in_anchor_dir(ctx, &mut cmd, config_yml)?;
        check_command_output(&output, format_args!("jekyll build in {}", anchor_display_dir(config_yml)))
    }
}

impl Processor for JekyllProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    fn standard_config(&self) -> Option<&crate::config::StandardConfig> {
        Some(&self.config.standard)
    }

    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }


    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean_output_dir(product, &product.processor, verbose)
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        self.should_process() && !file_index.scan(&self.config.standard, true).is_empty()
    }

    fn required_tools(&self) -> Vec<String> {
        vec![self.config.standard.command.clone(), "ruby".to_string()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        if !self.should_process() {
            return Ok(());
        }

        discover_directory_products(graph, DirectoryProductOpts {
            scan: &self.config.standard,
            file_index,
            dep_inputs: &self.config.standard.dep_inputs,
            cfg_hash: &self.config,
            checksum_fields: <crate::config::JekyllConfig as crate::config::KnownFields>::checksum_fields(),
            siblings: &SiblingFilter {
                extensions: &[""],
                excludes: &["/.git/", "/out/", "/.rsconstruct/", "/_site/"],
            },
            processor_name: instance_name,
            output_dir_name: Some("_site"),
        })
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.execute_jekyll(ctx, product.primary_input())
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(JekyllProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "jekyll",
        processor_type: crate::processors::ProcessorType::Creator,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::JekyllConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::JekyllConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::JekyllConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::JekyllConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::JekyllConfig>,
        keywords: &["ruby", "jekyll", "static-site", "html", "markdown", "web", "gem"],
        description: "Build Jekyll sites",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
