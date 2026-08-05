//! Processor plugin registry.
//!
//! Every built-in processor (checker, generator, creator, mass-generator) submits
//! a [`ProcessorPlugin`] entry via `inventory::submit!`. The inventory is collected
//! at link time.

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::processors::{Processor, ProcessorType};

/// A processor plugin. One struct for all processor types.
/// Each processor file submits one of these via `inventory::submit!`.
///
/// The plugin is a factory: it knows its name, type, how to create a processor
/// from TOML config, and metadata about its config fields.
///
/// The framework applies defaults to the TOML before calling `create`.
/// The `create` function deserializes the TOML and returns a fully configured,
/// immutable processor.
pub struct ProcessorPlugin {
    pub name: &'static str,
    /// Processor type (checker, generator, creator, explicit).
    pub processor_type: ProcessorType,
    /// Implementation version. **Bump this when changes would make the processor
    /// produce different output for the same inputs**, or change which inputs are
    /// discovered, which outputs are declared, or how config fields are interpreted.
    /// Do NOT bump for refactors, comments, reformats, or behavior-preserving
    /// bug fixes. See `docs/src/processor-versioning.md` for the full bump rule.
    ///
    /// The version is mixed into every product's cache key, so bumping here
    /// invalidates caches only for this processor (leaves others untouched).
    pub version: u32,
    /// Create a processor from resolved TOML config (defaults already applied).
    pub create: fn(&toml::Value) -> Result<Box<dyn Processor>>,
    /// The processor's custom config fields — THE schema. Every projection
    /// (known/checksum/must fields, descriptions, expected types) derives
    /// from this list plus the implicit `StandardConfig` fields, so a field
    /// is declared exactly once, in the processor's own file. Standard
    /// fields need no entry unless overridden (e.g. `required: true` on
    /// "command"); a spec whose name matches a standard field replaces the
    /// inherited one.
    pub fields: &'static [crate::config::FieldSpec],
    /// Standard fields this processor does NOT accept (e.g. `cc` takes no
    /// "command" — compilers come from cc.yaml). Listed fields are removed
    /// from the derived known/checksum sets, so the validator rejects them.
    pub omit_standard_fields: &'static [&'static str],
    /// Scan defaults (`src_extensions` etc.), applied with provenance before
    /// deserialization. `None` for processors that scan nothing by default
    /// (script, creator, generator, explicit).
    pub scan_defaults: Option<crate::config::ScanDefaultsData>,
    /// Processor defaults (`command`, `dep_auto`, ...), applied with
    /// provenance before deserialization. `None` when every default is empty.
    pub defaults: Option<crate::config::ProcessorDefaults>,
    /// Return the default config as pretty JSON. Receives the processor name
    /// so it can apply the correct defaults.
    pub defconfig_json: fn(&str) -> Option<String>,
    /// Search keywords for `processors search`.
    pub keywords: &'static [&'static str],
    /// Human-readable description (static, no instantiation needed).
    pub description: &'static str,
    /// Whether this is a native (pure Rust) processor.
    pub is_native: bool,
    /// Whether this processor has fix capability (`rsconstruct fix`).
    pub can_fix: bool,
    /// Whether this processor can execute multiple products in one invocation.
    /// Static capability — if false, the `batch` config field has no effect at runtime.
    pub supports_batch: bool,
    /// Hard cap on parallel jobs for this processor. `None` means no cap.
    /// `Some(1)` means the processor must run one product at a time (e.g. package
    /// managers, whole-project aggregators). The effective `max_jobs` is
    /// `min(config.max_jobs, max_jobs_cap)` with `None` treated as unlimited.
    pub max_jobs_cap: Option<usize>,
}

unsafe impl Sync for ProcessorPlugin {}

inventory::collect!(ProcessorPlugin);

pub fn all_plugins() -> impl Iterator<Item = &'static ProcessorPlugin> {
    inventory::iter::<ProcessorPlugin>.into_iter()
}

/// Look up a processor plugin by type name (e.g. "marp"). For multi-instance
/// names (e.g. "explicit.foo"), strip the instance suffix before lookup.
pub fn find_plugin(name: &str) -> Option<&'static ProcessorPlugin> {
    let type_name = name.split('.').next().unwrap_or(name);
    all_plugins().find(|p| p.name == type_name)
}

/// Return the static description for a processor by instance name, or `""` if unknown.
pub fn description_of(name: &str) -> &'static str {
    find_plugin(name).map_or("", |p| p.description)
}

/// Return the processor type for a processor by instance name, or `Checker` if unknown.
pub fn processor_type_of(name: &str) -> crate::processors::ProcessorType {
    find_plugin(name)
        .map_or(crate::processors::ProcessorType::Checker, |p| p.processor_type)
}

/// Return whether a processor is native (pure Rust) by instance name.
pub fn is_native(name: &str) -> bool {
    find_plugin(name).is_some_and(|p| p.is_native)
}

/// Return whether a processor can fix by instance name.
pub fn can_fix(name: &str) -> bool {
    find_plugin(name).is_some_and(|p| p.can_fix)
}

