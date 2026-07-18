use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{default_true, default_cc_compiler, default_cxx_compiler, default_output_suffix, KnownFields, ScanDefaultsData};

/// Universal processor config with all standard fields.
/// Checkers, generators, and simple processors all use this.
/// Fields not relevant to a given processor type are simply ignored.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct StandardConfig {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub formats: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub dep_inputs: Vec<String>,
    #[serde(default)]
    pub dep_auto: Vec<String>,
    #[serde(default)]
    pub output_dir: String,
    #[serde(default = "default_true")]
    pub batch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_jobs: Option<usize>,
    /// Whether to cache this processor's outputs. Default true.
    /// Set to false to always rebuild and never store results.
    #[serde(default = "default_true")]
    pub cache: bool,
    /// Whether this processor is active. Set to false to disable without
    /// removing the stanza from rsconstruct.toml.
    #[serde(default = "default_true")]
    pub enabled: bool,
    // --- Scan fields (file discovery) ---
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_dirs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_extensions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_exclude_dirs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_exclude_files: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_exclude_paths: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_files: Option<Vec<String>>,
}

impl Default for StandardConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            formats: Vec::new(),
            args: Vec::new(),
            dep_inputs: Vec::new(),
            dep_auto: Vec::new(),
            output_dir: String::new(),
            batch: true,
            max_jobs: None,
            cache: true,
            enabled: true,
            src_dirs: None,
            src_extensions: None,
            src_exclude_dirs: None,
            src_exclude_files: None,
            src_exclude_paths: None,
            src_files: None,
        }
    }
}

impl StandardConfig {
    /// Fill in None scan fields from scan defaults data.
    pub(crate) fn resolve_scan(&mut self, defaults: &ScanDefaultsData) {
        if self.src_dirs.is_none() {
            self.src_dirs = Some(defaults.src_dirs.iter().map(std::string::ToString::to_string).collect());
        }
        if self.src_extensions.is_none() {
            self.src_extensions = Some(defaults.src_extensions.iter().map(std::string::ToString::to_string).collect());
        }
        if self.src_exclude_dirs.is_none() {
            self.src_exclude_dirs = Some(defaults.src_exclude_dirs.iter().map(std::string::ToString::to_string).collect());
        }
        if self.src_exclude_files.is_none() {
            self.src_exclude_files = Some(Vec::new());
        }
        if self.src_exclude_paths.is_none() {
            self.src_exclude_paths = Some(Vec::new());
        }
        if self.src_files.is_none() {
            self.src_files = Some(Vec::new());
        }
    }

    pub(crate) fn src_dirs(&self) -> &[String] {
        self.src_dirs.as_deref().expect(crate::errors::SCAN_CONFIG_NOT_RESOLVED)
    }
    pub(crate) fn src_extensions(&self) -> &[String] {
        self.src_extensions.as_deref().expect(crate::errors::SCAN_CONFIG_NOT_RESOLVED)
    }
    pub(crate) fn src_exclude_dirs(&self) -> &[String] {
        self.src_exclude_dirs.as_deref().expect(crate::errors::SCAN_CONFIG_NOT_RESOLVED)
    }
    pub(crate) fn src_exclude_files(&self) -> &[String] {
        self.src_exclude_files.as_deref().expect(crate::errors::SCAN_CONFIG_NOT_RESOLVED)
    }
    pub(crate) fn src_exclude_paths(&self) -> &[String] {
        self.src_exclude_paths.as_deref().expect(crate::errors::SCAN_CONFIG_NOT_RESOLVED)
    }
    pub(crate) fn src_files(&self) -> &[String] {
        self.src_files.as_deref().expect(crate::errors::SCAN_CONFIG_NOT_RESOLVED)
    }

    /// Return the command string, or error with context if it was never set.
    pub(crate) fn require_command(&self, context: &str) -> anyhow::Result<&str> {
        if self.command.is_empty() {
            anyhow::bail!("'command' is not set for processor '{context}'");
        }
        Ok(&self.command)
    }
}

impl KnownFields for StandardConfig {
    fn known_fields() -> &'static [&'static str] {
        // Note: "enabled" and "cache" are universal — declared once in
        // STANDARD_EXTRA_FIELDS and merged in by the validator, not repeated here.
        &["command", "formats", "args", "dep_inputs", "dep_auto", "output_dir", "batch", "max_jobs"]
    }
    fn checksum_fields() -> &'static [&'static str] {
        // formats and output_dir are excluded: format is encoded as a per-product
        // variant in the cache key, and output_dir is encoded in the product's
        // declared outputs path — both would double-count if hashed here.
        &["command", "args"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        // "enabled" and "cache" descriptions live in SHARED_FIELD_DESCRIPTIONS.
        &[
            ("command",    "Path to the tool executable"),
            ("formats",    "Output formats to generate"),
            ("args",       "Extra arguments passed to the tool"),
            ("output_dir", "Directory where generated output files are written"),
        ]
    }
}

/// Simple checker config. No custom fields.
/// Unused StandardConfig fields: formats, output_dir.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct CheckerConfig {
    #[serde(flatten)]
    pub standard: StandardConfig,
}
impl KnownFields for CheckerConfig {
    fn known_fields() -> &'static [&'static str] { StandardConfig::known_fields() }
    fn checksum_fields() -> &'static [&'static str] { StandardConfig::checksum_fields() }
    fn must_fields() -> &'static [&'static str] { StandardConfig::must_fields() }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] { StandardConfig::field_descriptions() }
}

/// Alias for CheckerConfig (used by SimpleChecker).
pub type CheckerConfigWithCommand = CheckerConfig;

/// Config for Creator processors — run a command and cache declared outputs.
#[derive(Debug, Deserialize, Serialize, Clone)]
/// Creator processor config.
/// Custom fields: output_dirs, output_files.
/// Unused StandardConfig fields: formats, output_dir.
#[derive(Default)]
pub struct CreatorConfig {
    /// Directories to cache after the command runs.
    #[serde(default)]
    pub output_dirs: Vec<String>,
    /// Individual files to cache after the command runs.
    #[serde(default)]
    pub output_files: Vec<String>,
    #[serde(flatten)]
    pub standard: StandardConfig,
}


