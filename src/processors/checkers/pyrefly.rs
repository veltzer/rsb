//! pyrefly checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_pyrefly(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Type-check Python files using pyrefly", subcommand: Some("check"), prepend_args: &["--disable-project-excludes-heuristics"], extra_tools: &[], fix_subcommand: None, fix_prepend_args: &[], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "pyrefly", processor_type: crate::processors::ProcessorType::Checker, create: create_pyrefly,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".py"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "pyrefly", dep_auto: &["pyproject.toml"], ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["python", "type-checker", "types", "py", "pip"],
    description: "Type-check Python files using pyrefly",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
