mod analyzer_configs;
mod processor_configs;
mod provenance;
mod variables;
#[cfg(test)]
mod tests;

pub use analyzer_configs::*;
pub use processor_configs::*;
pub use provenance::{FieldProvenance, ProvenanceMap, Section, SpanMap};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::errors;
use variables::substitute_variables;

const CONFIG_FILE: &str = "rsconstruct.toml";
/// Optional per-repo overlay merged over `rsconstruct.toml` at load time.
/// Lets many repos share one canonical main config while keeping
/// repo-specific sections (dependencies, explicit generators, excludes)
/// in a separate local file.
pub const LOCAL_CONFIG_FILE: &str = "rsconstruct.local.toml";

/// Scan field names in StandardConfig.
/// These are automatically appended to every processor's known fields during validation.
pub const SCAN_CONFIG_FIELDS: &[&str] = &[
    "src_dirs", "src_extensions", "src_exclude_dirs", "src_exclude_files", "src_exclude_paths", "src_files",
];

/// Universal StandardConfig fields that apply to every processor.
/// Automatically appended to every processor's known_fields list during validation
/// and to the defconfig display table — individual processors don't need to repeat them.
pub const STANDARD_EXTRA_FIELDS: &[&str] = &["enabled"];

pub trait KnownFields {
    /// Return the known fields for this config struct, excluding scan fields.
    fn known_fields() -> &'static [&'static str];

    /// Return only the fields that affect build output.
    /// Changes to these fields should trigger config change detection.
    /// Fields not listed here (e.g., src_dirs, src_exclude_dirs, batch, max_jobs)
    /// are discovery or execution parameters that don't affect what the tool produces.
    fn checksum_fields() -> &'static [&'static str];

    /// Return fields that must be explicitly set (non-empty) for the processor to work.
    /// If any of these fields are absent or empty in the user's config, an error is reported.
    /// Default is no required fields.
    fn must_fields() -> &'static [&'static str] {
        &[]
    }

    /// Return (field_name, description) pairs for processor-specific fields only.
    /// Shared scan/dep/exec field descriptions are added by the display layer.
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[]
    }
}

/// Default scan configuration for a processor, as plain data.
/// Used to resolve None scan fields in StandardConfig after TOML deserialization.
pub struct ScanDefaultsData {
    pub src_dirs: &'static [&'static str],
    pub src_extensions: &'static [&'static str],
    pub src_exclude_dirs: &'static [&'static str],
}

/// Per-processor default values applied after TOML deserialization.
#[derive(Default)]
pub struct ProcessorDefaults {
    /// Default command binary name. Empty if not applicable.
    pub command: &'static str,
    /// Default dep_auto entries.
    pub dep_auto: &'static [&'static str],
    /// Default output directory (generators only). Empty if not applicable.
    pub output_dir: &'static str,
    /// Default output formats (generators only). Empty if not applicable.
    pub formats: &'static [&'static str],
    /// Default args. Empty if not applicable.
    pub args: &'static [&'static str],
    /// Override for batch. None means leave the StandardConfig default (true).
    pub batch: Option<bool>,
}


/// Parameters for a simple checker processor — pure data, no macros.
#[derive(Copy, Clone)]
pub struct SimpleCheckerParams {
    /// Human-readable description
    pub description: &'static str,
    /// Optional subcommand (e.g., "check" for ruff)
    pub subcommand: Option<&'static str>,
    /// Args prepended before config args (e.g., ["--check"] for black)
    pub prepend_args: &'static [&'static str],
    /// Additional tools required beyond the command (e.g., ["python3", "node"])
    pub extra_tools: &'static [&'static str],
    /// Fix mode: subcommand to use (e.g., Some("format") for ruff).
    /// None means same command/subcommand as check but with different args.
    /// If both `fix_subcommand` and `fix_prepend_args` are unset, the
    /// processor has no fix capability.
    pub fix_subcommand: Option<&'static str>,
    /// Args prepended in fix mode (e.g., &["--write"] for prettier, &["--fix"] for eslint).
    /// Empty means no fix capability (unless fix_subcommand is set).
    pub fix_prepend_args: &'static [&'static str],
    /// Whether fix mode supports batch execution. Defaults follow check batch.
    pub fix_batch: Option<bool>,
}

/// Validate dep_inputs paths exist and return them as PathBufs.
/// Paths are relative to project root (which is cwd).
pub fn resolve_extra_inputs(dep_inputs: &[String]) -> Result<Vec<PathBuf>> {
    let mut resolved = Vec::new();
    for p in dep_inputs {
        if p.contains('*') || p.contains('?') || p.contains('[') {
            // Glob pattern: expand to matching files
            for entry in glob::glob(p)
                .with_context(|| format!("Invalid glob pattern in dep_inputs: {p}"))?
            {
                let path = crate::errors::ctx(entry, &format!("Failed to read glob entry for: {p}"))?;
                if path.is_file() {
                    resolved.push(path);
                }
            }
        } else {
            let path = PathBuf::from(p);
            if !path.exists() {
                anyhow::bail!("dep_inputs file not found: {p}");
            }
            resolved.push(path);
        }
    }
    Ok(resolved)
}

/// Descriptions for scan fields shared by every processor.
pub const SCAN_FIELD_DESCRIPTIONS: &[(&str, &str)] = &[
    ("src_dirs",            "Directories to scan for source files"),
    ("src_extensions",      "File extensions to match during scanning"),
    ("src_exclude_dirs",    "Directory path segments to skip during scanning"),
    ("src_exclude_files",   "File names to exclude from scanning"),
    ("src_exclude_paths",   "Relative paths to exclude from scanning"),
    ("src_files",           "Additional files to include alongside normal scanning"),
];

/// Descriptions for execution/dependency fields shared by most processors.
pub const SHARED_FIELD_DESCRIPTIONS: &[(&str, &str)] = &[
    ("dep_inputs",  "Extra files that trigger a rebuild when their content changes"),
    ("dep_auto",    "Config files silently added as dep_inputs when they exist on disk"),
    ("batch",       "Pass all matched files to the tool in a single invocation"),
    ("max_jobs",    "Maximum parallel jobs for this processor (overrides global --jobs)"),
    ("enabled",     "Set to false to disable this processor without removing the stanza"),
];

/// Compute a config hash including only the fields named in `checksum_fields`.
/// This is allowlist-based: any key not in `checksum_fields` is removed before
/// hashing. Each processor declares its own checksum_fields() list, which is the
/// single source of truth for which config fields trigger cache invalidation.
pub fn output_config_hash(value: &impl Serialize, checksum_fields: &[&str]) -> String {
    let json_value: serde_json::Value = serde_json::to_value(value).expect(errors::CONFIG_SERIALIZE);
    let filtered = if let serde_json::Value::Object(map) = json_value {
        let kept: serde_json::Map<String, serde_json::Value> = map.into_iter()
            .filter(|(k, _)| checksum_fields.contains(&k.as_str()))
            .collect();
        serde_json::Value::Object(kept)
    } else {
        json_value
    };
    let json = serde_json::to_string(&filtered).expect(errors::CONFIG_SERIALIZE);
    let hash = Sha256::digest(json.as_bytes());
    hex::encode(hash)
}



const DEFAULT_PLUGINS_DIR: &str = "plugins";

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PluginsConfig {
    #[serde(default = "default_plugins_dir")]
    pub dir: String,
}

fn default_plugins_dir() -> String {
    DEFAULT_PLUGINS_DIR.into()
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self { dir: DEFAULT_PLUGINS_DIR.into() }
    }
}

/// Declared project dependencies by package manager.
/// Used by `rsconstruct doctor` to verify and `rsconstruct tools install-deps` to install.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct DependenciesConfig {
    /// Python packages (installed via pip)
    #[serde(default)]
    pub pip: Vec<String>,
    /// Node.js packages (installed via npm)
    #[serde(default)]
    pub npm: Vec<String>,
    /// Ruby gems (installed via gem)
    #[serde(default)]
    pub gem: Vec<String>,
    /// System packages (checked via `which`, not auto-installed)
    #[serde(default)]
    pub system: Vec<String>,
    /// Wrap apt/dnf/pacman invocations with `eatmydata` to skip fsync calls
    /// and speed up package installs.
    ///
    /// Default: `false`. The post-config phase hook
    /// `eatmydata_ci_default` flips this to `true` when `CI=true` is in
    /// the environment. To turn the wrap off in CI, unset `CI` (or set
    /// it to a non-`true` value); to turn it on outside CI, run with
    /// `CI=true rsconstruct tools install-deps`. The CLI flag
    /// `--no-eatmydata` always wins.
    ///
    /// This is not a normal user-facing config field — it's set by the
    /// runtime, not declared in `rsconstruct.toml`. (The deserializer
    /// still accepts it for round-trip compatibility, but setting it in
    /// the file is overridden by the phase hook.)
    #[serde(default)]
    pub eatmydata: bool,
}

impl DependenciesConfig {
    pub const fn is_empty(&self) -> bool {
        self.pip.is_empty() && self.npm.is_empty() && self.gem.is_empty() && self.system.is_empty()
    }
}

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub build: BuildConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub processor: ProcessorConfig,
    #[serde(default)]
    pub analyzer: AnalyzerConfig,
    #[serde(default)]
    pub completions: CompletionsConfig,
    #[serde(default)]
    pub graph: GraphConfig,
    #[serde(default)]
    pub plugins: PluginsConfig,
    #[serde(default)]
    pub dependencies: DependenciesConfig,
    #[serde(default)]
    pub command: CommandsConfig,
    /// Field-level provenance for every top-level `[section]` (build, cache,
    /// graph, plugins, dependencies, command, completions). Each key is the
    /// section name; the inner map's keys are field names within that section.
    /// Populated by `Config::load` from a toml_edit walk — never present in
    /// user TOML, so skipped during (de)serialization.
    #[serde(skip)]
    pub global_provenance: HashMap<String, ProvenanceMap>,
}

/// Configuration for the `symlink-install` command.
/// `sources[i]` is symlinked to `targets[i]`. Both arrays must be the same length.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct SymlinkInstallConfig {
    /// Source folders containing files to symlink
    #[serde(default)]
    pub sources: Vec<String>,
    /// Target folders where symlinks are created (same length as sources)
    #[serde(default)]
    pub targets: Vec<String>,
}