impl KnownFields for CreatorConfig {
    fn known_fields() -> &'static [&'static str] {
        &["command", "args", "dep_inputs", "dep_auto", "output_dirs", "output_files", "batch", "max_jobs"]
    }
    fn checksum_fields() -> &'static [&'static str] {
        // output_dirs/output_files are excluded: the output paths themselves are
        // already part of the product's identity, not its config-change checksum.
        &["command", "args"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command",      "Command to run"),
            ("args",         "Extra arguments passed to the command"),
            ("output_dirs",  "Directories to cache after the command runs"),
            ("output_files", "Individual files to cache after the command runs"),
        ]
    }
}
/// Tera template processor config. No custom fields.
/// Unused StandardConfig fields: command, formats, output_dir.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct TeraConfig {
    #[serde(flatten)]
    pub standard: StandardConfig,
}
impl KnownFields for TeraConfig {
    fn known_fields() -> &'static [&'static str] { StandardConfig::known_fields() }
    fn checksum_fields() -> &'static [&'static str] { StandardConfig::checksum_fields() }
    fn must_fields() -> &'static [&'static str] { StandardConfig::must_fields() }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] { StandardConfig::field_descriptions() }
}

/// Mako template processor config. No custom fields.
/// `command` is the Python interpreter used to render (default: python3).
/// Unused StandardConfig fields: formats, output_dir.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct MakoConfig {
    #[serde(flatten)]
    pub standard: StandardConfig,
}
impl KnownFields for MakoConfig {
    fn known_fields() -> &'static [&'static str] { StandardConfig::known_fields() }
    fn checksum_fields() -> &'static [&'static str] { StandardConfig::checksum_fields() }
    fn must_fields() -> &'static [&'static str] { StandardConfig::must_fields() }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] { StandardConfig::field_descriptions() }
}

/// Jinja2 template processor config. No custom fields.
/// `command` is the Python interpreter used to render (default: python3).
/// Unused StandardConfig fields: formats, output_dir.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct Jinja2Config {
    #[serde(flatten)]
    pub standard: StandardConfig,
}
impl KnownFields for Jinja2Config {
    fn known_fields() -> &'static [&'static str] { StandardConfig::known_fields() }
    fn checksum_fields() -> &'static [&'static str] { StandardConfig::checksum_fields() }
    fn must_fields() -> &'static [&'static str] { StandardConfig::must_fields() }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] { StandardConfig::field_descriptions() }
}

/// Engines accepted for `pandoc.pdf_engine`. Empty string means "use pandoc's default".
pub const PANDOC_PDF_ENGINES: &[&str] = &["pdflatex", "xelatex", "lualatex", "tectonic", "wkhtmltopdf", "weasyprint", "prince", "context"];

/// Pandoc processor config. Custom field: pdf_engine (forwarded to --pdf-engine
/// when format == pdf). Empty string keeps pandoc's default engine.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PandocConfig {
    #[serde(default)]
    pub pdf_engine: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}
impl Default for PandocConfig {
    fn default() -> Self {
        Self {
            pdf_engine: String::new(),
            standard: StandardConfig {
                command: "pandoc".into(),
                output_dir: "out/pandoc".into(),
                formats: vec!["pdf".into(), "html".into(), "docx".into()],
                ..StandardConfig::default()
            },
        }
    }
}
impl KnownFields for PandocConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "command", "args", "dep_inputs", "dep_auto", "formats", "output_dir",
            "batch", "max_jobs", "src_dirs", "src_extensions",
            "src_exclude_dirs", "src_exclude_files", "src_exclude_paths", "src_files",
            "pdf_engine",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["command", "args", "formats", "output_dir", "pdf_engine"]
    }
    fn must_fields() -> &'static [&'static str] { StandardConfig::must_fields() }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command",    "Path to the pandoc executable"),
            ("args",       "Extra arguments passed to pandoc"),
            ("formats",    "Output formats to generate (pdf, html, docx, …)"),
            ("output_dir", "Directory where converted files are written"),
            ("pdf_engine", "PDF engine for --pdf-engine (e.g. xelatex, lualatex). Empty = pandoc default (pdflatex)."),
        ]
    }
}

const fn default_marp_timeout_secs() -> u64 { 20 }
const fn default_marp_max_attempts() -> u32 { 3 }

/// Marp config. Adds tunables for the per-invocation timeout and the retry
/// budget, since marp-cli occasionally hangs under chromium-headless and the
/// right values are project-dependent.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MarpConfig {
    /// Per-invocation wall-clock timeout in seconds.
    #[serde(default = "default_marp_timeout_secs")]
    pub timeout_secs: u64,
    /// Total attempts (including the first) when marp times out. Set to 1 to
    /// disable retrying.
    #[serde(default = "default_marp_max_attempts")]
    pub max_attempts: u32,
    #[serde(flatten)]
    pub standard: StandardConfig,
}
impl Default for MarpConfig {
    fn default() -> Self {
        Self {
            timeout_secs: default_marp_timeout_secs(),
            max_attempts: default_marp_max_attempts(),
            standard: StandardConfig {
                command: "marp".into(),
                output_dir: "out/marp".into(),
                formats: vec!["pdf".into()],
                ..StandardConfig::default()
            },
        }
    }
}
impl KnownFields for MarpConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "command", "args", "dep_inputs", "dep_auto", "formats", "output_dir",
            "batch", "max_jobs", "src_dirs", "src_extensions",
            "src_exclude_dirs", "src_exclude_files", "src_exclude_paths", "src_files",
            "timeout_secs", "max_attempts",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        // timeout_secs / max_attempts only affect HOW we run marp, not the
        // bytes of the produced PDF — exclude from checksum to avoid pointless
        // rebuilds when a user bumps the timeout.
        &["command", "args", "formats", "output_dir"]
    }
    fn must_fields() -> &'static [&'static str] { StandardConfig::must_fields() }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command",      "Path to the marp executable"),
            ("args",         "Extra arguments passed to marp"),
            ("formats",      "Output formats to generate (pdf, html, …)"),
            ("output_dir",   "Directory where converted files are written"),
            ("timeout_secs", "Per-invocation wall-clock timeout in seconds"),
            ("max_attempts", "Total attempts (including the first) when marp times out"),
        ]
    }
}






pub type MarpImagesConfig = CheckerConfig;

/// ClangTidy config. Custom fields: compiler_args.
/// Unused StandardConfig fields: command, formats, output_dir.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct ClangTidyConfig {
    #[serde(default)]
    pub compiler_args: Vec<String>,
    #[serde(flatten)]
    pub standard: StandardConfig,
}


impl KnownFields for ClangTidyConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "args", "compiler_args", "dep_inputs", "dep_auto", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["args", "compiler_args"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("args",          "Extra arguments passed to clang-tidy"),
            ("compiler_args", "Compiler flags forwarded to clang-tidy for parsing"),
        ]
    }
}

/// Method for scanning C/C++ header dependencies
#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IncludeScanner {
    /// Native regex-based scanner (fast, no external process)
    #[default]
    Native,
    /// Use gcc/g++ -MM (accurate but slower, spawns external process)
    Compiler,
}

