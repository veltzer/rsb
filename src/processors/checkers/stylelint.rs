//! stylelint checker — registered as a {`SimpleChecker`}.

use crate::processors::SimpleChecker;
use crate::config::SimpleCheckerParams;

fn create_stylelint(toml: &toml::Value) -> anyhow::Result<Box<dyn crate::processors::Processor>> {
    crate::registries::deserialize_and_create(toml, |cfg| Box::new(SimpleChecker::new(cfg, SimpleCheckerParams { description: "Lint CSS/SCSS files using stylelint", subcommand: None, prepend_args: &[], extra_tools: &["node"], fix_subcommand: None, fix_prepend_args: &["--fix"], fix_batch: None })))
}
inventory::submit! { crate::registries::ProcessorPlugin {
    version: 1,
    name: "stylelint", processor_type: crate::processors::ProcessorType::Checker, create: create_stylelint,
    known_fields: crate::registries::typed_known_fields::<crate::config::StandardConfig>,
    checksum_fields: crate::registries::typed_checksum_fields::<crate::config::StandardConfig>,
    must_fields: crate::registries::typed_must_fields::<crate::config::StandardConfig>,
    field_descriptions: crate::registries::typed_field_descriptions::<crate::config::StandardConfig>,
    defconfig_json: crate::registries::default_config_json::<crate::config::StandardConfig>,
    keywords: &["css", "scss", "less", "linter", "web", "frontend", "node", "npm"],
    description: "Lint CSS/SCSS files using stylelint",
    is_native: false,
    can_fix: true,
    supports_batch: true,
    max_jobs_cap: None,
} }