/// Configuration for custom commands.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct CommandsConfig {
    #[serde(default)]
    pub symlink_install: SymlinkInstallConfig,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct BuildConfig {
    /// Number of parallel jobs (1 = sequential, 0 = auto-detect CPU cores)
    #[serde(default = "default_parallel")]
    pub parallel: usize,
    /// Maximum files per batch for batch-capable processors.
    /// 0 = no limit (all files in one batch). To disable batching use
    /// `--batch-size -1` on the CLI or `batch = false` per processor.
    #[serde(default)]
    pub batch_size: usize,
    /// Global output directory prefix (default: "out").
    /// Processor output_dir fields that start with "out/" will use this as the base instead.
    #[serde(default = "default_output_dir")]
    pub output_dir: String,
    /// Maximum total argument length (in bytes) before splitting a checker
    /// invocation into multiple invocations. Linux's `ARG_MAX` is typically
    /// ~2MB; the default leaves headroom for env vars and the tool path.
    #[serde(default = "default_max_arg_len")]
    pub max_arg_len: usize,
    /// When true, a `src_dirs` entry that doesn't exist on disk deactivates
    /// the processor's scan of that directory instead of failing the build.
    /// Lets one shared config file serve repos with different layouts: a
    /// processor whose directories are all absent simply matches no files.
    /// Default: false — a missing directory is treated as a typo and fails.
    #[serde(default)]
    pub skip_missing_src_dirs: bool,
    /// Maximum fixed-point discovery passes. Discovery repeats while
    /// processors keep adding products from each other's declared outputs;
    /// a config still adding products at the cap fails the build instead of
    /// silently truncating the graph.
    #[serde(default = "default_max_discovery_passes")]
    pub max_discovery_passes: usize,
    /// When true (the default), the versions of the external tools a
    /// processor invokes are mixed into its products' cache keys, so
    /// upgrading a tool invalidates everything that tool produced.
    ///
    /// This used to be opt-in via `rsconstruct tools lock`: without a lock
    /// file no version was mixed in at all, and a tool upgrade silently left
    /// every cached PASS valid. Versions are now queried live (cached per
    /// build in `.rsconstruct/toolver.redb`, keyed by the tool's path, size
    /// and mtime, so the `--version` subprocesses are paid once per upgrade
    /// rather than once per build).
    ///
    /// Set to false for a build that must not shell out to `--version` at
    /// all; cache keys then ignore tool identity, as they did before.
    #[serde(default = "default_hash_tool_versions")]
    pub hash_tool_versions: bool,
}

const fn default_parallel() -> usize {
    0
}

fn default_output_dir() -> String {
    "out".into()
}

const fn default_max_arg_len() -> usize {
    1_000_000
}

const fn default_max_discovery_passes() -> usize {
    10
}

const fn default_hash_tool_versions() -> bool {
    true
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            parallel: 0,
            batch_size: 0, // Default: batching enabled, no size limit
            output_dir: "out".into(),
            max_arg_len: default_max_arg_len(),
            skip_missing_src_dirs: false,
            max_discovery_passes: default_max_discovery_passes(),
            hash_tool_versions: default_hash_tool_versions(),
        }
    }
}

/// Method used to restore files from cache
#[derive(Debug, Deserialize, Serialize, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RestoreMethod {
    /// Auto-detect: use copy in CI environments (CI=true), hardlink otherwise.
    #[default]
    Auto,
    Hardlink,
    Copy,
}

impl RestoreMethod {
    /// Resolve `Auto` to a concrete method based on environment.
    pub fn resolve(self) -> Self {
        match self {
            RestoreMethod::Auto => {
                if std::env::var("CI").is_ok_and(|v| v == "true") {
                    RestoreMethod::Copy
                } else {
                    RestoreMethod::Hardlink
                }
            }
            other => other,
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    #[serde(default)]
    pub restore_method: RestoreMethod,
    /// Whether to compress cached objects with zstd (default: false).
    /// Incompatible with hardlink restore method.
    #[serde(default)]
    pub compression: bool,
    /// Remote cache URL (e.g., "s3://bucket/prefix", "http://host:port/path", or local "file:///path")
    #[serde(default)]
    pub remote: Option<String>,
    /// Whether to push local builds to remote cache (default: true)
    #[serde(default = "default_true")]
    pub remote_push: bool,
    /// Whether to pull from remote cache on miss (default: true)
    #[serde(default = "default_true")]
    pub remote_pull: bool,
    /// Whether to use mtime pre-check to skip unchanged file checksums (default: true)
    #[serde(default = "default_true")]
    pub mtime_check: bool,
    /// How long a cached HTTP response (remote JSON schemas, etc.) stays
    /// fresh, in seconds. Default: 7 days.
    ///
    /// The webcache previously had no expiry at all: a URL fetched once was
    /// served from disk forever, so a schema that changed upstream was never
    /// picked up and the database only ever grew. Set to 0 to disable the
    /// webcache (always re-fetch).
    #[serde(default = "default_webcache_ttl_secs")]
    pub webcache_ttl_secs: u64,
}

/// Seven days. Long enough that a normal working week of builds hits the
/// cache, short enough that an upstream schema change lands without anyone
/// having to know `rsconstruct cache webcache clear` exists.
const fn default_webcache_ttl_secs() -> u64 {
    7 * 24 * 60 * 60
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            restore_method: RestoreMethod::default(),
            compression: false,
            remote: None,
            remote_push: true,
            remote_pull: true,
            mtime_check: true,
            webcache_ttl_secs: default_webcache_ttl_secs(),
        }
    }
}

pub const fn default_true() -> bool {
    true
}

/// A single processor instance parsed from the TOML config.
/// `[processor.pylint]` produces one instance with type_name="pylint", instance_name="pylint".
/// `[processor.pylint.core]` produces one with type_name="pylint", instance_name="pylint.core".
#[derive(Debug, Clone)]
pub struct ProcessorInstance {
    /// Instance name: "pylint" for single, "pylint.core" for named
    pub instance_name: String,
    /// Processor type name: always "pylint"
    pub type_name: String,
    /// The raw TOML config for this instance (deserialized lazily per processor type)
    pub config_toml: toml::Value,
    /// Source of every field in `config_toml` (user TOML, processor default, scan default, …).
    pub provenance: ProvenanceMap,
}

use crate::registries::{self as registry, ProcessorPlugin};

pub fn find_registry_entry(type_name: &str) -> Option<&'static ProcessorPlugin> {
    registry::all_plugins().find(|e| e.name == type_name)
}

/// Return all registered processor plugins.
pub fn registry_entries() -> impl Iterator<Item = &'static ProcessorPlugin> {
    registry::all_plugins()
}

/// Return all known builtin processor type names.
pub fn all_type_names() -> Vec<&'static str> {
    registry::all_plugins().map(|e| e.name).collect()
}

/// Check if a name is a known builtin processor type.
pub fn is_builtin_type(name: &str) -> bool {
    find_registry_entry(name).is_some()
}

/// Seed a provenance map with every top-level key currently present in `value`,
/// marking each as originating from the user's TOML. Defaults applied afterwards
/// will see these entries and not overwrite them.
///
/// Line numbers are filled in later by the toml_edit span pass; until then we
/// use line 0 as a sentinel meaning "from user TOML, line unknown".
pub fn seed_user_provenance(value: &toml::Value) -> ProvenanceMap {
    let mut map = ProvenanceMap::new();
    if let Some(table) = value.as_table() {
        for key in table.keys() {
            map.insert(key.clone(), FieldProvenance::UserToml { line: 0 });
        }
    }
    map
}

/// Resolve scan and processor defaults for an instance config in-place.
pub fn resolve_instance_defaults(
    type_name: &str,
    value: &mut toml::Value,
    provenance: &mut ProvenanceMap,
) {
    if find_registry_entry(type_name).is_some() {
        registry::apply_all_defaults(type_name, value, provenance);
    }
}

impl ProcessorConfig {
    /// Collect unique scan directories from all declared instances.
    pub(crate) fn src_dirs(&self) -> Vec<String> {
        let mut dirs: Vec<String> = self.instances.iter()
            .flat_map(|inst| {
                inst.config_toml.get("src_dirs")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flat_map(|arr| arr.iter().filter_map(|v| v.as_str().map(std::string::ToString::to_string)))
                    .filter(|d| !d.is_empty())
            })
            .collect();
        dirs.sort();
        dirs.dedup();
        dirs
    }

    /// Return known fields for a builtin processor type, or None for Lua plugins.
    pub(crate) fn known_fields_for(type_name: &str) -> Option<&'static [&'static str]> {
        find_registry_entry(type_name).map(|e| (e.known_fields)())
    }

    /// Return checksum-affecting fields for a builtin processor type, or None for Lua plugins.
    pub(crate) fn checksum_fields_for(type_name: &str) -> Option<&'static [&'static str]> {
        find_registry_entry(type_name).map(|e| (e.checksum_fields)())
    }

    /// Return must fields (required non-empty fields) for a builtin processor type, or None for Lua plugins.
    pub(crate) fn must_fields_for(type_name: &str) -> Option<&'static [&'static str]> {
        find_registry_entry(type_name).map(|e| (e.must_fields)())
    }

    /// Return (field, description) pairs for a builtin processor type, or None for Lua plugins.
    pub(crate) fn field_descriptions_for(type_name: &str) -> Option<&'static [(&'static str, &'static str)]> {
        find_registry_entry(type_name).map(|e| (e.field_descriptions)())
    }

    /// Return the default src_dirs for a builtin processor type, or None for Lua plugins.
    pub(crate) fn default_src_dirs_for(type_name: &str) -> Option<&'static [&'static str]> {
        scan_defaults_for(type_name).map(|d| d.src_dirs)
    }

    /// Return the default config for a processor type as pretty JSON, or None if unknown.
    pub(crate) fn defconfig_json(type_name: &str) -> Option<String> {
        let entry = find_registry_entry(type_name)?;
        (entry.defconfig_json)(entry.name)
    }
}

