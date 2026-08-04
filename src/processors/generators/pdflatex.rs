use anyhow::{Context, Result};
use std::fs;
use std::process::Command;

use crate::config::PdflatexConfig;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, run_command, check_command_output};

use super::DiscoverParams;

/// Temp file extensions produced by pdflatex that should be cleaned between runs.
pub struct PdflatexProcessor {
    config: PdflatexConfig,
}

impl PdflatexProcessor {
    pub const fn new(config: PdflatexConfig) -> Self {
        Self {
            config,
        }
    }
}

impl Processor for PdflatexProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }

    // Serialize the FULL config (the trait default covers StandardConfig
    // only), so the extra fields reach config-change detection.
    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn required_tools(&self) -> Vec<String> {
        let mut tools = vec![self.config.standard.command.clone()];
        if self.config.qpdf {
            tools.push("qpdf".to_string());
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
            checksum_fields: <crate::config::PdflatexConfig as crate::config::KnownFields>::checksum_fields(),
        };
        super::discover_single_format(graph, file_index, &params, "pdf")
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let input = product.primary_input();
        let final_output = product.primary_output();

        // pdflatex writes output next to the input or in -output-directory
        // We use a temp directory for the build, then move the PDF to the final output location.
        let input_stem = input.file_stem()
            .context("pdflatex input has no file stem")?
            .to_string_lossy();

        // Ensure output directory exists
        crate::processors::ensure_output_dir(final_output)?;

        let build_dir = crate::processors::parent_dir(final_output);

        // Per-product scratch directory. pdflatex's aux/toc/log files are
        // keyed by stem only, so two same-stem sources sharing an output
        // parent would corrupt each other's temp files under -j (silently
        // producing PDFs with broken cross-references) — the same hazard
        // marp solved with a per-invocation namespace. The scratch dir also
        // makes cleanup a single remove_dir_all instead of a by-extension
        // guess list.
        let scratch = build_dir.join(format!(".pdflatex-{}",
            &crate::checksum::bytes_checksum(input.display().to_string().as_bytes())[..12]));
        fs::create_dir_all(&scratch)
            .with_context(|| format!("Failed to create pdflatex scratch dir: {}", scratch.display()))?;

        // Run pdflatex N times into the scratch dir. Nothing is cleaned
        // between runs: the .aux/.toc written by run N is exactly what run
        // N+1 consumes to resolve \ref and the table of contents — an
        // earlier version deleted them between runs, which made multi-pass
        // (the default, runs = 2) behave like a single cold run. Floor at 1:
        // runs = 0 would skip pdflatex entirely and record a "successful"
        // product with no output.
        for run in 0..self.config.runs.max(1) {
            let mut cmd = Command::new(&self.config.standard.command);
            if self.config.shell_escape {
                cmd.arg("-shell-escape");
            }
            cmd.arg("-interaction=nonstopmode");
            cmd.arg("-halt-on-error");
            cmd.arg(format!("-output-directory={}", scratch.display()));
            for arg in &self.config.standard.args {
                cmd.arg(arg);
            }
            cmd.arg(input);

            let out = run_command(ctx, &cmd)?;
            check_command_output(&out, format_args!("pdflatex run {} of {}", run + 1, input.display()))?;
        }

        let pdf_in_scratch = scratch.join(format!("{input_stem}.pdf"));

        // Optional qpdf post-processing
        if self.config.qpdf {
            let qpdf_tmp = scratch.join(format!("{input_stem}.qpdf.pdf"));

            let mut cmd = Command::new("qpdf");
            cmd.arg("--deterministic-id");
            cmd.arg("--linearize");
            cmd.arg(&pdf_in_scratch);
            cmd.arg(&qpdf_tmp);

            let out = run_command(ctx, &cmd)?;
            check_command_output(&out, format_args!("qpdf {}", pdf_in_scratch.display()))?;

            // Replace original with linearized version
            fs::rename(&qpdf_tmp, &pdf_in_scratch)
                .with_context(|| format!("Failed to rename qpdf output: {}", qpdf_tmp.display()))?;
        }

        // Move the finished PDF into place (same filesystem: scratch lives
        // under the output parent), then drop the scratch dir wholesale.
        fs::rename(&pdf_in_scratch, final_output)
            .with_context(|| format!("Failed to move pdflatex output into place: {}", final_output.display()))?;
        fs::remove_dir_all(&scratch)
            .with_context(|| format!("Failed to remove pdflatex scratch dir: {}", scratch.display()))?;

        Ok(())
    }

}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(PdflatexProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "pdflatex",
        processor_type: crate::processors::ProcessorType::Generator,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::PdflatexConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::PdflatexConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::PdflatexConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::PdflatexConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::PdflatexConfig>,
        keywords: &["latex", "tex", "pdf", "generator", "typesetting"],
        description: "Compile LaTeX documents using pdflatex",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
