//! rumdl checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_rumdl(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Lint Markdown files using rumdl", subcommand: Some("check"), prepend_args: &[], extra_tools: &[], fix_subcommand: None, fix_prepend_args: &["--fix"], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "rumdl", processor_type: crate::processors::ProcessorType::Checker, create: create_rumdl,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "rumdl", dep_auto: &[".rumdl.toml"], ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["markdown", "md", "linter", "rust"],
    description: "Lint Markdown files using rumdl",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