/// Return scan defaults for a builtin processor type.
pub fn scan_defaults_for(type_name: &str) -> Option<ScanDefaultsData> {
    Some(match type_name {
        "tera" => ScanDefaultsData { src_dirs: &["tera.templates"], src_extensions: &[".tera"], src_exclude_dirs: &[] },
        "ruff" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".py"], src_exclude_dirs: &[] },
        "pylint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".py"], src_exclude_dirs: &[] },
        "mypy" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".py"], src_exclude_dirs: &[] },
        "pyrefly" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".py"], src_exclude_dirs: &[] },
        "black" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".py"], src_exclude_dirs: &[] },
        "doctest" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".py"], src_exclude_dirs: &[] },
        "pytest" => ScanDefaultsData { src_dirs: &["tests"], src_extensions: &[".py"], src_exclude_dirs: &[] },
        "cc_single_file" => ScanDefaultsData { src_dirs: &["src"], src_extensions: &[".c", ".cc"], src_exclude_dirs: &[] },
        "cc" => ScanDefaultsData { src_dirs: &[], src_extensions: &["cc.yaml"], src_exclude_dirs: &[] },
        "cppcheck" => ScanDefaultsData { src_dirs: &["src"], src_extensions: &[".c", ".cc"], src_exclude_dirs: &[] },
        "clang_tidy" => ScanDefaultsData { src_dirs: &["src"], src_extensions: &[".c", ".cc"], src_exclude_dirs: &[] },
        "zspell" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "shellcheck" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".sh", ".bash"], src_exclude_dirs: &[] },
        "luacheck" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".lua"], src_exclude_dirs: &[] },
        "make" => ScanDefaultsData { src_dirs: &[], src_extensions: &["Makefile"], src_exclude_dirs: &[] },
        "cargo" => ScanDefaultsData { src_dirs: &[], src_extensions: &["Cargo.toml"], src_exclude_dirs: &[] },
        "clippy" => ScanDefaultsData { src_dirs: &[], src_extensions: &["Cargo.toml"], src_exclude_dirs: &[] },
        "rumdl" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "yamllint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".yml", ".yaml"], src_exclude_dirs: &[] },
        "jq" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".json"], src_exclude_dirs: &[] },
        "jsonlint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".json"], src_exclude_dirs: &[] },
        "taplo" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".toml"], src_exclude_dirs: &[] },
        "json_schema" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".json"], src_exclude_dirs: &[] },
        "tags" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "pip" => ScanDefaultsData { src_dirs: &[], src_extensions: &["requirements.txt"], src_exclude_dirs: &[] },
        "requirements" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".py"], src_exclude_dirs: &[] },
        "sphinx" => ScanDefaultsData { src_dirs: &[], src_extensions: &["conf.py"], src_exclude_dirs: &[] },
        "mdbook" => ScanDefaultsData { src_dirs: &[], src_extensions: &["book.toml"], src_exclude_dirs: &[] },
        "npm" => ScanDefaultsData { src_dirs: &[], src_extensions: &["package.json"], src_exclude_dirs: &[] },
        "gem" => ScanDefaultsData { src_dirs: &[], src_extensions: &["Gemfile"], src_exclude_dirs: &[] },
        "mdl" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "markdownlint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "aspell" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "marp" => ScanDefaultsData { src_dirs: &["marp"], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "pandoc" => ScanDefaultsData { src_dirs: &["pandoc"], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "markdown2html" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "pdflatex" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".tex"], src_exclude_dirs: &[] },
        "a2x" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".txt"], src_exclude_dirs: &[] },
        "ascii" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "terms" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "chromium" => ScanDefaultsData { src_dirs: &["out/marp"], src_extensions: &[".html"], src_exclude_dirs: &[] },
        "mako" => ScanDefaultsData { src_dirs: &["templates.mako"], src_extensions: &[".mako"], src_exclude_dirs: &[] },
        "jinja2" => ScanDefaultsData { src_dirs: &["templates.jinja2"], src_extensions: &[".j2"], src_exclude_dirs: &[] },
        "mermaid" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".mmd"], src_exclude_dirs: &[] },
        "drawio" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".drawio"], src_exclude_dirs: &[] },
        "libreoffice" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".odp"], src_exclude_dirs: &[] },
        "protobuf" => ScanDefaultsData { src_dirs: &["proto"], src_extensions: &[".proto"], src_exclude_dirs: &[] },
        "pdfunite" => ScanDefaultsData { src_dirs: &[], src_extensions: &["course.yaml"], src_exclude_dirs: &[] },
        "ipdfunite" => ScanDefaultsData { src_dirs: &[], src_extensions: &["course.yaml"], src_exclude_dirs: &[] },
        "script" => ScanDefaultsData { src_dirs: &[], src_extensions: &[], src_exclude_dirs: &[] },
        "creator" => ScanDefaultsData { src_dirs: &[], src_extensions: &[], src_exclude_dirs: &[] },
        "generator" => ScanDefaultsData { src_dirs: &[], src_extensions: &[], src_exclude_dirs: &[] },
        "explicit" => ScanDefaultsData { src_dirs: &[], src_extensions: &[], src_exclude_dirs: &[] },
        "linux_module" => ScanDefaultsData { src_dirs: &[], src_extensions: &["linux-module.yaml"], src_exclude_dirs: &[] },
        "cpplint" => ScanDefaultsData { src_dirs: &["src"], src_extensions: &[".c", ".cc", ".h", ".hh"], src_exclude_dirs: &[] },
        "checkpatch" => ScanDefaultsData { src_dirs: &["src"], src_extensions: &[".c", ".h"], src_exclude_dirs: &[] },
        "objdump" => ScanDefaultsData { src_dirs: &["out/cc_single_file"], src_extensions: &[".elf"], src_exclude_dirs: &[] },
        "prettier" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs", ".css", ".scss", ".less", ".html", ".json", ".md", ".yaml", ".yml"], src_exclude_dirs: &[] },
        "eslint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs"], src_exclude_dirs: &[] },
        "jshint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".js", ".jsx", ".mjs", ".cjs"], src_exclude_dirs: &[] },
        "htmlhint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".html", ".htm"], src_exclude_dirs: &[] },
        "tidy" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".html", ".htm"], src_exclude_dirs: &[] },
        "stylelint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".css", ".scss", ".sass", ".less"], src_exclude_dirs: &[] },
        "jslint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".js"], src_exclude_dirs: &[] },
        "standard" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".js"], src_exclude_dirs: &[] },
        "htmllint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".html", ".htm"], src_exclude_dirs: &[] },
        "php_lint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".php"], src_exclude_dirs: &[] },
        "perlcritic" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".pl", ".pm"], src_exclude_dirs: &[] },
        "xmllint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".xml", ".svg"], src_exclude_dirs: &[] },
        "svglint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".svg"], src_exclude_dirs: &[] },
        "svgo" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".svg"], src_exclude_dirs: &[] },
        "checkstyle" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".java"], src_exclude_dirs: &[] },
        "yq" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".yml", ".yaml"], src_exclude_dirs: &[] },
        "cmake" => ScanDefaultsData { src_dirs: &[], src_extensions: &["CMakeLists.txt"], src_exclude_dirs: &[] },
        "hadolint" => ScanDefaultsData { src_dirs: &[], src_extensions: &["Dockerfile"], src_exclude_dirs: &[] },
        "jekyll" => ScanDefaultsData { src_dirs: &[], src_extensions: &["_config.yml"], src_exclude_dirs: &[] },
        "sass" => ScanDefaultsData { src_dirs: &["sass"], src_extensions: &[".scss", ".sass"], src_exclude_dirs: &[] },
        "ijq" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".json"], src_exclude_dirs: &[] },
        "ijsonlint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".json"], src_exclude_dirs: &[] },
        "iyamllint" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".yml", ".yaml"], src_exclude_dirs: &[] },
        "iyamlschema" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".yml", ".yaml"], src_exclude_dirs: &[] },
        "itaplo" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".toml"], src_exclude_dirs: &[] },
        "imarkdown2html" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "isass" => ScanDefaultsData { src_dirs: &["sass"], src_extensions: &[".scss", ".sass"], src_exclude_dirs: &[] },
        "yaml2json" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".yml", ".yaml"], src_exclude_dirs: &[] },
        "rust_single_file" => ScanDefaultsData { src_dirs: &["src"], src_extensions: &[".rs"], src_exclude_dirs: &[] },
        "slidev" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "encoding" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".py", ".rs", ".js", ".ts", ".c", ".cc", ".h", ".hh", ".java", ".rb", ".go", ".sh", ".bash", ".lua", ".pl", ".pm", ".php", ".md", ".yaml", ".yml", ".json", ".toml", ".xml", ".html", ".htm", ".css", ".scss", ".sass", ".tex", ".txt"], src_exclude_dirs: &[] },
        "duplicate_files" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".py", ".rs", ".js", ".ts", ".c", ".cc", ".h", ".hh", ".java", ".rb", ".go", ".sh", ".md", ".yaml", ".yml", ".json", ".toml", ".xml", ".html", ".css"], src_exclude_dirs: &[] },
        "marp_images" => ScanDefaultsData { src_dirs: &["marp"], src_extensions: &[".md"], src_exclude_dirs: &[] },
        "license_header" => ScanDefaultsData { src_dirs: &[], src_extensions: &[".py", ".rs", ".js", ".ts", ".c", ".cc", ".h", ".hh", ".java", ".rb", ".go", ".sh", ".bash"], src_exclude_dirs: &[] },
        _ => return None,
    })
}

