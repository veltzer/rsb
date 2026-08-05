//! checkpatch checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_checkpatch(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Check kernel patches using checkpatch.pl", subcommand: None, prepend_args: &["--no-tree", "-f"], extra_tools: &["perl"], fix_subcommand: None, fix_prepend_args: &[], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "checkpatch", processor_type: crate::processors::ProcessorType::Checker, create: create_checkpatch,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".c", ".h"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "checkpatch.pl", ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["c", "linux", "kernel", "checker", "patch"],
    description: "Check kernel patches using checkpatch.pl",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