/// Configuration for a single compiler profile
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CompilerProfile {
    /// Profile name (used in output paths, e.g., "gcc", "clang")
    pub name: String,
    /// Whether this profile is enabled (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_cc_compiler")]
    pub cc: String,
    #[serde(default = "default_cxx_compiler")]
    pub cxx: String,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub cxxflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
    #[serde(default = "default_output_suffix")]
    pub output_suffix: String,
}

/// CcSingleFile config. Custom fields: cc, cxx, cflags, cxxflags, ldflags, output_suffix, compilers, include_paths, include_scanner.
/// Unused StandardConfig fields: command, formats.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CcSingleFileConfig {
    /// Legacy single-compiler fields (used when `compilers` is empty)
    #[serde(default = "default_cc_compiler")]
    pub cc: String,
    #[serde(default = "default_cxx_compiler")]
    pub cxx: String,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub cxxflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
    #[serde(default = "default_output_suffix")]
    pub output_suffix: String,
    /// Multiple compiler profiles (if set, overrides legacy fields)
    #[serde(default)]
    pub compilers: Vec<CompilerProfile>,
    /// Shared settings across all compilers
    #[serde(default)]
    pub include_paths: Vec<String>,
    /// Method for scanning header dependencies (native or compiler)
    #[serde(default)]
    pub include_scanner: IncludeScanner,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl CcSingleFileConfig {
    /// Get the list of enabled compiler profiles to use.
    /// If `compilers` is set, returns enabled profiles from that list.
    /// Otherwise, creates a single profile from the legacy fields.
    pub(crate) fn get_compiler_profiles(&self) -> Vec<CompilerProfile> {
        if !self.compilers.is_empty() {
            self.compilers.iter().filter(|p| p.enabled).cloned().collect()
        } else {
            // Legacy mode: create single profile from top-level fields
            vec![CompilerProfile {
                name: String::new(), // Empty name = no subdirectory
                enabled: true,
                cc: self.cc.clone(),
                cxx: self.cxx.clone(),
                cflags: self.cflags.clone(),
                cxxflags: self.cxxflags.clone(),
                ldflags: self.ldflags.clone(),
                output_suffix: self.output_suffix.clone(),
            }]
        }
    }
}

impl Default for CcSingleFileConfig {
    fn default() -> Self {
        Self {
            cc: "gcc".into(),
            cxx: "g++".into(),
            cflags: Vec::new(),
            cxxflags: Vec::new(),
            ldflags: Vec::new(),
            output_suffix: ".elf".into(),
            compilers: Vec::new(),
            include_paths: Vec::new(),
            include_scanner: IncludeScanner::default(),
            standard: StandardConfig {
                output_dir: "out/cc_single_file".into(),
                ..StandardConfig::default()
            },
        }
    }
}

impl KnownFields for CcSingleFileConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "cc", "cxx", "cflags", "cxxflags", "ldflags", "output_suffix",
            "compilers", "include_paths", "dep_inputs", "dep_auto", "output_dir",
            "include_scanner", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &[
            "cc", "cxx", "cflags", "cxxflags", "ldflags", "output_suffix",
            "compilers", "include_paths", "output_dir", "include_scanner",
        ]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("cc",              "C compiler executable"),
            ("cxx",             "C++ compiler executable"),
            ("output_suffix",   "Suffix appended to output binary names"),
            ("compilers",       "Named compiler profiles (overrides cc/cxx when set)"),
            ("include_paths",   "Additional header search directories"),
            ("output_dir",      "Directory where compiled binaries are written"),
            ("include_scanner", "Header dependency scanner: native (fast) or compiler (accurate)"),
        ]
    }
}

// --- cc (full C/C++ project builds) ---

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CcLibraryDef {
    pub name: String,
    #[serde(default = "default_cc_lib_type")]
    pub lib_type: String,
    pub sources: Vec<String>,
    #[serde(default)]
    pub include_dirs: Vec<String>,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub cxxflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
}

fn default_cc_lib_type() -> String {
    "shared".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CcProgramDef {
    pub name: String,
    pub sources: Vec<String>,
    #[serde(default)]
    pub link: Vec<String>,
    #[serde(default)]
    pub include_dirs: Vec<String>,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub cxxflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
}

/// Parsed contents of a cc.yaml file.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct CcManifest {
    #[serde(default = "default_cc_compiler")]
    pub cc: String,
    #[serde(default = "default_cxx_compiler")]
    pub cxx: String,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub cxxflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
    #[serde(default)]
    pub include_dirs: Vec<String>,
    #[serde(default)]
    pub libraries: Vec<CcLibraryDef>,
    #[serde(default)]
    pub programs: Vec<CcProgramDef>,
}

/// CC (full C/C++ project) config. Custom: cc, cxx, cflags, cxxflags, ldflags, include_dirs, single_invocation, cache_output_dir.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CcConfig {
    #[serde(default = "default_cc_compiler")]
    pub cc: String,
    #[serde(default = "default_cxx_compiler")]
    pub cxx: String,
    #[serde(default)]
    pub cflags: Vec<String>,
    #[serde(default)]
    pub cxxflags: Vec<String>,
    #[serde(default)]
    pub ldflags: Vec<String>,
    #[serde(default)]
    pub include_dirs: Vec<String>,
    #[serde(default)]
    pub single_invocation: bool,
    #[serde(default = "default_true")]
    pub cache_output_dir: bool,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for CcConfig {
    fn default() -> Self {
        Self {
            cc: "gcc".into(),
            cxx: "g++".into(),
            cflags: Vec::new(),
            cxxflags: Vec::new(),
            ldflags: Vec::new(),
            include_dirs: Vec::new(),
            single_invocation: false,
            cache_output_dir: true,
            standard: StandardConfig::default(),
        }
    }
}

impl KnownFields for CcConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "cc", "cxx", "cflags", "cxxflags", "ldflags",
            "include_dirs", "single_invocation",
            "dep_inputs", "cache_output_dir", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &[
            "cc", "cxx", "cflags", "cxxflags", "ldflags",
            "include_dirs", "single_invocation",
        ]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("cc",                "C compiler executable"),
            ("cxx",               "C++ compiler executable"),
            ("cflags",            "Flags passed to the C compiler"),
            ("cxxflags",          "Flags passed to the C++ compiler"),
            ("ldflags",           "Flags passed to the linker"),
            ("include_dirs",      "Additional header search directories"),
            ("single_invocation", "Compile all sources in one compiler call"),
            ("cache_output_dir",  "Cache the entire output directory as a unit"),
        ]
    }
}

