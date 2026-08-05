use anyhow::Result;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::StandardConfig;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{
    Processor,
    run_command, check_command_output, execute_generator_batch,
    config_file_inputs,
};
use crate::config::{output_config_hash, resolve_extra_inputs};

fn default_generator_output_extension() -> String {
    "out".into()
}

/// Generator config. Custom: `output_extension`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GeneratorConfig {
    #[serde(default = "default_generator_output_extension")]
    pub output_extension: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for GeneratorConfig {
    fn default() -> Self {
        Self {
            output_extension: "out".into(),
            standard: StandardConfig::default(),
        }
    }
}

pub struct GeneratorProcessor {
    config: GeneratorConfig,
}

impl GeneratorProcessor {
    pub const fn new(config: GeneratorConfig) -> Self {
        Self {
            config,
        }
    }

    const fn should_process(&self) -> bool {
        !self.config.standard.command.is_empty()
    }

    fn execute_product(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.run_pairs(ctx, &[(product.primary_input(), product.primary_output())])
    }

    fn run_pairs(&self, ctx: &crate::build_context::BuildContext, pairs: &[(&Path, &Path)]) -> Result<()> {
        for pair in pairs {
            crate::processors::ensure_output_dir(pair.1)?;
        }

        let command = self.config.standard.require_command(crate::processors::names::GENERATOR)?;
        let mut cmd = Command::new(command);
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        for (input, output) in pairs {
            cmd.arg(input);
            cmd.arg(output);
        }

        let out = run_command(ctx, &cmd)?;
        check_command_output(&out, format_args!("{} ({} file(s))", command, pairs.len()))
    }
}

impl Processor for GeneratorProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        self.should_process() && !file_index.scan(&self.config.standard, true).is_empty()
    }

    fn required_tools(&self) -> Vec<String> {
        if self.config.standard.command.is_empty() {
            Vec::new()
        } else {
            vec![self.config.standard.command.clone()]
        }
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        if !self.should_process() {
            return Ok(());
        }
        let files = file_index.scan(&self.config.standard, true);
        if files.is_empty() {
            return Ok(());
        }

        let hash = Some(output_config_hash(&self.config, &crate::config::checksum_fields_of(instance_name)));
        let mut dep_inputs = self.config.standard.dep_inputs.clone();
        for ai in &self.config.standard.dep_auto {
            dep_inputs.extend(config_file_inputs(ai));
        }
        // If the command is a local file, depend on its contents
        let command = &self.config.standard.command;
        dep_inputs.extend(config_file_inputs(command));
        let extra = resolve_extra_inputs(&dep_inputs)?;
        let src_dirs = self.config.standard.src_dirs();

        for source in &files {
            let output = super::output_path(
                source, src_dirs, &self.config.standard.output_dir, &self.config.output_extension,
            );
            let mut inputs = Vec::with_capacity(1 + extra.len());
            inputs.push(source.clone());
            inputs.extend_from_slice(&extra);
            graph.add_product(inputs, vec![output], instance_name, hash.clone())?;
        }
        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.execute_product(ctx, product)
    }

    fn execute_batch(&self, ctx: &crate::build_context::BuildContext, products: &[&Product]) -> Vec<Result<()>> {
        execute_generator_batch(ctx, products, |ctx, pairs| self.run_pairs(ctx, pairs))
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(GeneratorProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "generator",
        processor_type: crate::processors::ProcessorType::Generator,
        create: plugin_create,
        fields: &[
            crate::config::FieldSpec { name: "output_extension", ty: crate::config::FieldType::String,
                affects_output: true, required: false,
                doc: "File extension for generated output files" },
        ],
        omit_standard_fields: &["formats"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[], src_exclude_dirs: &[] }),
        defaults: Some(crate::config::ProcessorDefaults { output_dir: "out/generator", ..crate::config::ProcessorDefaults::EMPTY }),
        defconfig_json: crate::registries::default_config_json::<GeneratorConfig>,
        keywords: &["generator", "generic"],
        description: "Run a user-configured script as a generator",
        is_native: false,
        can_fix: false,
        supports_batch: true,
        max_jobs_cap: None,
    }
}
