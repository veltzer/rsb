//! chromium generator — registered as a `SimpleGenerator` with a custom execute fn.

use std::fs;
use std::process::Command;
use anyhow::{Context, Result};

use crate::config::StandardConfig;
use crate::graph::Product;
use crate::processors::{run_command, check_command_output, ensure_output_dir};

use crate::processors::{SimpleGenerator, SimpleGeneratorParams, DiscoverMode};

fn execute_chromium(ctx: &crate::build_context::BuildContext, config: &StandardConfig, product: &Product) -> Result<()> {
    let input = product.primary_input();
    let output = product.primary_output();
    ensure_output_dir(output)?;
    let abs_input = fs::canonicalize(input)
        .with_context(|| format!("Failed to resolve absolute path for: {}", input.display()))?;
    let input_url = format!("file://{}", abs_input.display());
    let command = config.require_command("chromium")?;
    let mut cmd = Command::new(command);
    cmd.arg("--headless");
    cmd.arg("--disable-gpu");
    cmd.arg("--no-sandbox");
    cmd.arg(format!("--print-to-pdf={}", output.display()));
    for arg in &config.args { cmd.arg(arg); }
    cmd.arg(&input_url);
    let out = run_command(ctx, &cmd)?;
    check_command_output(&out, format_args!("chromium {}", input.display()))
}


fn create_chromium(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleGenerator::new(cfg, SimpleGeneratorParams { extra_tools: &[], extra_tools_fn: None, discover_mode: DiscoverMode::SingleFormat("pdf"), execute_fn: execute_chromium, is_native: false })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "chromium", processor_type: crate::processors::ProcessorType::Generator, create: create_chromium,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".html"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { output_dir: "out/chromium", command: "google-chrome", ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["html", "pdf", "converter", "browser", "web"],
    description: "Convert files to PDF using Chromium",
    is_native: false,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