// --- linux_module (Linux kernel module builds) ---

/// A single kernel module definition inside linux-module.yaml.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct LinuxModuleModuleDef {
    pub name: String,
    pub sources: Vec<String>,
    #[serde(default)]
    pub extra_cflags: Vec<String>,
}

/// Parsed contents of a linux-module.yaml file.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct LinuxModuleManifest {
    #[serde(default = "default_make_tool")]
    pub make: String,
    #[serde(default)]
    pub kdir: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub cross_compile: Option<String>,
    #[serde(default = "default_linux_module_v")]
    pub v: u32,
    #[serde(default = "default_linux_module_w")]
    pub w: u32,
    pub modules: Vec<LinuxModuleModuleDef>,
}

fn default_make_tool() -> String {
    "make".into()
}

const fn default_linux_module_v() -> u32 {
    0
}

const fn default_linux_module_w() -> u32 {
    1
}

/// Linux module config. No custom fields.
/// Unused StandardConfig fields: command, formats, output_dir, args.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct LinuxModuleConfig {
    #[serde(flatten)]
    pub standard: StandardConfig,
}
impl KnownFields for LinuxModuleConfig {
    fn known_fields() -> &'static [&'static str] { StandardConfig::known_fields() }
    fn checksum_fields() -> &'static [&'static str] { StandardConfig::checksum_fields() }
    fn must_fields() -> &'static [&'static str] { StandardConfig::must_fields() }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] { StandardConfig::field_descriptions() }
}

fn default_zspell_language() -> String {
    "en_US".into()
}

fn default_zspell_words_file() -> String {
    ".zspell-words".into()
}

fn default_zspell_dict_dir() -> String {
    "/usr/share/hunspell".into()
}

/// Zspell config. Custom fields: language, words_file, auto_add_words, dict_dir.
/// Unused StandardConfig fields: command, formats, output_dir.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ZspellConfig {
    #[serde(default = "default_zspell_language")]
    pub language: String,
    #[serde(default = "default_zspell_words_file")]
    pub words_file: String,
    /// When true, automatically add misspelled words to words_file instead of failing
    #[serde(default)]
    pub auto_add_words: bool,
    /// Directory holding the hunspell .aff/.dic files for `language`.
    #[serde(default = "default_zspell_dict_dir")]
    pub dict_dir: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for ZspellConfig {
    fn default() -> Self {
        Self {
            language: "en_US".into(),
            words_file: ".zspell-words".into(),
            auto_add_words: false,
            dict_dir: default_zspell_dict_dir(),
            standard: StandardConfig::default(),
        }
    }
}

impl KnownFields for ZspellConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "language", "words_file", "auto_add_words", "dict_dir", "dep_inputs", "dep_auto", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["language", "auto_add_words", "dict_dir"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("language",       "Language/locale for spell checking"),
            ("words_file",     "Path to a personal dictionary file"),
            ("auto_add_words", "When true, automatically add misspelled words to words_file instead of failing"),
            ("dict_dir",       "Directory containing hunspell .aff/.dic files"),
        ]
    }
}

fn default_cargo() -> String {
    "cargo".into()
}

fn default_cargo_profiles() -> Vec<String> {
    vec!["dev".into(), "release".into()]
}

/// Cargo config. Custom: cargo, profiles, cache_output_dir.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CargoConfig {
    #[serde(default = "default_cargo")]
    pub cargo: String,
    #[serde(default = "default_cargo_profiles")]
    pub profiles: Vec<String>,
    #[serde(default = "default_true")]
    pub cache_output_dir: bool,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for CargoConfig {
    fn default() -> Self {
        Self {
            cargo: "cargo".into(),
            profiles: default_cargo_profiles(),
            cache_output_dir: true,
            standard: StandardConfig {
                command: "build".into(),
                ..StandardConfig::default()
            },
        }
    }
}

impl KnownFields for CargoConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "cargo", "command", "args", "dep_inputs", "profiles",
            "cache_output_dir", "batch", "max_jobs", "src_extensions", "src_exclude_dirs", "src_exclude_files",
            "src_exclude_paths",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["cargo", "command", "args", "profiles"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("cargo",   "Path to the cargo executable"),
            ("command", "Cargo subcommand to run (e.g. build, test)"),
            ("args",    "Extra arguments passed to cargo"),
            ("profiles", "Build profiles to run (e.g. dev, release)"),
            ("cache_output_dir", "Cache the entire output directory as a unit"),
        ]
    }
}


#[derive(Debug, Deserialize, Serialize, Clone)]
/// Clippy config. Custom: cargo.
pub struct ClippyConfig {
    #[serde(default = "default_cargo")]
    pub cargo: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for ClippyConfig {
    fn default() -> Self {
        Self {
            cargo: "cargo".into(),
            standard: StandardConfig { command: "clippy".into(), ..StandardConfig::default() },
        }
    }
}

impl KnownFields for ClippyConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "cargo", "command", "args", "dep_inputs", "dep_auto", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["cargo", "command", "args"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("cargo",   "Path to the cargo executable"),
            ("command", "Cargo subcommand to run (defaults to clippy)"),
            ("args",    "Extra arguments passed to cargo clippy"),
        ]
    }
}

/// Make config. Custom: target.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MakeConfig {
    #[serde(default)]
    pub target: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for MakeConfig {
    fn default() -> Self {
        Self {
            target: String::new(),
            standard: StandardConfig { command: "make".into(), ..StandardConfig::default() },
        }
    }
}

impl KnownFields for MakeConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "command", "args", "target", "dep_inputs", "dep_auto", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["command", "args", "target"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command", "Path to the make executable"),
            ("args",   "Extra arguments passed to make"),
            ("target", "Make target to build"),
        ]
    }
}








pub type JsonSchemaConfig = CheckerConfig;

fn default_tags_output() -> String {
    "out/tags/tags.db".into()
}

fn default_tags_dir() -> String {
    "tags".into()
}

