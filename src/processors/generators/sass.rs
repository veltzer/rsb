//! sass generator — registered as a `SimpleGenerator` with a custom execute fn.

use std::process::Command;
use anyhow::Result;

use crate::config::StandardConfig;
use crate::graph::Product;
use crate::processors::{run_command, check_command_output, ensure_output_dir};

use crate::processors::{SimpleGenerator, SimpleGeneratorParams, DiscoverMode};

fn execute_sass(ctx: &crate::build_context::BuildContext, config: &StandardConfig, product: &Product) -> Result<()> {
    let input = product.primary_input();
    let output = product.primary_output();
    ensure_output_dir(output)?;
    let command = config.require_command("sass")?;
    let mut cmd = Command::new(command);
    for arg in &config.args { cmd.arg(arg); }
    cmd.arg(input).arg(output);
    let out = run_command(ctx, &cmd)?;
    check_command_output(&out, format_args!("sass {}", input.display()))
}


fn create_sass(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleGenerator::new(cfg, SimpleGeneratorParams { extra_tools: &[], extra_tools_fn: None, discover_mode: DiscoverMode::SingleFormat("css"), execute_fn: execute_sass, is_native: false })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "sass", processor_type: crate::processors::ProcessorType::Generator, create: create_sass,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".scss", ".sass"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { output_dir: "out/sass", command: "sass", ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["sass", "scss", "css", "converter", "web", "frontend"],
    description: "Compile Sass/SCSS to CSS",
    is_native: false,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
