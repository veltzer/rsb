use anyhow::Result;
use std::path::Path;

use crate::config::{ScriptConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, config_file_inputs, run_checker, execute_checker_batch};

pub struct ScriptProcessor {
    config: ScriptConfig,
}

impl ScriptProcessor {
    pub const fn new(config: ScriptConfig) -> Self {
        Self {
            config,
        }
    }

    fn check_files(&self, ctx: &crate::build_context::BuildContext, files: &[&Path]) -> Result<()> {
        let command = self.config.standard.require_command(crate::processors::names::SCRIPT)?;
        run_checker(ctx, command, None, &self.config.standard.args, files, ctx.max_arg_len())
    }

    const fn has_fix(&self) -> bool {
        !self.config.fix_command.is_empty()
    }

    fn fix_files(&self, ctx: &crate::build_context::BuildContext, files: &[&Path]) -> Result<()> {
        run_checker(ctx, &self.config.fix_command, None, &self.config.fix_args, files, ctx.max_arg_len())
    }
}

impl Processor for ScriptProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn required_tools(&self) -> Vec<String> {
        if self.config.standard.command.is_empty() {
            Vec::new()
        } else {
            vec![self.config.standard.command.clone()]
        }
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        if self.config.standard.command.is_empty() {
            return Ok(());
        }
        let files = file_index.scan(&self.config.standard, true);
        if files.is_empty() {
            return Ok(());
        }
        let hash = Some(output_config_hash(&self.config, <crate::config::ScriptConfig as crate::config::KnownFields>::checksum_fields()));
        let mut dep_inputs = self.config.standard.dep_inputs.clone();
        for ai in &self.config.standard.dep_auto {
            dep_inputs.extend(config_file_inputs(ai));
        }
        // If the command is a local file, depend on its contents
        dep_inputs.extend(config_file_inputs(&self.config.standard.command));
        let extra = resolve_extra_inputs(&dep_inputs)?;
        for file in files {
            let mut inputs = Vec::with_capacity(1 + extra.len());
            inputs.push(file);
            inputs.extend_from_slice(&extra);
            graph.add_product(inputs, vec![], instance_name, hash.clone())?;
        }
        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.check_files(ctx, &[product.primary_input()])
    }

    fn execute_batch(&self, ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        execute_checker_batch(ctx, products, |ctx, files| self.check_files(ctx, files))
    }

    fn config_has_fix(&self) -> bool {
        !self.config.fix_command.is_empty()
    }

    fn fix(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.fix_files(ctx, &[product.primary_input()])
    }

    fn supports_fix_batch(&self) -> bool {
        self.has_fix() && self.config.fix_batch.unwrap_or(self.config.standard.batch)
    }

    fn fix_batch(&self, ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        execute_checker_batch(ctx, products, |ctx, files| self.fix_files(ctx, files))
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(ScriptProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "script",
        processor_type: crate::processors::ProcessorType::Checker,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::ScriptConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::ScriptConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::ScriptConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::ScriptConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::ScriptConfig>,
        keywords: &["shell", "script", "checker", "sh", "bash"],
        description: "Run a user-configured script as a checker",
        is_native: false,
        can_fix: false,
        supports_batch: true,
        max_jobs_cap: None,
    }
}