/// Look up a processor's implementation version by instance name.
/// Returns `None` for processor names not in the builtin registry (e.g. Lua plugins).
/// Used by `Product::descriptor_key` to mix the processor's version into every
/// cache key, so bumping a processor's `version` invalidates exactly that
/// processor's cached outputs.
///
/// Must route through `find_plugin`: products carry *instance* names
/// ("pylint.core"), and a hand-rolled lookup that misses the suffix strip
/// silently keys every multi-instance processor at v0, so version bumps
/// never invalidate their cached results.
pub fn processor_version(name: &str) -> Option<u32> {
    find_plugin(name).map(|p| p.version)
}

/// Build a clap value parser that accepts any registered processor type name (pname).
pub fn processor_name_parser() -> clap::builder::PossibleValuesParser {
    let mut names: Vec<&'static str> = all_plugins().map(|p| p.name).collect();
    names.sort_unstable();
    clap::builder::PossibleValuesParser::new(names)
}

/// Apply both processor defaults and scan defaults to a TOML value.
/// Every field that's injected is recorded in `provenance`.
pub fn apply_all_defaults(
    name: &str,
    value: &mut toml::Value,
    provenance: &mut crate::config::ProvenanceMap,
) {
    crate::config::apply_processor_defaults(name, value, provenance);
    crate::config::apply_scan_defaults(name, value, provenance);
}

// --- Helpers that processor files call from their create/defconfig functions ---

/// Deserialize TOML into config type C and call the constructor.
/// The TOML should already have defaults applied by the framework.
pub fn deserialize_and_create<C: Default + DeserializeOwned>(
    config_toml: &toml::Value, ctor: fn(C) -> Box<dyn Processor>,
) -> Result<Box<dyn Processor>> {
    let cfg: C = toml::from_str(&toml::to_string(config_toml)?)?;
    Ok(ctor(cfg))
}

/// Like [`deserialize_and_create`], for processors whose constructor can fail
/// (e.g. reading a support file like a personal dictionary).
pub fn deserialize_and_try_create<C: Default + DeserializeOwned>(
    config_toml: &toml::Value, ctor: fn(C) -> Result<Box<dyn Processor>>,
) -> Result<Box<dyn Processor>> {
    let cfg: C = toml::from_str(&toml::to_string(config_toml)?)?;
    ctor(cfg)
}

/// Build default config JSON for a config type, applying defaults for the given processor name.
pub fn default_config_json<C: Default + DeserializeOwned + Serialize>(name: &str) -> Option<String> {
    let mut val = toml::Value::Table(toml::map::Map::new());
    let mut prov = crate::config::ProvenanceMap::new();
    apply_all_defaults(name, &mut val, &mut prov);
    let cfg: C = toml::from_str(&toml::to_string(&val).ok()?).ok()?;
    let json_val = serde_json::to_value(&cfg).ok()?;

    // Defense-in-depth check (debug builds only): every key the config
    // serializes must be recognized by the derived schema (the plugin's
    // FieldSpec list plus the implicit StandardConfig/scan fields) — a
    // serialized-but-undeclared field would be invisible to validation and
    // to checksum membership.
    // Runtime-gated, not `#[cfg]`-forked: a cfg'd-out block is code that
    // `cargo test` can never compile. `cfg!` keeps it compiled everywhere
    // and evaluated only in debug builds.
    if cfg!(debug_assertions)
        && let Some(obj) = json_val.as_object() {
            use crate::config::KnownFields as _;
            let known: std::collections::HashSet<&str> =
                crate::config::ProcessorConfig::known_fields_for(name).unwrap_or_default()
                    .into_iter()
                    .chain(crate::config::StandardConfig::known_fields().iter().copied())
                    .chain(crate::config::SCAN_CONFIG_FIELDS.iter().copied())
                    .chain(crate::config::STANDARD_EXTRA_FIELDS.iter().copied())
                    .collect();
            for key in obj.keys() {
                debug_assert!(
                    known.contains(key.as_str()),
                    "Processor '{name}': default config field '{key}' is serialized but not declared in its FieldSpec list or scan fields"
                );
            }
        }

    serde_json::to_string_pretty(&json_val).ok()
}

// Typed KnownFields projection — used by ANALYZER plugin entries only
// (AnalyzerPlugin still carries metadata as fn pointers). Processor plugins
// declare a FieldSpec list instead; do not use this in ProcessorPlugin
// entries.
pub fn typed_known_fields<C: crate::config::KnownFields>() -> &'static [&'static str] { C::known_fields() }

#[cfg(test)]
mod tests {
    use super::*;

    /// Every registry accessor must answer identically for a type name and
    /// for an instance name of that type. Products carry instance names
    /// ("pylint.core"), so an accessor that skips the suffix strip silently
    /// misbehaves only for multi-instance configs — `processor_version` did
    /// exactly that, keying every multi-instance processor's cache at v0 so
    /// version bumps never invalidated their cached results.
    #[test]
    fn accessors_resolve_instance_names_like_type_names() {
        for plugin in all_plugins() {
            let instance = format!("{}.someinst", plugin.name);
            assert_eq!(processor_version(&instance), Some(plugin.version),
                "processor_version must strip the instance suffix for '{instance}'");
            assert_eq!(processor_version(plugin.name), Some(plugin.version));
            assert_eq!(is_native(&instance), is_native(plugin.name),
                "is_native must strip the instance suffix for '{instance}'");
            assert_eq!(can_fix(&instance), can_fix(plugin.name),
                "can_fix must strip the instance suffix for '{instance}'");
            assert_eq!(description_of(&instance), description_of(plugin.name),
                "description_of must strip the instance suffix for '{instance}'");
        }
    }
}
