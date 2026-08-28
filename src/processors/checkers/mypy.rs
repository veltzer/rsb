//! mypy checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_mypy(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Type-check Python files using mypy", subcommand: None, prepend_args: &[], extra_tools: &["python3"], fix_subcommand: None, fix_prepend_args: &[], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "mypy", processor_type: crate::processors::ProcessorType::Checker, create: create_mypy,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".py"], src_exclude_dirs: &[] }),
    // Every config file mypy's discovery order consults: an edit to any of
    // them must invalidate cached results. pyproject.toml is where the
    // fleet keeps [tool.mypy] since the .mypy.ini unification.
    defaults: Some(crate::config::ProcessorDefaults { command: "mypy", dep_auto: &["mypy.ini", ".mypy.ini", "pyproject.toml"], ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["python", "type-checker", "types", "py", "pip"],
    description: "Type-check Python files using mypy",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
