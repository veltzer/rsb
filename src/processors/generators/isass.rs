//! isass generator — registered as a `SimpleGenerator` with a custom execute fn.

use std::fs;
use anyhow::{Context, Result};

use crate::config::StandardConfig;
use crate::graph::Product;
use crate::processors::ensure_output_dir;

use crate::processors::{SimpleGenerator, SimpleGeneratorParams, DiscoverMode};

fn execute_isass(_ctx: &crate::build_context::BuildContext, _config: &StandardConfig, product: &Product) -> Result<()> {
    let input = product.primary_input();
    let output = product.primary_output();
    ensure_output_dir(output)?;
    let css = grass::from_path(input, &grass::Options::default())
        .map_err(|e| anyhow::anyhow!("Failed to compile {}: {}", input.display(), e))?;
    fs::write(output, &css)
        .with_context(|| format!("Failed to write {}", output.display()))?;
    Ok(())
}


fn create_isass(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleGenerator::new(cfg, SimpleGeneratorParams { extra_tools: &[], extra_tools_fn: None, discover_mode: DiscoverMode::SingleFormat("css"), execute_fn: execute_isass, is_native: true })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "isass", processor_type: crate::processors::ProcessorType::Generator, create: create_isass,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".scss", ".sass"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { output_dir: "out/isass", ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["sass", "scss", "css", "converter", "web", "frontend"],
    description: "Compile Sass/SCSS to CSS (in-process)",
    is_native: true,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
