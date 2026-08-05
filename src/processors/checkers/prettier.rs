//! prettier checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_prettier(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Check formatting with prettier", subcommand: None, prepend_args: &["--check"], extra_tools: &[], fix_subcommand: None, fix_prepend_args: &["--write"], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "prettier", processor_type: crate::processors::ProcessorType::Checker, create: create_prettier,
    fields: &[],
    omit_standard_fields: &[],
    scan_defaults: Some(crate::config::ScanDefaultsData { src_dirs: &[], src_extensions: &[".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs", ".css", ".scss", ".less", ".html", ".json", ".md", ".yaml", ".yml"], src_exclude_dirs: &[] }),
    defaults: Some(crate::config::ProcessorDefaults { command: "prettier", dep_auto: &[".prettierrc", ".prettierrc.json", ".prettierrc.js", ".prettierrc.yml", ".prettierrc.yaml", ".prettierrc.toml", ".prettierrc.cjs", ".prettierrc.mjs", "prettier.config.js", "prettier.config.cjs", "prettier.config.mjs"], ..crate::config::ProcessorDefaults::EMPTY }),
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["javascript", "typescript", "css", "html", "json", "markdown", "yaml", "formatter", "web", "frontend", "node", "npm"],
    description: "Check formatting with prettier",
    is_native: false,
    can_fix: false,
    supports_batch: true,
    max_jobs_cap: None,
} }
