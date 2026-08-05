//! black checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_black(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    // Black's fix is its bare invocation (reformat in place); --quiet is the
    // explicit fix marker — without any fix param, has_fix() would be false
    // and batch fixing silently disabled. Fix capability is currently off
    // registry-wide (every plugin has can_fix: false); the marker keeps the
    // fix path coherent if it is ever re-enabled.
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Check Python code formatting using black", subcommand: None, prepend_args: &["--check"], extra_tools: &["python3"], fix_subcommand: None, fix_prepend_args: &["--quiet"], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "black", processor_type: crate::processors::ProcessorType::Checker, create: create_black,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".py"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "black", dep_auto: &["pyproject.toml"], ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["python", "formatter", "py", "pip"],
    description: "Check Python code formatting using black",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
