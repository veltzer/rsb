use anyhow::Result;
use std::process::Command;

use crate::config::{Jinja2Config, output_config_hash, resolve_extra_inputs};
use crate::file_index::FileIndex;
use crate::graph::{BuildGraph, Product};
use crate::processors::{Processor, run_command, check_command_output};

use super::TemplateItem;

/// Render a Jinja2 template via the configured Python interpreter and write to output
fn render_jinja2(ctx: &crate::build_context::BuildContext, python: &str, item: &TemplateItem) -> Result<()> {
    crate::processors::ensure_output_dir(&item.output_path)?;

    let source = item.source_path.display().to_string()
        .replace('\\', "\\\\").replace('\'', "\\'");
    let target = item.output_path.display().to_string()
        .replace('\\', "\\\\").replace('\'', "\\'");

    let python_script = format!(
        r#"
import jinja2, os
loader = jinja2.FileSystemLoader('.')
env = jinja2.Environment(loader=loader)
template = env.get_template('{source}')
output = template.render(**os.environ)
with open('{target}', 'w') as f:
    f.write(output)
"#
    );

    let mut cmd = Command::new(python);
    cmd.arg("-c").arg(&python_script);
    let output = run_command(ctx, &cmd)?;
    check_command_output(&output, format!("jinja2 render {}", item.source_path.display()))
}

pub struct Jinja2Processor {
    config: Jinja2Config,
}

impl Jinja2Processor {
    pub const fn new(config: Jinja2Config) -> Self {
        Self {
            config,
        }
    }
}

impl Processor for Jinja2Processor {
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
        vec![self.config.standard.command.clone()]
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
                Some(output_config_hash(&self.config, <crate::config::Jinja2Config as crate::config::KnownFields>::checksum_fields())),
            )?;
        }

        Ok(())
    }

    fn execute(&self, ctx: &crate::build_context::BuildContext, product: &Product) -> Result<()> {
        let item = TemplateItem::new(
            product.primary_input().to_path_buf(),
            product.primary_output().to_path_buf(),
        );
        let python = self.config.standard.require_command("jinja2")?;
        render_jinja2(ctx, python, &item)
    }
}

fn plugin_create(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(Jinja2Processor::new(cfg)))
}
inventory::submit! {
    crate::registries::ProcessorPlugin {
        version: 1,
        name: "jinja2",
        processor_type: crate::processors::ProcessorType::Generator,
        create: plugin_create,
        defconfig_json: crate::registries::default_config_json::<crate::config::Jinja2Config>,
        known_fields: crate::registries::typed_known_fields::<crate::config::Jinja2Config>,
        checksum_fields: crate::registries::typed_checksum_fields::<crate::config::Jinja2Config>,
        must_fields: crate::registries::typed_must_fields::<crate::config::Jinja2Config>,
        field_descriptions: crate::registries::typed_field_descriptions::<crate::config::Jinja2Config>,
        keywords: &["python", "template", "generator", "jinja", "pip"],
        description: "Render Jinja2 templates into output files",
        is_native: false,
        can_fix: false,
        supports_batch: false,
        max_jobs_cap: None,
    }
}