const fn default_tags_similar_files_limit() -> usize { 10 }
const fn default_tags_suggested_tags_limit() -> usize { 15 }
const fn default_tags_common_tags_limit() -> usize { 20 }

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TagsConfig {
    #[serde(default = "default_tags_output")]
    pub output: String,
    /// Directory containing tag list files.
    /// Each `<name>.txt` file defines allowed tags as `<name>:<line>` pairs.
    #[serde(default = "default_tags_dir")]
    pub tags_dir: String,
    /// Frontmatter fields that every markdown file must have.
    #[serde(default)]
    pub required_fields: Vec<String>,
    /// Scalar fields whose values must exist in the corresponding tag_lists file.
    #[serde(default)]
    pub required_values: Vec<String>,
    /// Fields whose values must be unique across all files.
    #[serde(default)]
    pub unique_fields: Vec<String>,
    /// Expected types for fields: "scalar", "list", or "number".
    #[serde(default)]
    pub field_types: HashMap<String, String>,
    /// Groups of fields where at least one group must be fully present.
    /// Each inner Vec is a group; a file passes if all fields in any one group are present.
    #[serde(default)]
    pub required_field_groups: Vec<Vec<String>>,
    /// Require list-type fields to have items in sorted order.
    #[serde(default)]
    pub sorted_tags: bool,
    /// Fail the build when tags in the allowlist are not used by any file.
    #[serde(default)]
    pub check_unused: bool,
    /// How many most-similar files to inspect when generating tag suggestions.
    #[serde(default = "default_tags_similar_files_limit")]
    pub similar_files_limit: usize,
    /// Maximum tag suggestions emitted by `tags suggest`.
    #[serde(default = "default_tags_suggested_tags_limit")]
    pub suggested_tags_limit: usize,
    /// Maximum "most common tags" reported when a file has no tags of its own.
    #[serde(default = "default_tags_common_tags_limit")]
    pub common_tags_limit: usize,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for TagsConfig {
    fn default() -> Self {
        Self {
            output: "out/tags/tags.db".into(),
            tags_dir: "tags".into(),
            required_fields: Vec::new(),
            required_values: Vec::new(),
            unique_fields: Vec::new(),
            field_types: HashMap::new(),
            required_field_groups: Vec::new(),
            sorted_tags: false,
            check_unused: false,
            similar_files_limit: default_tags_similar_files_limit(),
            suggested_tags_limit: default_tags_suggested_tags_limit(),
            common_tags_limit: default_tags_common_tags_limit(),
            standard: StandardConfig::default(),
        }
    }
}

impl KnownFields for TagsConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "output", "tags_dir", "required_fields", "required_values",
            "unique_fields", "field_types", "required_field_groups", "sorted_tags",
            "check_unused", "similar_files_limit", "suggested_tags_limit",
            "common_tags_limit", "dep_inputs", "dep_auto", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        // The validation-rule fields are included: validation runs during the
        // cached build, so editing a rule (e.g. adding a required_fields
        // entry) must invalidate cached PASS results — nothing else re-checks
        // them. Only the *_limit fields are excluded; they shape `tags
        // suggest` CLI output, not build results.
        &[
            "output", "tags_dir", "required_fields", "required_values",
            "unique_fields", "field_types", "required_field_groups",
            "sorted_tags", "check_unused",
        ]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("output",                "Output tags database file path"),
            ("tags_dir",              "Directory containing tag list files"),
            ("required_fields",       "Frontmatter fields that every markdown file must have"),
            ("required_values",       "Scalar fields whose values must exist in the tag lists file"),
            ("unique_fields",         "Fields whose values must be unique across all files"),
            ("field_types",           "Expected types for fields: scalar, list, or number"),
            ("required_field_groups", "Groups of fields where at least one group must be fully present"),
            ("sorted_tags",           "Require list-type fields to have items in sorted order"),
            ("check_unused",          "Fail the build when tags in the allowlist are not used by any file"),
            ("similar_files_limit",   "How many most-similar files to inspect when generating tag suggestions"),
            ("suggested_tags_limit",  "Maximum tag suggestions emitted by `tags suggest`"),
            ("common_tags_limit",     "Maximum 'most common tags' reported when a file has no tags of its own"),
        ]
    }
}


/// Script processor config. No custom fields.
/// Unused StandardConfig fields: formats, output_dir.
/// Note: empty command means "no command configured".
#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct ScriptConfig {
    #[serde(flatten)]
    pub standard: StandardConfig,
    /// Command to run in fix mode (`rsconstruct fix`). Empty means no fix capability.
    #[serde(default)]
    pub fix_command: String,
    /// Arguments for the fix command (prepended before file paths).
    #[serde(default)]
    pub fix_args: Vec<String>,
    /// Whether fix mode supports batch execution (multiple files per invocation).
    /// Defaults to the same value as `batch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix_batch: Option<bool>,
}
impl KnownFields for ScriptConfig {
    fn known_fields() -> &'static [&'static str] {
        &["command", "formats", "args", "dep_inputs", "dep_auto", "output_dir", "batch", "max_jobs",
          "fix_command", "fix_args", "fix_batch"]
    }
    fn checksum_fields() -> &'static [&'static str] { StandardConfig::checksum_fields() }
    fn must_fields() -> &'static [&'static str] { StandardConfig::must_fields() }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command",     "Path to the checker tool executable"),
            ("args",        "Extra arguments passed to the checker tool"),
            ("fix_command", "Path to the fixer tool executable (empty = no fix capability)"),
            ("fix_args",    "Arguments for the fixer (prepended before file paths)"),
            ("fix_batch",   "Whether fix mode supports batch execution (default: same as batch)"),
        ]
    }
}

fn default_generator_output_extension() -> String {
    "out".into()
}

/// Generator config. Custom: output_extension.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GeneratorConfig {
    #[serde(default = "default_generator_output_extension")]
    pub output_extension: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for GeneratorConfig {
    fn default() -> Self {
        Self {
            output_extension: "out".into(),
            standard: StandardConfig { output_dir: "out/generator".into(), ..StandardConfig::default() },
        }
    }
}

impl KnownFields for GeneratorConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "command", "output_dir", "output_extension", "args",
            "dep_inputs", "dep_auto", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["command", "output_dir", "output_extension", "args"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command",          "Script or executable to run as a generator (required)"),
            ("args",             "Extra arguments passed to the command before file paths"),
            ("output_dir",       "Directory where generated output files are written"),
            ("output_extension", "File extension for generated output files"),
        ]
    }
}

// --- explicit processor (many inputs → few outputs, fully declared) ---

/// Explicit config. Custom: inputs, input_globs, output_files, output_dirs.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ExplicitConfig {
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub input_globs: Vec<String>,
    #[serde(default)]
    pub output_files: Vec<String>,
    #[serde(default)]
    pub output_dirs: Vec<String>,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for ExplicitConfig {
    fn default() -> Self {
        Self {
            inputs: Vec::new(),
            input_globs: Vec::new(),
            output_files: Vec::new(),
            output_dirs: Vec::new(),
            standard: StandardConfig { command: "true".into(), ..StandardConfig::default() },
        }
    }
}

