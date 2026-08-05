//! pandoc generator with optional --pdf-engine support.
//!
//! Honors `pdf_engine` from `[processor.pandoc]`. When set and the output
//! format is `pdf`, the engine is forwarded to pandoc as `--pdf-engine=<name>`.
//! For non-pdf outputs the field is silently ignored. Empty string keeps
//! pandoc's default (pdflatex).
//!
//! Registered as a plain `SimpleGenerator` over `PandocConfig`: the extra
//! config field is reachable from `execute_fn` and `extra_tools_fn`, so none
//! of the `Processor` boilerplate needs restating here.

use anyhow::{Context, Result, bail};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::StandardConfig;
use crate::graph::Product;
use crate::processors::{
    DiscoverMode, SimpleGenerator, SimpleGeneratorParams,
    run_command, check_command_output, ensure_output_dir,
};

/// Engines accepted for `pandoc.pdf_engine`. Empty string means "use pandoc's default".
pub const PANDOC_PDF_ENGINES: &[&str] = &["pdflatex", "xelatex", "lualatex", "tectonic", "wkhtmltopdf", "weasyprint", "prince", "context"];

/// Pandoc processor config. Custom field: `pdf_engine` (forwarded to --pdf-engine
/// when format == pdf). Empty string keeps pandoc's default engine.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct PandocConfig {
    #[serde(default)]
    pub pdf_engine: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}
impl AsRef<StandardConfig> for PandocConfig {
    fn as_ref(&self) -> &StandardConfig {
        &self.standard
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

/// The configured PDF engine is a tool this processor shells out to, so it
/// has to be declared alongside `command` rather than discovered at run time.
fn pdf_engine_tools(config: &PandocConfig) -> Vec<String> {
    if config.pdf_engine.is_empty() {
        Vec::new()
    } else {
        vec![config.pdf_engine.clone()]
    }
}

fn execute_pandoc(
    ctx: &crate::build_context::BuildContext,
    config: &PandocConfig,
    product: &Product,
) -> Result<()> {
    let input = product.primary_input();
    let output = product.primary_output();
    let format = output.extension()
        .context("pandoc output has no extension")?
        .to_string_lossy();
    ensure_output_dir(output)?;

    let mut cmd = Command::new(&config.standard.command);
    cmd.env("SOURCE_DATE_EPOCH", "0");
    cmd.arg("--to").arg(format.as_ref());
    if format.as_ref() == "pdf" {
        // \pdftrailerid is a pdflatex primitive and would be undefined
        // under xelatex/lualatex. Only emit it when the engine is the
        // pdflatex default (empty string) or explicitly pdflatex.
        let engine = config.pdf_engine.as_str();
        if engine.is_empty() || engine == "pdflatex" {
            cmd.arg("-V").arg(r"header-includes=\pdftrailerid{}");
        }
        if !engine.is_empty() {
            cmd.arg(format!("--pdf-engine={engine}"));
        }
    }
    for arg in &config.standard.args { cmd.arg(arg); }
    cmd.arg(input);
    cmd.arg("-o").arg(output);

    let out = run_command(ctx, &cmd)?;
    check_command_output(&out, format_args!("pandoc {}", input.display()))
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    let cfg: PandocConfig = ::toml::from_str(&::toml::to_string(toml)?)?;
    validate_pdf_engine(&cfg.pdf_engine)?;
    Ok(Box::new(SimpleGenerator::new(cfg, SimpleGeneratorParams {
        extra_tools: &[],
        extra_tools_fn: Some(pdf_engine_tools),
        discover_mode: DiscoverMode::MultiFormat,
        execute_fn: execute_pandoc,
        is_native: false,
    })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "pandoc", processor_type: crate::processors::ProcessorType::Generator, create: plugin_create,
    fields: &[
        crate::config::FieldSpec { name: "pdf_engine", ty: crate::config::FieldType::String,
            affects_output: true, required: false,
            doc: "PDF engine for --pdf-engine (e.g. xelatex, lualatex). Empty = pandoc default (pdflatex)." },
    ],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { output_dir: "out/pandoc", formats: &["pdf", "html", "docx"], command: "pandoc", ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<PandocConfig>,
    keywords: &["markdown", "converter", "pdf", "html", "docx", "generator"],
    description: "Convert documents using pandoc",
    is_native: false,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