/// Return per-processor default values (command, dep_auto, batch override).
/// Only needed for processors whose defaults differ from the struct's Default impl.
pub fn processor_defaults_for(type_name: &str) -> Option<ProcessorDefaults> {
    let d = ProcessorDefaults::default();
    Some(match type_name {
        "ruff" => ProcessorDefaults { command: "ruff", dep_auto: &["ruff.toml", ".ruff.toml", "pyproject.toml"], ..d },
        "pylint" => ProcessorDefaults { command: "pylint", dep_auto: &[".pylintrc"], ..d },
        "pytest" => ProcessorDefaults { command: "pytest", dep_auto: &["conftest.py", "pytest.ini", "pyproject.toml"], ..d },
        "black" => ProcessorDefaults { command: "black", dep_auto: &["pyproject.toml"], ..d },
        "mypy" => ProcessorDefaults { command: "mypy", dep_auto: &["mypy.ini"], ..d },
        "pyrefly" => ProcessorDefaults { command: "pyrefly", dep_auto: &["pyproject.toml"], ..d },
        "rumdl" => ProcessorDefaults { command: "rumdl", dep_auto: &[".rumdl.toml"], ..d },
        "yamllint" => ProcessorDefaults { command: "yamllint", dep_auto: &[".yamllint", ".yamllint.yml", ".yamllint.yaml"], ..d },
        "jq" => ProcessorDefaults { command: "jq", ..d },
        "jsonlint" => ProcessorDefaults { command: "jsonlint", ..d },
        "taplo" => ProcessorDefaults { command: "taplo", dep_auto: &["taplo.toml", ".taplo.toml"], ..d },
        "cppcheck" => ProcessorDefaults { command: "cppcheck", args: &["--error-exitcode=1", "--enable=warning,style,performance,portability"], dep_auto: &[".cppcheck"], batch: Some(false), ..d },
        "cpplint" => ProcessorDefaults { command: "cpplint", ..d },
        "checkpatch" => ProcessorDefaults { command: "checkpatch.pl", ..d },
        "shellcheck" => ProcessorDefaults { command: "shellcheck", dep_auto: &[".shellcheckrc"], ..d },
        "luacheck" => ProcessorDefaults { command: "luacheck", dep_auto: &[".luacheckrc"], ..d },
        "prettier" => ProcessorDefaults { command: "prettier", dep_auto: &[".prettierrc", ".prettierrc.json", ".prettierrc.js", ".prettierrc.yml", ".prettierrc.yaml", ".prettierrc.toml", ".prettierrc.cjs", ".prettierrc.mjs", "prettier.config.js", "prettier.config.cjs", "prettier.config.mjs"], ..d },
        "eslint" => ProcessorDefaults { command: "eslint", dep_auto: &[".eslintrc", ".eslintrc.json", ".eslintrc.js", ".eslintrc.yml", ".eslintrc.yaml", ".eslintrc.cjs", "eslint.config.js", "eslint.config.mjs", "eslint.config.cjs"], ..d },
        "jshint" => ProcessorDefaults { command: "jshint", dep_auto: &[".jshintrc"], ..d },
        "htmlhint" => ProcessorDefaults { command: "htmlhint", dep_auto: &[".htmlhintrc"], ..d },
        "stylelint" => ProcessorDefaults { command: "stylelint", dep_auto: &[".stylelintrc", ".stylelintrc.json", ".stylelintrc.yml", ".stylelintrc.yaml", ".stylelintrc.js", ".stylelintrc.cjs", "stylelint.config.js", "stylelint.config.cjs"], ..d },
        "perlcritic" => ProcessorDefaults { command: "perlcritic", dep_auto: &[".perlcriticrc"], ..d },
        "svglint" => ProcessorDefaults { command: "svglint", dep_auto: &[".svglintrc.js"], ..d },
        "svgo" => ProcessorDefaults { command: "svgo", dep_auto: &["svgo.config.js", "svgo.config.mjs", "svgo.config.cjs"], ..d },
        "checkstyle" => ProcessorDefaults { command: "checkstyle", dep_auto: &["checkstyle.xml"], ..d },
        "cmake" => ProcessorDefaults { command: "cmakelint", ..d },
        "doctest" => ProcessorDefaults { command: "python3", ..d },
        "hadolint" => ProcessorDefaults { command: "hadolint", ..d },
        "htmllint" => ProcessorDefaults { command: "htmllint", ..d },
        "jslint" => ProcessorDefaults { command: "jslint", ..d },
        "php_lint" => ProcessorDefaults { command: "php", ..d },
        "slidev" => ProcessorDefaults { command: "slidev", ..d },
        "standard" => ProcessorDefaults { command: "standard", ..d },
        "tidy" => ProcessorDefaults { command: "tidy", ..d },
        "xmllint" => ProcessorDefaults { command: "xmllint", ..d },
        "yq" => ProcessorDefaults { command: "yq", ..d },
        // Generators
        "marp" => ProcessorDefaults { output_dir: "out/marp", formats: &["pdf"], args: &["--html", "--allow-local-files"], command: "marp", ..d },
        "markdown2html" => ProcessorDefaults { output_dir: "out/markdown2html", command: "markdown", ..d },
        "chromium" => ProcessorDefaults { output_dir: "out/chromium", command: "google-chrome", ..d },
        "mermaid" => ProcessorDefaults { output_dir: "out/mermaid", formats: &["png"], command: "mmdc", ..d },
        "drawio" => ProcessorDefaults { output_dir: "out/drawio", formats: &["png"], command: "drawio", ..d },
        "libreoffice" => ProcessorDefaults { output_dir: "out/libreoffice", formats: &["pdf"], command: "libreoffice", ..d },
        "protobuf" => ProcessorDefaults { output_dir: "out/protobuf", command: "protoc", ..d },
        "cc_single_file" => ProcessorDefaults { output_dir: "out/cc_single_file", ..d },
        "cargo" => ProcessorDefaults { command: "build", ..d },
        "clippy" => ProcessorDefaults { command: "clippy", ..d },
        "generator" => ProcessorDefaults { output_dir: "out/generator", ..d },
        "explicit" => d,
        "sphinx" => ProcessorDefaults { command: "sphinx-build", output_dir: "docs", ..d },
        "mdbook" => ProcessorDefaults { command: "mdbook", output_dir: "book", ..d },
        "npm" => ProcessorDefaults { command: "npm", ..d },
        "sass" => ProcessorDefaults { output_dir: "out/sass", command: "sass", ..d },
        "pandoc" => ProcessorDefaults { output_dir: "out/pandoc", formats: &["pdf", "html", "docx"], command: "pandoc", ..d },
        "a2x" => ProcessorDefaults { output_dir: "out/a2x", command: "a2x", ..d },
        "objdump" => ProcessorDefaults { output_dir: "out/objdump", command: "objdump", ..d },
        "imarkdown2html" => ProcessorDefaults { output_dir: "out/imarkdown2html", ..d },
        "isass" => ProcessorDefaults { output_dir: "out/isass", ..d },
        "yaml2json" => ProcessorDefaults { output_dir: "out/yaml2json", ..d },
        // Checkers with custom dep_auto
        "mdl" => ProcessorDefaults { command: "mdl", dep_auto: &[".mdlrc"], ..d },
        "markdownlint" => ProcessorDefaults { command: "markdownlint", dep_auto: &[".markdownlint.json", ".markdownlint.jsonc", ".markdownlint.yaml"], ..d },
        "aspell" => ProcessorDefaults { command: "aspell", dep_auto: &[".aspell.conf", ".aspell.en.pws", ".aspell.en.prepl"], ..d },
        // Generators with custom output_dir
        "pdflatex" => ProcessorDefaults { command: "pdflatex", output_dir: "out/pdflatex", ..d },
        "rust_single_file" => ProcessorDefaults { command: "rustc", output_dir: "out/rust_single_file", ..d },
        "pdfunite" => ProcessorDefaults { command: "pdfunite", output_dir: "out/pdfunite", ..d },
        "ipdfunite" => ProcessorDefaults { output_dir: "out/ipdfunite", ..d },
        // Creators with custom command
        "gem" => ProcessorDefaults { command: "bundle", ..d },
        "pip" => ProcessorDefaults { command: "pip", ..d },
        "make" => ProcessorDefaults { command: "make", ..d },
        "jekyll" => ProcessorDefaults { command: "jekyll", ..d },
        // Python-script generators: command is the Python interpreter
        "jinja2" => ProcessorDefaults { command: "python3", ..d },
        "mako" => ProcessorDefaults { command: "python3", ..d },
        _ => return None,
    })
}

/// Apply processor-specific defaults to a config TOML value.
/// Sets command and dep_auto if they weren't explicitly provided by the user.
/// Every field that's actually injected is recorded in `provenance` as a processor default.
/// Fields already present in `provenance` (i.e. user-set) are skipped.
pub fn apply_processor_defaults(
    type_name: &str,
    value: &mut toml::Value,
    provenance: &mut ProvenanceMap,
) {
    let Some(defaults) = processor_defaults_for(type_name) else { return };
    let Some(table) = value.as_table_mut() else { return };
    set_string_default(table, "command", defaults.command, provenance, FieldProvenance::ProcessorDefault);
    set_string_default(table, "output_dir", defaults.output_dir, provenance, FieldProvenance::ProcessorDefault);
    set_string_array_default(table, "dep_auto", defaults.dep_auto, provenance, FieldProvenance::ProcessorDefault);
    set_string_array_default(table, "formats", defaults.formats, provenance, FieldProvenance::ProcessorDefault);
    set_string_array_default(table, "args", defaults.args, provenance, FieldProvenance::ProcessorDefault);
    if let Some(batch) = defaults.batch
        && !table.contains_key("batch")
    {
        table.insert("batch".into(), toml::Value::Boolean(batch));
        provenance::record_if_absent(provenance, "batch", FieldProvenance::ProcessorDefault);
    }
}

fn set_string_default(
    table: &mut toml::map::Map<String, toml::Value>,
    key: &str,
    val: &str,
    provenance: &mut ProvenanceMap,
    source: FieldProvenance,
) {
    if !val.is_empty() && !table.contains_key(key) {
        table.insert(key.into(), toml::Value::String(val.into()));
        provenance::record_if_absent(provenance, key, source);
    }
}

fn set_string_array_default(
    table: &mut toml::map::Map<String, toml::Value>,
    key: &str,
    vals: &[&str],
    provenance: &mut ProvenanceMap,
    source: FieldProvenance,
) {
    if !vals.is_empty() && !table.contains_key(key) {
        let arr: Vec<toml::Value> = vals.iter().map(|s| toml::Value::String(s.to_string())).collect();
        table.insert(key.into(), toml::Value::Array(arr));
        provenance::record_if_absent(provenance, key, source);
    }
}

fn set_empty_array_default(
    table: &mut toml::map::Map<String, toml::Value>,
    key: &str,
    provenance: &mut ProvenanceMap,
    source: FieldProvenance,
) {
    if !table.contains_key(key) {
        table.insert(key.into(), toml::Value::Array(Vec::new()));
        provenance::record_if_absent(provenance, key, source);
    }
}

fn set_maybe_empty_array_default(
    table: &mut toml::map::Map<String, toml::Value>,
    key: &str,
    vals: &[&str],
    provenance: &mut ProvenanceMap,
    source: FieldProvenance,
) {
    if !table.contains_key(key) {
        let arr: Vec<toml::Value> = vals.iter().map(|s| toml::Value::String(s.to_string())).collect();
        table.insert(key.into(), toml::Value::Array(arr));
        provenance::record_if_absent(provenance, key, source);
    }
}

/// Apply scan defaults to a config TOML value.
/// Sets src_dirs, src_extensions, and src_exclude_dirs if not explicitly provided.
/// Every field that's actually injected is recorded in `provenance` as a scan default.
pub fn apply_scan_defaults(
    type_name: &str,
    value: &mut toml::Value,
    provenance: &mut ProvenanceMap,
) {
    let Some(defaults) = scan_defaults_for(type_name) else { return };
    let Some(table) = value.as_table_mut() else { return };
    set_maybe_empty_array_default(table, "src_dirs", defaults.src_dirs, provenance, FieldProvenance::ScanDefault);
    set_maybe_empty_array_default(table, "src_extensions", defaults.src_extensions, provenance, FieldProvenance::ScanDefault);
    set_maybe_empty_array_default(table, "src_exclude_dirs", defaults.src_exclude_dirs, provenance, FieldProvenance::ScanDefault);
    set_empty_array_default(table, "src_exclude_files", provenance, FieldProvenance::ScanDefault);
    set_empty_array_default(table, "src_exclude_paths", provenance, FieldProvenance::ScanDefault);
    set_empty_array_default(table, "src_files", provenance, FieldProvenance::ScanDefault);
}

/// Processor configuration: a collection of declared processor instances.
/// Each `[processor.TYPE]` or `[processor.TYPE.NAME]` section in rsconstruct.toml
/// creates a ProcessorInstance. No instances exist by default — only what's declared.
#[derive(Debug, Default)]
pub struct ProcessorConfig {
    /// All declared processor instances
    pub instances: Vec<ProcessorInstance>,
    /// Lua plugin configs (processor types not in the builtin registry)
    pub extra: HashMap<String, toml::Value>,
}

impl Serialize for ProcessorConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        for inst in &self.instances {
            // For named instances (type.name), we need to nest
            if inst.instance_name.contains('.') {
                // Handled as part of the parent type table
            } else {
                map.serialize_entry(&inst.instance_name, &inst.config_toml)?;
            }
        }
        // Group named instances by type
        let mut types: HashMap<&str, Vec<&ProcessorInstance>> = HashMap::new();
        for inst in &self.instances {
            if let Some(dot) = inst.instance_name.find('.') {
                let type_name = &inst.instance_name[..dot];
                types.entry(type_name).or_default().push(inst);
            }
        }
        for (type_name, insts) in &types {
            let mut table = toml::map::Map::new();
            for inst in insts {
                let name = &inst.instance_name[type_name.len() + 1..];
                table.insert(name.to_string(), inst.config_toml.clone());
            }
            map.serialize_entry(type_name, &toml::Value::Table(table))?;
        }
        for (name, value) in &self.extra {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for ProcessorConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let table = toml::Value::deserialize(deserializer)?;
        Ok(ProcessorConfig::from_toml(&table))
    }
}