impl KnownFields for ExplicitConfig {
    fn known_fields() -> &'static [&'static str] {
        &["command", "args", "inputs", "input_globs", "output_files", "output_dirs"]
    }
    fn checksum_fields() -> &'static [&'static str] {
        // inputs/input_globs are excluded: input changes are detected by content
        // hashing of the resolved input file set, not by hashing the pattern list.
        &["command", "args", "output_files", "output_dirs"]
    }
    fn must_fields() -> &'static [&'static str] {
        &["command"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command",      "Command to run to produce the outputs"),
            ("args",         "Extra arguments passed before input/output paths"),
            ("inputs",       "Explicit list of input files"),
            ("input_globs",  "Glob patterns for input files"),
            ("output_files", "Output files produced by the command"),
            ("output_dirs",  "Output directories produced by the command"),
        ]
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
/// Pip config. Custom: none (uses standard.command as the pip executable).
pub struct PipConfig {
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for PipConfig {
    fn default() -> Self {
        Self { standard: StandardConfig { command: "pip".into(), ..StandardConfig::default() } }
    }
}

impl KnownFields for PipConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "command", "args", "dep_inputs", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["command", "args"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command", "Path to the pip executable"),
            ("args",    "Extra arguments passed to pip install"),
        ]
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
/// Requirements generator config. Produces a requirements.txt from Python imports.
pub struct RequirementsConfig {
    /// Output file path.
    #[serde(default = "default_requirements_output")]
    pub output: String,
    /// Import names to never emit (e.g. internal vendored modules).
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Sort entries alphabetically. When false, entries appear in first-seen order.
    #[serde(default = "default_true")]
    pub sorted: bool,
    /// Include a "# Generated by rsconstruct" comment header.
    #[serde(default = "default_true")]
    pub header: bool,
    /// User-supplied import→distribution mapping overrides. Wins over the built-in table.
    #[serde(default)]
    pub mapping: HashMap<String, String>,
    /// Distribution names to always include in the output, even when no
    /// `import` statement references them. Use this for transitive runtime
    /// dependencies that an upstream package needs at import time but fails
    /// to declare in its own metadata (e.g. `setuptools` for packages that
    /// `import pkg_resources`).
    #[serde(default)]
    pub extra: Vec<String>,
    /// Project-relative directories to treat as Python source roots. An import
    /// is classified as local (and excluded from requirements.txt) if it
    /// resolves to `<root>/<module>.py` or `<root>/<module>/__init__.py` for
    /// any of these roots. The importer's own directory and the project root
    /// are always checked in addition to these. Use this when modules live in
    /// a directory that is added to sys.path at runtime (e.g. via PYTHONPATH
    /// or sys.path.insert) so the static scanner doesn't mistake them for
    /// PyPI distributions.
    #[serde(default)]
    pub python_paths: Vec<String>,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

fn default_requirements_output() -> String {
    "requirements.txt".into()
}

impl Default for RequirementsConfig {
    fn default() -> Self {
        Self {
            output: default_requirements_output(),
            exclude: Vec::new(),
            sorted: true,
            header: true,
            mapping: HashMap::new(),
            extra: Vec::new(),
            python_paths: Vec::new(),
            standard: StandardConfig::default(),
        }
    }
}

impl KnownFields for RequirementsConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "output", "exclude", "sorted", "header", "mapping", "extra", "python_paths",
            "dep_inputs", "dep_auto", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["output", "exclude", "sorted", "header", "mapping", "extra", "python_paths"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("output",       "Path of the generated requirements.txt file"),
            ("exclude",      "Import names to never emit (e.g. internal vendored modules)"),
            ("sorted",       "Sort entries alphabetically (false preserves first-seen order)"),
            ("header",       "Include a comment header line in the generated file"),
            ("mapping",      "Per-project import→distribution overrides (win over built-in table)"),
            ("extra",        "Distribution names to always include, e.g. transitive deps undeclared by upstream"),
            ("python_paths", "Project-relative source roots resolved when classifying imports as local"),
        ]
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
/// Sphinx config. Custom: working_dir, cache_output_dir.
pub struct SphinxConfig {
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default = "default_true")]
    pub cache_output_dir: bool,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for SphinxConfig {
    fn default() -> Self {
        Self {
            working_dir: None,
            cache_output_dir: true,
            standard: StandardConfig {
                command: "sphinx-build".into(),
                output_dir: "docs".into(),
                ..StandardConfig::default()
            },
        }
    }
}

impl KnownFields for SphinxConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "command", "output_dir", "working_dir", "args", "dep_inputs", "cache_output_dir", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["command", "output_dir", "working_dir", "args"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command",     "Path to the sphinx-build executable"),
            ("output_dir",  "Directory where built docs are written"),
            ("working_dir", "Working directory for sphinx-build (defaults to conf.py location)"),
            ("args",        "Extra arguments passed to sphinx-build"),
            ("cache_output_dir", "Cache the entire output directory as a unit"),
        ]
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
/// Mdbook config. Custom: cache_output_dir.
pub struct MdbookConfig {
    #[serde(default = "default_true")]
    pub cache_output_dir: bool,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for MdbookConfig {
    fn default() -> Self {
        Self {
            cache_output_dir: true,
            standard: StandardConfig {
                command: "mdbook".into(),
                output_dir: "book".into(),
                ..StandardConfig::default()
            },
        }
    }
}

impl KnownFields for MdbookConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "command", "output_dir", "args", "dep_inputs", "cache_output_dir", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["command", "output_dir", "args"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command",    "Path to the mdbook executable"),
            ("output_dir", "Directory where the built book is written"),
            ("args",       "Extra arguments passed to mdbook build"),
            ("cache_output_dir", "Cache the entire output directory as a unit"),
        ]
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
/// Npm config. Custom: cache_output_dir.
pub struct NpmConfig {
    #[serde(default = "default_true")]
    pub cache_output_dir: bool,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for NpmConfig {
    fn default() -> Self {
        Self {
            cache_output_dir: true,
            standard: StandardConfig { command: "npm".into(), ..StandardConfig::default() },
        }
    }
}

impl KnownFields for NpmConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "command", "args", "dep_inputs", "cache_output_dir", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["command", "args"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command", "Path to the npm executable"),
            ("args",    "Arguments passed to npm install"),
            ("cache_output_dir", "Cache the entire output directory as a unit"),
        ]
    }
}

fn default_gem_home() -> String {
    "gems".into()
}

fn default_gem_stamp() -> String {
    "out/gem/root.stamp".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MdlConfig {
    #[serde(default)]
    pub local_repo: bool,
    #[serde(default = "default_gem_home")]
    pub gem_home: String,
    #[serde(default = "default_gem_stamp")]
    pub gem_stamp: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for MdlConfig {
    fn default() -> Self {
        Self {
            local_repo: false,
            gem_home: "gems".into(),
            gem_stamp: "out/gem/root.stamp".into(),
            standard: StandardConfig { command: "mdl".into(), ..StandardConfig::default() },
        }
    }
}

impl KnownFields for MdlConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "local_repo", "gem_home", "command", "args", "dep_inputs", "dep_auto", "gem_stamp", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["local_repo", "gem_home", "command", "args", "gem_stamp"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("local_repo", "Use a local gem repository instead of system install"),
            ("gem_home",   "Path to the local gem repository"),
            ("command",    "Path to the mdl executable"),
            ("args",       "Extra arguments passed to mdl"),
            ("gem_stamp",  "Stamp file tracking the local gem installation"),
        ]
    }
}



#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct MarkdownlintConfig {
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for MarkdownlintConfig {
    fn default() -> Self {
        Self {
            standard: StandardConfig {
                command: "markdownlint".into(),
                ..StandardConfig::default()
            },
        }
    }
}

impl KnownFields for MarkdownlintConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "command", "args", "dep_inputs", "dep_auto", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["command", "args"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command", "Path to the markdownlint executable"),
            ("args",    "Extra arguments passed to markdownlint"),
        ]
    }
}


