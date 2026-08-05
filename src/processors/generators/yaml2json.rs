//! yaml2json generator — registered as a `SimpleGenerator` with a custom execute fn.

use std::fs;
use anyhow::{Context, Result};

use crate::config::StandardConfig;
use crate::graph::Product;
use crate::processors::ensure_output_dir;

use crate::processors::{SimpleGenerator, SimpleGeneratorParams, DiscoverMode};

fn execute_yaml2json(_ctx: &crate::build_context::BuildContext, _config: &StandardConfig, product: &Product) -> Result<()> {
    let input = product.primary_input();
    let output = product.primary_output();
    ensure_output_dir(output)?;
    let contents = fs::read_to_string(input)
        .with_context(|| format!("Failed to read {}", input.display()))?;
    let value: serde_json::Value = serde_yml::from_str(&contents)
        .with_context(|| format!("Failed to parse YAML from {}", input.display()))?;
    let json = serde_json::to_string_pretty(&value)
        .with_context(|| format!("Failed to serialize JSON for {}", input.display()))?;
    fs::write(output, json)
        .with_context(|| format!("Failed to write {}", output.display()))?;
    Ok(())
}


// --- Plugin registrations ---

// --- Plugin registrations ---


fn create_yaml2json(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleGenerator::new(cfg, SimpleGeneratorParams { extra_tools: &[], extra_tools_fn: None, discover_mode: DiscoverMode::SingleFormat("json"), execute_fn: execute_yaml2json, is_native: true })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "yaml2json", processor_type: crate::processors::ProcessorType::Generator, create: create_yaml2json,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".yml", ".yaml"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { output_dir: "out/yaml2json", ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["yaml", "json", "converter", "yml", "generator"],
    description: "Convert YAML files to JSON (in-process)",
    is_native: true,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
