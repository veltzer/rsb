//! taplo checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_taplo(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Check TOML files using taplo", subcommand: Some("check"), prepend_args: &[], extra_tools: &[], fix_subcommand: Some("fmt"), fix_prepend_args: &[], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "taplo", processor_type: crate::processors::ProcessorType::Checker, create: create_taplo,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".toml"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "taplo", dep_auto: &["taplo.toml", ".taplo.toml"], ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["toml", "formatter", "linter", "validator"],
    description: "Check TOML files using taplo",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