fn default_aspell_conf() -> String {
    ".aspell.conf".into()
}

fn default_aspell_words_file() -> String {
    ".aspell.en.pws".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AspellConfig {
    #[serde(default = "default_aspell_conf")]
    pub conf: String,
    #[serde(default)]
    pub auto_add_words: bool,
    #[serde(default = "default_aspell_words_file")]
    pub words_file: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for AspellConfig {
    fn default() -> Self {
        Self {
            conf: ".aspell.conf".into(),
            auto_add_words: false,
            words_file: ".aspell.en.pws".into(),
            standard: StandardConfig { command: "aspell".into(), ..StandardConfig::default() },
        }
    }
}

impl KnownFields for AspellConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "command", "args", "conf", "auto_add_words", "words_file", "dep_inputs", "dep_auto", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["command", "args", "conf", "auto_add_words"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command",        "Path to the aspell executable"),
            ("args",           "Extra arguments passed to aspell"),
            ("conf",           "Path to the aspell configuration file"),
            ("auto_add_words", "When true, automatically add misspelled words to words_file instead of failing"),
            ("words_file",     "Path to the personal word list file"),
        ]
    }
}


pub type AsciiConfig = CheckerConfig;

fn default_dir_terms_unambiguous() -> String {
    "terms/unambiguous".into()
}

fn default_dir_terms_ambiguous() -> String {
    "terms/ambiguous".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TermsConfig {
    #[serde(default = "default_dir_terms_unambiguous")]
    pub dir_terms_unambiguous: String,
    #[serde(default = "default_dir_terms_ambiguous")]
    pub dir_terms_ambiguous: String,
    /// When true (default), backticking an ambiguous term is a build error
    /// and `terms fix` strips those backticks. When false, ambiguous terms
    /// are loaded only to validate the disjoint invariant; their use in
    /// backticks is neither flagged nor changed.
    #[serde(default = "default_true")]
    pub forbid_backticked_ambiguous: bool,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for TermsConfig {
    fn default() -> Self {
        Self {
            dir_terms_unambiguous: "terms/unambiguous".into(),
            dir_terms_ambiguous: "terms/ambiguous".into(),
            forbid_backticked_ambiguous: true,
            standard: StandardConfig::default(),
        }
    }
}

impl KnownFields for TermsConfig {
    fn known_fields() -> &'static [&'static str] {
        &["dir_terms_unambiguous", "dir_terms_ambiguous", "forbid_backticked_ambiguous", "dep_inputs", "dep_auto", "batch", "max_jobs"]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["dir_terms_unambiguous", "dir_terms_ambiguous", "forbid_backticked_ambiguous"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("dir_terms_unambiguous", "Directory containing unambiguous term definition files (must be backticked in prose)"),
            ("dir_terms_ambiguous", "Optional directory of ambiguous terms; build fails if any term overlaps with dir_terms_unambiguous"),
            ("forbid_backticked_ambiguous", "If true (default), backticking an ambiguous term is a build error and `terms fix` strips those backticks"),
        ]
    }
}




const fn default_pdflatex_runs() -> usize {
    2
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PdflatexConfig {
    #[serde(default = "default_pdflatex_runs")]
    pub runs: usize,
    #[serde(default = "default_true")]
    pub qpdf: bool,
    /// Pass -shell-escape to pdflatex. Historically always on; note it lets
    /// any .tex input execute arbitrary shell commands during the build.
    #[serde(default = "default_true")]
    pub shell_escape: bool,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for PdflatexConfig {
    fn default() -> Self {
        Self {
            runs: 2,
            qpdf: true,
            shell_escape: true,
            standard: StandardConfig {
                command: "pdflatex".into(),
                output_dir: "out/pdflatex".into(),
                ..StandardConfig::default()
            },
        }
    }
}

impl KnownFields for PdflatexConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "command", "args", "dep_inputs", "dep_auto", "runs", "qpdf", "shell_escape", "output_dir", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["command", "args", "runs", "qpdf", "shell_escape", "output_dir"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command",      "Path to the pdflatex executable"),
            ("args",         "Extra arguments passed to pdflatex"),
            ("runs",         "Number of pdflatex compilation passes"),
            ("qpdf",         "Run qpdf to optimize the output PDF"),
            ("shell_escape", "Pass -shell-escape to pdflatex (lets .tex files run shell commands)"),
            ("output_dir",   "Directory where compiled PDFs are written"),
        ]
    }
}




#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GemConfig {
    #[serde(default = "default_gem_home")]
    pub gem_home: String,
    #[serde(default = "default_true")]
    pub cache_output_dir: bool,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for GemConfig {
    fn default() -> Self {
        Self {
            gem_home: "gems".into(),
            cache_output_dir: true,
            standard: StandardConfig { command: "bundle".into(), ..StandardConfig::default() },
        }
    }
}

impl KnownFields for GemConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "command", "gem_home", "args", "dep_inputs", "cache_output_dir", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["command", "gem_home", "args"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command",  "Path to the bundler executable"),
            ("gem_home", "Directory where gems are installed"),
            ("args",     "Extra arguments passed to bundler install"),
            ("cache_output_dir", "Cache the entire output directory as a unit"),
        ]
    }
}







pub type IjqConfig = CheckerConfig;

pub type IjsonlintConfig = CheckerConfig;

