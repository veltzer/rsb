use anyhow::Result;
use std::path::Path;

use crate::config::AsciiConfig;
use crate::graph::Product;

pub struct AsciiProcessor {
    config: AsciiConfig,
}

impl AsciiProcessor {
    pub const fn new(config: AsciiConfig) -> Self {
        Self { config }
    }

    fn execute_product(&self, product: &Product) -> Result<()> {
        self.check_files(&[product.primary_input()])
    }

    fn check_files(&self, files: &[&Path]) -> Result<()> {
        let mut errors = Vec::new();

        for file in files {
            let content = crate::errors::ctx(std::fs::read(file), &format!("Failed to read {}", file.display()))?;
            let mut line_num = 1usize;
            let mut col = 1usize;
            let mut line_errors: Vec<String> = Vec::new();

            for &byte in &content {
                if byte == b'\n' {
                    line_num += 1;
                    col = 1;
                } else if !byte.is_ascii() {
                    line_errors.push(format!(
                        "{}:{}:{}: non-ASCII byte 0x{:02x}",
                        file.display(), line_num, col, byte,
                    ));
                    col += 1;
                } else {
                    col += 1;
                }
            }

            if !line_errors.is_empty() {
                errors.extend(line_errors);
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("Non-ASCII characters found:\n{}", errors.join("\n"))
        }
    }
}

impl crate::processors::Processor for AsciiProcessor {
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
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(AsciiProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "ascii",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::AsciiConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::AsciiConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::AsciiConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::AsciiConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::AsciiConfig>,
        keywords: &["checker", "encoding", "ascii", "text", "validator"],
        description: "Check files for non-ASCII characters",
        is_native: true,
        can_fix: false,
        supports_batch: true,
        max_jobs_cap: None,
    }
}