impl ProcessorConfig {
    /// Parse the `[processor]` table from TOML into instances.
    pub(crate) fn from_toml(value: &toml::Value) -> Self {
        let Some(table) = value.as_table() else {
            return Self::default();
        };

        let mut instances = Vec::new();
        let mut extra = HashMap::new();

        for (key, val) in table {
            // skip non-table entries
            let Some(sub_table) = val.as_table() else { continue };

            if is_builtin_type(key) {
                // Check if this is single-instance or multi-instance
                if Self::is_multi_instance(key, sub_table) {
                    // Multi-instance: [processor.pylint.core], [processor.pylint.tests]
                    for (name, inst_val) in sub_table {
                        let instance_name = format!("{key}.{name}");
                        let mut config = inst_val.clone();
                        let mut provenance = seed_user_provenance(&config);
                        resolve_instance_defaults(key, &mut config, &mut provenance);
                        instances.push(ProcessorInstance {
                            instance_name,
                            type_name: key.clone(),
                            config_toml: config,
                            provenance,
                        });
                    }
                } else {
                    // Single instance: [processor.pylint]
                    let mut config = val.clone();
                    let mut provenance = seed_user_provenance(&config);
                    resolve_instance_defaults(key, &mut config, &mut provenance);
                    instances.push(ProcessorInstance {
                        instance_name: key.clone(),
                        type_name: key.clone(),
                        config_toml: config,
                        provenance,
                    });
                }
            } else {
                // Unknown type — Lua plugin
                extra.insert(key.clone(), val.clone());
            }
        }

        Self { instances, extra }
    }

    /// Determine if a processor section contains named sub-instances (multi-instance)
    /// or direct config fields (single-instance).
    ///
    /// Heuristic: if ALL values are tables AND none of the keys match known config
    /// field names for this processor type, it's multi-instance.
    fn is_multi_instance(type_name: &str, table: &toml::map::Map<String, toml::Value>) -> bool {
        if table.is_empty() {
            return false;
        }

        let known = Self::known_fields_for(type_name);
        let known_fields: Vec<&str> = match known {
            Some(fields) => fields.iter()
                .chain(SCAN_CONFIG_FIELDS.iter())
                .chain(STANDARD_EXTRA_FIELDS.iter())
                .copied()
                .collect(),
            None => return false,
        };

        // If ANY key is a known config field, it's single-instance
        for key in table.keys() {
            if known_fields.contains(&key.as_str()) {
                return false;
            }
        }

        // If ALL values are tables, it's multi-instance
        table.values().all(toml::Value::is_table)
    }

    /// Resolve scan defaults for all instances.
    pub(crate) fn resolve_scan_defaults(&mut self) {
        for inst in &mut self.instances {
            resolve_instance_defaults(&inst.type_name, &mut inst.config_toml, &mut inst.provenance);
        }
    }

    /// Rewrite output paths in processor configs:
    /// 1. For named instances (e.g., `pylint.core`), remap default `out/{type_name}`
    ///    to `out/{instance_name}` so each instance gets its own output directory.
    /// 2. If the global `output_dir` is not "out", replace the `out/` prefix with the
    ///    global value (e.g., `build/` → `build/marp`).
    ///
    /// When a field was originally a ProcessorDefault and gets rewritten here, its
    /// provenance is upgraded to OutputDirDefault so `config show` can explain why
    /// the value differs from the processor's own default.
    pub(crate) fn apply_output_dir_defaults(&mut self, global_output_dir: &str) {
        for inst in &mut self.instances {
            let type_default_prefix = format!("out/{}", inst.type_name);
            let instance_prefix = format!("{}/{}", global_output_dir, inst.instance_name);

            for field in &["output_dir", "output"] {
                // A user-set or CLI-set value is explicitly chosen — never
                // rewrite it (previously only the provenance label was
                // guarded while the value itself was still rewritten).
                if matches!(
                    inst.provenance.get(*field),
                    Some(FieldProvenance::UserToml { .. } | FieldProvenance::LocalToml { .. } | FieldProvenance::CliOverride),
                ) {
                    continue;
                }
                let Some(val) = inst.config_toml.get(field).and_then(|v| v.as_str()).map(std::string::ToString::to_string) else { continue };
                // Prefix matches must respect path boundaries: `out/marp`
                // must not match `out/marpdeck/...`.
                let type_rest = val.strip_prefix(&type_default_prefix)
                    .filter(|r| r.is_empty() || r.starts_with('/'));
                let new_val = if inst.instance_name != inst.type_name
                    && let Some(rest) = type_rest {
                    // Named instance: remap out/{type} → {global}/{instance}
                    format!("{instance_prefix}{rest}")
                } else if global_output_dir != "out"
                    && let Some(rest) = val.strip_prefix("out/") {
                    // Global output dir override: remap out/ → {global}/
                    format!("{global_output_dir}/{rest}")
                } else {
                    continue;
                };
                if let Some(table) = inst.config_toml.as_table_mut() {
                    table.insert(field.to_string(), toml::Value::String(new_val));
                }
                inst.provenance.insert((*field).to_string(), FieldProvenance::OutputDirDefault);
            }
        }
    }

    /// Get the first instance of a given type (for single-instance access).
    /// Returns None if no instance of that type is declared.
    pub(crate) fn first_instance_of_type(&self, type_name: &str) -> Option<&ProcessorInstance> {
        self.instances.iter().find(|i| i.type_name == type_name)
    }

    /// Deserialize the config for `type_name`, whether or not the user
    /// declared the instance.
    ///
    /// A declared instance already has registry defaults baked into its
    /// `config_toml` by `Config::load`. An undeclared one has nothing, and
    /// `C::default()` leaves scan fields unresolved — reaching a scan
    /// accessor then panics with "scan fields not resolved". So the
    /// undeclared case is routed through the same `apply_all_defaults` the
    /// declared case used, instead of each caller hand-resolving.
    pub(crate) fn instance_config_or_default<C: serde::de::DeserializeOwned>(
        &self,
        type_name: &str,
    ) -> Result<C> {
        if let Some(inst) = self.first_instance_of_type(type_name) {
            return inst.config_toml.clone().try_into()
                .with_context(|| format!("Failed to parse [processor.{type_name}] config"));
        }
        let mut value = toml::Value::Table(toml::map::Map::new());
        let mut provenance = ProvenanceMap::new();
        crate::registries::processor::apply_all_defaults(type_name, &mut value, &mut provenance);
        value.try_into()
            .with_context(|| format!("Failed to build default config for processor '{type_name}'"))
    }

    /// Get a typed config value from an instance's TOML config.
    /// Returns the default if the instance is not declared or the field is missing.
    pub(crate) fn instance_field_str(&self, type_name: &str, field: &str) -> Option<String> {
        self.first_instance_of_type(type_name)
            .and_then(|inst| inst.config_toml.get(field))
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
    }
}


pub fn default_cc_compiler() -> String {
    "gcc".into()
}

pub fn default_cxx_compiler() -> String {
    "g++".into()
}

pub fn default_output_suffix() -> String {
    ".elf".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CompletionsConfig {
    #[serde(default = "default_shells")]
    pub shells: Vec<String>,
}

fn default_shells() -> Vec<String> {
    vec!["bash".into()]
}

impl Default for CompletionsConfig {
    fn default() -> Self {
        Self { shells: vec!["bash".into()] }
    }
}

/// A single analyzer instance parsed from the TOML config.
/// `[analyzer.cpp]` produces one instance with type_name="cpp", instance_name="cpp".
/// `[analyzer.cpp.kernel]` produces one with type_name="cpp", instance_name="cpp.kernel".
#[derive(Debug, Clone)]
pub struct AnalyzerInstance {
    /// Instance name: "cpp" for single, "cpp.kernel" for named
    pub instance_name: String,
    /// Analyzer type name (must match a registered AnalyzerPlugin name)
    pub type_name: String,
    /// The raw TOML config for this instance
    pub config_toml: toml::Value,
    /// Source of every field in `config_toml` (user TOML, serde default, …).
    pub provenance: ProvenanceMap,
}

/// Configuration for dependency analyzers.
/// Each `[analyzer.NAME]` section in rsconstruct.toml creates an AnalyzerInstance.
/// No analyzers run unless explicitly declared in the config.
#[derive(Debug, Default)]
pub struct AnalyzerConfig {
    /// All declared analyzer instances
    pub instances: Vec<AnalyzerInstance>,
}

impl Serialize for AnalyzerConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        // Emit single instances directly; group named sub-instances under the type.
        for inst in &self.instances {
            if inst.instance_name == inst.type_name {
                map.serialize_entry(&inst.instance_name, &inst.config_toml)?;
            }
        }
        let mut types: HashMap<&str, Vec<&AnalyzerInstance>> = HashMap::new();
        for inst in &self.instances {
            if inst.instance_name != inst.type_name {
                types.entry(inst.type_name.as_str()).or_default().push(inst);
            }
        }
        for (type_name, insts) in &types {
            let mut table = toml::map::Map::new();
            for inst in insts {
                let name = &inst.instance_name[type_name.len() + 1..];
                table.insert(name.to_string(), inst.config_toml.clone());
            }
            map.serialize_entry(type_name, &toml::Value::Table(table))?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for AnalyzerConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let table = toml::Value::deserialize(deserializer)?;
        AnalyzerConfig::from_toml(&table).map_err(serde::de::Error::custom)
    }
}

impl AnalyzerConfig {
    /// Parse the `[analyzer]` table from TOML into instances.
    ///
    /// Supports both single-instance and multi-instance syntax:
    /// - `[analyzer.cpp]` with config fields → one instance, instance_name="cpp"
    /// - `[analyzer.cpp.kernel]` + `[analyzer.cpp.userspace]` → two instances,
    ///   instance_name="cpp.kernel" and "cpp.userspace"
    pub(crate) fn from_toml(value: &toml::Value) -> Result<Self> {
        let Some(table) = value.as_table() else {
            return Ok(Self::default());
        };

        let mut instances = Vec::new();
        for (type_name, val) in table {
            if registry::find_analyzer_plugin(type_name).is_none() {
                anyhow::bail!("Unknown analyzer '{type_name}'. Run 'rsconstruct analyzers list' to see available analyzers.");
            }
            let Some(sub_table) = val.as_table() else {
                anyhow::bail!("Expected [analyzer.{type_name}] to be a table");
            };
            if Self::is_multi_instance(sub_table) {
                for (name, inst_val) in sub_table {
                    let provenance = seed_user_provenance(inst_val);
                    instances.push(AnalyzerInstance {
                        instance_name: format!("{type_name}.{name}"),
                        type_name: type_name.clone(),
                        config_toml: inst_val.clone(),
                        provenance,
                    });
                }
            } else {
                let provenance = seed_user_provenance(val);
                instances.push(AnalyzerInstance {
                    instance_name: type_name.clone(),
                    type_name: type_name.clone(),
                    config_toml: val.clone(),
                    provenance,
                });
            }
        }
        Ok(Self { instances })
    }

    /// Multi-instance iff the table is non-empty and every value is itself a
    /// table. Single-instance if any value is a scalar/array (i.e. a config field).
    fn is_multi_instance(table: &toml::map::Map<String, toml::Value>) -> bool {
        !table.is_empty() && table.values().all(toml::Value::is_table)
    }

}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct GraphConfig {
    #[serde(default)]
    pub viewer: Option<String>,
    /// Reject products with no input files. Default: true.
    #[serde(default = "default_true")]
    pub validate_empty_inputs: bool,
    /// Validate that dependency references point to existing products. Default: true.
    #[serde(default = "default_true")]
    pub validate_dep_references: bool,
    /// Warn when the same input file appears in multiple products of the same processor. Default: false.
    #[serde(default)]
    pub validate_duplicate_inputs: bool,
    /// Check for cycles immediately after resolving dependencies (rather than at topological sort time). Default: false.
    #[serde(default)]
    pub validate_early_cycles: bool,
}

