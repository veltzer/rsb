use anyhow::{Context, Result};
use std::path::Path;

use crate::config::ItaploConfig;
use crate::graph::Product;

pub struct ItaploProcessor {
    config: ItaploConfig,
}

impl ItaploProcessor {
    pub const fn new(config: ItaploConfig) -> Self {
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
            if let Err(e) = toml::from_str::<toml::Value>(&contents) {
                errors.push(format!("{}: {}", file.display(), e));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("Invalid TOML:\n{}", errors.join("\n"))
        }
    }
}

impl crate::processors::Processor for ItaploProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
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
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(ItaploProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "itaplo",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::ItaploConfig>,
        fields: &[],
        omit_standard_fields: &[],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".toml"], src_exclude_dirs: &[] }),
        defaults: None,
        keywords: &["toml", "formatter", "linter", "validator"],
        description: "Validate TOML files (in-process)",
        is_native: true,
        can_fix: false,
        supports_batch: true,
        max_jobs_cap: None,
    }
}