pub type IyamllintConfig = CheckerConfig;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct IyamlschemaConfig {
    #[serde(default = "default_true")]
    pub check_ordering: bool,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for IyamlschemaConfig {
    fn default() -> Self {
        Self {
            check_ordering: true,
            standard: StandardConfig::default(),
        }
    }
}

impl KnownFields for IyamlschemaConfig {
    fn known_fields() -> &'static [&'static str] {
        &["check_ordering", "dep_inputs", "dep_auto", "batch", "max_jobs"]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["check_ordering"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("check_ordering", "Require YAML keys to appear in the order defined by the schema"),
        ]
    }
}


pub type ItaploConfig = CheckerConfig;

fn default_rust_single_file_output_suffix() -> String { ".elf".into() }
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RustSingleFileConfig {
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default = "default_rust_single_file_output_suffix")]
    pub output_suffix: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for RustSingleFileConfig {
    fn default() -> Self {
        Self {
            flags: Vec::new(),
            output_suffix: ".elf".into(),
            standard: StandardConfig {
                command: "rustc".into(),
                output_dir: "out/rust_single_file".into(),
                ..StandardConfig::default()
            },
        }
    }
}

impl KnownFields for RustSingleFileConfig {
    fn known_fields() -> &'static [&'static str] {
        &["command", "flags", "output_suffix", "dep_inputs", "dep_auto", "output_dir", "batch", "max_jobs"]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["command", "flags", "output_suffix", "output_dir"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command",       "Path to the rustc executable"),
            ("flags",         "Extra flags passed to rustc"),
            ("output_suffix", "Suffix appended to output binary names"),
            ("output_dir",    "Directory where compiled binaries are written"),
        ]
    }
}


fn default_pdfunite_source_dir() -> String {
    "marp/courses".into()
}

fn default_pdfunite_source_ext() -> String {
    ".md".into()
}

fn default_pdfunite_source_output_dir() -> String {
    "out/marp".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct PdfuniteConfig {
    #[serde(default = "default_pdfunite_source_dir")]
    pub source_dir: String,
    #[serde(default = "default_pdfunite_source_ext")]
    pub source_ext: String,
    #[serde(default = "default_pdfunite_source_output_dir")]
    pub source_output_dir: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for PdfuniteConfig {
    fn default() -> Self {
        Self {
            source_dir: "marp/courses".into(),
            source_ext: ".md".into(),
            source_output_dir: "out/marp".into(),
            standard: StandardConfig {
                command: "pdfunite".into(),
                output_dir: "out/pdfunite".into(),
                ..StandardConfig::default()
            },
        }
    }
}

impl KnownFields for PdfuniteConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "command", "source_dir", "source_ext", "source_output_dir",
            "args", "dep_inputs", "dep_auto", "output_dir", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &[
            "command", "source_dir", "source_ext", "source_output_dir",
            "args", "output_dir",
        ]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("command",           "Path to the pdfunite executable"),
            ("source_dir",        "Directory containing course YAML files listing PDFs to merge"),
            ("source_ext",        "Extension of source files used to find PDFs"),
            ("source_output_dir", "Directory where source PDFs (to be merged) are located"),
            ("args",              "Extra arguments passed to pdfunite"),
            ("output_dir",        "Directory where merged PDFs are written"),
        ]
    }
}


// --- ipdfunite (internal PDF merge, no external binary) ---

fn default_ipdfunite_source_dir() -> String {
    "marp/courses".into()
}

fn default_ipdfunite_source_ext() -> String {
    ".md".into()
}

fn default_ipdfunite_source_output_dir() -> String {
    "out/marp".into()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct IpdfuniteConfig {
    #[serde(default = "default_ipdfunite_source_dir")]
    pub source_dir: String,
    #[serde(default = "default_ipdfunite_source_ext")]
    pub source_ext: String,
    #[serde(default = "default_ipdfunite_source_output_dir")]
    pub source_output_dir: String,
    #[serde(flatten)]
    pub standard: StandardConfig,
}

impl Default for IpdfuniteConfig {
    fn default() -> Self {
        Self {
            source_dir: "marp/courses".into(),
            source_ext: ".md".into(),
            source_output_dir: "out/marp".into(),
            standard: StandardConfig { output_dir: "out/ipdfunite".into(), ..StandardConfig::default() },
        }
    }
}

impl KnownFields for IpdfuniteConfig {
    fn known_fields() -> &'static [&'static str] {
        &[
            "source_dir", "source_ext", "source_output_dir",
            "dep_inputs", "dep_auto", "output_dir", "batch", "max_jobs",
        ]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &[
            "source_dir", "source_ext", "source_output_dir", "output_dir",
        ]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("source_dir",        "Directory containing course YAML files listing PDFs to merge"),
            ("source_ext",        "Extension of source files used to find PDFs"),
            ("source_output_dir", "Directory where source PDFs (to be merged) are located"),
            ("output_dir",        "Directory where merged PDFs are written"),
        ]
    }
}




// --- tidy (HTML validator) ---

// --- stylelint (CSS linter) ---

// --- jslint (JavaScript linter) ---

// --- standard (JavaScript style checker) ---

// --- htmllint (HTML linter) ---

// --- php_lint (PHP syntax checker) ---

// --- perlcritic (Perl code analyzer) ---

// --- xmllint (XML validator) ---

// --- svglint (SVG linter) ---

// --- svgo (SVG validator via svgo; stdout discarded, non-zero exit = malformed) ---

// --- checkstyle (Java style checker) ---

// --- yq (YAML processor/validator) ---

// --- cmake (CMake build system) ---

// --- docker (Docker image build) ---

// --- jekyll (Static site generator) ---
pub type JekyllConfig = CheckerConfig;

// --- slidev (Slidev presentations) ---

// --- encoding (UTF-8 validation) ---
pub type EncodingConfig = CheckerConfig;

// --- duplicate_files (duplicate detection by SHA-256) ---
pub type DuplicateFilesConfig = CheckerConfig;

// --- marp_images (validate image references in Marp presentations) ---

// --- license_header (verify license headers in source files) ---
#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct LicenseHeaderConfig {
    #[serde(default)]
    pub header_lines: Vec<String>,
    #[serde(flatten)]
    pub standard: StandardConfig,
}


impl KnownFields for LicenseHeaderConfig {
    fn known_fields() -> &'static [&'static str] {
        &["args", "dep_inputs", "dep_auto", "batch", "max_jobs", "header_lines"]
    }
    fn checksum_fields() -> &'static [&'static str] {
        &["args", "header_lines"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("args",         "Extra arguments passed to the license header checker"),
            ("header_lines", "Lines of the license header that must appear at the top of each file"),
        ]
    }
}

