use anyhow::{Context, Result};
use std::path::Path;

use crate::config::IyamllintConfig;
use crate::graph::Product;

pub struct IyamllintProcessor {
    config: IyamllintConfig,
}

impl IyamllintProcessor {
    pub const fn new(config: IyamllintConfig) -> Self {
        Self { config }
    }

    fn execute_product(&self, product: &Product) -> Result<()> {
        self.check_files(&[product.primary_input()])
    }

    fn check_files(&self, files: &[&Path]) -> Result<()> {
        let mut errors = Vec::new();

        for file in files {
            let contents = std::fs::read_to_string(file)
                .with_context(|| format!("Failed to read {}", file.display()))?;
            if let Err(e) = serde_yaml::from_str::<serde_yaml::Value>(&contents) {
                errors.push(format!("{}: {}", file.display(), e));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("Invalid YAML:\n{}", errors.join("\n"))
        }
    }
}

impl crate::processors::Processor for IyamllintProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    fn standard_config(&self) -> Option<&crate::config::StandardConfig> {
        Some(&self.config.standard)
    }

    fn required_tools(&self) -> Vec<String> {
        Vec::new()
    }


    fn execute(&self, _ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.execute_product(product)
    }


    fn execute_batch(&self, _ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        crate::processors::execute_checker_batch_per_file(products, |file| self.check_files(&[file]))
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(IyamllintProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "iyamllint",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::IyamllintConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::IyamllintConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::IyamllintConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::IyamllintConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::IyamllintConfig>,
        keywords: &["yaml", "yml", "linter", "validator"],
        description: "Validate YAML files (in-process)",
        is_native: true,
        can_fix: false,
        supports_batch: true,
        max_jobs_cap: None,
    }
}
