use anyhow::Result;
use std::process::Command;

use crate::config::MarkdownlintConfig;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, check_command_output, run_command};

pub struct MarkdownlintProcessor {
    config: MarkdownlintConfig,
}

impl MarkdownlintProcessor {
    pub const fn new(config: MarkdownlintConfig) -> Self {
        Self {
            config,
        }
    }
}

impl Processor for MarkdownlintProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    // Serialize the FULL config (the trait default covers StandardConfig
    // only), so the extra fields reach config-change detection.
    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn required_tools(&self) -> Vec<String> {
        vec![self.config.standard.command.clone()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        crate::processors::discover_checker_products(
            graph, &self.config.standard, file_index,
            &self.config.standard.dep_inputs, &self.config.standard.dep_auto,
            &self.config,
            <crate::config::MarkdownlintConfig as crate::config::KnownFields>::checksum_fields(),
            instance_name,
        )
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let file = product.primary_input();
        let mut cmd = Command::new(&self.config.standard.command);
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        cmd.arg(file);
        let output = run_command(ctx, &cmd)?;
        check_command_output(&output, format_args!("markdownlint {}", file.display()))
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(MarkdownlintProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "markdownlint",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::MarkdownlintConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::MarkdownlintConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::MarkdownlintConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::MarkdownlintConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::MarkdownlintConfig>,
        keywords: &["markdown", "md", "linter", "node", "npm"],
        description: "Lint Markdown files using markdownlint",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
