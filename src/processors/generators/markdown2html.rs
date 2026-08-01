//! markdown2html generator — registered as a `SimpleGenerator` with a custom execute fn.

use std::fs;
use std::process::Command;
use anyhow::{Context, Result};

use crate::config::StandardConfig;
use crate::graph::Product;
use crate::processors::{run_command_capture, check_command_output, ensure_output_dir};

use crate::processors::{SimpleGenerator, SimpleGeneratorParams, DiscoverMode};

fn execute_markdown2html(ctx: &crate::build_context::BuildContext, config: &StandardConfig, product: &Product) -> Result<()> {
    let input = product.primary_input();
    let output = product.primary_output();
    ensure_output_dir(output)?;
    let command = config.require_command("markdown2html")?;
    let mut cmd = Command::new(command);
    for arg in &config.args { cmd.arg(arg); }
    cmd.arg(input);
    let out = run_command_capture(ctx, &cmd)?;
    check_command_output(&out, format_args!("markdown {}", input.display()))?;
    fs::write(output, &out.stdout)
        .with_context(|| format!("Failed to write markdown output: {}", output.display()))?;
    Ok(())
}


fn create_markdown2html(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleGenerator::new(cfg, SimpleGeneratorParams { extra_tools: &["perl"], extra_tools_fn: None, discover_mode: DiscoverMode::SingleFormat("html"), execute_fn: execute_markdown2html, is_native: false })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "markdown2html", processor_type: crate::processors::ProcessorType::Generator, create: create_markdown2html,
    known_fields: crate::registries::typed_known_fields::<crate::config::StandardConfig>,
    checksum_fields: crate::registries::typed_checksum_fields::<crate::config::StandardConfig>,
    must_fields: crate::registries::typed_must_fields::<crate::config::StandardConfig>,
    field_descriptions: crate::registries::typed_field_descriptions::<crate::config::StandardConfig>,
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["markdown", "md", "html", "converter", "generator"],
    description: "Convert Markdown files to HTML",
    is_native: false,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
