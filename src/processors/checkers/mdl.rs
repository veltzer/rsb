use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{StandardConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, check_command_output, config_file_inputs, run_command};

fn default_gem_home() -> String {
    "gems".into()
}

fn default_gem_stamp() -> String {
    "out/gem/root.stamp".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MdlConfig {
    #[serde(default)]
    pub local_repo: bool,
    #[serde(default = "default_gem_home")]
    pub gem_home: String,
    #[serde(default = "default_gem_stamp")]
    pub gem_stamp: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for MdlConfig {
    fn default() -> Self {
        Self {
            local_repo: false,
            gem_home: "gems".into(),
            gem_stamp: "out/gem/root.stamp".into(),
            standard: StandardConfig::default(),
        }
    }
}

pub struct MdlProcessor {
    config: MdlConfig,
}

impl MdlProcessor {
    pub const fn new(config: MdlConfig) -> Self {
        Self {
            config,
        }
    }
}

impl Processor for MdlProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    // Serialize the FULL config (the trait default covers StandardConfig
    // only), so the extra fields reach config-change detection.
    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn required_tools(&self) -> Vec<String> {
        vec![self.config.standard.command.clone(), "ruby".to_string()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let files = file_index.scan(&self.config.standard, true);
        if files.is_empty() {
            return Ok(());
        }
        let hash = Some(output_config_hash(&self.config, &crate::config::checksum_fields_of(instance_name)));
        let mut dep_inputs = self.config.standard.dep_inputs.clone();
        for ai in &self.config.standard.dep_auto {
            dep_inputs.extend(config_file_inputs(ai));
        }
        let extra = resolve_extra_inputs(&dep_inputs)?;

        // Only depend on the gem stamp when using a local repo install
        let gem_stamp = if self.config.local_repo {
            Some(PathBuf::from(&self.config.gem_stamp))
        } else {
            None
        };

        for file in files {
            let mut inputs = Vec::with_capacity(1 + extra.len() + 1);
            inputs.push(file);
            inputs.extend_from_slice(&extra);
            if let Some(ref stamp) = gem_stamp {
                inputs.push(stamp.clone());
            }
            graph.add_product(inputs, vec![], instance_name, hash.clone())?;
        }
        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let file = product.primary_input();
        let mut cmd = Command::new(&self.config.standard.command);
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        cmd.arg(file);
        if self.config.local_repo {
            cmd.env("GEM_HOME", &self.config.gem_home);
            cmd.env("GEM_PATH", &self.config.gem_home);
        }
        let output = run_command(ctx, &cmd)?;
        check_command_output(&output, format_args!("mdl {}", file.display()))
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(MdlProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "mdl",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        fields: &[
            crate::config::FieldSpec { name: "local_repo", ty: crate::config::FieldType::Bool,
                affects_output: true, required: false,
                doc: "Use a local gem repository instead of system install" },
            crate::config::FieldSpec { name: "gem_home", ty: crate::config::FieldType::String,
                affects_output: true, required: false,
                doc: "Path to the local gem repository" },
            crate::config::FieldSpec { name: "gem_stamp", ty: crate::config::FieldType::String,
                affects_output: true, required: false,
                doc: "Stamp file tracking the local gem installation" },
        ],
        omit_standard_fields: &["formats", "output_dir"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] }),
        defaults: Some(crate::config::ProcessorDefaults { command: "mdl", dep_auto: &[".mdlrc"], ..crate::config::ProcessorDefaults::EMPTY }),
        defconfig_json: crate::registries::default_config_json::<MdlConfig>,
        keywords: &["markdown", "md", "linter", "ruby", "gem"],
        description: "Lint Markdown files using mdl (markdownlint)",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
