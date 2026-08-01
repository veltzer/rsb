use anyhow::Result;
use std::path::Path;
use std::process::Command;

use crate::config::MdbookConfig;
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, SiblingFilter, DirectoryProductOpts, discover_directory_products, run_in_anchor_dir, anchor_display_dir, check_command_output};

pub struct MdbookProcessor {
    config: MdbookConfig,
}

impl MdbookProcessor {
    pub const fn new(config: MdbookConfig) -> Self {
        Self {
            config,
        }
    }

    /// Run mdbook build in the book.toml's directory
    fn execute_mdbook(&self, ctx: &crate::build_context::BuildContext, book_toml: &Path) -> Result<()> {
        let mut cmd = Command::new(&self.config.standard.command);
        cmd.arg("build");
        cmd.arg(".");
        for arg in &self.config.standard.args {
            cmd.arg(arg);
        }
        let output = run_in_anchor_dir(ctx, &mut cmd, book_toml)?;
        check_command_output(&output, format_args!("mdbook build in {}", anchor_display_dir(book_toml)))
    }
}

impl Processor for MdbookProcessor {
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
        vec![self.config.standard.command.clone()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        discover_directory_products(graph, DirectoryProductOpts {
            scan: &self.config.standard,
            file_index,
            dep_inputs: &self.config.standard.dep_inputs,
            cfg_hash: &self.config,
            checksum_fields: <crate::config::MdbookConfig as crate::config::KnownFields>::checksum_fields(),
            siblings: &SiblingFilter {
                extensions: &[".md", ".toml"],
                excludes: &["/.git/", "/out/", "/.rsconstruct/", "/book/"],
            },
            processor_name: instance_name,
            output_dir_name: if self.config.cache_output_dir {
                Some(self.config.standard.output_dir.as_str())
            } else {
                None
            },
        })
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        self.execute_mdbook(ctx, product.primary_input())
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(MdbookProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "mdbook",
        processor_type: crate::processors::ProcessorType::Creator,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::MdbookConfig>,
        known_fields: crate::registries::typed_known_fields::<crate::config::MdbookConfig>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::MdbookConfig>,
        must_fields: crate::registries::typed_must_fields::<crate::config::MdbookConfig>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::MdbookConfig>,
        keywords: &["markdown", "md", "rust", "documentation", "book", "html"],
        description: "Build mdbook documentation",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: Some(1),
    }
}
