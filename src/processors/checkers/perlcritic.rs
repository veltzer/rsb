//! perlcritic checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_perlcritic(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Analyze Perl code using perlcritic", subcommand: None, prepend_args: &[], extra_tools: &["perl"], fix_subcommand: None, fix_prepend_args: &[], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "perlcritic", processor_type: crate::processors::ProcessorType::Checker, create: create_perlcritic,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".pl", ".pm"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "perlcritic", dep_auto: &[".perlcriticrc"], ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["perl", "linter", "pl", "pm"],
    description: "Analyze Perl code using perlcritic",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
