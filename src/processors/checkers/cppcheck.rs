//! cppcheck checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_cppcheck(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Static analysis for C/C++ using cppcheck", subcommand: None, prepend_args: &[], extra_tools: &[], fix_subcommand: None, fix_prepend_args: &[], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "cppcheck", processor_type: crate::processors::ProcessorType::Checker, create: create_cppcheck,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".c", ".cc"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "cppcheck", args: &["--error-exitcode=1", "--enable=warning,style,performance,portability"], dep_auto: &[".cppcheck"], batch: Some(false), ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["c", "cpp", "checker", "linter", "cc", "h", "hpp"],
    description: "Static analysis for C/C++ using cppcheck",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