/// Expected TOML type for a config field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldType {
    String,
    Bool,
    Integer,
    StringArray,
    /// Array of tables (e.g., [[processor.cc_single_file.compilers]])
    TableArray,
    /// Inline table (e.g., tags.field_types = { author = "string" })
    Table,
    /// Array with element types the validator doesn't constrain
    /// (e.g. tags.required_field_groups is an array of string arrays).
    Array,
}

impl FieldType {
    const fn label(self) -> &'static str {
        match self {
            FieldType::String => "a string",
            FieldType::Bool => "a boolean",
            FieldType::Integer => "an integer",
            FieldType::StringArray => "an array of strings",
            FieldType::TableArray => "an array of tables",
            FieldType::Table => "a table",
            FieldType::Array => "an array",
        }
    }

    /// Check whether a TOML value matches this expected type.
    fn matches(self, value: &toml::Value) -> bool {
        match self {
            FieldType::String => value.is_str(),
            FieldType::Bool => value.as_bool().is_some(),
            FieldType::Integer => value.is_integer(),
            FieldType::StringArray => value.as_array()
                .is_some_and(|arr| arr.iter().all(toml::Value::is_str)),
            FieldType::TableArray => value.as_array()
                .is_some_and(|arr| arr.iter().all(toml::Value::is_table)),
            FieldType::Table => value.is_table(),
            FieldType::Array => value.is_array(),
        }
    }

    const fn describe_value(value: &toml::Value) -> &'static str {
        match value {
            toml::Value::String(_) => "a string",
            toml::Value::Integer(_) => "an integer",
            toml::Value::Float(_) => "a float",
            toml::Value::Boolean(_) => "a boolean",
            toml::Value::Datetime(_) => "a datetime",
            toml::Value::Array(_) => "an array",
            toml::Value::Table(_) => "a table",
        }
    }
}

/// Return the expected TOML type for a processor config field.
/// Fields common to all processors (scan fields, enabled, args, dep_inputs)
/// are handled generically. Processor-specific fields are looked up by processor name.
fn expected_field_type(processor: &str, field: &str) -> Option<FieldType> {
    // Scan fields — shared by all processors
    match field {
        "src_dirs" => return Some(FieldType::StringArray),
        "src_extensions" => return Some(FieldType::StringArray),
        "src_exclude_dirs" => return Some(FieldType::StringArray),
        "src_exclude_files" => return Some(FieldType::StringArray),
        "src_exclude_paths" => return Some(FieldType::StringArray),
        "src_files" => return Some(FieldType::StringArray),
        // Common processor fields
        "args" => return Some(FieldType::StringArray),
        "dep_inputs" => return Some(FieldType::StringArray),
        "dep_auto" => return Some(FieldType::StringArray),
        "max_jobs" => return Some(FieldType::Integer),
        "enabled" => return Some(FieldType::Bool),
        "batch" => return Some(FieldType::Bool),
        // Near-universal fields from StandardConfig — declared by almost
        // every processor, so they are validated generically rather than
        // repeated in every processor-specific arm below.
        "command" => return Some(FieldType::String),
        "output_dir" => return Some(FieldType::String),
        "formats" => return Some(FieldType::StringArray),
        _ => {}
    }

    // Processor-specific fields
    match (processor, field) {
        // tera
        // tera has no processor-specific fields
        // ruff
        ("ruff", "command") => Some(FieldType::String),
        // cc_single_file
        ("cc_single_file", "cc" | "cxx" | "output_suffix") => Some(FieldType::String),
        ("cc_single_file", "cflags" | "cxxflags" | "ldflags" | "include_paths") => Some(FieldType::StringArray),
        ("cc_single_file", "include_scanner") => Some(FieldType::String),
        ("cc_single_file", "compilers") => Some(FieldType::TableArray),
        // cc (full project builds)
        ("cc", "cc" | "cxx") => Some(FieldType::String),
        ("cc", "cflags" | "cxxflags" | "ldflags" | "include_dirs") => Some(FieldType::StringArray),
        ("cc", "single_invocation" | "cache_output_dir") => Some(FieldType::Bool),
        // cppcheck — only common fields
        // clang_tidy
        ("clang_tidy", "compiler_args") => Some(FieldType::StringArray),
        // zspell
        ("zspell", "language" | "words_file") => Some(FieldType::String),
        ("zspell", "auto_add_words") => Some(FieldType::Bool),
        // make
        ("make", "command" | "target") => Some(FieldType::String),
        // cargo / clippy
        ("cargo", "profiles") => Some(FieldType::StringArray),
        ("cargo" | "clippy", "cargo" | "command") => Some(FieldType::String),
        // mypy, pyrefly, shellcheck, rumdl, yamllint, jq, jsonlint, taplo
        ("mypy" | "pyrefly" | "shellcheck" | "luacheck" | "script", "checker") => Some(FieldType::String),
        ("rumdl" | "yamllint" | "jsonlint" | "taplo", "linter") => Some(FieldType::String),
        ("jq", "checker") => Some(FieldType::String),
        // tags
        ("tags", "output" | "tags_dir") => Some(FieldType::String),
        // pip
        ("pip", "command") => Some(FieldType::String),
        // sphinx
        ("sphinx", "command" | "output_dir" | "working_dir") => Some(FieldType::String),
        // mdbook
        ("mdbook", "command" | "output_dir") => Some(FieldType::String),
        // npm
        ("npm", "command") => Some(FieldType::String),
        // gem
        ("gem", "command" | "gem_home") => Some(FieldType::String),
        // mdl
        ("mdl", "gem_home" | "command" | "gem_stamp") => Some(FieldType::String),
        ("mdl", "local_repo") => Some(FieldType::Bool),
        // aspell
        ("aspell", "command" | "conf" | "words_file") => Some(FieldType::String),
        ("aspell", "auto_add_words") => Some(FieldType::Bool),
        // marp
        ("marp", "timeout_secs" | "max_attempts") => Some(FieldType::Integer),
        // pandoc
        ("pandoc", "pdf_engine") => Some(FieldType::String),
        // pdflatex
        ("pdflatex", "runs") => Some(FieldType::Integer),
        ("pdflatex", "qpdf" | "shell_escape") => Some(FieldType::Bool),
        // a2x
        ("a2x", "a2x") => Some(FieldType::String),
        // pdfunite / ipdfunite
        ("pdfunite" | "ipdfunite", "source_dir" | "source_ext" | "source_output_dir") => Some(FieldType::String),
        // cache_output_dir — shared by creators
        ("cargo" | "sphinx" | "mdbook" | "npm" | "gem", "cache_output_dir") => Some(FieldType::Bool),
        // creator / explicit — declared output sets
        ("creator" | "explicit", "output_dirs" | "output_files") => Some(FieldType::StringArray),
        ("explicit", "inputs" | "input_globs") => Some(FieldType::StringArray),
        // generator
        ("generator", "output_extension") => Some(FieldType::String),
        // rust_single_file
        ("rust_single_file", "flags") => Some(FieldType::StringArray),
        ("rust_single_file", "output_suffix") => Some(FieldType::String),
        // script — config-declared fix capability
        ("script", "fix_command") => Some(FieldType::String),
        ("script", "fix_args") => Some(FieldType::StringArray),
        ("script", "fix_batch") => Some(FieldType::Bool),
        // iyamlschema
        ("iyamlschema", "check_ordering") => Some(FieldType::Bool),
        // license_header
        ("license_header", "header_lines") => Some(FieldType::StringArray),
        // zspell
        ("zspell", "dict_dir") => Some(FieldType::String),
        // requirements
        ("requirements", "output") => Some(FieldType::String),
        ("requirements", "exclude" | "extra" | "python_paths") => Some(FieldType::StringArray),
        ("requirements", "header" | "sorted") => Some(FieldType::Bool),
        ("requirements", "mapping") => Some(FieldType::Table),
        // terms
        ("terms", "dir_terms_ambiguous" | "dir_terms_unambiguous") => Some(FieldType::String),
        ("terms", "forbid_backticked_ambiguous") => Some(FieldType::Bool),
        // tags
        ("tags", "required_fields" | "required_values" | "unique_fields") => Some(FieldType::StringArray),
        ("tags", "field_types") => Some(FieldType::Table),
        ("tags", "required_field_groups") => Some(FieldType::Array),
        ("tags", "sorted_tags" | "check_unused") => Some(FieldType::Bool),
        ("tags", "common_tags_limit" | "similar_files_limit" | "suggested_tags_limit") => Some(FieldType::Integer),
        _ => None,
    }
}

/// Validate fields in a single processor config table.
fn validate_single_processor(
    type_name: &str,
    section_label: &str,
    table: &toml::map::Map<String, toml::Value>,
    errors: &mut Vec<String>,
) {
    let own_fields: &[&str] = match ProcessorConfig::known_fields_for(type_name) {
        Some(fields) => fields,
        None => return, // unknown = Lua plugin, skip
    };

    // A zero-permit semaphore can never grant a permit: max_jobs = 0 would
    // deadlock the build waiting forever, so reject it at load time.
    if table.get("max_jobs").and_then(toml::Value::as_integer) == Some(0) {
        errors.push(format!(
            "[{section_label}]: 'max_jobs' must be greater than 0 (use 'enabled = false' to turn the processor off)",
        ));
    }

    for (key, field_value) in table {
        if !own_fields.contains(&key.as_str())
            && !SCAN_CONFIG_FIELDS.contains(&key.as_str())
            && !STANDARD_EXTRA_FIELDS.contains(&key.as_str())
        {
            let all_fields: Vec<&str> = own_fields.iter()
                .chain(SCAN_CONFIG_FIELDS.iter())
                .chain(STANDARD_EXTRA_FIELDS.iter())
                .copied()
                .collect();
            errors.push(format!(
                "[{}]: unknown field '{}' (valid fields: {})",
                section_label, key, all_fields.join(", ")
            ));
            continue;
        }

        if let Some(expected) = expected_field_type(type_name, key)
            && !expected.matches(field_value)
        {
            errors.push(format!(
                "[{}]: field '{}' must be {}, got {} ({})",
                section_label, key, expected.label(),
                FieldType::describe_value(field_value),
                field_value,
            ));
        }
    }

    // Check that all must_fields are present and non-empty
    if let Some(must) = ProcessorConfig::must_fields_for(type_name) {
        for field in must {
            match table.get(*field) {
                None => {
                    errors.push(format!(
                        "[{section_label}]: required field '{field}' must be specified",
                    ));
                }
                Some(toml::Value::Array(arr)) if arr.is_empty() => {
                    errors.push(format!(
                        "[{section_label}]: required field '{field}' must not be empty",
                    ));
                }
                Some(toml::Value::String(s)) if s.is_empty() => {
                    errors.push(format!(
                        "[{section_label}]: required field '{field}' must not be empty",
                    ));
                }
                _ => {} // present and non-empty: OK
            }
        }
    }

    // Require src_dirs for processors whose default would scan the project root.
    // This prevents accidentally scanning everything when the user forgets to set src_dirs.
    // Exempt processors that don't use src_dirs for file discovery,
    // and processors that specify src_files (files are explicitly listed).
    const SCAN_DIRS_EXEMPT: &[&str] = &["explicit", "pdfunite", "ipdfunite"];
    let has_match_paths = table.get("src_files")
        .and_then(|v| v.as_array())
        .is_some_and(|arr| !arr.is_empty());
    if ProcessorConfig::default_src_dirs_for(type_name).is_some_and(<[&str]>::is_empty)
    && !SCAN_DIRS_EXEMPT.contains(&type_name)
    && !has_match_paths {
        match table.get("src_dirs") {
            None => {
                errors.push(format!(
                    "[{section_label}]: 'src_dirs' must be specified (this processor defaults to scanning the project root)",
                ));
            }
            Some(toml::Value::Array(arr)) if arr.is_empty() => {
                errors.push(format!(
                    "[{section_label}]: 'src_dirs' must not be empty; specify actual directories to scan",
                ));
            }
            Some(toml::Value::Array(arr))
                if arr.iter().any(|v| v.as_str().is_some_and(str::is_empty)) =>
            {
                errors.push(format!(
                    "[{section_label}]: 'src_dirs' must not contain empty strings; specify actual directories to scan",
                ));
            }
            _ => {} // present and non-empty: OK
        }
    }

}

