//! standard checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_standard(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Check JavaScript style using standard", subcommand: None, prepend_args: &[], extra_tools: &["node"], fix_subcommand: None, fix_prepend_args: &["--fix"], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "standard", processor_type: crate::processors::ProcessorType::Checker, create: create_standard,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".js"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "standard", ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["javascript", "linter", "js", "node", "npm", "web", "frontend"],
    description: "Check JavaScript style using standard",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
