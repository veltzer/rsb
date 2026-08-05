//! yamllint checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_yamllint(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Lint YAML files using yamllint", subcommand: None, prepend_args: &[], extra_tools: &["python3"], fix_subcommand: None, fix_prepend_args: &[], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "yamllint", processor_type: crate::processors::ProcessorType::Checker, create: create_yamllint,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".yml", ".yaml"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "yamllint", dep_auto: &[".yamllint", ".yamllint.yml", ".yamllint.yaml"], ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["yaml", "yml", "linter", "validator", "pip"],
    description: "Lint YAML files using yamllint",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
