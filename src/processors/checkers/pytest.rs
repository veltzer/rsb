//! pytest checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_pytest(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Run Python tests using pytest", subcommand: None, prepend_args: &[], extra_tools: &["python3"], fix_subcommand: None, fix_prepend_args: &[], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "pytest", processor_type: crate::processors::ProcessorType::Checker, create: create_pytest,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".py"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "pytest", dep_auto: &["conftest.py", "pytest.ini", "pyproject.toml"], ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["python", "tester", "testing", "py", "pip"],
    description: "Run Python tests using pytest",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
