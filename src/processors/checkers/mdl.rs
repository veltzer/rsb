use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;

use crate::config::{MdlConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, check_command_output, config_file_inputs, run_command};

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
        let hash = Some(output_config_hash(&self.config, <crate::config::MdlConfig as crate::config::KnownFields>::checksum_fields()));
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
        defconfig_json: crate::registries::default_config_json::<crate::config::MdlConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::MdlConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::MdlConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::MdlConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::MdlConfig>,
        keywords: &["markdown", "md", "linter", "ruby", "gem"],
        description: "Lint Markdown files using mdl (markdownlint)",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