/// Validate that all fields in `[processor.X]` sections are known fields for that processor
/// and have the correct TOML types. Supports both single-instance and multi-instance formats.
/// Returns a list of error strings (empty if valid). `Config::load` combines this with the
/// analyzer validator output under a single "Invalid config:" header.
fn validate_processor_fields_raw(raw: &toml::Value) -> Vec<String> {
    let Some(processor_table) = raw.get("processor").and_then(|v| v.as_table()) else {
        return Vec::new();
    };

    let mut errors = Vec::new();

    for (name, value) in processor_table {
        // A scalar under [processor] (e.g. `ruff = true`) is a config mistake
        // that would otherwise be silently dropped.
        let Some(table) = value.as_table() else {
            errors.push(format!(
                "[processor]: '{name}' must be a section (e.g. [processor.{name}]), got {}",
                FieldType::describe_value(value),
            ));
            continue;
        };

        if !is_builtin_type(name) {
            // Check if there's a matching Lua plugin file
            let plugins_dir = raw.get("plugins")
                .and_then(|p| p.get("dir"))
                .and_then(|d| d.as_str())
                .unwrap_or(DEFAULT_PLUGINS_DIR);
            let plugin_path = std::path::Path::new(plugins_dir)
                .join(format!("{name}.lua"));
            if !plugin_path.exists() {
                errors.push(format!(
                    "[processor.{}]: unknown processor type '{}' (not a builtin processor or Lua plugin at {})",
                    name, name, plugin_path.display(),
                ));
            }
            continue;
        }

        // Check if multi-instance
        if ProcessorConfig::is_multi_instance(name, table) {
            for (inst_name, inst_value) in table {
                if let Some(inst_table) = inst_value.as_table() {
                    let section = format!("processor.{name}.{inst_name}");
                    validate_single_processor(name, &section, inst_table, &mut errors);
                }
            }
        } else {
            let section = format!("processor.{name}");
            validate_single_processor(name, &section, table, &mut errors);
        }
    }

    errors
}

/// Validate `[analyzer.X]` sections: reject unknown analyzer types and unknown
/// fields within each section. Runs at config-load time, before any analyzer
/// is instantiated, so users see schema errors up front instead of at build
/// time. Returns a list of error strings (empty if valid). `Config::load`
/// combines this with the processor validator output under a single
/// "Invalid config:" header.
fn validate_analyzer_fields_raw(raw: &toml::Value) -> Vec<String> {
    let Some(analyzer_table) = raw.get("analyzer").and_then(|v| v.as_table()) else {
        return Vec::new();
    };

    let mut errors = Vec::new();

    for (type_name, value) in analyzer_table {
        let Some(table) = value.as_table() else {
            errors.push(format!("[analyzer.{type_name}]: expected a table"));
            continue;
        };

        let Some(plugin) = registry::find_analyzer_plugin(type_name) else {
            errors.push(format!(
                "[analyzer.{type_name}]: unknown analyzer type '{type_name}' (run 'rsconstruct analyzers list' to see available)",
            ));
            continue;
        };

        // Multi-instance: `[analyzer.cpp.kernel]` / `[analyzer.cpp.userspace]`.
        // Detected iff every value is itself a table.
        let is_multi_instance = !table.is_empty() && table.values().all(toml::Value::is_table);

        if is_multi_instance {
            for (inst_name, inst_value) in table {
                if let Some(inst_table) = inst_value.as_table() {
                    let section = format!("analyzer.{type_name}.{inst_name}");
                    validate_analyzer_section(plugin, &section, inst_table, &mut errors);
                }
            }
        } else {
            let section = format!("analyzer.{type_name}");
            validate_analyzer_section(plugin, &section, table, &mut errors);
        }
    }

    errors
}

/// Check a single analyzer section's fields against the plugin's known_fields list.
fn validate_analyzer_section(
    plugin: &registry::AnalyzerPlugin,
    section_label: &str,
    table: &toml::map::Map<String, toml::Value>,
    errors: &mut Vec<String>,
) {
    let known = (plugin.known_fields)();
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            errors.push(format!(
                "[{}]: unknown field '{}' (valid fields: {})",
                section_label, key, known.join(", ")
            ));
        }
    }
}

/// Read a config file and apply `[vars]` substitution to its content.
/// Substitution is per-file: each file's `${...}` references resolve against
/// its own `[vars]` section only.
fn read_and_substitute(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read config file: {}", path.display()))?;
    substitute_variables(&content)
        .map_err(|e| crate::exit_code::config_error(
            format!("Failed to substitute variables in {}: {e:#}", path.display())))
}

/// Deep-merge `overlay` into `base`: tables merge recursively, while arrays
/// and scalars from the overlay replace the base value wholesale. Keys only
/// present in the overlay are added.
fn merge_toml_values(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_table), toml::Value::Table(overlay_table)) => {
            for (key, overlay_val) in overlay_table {
                match base_table.get_mut(&key) {
                    Some(base_val) if base_val.is_table() && overlay_val.is_table() => {
                        merge_toml_values(base_val, overlay_val);
                    }
                    _ => {
                        base_table.insert(key, overlay_val);
                    }
                }
            }
        }
        (base_val, overlay_val) => *base_val = overlay_val,
    }
}

impl Config {
    pub(crate) fn require_config() -> Result<()> {
        let config_path = Path::new(CONFIG_FILE);
        if !config_path.exists() {
            let message = if Path::new(LOCAL_CONFIG_FILE).exists() {
                format!("{LOCAL_CONFIG_FILE} found without {CONFIG_FILE} — the local overlay only extends a main config file. Run 'rsconstruct init' to create one.")
            } else {
                "No rsconstruct.toml found. Run 'rsconstruct init' to create one.".to_string()
            };
            return Err(crate::exit_code::RsconstructError::new(
                crate::exit_code::RsconstructExitCode::ConfigError,
                message,
            ).into());
        }
        Ok(())
    }

    pub(crate) fn load() -> Result<Self> {
        let config_path = Path::new(CONFIG_FILE);
        let local_path = Path::new(LOCAL_CONFIG_FILE);

        let (mut config, span_map, global_span_map, local_span_map, local_global_span_map) = if config_path.exists() {
            let substituted = read_and_substitute(config_path)?;
            let mut raw: toml::Value = toml::from_str(&substituted)
                .map_err(|e| crate::exit_code::config_error(
                    format!("Failed to parse config file {}: {e}", config_path.display())))?;
            // Overlay: rsconstruct.local.toml, when present, is deep-merged
            // over the main config. Tables merge recursively; arrays and
            // scalars from the local file replace the main file's values.
            // `[vars]` substitution is per-file: each file's `${...}`
            // references resolve against its own `[vars]` section only.
            let local_substituted = if local_path.exists() {
                let local_content = read_and_substitute(local_path)?;
                let local_raw: toml::Value = toml::from_str(&local_content)
                    .map_err(|e| crate::exit_code::config_error(
                        format!("Failed to parse config file {}: {e}", local_path.display())))?;
                merge_toml_values(&mut raw, local_raw);
                Some(local_content)
            } else {
                None
            };
            // Run both schema validators before serde sees the config, so users
            // get pretty per-section errors instead of serde's raw messages.
            // Errors from both validators are surfaced together under a single
            // "Invalid config:" header.
            let mut all_errors = validate_processor_fields_raw(&raw);
            all_errors.extend(validate_analyzer_fields_raw(&raw));
            if !all_errors.is_empty() {
                return Err(crate::exit_code::config_error(
                    format!("Invalid config:\n{}", all_errors.join("\n"))));
            }
            let config: Config = raw.try_into()
                .map_err(|e| crate::exit_code::config_error(
                    format!("Failed to parse config file {}: {e}", config_path.display())))?;
            // Capture byte-level spans from the substituted sources so we can
            // report user-set fields as `rsconstruct.toml:<line>` (or
            // `rsconstruct.local.toml:<line>`) instead of the sentinel
            // `line: 0` we seeded during deserialization.
            let spans = provenance::build_span_map(&substituted);
            let global_spans = provenance::build_global_span_map(&substituted);
            let (local_spans, local_global_spans) = match &local_substituted {
                Some(content) => (provenance::build_span_map(content), provenance::build_global_span_map(content)),
                None => (SpanMap::new(), provenance::GlobalSpanMap::new()),
            };
            (config, spans, global_spans, local_spans, local_global_spans)
        } else {
            if local_path.exists() {
                anyhow::bail!(
                    "{LOCAL_CONFIG_FILE} found without {CONFIG_FILE} — the local overlay only extends a main config file",
                );
            }
            (Config::default(), SpanMap::new(), provenance::GlobalSpanMap::new(), SpanMap::new(), provenance::GlobalSpanMap::new())
        };
        config.processor.resolve_scan_defaults();
        config.processor.apply_output_dir_defaults(&config.build.output_dir);
        config.apply_span_map(&span_map, &local_span_map);
        config.populate_global_provenance(&global_span_map, &local_global_span_map)?;
        crate::phases::run_post_config_hooks(&mut config)?;
        Ok(config)
    }

    /// Apply CLI-level config overrides from `--iset` and `--pset` flags.
    ///
    /// `iset` entries match a single processor instance by `iname` (the section
    /// name in `[processor.<iname>]`). `pset` entries match every instance whose
    /// processor type (`pname`) equals the prefix.
    ///
    /// Each entry has the form `<name>.<field>=<value>`. The value is parsed as
    /// a TOML scalar/array/table; if it fails to parse, it is treated as a bare
    /// string. The resolved value's TOML type must match the field's declared
    /// type (e.g. `max_jobs` must be an integer).
    ///
    /// Errors hard on: malformed entry, unknown iname/pname, no matching
    /// instances for a pname, unknown field for the processor type, and
    /// type mismatch.
    pub(crate) fn apply_overrides(&mut self, iset: &[String], pset: &[String]) -> Result<()> {
        for raw in iset {
            let (iname, field, value) = parse_override_entry(raw, "--iset")?;
            apply_override_to_instances(&mut self.processor.instances, field, &value, |inst| {
                inst.instance_name == iname
            }, "iname", iname)?;
        }
        for raw in pset {
            let (pname, field, value) = parse_override_entry(raw, "--pset")?;
            apply_override_to_instances(&mut self.processor.instances, field, &value, |inst| {
                inst.type_name == pname
            }, "pname", pname)?;
        }

        // Overrides bypass load-time validation (it runs on the raw TOML,
        // before overrides exist), so the semantic rules — the max_jobs > 0
        // deadlock guard, must_fields, the src_dirs rule — must be re-checked
        // against the effective config. Otherwise `--iset x.max_jobs=0`
        // hangs the build forever.
        let mut errors = Vec::new();
        for inst in &self.processor.instances {
            if let Some(table) = inst.config_toml.as_table() {
                let section_label = format!("processor.{}", inst.instance_name);
                validate_single_processor(&inst.type_name, &section_label, table, &mut errors);
            }
        }
        if !errors.is_empty() {
            return Err(crate::exit_code::config_error(
                format!("Invalid config after CLI overrides:\n{}", errors.join("\n"))));
        }
        Ok(())
    }

