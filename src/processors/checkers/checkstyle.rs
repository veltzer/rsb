//! checkstyle checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_checkstyle(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Check Java code style using Checkstyle", subcommand: None, prepend_args: &[], extra_tools: &[], fix_subcommand: None, fix_prepend_args: &[], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "checkstyle", processor_type: crate::processors::ProcessorType::Checker, create: create_checkstyle,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".java"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "checkstyle", dep_auto: &["checkstyle.xml"], ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["java", "linter", "checker", "style"],
    description: "Check Java code style using Checkstyle",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
