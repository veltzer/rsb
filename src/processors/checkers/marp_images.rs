use anyhow::Result;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

use crate::config::MarpImagesConfig;
use crate::graph::Product;

pub struct MarpImagesProcessor {
    config: MarpImagesConfig,
}

static IMAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    // The path capture must stop before whitespace, or `![alt](img.png "Title")`
    // captures "img.png " (trailing space) and the file check always fails.
    Regex::new(r#"!\[[^\]]*\]\(\s*([^)"\s]+)(?:\s+"[^"]*")?\s*\)"#).unwrap()
});

impl MarpImagesProcessor {
    pub const fn new(config: MarpImagesConfig) -> Self {
        Self { config }
    }

    fn execute_product(&self, product: &Product) -> Result<()> {
        self.check_files(&[product.primary_input()])
    }

    fn check_files(&self, files: &[&Path]) -> Result<()> {
        let mut errors = Vec::new();

        for &file in files {
            let content = crate::errors::ctx(std::fs::read_to_string(file), &format!("Failed to read {}", file.display()))?;
            let dir = crate::processors::parent_dir(file);

            for (line_num, line) in content.lines().enumerate() {
                for cap in IMAGE_RE.captures_iter(line) {
                    let image_path = &cap[1];
                    // Skip external URLs and data URIs
                    if image_path.starts_with("http://")
                        || image_path.starts_with("https://")
                        || image_path.starts_with("data:")
                    {
                        continue;
                    }
                    let resolved = dir.join(image_path);
                    if !resolved.exists() {
                        errors.push(format!(
                            "{}:{}: missing image: {}",
                            file.display(),
                            line_num + 1,
                            image_path,
                        ));
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!("Missing image references:\n{}", errors.join("\n"))
        }
    }
}

impl crate::processors::Processor for MarpImagesProcessor {
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
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(MarpImagesProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "marp_images",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        fields: &[],
        omit_standard_fields: &[],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] }),
        defaults: None,
        defconfig_json: crate::registries::default_config_json::<crate::config::MarpImagesConfig>,
        keywords: &["markdown", "marp", "images", "checker", "presentation"],
        description: "Validate image references in Marp markdown presentations",
        is_native: true,
        can_fix: false,
        supports_batch: true,
        max_jobs_cap: None,
    }
}
