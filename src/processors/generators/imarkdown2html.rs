//! imarkdown2html generator — registered as a `SimpleGenerator` with a custom execute fn.

use std::fs;
use anyhow::{Context, Result};

use crate::config::StandardConfig;
use crate::graph::Product;
use crate::processors::ensure_output_dir;

use crate::processors::{SimpleGenerator, SimpleGeneratorParams, DiscoverMode};

fn execute_imarkdown2html(_ctx: &crate::build_context::BuildContext, _config: &StandardConfig, product: &Product) -> Result<()> {
    let input = product.primary_input();
    let output = product.primary_output();
    ensure_output_dir(output)?;
    let contents = fs::read_to_string(input)
        .with_context(|| format!("Failed to read {}", input.display()))?;
    let parser = pulldown_cmark::Parser::new(&contents);
    let mut html_output = String::new();
    pulldown_cmark::html::push_html(&mut html_output, parser);
    fs::write(output, &html_output)
        .with_context(|| format!("Failed to write {}", output.display()))?;
    Ok(())
}


fn create_imarkdown2html(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleGenerator::new(cfg, SimpleGeneratorParams { extra_tools: &[], extra_tools_fn: None, discover_mode: DiscoverMode::SingleFormat("html"), execute_fn: execute_imarkdown2html, is_native: true })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "imarkdown2html", processor_type: crate::processors::ProcessorType::Generator, create: create_imarkdown2html,
    known_fields: crate::registries::typed_known_fields::<crate::config::StandardConfig>,
    checksum_fields: crate::registries::typed_checksum_fields::<crate::config::StandardConfig>,
    must_fields: crate::registries::typed_must_fields::<crate::config::StandardConfig>,
    field_descriptions: crate::registries::typed_field_descriptions::<crate::config::StandardConfig>,
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["markdown", "md", "html", "converter", "generator"],
    description: "Convert Markdown to HTML (in-process)",
    is_native: true,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
