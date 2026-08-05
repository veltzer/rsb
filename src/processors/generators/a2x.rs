//! a2x generator — registered as a `SimpleGenerator` with a custom execute fn.

use std::fs;
use std::process::Command;
use anyhow::{Context, Result};

use crate::config::StandardConfig;
use crate::graph::Product;
use crate::processors::{run_command, check_command_output, ensure_output_dir};

use crate::processors::{SimpleGenerator, SimpleGeneratorParams, DiscoverMode};

fn execute_a2x(ctx: &crate::build_context::BuildContext, config: &StandardConfig, product: &Product) -> Result<()> {
    let input = product.primary_input();
    let output = product.primary_output();
    ensure_output_dir(output)?;
    let command = config.require_command("a2x")?;
    let mut cmd = Command::new(command);
    for arg in &config.args { cmd.arg(arg); }
    cmd.arg(input);
    let out = run_command(ctx, &cmd)?;
    check_command_output(&out, format_args!("a2x {}", input.display()))?;
    // a2x generates the PDF next to the input file — move it to the output path
    let stem = input.file_stem()
        .context("a2x input has no file stem")?;
    let generated = input.with_file_name(format!("{}.pdf", stem.to_string_lossy()));
    if generated != *output {
        // a2x exiting 0 without producing the PDF must fail here, not later
        // as a confusing cache-store error about a missing output.
        if !generated.exists() {
            anyhow::bail!(
                "a2x reported success but did not produce {}",
                generated.display()
            );
        }
        fs::rename(&generated, output)
            .with_context(|| format!("Failed to move a2x output from {} to {}", generated.display(), output.display()))?;
    }
    Ok(())
}


fn create_a2x(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleGenerator::new(cfg, SimpleGeneratorParams { extra_tools: &["python3"], extra_tools_fn: None, discover_mode: DiscoverMode::SingleFormat("pdf"), execute_fn: execute_a2x, is_native: false })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "a2x", processor_type: crate::processors::ProcessorType::Generator, create: create_a2x,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".txt"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { output_dir: "out/a2x", command: "a2x", ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["asciidoc", "converter", "generator", "documentation", "html", "pdf"],
    description: "Convert AsciiDoc files to PDF",
    is_native: false,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
