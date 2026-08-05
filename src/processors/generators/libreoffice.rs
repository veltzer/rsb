//! libreoffice generator — registered as a `SimpleGenerator` with a custom execute fn.

use std::fs;
use std::process::Command;
use anyhow::{Context, Result};

use crate::config::StandardConfig;
use crate::graph::Product;
use crate::processors::{run_command, check_command_output};

use crate::processors::{SimpleGenerator, SimpleGeneratorParams, DiscoverMode};

fn execute_libreoffice(ctx: &crate::build_context::BuildContext, config: &StandardConfig, product: &Product) -> Result<()> {
    let input = product.primary_input();
    let output = product.primary_output();
    let format = output.extension()
        .context("libreoffice output has no extension")?
        .to_string_lossy();
    let output_dir = output.parent()
        .context("libreoffice output has no parent directory")?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("Failed to create libreoffice output directory: {}", output_dir.display()))?;
    let command = config.require_command("libreoffice")?;
    // LibreOffice can't run concurrent headless conversions against one user
    // profile, so serialize per user: a shared machine-global lock file would
    // belong to whichever user created it first and fail flock for everyone
    // else. temp_dir() also honors TMPDIR.
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let lock_file = std::env::temp_dir().join(format!("rsconstruct_libreoffice_{user}"));
    let mut cmd = Command::new("flock");
    cmd.arg(&lock_file);
    cmd.arg(command);
    cmd.arg("--headless");
    cmd.arg("--convert-to").arg(format.as_ref());
    cmd.arg("--outdir").arg(output_dir);
    for arg in &config.args { cmd.arg(arg); }
    cmd.arg(input);
    let out = run_command(ctx, &cmd)?;
    check_command_output(&out, format_args!("libreoffice {}", input.display()))
}

fn create_libreoffice(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleGenerator::new(cfg, SimpleGeneratorParams { extra_tools: &["flock"], extra_tools_fn: None, discover_mode: DiscoverMode::MultiFormat, execute_fn: execute_libreoffice, is_native: false })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "libreoffice", processor_type: crate::processors::ProcessorType::Generator, create: create_libreoffice,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".odp"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { output_dir: "out/libreoffice", formats: &["pdf"], command: "libreoffice", ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["document", "converter", "pdf", "docx", "odt", "generator"],
    description: "Convert documents using LibreOffice",
    is_native: false,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
