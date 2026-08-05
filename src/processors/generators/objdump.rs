//! objdump generator — registered as a `SimpleGenerator` with a custom execute fn.

use std::fs;
use std::process::Command;
use anyhow::{Context, Result};

use crate::config::StandardConfig;
use crate::graph::Product;
use crate::processors::{run_command_capture, check_command_output, ensure_output_dir};

use crate::processors::{SimpleGenerator, SimpleGeneratorParams, DiscoverMode};

fn execute_objdump(ctx: &crate::build_context::BuildContext, config: &StandardConfig, product: &Product) -> Result<()> {
    let input = product.primary_input();
    let output = product.primary_output();
    ensure_output_dir(output)?;
    let command = config.require_command("objdump")?;
    let mut cmd = Command::new(command);
    cmd.arg("--disassemble").arg("--source");
    for arg in &config.args { cmd.arg(arg); }
    cmd.arg(input);
    let out = run_command_capture(ctx, &cmd)?;
    check_command_output(&out, format_args!("objdump {}", input.display()))?;
    fs::write(output, &out.stdout)
        .with_context(|| format!("Failed to write objdump output: {}", output.display()))?;
    Ok(())
}


fn create_objdump(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleGenerator::new(cfg, SimpleGeneratorParams { extra_tools: &[], extra_tools_fn: None, discover_mode: DiscoverMode::SingleFormat("dis"), execute_fn: execute_objdump, is_native: false })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "objdump", processor_type: crate::processors::ProcessorType::Generator, create: create_objdump,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".elf"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { output_dir: "out/objdump", command: "objdump", ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["binary", "disassembler", "c", "cpp", "generator"],
    description: "Disassemble object files using objdump",
    is_native: false,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
