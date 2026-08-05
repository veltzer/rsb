//! ruff checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_ruff(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Lint and format Python files using ruff", subcommand: Some("check"), prepend_args: &[], extra_tools: &[], fix_subcommand: Some("check"), fix_prepend_args: &["--fix"], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "ruff", processor_type: crate::processors::ProcessorType::Checker, create: create_ruff,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".py"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "ruff", dep_auto: &["ruff.toml", ".ruff.toml", "pyproject.toml"], ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["python", "linter", "formatter", "py", "pip"],
    description: "Lint and format Python files using ruff",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
