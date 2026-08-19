use serde::{Deserialize, Serialize};

use super::{default_true, KnownFields};

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
    /// Extra tools this processor needs beyond `command`.
    ///
    /// `required_tools()` normally reports just `command`, which is right when
    /// the processor invokes the tool directly. It is wrong when `command` is a
    /// wrapper -- a script that shells out to something else -- because the real
    /// tool is then invisible to `tools install` and to version locking, and the
    /// build only fails later, at the point the wrapper runs.
    #[serde(default)]
    pub required_tools: Vec<String>,
    #[serde(default = "default_true")]
    pub batch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_jobs: Option<usize>,
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
            required_tools: Vec::new(),
            batch: true,
            max_jobs: None,
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

/// Lets `SimpleChecker`/`SimpleGenerator` be generic over their config type:
/// every processor config either *is* a `StandardConfig` or wraps one, and
/// this is how the generic code reaches the scan/discover half of it.
impl AsRef<Self> for StandardConfig {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl KnownFields for StandardConfig {
    fn known_fields() -> &'static [&'static str] {
        // Note: "enabled" is universal — declared once in
        // STANDARD_EXTRA_FIELDS and merged in by the validator, not repeated here.
        &["command", "formats", "args", "dep_inputs", "dep_auto", "output_dir", "required_tools", "batch", "max_jobs"]
    }
    fn checksum_fields() -> &'static [&'static str] {
        // formats and output_dir are excluded: format is encoded as a per-product
        // variant in the cache key, and output_dir is encoded in the product's
        // declared outputs path — both would double-count if hashed here.
        &["command", "args"]
    }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] {
        // The "enabled" description lives in SHARED_FIELD_DESCRIPTIONS.
        &[
            ("command",    "Path to the tool executable"),
            ("formats",    "Output formats to generate"),
            ("args",       "Extra arguments passed to the tool"),
            ("output_dir", "Directory where generated output files are written"),
            ("required_tools", "Extra tools needed beyond `command` (for wrapper scripts)"),
        ]
    }
}

/// Simple checker config. No custom fields.
/// Unused `StandardConfig` fields: formats, `output_dir`.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[derive(Default)]
pub struct CheckerConfig {
    #[serde(flatten)]
    pub standard: StandardConfig,
}
impl KnownFields for CheckerConfig {
    fn known_fields() -> &'static [&'static str] { StandardConfig::known_fields() }
    fn checksum_fields() -> &'static [&'static str] { StandardConfig::checksum_fields() }
    fn field_descriptions() -> &'static [(&'static str, &'static str)] { StandardConfig::field_descriptions() }
}

/// Alias for `CheckerConfig` (used by `SimpleChecker`).
pub type CheckerConfigWithCommand = CheckerConfig;

// CreatorConfig lives in src/processors/creators/creator.rs.

// TeraConfig lives in src/processors/generators/tera.rs.

// MakoConfig lives in src/processors/generators/mako.rs.

// Jinja2Config lives in src/processors/generators/jinja2.rs.

// PandocConfig (and PANDOC_PDF_ENGINES) live in src/processors/generators/pandoc.rs.

pub type MarpImagesConfig = CheckerConfig;

// CcSingleFileConfig (and IncludeScanner/CompilerProfile) live in
// src/processors/generators/cc_single_file.rs.

// CcConfig (and the cc.yaml manifest types) live in src/processors/creators/cc.rs.

// LinuxModuleConfig (and the linux-module.yaml manifest types) live in
// src/processors/generators/linux_module.rs.

// ZspellConfig lives in src/processors/checkers/zspell.rs.

// CargoConfig lives in src/processors/creators/cargo.rs.

// MakeConfig lives in src/processors/checkers/make.rs.








pub type JsonSchemaConfig = CheckerConfig;

// TagsConfig lives in src/processors/generators/tags.rs.

// ScriptConfig lives in src/processors/checkers/script.rs.

// GeneratorConfig lives in src/processors/generators/generator.rs.

// ExplicitConfig lives in src/processors/explicit/explicit.rs.

// PipConfig lives in src/processors/creators/pip.rs.

// RequirementsConfig lives in src/processors/generators/requirements.rs.

// SphinxConfig lives in src/processors/creators/sphinx.rs.

// MdbookConfig lives in src/processors/creators/mdbook.rs.

// NpmConfig lives in src/processors/creators/npm.rs.

// MdlConfig lives in src/processors/checkers/mdl.rs.



// MarkdownlintConfig lives in src/processors/checkers/markdownlint.rs.


pub type AsciiConfig = CheckerConfig;

// TermsConfig lives in src/processors/checkers/terms.rs.




// PdflatexConfig lives in src/processors/generators/pdflatex.rs.

// GemConfig lives in src/processors/creators/gem.rs.







pub type IjqConfig = CheckerConfig;

pub type IjsonlintConfig = CheckerConfig;

pub type IyamllintConfig = CheckerConfig;

pub type ItaploConfig = CheckerConfig;

// RustSingleFileConfig lives in src/processors/generators/rust_single_file.rs.

// PdfuniteConfig lives in src/processors/generators/pdfunite.rs.

// IpdfuniteConfig lives in src/processors/generators/ipdfunite.rs.



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
// LicenseHeaderConfig lives in src/processors/checkers/license_header.rs.

