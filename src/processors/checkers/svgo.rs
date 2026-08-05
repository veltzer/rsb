//! svgo checker — registered as a {`SimpleChecker`}.
//! Runs `svgo --quiet -o - -i <file>` to validate SVG files; stdout is
//! discarded and we only care about svgo's exit code (non-zero = malformed SVG).
//! Batch is disabled: svgo requires matching input/output counts, so N inputs
//! with the single `-o -` would be rejected. (`-o -` writes to stdout, which
//! also avoids the non-portable /dev/null.)

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_svgo(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Validate SVG files using svgo (stdout discarded; non-zero exit = malformed SVG)", subcommand: None, prepend_args: &["--quiet", "-o", "-", "-i"], extra_tools: &[], fix_subcommand: None, fix_prepend_args: &[], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "svgo", processor_type: crate::processors::ProcessorType::Checker, create: create_svgo,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".svg"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "svgo", dep_auto: &["svgo.config.js", "svgo.config.mjs", "svgo.config.cjs"], ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["svg", "optimizer", "xml", "node", "npm"],
    description: "Validate SVG files using svgo (stdout discarded; non-zero exit = malformed SVG)",
    is_native: false,
    can_fix: false,
    supports_batch: false,
    max_jobs_cap: None,
} }
