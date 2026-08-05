use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{StandardConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, SiblingFilter, run_command, anchor_display_dir, check_command_output};

#[derive(Debug, Deserialize, Serialize, Clone)]
/// Sphinx config. Custom: `working_dir`, `cache_output_dir`.
pub struct SphinxConfig {
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default = "crate::config::default_true")]
    pub cache_output_dir: bool,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for SphinxConfig {
    fn default() -> Self {
        Self {
            working_dir: None,
            cache_output_dir: true,
            standard: StandardConfig::default(),
        }
    }
}

pub struct SphinxProcessor {
    config: SphinxConfig,
}

impl SphinxProcessor {
    pub const fn new(config: SphinxConfig) -> Self {
        Self {
            config,
        }
    }

    /// Run sphinx-build from the project root.
    /// Source dir is the directory containing conf.py (e.g. "sphinx"),
    /// output dir is at project root level (e.g. "docs").
    fn execute_sphinx(&self, ctx: &crate::build_context::BuildContext, conf_py: &Path) -> Result<()> {
        let mut cmd = Command::new(&self.config.standard.command);
        let anchor_dir = crate::processors::parent_dir_or_empty(conf_py);
        // Source dir is the directory containing conf.py (e.g. "sphinx")
        if anchor_dir.as_os_str().is_empty() {
            cmd.arg(".");
        } else {
            cmd.arg(anchor_dir);
        }
        // Output dir at project root level (e.g. "docs")
        cmd.arg(&self.config.standard.output_dir);
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        if let Some(ref dir) = self.config.working_dir {
            cmd.current_dir(dir);
        }
        let output = run_command(ctx, &cmd)?;
        check_command_output(&output, format_args!("sphinx-build in {}", anchor_display_dir(conf_py)))
    }
}

impl Processor for SphinxProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean_output_dir(product, &product.processor, verbose)
    }

    fn required_tools(&self) -> Vec<String> {
        vec![self.config.standard.command.clone(), "python3".to_string()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let Some(files) = crate::processors::scan_or_skip(&self.config.standard, file_index) else {
            return Ok(());
        };
        let hash = Some(output_config_hash(&self.config, &crate::config::checksum_fields_of(instance_name)));
        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;
        let siblings = SiblingFilter {
            extensions: &[".rst", ".py", ".md"],
            excludes: &["/.git/", "/out/", "/.rsconstruct/", "/_build/", "/docs/"],
        };
        for anchor in files {
            let anchor_dir = anchor.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
            let sibling_files = file_index.query(
                &anchor_dir, siblings.extensions, siblings.excludes, &[], &[], &[],
            );
            let inputs = crate::processors::build_anchor_inputs(&anchor, &sibling_files, &extra);
            if self.config.cache_output_dir {
                // output_dir is at project root, NOT joined with anchor_dir
                let output_dir = PathBuf::from(&self.config.standard.output_dir);
                graph.add_product_with_output_dir(inputs, vec![], instance_name, hash.clone(), output_dir)?;
            } else {
                graph.add_product(inputs, vec![], instance_name, hash.clone())?;
            }
        }
        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.execute_sphinx(ctx, product.primary_input())
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SphinxProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "sphinx",
        processor_type: crate::processors::ProcessorType::Creator,
        create: plugin_create,
        fields: &[
            crate::config::FieldSpec { name: "working_dir", ty: crate::config::FieldType::String,
                affects_output: true, required: false,
                doc: "Working directory for sphinx-build (defaults to conf.py location)" },
            crate::config::FieldSpec { name: "cache_output_dir", ty: crate::config::FieldType::Bool,
                affects_output: false, required: false,
                doc: "Cache the entire output directory as a unit" },
        ],
        omit_standard_fields: &["formats", "dep_auto"],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &["conf.py"], src_exclude_dirs: &[] }),
        defaults: Some(crate::config::ProcessorDefaults { command: "sphinx-build", output_dir: "docs", ..crate::config::ProcessorDefaults::EMPTY }),
        defconfig_json: crate::registries::default_config_json::<SphinxConfig>,
        keywords: &["python", "sphinx", "documentation", "rst", "html", "pip"],
        description: "Build Sphinx documentation",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
