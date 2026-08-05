use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{StandardConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, run_in_anchor_dir, anchor_display_dir, check_command_output};

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
/// Pip config. Custom: none (uses standard.command as the pip executable).
pub struct PipConfig {
    #[serde(flatten)]
    pub standard: StandardConfig,
}

pub struct PipProcessor {
    config: PipConfig,
}

impl PipProcessor {
    pub const fn new(config: PipConfig) -> Self {
        Self {
            config,
        }
    }

    /// Run pip install -r requirements.txt in the file's directory
    fn execute_pip(&self, ctx: &crate::build_context::BuildContext, requirements_txt: &Path) -> Result<()> {
        let mut cmd = Command::new(&self.config.standard.command);
        cmd.arg("install");
        cmd.arg("-r").arg(requirements_txt.file_name()
            .context("requirements.txt path has no filename")?
        );
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        let output = run_in_anchor_dir(ctx, &mut cmd, requirements_txt)?;
        check_command_output(&output, format_args!("pip install in {}", anchor_display_dir(requirements_txt)))
    }
}

impl Processor for PipProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        !file_index.scan(&self.config.standard, false).is_empty()
    }

    fn config_json(&self) -> Option<String> {
        serde_json::to_string(&self.config).ok()
    }

    fn required_tools(&self) -> Vec<String> {
        vec![self.config.standard.command.clone(), "python3".to_string()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let files = file_index.scan(&self.config.standard, false);
        if files.is_empty() {
            return Ok(());
        }

        let hash = Some(output_config_hash(&self.config, &crate::config::checksum_fields_of(instance_name)));
        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;

        for anchor in files {
            let mut inputs: Vec<PathBuf> = Vec::with_capacity(1 + extra.len());
            inputs.push(anchor.clone());
            inputs.extend_from_slice(&extra);

            graph.add_product(inputs, vec![], instance_name, hash.clone())?;
        }

        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.execute_pip(ctx, product.primary_input())
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(PipProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "pip",
        processor_type: crate::processors::ProcessorType::Creator,
        create: plugin_create,
        fields: &[],
        omit_standard_fields: &["formats", "dep_auto", "output_dir"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &["requirements.txt"], src_exclude_dirs: &[] }),
        defaults: Some(crate::config::ProcessorDefaults { command: "pip", ..crate::config::ProcessorDefaults::EMPTY }),
        defconfig_json: crate::registries::default_config_json::<PipConfig>,
        keywords: &["python", "pip", "package-manager", "py"],
        description: "Install Python dependencies using pip",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
