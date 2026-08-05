//! drawio generator — registered as a `SimpleGenerator` with a custom execute fn.

use std::process::Command;
use anyhow::{Context, Result};

use crate::config::StandardConfig;
use crate::graph::Product;
use crate::processors::{run_command, check_command_output, ensure_output_dir};

use crate::processors::{SimpleGenerator, SimpleGeneratorParams, DiscoverMode};

fn execute_drawio(ctx: &crate::build_context::BuildContext, config: &StandardConfig, product: &Product) -> Result<()> {
    let input = product.primary_input();
    let output = product.primary_output();
    let format = output.extension()
        .context("drawio output has no extension")?
        .to_string_lossy();
    ensure_output_dir(output)?;
    let command = config.require_command("drawio")?;
    let mut cmd = Command::new(command);
    cmd.arg("--export");
    cmd.arg("--format").arg(format.as_ref());
    cmd.arg("--output").arg(output);
    for arg in &config.args { cmd.arg(arg); }
    cmd.arg(input);
    let out = run_command(ctx, &cmd)?;
    check_command_output(&out, format_args!("drawio {}", input.display()))
}


fn create_drawio(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleGenerator::new(cfg, SimpleGeneratorParams { extra_tools: &[], extra_tools_fn: None, discover_mode: DiscoverMode::MultiFormat, execute_fn: execute_drawio, is_native: false })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "drawio", processor_type: crate::processors::ProcessorType::Generator, create: create_drawio,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".drawio"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { output_dir: "out/drawio", formats: &["png"], command: "drawio", ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["diagram", "drawio", "converter", "svg", "png", "generator"],
    description: "Export draw.io diagrams to images",
    is_native: false,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