    /// Seed `global_provenance` for every top-level section. Every field
    /// listed in the span map is `UserToml { line }`; every other field of
    /// the section — discovered by serializing the section struct and reading
    /// its keys — is `SerdeDefault`.
    fn populate_global_provenance(
        &mut self,
        global_spans: &provenance::GlobalSpanMap,
        local_global_spans: &provenance::GlobalSpanMap,
    ) -> Result<()> {
        // Serialize the whole config once so we can walk each section's
        // effective keys without reaching into every section struct.
        let serialized = toml::Value::try_from(&*self)
            .context("Failed to serialize config for global provenance walk")?;
        let Some(root) = serialized.as_table() else { return Ok(()) };
        for (section_name, section_value) in root {
            // Skip processor/analyzer — those are tracked per-instance in the
            // instances' own provenance maps.
            if section_name == "processor" || section_name == "analyzer" {
                continue;
            }
            let Some(section_table) = section_value.as_table() else { continue };
            let user_fields = global_spans.get(section_name);
            let local_fields = local_global_spans.get(section_name);
            let mut map = ProvenanceMap::new();
            for field in section_table.keys() {
                // The local overlay wins the merge, so a field set in both
                // files is attributed to rsconstruct.local.toml.
                let source = if let Some(&line) = local_fields.and_then(|f| f.get(field)) {
                    FieldProvenance::LocalToml { line }
                } else if let Some(&line) = user_fields.and_then(|f| f.get(field)) {
                    FieldProvenance::UserToml { line }
                } else {
                    FieldProvenance::SerdeDefault
                };
                map.insert(field.clone(), source);
            }
            self.global_provenance.insert(section_name.clone(), map);
        }
        Ok(())
    }

    /// Replace sentinel `UserToml { line: 0 }` entries with real line numbers
    /// from the toml_edit pass. Any user-set field that didn't get a span
    /// stays at line 0 (fine — the `config show` formatter falls back to
    /// "from rsconstruct.toml" without a line number).
    fn apply_span_map(&mut self, spans: &SpanMap, local_spans: &SpanMap) {
        for inst in &mut self.processor.instances {
            apply_spans_to_instance(&mut inst.provenance, spans, local_spans, Section::Processor, &inst.instance_name);
        }
        for inst in &mut self.analyzer.instances {
            apply_spans_to_instance(&mut inst.provenance, spans, local_spans, Section::Analyzer, &inst.instance_name);
        }
    }
}

fn apply_spans_to_instance(
    provenance: &mut ProvenanceMap,
    spans: &SpanMap,
    local_spans: &SpanMap,
    section: Section,
    instance_name: &str,
) {
    let keys: Vec<String> = provenance.keys().cloned().collect();
    for key in keys {
        if let Some(FieldProvenance::UserToml { line }) = provenance.get(&key) {
            if *line != 0 {
                continue; // already enriched
            }
            let span_key = (section, instance_name.to_string(), key.clone());
            // The local overlay wins the merge, so a field set in both files
            // is attributed to rsconstruct.local.toml.
            if let Some(&real_line) = local_spans.get(&span_key) {
                provenance.insert(key, FieldProvenance::LocalToml { line: real_line });
            } else if let Some(&real_line) = spans.get(&span_key) {
                provenance.insert(key, FieldProvenance::UserToml { line: real_line });
            }
        }
    }
}

/// Parse a single `--iset`/`--pset` entry of the form `<name>.<field>=<value>`.
/// Returns `(name, field, parsed_value)`. The value is parsed as a TOML scalar/
/// array/table; if parsing fails it is treated as a bare string. Hard-errors on
/// missing dot, missing equals, or empty name/field.
fn parse_override_entry<'a>(raw: &'a str, flag: &str) -> Result<(&'a str, &'a str, toml::Value)> {
    let (lhs, value_str) = raw.split_once('=').ok_or_else(|| anyhow::anyhow!(
        "{flag} '{raw}': missing '=' (expected <name>.<field>=<value>)"
    ))?;
    // Split on the LAST dot: instance names may contain dots (`pylint.core`)
    // while field names never do.
    let (name, field) = lhs.rsplit_once('.').ok_or_else(|| anyhow::anyhow!(
        "{flag} '{raw}': missing '.' between name and field (expected <name>.<field>=<value>)"
    ))?;
    if name.is_empty() {
        anyhow::bail!("{flag} '{raw}': empty name before '.'");
    }
    if field.is_empty() {
        anyhow::bail!("{flag} '{raw}': empty field between '.' and '='");
    }
    // Parse as a TOML value via a synthetic "v = <value>" doc; fall back to a
    // bare string so users can write `--iset marp.command=marp` without quoting.
    let parsed: toml::Value = match toml::from_str::<toml::Value>(&format!("v = {value_str}")) {
        Ok(toml::Value::Table(mut t)) => t.remove("v").unwrap_or_else(|| toml::Value::String(value_str.to_string())),
        _ => toml::Value::String(value_str.to_string()),
    };
    Ok((name, field, parsed))
}

/// Apply an override to every instance matching `predicate`. Hard-errors when
/// no instances match, when the field is unknown for the processor type, or
/// when the value's TOML type doesn't match the field's declared type.
fn apply_override_to_instances(
    instances: &mut [ProcessorInstance],
    field: &str,
    value: &toml::Value,
    predicate: impl Fn(&ProcessorInstance) -> bool,
    name_kind: &str,
    name: &str,
) -> Result<()> {
    let matching_indices: Vec<usize> = instances.iter().enumerate()
        .filter(|(_, inst)| predicate(inst))
        .map(|(i, _)| i)
        .collect();
    if matching_indices.is_empty() {
        anyhow::bail!("no processor instance with {name_kind} '{name}'");
    }
    for i in matching_indices {
        let inst = &mut instances[i];
        let type_name = inst.type_name.clone();
        validate_override_field(&type_name, field, value, &inst.instance_name)?;
        if let Some(table) = inst.config_toml.as_table_mut() {
            table.insert(field.to_string(), value.clone());
            inst.provenance.insert(field.to_string(), FieldProvenance::CliOverride);
        } else {
            anyhow::bail!(
                "instance '{}' config is not a table (cannot apply override)",
                inst.instance_name
            );
        }
    }
    Ok(())
}

/// Validate that `field` is a known config field for `type_name` and that
/// `value`'s TOML type matches the field's declared type.
fn validate_override_field(
    type_name: &str,
    field: &str,
    value: &toml::Value,
    instance_label: &str,
) -> Result<()> {
    let own_fields = ProcessorConfig::known_fields_for(type_name).unwrap_or(&[]);
    let is_known = own_fields.contains(&field)
        || SCAN_CONFIG_FIELDS.contains(&field)
        || STANDARD_EXTRA_FIELDS.contains(&field);
    if !is_known {
        let mut all_fields: Vec<&str> = own_fields.iter()
            .chain(SCAN_CONFIG_FIELDS.iter())
            .chain(STANDARD_EXTRA_FIELDS.iter())
            .copied()
            .collect();
        all_fields.sort_unstable();
        all_fields.dedup();
        anyhow::bail!(
            "instance '{instance_label}' (type {type_name}): unknown field '{field}' (valid fields: {})",
            all_fields.join(", ")
        );
    }
    if let Some(expected) = expected_field_type(type_name, field)
        && !expected.matches(value)
    {
        anyhow::bail!(
            "instance '{instance_label}' (type {type_name}): field '{field}' must be {}, got {} ({value})",
            expected.label(),
            FieldType::describe_value(value),
        );
    }
    Ok(())
}

/// Extract a `StandardConfig` with scan fields from a dynamic TOML table (used by Lua plugins).
/// Falls back to the given defaults for any missing scan fields.
pub fn standard_config_from_toml(
    value: &toml::Value,
    default_src_dirs: &[&str],
    default_src_extensions: &[&str],
    default_exclude_dirs: &[&str],
) -> StandardConfig {
    let table = value.as_table();

    let toml_array = |key: &str| -> Option<Vec<String>> {
        table
            .and_then(|t| t.get(key))
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
    };

    let mut cfg = StandardConfig {
        src_dirs: toml_array("src_dirs"),
        src_extensions: toml_array("src_extensions"),
        src_exclude_dirs: toml_array("src_exclude_dirs"),
        src_exclude_files: toml_array("src_exclude_files"),
        src_exclude_paths: toml_array("src_exclude_paths"),
        src_files: toml_array("src_files"),
        ..StandardConfig::default()
    };
    // Fill defaults for None fields
    if cfg.src_dirs.is_none() {
        cfg.src_dirs = Some(default_src_dirs.iter().map(std::string::ToString::to_string).collect());
    }
    if cfg.src_extensions.is_none() {
        cfg.src_extensions = Some(default_src_extensions.iter().map(std::string::ToString::to_string).collect());
    }
    if cfg.src_exclude_dirs.is_none() {
        cfg.src_exclude_dirs = Some(default_exclude_dirs.iter().map(std::string::ToString::to_string).collect());
    }
    if cfg.src_exclude_files.is_none() {
        cfg.src_exclude_files = Some(Vec::new());
    }
    if cfg.src_exclude_paths.is_none() {
        cfg.src_exclude_paths = Some(Vec::new());
    }
    if cfg.src_files.is_none() {
        cfg.src_files = Some(Vec::new());
    }
    cfg
}

/// Post-config hook: when `CI=true` is in the environment, enable the
/// `eatmydata` wrap for `tools install` / `tools install-deps`. The
/// trade-off (loss-on-power-cut for a 3-10× speedup) is right for
/// transient CI hosts and wrong for developer workstations, so we tie
/// the policy to `CI=true`. Users who want a different policy adjust
/// the env var.
#[allow(clippy::unnecessary_wraps)] // Result<()> required by PhaseHook::run signature.
fn eatmydata_ci_default(config: &mut Config) -> anyhow::Result<()> {
    if std::env::var("CI").is_ok_and(|v| v == "true") {
        config.dependencies.eatmydata = true;
    }
    Ok(())
}

inventory::submit! { crate::phases::PhaseHook {
    name: "eatmydata_ci_default",
    description: "When CI=true, enable eatmydata wrapping for apt/dnf/pacman installs",
    function: concat!(module_path!(), "::eatmydata_ci_default"),
    location: concat!(file!(), ":", line!()),
    run: eatmydata_ci_default,
} }
