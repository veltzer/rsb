use anyhow::Result;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::StandardConfig;
use crate::graph::Product;
use crate::processors::{run_command, check_command_output};

/// `ClangTidy` config. Custom fields: `compiler_args`.
/// Unused `StandardConfig` fields: command, formats, `output_dir`.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct ClangTidyConfig {
    #[serde(default)]
    pub compiler_args: Vec<String>,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

pub struct ClangTidyProcessor {
    config: ClangTidyConfig,
}

impl ClangTidyProcessor {
    pub const fn new(config: ClangTidyConfig) -> Self {
        Self { config }
    }

    fn execute_product(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let mut cmd = Command::new("clang-tidy");
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        cmd.arg(product.primary_input());
        // Add -- to separate clang-tidy args from compiler args
        cmd.arg("--");
        for arg in &self.config.compiler_args {
            cmd.arg(arg);
        }

        let output = run_command(ctx, &cmd)?;
        check_command_output(&output, "clang-tidy")
    }
}

impl crate::processors::Processor for ClangTidyProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn auto_detect(&self, file_index: &crate::file_index::FileIndex) -> bool {
        crate::processors::checker_auto_detect(&self.config.standard, file_index)
    }

    fn required_tools(&self) -> Vec<String> {
        vec!["clang-tidy".to_string()]
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

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.execute_product(ctx, product)
    }

    fn config_json(&self) -> Option<String> {
        serde_json::to_string(&self.config).ok()
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(ClangTidyProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "clang_tidy",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        fields: &[
            crate::config::FieldSpec { name: "compiler_args", ty: crate::config::FieldType::StringArray,
                affects_output: true, required: false,
                doc: "Compiler flags forwarded to clang-tidy for parsing" },
        ],
        omit_standard_fields: &["command", "formats", "output_dir"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".c", ".cc"], src_exclude_dirs: &[] }),
        defaults: None,
        defconfig_json: crate::registries::default_config_json::<ClangTidyConfig>,
        keywords: &["c", "cpp", "linter", "clang", "checker", "cc", "h", "hpp"],
        description: "Run clang-tidy static analysis on C/C++ source files",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
