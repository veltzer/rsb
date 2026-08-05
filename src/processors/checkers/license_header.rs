use anyhow::Result;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::StandardConfig;
use crate::graph::Product;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct LicenseHeaderConfig {
    #[serde(default)]
    pub header_lines: Vec<String>,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

pub struct LicenseHeaderProcessor {
    config: LicenseHeaderConfig,
}

impl LicenseHeaderProcessor {
    pub const fn new(config: LicenseHeaderConfig) -> Self {
        Self { config }
    }

    fn execute_product(&self, product: &Product) -> Result<()> {
        self.check_files(&[product.primary_input()])
    }

    fn check_files(&self, files: &[&Path]) -> Result<()> {
        if self.config.header_lines.is_empty() {
            return Ok(());
        }
        let mut errors = Vec::new();

        for &file in files {
            let content = crate::errors::ctx(std::fs::read_to_string(file), &format!("Failed to read {}", file.display()))?;
            let mut lines = content.lines();

            // Skip shebang line if present
            let mut first_line = lines.next().unwrap_or("");
            if first_line.starts_with("#!") {
                first_line = lines.next().unwrap_or("");
            }

            let file_lines: Vec<&str> = std::iter::once(first_line).chain(lines).collect();

            let mut found = false;
            for header_line in &self.config.header_lines {
                if file_lines.iter().any(|l| l.contains(header_line.as_str())) {
                    found = true;
                    break;
                }
            }

            if !found {
                errors.push(format!(
                    "{}: missing license header (expected one of: {})",
                    file.display(),
                    self.config.header_lines.join(", "),
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            anyhow::bail!(
                "{} file(s) missing license headers:\n{}",
                errors.len(),
                errors.join("\n"),
            )
        }
    }
}

impl crate::processors::Processor for LicenseHeaderProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    // Serialize the FULL config (the trait default covers StandardConfig
    // only), so the extra fields reach config-change detection.
    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn auto_detect(&self, file_index: &crate::file_index::FileIndex) -> bool {
        crate::processors::checker_auto_detect(&self.config.standard, file_index)
    }

    fn required_tools(&self) -> Vec<String> {
        Vec::new()
    }

    fn discover(
        &self,
        graph: &mut crate::graph::BuildGraph,
        file_index: &crate::file_index::FileIndex,
        instance_name: &str,
    ) -> anyhow::Result<()> {
        crate::processors::discover_checker_products(
            graph, &self.config.standard, file_index,
            &self.config.standard.dep_inputs, &self.config.standard.dep_auto,
            &self.config,
            &crate::config::checksum_fields_of(instance_name),
            instance_name,
        )
    }

    fn execute(&self, _ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.execute_product(product)
    }

    fn execute_batch(&self, _ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        crate::processors::execute_checker_batch_per_file(products, |file| self.check_files(&[file]))
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(LicenseHeaderProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "license_header",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        fields: &[
            crate::config::FieldSpec { name: "header_lines", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Lines of the license header that must appear at the top of each file" },
        ],
        omit_standard_fields: &["command", "formats", "output_dir"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".py", ".rs", ".js", ".ts", ".c", ".cc", ".h", ".hh", ".java", ".rb", ".go", ".sh", ".bash"], src_exclude_dirs: &[] }),
        defaults: None,
        defconfig_json: crate::registries::default_config_json::<LicenseHeaderConfig>,
        keywords: &["checker", "license", "header", "copyright"],
        description: "Verify source files contain required license headers",
        is_native: true,
        can_fix: false,
        supports_batch: true,
        max_jobs_cap: None,
    }
}
