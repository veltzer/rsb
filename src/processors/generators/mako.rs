use anyhow::Result;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::config::{StandardConfig, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, run_command, check_command_output};

use super::TemplateItem;

/// Mako template processor config. No custom fields.
/// `command` is the Python interpreter used to render (default: python3).
/// Unused `StandardConfig` fields: formats, `output_dir`.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct MakoConfig {
    #[serde(flatten)]
    pub standard: StandardConfig,
}

/// Render a Mako template via the configured Python interpreter and write to output
fn render_mako(ctx: &crate::build_context::BuildContext, python: &str, item: &TemplateItem) -> Result<()> {
    // Ensure parent directory of output exists
    crate::processors::ensure_output_dir(&item.output_path)?;

    let source = item.source_path.display().to_string()
        .replace('\\', "\\\\").replace('\'', "\\'");
    let target = item.output_path.display().to_string()
        .replace('\\', "\\\\").replace('\'', "\\'");

    let python_script = format!(
        r"
import mako.template, mako.lookup
lookup = mako.lookup.TemplateLookup(directories=['.'])
t = mako.template.Template(filename='{source}', lookup=lookup)
output = t.render()
with open('{target}', 'w') as f:
    f.write(output)
"
    );

    let mut cmd = Command::new(python);
    cmd.arg("-c").arg(&python_script);
    let output = run_command(ctx, &cmd)?;
    check_command_output(&output, format!("mako render {}", item.source_path.display()))
}

pub struct MakoProcessor {
    config: MakoConfig,
}

impl MakoProcessor {
    pub const fn new(config: MakoConfig) -> Self {
        Self {
            config,
        }
    }
}

impl Processor for MakoProcessor {
    fn scan_config(&self) -> &crate::config::StandardConfig {
        &self.config.standard
    }


    fn config_json(&self) -> Option<String> {
        crate::processors::ProcessorBase::config_json(&self.config)
    }

    fn clean(&self, product: &crate::graph::Product, verbose: bool) -> anyhow::Result<usize> {
        crate::processors::ProcessorBase::clean(product, &product.processor, verbose)
    }

    fn auto_detect(&self, file_index: &FileIndex) -> bool {
        !super::find_templates(&self.config.standard, file_index).is_empty()
    }

    fn required_tools(&self) -> Vec<String> {
        // `command` is the interpreter (python3); rendering additionally needs
        // the Mako library, which `render_mako` imports. `mako-render` is the
        // console script from that same distribution — probing it is how a
        // library dependency becomes visible to the executable-based tool
        // registry, so `tools install` covers it like any other tool.
        vec![self.config.standard.command.clone(), "mako-render".to_string()]
    }

    fn discover(&self, graph: &mut BuildGraph, file_index: &FileIndex, instance_name: &str) -> Result<()> {
        let items = super::find_templates(&self.config.standard, file_index);
        let extra = resolve_extra_inputs(&self.config.standard.dep_inputs)?;

        for item in items {
            let mut inputs = Vec::with_capacity(1 + extra.len());
            inputs.push(item.source_path.clone());
            inputs.extend_from_slice(&extra);
            graph.add_product(
                inputs,
                vec![item.output_path.clone()],
                instance_name,
                Some(output_config_hash(&self.config, &crate::config::checksum_fields_of(instance_name))),
            )?;
        }

        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let item = TemplateItem::new(
            product.primary_input().to_path_buf(),
            product.primary_output().to_path_buf(),
        );
        let python = self.config.standard.require_command("mako")?;
        render_mako(ctx, python, &item)
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(MakoProcessor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "mako",
        processor_type: crate::processors::ProcessorType::Generator,
        create: plugin_create,
        fields: &[],
        omit_standard_fields: &[],
        scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".mako"], src_exclude_dirs: &[] }),
        defaults: Some(crate::config::ProcessorDefaults { command: "python3", ..crate::config::ProcessorDefaults::EMPTY }),
        defconfig_json: crate::registries::default_config_json::<MakoConfig>,
        keywords: &["python", "template", "generator", "pip"],
        description: "Render Mako templates into output files",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
