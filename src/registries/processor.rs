//! Processor plugin registry.
//!
//! Every built-in processor (checker, generator, creator, mass-generator) submits
//! a [`ProcessorPlugin`] entry via `inventory::submit!`. The inventory is collected
//! at link time.

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::config::KnownFields;
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
    /// Config metadata
    pub known_fields: fn() -> &'static [&'static str],
    pub checksum_fields: fn() -> &'static [&'static str],
    pub must_fields: fn() -> &'static [&'static str],
    pub field_descriptions: fn() -> &'static [(&'static str, &'static str)],
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
    find_plugin(name).map(|p| p.description).unwrap_or("")
}

/// Return the processor type for a processor by instance name, or `Checker` if unknown.
pub fn processor_type_of(name: &str) -> crate::processors::ProcessorType {
    find_plugin(name)
        .map(|p| p.processor_type)
        .unwrap_or(crate::processors::ProcessorType::Checker)
}

/// Return whether a processor is native (pure Rust) by instance name.
pub fn is_native(name: &str) -> bool {
    find_plugin(name).is_some_and(|p| p.is_native)
}

/// Return whether a processor can fix by instance name.
pub fn can_fix(name: &str) -> bool {
    find_plugin(name).is_some_and(|p| p.can_fix)
}

/// Look up a processor's implementation version by name.
/// Returns `None` for processor names not in the builtin registry (e.g. Lua plugins).
/// Used by `Product::descriptor_key` to mix the processor's version into every
/// cache key, so bumping a processor's `version` invalidates exactly that
/// processor's cached outputs.
pub fn processor_version(name: &str) -> Option<u32> {
    all_plugins().find(|p| p.name == name).map(|p| p.version)
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
pub fn default_config_json<C: Default + DeserializeOwned + Serialize + KnownFields>(name: &str) -> Option<String> {
    let mut val = toml::Value::Table(toml::map::Map::new());
    let mut prov = crate::config::ProvenanceMap::new();
    apply_all_defaults(name, &mut val, &mut prov);
    let cfg: C = toml::from_str(&toml::to_string(&val).ok()?).ok()?;
    let json_val = serde_json::to_value(&cfg).ok()?;

    // Stage C defense-in-depth check (debug builds only): every key the user
    // can write must be classified as either "in checksum_fields" (affects
    // output) or "in known_fields - checksum_fields" (recognized but not
    // hashed). Configs that `#[serde(flatten)]` StandardConfig inherit all of
    // its fields at serialization time even when the per-processor
    // known_fields() omits them, so StandardConfig's known_fields are also
    // accepted as recognized.
    #[cfg(debug_assertions)]
    {
        if let Some(obj) = json_val.as_object() {
            let known: std::collections::HashSet<&str> = C::known_fields().iter().copied()
                .chain(crate::config::StandardConfig::known_fields().iter().copied())
                .chain(crate::config::SCAN_CONFIG_FIELDS.iter().copied())
                .chain(crate::config::STANDARD_EXTRA_FIELDS.iter().copied())
                .collect();
            let checksum: std::collections::HashSet<&str> = C::checksum_fields().iter().copied().collect();
            for key in obj.keys() {
                debug_assert!(
                    known.contains(key.as_str()),
                    "Processor '{name}': default config field '{key}' is serialized but not declared in known_fields() or scan fields"
                );
            }
            for k in &checksum {
                debug_assert!(
                    known.contains(k),
                    "Processor '{name}': checksum_fields() contains '{k}' which is not in known_fields()"
                );
            }
        }
    }

    serde_json::to_string_pretty(&json_val).ok()
}

pub fn typed_known_fields<C: KnownFields>() -> &'static [&'static str] { C::known_fields() }
pub fn typed_checksum_fields<C: KnownFields>() -> &'static [&'static str] { C::checksum_fields() }
pub fn typed_must_fields<C: KnownFields>() -> &'static [&'static str] { C::must_fields() }
pub fn typed_field_descriptions<C: KnownFields>() -> &'static [(&'static str, &'static str)] { C::field_descriptions() }
