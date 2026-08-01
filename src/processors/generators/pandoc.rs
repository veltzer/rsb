//! pandoc generator with optional --pdf-engine support.
//!
//! Honors `pdf_engine` from `[processor.pandoc]`. When set and the output
//! format is `pdf`, the engine is forwarded to pandoc as `--pdf-engine=<name>`.
//! For non-pdf outputs the field is silently ignored. Empty string keeps
//! pandoc's default (pdflatex).

use anyhow::{Context, Result, bail};
use std::process::Command;

use crate::config::{PandocConfig, PANDOC_PDF_ENGINES};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, run_command, check_command_output, ensure_output_dir};

use super::DiscoverParams;

pub struct PandocProcessor {
    config: PandocConfig,
}

impl PandocProcessor {
    pub const fn new(config: PandocConfig) -> Self {
        Self { config }
    }
}

fn validate_pdf_engine(engine: &str) -> Result<()> {
    if !engine.is_empty() && !PANDOC_PDF_ENGINES.contains(&engine) {
        bail!(
            "[processor.pandoc] pdf_engine = \"{}\" is not recognized. Valid values: {}",
            engine,
            PANDOC_PDF_ENGINES.join(", "),
        );
    }
    Ok(())
}

impl Processor for PandocProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    fn clean(&self, product: &Product, verbose: bool) -> Result<usize> {
        crate::processors::ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn required_tools(&self) -> Vec<String> {
        let mut tools = vec![self.config.standard.command.clone()];
        if !self.config.pdf_engine.is_empty() {
            tools.push(self.config.pdf_engine.clone());
        }
        tools
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let params = DiscoverParams {
            scan: &self.config.standard,
            dep_inputs: &self.config.standard.dep_inputs,
            config: &self.config,
            output_dir: &self.config.standard.output_dir,
            processor_name: instance_name,
            checksum_fields: <crate::config::PandocConfig as crate::config::KnownFields>::checksum_fields(),
        };
        super::discover_multi_format(graph, file_index, &params, &self.config.standard.formats)
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let input = product.primary_input();
        let output = product.primary_output();
        let format = output.extension()
            .context("pandoc output has no extension")?
            .to_string_lossy();
        ensure_output_dir(output)?;

        let mut cmd = Command::new(&self.config.standard.command);
        cmd.env("SOURCE_DATE_EPOCH", "0");
        cmd.arg("--to").arg(format.as_ref());
        if format.as_ref() == "pdf" {
            // \pdftrailerid is a pdflatex primitive and would be undefined
            // under xelatex/lualatex. Only emit it when the engine is the
            // pdflatex default (empty string) or explicitly pdflatex.
            let engine = self.config.pdf_engine.as_str();
            if engine.is_empty() || engine == "pdflatex" {
                cmd.arg("-V").arg(r"header-includes=\pdftrailerid{}");
            }
            if !engine.is_empty() {
                cmd.arg(format!("--pdf-engine={engine}"));
            }
        }
        for arg in &self.config.standard.args { cmd.arg(arg); }
        cmd.arg(input);
        cmd.arg("-o").arg(output);

        let out = run_command(ctx, &cmd)?;
        check_command_output(&out, format_args!("pandoc {}", input.display()))
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    let cfg: PandocConfig = ::toml::from_str(&::toml::to_string(toml)?)?;
    validate_pdf_engine(&cfg.pdf_engine)?;
    Ok(Box::new(PandocProcessor::new(cfg)))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "pandoc", processor_type: crate::processors::ProcessorType::Generator, create: plugin_create,
    known_fields: crate::registries::typed_known_fields::<crate::config::PandocConfig>,
    checksum_fields: crate::registries::typed_checksum_fields::<crate::config::PandocConfig>,
    must_fields: crate::registries::typed_must_fields::<crate::config::PandocConfig>,
    field_descriptions: crate::registries::typed_field_descriptions::<crate::config::PandocConfig>,
    defconfig_json: crate::registries::default_config_json::<crate::config::PandocConfig>,
    keywords: &["markdown", "converter", "pdf", "html", "docx", "generator"],
    description: "Convert documents using pandoc",
    is_native: false,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
