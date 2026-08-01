//! protobuf generator — registered as a `SimpleGenerator` with a custom execute fn.

use std::process::Command;
use anyhow::Result;

use crate::config::StandardConfig;
use crate::graph::Product;
use crate::processors::{run_command, check_command_output, ensure_output_dir};

use crate::processors::{SimpleGenerator, SimpleGeneratorParams, DiscoverMode};

fn execute_protobuf(ctx: &crate::build_context::BuildContext, config: &StandardConfig, product: &Product) -> Result<()> {
    let input = product.primary_input();
    let output = product.primary_output();
    let output_dir = output.parent().unwrap_or(std::path::Path::new("."));
    ensure_output_dir(output)?;
    let command = config.require_command("protobuf")?;
    let mut cmd = Command::new(command);
    if let Some(parent) = input.parent() {
        cmd.arg(format!("--proto_path={}", parent.display()));
    }
    cmd.arg(format!("--cpp_out={}", output_dir.display()));
    for arg in &config.args { cmd.arg(arg); }
    cmd.arg(input);
    let out = run_command(ctx, &cmd)?;
    check_command_output(&out, format_args!("protoc {}", input.display()))
}


fn create_protobuf(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleGenerator::new(cfg, SimpleGeneratorParams { extra_tools: &[], discover_mode: DiscoverMode::SingleFormat("pb.cc"), execute_fn: execute_protobuf, is_native: false })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "protobuf", processor_type: crate::processors::ProcessorType::Generator, create: create_protobuf,
    known_fields: crate::registries::typed_known_fields::<crate::config::StandardConfig>,
    checksum_fields: crate::registries::typed_checksum_fields::<crate::config::StandardConfig>,
    must_fields: crate::registries::typed_must_fields::<crate::config::StandardConfig>,
    field_descriptions: crate::registries::typed_field_descriptions::<crate::config::StandardConfig>,
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["protobuf", "proto", "generator", "grpc", "serialization"],
    description: "Compile Protocol Buffer definitions",
    is_native: false,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
